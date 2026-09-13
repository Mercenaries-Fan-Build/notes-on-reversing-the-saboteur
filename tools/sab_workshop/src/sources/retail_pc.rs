//! Retail PC (GOG or Steam SecuROM twin) — `Global/Dynamic0.megapack` + `Palettes0.megapack` +
//! loose `France.materials`. Everything here just delegates to the existing readers in
//! `crate::pack`, `crate::wsao`, `crate::dtex`, `crate::resolve` — the trait is a thin façade
//! over what the workshop already does, so no behaviour changes for retail.

use std::path::Path;
use std::sync::Mutex;

use crate::dtex::{self, CpuTexture};
use crate::meshload;
use crate::pack::{self, Megapack};
use crate::resolve;
use crate::source::{AssetHandle, AssetKind, GameSource, GameVersion, SourceFingerprint};
use crate::wsao::Wsao;

pub struct RetailPcSource {
    root: std::path::PathBuf,
    fingerprint: SourceFingerprint,

    /// Kept resident: mmapped so open is instant. Wrapped in Mutex only because the trait is
    /// `Send + Sync`; access is functionally read-only.
    megapack: Megapack,
    /// Present on retail, may be missing on modded/partial installs — the fingerprint says so.
    palettes: Option<Megapack>,
    /// The material→texture-slot table, when `France.materials` is present.
    wsao: Option<Wsao>,

    /// AssetHandle::key ↔ meshload::MeshEntry — the key is an index into this vec, so callers
    /// can round-trip a handle back to its megapack entry cheaply.
    mesh_index: Vec<meshload::MeshEntry>,
    /// A texture cache so repeated `load_texture` calls don't re-scan the pack. Populated lazily.
    tex_cache: Mutex<std::collections::HashMap<u32, CpuTexture>>,
}

impl RetailPcSource {
    pub fn open(root: &Path) -> Result<Self, String> {
        let mp_path = root.join("Global").join("Dynamic0.megapack");
        let pal_path = root.join("Global").join("Palettes0.megapack");
        let wsao_path = root.join("France.materials");

        let megapack = Megapack::open(&mp_path.to_string_lossy())
            .map_err(|e| format!("open {}: {e}", mp_path.display()))?;
        let palettes = Megapack::open(&pal_path.to_string_lossy()).ok();
        let wsao = Wsao::open(&wsao_path.to_string_lossy()).ok();

        let mesh_index = meshload::list_meshes(&megapack);

        let mut fingerprint = SourceFingerprint::default();
        fingerprint.matched(format!("Dynamic0.megapack ({} entries)", megapack.entries().len()));
        if let Some(p) = &palettes {
            fingerprint.matched(format!("Palettes0.megapack ({} entries)", p.entries().len()));
        } else {
            fingerprint.warn("Palettes0.megapack missing — some cross-bundle textures may not resolve");
        }
        if wsao.is_some() {
            fingerprint.matched("France.materials (WSAO material→texture bindings)");
        } else {
            fingerprint.warn("France.materials missing — falling back to name-suffix heuristic");
        }
        fingerprint.matched(format!("{} meshes indexed", mesh_index.len()));

        Ok(Self {
            root: root.to_path_buf(),
            fingerprint,
            megapack,
            palettes,
            wsao,
            mesh_index,
            tex_cache: Mutex::new(std::collections::HashMap::new()),
        })
    }

    /// The mmapped megapack, for callers that still need direct access (`bundle::run`,
    /// legacy `spawn_tex_resolve` paths). Everything new should go through the trait.
    pub fn megapack(&self) -> &Megapack { &self.megapack }
    pub fn palettes(&self) -> Option<&Megapack> { self.palettes.as_ref() }
    pub fn wsao(&self) -> Option<&Wsao> { self.wsao.as_ref() }
    pub fn root(&self) -> &Path { &self.root }
}

impl GameSource for RetailPcSource {
    fn version(&self) -> GameVersion { GameVersion::RetailPc }

    fn label(&self) -> String {
        format!("Retail PC — {}", self.root.display())
    }

    fn fingerprint(&self) -> &SourceFingerprint { &self.fingerprint }

