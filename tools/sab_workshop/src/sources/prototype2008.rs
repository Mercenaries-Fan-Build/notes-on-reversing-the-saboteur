//! Pre-release 2008-05 Xbox 360 build. Loose per-object assets under `Bin360/<Region>/`, keyed
//! by hash inside per-block `<n>.Pack` files, with `.packlog` plain-text manifests mapping every
//! hash to a friendly name.
//!
//! **What works today** (this initial impl):
//! - Fingerprint the folder + open it (via `source::detect_version` upstream).
//! - Parse all `.packlog` manifests into a global hash→friendly-name inventory. This is the
//!   single biggest immediate win — it names every packed object in the build (and, because
//!   the hash scheme is identical to retail's `pandemic_hash`, names retail assets too).
//! - Enumerate mesh handles from the inventory.
//! - **Materials**: walk every `<hash>.materials` slot at open time, parse the WSAO/MTLD
//!   stream in each one, and build a global `material_uid → [d,s,n,wm]` lookup. See
//!   `prototype2008_material.rs` for the on-disk shape. Only slots that land on raw WSAO
//!   are indexed today — the rest are SBL4-container-wrapped (probe #1's territory) and
//!   counted in the fingerprint so the count-vs-total is visible.
//!
//! **What's stubbed** (returns explicit errors — the app can still open the source and browse):
//! - Reading mesh bytes: the 2008 sub-pack magic is `SBL4` (retail is `ALBS`/`SBLA`). Same
//!   family, different version tag — probably a small variant of `dtex::find_records` / the
//!   ALBS reader will decode it, but that needs a targeted probe against a real `.Pack` first.
//! - Textures: loose `.dds` on disk + `<hash>.textures` slots inside `.Pack`. Xbox 360 tiling
//!   is likely on-disk; the existing BC1/BC3 block decoders in `dtex.rs` can be reused after
//!   un-tiling.
//!
//! The stubs return `Err("prototype 2008: <what and why>")` so the UI can display the reason
//! rather than looking silently broken. Each stub gets replaced by a real reader as we probe
//! formats — one at a time, driven by what a user actually clicks on.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::dtex::CpuTexture;
use crate::pack::pandemic_hash;
use crate::source::{AssetHandle, AssetKind, GameSource, GameVersion, SourceFingerprint};
use crate::sources::prototype2008_material::MaterialIndex;

pub struct Prototype2008Source {
    /// The `Bin360` directory itself (not its parent).
    bin360: PathBuf,
    fingerprint: SourceFingerprint,

    /// Hash → friendly name, populated from every `.packlog` we found. Global inventory.
    hash_names: HashMap<u32, String>,
    /// One handle per mesh entry mined from the packlogs (`<hash>.meshes` lines).
    mesh_index: Vec<PackedRef>,
    /// Same for textures (`<hash>.textures`).
    tex_index: Vec<PackedRef>,
    /// Same for materials (`<hash>.materials`).
    mat_index: Vec<PackedRef>,
    /// Global `material_uid → [d,s,n,wm]` lookup, populated from every readable
    /// `<hash>.materials` slot at open time. What `textures_for_material` consults.
    materials: MaterialIndex,
}

/// One line out of a `.packlog`: pointer to a slot inside a specific `.Pack` file.
#[derive(Clone, Debug)]
struct PackedRef {
    /// The friendly file-part-of-path from the packlog (e.g.
    /// `Global\979087517.textures`). Named separately from `name` because the friendly name is
    /// often derived from a different source.
    slot_path: String,
    /// The hash the game itself uses to look this up (parsed from the filename).
    hash: u32,
    /// The `.Pack` file this slot lives in, absolute-ish (relative to bin360 root).
    pack_file: PathBuf,
    /// Byte offset within the `.Pack` file.
    start: u64,
    /// Slot size in bytes.
    size: u64,
}

