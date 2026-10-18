//! A model's textures: the pool a build's material data binds, each submesh's per-slot binding and
//! how it was arrived at, and the user's own bindings persisted beside the model (the sidecar).
//!
//! Each build binds from its own material data ([`crate::source::GameSource::resolve_textures`]);
//! retail resolves the DTEX name hashes its WSAO table names with `take_hashes_from_*` here.

use crate::dtex::{self, CpuTexture};
use crate::pack::Megapack;

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

/// One texture of a model's pool. Holds the RAW (still-compressed) record and only its header
/// metadata — decoding is LAZY (`decode()`), so a pool costs pixels only for what is drawn or shown.
#[derive(Clone)]
pub struct TexAsset {
    pub name: String,
    pub role: Role,
    pub width: u32,
    pub height: u32,
    pub format: String,
    record: Record,
}

/// The stored record a [`TexAsset`] decodes from.
#[derive(Clone)]
enum Record {
    /// A retail DTEX record.
    Dtex(Vec<u8>),
    /// A 2008 `textures` slot record; a normal map gets its z rebuilt for display.
    Proto2008 { record: std::sync::Arc<Vec<u8>>, normal: bool },
}

impl TexAsset {
    /// A retail DTEX record; the header is read now, the pixels when decoded.
    pub fn dtex(record: Vec<u8>, role: Role) -> Result<TexAsset, String> {
        let d = dtex::parse(&record)?;
        Ok(TexAsset { name: d.name.clone(), role, width: d.width as u32, height: d.height as u32, format: d.format_name(), record: Record::Dtex(record) })
    }

    /// A 2008 `textures` slot record.
    pub fn proto2008(name: String, role: Role, width: u32, height: u32, format: String, record: std::sync::Arc<Vec<u8>>, normal: bool) -> TexAsset {
        TexAsset { name, role, width, height, format, record: Record::Proto2008 { record, normal } }
    }

    /// Decode this record's finest mip to RGBA (call when binding it to a submesh).
    pub fn decode(&self) -> Result<CpuTexture, String> {
        match &self.record {
            Record::Dtex(r) => dtex::decode(r),
            Record::Proto2008 { record, normal } => crate::sources::prototype2008::decode_texture(record, *normal, None),
        }
    }

    /// Decode a small mip for a contact-sheet thumbnail — cheap enough to do for the whole pool.
    pub fn decode_preview(&self, min_dim: u32) -> Result<CpuTexture, String> {
        match &self.record {
            Record::Dtex(r) => dtex::decode_preview(r, min_dim),
            Record::Proto2008 { record, normal } => crate::sources::prototype2008::decode_texture(record, *normal, Some(min_dim)),
        }
    }
}

/// A model's textures as its build's material data binds them: the pool, each submesh's per-slot
/// binding and how it was arrived at, and the pixels of every bound slot as `(submesh, slot, pixels)`.
pub struct ModelTex {
    pub assets: Vec<TexAsset>,
    pub submesh_tex: Vec<SubmeshBinding>,
    pub submesh_prov: Vec<Prov>,
    pub decoded: Vec<(usize, usize, CpuTexture)>,
}

/// Resolve DTEX name-hashes (as produced by WSAO — `pandemic_hash(textureName)`) to their records
/// within a set of raw sub-pack slices. Each hash that resolves is REMOVED from `needed` and its asset
/// inserted into `out` (first occurrence wins), with role `Other` for the caller to set.
pub fn take_hashes_from_slices(
    slices: &[&[u8]],
    needed: &mut std::collections::HashSet<u32>,
    out: &mut std::collections::HashMap<u32, TexAsset>,
) -> Result<(), String> {
    for sub in slices {
        if needed.is_empty() {
            break;
        }
        for (off, len, name) in dtex::find_records(sub) {
            let h = sab_formats::pandemic_hash(&name);
            if !needed.remove(&h) {
                continue;
            }
            out.insert(h, TexAsset::dtex(sub[off..off + len].to_vec(), Role::Other).map_err(|e| format!("{name}: {e}"))?);
        }
    }
    Ok(())
}