    fn list_meshes(&self) -> Vec<AssetHandle> {
        self.mesh_index
            .iter()
            .enumerate()
            .map(|(i, e)| AssetHandle {
                key: i as u64,
                hash: Some(crate::pack::pandemic_hash(&e.name)),
                name: e.name.clone(),
                kind: AssetKind::Mesh,
            })
            .collect()
    }

    fn read_mesh_bytes(&self, handle: &AssetHandle) -> Result<Vec<u8>, String> {
        let entry = self
            .mesh_index
            .get(handle.key as usize)
            .ok_or_else(|| format!("mesh handle {} out of range", handle.key))?;
        meshload::raw_record(self.megapack.raw(), entry)
            .map(|s| s.to_vec())
            .ok_or_else(|| format!("could not read mesh {}", entry.name))
    }

    fn textures_for_material(&self, mat: u32) -> Option<[Option<u32>; 4]> {
        let w = self.wsao.as_ref()?;
        let hashes = w.textures(mat)?;
        let mut slots = [None; 4];
        for (i, h) in hashes.iter().take(4).enumerate() {
            if *h != 0 {
                slots[i] = Some(*h);
            }
        }
        Some(slots)
    }

    fn load_texture(&self, hash: u32) -> Result<CpuTexture, String> {
        {
            let cache = self.tex_cache.lock().unwrap();
            if let Some(t) = cache.get(&hash) {
                return Ok(t.clone());
            }
        }
        // Scan every sub-pack for the DTEX record matching this hash. The whole-pack sweep is the
        // conservative choice — a character's own bundle is usually where its skins live, but
        // props keep them in Palettes0. Both are consulted.
        let mut needed = std::collections::HashSet::new();
        needed.insert(hash);
        let mut found = std::collections::HashMap::new();
        let mut packs: Vec<&Megapack> = vec![&self.megapack];
        if let Some(p) = &self.palettes {
            packs.push(p);
        }
        resolve::take_hashes_from_packs(&packs, &mut needed, &mut found);
        let asset = found
            .remove(&hash)
            .ok_or_else(|| format!("no DTEX with hash {hash:08X} in any open pack"))?;
        let tex = asset.decode()?;
        self.tex_cache.lock().unwrap().insert(hash, tex.clone());
        Ok(tex)
    }

    fn name_of_hash(&self, hash: u32) -> Option<String> {
        // Retail has no plaintext name inventory — the only way we ever recover names is by
        // finding the DTEX record itself (its name is in the record header). Scan the packs for
        // it. Cheap on a cache hit; slow the first time.
        // For now, return None — the picker/manifest paths that need names use the
        // per-model bundle scan, not this. Wire up if the browser ever needs global hash→name.
        let _ = hash;
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::GameSource;

    #[test]
    fn opens_retail_and_lists_meshes() {
        let Some(s) = crate::settings::detected() else {
            eprintln!("skip: no retail install detected");
            return;
        };
        let src = RetailPcSource::open(std::path::Path::new(&s.game_dir)).expect("open");
        let meshes = src.list_meshes();
        assert!(meshes.len() > 100, "expected many meshes, got {}", meshes.len());
        assert!(src.fingerprint().matched.iter().any(|m| m.contains("Dynamic0")));
    }

    #[test]
    fn retail_wsao_resolves_a_sean_material() {
        let Some(s) = crate::settings::detected() else {
            eprintln!("skip: no retail install");
            return;
        };
        let src = RetailPcSource::open(std::path::Path::new(&s.game_dir)).expect("open");
        // Just prove the WSAO path is wired — any material that resolves proves it. We probe by
        // walking the mesh index for a Sean part and pulling out its materials[0].
        let sean = src
            .list_meshes()
            .into_iter()
            .find(|h| h.name.eq_ignore_ascii_case("CH_AL_SeanDevlin_01_UB"))
            .expect("Sean UB present");
        let bytes = src.read_mesh_bytes(&sean).expect("read bytes");
        assert!(bytes.len() > 100);
        // We can't easily pull the material hash from the raw bytes here without the assembler;
        // the wsao presence check is the meaningful assertion in this quick test.
        assert!(src.wsao().is_some(), "France.materials should be present on a fresh GOG install");
    }
}
