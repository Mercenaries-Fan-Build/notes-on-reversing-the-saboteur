//! Character texture resolver.
//!
//! The click path binds textures the engine's way: a submesh's `materials[0]` → WSMA record →
//! WSTX slot 0 (colour) → a DTEX name-hash, resolved to a record by `take_hashes_from_*` here (see
//! `wsao` and `app::resolve_model_textures`). France.materials IS present on the retail PC build, so
//! this is a real resolution, not a guess.
//!
//! The name-suffix heuristic below (`classify`, `autoseed_for_part`, `texture_pool_for`) is the
//! FALLBACK for when the WSAO table can't answer — DTEX names are PLAINTEXT, so we find the bundles
//! carrying the character token and classify each record by suffix (`_D` diffuse, `_N`/`_NM` normal,
//! `_S` spec, `_WM`/`_MASK` mask). It only works for the ~27 assets whose textures carry a `_D`
//! suffix (Sean's base outfit among them), which is why WSAO is the primary path.

use crate::dtex::{self, CpuTexture};
use crate::pack::{self, Megapack};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    Diffuse,
    Normal,
    Spec,
    Mask,
    Other,
}

impl Role {
    pub fn label(self) -> &'static str {
        match self {
            Role::Diffuse => "diffuse",
            Role::Normal => "normal",
            Role::Spec => "spec",
            Role::Mask => "mask",
            Role::Other => "other",
        }
    }
}

/// One texture found in the character's bundles. Holds the RAW (still-compressed) DTEX record and
/// only its header metadata — decoding is LAZY (`decode()`), because a bundle sweep turns up a couple
/// hundred records and eagerly BC-decoding them all would cost seconds and hundreds of MB of RGBA for
/// the handful actually bound to a submesh.
#[derive(Clone)]
pub struct TexAsset {
    pub name: String,
    pub role: Role,
    pub width: u32,
    pub height: u32,
    pub format: String,
    record: Vec<u8>,
}

impl TexAsset {
    /// Decode this record's finest mip to RGBA (call when binding it to a submesh).
    pub fn decode(&self) -> Result<CpuTexture, String> {
        dtex::decode(&self.record)
    }

    /// Decode a small mip for a contact-sheet thumbnail — cheap enough to do for the whole pool.
    pub fn decode_preview(&self, min_dim: u32) -> Result<CpuTexture, String> {
        dtex::decode_preview(&self.record, min_dim)
    }

    /// Mip count, for the record inspector.
    pub fn mip_count(&self) -> u16 {
        dtex::parse(&self.record).map(|d| d.mips).unwrap_or(0)
    }
}

/// Classify a DTEX by its name suffix (the game's role convention).
pub fn classify(name: &str) -> Role {
    let n = name.to_ascii_lowercase();
    if n.ends_with("_nm") || n.ends_with("_n") {
        Role::Normal
    } else if n.ends_with("_s") {
        Role::Spec
    } else if n.ends_with("_wm") || n.ends_with("_mask") {
        Role::Mask
    } else if n.ends_with("_d") {
        Role::Diffuse
    } else {
        Role::Other
    }
}

/// Load the textures from the character's bundles in `megapack_path`. `token` is an ASCII substring
/// (e.g. "SeanDevlinn") matched case-insensitively against the raw bundle bytes to select which
/// bundles are this character's — the pre-scan keeps us from running the expensive record walk over
/// the whole 715 MB pack.
///
/// EVERY DTEX record in a matched bundle is returned, not just token-named ones: a character bundle
/// also carries its accessory textures (`CH_AC_Eyes_*`, `CH_AC_Mouth`), which are exactly what the
/// head submeshes need. Duplicate names collapse to the first occurrence.
pub fn load_character_textures(megapack_path: &str, token: &str) -> Result<Vec<TexAsset>, String> {
    let mp = Megapack::open(megapack_path)?;
    Ok(textures_in(&mp, token))
}