/// [`take_hashes_from_slices`] over every sub-pack of `packs`, one buffered pass, stopping the moment
/// `needed` is empty.
pub fn take_hashes_from_packs(
    packs: &[&Megapack],
    needed: &mut std::collections::HashSet<u32>,
    out: &mut std::collections::HashMap<u32, TexAsset>,
) -> Result<(), String> {
    let mut buf: Vec<u8> = Vec::new();
    for mp in packs {
        for (i, e) in mp.entries().iter().enumerate() {
            if needed.is_empty() {
                return Ok(());
            }
            let sub = mp.read_into(e, &mut buf)?;
            if !sub.is_empty() {
                take_hashes_from_slices(&[sub], needed, out)?;
            }
            if i % 16 == 0 {
                std::thread::yield_now();
            }
        }
    }
    Ok(())
}

/// How a submesh's texture binding was arrived at.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Prov {
    /// The build's material data bound it, or the user picked it.
    Bound,
    /// Saved in a sidecar without being marked a decision.
    Seeded,
    /// No texture bound.
    Unresolved,
}

/// Per-submesh, per-slot binding. Slot indices mirror the WSMA texture-slice ordering documented in
/// `wsao.rs`: 0=diffuse, 1=spec, 2=normal, 3=wm (mask). This is the game's own ordering — the one
/// `Wsao::textures` returns — not a Mercs 2 import.
///
/// `Ord + Hash` come from the underlying array so a `SubmeshBinding` can key a `BTreeMap` (the
/// glTF material dedup uses this — one material per distinct tuple of slot indices).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SubmeshBinding {
    pub slots: [Option<usize>; 4],
}

impl SubmeshBinding {
    pub const DIFFUSE: usize = 0;
    pub const SPEC: usize = 1;
    pub const NORMAL: usize = 2;
    pub const MASK: usize = 3;
    pub const NUM_SLOTS: usize = 4;
    pub const SLOT_ROLES: [Role; 4] = [Role::Diffuse, Role::Spec, Role::Normal, Role::Mask];
    pub const SLOT_NAMES: [&'static str; 4] = ["diffuse", "spec", "normal", "mask"];

    pub fn diffuse(&self) -> Option<usize> { self.slots[Self::DIFFUSE] }
    pub fn spec(&self) -> Option<usize> { self.slots[Self::SPEC] }
    pub fn normal(&self) -> Option<usize> { self.slots[Self::NORMAL] }
    pub fn mask(&self) -> Option<usize> { self.slots[Self::MASK] }
    pub fn slot(&self, slot: usize) -> Option<usize> {
        self.slots.get(slot).copied().flatten()
    }
    pub fn set(&mut self, slot: usize, idx: Option<usize>) {
        if slot < Self::NUM_SLOTS {
            self.slots[slot] = idx;
        }
    }
    pub fn is_bound(&self) -> bool { self.slots.iter().any(|s| s.is_some()) }
    /// `(slot_index, asset_index)` for every populated slot, in slot order.
    pub fn iter_bound(&self) -> impl Iterator<Item = (usize, usize)> + '_ {
        self.slots.iter().enumerate().filter_map(|(i, s)| s.map(|a| (i, a)))
    }
}

/// Per-submesh serialized bindings. Slot fields are `Option` and default-empty so an old sidecar (or
/// a slot never populated) round-trips cleanly.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct SlotSidecar {
    pub diffuse: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spec: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub normal: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mask: Option<String>,
}