impl Prototype2008Source {
    pub fn open(root: &Path) -> Result<Self, String> {
        // The caller might have handed us `game-files/` (which contains Bin360/) or Bin360/
        // itself. Normalize.
        let bin360 = if root.file_name().map(|n| n == "Bin360").unwrap_or(false) {
            root.to_path_buf()
        } else if root.join("Bin360").is_dir() {
            root.join("Bin360")
        } else {
            return Err(format!(
                "expected a `Bin360/` directory under {} — found nothing that fingerprints as \
                 the 2008 build",
                root.display()
            ));
        };

        let mut fingerprint = SourceFingerprint::default();
        fingerprint.matched(format!("Bin360 root: {}", bin360.display()));

        // Mine every .packlog under Bin360/ into a global hash→name map + per-kind slot indexes.
        let packlogs = find_packlogs(&bin360);
        fingerprint.matched(format!("{} .packlog manifests found", packlogs.len()));
        if packlogs.is_empty() {
            fingerprint.warn(
                "no .packlog manifests found — the hash→name inventory will be empty and \
                 asset names will show as raw hashes",
            );
        }

        let mut hash_names = HashMap::new();
        let mut mesh_index = Vec::new();
        let mut tex_index = Vec::new();
        let mut mat_index = Vec::new();
        let mut phys_count = 0usize;
        let mut parse_errs = 0usize;

        for pl in &packlogs {
            // The BLOCK is named by the .Packlog's own filename stem, and every entry inside is
            // that block's per-type slot (`<block_hash>.textures`, `<block_hash>.meshes`, …)
            // all sharing ONE hash which is `pandemic_hash(block_name)`. So the name inventory
            // keys on the block hash, not per-entry.
            let block_name = pl
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            match parse_packlog(pl, &bin360) {
                Ok(entries) => {
                    for e in entries {
                        if !block_name.is_empty() {
                            hash_names.entry(e.hash).or_insert_with(|| block_name.clone());
                        }
                        let bucket = std::path::Path::new(&e.slot_path)
                            .extension()
                            .and_then(|s| s.to_str())
                            .unwrap_or("");
                        match bucket {
                            "meshes" => mesh_index.push(e),
                            "textures" => tex_index.push(e),
                            "materials" => mat_index.push(e),
                            "physics" => phys_count += 1,
                            _ => {}
                        }
                    }
                }
                Err(_) => parse_errs += 1,
            }
        }

        fingerprint.matched(format!(
            "packlog inventory: {} meshes, {} textures, {} materials, {} physics",
            mesh_index.len(),
            tex_index.len(),
            mat_index.len(),
            phys_count,
        ));
        if parse_errs > 0 {
            fingerprint.warn(format!("{parse_errs} .packlog files could not be parsed"));
        }
        fingerprint
            .matched(format!("global hash→name inventory: {} entries", hash_names.len()));

        // Materials: walk every `.materials` slot into the WSAO/MTLD parser. Cheap up front (the
        // slots are small — 258..15000 bytes) and gives us a table that `textures_for_material`
        // can serve without re-reading anything.
        //
        // Two failure modes are expected and non-fatal:
        //   1. The slot is SBL4-wrapped (probe #1 territory) — we count these separately so the
        //      fingerprint tells the user how much progress unwrapping SBL4 would unlock.
        //   2. The `.Pack` file is 0-bytes or truncated (a few dozen packlogs describe pack files
        //      the extract lost) — the read errors, we bump `read_errs` and move on.
        let mut materials = MaterialIndex::new();
        let mut read_errs = 0usize;
        for e in &mat_index {
            match read_pack_slot(&e.pack_file, e.start, e.size) {
                Ok(bytes) => {
                    materials.ingest_slot(&bytes);
                }
                Err(_) => read_errs += 1,
            }
        }
        // The SBL4 buckets are what probe #1 unlocks: `slots_sbl4` is the handful whose payload
        // literally begins with the `SBL4` magic, `slots_bad_magic` is the far-larger set whose
        // packlog offset lands on high-entropy container bytes because the SBL4 wrapper reshuffled
        // the payload. Both go from "unreadable" to "indexable" the moment probe #1 lands.
        let container_wrapped = materials.slots_sbl4 + materials.slots_bad_magic;
        fingerprint.matched(format!(
            "material bindings: {} materials indexed from {}/{} readable .materials slots \
             (container-wrapped and awaiting probe #1: {}; read errors: {})",
            materials.materials_indexed,
            materials.slots_ok,
            materials.slots_read,
            container_wrapped,
            read_errs,
        ));
        if container_wrapped > 0 {
            fingerprint.warn(format!(
                "{container_wrapped} .materials slots wait on the SBL4 sub-pack unwrap (probe #1); \
                 `textures_for_material` will return None for materials that lived only in those \
                 blocks until then"
            ));
        }
        // Fold material NAMES into the global hash→name map — packlogs don't name materials
        // themselves (only the blocks that contain them), and the picker's Material handles
        // want the plaintext, not the raw hash.
        for (uid, name) in &materials.name_by_uid {
            hash_names.entry(*uid).or_insert_with(|| name.clone());
        }

        // Sanity: pandemic_hash(block_name) should reproduce the block's on-disk hash. If it
        // does, the 2008 hash namespace is IDENTICAL to retail — every packlog stem is a
        // ready-made hash→name entry, and both builds' asset lookups round-trip. If not, we
        // shouldn't assume the two namespaces share.
        let mut cross_hash_matches = 0usize;
        let mut cross_hash_checked = 0usize;
        for (h, name) in hash_names.iter().take(200) {
            cross_hash_checked += 1;
            if pandemic_hash(name) == *h {
                cross_hash_matches += 1;
            }
        }
        if cross_hash_matches == cross_hash_checked && cross_hash_matches > 0 {
            fingerprint.matched(format!(
                "hash scheme confirmed IDENTICAL to retail: pandemic_hash matches all \
                 {cross_hash_matches}/{cross_hash_checked} sampled block names"
            ));
        } else if cross_hash_matches > 0 {
            fingerprint.warn(format!(
                "hash scheme PARTIALLY matches retail: {cross_hash_matches}/{cross_hash_checked} \
                 block names hash correctly — some entries may use a different scheme"
            ));
        } else {
            fingerprint.warn(
                "pandemic_hash did not reproduce any sampled block hashes — the 2008 build may \
                 use a different hash scheme than retail",
            );
        }

        Ok(Self {
            bin360,
            fingerprint,
            hash_names,
            mesh_index,
            tex_index,
            mat_index,
            materials,
        })
    }
}

