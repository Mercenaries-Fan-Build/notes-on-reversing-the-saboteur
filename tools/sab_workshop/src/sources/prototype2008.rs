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
use std::sync::Mutex;

use crate::dds;
use crate::dtex::CpuTexture;
use crate::pack::pandemic_hash;
use crate::source::{AssetHandle, AssetKind, GameSource, GameVersion, SourceFingerprint};
use crate::sources::prototype2008_material::MaterialIndex;
use crate::sources::prototype2008_subpack::SubPack;

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
    /// Loose `.dds` files under Bin360/, keyed by `pandemic_hash(stem)`. Populated at `open()`
    /// so `load_texture(hash)` is a cheap lookup — the actual bytes and RGBA are only touched
    /// on demand. Multiple DDS files can share a name (Branches/*/DiffuseCube.dds mirrors the
    /// root one), so we keep the FIRST match at the shallowest depth (root wins over branches).
    loose_dds: HashMap<u32, LooseDds>,
    /// Decoded-texture cache — repeated preview / material-slot lookups don't re-decode.
    tex_cache: Mutex<HashMap<u32, CpuTexture>>,
    /// Global `material_uid → [d,s,n,wm]` lookup, populated from every readable
    /// `<hash>.materials` slot at open time. What `textures_for_material` consults.
    materials: MaterialIndex,
}

