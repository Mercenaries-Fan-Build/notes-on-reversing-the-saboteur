//! Retail PC (GOG or Steam SecuROM twin) as a [`GameSource`]: meshes from `Global/Dynamic0.megapack`,
//! textures bound through `France.materials` (WSAO) to DTEX records in `Dynamic0` and
//! `Global/Palettes0.megapack`, templates from `France/loosefiles_BinPC.pack` (`AULB`), clips from
//! `Animations.pack`, game text from `Cinematics/Dialog/<Language>/GameText.dlg`.
//!
//! Meshes: one item per name. `Dynamic0` stores 6807 mesh records under 1559 names, and every record
//! of a name is byte-identical to the others (measured over all 988 repeated names), so a template's
//! reference to a name has one answer wherever it is loaded from.
//!
//! Clips: the `ANIM` block stores, per track, either an index into the biped rig every human clip
//! is authored on, or (exotic skeletons) a bone name hash. A clip's values are indices when every
//! bound one is below the biped's bone count; they are turned into the biped's bone hashes so every
//! clip binds to a loaded rig by hash. The biped is the rig of the build's player, assembled from its
//! `Player` template.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use sab_formats::gametemplates::GameTemplates;
use sab_formats::pandemic_hash;

use crate::anim_index::{self, ClipInfo, UNBOUND};
use crate::clip::AnimClip;
use crate::dtex::{self, CpuTexture};
use crate::formats::SubMesh;
use crate::havok::{self, Packfile, QsTransform, SplineAnim};
use crate::meshload::{self, LoadedMesh, MeshEntry};
use crate::pack::Megapack;
use crate::resolve::{self, ModelTex, Prov, Role, SubmeshBinding, TexAsset};
use crate::source::{ClipRow, GameSource, GameVersion, IconRow, MeshFind, MeshItem, Overrides, PartRange, RawRecord, SourceFingerprint};
use crate::wsao::Wsao;

/// The game-text languages, as `Cinematics/Dialog/` names them.
const LANGUAGES: [&str; 7] = ["English", "French", "German", "Italian", "Polish", "Russian", "Random"];

/// `Animations.pack`: the bytes and the offset of each `hkaSplineCompressedAnimation`.
struct PackData {
    file: Vec<u8>,
    blob_off: usize,
    hk_size: usize,
    /// Data-relative offset of the N-th spline animation.
    scas: Vec<usize>,
}

impl PackData {
    fn load(path: &str) -> Result<PackData, String> {
        let file = std::fs::read(path).map_err(|e| format!("read {path}: {e}"))?;
        let ap = havok::parse_ap0l(&file).map_err(|e| format!("{path}: {e}"))?;
        let blob = &file[ap.blob_off..ap.blob_off + ap.hk_size];
        let pk = Packfile::parse(blob).map_err(|e| format!("{path}: {e}"))?;
        let scas = pk.vfixups.iter().filter(|(_, c)| c == "hkaSplineCompressedAnimation").map(|(s, _)| *s).collect();
        Ok(PackData { blob_off: ap.blob_off, hk_size: ap.hk_size, scas, file })
    }

    fn blob(&self) -> &[u8] {
        &self.file[self.blob_off..self.blob_off + self.hk_size]
    }
}

/// A retail clip: its spline animation, sampled from the shared pack bytes.
pub struct RetailClip {
    anim: SplineAnim,
    pack: Arc<PackData>,
    duration: f32,
    bones: Vec<u32>,
}

impl RetailClip {
    /// The spline decoder and the Havok blob it reads, for codec diagnostics.
    pub fn spline(&self) -> (&SplineAnim, &[u8]) {
        (&self.anim, self.pack.blob())
    }
}

impl AnimClip for RetailClip {
    fn duration(&self) -> f32 {
        self.duration
    }

    fn num_transform_tracks(&self) -> usize {
        self.anim.num_transform_tracks
    }

    fn num_frames(&self) -> usize {
        self.anim.num_frames.max(1)
    }