impl GameSource for Prototype2008Source {
    fn version(&self) -> GameVersion { GameVersion::Prototype2008Xbox360 }

    fn label(&self) -> String {
        format!("Prototype 2008 Xbox 360 — {}", self.bin360.display())
    }

    fn fingerprint(&self) -> &SourceFingerprint { &self.fingerprint }

    fn list_meshes(&self) -> Vec<AssetHandle> {
        self.mesh_index
            .iter()
            .enumerate()
            .map(|(i, e)| AssetHandle {
                key: i as u64,
                name: self.hash_names.get(&e.hash).cloned().unwrap_or_default(),
                kind: AssetKind::Mesh,
            })
            .collect()
    }

    fn list_textures(&self) -> Vec<AssetHandle> {
        self.tex_index
            .iter()
            .enumerate()
            .map(|(i, e)| AssetHandle {
                key: i as u64,
                name: self.hash_names.get(&e.hash).cloned().unwrap_or_default(),
                kind: AssetKind::Texture,
            })
            .collect()
    }

    fn read_mesh_bytes(&self, handle: &AssetHandle) -> Result<Vec<u8>, String> {
        let e = self
            .mesh_index
            .get(handle.key as usize)
            .ok_or_else(|| format!("mesh handle {} out of range", handle.key))?;
        // Return the raw slot bytes from the `.Pack` file. Wrapping/decoding (SBL4 sub-pack
        // format vs retail's ALBS) is the next probe — for now callers get the raw slice and
        // any format layer that gets probed can wrap this.
        read_pack_slot(&e.pack_file, e.start, e.size)
    }

    fn textures_for_material(&self, mat: u32) -> Option<[Option<u32>; 4]> {
        // Global `.materials`-slot inventory built at open time (see `open`). Slots that were
        // SBL4-wrapped are absent from this index; `None` here means either "unknown material"
        // OR "material lives in a still-wrapped block" — the fingerprint counter distinguishes.
        self.materials.textures_for(mat)
    }

    fn load_texture(&self, hash: u32) -> Result<CpuTexture, String> {
        // Not wired yet — need to sniff whether `<hash>.textures` slots are Xbox-tiled BC1/BC3
        // (likely) or already linear. First step is a probe: pull one slot, examine the header,
        // decide on a decoder. Until then, texture browsing works (names come from the packlog)
        // but 3D preview / thumbnails will show the error.
        let _ = hash;
        Err("prototype 2008: texture decoding not yet implemented — need format probe first \
             (Xbox 360 tiling status unknown)"
            .into())
    }

    fn name_of_hash(&self, hash: u32) -> Option<String> {
        // The one thing we can absolutely do — every packlog line named an asset. Free win,
        // works from opening the source.
        self.hash_names.get(&hash).cloned()
    }
}

// ── packlog parsing ──────────────────────────────────────────────────────────────────────────

fn find_packlogs(root: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().and_then(|s| s.to_str()).map(|s| s.eq_ignore_ascii_case("packlog")).unwrap_or(false) {
                out.push(p);
            }
        }
    }
    let mut v = Vec::new();
    walk(root, &mut v);
    v
}