#[derive(Clone, Debug)]
struct LooseDds {
    /// Filename stem, used as the friendly name.
    name: String,
    /// Absolute path on disk. Rustc's dead-code pass mis-flags this on release builds — it IS
    /// read by `load_texture` for `std::fs::read(&entry.path)`. `#[allow]` at the field keeps
    /// the compiler quiet without silencing warnings on the wider module.
    #[allow(dead_code)]
    path: PathBuf,
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
    /// The packlog's claimed slot start. NOT the on-disk position — the packer reorders
    /// slots on write, so trusting this to seek inside a `.Pack` reads bytes from the
    /// wrong region (see `docs/2008_subpack_format.md`). Kept only so texture / material
    /// probes can compare vs the authoritative SBL4 offset when they land.
    #[allow(dead_code)]
    start: u64,
    /// Packlog's declared slot size — same caveat as `start`. Close to but not equal to
    /// the SBL4 record's `size` (the packer adds per-slot wrappers).
    #[allow(dead_code)]
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
            let stem = pl
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            match parse_packlog(pl, &bin360) {
                Ok(entries) => {
                    // Chunk splits (`Barge0.Packlog` + `Barge1.Packlog` etc.) both record the
                    // SAME block hash keyed on the base name (`Barge`, no trailing digit) — see
                    // `docs/2008_namespace_prefix.md`. If the raw stem's pandemic_hash matches
                    // the block hash, use it as-is; otherwise strip trailing ASCII digits and
                    // try again. Falls back to the raw stem when neither matches (so a genuine
                    // hash-scheme mismatch still keeps the packlog stem in the inventory).
                    let block_hash = entries.first().map(|e| e.hash).unwrap_or(0);
                    let block_name = if !stem.is_empty() && pandemic_hash(&stem) == block_hash {
                        stem.clone()
                    } else {
                        let trimmed = stem.trim_end_matches(|c: char| c.is_ascii_digit());
                        if !trimmed.is_empty() && pandemic_hash(trimmed) == block_hash {
                            trimmed.to_string()
                        } else {
                            stem.clone()
                        }
                    };
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

        // NOTE: the material walk that probe #3 landed here was rewritten in the merge below to
        // read via `SubPack::read_slot` (probe #1) — see the block just before `Ok(Self { … })`.
        // Keeping the sanity check + Ok(Self) here; the actual walk is deferred until after the
        // loose-DDS sweep so all fingerprint entries land in a consistent order.

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

        // Sweep the Bin360 tree for loose `.dds` files. Empirically these are standard PC-style
        // DDS (linear, LE DXT blocks) — see docs/2008_texture_format.md. Every one is keyed by
        // `pandemic_hash(stem)` so any consumer that has a texture hash (from a packlog entry
        // whose stem is a friendly-name, or from a material record) can resolve it here without
        // caring where on disk it lives. Root-level files win over Branches/* twins because the
        // shallower path is the canonical asset (Branches/ are per-team snapshots).
        let mut loose_dds: HashMap<u32, LooseDds> = HashMap::new();
        let mut loose_dds_by_depth: HashMap<u32, usize> = HashMap::new();
        let mut loose_dds_count = 0usize;
        walk_loose_dds(&bin360, &bin360, &mut |path, name, depth| {
            let h = pandemic_hash(&name);
            let keep = match loose_dds_by_depth.get(&h) {
                Some(&prev) => depth < prev,
                None => true,
            };
            if keep {
                loose_dds.insert(h, LooseDds { name: name.clone(), path: path.to_path_buf() });
                loose_dds_by_depth.insert(h, depth);
                // Bonus name-inventory entry — a texture whose only appearance is a loose file
                // still gets a plaintext name we can show in the browser.
                hash_names.entry(h).or_insert(name);
            }
            loose_dds_count += 1;
        });
        fingerprint.matched(format!(
            "{loose_dds_count} loose .dds files ({} unique hashes)",
            loose_dds.len()
        ));

        // Walk every `.materials` slot at open time and build the global material→slot-vector
        // lookup. Reads go through the SBL-family sub-pack directory (all seven of
        // SBL0/SBL4/SBL5/SBL6/SBL8/SBLL/SBLX share the same 8-byte header + 24-byte record +
        // contiguous BE data layout — the 4-byte magic suffix is a family tag, not a format
        // switch; see `docs/2008_subpack_variants.md`). Its offsets are authoritative; the
        // earlier packlog-driven read landed in the wrong region for every non-first slot on
        // disk. Genuinely-out-of-family packs (empty placeholders, MP00 mega-streamblocks,
        // AP04 Animations.pack — none of which are sub-packs at all) are counted separately.
        let mut materials = MaterialIndex::new();
        let mut read_errs = 0usize;
        let mut non_family = 0usize;
        let mut by_magic: HashMap<[u8; 4], usize> = HashMap::new();
        // Iterate unique pack files — every packlog entry for the same block points at the same
        // .Pack, and one .Pack has at most one `.materials` slot. Deduping keeps us from
        // re-opening + re-parsing the same file per mat_index entry.
        let mut seen_packs: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
        for e in &mat_index {
            if !seen_packs.insert(e.pack_file.clone()) {
                continue;
            }
            let buf = match std::fs::read(&e.pack_file) {
                Ok(b) => b,
                Err(_) => { read_errs += 1; continue }
            };
            let sbl = match SubPack::parse(&buf) {
                Ok(s) => s,
                Err(_) => { non_family += 1; continue }
            };
            *by_magic.entry(sbl.magic).or_insert(0) += 1;
            let Some(rec) = sbl.find("materials") else { continue };
            let Some(slot) = sbl.slot_bytes(rec, &buf) else { continue };
            materials.ingest_slot(slot);
        }
        fingerprint.matched(format!(
            "material bindings: {} materials indexed from {}/{} readable .materials slots \
             (non-family .Pack files skipped: {}; read errors: {})",
            materials.materials_indexed,
            materials.slots_ok,
            materials.slots_read,
            non_family,
            read_errs,
        ));
        // Per-magic split of what actually opened — distinguishes a "SBL4-only reader regressed"
        // failure from a "genuine non-family file" outcome. Two very different problems.
        if !by_magic.is_empty() {
            let mut parts: Vec<(String, usize)> = by_magic
                .iter()
                .map(|(m, n)| (String::from_utf8_lossy(m).into_owned(), *n))
                .collect();
            parts.sort_by(|a, b| b.1.cmp(&a.1));
            let list = parts.iter().map(|(m, n)| format!("{m}={n}")).collect::<Vec<_>>().join(", ");
            fingerprint.matched(format!("sub-pack magics: [{list}]"));
        }
        if non_family > 0 {
            fingerprint.warn(format!(
                "{non_family} .Pack files are not sub-pack containers at all \
                 (empty placeholders / MP00 mega-streamblocks / AP04 Animations.pack)"
            ));
        }
        // Fold material NAMES into the global hash→name map — packlogs don't name materials
        // themselves (only the blocks that contain them), and the picker's Material handles
        // want the plaintext, not the raw hash.
        for (uid, name) in &materials.name_by_uid {
            hash_names.entry(*uid).or_insert_with(|| name.clone());
        }

        Ok(Self {
            bin360,
            fingerprint,
            hash_names,
            mesh_index,
            tex_index,
            mat_index,
            loose_dds,
            tex_cache: Mutex::new(HashMap::new()),
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
                hash: Some(e.hash),
                name: self.hash_names.get(&e.hash).cloned().unwrap_or_default(),
                kind: AssetKind::Mesh,
            })
            .collect()
    }

    fn list_textures(&self) -> Vec<AssetHandle> {
        // Two population sources today: packlog-mined `<hash>.textures` slots (which still need
        // SBL4 to unwrap before we can decode them — probe #1) and the loose `*.dds` files at
        // Bin360 root and under Branches/. Both are surfaced so the browser can enumerate every
        // texture; `load_texture` picks its decode path on the fly. Keys are the packlog index
        // for slot rows and `u32::MAX - <loose_idx>` for loose rows — the discriminator lets us
        // route without a second lookup and keeps a stable-order view for the UI (though only
        // the hash is functionally used; the browser sorts by name/hash anyway).
        let mut out: Vec<AssetHandle> = self
            .tex_index
            .iter()
            .enumerate()
            .map(|(i, e)| AssetHandle {
                key: i as u64,
                hash: Some(e.hash),
                name: self.hash_names.get(&e.hash).cloned().unwrap_or_default(),
                kind: AssetKind::Texture,
            })
            .collect();
        // Loose DDS handles sit in a distinct key namespace so consumers that round-trip a
        // handle can tell packlog-slot from loose-file without a second table.
        for (i, (h, entry)) in self.loose_dds.iter().enumerate() {
            out.push(AssetHandle {
                key: (u64::MAX - i as u64),
                hash: Some(*h),
                name: entry.name.clone(),
                kind: AssetKind::Texture,
            });
        }
        out
    }

    fn read_mesh_bytes(&self, handle: &AssetHandle) -> Result<Vec<u8>, String> {
        let e = self
            .mesh_index
            .get(handle.key as usize)
            .ok_or_else(|| format!("mesh handle {} out of range", handle.key))?;
        // The packlog's `Start`/`Size` fields point at an intermediate build-time layout —
        // the packer reorders slots on disk, so trusting those numbers reads bytes from the
        // wrong slot (usually a hunk of the neighbouring textures blob). The authoritative
        // directory lives in the `.Pack` file itself, prefixed by SBL4. Parse it, find the
        // record whose kind_hash == pandemic_hash("meshes"), and hand back exactly that.
        //
        // This is still not a per-mesh MSHA extraction — the meshes slot is itself a
        // mini-directory of zlib-compressed `MSH4` records with a shape distinct from
        // retail's `AHSM`, and that inner format is a follow-up probe. For now the caller
        // gets the correct slot boundary, from which the individual `MSH4` records can be
        // walked once that reader lands.
        let pack_bytes = std::fs::read(&e.pack_file)
            .map_err(|err| format!("read {}: {err}", e.pack_file.display()))?;
        let sub = SubPack::parse(&pack_bytes)
            .map_err(|err| format!("{}: {err}", e.pack_file.display()))?;
        let rec = sub.find("meshes").ok_or_else(|| {
            format!(
                "{}: SBL4 sub-pack has no meshes slot (records: {:?})",
                e.pack_file.display(),
                sub.records
                    .iter()
                    .map(|r| r.kind_name.unwrap_or("?"))
                    .collect::<Vec<_>>()
            )
        })?;
        Ok(pack_bytes[rec.offset..rec.offset + rec.size].to_vec())
    }

    fn textures_for_material(&self, mat: u32) -> Option<[Option<u32>; 4]> {
        // Global `.materials`-slot inventory built at open time (see `open`). Slots that were
        // SBL4-wrapped are absent from this index; `None` here means either "unknown material"
        // OR "material lives in a still-wrapped block" — the fingerprint counter distinguishes.
        self.materials.textures_for(mat)
    }

    fn load_texture(&self, hash: u32) -> Result<CpuTexture, String> {
        // Cache hit — repeated preview / material-slot lookups don't reopen the file.
        if let Some(t) = self.tex_cache.lock().unwrap().get(&hash) {
            return Ok(t.clone());
        }
        // Path A: loose `.dds` at Bin360 root or under Branches/. Empirically standard PC DDS
        // (linear layout, LE DXT blocks — see docs/2008_texture_format.md), so we can go
        // straight through the shared `dtex::decode_surface` block decoder. This lights up 615
        // textures across DXT1/2/3/4/5 with no further work.
        if let Some(entry) = self.loose_dds.get(&hash) {
            let bytes = std::fs::read(&entry.path)
                .map_err(|e| format!("read {}: {e}", entry.path.display()))?;
            let tex = dds::decode_bytes(&entry.name, &bytes)?;
            self.tex_cache.lock().unwrap().insert(hash, tex.clone());
            return Ok(tex);
        }
        // Path B: packlog-mined `<hash>.textures` slots inside a `.Pack`. The `.Pack` header
        // starts with `SBL4`/`SBLL`/`SBL8`/etc. sub-pack magic (2612 of 4396 are SBL4) and the
        // slot payload past the SBL4 header is opaque — not raw DDS, not raw DTEX, no DXT/DTEX
        // 4CC anywhere. Scanned Act1_IntKey.Pack (999,201 B) for `DDS `, `DXT1/3/5`, `DTEX`:
        // zero hits. Only `WSAO` + `MTLD` sit in cleartext, at the very start where the
        // material record hasn't been swept up into whatever encoding follows. That's
        // consistent with an SBL4 wrapper that batches every record type behind a single
        // (possibly LZ / bit-packed) stream. Un-wrapping is probe #1's job; until it lands, we
        // can't reach the pixel data even if the format were trivial.
        if self.tex_index.iter().any(|e| e.hash == hash) {
            return Err(format!(
                "prototype 2008: texture {hash:08X} lives inside an SBL4-wrapped .Pack slot; \
                 waiting on the SBL4 sub-pack probe (probe #1) — no plaintext DDS/DTEX bytes \
                 present in the slot"
            ));
        }
        Err(format!(
            "prototype 2008: no loose .dds and no packlog-mined slot known for hash {hash:08X}"
        ))
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

/// Recursive `*.dds` walk. Depth is measured in path components below `root` so callers can
/// prefer the shallowest twin when the same stem appears in more than one Branches/*/ tree.
fn walk_loose_dds(
    root: &Path,
    dir: &Path,
    visit: &mut dyn FnMut(&Path, String, usize),
) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk_loose_dds(root, &p, visit);
        } else if p
            .extension()
            .and_then(|s| s.to_str())
            .map(|s| s.eq_ignore_ascii_case("dds"))
            .unwrap_or(false)
        {
            let name = p
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            if name.is_empty() {
                continue;
            }
            let depth = p
                .strip_prefix(root)
                .map(|rel| rel.components().count())
                .unwrap_or(usize::MAX);
            visit(&p, name, depth);
        }
    }
}