/// As `load_character_textures`, but over an already-open megapack (the app keeps one resident for
/// browsing + click-to-load, so re-reading 715 MB per asset would be wasteful).
pub fn textures_in(mp: &Megapack, token: &str) -> Vec<TexAsset> {
    // The token (character name) can appear anywhere in a bundle — its main body textures live past
    // any small header window (proven: a 64 KB window found only 3 of 222) — so the token match must
    // scan every sub-pack fully. To avoid faulting the whole 714 MB pack into our working set (which
    // froze the UI), read each sub-pack via a BUFFERED file read into one reused buffer (bounded
    // memory), and yield periodically so this background worker never monopolises the machine.
    let mut out: Vec<TexAsset> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    let mut buf: Vec<u8> = Vec::new();
    for (i, e) in mp.entries().iter().enumerate() {
        let sub = mp.read_into(e, &mut buf);
        if !sub.is_empty() && pack::contains_ascii_ci(sub, token) {
            collect_records(sub, &mut out, &mut seen);
        }
        if i % 16 == 0 {
            std::thread::yield_now();
        }
    }
    out.sort_by(|a, b| a.name.to_ascii_lowercase().cmp(&b.name.to_ascii_lowercase()));
    out
}

/// Every texture in ONE bundle — used for a model loaded from the navigator (its own bundle carries
/// its skins).
pub fn textures_in_slice(sub: &[u8]) -> Vec<TexAsset> {
    let mut out = Vec::new();
    let mut seen = Vec::new();
    collect_records(sub, &mut out, &mut seen);
    out.sort_by(|a, b| a.name.to_ascii_lowercase().cmp(&b.name.to_ascii_lowercase()));
    out
}

fn collect_records(sub: &[u8], out: &mut Vec<TexAsset>, seen: &mut Vec<String>) {
    for (off, len, name) in dtex::find_records(sub) {
        if seen.iter().any(|s: &String| s.eq_ignore_ascii_case(&name)) {
            continue;
        }
        let record = &sub[off..off + len];
        // Header only — no inflate/BC decode here (see TexAsset).
        let Ok(d) = dtex::parse(record) else { continue };
        seen.push(name.clone());
        out.push(TexAsset {
            role: classify(&name),
            name,
            width: d.width as u32,
            height: d.height as u32,
            format: d.format_name(),
            record: record.to_vec(),
        });
    }
}

/// Generic seed for a model loaded from the navigator: we have no part decomposition (that's the
/// merged-Sean case in `autoseed`), so bind every submesh to the pool's first diffuse as a visible
/// starting point. The picker is how it gets correct — see the Materials note in the README.
pub fn autoseed_generic(n_submeshes: usize, assets: &[TexAsset]) -> Vec<Option<usize>> {
    let first_diffuse = assets.iter().position(|a| a.role == Role::Diffuse);
    vec![first_diffuse; n_submeshes]
}

/// Seed a PART's submeshes from that part's own texture pool.
///
/// `autoseed_generic` hands the first diffuse in the pool to everything, which is a coin flip: a
/// head bundle also carries eye and mouth maps, so the face would come out wearing an eyeball.
/// A part knows what it is from its name suffix (`…_HD`, `…_UB`, `…_LB`, `…_GR`, `…_HAT`), and the
/// artists named the textures to match (`Head_D`, `jacket_D`, `pants_D`, `hand_D`, `hat_D`), so
/// match the two. Falls back to the first diffuse when nothing matches — better a plausible skin
/// than none.
///
/// This is still a heuristic, not a resolution: WSAO, which would name each material outright, is
/// absent from the PC build. The Materials picker is where a wrong slot gets corrected.
/// The outfit token inside a part's mesh name: `CH_AL_SeanRacing_UB` -> `SeanRacing`.
///
/// This is the string the character's TEXTURE bundles actually contain. Searching for the full mesh
/// name finds nothing — no texture is called `CH_AL_SeanRacing_UB` — which is why an outfit whose
/// parts were named unlike the base character came out entirely untextured.
pub fn outfit_token(part_name: &str) -> String {
    let mut t = part_name.to_string();
    for p in ["CH_AL_", "CH_MB_", "CH_CF_", "CH_CM_", "CH_SS_", "CH_GS_", "CH_NZ_", "CH_CV_", "CH_"] {
        if let Some(r) = t.strip_prefix(p) {
            t = r.to_string();
            break;
        }
    }
    match t.rsplit_once('_') {
        Some((head, _)) => head.to_string(),
        None => t,
    }
}