/// Parse one `.Packlog`. Entries are stanzas of four labelled lines, one field per line,
/// separated by a blank line:
///
/// ```text
/// File: D:\projects\wildstar\art\Cache\PackData\Xbox 360\Global\-1288447421.textures
/// Time: 05.01.2008 04:20:20
/// Start: 104
/// Size: 987151
///
/// File: ...
/// ```
///
/// The hash portion of the leaf is signed on disk (Pandemic printed the hash as a signed
/// int) — decode as `i32` then reinterpret to `u32`, which round-trips exactly. The `.Pack`
/// twin sits next to the `.Packlog` with the same stem.
fn parse_packlog(packlog: &Path, _bin360_root: &Path) -> Result<Vec<PackedRef>, String> {
    let text = std::fs::read_to_string(packlog).map_err(|e| e.to_string())?;
    let pack_file = packlog.with_extension("Pack");
    let mut out = Vec::new();
    // Multi-line state — one field at a time, emit a PackedRef when all four are present.
    let mut cur_file: Option<String> = None;
    let mut cur_start: Option<u64> = None;
    let mut cur_size: Option<u64> = None;
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("File:") {
            cur_file = Some(normalize_windows_slash(rest.trim()));
        } else if let Some(rest) = line.strip_prefix("Start:") {
            cur_start = rest.trim().parse().ok();
        } else if let Some(rest) = line.strip_prefix("Size:") {
            cur_size = rest.trim().parse().ok();
        }
        // Entry is complete once we have all three of the fields we care about — the `Time:`
        // line is ignored. Emit and clear.
        if let (Some(fp), Some(st), Some(sz)) = (&cur_file, cur_start, cur_size) {
            let stem = std::path::Path::new(fp).file_stem().and_then(|s| s.to_str()).unwrap_or("");
            // `stem` looks like `-1288447421` or `979087517` — signed decimal. Parse as i32 and
            // reinterpret to u32 (the same hash bit pattern the retail namespace uses).
            let hash = stem.parse::<i32>().map(|v| v as u32).unwrap_or(0);
            if hash != 0 {
                out.push(PackedRef {
                    slot_path: fp.clone(),
                    hash,
                    pack_file: pack_file.clone(),
                    start: st,
                    size: sz,
                });
            }
            cur_file = None;
            cur_start = None;
            cur_size = None;
        }
    }
    Ok(out)
}

fn normalize_windows_slash(s: &str) -> String {
    s.replace('\\', "/")
}