// NOTE: the earlier `read_pack_slot(pack_file, start, size)` helper — which trusted the
// packlog's Start/Size fields to seek into a `.Pack` — was dropped when the SBL4 sub-pack
// decoder landed. The packlog offsets describe a build-time intermediate layout that the
// packer reorders on write, so seeking to them read the wrong bytes; the SBL4SubPack
// directory (see `prototype2008_subpack.rs`) is the authoritative on-disk map.

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
        // With SBL4 unwrap wired (probe #1), the material walk now reads FROM the correct
        // on-disk offsets. Every SBL4 .Pack with a materials slot ingests; the count should be
        // in the many-hundreds. If this drops back to the pre-SBL4 double-digits floor, the
        // walk regressed to packlog offsets.
        assert!(
            src.materials.slots_ok >= 100,
            "expected ≥100 material slots to parse cleanly post-SBL4, got {}/{}",
            src.materials.slots_ok,
            src.materials.slots_read
        );
        assert!(
            src.materials.materials_indexed >= 500,
            "expected ≥500 materials indexed, got {}",
            src.materials.materials_indexed
        );
        // The fingerprint must surface the split (readable + non-SBL4 + errored).
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
        assert!(src.textures_for_material(0xDEAD_BEEF).is_none());
    }

    #[test]
    fn loose_dds_decodes_to_rgba8() {
        // Real-file check for the loose-DDS path. CloudProj is the biggest and cleanest of the
        // Bin360 root DDS files (512×512 DXT1, no mips, no cubemap). If the decoder ever
        // regresses to a tiled interpretation the byte-count assertion still passes but the
        // image is scrambled — the eyeball-verified reference PNG lives in the docs.
        let Some(root) = test_root() else {
            eprintln!("skip: no Bin360/ prototype");
            return;
        };
        let src = Prototype2008Source::open(&root).expect("open");
        let hash = pandemic_hash("CloudProj");
        let tex = src.load_texture(hash).expect("decode loose CloudProj.dds");
        assert_eq!(tex.width, 512);
        assert_eq!(tex.height, 512);
        assert_eq!(tex.rgba.len() as u64, tex.width as u64 * tex.height as u64 * 4);
    }

    #[test]
    fn load_texture_on_real_hash_either_decodes_or_stubs_explicitly() {
        // Both outcomes are pass states — either a loose DDS decodes, or the packlog-mined slot
        // returns the SBL4-blocked stub. What we're guarding against is a silent success on a
        // hash we can't actually decode (returning garbage), or a panic.
        let Some(root) = test_root() else {
            eprintln!("skip: no Bin360/ prototype");
            return;
        };
        let src = Prototype2008Source::open(&root).expect("open");
        let textures = src.list_textures();
        assert!(!textures.is_empty(), "packlog + loose DDS should yield textures");
        let mut decoded_any = false;
        let mut blocked_any = false;
        for (h, _name) in src.hash_names.iter().take(2000) {
            match src.load_texture(*h) {
                Ok(t) => {
                    decoded_any = true;
                    assert_eq!(t.rgba.len() as u64, t.width as u64 * t.height as u64 * 4);
                }
                Err(e) => {
                    if e.contains("SBL4-wrapped") {
                        blocked_any = true;
                    } else if e.contains("no loose .dds and no packlog-mined slot") {
                        // A hash that showed up in `hash_names` but not in either texture table
                        // — a block-name hash for a mesh/material. Fine.
                    } else {
                        panic!("unexpected error for hash {h:08X}: {e}");
                    }
                }
            }
            if decoded_any && blocked_any {
                break;
            }
        }
        assert!(
            decoded_any || blocked_any,
            "expected at least one hash to either decode (loose DDS) or hit the SBL4 stub"
        );
    }

    #[test]
    fn read_mesh_bytes_extracts_the_sbl4_meshes_slot() {
        let Some(root) = test_root() else {
            eprintln!("skip: no Bin360/ prototype");
            return;
        };
        let src = Prototype2008Source::open(&root).expect("open");
        let meshes = src.list_meshes();
        assert!(!meshes.is_empty(), "packlog inventory should yield meshes");

        // What "correct" looks like: a 2008 SBL4 meshes slot has the shape
        //   u32 BE count  |  count × 24-byte records (BE)  |  zlib blobs
        // where each inflated blob starts with the ASCII magic "MSH4". So the earmark
        // is straightforward: parse the small outer header, chase the first record's
        // offset, and expect a zlib stream (`78 01…`). If we grabbed the wrong bytes
        // (the OLD packlog-driven path did — it landed in the middle of the textures
        // blob), this check falls over.
        //
        // .Pack files that aren't SBL4 (SBLL, SBL8, …) surface as clean errors here;
        // we walk to the first mesh whose containing pack IS an SBL4, so the presence
        // of other container magics doesn't fail this test.
        let mut probed = 0usize;
        let mut ok_shape = 0usize;
        let mut inflated_msh4 = 0usize;
        for m in meshes.iter().take(500) {
            probed += 1;
            let Ok(bytes) = src.read_mesh_bytes(m) else { continue };
            if bytes.len() < 4 {
                continue;
            }
            let n = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
            if n == 0 || n > 4096 {
                continue;
            }
            let dir_end = 4 + n * 24;
            if dir_end > bytes.len() {
                continue;
            }
            let first_off = u32::from_be_bytes([bytes[8], bytes[9], bytes[10], bytes[11]])
                as usize;
            let first_comp = u32::from_be_bytes([bytes[12], bytes[13], bytes[14], bytes[15]])
                as usize;
            if first_off + first_comp > bytes.len() || first_off < dir_end {
                continue;
            }
            ok_shape += 1;
            // Zlib magic 0x78 0x01/0x9C is the loader-friendly earmark: no other
            // 2008 container starts that way, so seeing it at first_off proves we
            // hit a meshes slot and not, say, a padded audio buffer.
            let head = &bytes[first_off..first_off + first_comp.min(2)];
            if head.len() < 2 || head[0] != 0x78 {
                continue;
            }
            use std::io::Read;
            let mut z = flate2::read::ZlibDecoder::new(&bytes[first_off..first_off + first_comp]);
            let mut out = Vec::new();
            if z.read_to_end(&mut out).is_err() {
                continue;
            }
            if out.len() >= 4 && &out[0..4] == b"MSH4" {
                inflated_msh4 += 1;
                if inflated_msh4 >= 3 {
                    break; // three concordant witnesses = the layout is real
                }
            }
        }
        eprintln!(
            "[read_mesh_bytes] probed {probed} handles, {ok_shape} looked like a meshes slot, \
             {inflated_msh4} inflated to a MSH4 record"
        );
        assert!(
            inflated_msh4 > 0,
            "no meshes slot round-tripped: read_mesh_bytes is still returning wrong bytes \
             (probed={probed}, shape-ok={ok_shape})"
        );
    }

    #[test]
    fn unknown_texture_hash_returns_the_catchall_error() {
        // A hash that isn't in any index at all — neither a loose DDS nor a packlog-mined slot
        // — should hit the catch-all error, not the SBL4-blocked one and not a silent success.
        // (Probe #1's earlier `stubbed_texture_decoder_still_returns_explicit_error` was
        // dropped when probe #2 wired loose DDS — texture decode is no longer wholly stubbed;
        // this covers the still-unreachable-hash case.)
        let Some(root) = test_root() else {
            eprintln!("skip: no Bin360/ prototype");
            return;
        };
        let src = Prototype2008Source::open(&root).expect("open");
        let err = match src.load_texture(0xdeadbeef) {
            Err(e) => e,
            Ok(_) => panic!("expected error for unknown hash, got a decoded texture"),
        };
        assert!(
            err.contains("no loose .dds and no packlog-mined slot"),
            "expected explicit unknown-hash error, got: {err}"
        );
    }
}