/// Per-submesh texture assignment persisted next to the mesh (`<profile>.materials.json`). Stores
/// texture NAMES (stable across re-resolves) rather than indices. The picker writes it; a load
/// prefers it over what the material data bound.
#[derive(serde::Serialize, serde::Deserialize, Default)]
pub struct Sidecar {
    /// Legacy per-submesh diffuse-only binding, kept for reading files written before the per-slot
    /// widening. Old files carried only this; new files omit it in favour of `slots` (serialized as
    /// empty and skipped on write below).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub submeshes: Vec<Option<String>>,
    /// Per-submesh, per-slot binding. Empty in legacy sidecars — `load_sidecar` migrates the
    /// diffuse from `submeshes` when this is empty.
    #[serde(default)]
    pub slots: Vec<SlotSidecar>,
    /// Per submesh: was this an actual decision, or a binding saved alongside one? Without this the
    /// file cannot tell them apart — `save_sidecar` writes the whole assignment.
    ///
    /// Absent in sidecars written before this field existed. Those predate the distinction, so they
    /// are read as decisions (the conservative read: it keeps hand-authored work looking authored).
    #[serde(default)]
    pub bound: Vec<bool>,
}

pub fn sidecar_path(profile: &str) -> String {
    format!("{profile}.materials.json")
}

/// The sidecar "profile" for a model that has no file of its own — one assembled out of the game.
///
/// Such a model cannot keep its assignments beside itself, and it must NOT share the startup mesh's
/// sidecar. They go beside the settings instead, keyed by model name:
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

/// Read the sidecar and map its texture names onto `assets` indices, per submesh + per slot, with
/// each submesh's provenance. Returns `None` if there's no sidecar. Unknown names (texture missing
/// this run) resolve to `None` for that slot.
///
/// Migrates legacy sidecars in place: a file with only `submeshes` (pre-widening) reads as
/// diffuse-only, slots 1/2/3 empty. The next `save_sidecar` writes the new shape.
pub fn load_sidecar(
    mesh_path: &str,
    n_submeshes: usize,
    assets: &[TexAsset],
) -> Option<(Vec<SubmeshBinding>, Vec<Prov>)> {
    let text = std::fs::read_to_string(sidecar_path(mesh_path)).ok()?;
    let sc: Sidecar = serde_json::from_str(&text).ok()?;
    let find = |name: &str| assets.iter().position(|a| a.name.eq_ignore_ascii_case(name));
    let mut out = vec![SubmeshBinding::default(); n_submeshes];
    let mut prov = vec![Prov::Unresolved; n_submeshes];
    // Per-slot path (new-shape sidecar).
    for (i, slot) in sc.slots.iter().take(n_submeshes).enumerate() {
        out[i].slots[SubmeshBinding::DIFFUSE] = slot.diffuse.as_deref().and_then(find);
        out[i].slots[SubmeshBinding::SPEC] = slot.spec.as_deref().and_then(find);
        out[i].slots[SubmeshBinding::NORMAL] = slot.normal.as_deref().and_then(find);
        out[i].slots[SubmeshBinding::MASK] = slot.mask.as_deref().and_then(find);
    }
    // Legacy migration: a pre-widening sidecar has no `slots`, only `submeshes` (diffuse-only).
    if sc.slots.is_empty() {
        for (i, name) in sc.submeshes.iter().take(n_submeshes).enumerate() {
            out[i].slots[SubmeshBinding::DIFFUSE] = name.as_deref().and_then(find);
        }
    }
    for (i, b) in out.iter().enumerate() {
        prov[i] = match b.is_bound() {
            false => Prov::Unresolved,
            true if sc.bound.get(i).copied().unwrap_or(true) => Prov::Bound,
            true => Prov::Seeded,
        };
    }
    Some((out, prov))
}

/// Write the current per-slot assignment — and how each submesh was arrived at — to the sidecar.
pub fn save_sidecar(
    profile: &str,
    binding: &[SubmeshBinding],
    prov: &[Prov],
    assets: &[TexAsset],
) -> Result<(), String> {
    let name_of = |idx: Option<usize>| idx.and_then(|i| assets.get(i)).map(|a| a.name.clone());
    let sc = Sidecar {
        // Do not write the legacy field. `skip_serializing_if` keeps the file clean; readers still
        // accept it for old files.
        submeshes: Vec::new(),
        slots: binding
            .iter()
            .map(|b| SlotSidecar {
                diffuse: name_of(b.diffuse()),
                spec: name_of(b.spec()),
                normal: name_of(b.normal()),
                mask: name_of(b.mask()),
            })
            .collect(),
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