    fn sample_all(&self, time: f32) -> Vec<QsTransform> {
        self.anim.sample_at(self.pack.blob(), time)
    }

    fn bind_hashes(&self) -> &[u32] {
        &self.bones
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

pub struct RetailPcSource {
    root: PathBuf,
    fingerprint: SourceFingerprint,
    megapack: Megapack,
    palettes: Megapack,
    wsao: Wsao,
    /// `--dtexdir`: loose `<hash>.dtex` files consulted before the packs.
    dtex_dir: Option<String>,
    /// Item → its first record.
    entries: Vec<MeshEntry>,
    items: Vec<MeshItem>,
    /// Name hash → item.
    by_hash: HashMap<u32, usize>,
    /// Name hash → number of records of that name.
    copies: HashMap<u32, usize>,
    templates: GameTemplates,
    catalog: Vec<ClipInfo>,
    clips: Vec<ClipRow>,
    pack: Arc<PackData>,
}

/// Find the main object DB: the `AULB` blob in `France/loosefiles_BinPC.pack` with the most templates.
pub fn load_gametemplates(root: &Path) -> Result<GameTemplates, String> {
    let path = root.join("France").join("loosefiles_BinPC.pack");
    let data = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut best: Option<GameTemplates> = None;
    let mut from = 0usize;
    while let Some(rel) = data[from..].windows(4).position(|w| w == b"AULB") {
        let off = from + rel;
        if let Ok((gt, _)) = GameTemplates::parse(&data[off..]) {
            if best.as_ref().map_or(true, |b| gt.templates().count() > b.templates().count()) {
                best = Some(gt);
            }
        }
        from = off + 4;
    }
    best.ok_or_else(|| format!("{}: no AULB template table parses", path.display()))
}

impl RetailPcSource {
    pub fn open(root: &Path, overrides: &Overrides) -> Result<Self, String> {
        let at = |rel: &str| root.join(rel).to_string_lossy().into_owned();
        let mp_path = overrides.megapack.clone().unwrap_or_else(|| at("Global/Dynamic0.megapack"));
        let pal_path = overrides.palettes.clone().unwrap_or_else(|| at("Global/Palettes0.megapack"));
        let wsao_path = overrides.wsao.clone().unwrap_or_else(|| at("France.materials"));
        let anim_path = overrides.anim_pack.clone().unwrap_or_else(|| at("Animations.pack"));

        let megapack = Megapack::open(&mp_path).map_err(|e| format!("{mp_path}: {e}"))?;
        let palettes = Megapack::open(&pal_path).map_err(|e| format!("{pal_path}: {e}"))?;
        let wsao = Wsao::open(&wsao_path)?;
        let templates = load_gametemplates(root)?;

        let mut entries = Vec::new();
        let mut items = Vec::new();
        let mut by_hash = HashMap::new();
        let mut copies: HashMap<u32, usize> = HashMap::new();
        let records = meshload::list_meshes(&megapack);
        for e in &records {
            let h = pandemic_hash(&e.name);
            *copies.entry(h).or_default() += 1;
            if by_hash.contains_key(&h) {
                continue;
            }
            by_hash.insert(h, items.len());
            let bundle = megapack.entry_containing(e.file_off).map(|b| b.index).ok_or_else(|| format!("{}: in no bundle", e.name))?;
            items.push(MeshItem { name: e.name.clone(), location: format!("Dynamic0 bundle {bundle:08X} @ 0x{:X}", e.file_off), world: false });
            entries.push(e.clone());
        }

        let catalog = match &overrides.anim_index {
            Some(p) => anim_index::load(p)?,
            None => anim_index::from_pack(&anim_path)?,
        };
        let pack = Arc::new(PackData::load(&anim_path)?);
        if let Some(c) = catalog.iter().find(|c| c.index >= pack.scas.len()) {
            return Err(format!("clip '{}' is spline animation {} of {} in {anim_path}", c.name, c.index, pack.scas.len()));
        }

        let mut fingerprint = SourceFingerprint::default();
        fingerprint.matched(format!("Dynamic0.megapack: {} bundles, {} mesh records under {} names", megapack.entries().len(), records.len(), items.len()));
        fingerprint.matched(format!("Palettes0.megapack: {} bundles", palettes.entries().len()));
        fingerprint.matched("France.materials (WSAO)");
        fingerprint.matched(format!("GameTemplates: {} templates", templates.templates().count()));
        fingerprint.matched(format!("Animations.pack: {} clips", catalog.len()));

        let mut src = RetailPcSource {
            root: root.to_path_buf(),
            fingerprint,
            megapack,
            palettes,
            wsao,
            dtex_dir: overrides.dtex_dir.clone(),
            entries,
            items,
            by_hash,
            copies,
            templates,
            catalog,
            clips: Vec::new(),
            pack,
        };
        src.clips = src.clip_rows()?;
        Ok(src)
    }

    /// The catalog with every track's bone as a name hash.
    fn clip_rows(&self) -> Result<Vec<ClipRow>, String> {
        let assets = crate::assets::build(self);
        let player = crate::assets::player(&self.templates, &assets)?;
        let asm = assets[player].assembly();
        let (rig, skipped) = meshload::assemble_with(&asm.picks, |i| self.load_part(i), |i| self.items[i].name.clone())?;
        if !skipped.is_empty() {
            return Err(format!("the player {} assembles without {}", assets[player].name, skipped.join("; ")));
        }
        let biped = rig.bone_hashes;
        Ok(self
            .catalog
            .iter()
            .map(|c| {
                let indices = c.bones.iter().all(|&b| b == UNBOUND || (b as usize) < biped.len());
                let bones = c
                    .bones
                    .iter()
                    .map(|&b| match b {
                        UNBOUND => 0,
                        b if indices => biped[b as usize],
                        b => b,
                    })
                    .collect();
                ClipRow { name: c.name.clone(), duration: c.duration, bones }
            })
            .collect())
    }

    fn entry(&self, mesh: usize) -> Result<&MeshEntry, String> {
        self.entries.get(mesh).ok_or_else(|| format!("mesh {mesh} out of range"))
    }

    /// The texture records named by `needed`, from loose `--dtexdir` files, the parts' own bundles,
    /// then every bundle of both packs.
    fn find_textures(&self, own: &[&[u8]], mut needed: HashSet<u32>) -> Result<HashMap<u32, TexAsset>, String> {
        let mut found: HashMap<u32, TexAsset> = HashMap::new();
        if let Some(dir) = &self.dtex_dir {
            for h in needed.clone() {
                let path = format!("{dir}/{h:08X}.dtex");
                let Ok(bytes) = std::fs::read(&path) else { continue };
                let rec = if bytes.starts_with(b"DTEX") { bytes[4..].to_vec() } else { bytes };
                found.insert(h, TexAsset::dtex(rec, Role::Other).map_err(|e| format!("{path}: {e}"))?);
                needed.remove(&h);
            }
        }
        resolve::take_hashes_from_slices(own, &mut needed, &mut found)?;
        resolve::take_hashes_from_packs(&[&self.megapack, &self.palettes], &mut needed, &mut found)?;
        Ok(found)
    }
}

impl GameSource for RetailPcSource {
    fn version(&self) -> GameVersion {
        GameVersion::RetailPc
    }

    fn label(&self) -> String {
        format!("Retail PC — {}", self.root.display())
    }

    fn fingerprint(&self) -> &SourceFingerprint {
        &self.fingerprint
    }

    fn templates(&self) -> &GameTemplates {
        &self.templates
    }

    fn meshes(&self) -> &[MeshItem] {
        &self.items
    }

    fn find_mesh(&self, hash: u32, _template: &str) -> MeshFind {
        match self.by_hash.get(&hash) {
            Some(&i) => MeshFind::Found(i),
            None => MeshFind::Absent,
        }
    }

    fn load_part(&self, mesh: usize) -> Result<LoadedMesh, String> {
        meshload::load(self.megapack.raw(), self.entry(mesh)?).map_err(|e| format!("{}: {e}", self.items[mesh].name))
    }

    fn raw_part(&self, mesh: usize) -> Result<RawRecord, String> {
        let e = self.entry(mesh)?;
        let bytes = meshload::raw_record(self.megapack.raw(), e).ok_or_else(|| format!("{}: record past the end of the megapack", e.name))?;
        Ok(RawRecord { file: format!("{}.msha", e.name), bytes: bytes.to_vec(), location: self.items[mesh].location.clone() })
    }

    fn resolve_textures(&self, parts: &[PartRange], submeshes: &[SubMesh], _uvless: &[(u32, u32)]) -> Result<ModelTex, String> {
        // materials[0] → the WSMA texture slice, in [diffuse, spec, normal, mask] order (`wsao.rs`).
        // A zero hash is an empty slot.
        let want: Vec<[Option<u32>; SubmeshBinding::NUM_SLOTS]> = submeshes
            .iter()
            .map(|s| {
                let mut slots = [None; SubmeshBinding::NUM_SLOTS];
                if let Some(hashes) = s.materials.first().and_then(|&m| self.wsao.textures(m)) {
                    for (i, &h) in hashes.iter().take(SubmeshBinding::NUM_SLOTS).enumerate() {
                        if h != 0 {
                            slots[i] = Some(h);
                        }
                    }
                }
                slots
            })
            .collect();
        let needed: HashSet<u32> = want.iter().flat_map(|s| s.iter().copied()).flatten().collect();
        let mut own: Vec<&[u8]> = Vec::new();
        for p in parts {
            let e = self.entry(p.mesh)?;
            let b = self.megapack.entry_containing(e.file_off).ok_or_else(|| format!("{}: in no bundle", e.name))?;
            own.push(self.megapack.slice(&b));
        }
        let found = self.find_textures(&own, needed)?;

        // A texture's role is the slot the material binds it in; one bound in slots of two roles is
        // `Other`.
        let mut roles: HashMap<u32, Role> = HashMap::new();
        for slots in &want {
            for (s, h) in slots.iter().enumerate() {
                let Some(h) = h else { continue };
                let r = SubmeshBinding::SLOT_ROLES[s];
                roles.entry(*h).and_modify(|x| if *x != r { *x = Role::Other }).or_insert(r);
            }
        }
        let mut assets: Vec<TexAsset> = Vec::new();
        let mut index: HashMap<u32, usize> = HashMap::new();
        let mut hashes: Vec<u32> = found.keys().copied().collect();
        hashes.sort_unstable();
        let mut found = found;
        for h in hashes {
            let mut a = found.remove(&h).expect("listed");
            a.role = roles[&h];
            index.insert(h, assets.len());
            assets.push(a);
        }
        let mut submesh_tex = vec![SubmeshBinding::default(); submeshes.len()];
        let mut submesh_prov = vec![Prov::Unresolved; submeshes.len()];
        for (k, slots) in want.iter().enumerate() {
            for (s, h) in slots.iter().enumerate() {
                submesh_tex[k].slots[s] = h.and_then(|h| index.get(&h).copied());
            }
            if submesh_tex[k].is_bound() {
                submesh_prov[k] = Prov::Bound;
            }
        }
        let mut cache: HashMap<usize, CpuTexture> = HashMap::new();
        let mut decoded = Vec::new();
        for (i, b) in submesh_tex.iter().enumerate() {
            for (slot, ai) in b.iter_bound() {
                let tex = match cache.get(&ai) {
                    Some(t) => t.clone(),
                    None => {
                        let t = assets[ai].decode().map_err(|e| format!("texture {}: {e}", assets[ai].name))?;
                        cache.insert(ai, t.clone());
                        t
                    }
                };
                decoded.push((i, slot, tex));
            }
        }
        Ok(ModelTex { assets, submesh_tex, submesh_prov, decoded })
    }

    fn describe_mesh(&self, mesh: usize) -> Vec<(String, String)> {
        let Ok(e) = self.entry(mesh) else { return Vec::new() };
        let h = pandemic_hash(&e.name);
        let stored = meshload::raw_record(self.megapack.raw(), e).map(|r| r.len()).unwrap_or(0);
        vec![
            ("location".into(), self.items[mesh].location.clone()),
            ("hash".into(), format!("{h:08X}")),
            ("stored".into(), format!("{stored} bytes (MSHA header + two zlib blobs)")),
            ("records of this name".into(), format!("{} (identical)", self.copies.get(&h).copied().unwrap_or(0))),
        ]
    }

    fn world_blocks(&self) -> &[(String, Vec<usize>)] {
        &[]
    }

    fn clips(&self) -> &[ClipRow] {
        &self.clips
    }

    fn load_clip(&self, index: usize) -> Result<Box<dyn AnimClip>, String> {
        let c = self.catalog.get(index).ok_or_else(|| format!("clip {index} out of range"))?;
        let pk = Packfile::parse(self.pack.blob())?;
        let anim = havok::read_spline_anim(&pk, self.pack.scas[c.index]);
        let duration = if c.duration > 0.0 { c.duration } else { anim.duration };
        Ok(Box::new(RetailClip { anim, pack: self.pack.clone(), duration, bones: self.clips[index].bones.clone() }))
    }

    fn game_text_files(&self) -> Vec<(String, PathBuf)> {
        LANGUAGES
            .iter()
            .map(|l| (l.to_string(), self.root.join("Cinematics").join("Dialog").join(l).join("GameText.dlg")))
            .filter(|(_, p)| p.is_file())
            .collect()
    }

    fn icon_textures(&self, progress: &dyn Fn(usize, usize)) -> Result<Vec<IconRow>, String> {
        let total = self.palettes.entries().len();
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        for (ei, e) in self.palettes.entries().iter().enumerate() {
            for (off, len, name) in dtex::find_records(self.palettes.slice(e)) {
                let h = pandemic_hash(&name);
                if seen.insert(h) {
                    out.push(IconRow { name, hash: h, key: (ei, off, len) });
                }
            }
            if ei % 16 == 0 {
                progress(ei, total);
            }
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        progress(total, total);
        Ok(out)
    }

    fn decode_icon(&self, row: &IconRow, preview: Option<u32>) -> Result<CpuTexture, String> {
        let (ei, off, len) = row.key;
        let e = self.palettes.entries().get(ei).ok_or_else(|| format!("{}: bundle {ei} out of range", row.name))?;
        let sub = self.palettes.slice(e);
        let rec = sub.get(off..off + len).ok_or_else(|| format!("{}: record past its bundle", row.name))?;
        match preview {
            Some(min) => dtex::decode_preview(rec, min),
            None => dtex::decode(rec),
        }
        .map_err(|e| format!("{}: {e}", row.name))
    }

    fn publishable(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    /// The one-item-per-name rule: every record of a repeated mesh name in Dynamic0 is byte-identical
    /// (988 names repeat; none differ).
    #[cfg(feature = "retail")]
    #[test]
    fn repeated_mesh_names_are_identical_copies() {
        let src = crate::testenv::retail();
        let records = super::meshload::list_meshes(&src.megapack);
        let mut first: std::collections::HashMap<String, &[u8]> = std::collections::HashMap::new();
        let mut repeated = std::collections::HashSet::new();
        for e in &records {
            let raw = super::meshload::raw_record(src.megapack.raw(), e).expect("record in the megapack");
            match first.get(&e.name) {
                Some(prev) => {
                    assert!(*prev == raw, "two different records named {}", e.name);
                    repeated.insert(e.name.clone());
                }
                None => {
                    first.insert(e.name.clone(), raw);
                }
            }
        }
        assert_eq!((records.len(), first.len(), repeated.len()), (6807, 1559, 988));
    }
}