pub fn autoseed_for_part(part_name: &str, n_submeshes: usize, assets: &[TexAsset]) -> Vec<Option<usize>> {
    // part suffix -> the words the matching texture tends to use
    const SLOTS: &[(&str, &[&str])] = &[
        ("HD", &["head", "face"]),
        ("FM", &["head", "face"]),
        ("FX", &["head", "face"]),
        ("UB", &["jacket", "torso", "shirt", "coat", "body", "_ub"]),
        ("LB", &["pants", "leg", "trouser", "_lb"]),
        ("GR", &["hand", "glove", "_gr"]),
        ("HAT", &["hat", "cap", "helmet"]),
        ("HAIR", &["hair"]),
    ];
    let upper = part_name.to_ascii_uppercase();
    let suffix = upper.rsplit('_').next().unwrap_or("");
    let words: &[&str] = SLOTS
        .iter()
        .find(|(k, _)| *k == suffix || upper.ends_with(&format!("_{k}")))
        .map(|(_, w)| *w)
        .unwrap_or(&[]);

    // Words that belong to a DIFFERENT slot. A blind "first diffuse" fallback is how a face ended up
    // printed across a pair of trousers: an outfit whose textures are named unlike its parts
    // (SeanRacing_LB with no "pants"/"leg" texture) fell through to whatever sorted first, and a
    // head map often does. Better to leave a part untextured than to dress it in someone's face.
    let foreign: Vec<&str> = SLOTS
        .iter()
        .filter(|(k, _)| *k != suffix)
        .flat_map(|(_, w)| w.iter().copied())
        .filter(|w| !words.contains(w))
        .collect();
    let is_foreign = |n: &str| foreign.iter().any(|w| n.contains(w));

    let diffuse = |pred: &dyn Fn(&str) -> bool| {
        assets.iter().position(|a| {
            a.role == Role::Diffuse && pred(&a.name.to_ascii_lowercase())
        })
    };
    // The outfit's own token: `CH_AL_SeanRacing_LB` -> "seanracing". Outfits are often named after
    // themselves rather than after body parts (`SeanRacing_Suit_D`, not `..._pants_D`), so this is
    // what generalises past the fixed word list to every character.
    let token = outfit_token(part_name).to_ascii_lowercase();

    let pick = diffuse(&|n| words.iter().any(|w| n.contains(w)))
        // same outfit, and not something that plainly belongs to another slot
        .or_else(|| diffuse(&|n| !token.is_empty() && n.contains(&token) && !is_foreign(n)))
        // nothing named for this slot — take any diffuse that is not obviously another slot's
        .or_else(|| diffuse(&|n| !is_foreign(n)));
    vec![pick; n_submeshes]
}

/// Progressively broader search tokens for a model name. Saboteur asset names are
/// `<CAT>_<SUB>_<Thing…>[_variant]` and their textures usually share the `<Thing>` stem but NOT the
/// category prefix (`CH_AL_SeanDevlin_01_GR`'s skins are `CH_MB_SeanDevlinn_*`), so we drop the
/// two-segment prefix and then shed trailing segments, longest token first.
fn name_tokens(model: &str) -> Vec<String> {
    let segs: Vec<&str> = model.split('_').collect();
    // Drop a leading 2-segment category prefix when it looks like one (short codes: CH_AL, GB_WP…).
    let start = if segs.len() > 3 && segs[0].len() <= 3 && segs[1].len() <= 3 { 2 } else { 0 };
    let mut out = Vec::new();
    let mut end = segs.len();
    while end > start {
        let tok = segs[start..end].join("_");
        // A 1-2 char token would match half the pack.
        if tok.len() >= 3 {
            out.push(tok);
        }
        end -= 1;
    }
    out
}