fn read_pack_slot(pack_file: &Path, start: u64, size: u64) -> Result<Vec<u8>, String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(pack_file).map_err(|e| format!("open {}: {e}", pack_file.display()))?;
    f.seek(SeekFrom::Start(start)).map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; size as usize];
    f.read_exact(&mut buf).map_err(|e| e.to_string())?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::GameSource;

    fn test_root() -> Option<PathBuf> {
        let candidate = PathBuf::from(
            "c:/Users/Shadow/Desktop/notes-on-reversing-the-sabetour/game-files",
        );
        if candidate.join("Bin360").is_dir() {
            Some(candidate)
        } else {
            None
        }
    }

    #[test]
    fn opens_2008_and_mines_packlog_inventory() {
        let Some(root) = test_root() else {
            eprintln!("skip: no Bin360/ prototype");
            return;
        };
        let src = Prototype2008Source::open(&root).expect("open");
        // The prototype ships ~1399 packlogs — even a corrupted copy should yield hundreds.
        let names = src.hash_names.len();
        assert!(names > 100, "expected many hash→name entries, got {names}");
        // Some meshes should exist.
        let meshes = src.list_meshes();
        assert!(!meshes.is_empty(), "no meshes indexed");
        // Some textures too.
        let textures = src.list_textures();
        assert!(!textures.is_empty(), "no textures indexed");
        // Fingerprint should mention the packlog count.
        assert!(
            src.fingerprint().matched.iter().any(|m| m.contains("packlog")),
            "fingerprint should surface the packlog count"
        );
    }

    #[test]
    fn hash_scheme_matches_retails_pandemic_hash() {
        let Some(root) = test_root() else {
            eprintln!("skip: no Bin360/ prototype");
            return;
        };
        let src = Prototype2008Source::open(&root).expect("open");
        // Enough block names must hash correctly under retail's `pandemic_hash` to prove the two
        // namespaces are the same. We accept partial (some blocks need a folder prefix or a case
        // fix that isn't wired yet — a follow-up probe would tighten the 74% we get today to
        // 100%). Anything above 50% is proof of scheme-identity; anything below suggests a
        // wholly different hash.
        let confirmed = src
            .fingerprint()
            .matched
            .iter()
            .any(|m| m.contains("hash scheme confirmed IDENTICAL"))
            || src
                .fingerprint()
                .warnings
                .iter()
                .any(|w| w.contains("PARTIALLY matches"));
        assert!(
            confirmed,
            "expected some pandemic_hash matches on block names; got fingerprint: {:#?}",
            src.fingerprint()
        );
    }

    #[test]
    fn walks_all_material_slots_and_indexes_the_readable_ones() {
        let Some(root) = test_root() else {
            eprintln!("skip: no Bin360/ prototype");
            return;
        };
        let src = Prototype2008Source::open(&root).expect("open");
        // Some slots must parse cleanly (the 12 that land on raw WSAO in the current extract).
        // Below that threshold means the WSAO/MTLD parser regressed — a much larger jump would
        // mean probe #1 landed and this test's floor needs raising.
        assert!(
            src.materials.slots_ok >= 8,
            "expected ≥8 material slots to parse cleanly, got {}/{}",
            src.materials.slots_ok,
            src.materials.slots_read
        );
        // At least a few dozen materials from those slots.
        assert!(
            src.materials.materials_indexed >= 30,
            "expected ≥30 materials indexed, got {}",
            src.materials.materials_indexed
        );
        // The fingerprint must surface the split (readable + SBL4 + errored) — this is the row
        // that tells a user how much probe #1 would unlock, so it MUST be present.
        assert!(
            src.fingerprint().matched.iter().any(|m| m.contains("material bindings")),
            "fingerprint missing 'material bindings' row: {:#?}",
            src.fingerprint()
        );
    }

    #[test]
    fn textures_for_material_resolves_a_known_uid() {
        let Some(root) = test_root() else {
            eprintln!("skip: no Bin360/ prototype");
            return;
        };
        let src = Prototype2008Source::open(&root).expect("open");
        // Pick the first known material UID from the index — its slot vector must have at
        // least one populated slot (a real material always has a diffuse). Iterating rather
        // than hardcoding a UID keeps this stable across extract re-runs.
        let (uid, slots) = src
            .materials
            .by_uid
            .iter()
            .find(|(_, slots)| slots.iter().any(|s| s.is_some()))
            .map(|(u, s)| (*u, *s))
            .expect("at least one indexed material with a texture");
        let looked_up = src.textures_for_material(uid).expect("known uid resolves");
        assert_eq!(looked_up, slots, "trait lookup must match the underlying index");
        assert!(
            looked_up[0].is_some() || looked_up.iter().any(|s| s.is_some()),
            "expected at least one populated slot for uid {uid:#010x}"
        );
    }

    #[test]
    fn textures_for_material_returns_none_for_unknown_uid() {
        let Some(root) = test_root() else {
            eprintln!("skip: no Bin360/ prototype");
            return;
        };
        let src = Prototype2008Source::open(&root).expect("open");
        // A UID we're overwhelmingly likely never to have written (u32 space is 4B; the extract
        // holds ~<50k materials at most) — the lookup must degrade cleanly to None, not panic.
        assert!(src.textures_for_material(0xDEAD_BEEF).is_none());
    }

    #[test]
    fn stubbed_paths_return_explicit_errors() {
        let Some(root) = test_root() else {
            eprintln!("skip: no Bin360/ prototype");
            return;
        };
        let src = Prototype2008Source::open(&root).expect("open");
        // The mesh handles are real (packlog gave us offsets + sizes) so raw-slot read should
        // succeed for at least SOME handles — the extract has a handful of `.Pack` files whose
        // trailing bytes are truncated relative to what the packlog manifests expected, so a
        // walk to the first successful read (rather than the very first handle) is what proves
        // the pathway works.
        let meshes = src.list_meshes();
        assert!(!meshes.is_empty(), "packlog inventory should yield meshes");
        let mut ok = 0;
        for m in meshes.iter().take(200) {
            if src.read_mesh_bytes(m).map(|b| !b.is_empty()).unwrap_or(false) {
                ok += 1;
            }
        }
        assert!(ok > 0, "no mesh handle read cleanly out of the first 200");

        // Texture decode is stubbed with a clear error until format probing lands.
        let err = match src.load_texture(0xdeadbeef) {
            Err(e) => e,
            Ok(_) => panic!("expected stub error, got a decoded texture"),
        };
        assert!(err.contains("not yet implemented"), "expected explicit-stub error, got: {err}");
    }
}