/// Resolve DTEX name-hashes (as produced by WSAO — `pandemic_hash(textureName)`) to their records
/// within a set of raw sub-pack slices. Each hash that resolves is REMOVED from `needed` and its asset
/// inserted into `out` (first occurrence wins). Used to satisfy the common case — a character's skins
/// usually co-locate in the same bundle as its mesh — before paying for a whole-pack sweep.
pub fn take_hashes_from_slices(
    slices: &[&[u8]],
    needed: &mut std::collections::HashSet<u32>,
    out: &mut std::collections::HashMap<u32, TexAsset>,
) {
    for sub in slices {
        if needed.is_empty() {
            break;
        }
        for (off, len, name) in dtex::find_records(sub) {
            let h = pack::pandemic_hash(&name);
            if !needed.remove(&h) {
                continue;
            }
            let record = &sub[off..off + len];
            if let Ok(d) = dtex::parse(record) {
                out.insert(
                    h,
                    TexAsset {
                        role: classify(&name),
                        name,
                        width: d.width as u32,
                        height: d.height as u32,
                        format: d.format_name(),
                        record: record.to_vec(),
                    },
                );
            }
        }
    }
}

/// Whole-pack fallback for the WSAO hashes NOT co-located with the mesh — props keep their skins in
/// `Palettes0` (co-located only ~0.1% of the time), and ~23% of character texture refs cross bundles.
/// One buffered pass over every sub-pack, stopping the moment `needed` is empty.
pub fn take_hashes_from_packs(
    packs: &[&Megapack],
    needed: &mut std::collections::HashSet<u32>,
    out: &mut std::collections::HashMap<u32, TexAsset>,
) {
    let mut buf: Vec<u8> = Vec::new();
    for mp in packs {
        for (i, e) in mp.entries().iter().enumerate() {
            if needed.is_empty() {
                return;
            }
            let sub = mp.read_into(e, &mut buf);
            if !sub.is_empty() {
                take_hashes_from_slices(&[sub], needed, out);
            }
            if i % 16 == 0 {
                std::thread::yield_now();
            }
        }
    }
}

/// The texture pool for a model loaded from the navigator.
///
/// A character's skins sit in its OWN bundle (fast path). Plenty of assets don't work that way
/// though — props keep theirs in the shared palette archive — so if the bundle yields no diffuse we
/// sweep every open pack by name token (longest first) and take the first token that turns up a
/// diffuse. `packs` should be the dynamic pack AND `Palettes0` (see `Config::palettes`). An empty
/// pool means nothing matched: the model renders white and the picker has nothing to offer, which
/// the Materials panel says out loud.
pub fn texture_pool_for(packs: &[&Megapack], model_name: &str, bundle: &[u8]) -> Vec<TexAsset> {
    let own = textures_in_slice(bundle);
    if own.iter().any(|a| a.role == Role::Diffuse) {
        return own;
    }
    for tok in name_tokens(model_name) {
        for mp in packs {
            let hit = textures_in(mp, &tok);
            if hit.iter().any(|a| a.role == Role::Diffuse) {
                return hit;
            }
        }
    }
    own
}

/// Per-submesh texture assignment persisted next to the mesh (`<mesh>.materials.json`). Stores
/// texture NAMES (stable across re-resolves) rather than indices. This is the user-authored map that
/// stands in for the identity WSAO would have provided — the picker writes it, load prefers it over
/// the auto-seed.
/// How a submesh's texture binding was arrived at.
///
/// WSAO — which would *name* each material — is absent from the retail PC build, so nothing here is
/// ever truly "resolved"; it is only ever bound, well or badly. Keeping the distinction visible is
/// the point: a guess must never present itself as a decision.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Prov {
    /// A choice the user made in the picker. The sidecar keeps it.
    Bound,
    /// The body-part heuristic's guess (HD→head, UB→jacket, …). Wrong wherever one part uses
    /// several textures — the head's eyes and mouth both seed to `Head_D`.
    Seeded,
    /// No texture bound: a `materialHash` with no name to resolve it against.
    Unresolved,
}

#[derive(serde::Serialize, serde::Deserialize, Default)]
pub struct Sidecar {
    pub submeshes: Vec<Option<String>>,
    /// Per submesh: was this an actual decision, or just the auto-seed we happened to save
    /// alongside it? Without this the file cannot tell them apart — `save_sidecar` writes the whole
    /// assignment, seeded entries included, so on reload every guess would look like a choice.
    ///
    /// Absent in sidecars written before this field existed. Those predate the distinction, so they
    /// are read as decisions (the conservative read: it keeps hand-authored work looking authored).
    #[serde(default)]
    pub bound: Vec<bool>,
}

pub fn sidecar_path(profile: &str) -> String {
    format!("{profile}.materials.json")
}

/// The sidecar "profile" for a model that has no file of its own — one assembled out of the megapack.
///
/// Such a model cannot keep its assignments beside itself, and it must NOT share the startup mesh's
/// sidecar (which is what every clicked model used to overwrite, silently, one after another). They
/// go beside the settings instead, keyed by model name:
/// `%APPDATA%/sab_workshop/materials/<model>`.
pub fn profile_path(model_name: &str) -> String {
    let safe: String = model_name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
        .collect();
    crate::settings::settings_path()
        .parent()
        .map(|d| d.join("materials").join(&safe).to_string_lossy().replace('\\', "/"))
        .unwrap_or(safe)
}

/// Read the sidecar and map its texture names onto `assets` indices, per submesh, with each
/// binding's provenance. Returns `None` if there's no sidecar. Unknown names (texture missing this
/// run) resolve to `None` for that submesh.
pub fn load_sidecar(
    mesh_path: &str,
    n_submeshes: usize,
    assets: &[TexAsset],
) -> Option<(Vec<Option<usize>>, Vec<Prov>)> {
    let text = std::fs::read_to_string(sidecar_path(mesh_path)).ok()?;
    let sc: Sidecar = serde_json::from_str(&text).ok()?;
    let mut out = vec![None; n_submeshes];
    let mut prov = vec![Prov::Unresolved; n_submeshes];
    for (i, slot) in sc.submeshes.iter().take(n_submeshes).enumerate() {
        if let Some(name) = slot {
            out[i] = assets.iter().position(|a| a.name.eq_ignore_ascii_case(name));
        }
        prov[i] = match out[i] {
            None => Prov::Unresolved,
            // No `bound` array => an old sidecar => treat as authored.
            Some(_) if sc.bound.get(i).copied().unwrap_or(true) => Prov::Bound,
            Some(_) => Prov::Seeded,
        };
    }
    Some((out, prov))
}

/// Write the current assignment — and how each entry was arrived at — to the sidecar.
pub fn save_sidecar(
    profile: &str,
    assign: &[Option<usize>],
    prov: &[Prov],
    assets: &[TexAsset],
) -> Result<(), String> {
    let sc = Sidecar {
        submeshes: assign.iter().map(|a| a.map(|ai| assets[ai].name.clone())).collect(),
        bound: prov.iter().map(|p| *p == Prov::Bound).collect(),
    };
    let text = serde_json::to_string_pretty(&sc).map_err(|e| e.to_string())?;
    let path = sidecar_path(profile);
    // A profile path lives under the config dir, which may not exist on first save.
    if let Some(parent) = std::path::Path::new(&path).parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
    }
    std::fs::write(&path, text).map_err(|e| format!("write {path}: {e}"))
}

/// The provenance implied by a fresh auto-seed: anything it placed is a guess, the rest is nothing.
pub fn seed_prov(assign: &[Option<usize>]) -> Vec<Prov> {
    assign.iter().map(|a| if a.is_some() { Prov::Seeded } else { Prov::Unresolved }).collect()
}

/// Sean's merged parts, in merge order, paired with the diffuse-name keyword each maps to. The
/// merge (`merge_smsh.py`) concatenates parts in this order, so cumulative index counts give each
/// part's range in the merged buffer. (Character-specific seed; the picker overrides anything wrong.)
const SEAN_PARTS: [(&str, &str); 5] =
    [("HD", "head"), ("UB", "jacket"), ("LB", "pants"), ("GR", "hand"), ("HAT", "hat")];

/// Best-effort submesh → diffuse-asset seed. Reads the sibling `parts/sean_<PART>.smsh` files to
/// recover each merged part's index range, maps a submesh (by its `index_start`) to its part, then
/// to the part's keyword diffuse. Returns `assets` index per submesh, or `None` where unknown.
/// All-`None` if the parts dir is absent (then the picker is the only path).
pub fn autoseed(mesh_path: &str, submeshes: &[crate::formats::SubMesh], assets: &[TexAsset]) -> Vec<Option<usize>> {
    let parts_dir = std::path::Path::new(mesh_path)
        .parent()
        .map(|d| d.join("parts"))
        .unwrap_or_default();
    // (keyword, start, end) per part, in merge order.
    let mut bounds: Vec<(&str, u32, u32)> = Vec::new();
    let mut cursor = 0u32;
    for (part, keyword) in SEAN_PARTS {
        let p = parts_dir.join(format!("sean_{part}.smsh"));
        let Ok(bytes) = std::fs::read(&p) else {
            continue;
        };
        if bytes.len() < 16 || &bytes[0..4] != b"SMSH" {
            continue;
        }
        let ni = u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]);
        bounds.push((keyword, cursor, cursor + ni));
        cursor += ni;
    }
    let diffuse_for = |keyword: &str| -> Option<usize> {
        assets.iter().position(|a| {
            a.role == Role::Diffuse && a.name.to_ascii_lowercase().contains(keyword)
        })
    };
    submeshes
        .iter()
        .map(|sm| {
            let part = bounds.iter().find(|(_, s, e)| sm.index_start >= *s && sm.index_start < *e)?;
            diffuse_for(part.0)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Against the REAL install: find + decode Sean's diffuse textures from Dynamic0.megapack.
    /// Skips cleanly when no install is detected — the location is never assumed.
    #[test]
    fn resolve_sean_diffuse_from_megapack() {
        let Some(s) = crate::settings::detected() else {
            eprintln!("skip: no Saboteur install detected");
            return;
        };
        let mp = &s.megapack();
        if !std::path::Path::new(mp).exists() {
            eprintln!("skip: {mp} not present");
            return;
        }
        let assets = load_character_textures(mp, "SeanDevlinn").expect("resolve");
        let diffuse: Vec<&TexAsset> = assets.iter().filter(|a| a.role == Role::Diffuse).collect();
        eprintln!("resolved {} Sean textures ({} diffuse)", assets.len(), diffuse.len());
        assert!(!diffuse.is_empty(), "expected at least one Sean diffuse texture");
        // The head diffuse must be present, correctly sized, and actually decode to real pixels.
        let head = diffuse
            .iter()
            .find(|a| a.name.to_ascii_lowercase().contains("head"))
            .expect("expected a head diffuse");
        assert_eq!((head.width, head.height), (512, 512));
        let tex = head.decode().expect("decode head diffuse");
        assert_eq!(tex.rgba.len(), 512 * 512 * 4);
        let n = (512 * 512) as u64;
        let (r, b) = tex.rgba.chunks_exact(4).fold((0u64, 0u64), |(r, b), p| {
            (r + p[0] as u64, b + p[2] as u64)
        });
        // Skin diffuse reads warm (R > B) — guards a byte-swapped or blank decode.
        assert!(r / n > r.min(b) / n || r > b, "skin warmer than blue (R {} > B {})", r / n, b / n);
        eprintln!("head diffuse decoded: {}x{} mean R={} B={}", tex.width, tex.height, r / n, b / n);
    }
}
