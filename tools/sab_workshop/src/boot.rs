//! What the app shows on startup, resolved from the game install.
//!
//! The workshop used to boot from two files this repo generated — `output/skeletons/sean_full.smsh`
//! (a merged mesh) and `output/skeletons/CH_AL_SeanDevlin.skel` — and refused to start without them.
//! That made a released build unusable: it asked for build artefacts nobody downloading it has, to
//! show a character that is already sitting in `Dynamic0.megapack`. Nothing in either file is
//! anything but a re-encoding of pack bytes, and the app ALREADY reads those bytes — clicking a model
//! in the browser assembles it from the pack at runtime, mesh and rig together. So booting is just
//! that same path, run once before the window opens.
//!
//! [`load`] therefore prefers the install and falls back to files only when `--mesh`/`--skel` name
//! them explicitly (the Mattias/50-Cent port workflow, which inspects a mesh that is not in the pack
//! yet).

use crate::formats::{self, Bone, Smsh};
use crate::meshload::{self, MeshEntry};
use crate::pack;
use crate::Config;

/// The startup model: geometry, its rig, and — when it came from the pack — the parts it was
/// assembled from, which is what the WSAO texture resolve keys on.
pub struct BootModel {
    pub name: String,
    pub mesh: Smsh,
    pub bones: Vec<Bone>,
    /// The megapack entries this was assembled from. Empty for a file-loaded mesh, whose textures
    /// have to go through the legacy name-token path instead.
    pub parts: Vec<MeshEntry>,
    /// `(part name, index_start, index_count)` per part, in merge order.
    pub part_ranges: Vec<(String, u32, u32)>,
}

impl BootModel {
    /// True when this model came out of the game's own packs (so the engine's own material bindings
    /// can be used to texture it).
    pub fn from_pack(&self) -> bool {
        !self.parts.is_empty()
    }
}

/// The startup model: explicit `--mesh`/`--skel` files if given, else a character from the install.
pub fn load(cfg: &Config) -> Result<BootModel, String> {
    match (&cfg.mesh, &cfg.skel) {
        (Some(mesh), Some(skel)) => from_files(mesh, skel),
        // A loose mesh with no rig file: rig it from the INSTALL. A ported character (`sab_poc`) is
        // skinned to the game's own skeleton — that is what makes it a port — so the rig it needs is
        // already in the megapack, and asking for a generated `.skel` alongside it was asking for a
        // copy of something the user already has.
        (Some(mesh), None) => {
            let mut m = from_file_mesh(mesh)?;
            let rig = from_game(cfg)?;
            crate::formats::bind_rigid_attachments(
                &mut m.mesh,
                &crate::skinning::bind_world(&rig.bones),
            );
            println!("[sab_workshop] {}: rigged on {} from the install", m.name, rig.name);
            m.bones = rig.bones;
            Ok(m)
        }
        (None, Some(_)) => Err("--skel names a rig for --mesh, which was not given".into()),
        (None, None) => from_game(cfg),
    }
}

/// Load an SMSH from disk. The rig is left empty for the caller to supply.
fn from_file_mesh(mesh_path: &str) -> Result<BootModel, String> {
    let mesh = std::fs::read(mesh_path)
        .map_err(|e| format!("read {mesh_path}: {e}"))
        .and_then(|b| formats::read_smsh(&b))?;
    let name = std::path::Path::new(mesh_path)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "model".into());
    let n = mesh.indices.len() as u32;
    Ok(BootModel {
        name: name.clone(),
        mesh,
        bones: Vec::new(),
        parts: Vec::new(),
        part_ranges: vec![(name, 0, n)],
    })
}

/// Load a mesh + rig from loose files (`--mesh` / `--skel`).
fn from_files(mesh_path: &str, skel_path: &str) -> Result<BootModel, String> {
    let mut m = from_file_mesh(mesh_path)?;
    let text = std::fs::read_to_string(skel_path).map_err(|e| format!("read {skel_path}: {e}"))?;
    let bones = formats::read_skel(&text);
    if bones.is_empty() {
        return Err(format!("{skel_path} parsed to 0 bones"));
    }
    // Accessories are authored at the origin and parented by drawcall; place them now that the rig
    // is in hand. (`meshload` does this per part, so the pack path arrives already bound.)
    formats::bind_rigid_attachments(&mut m.mesh, &crate::skinning::bind_world(&bones));
    m.bones = bones;
    Ok(m)
}

/// Assemble a character straight out of `Dynamic0.megapack`.
fn from_game(cfg: &Config) -> Result<BootModel, String> {
    let mp = pack::Megapack::open(&cfg.megapack)?;
    let list = meshload::list_meshes(&mp);
    if list.is_empty() {
        return Err(format!("{}: no models found", cfg.megapack));
    }
    let (outfit, picked) = pick_character(&list, &cfg.boot_model)?;
    let lm = meshload::assemble(mp.raw(), &picked)?;
    // `assemble` skips parts it cannot decode, so report what actually went in, not what was asked
    // for — otherwise a silently-dropped part looks like a mesh bug later.
    let used: Vec<MeshEntry> = picked
        .iter()
        .filter(|e| lm.part_ranges.iter().any(|(n, _, _)| n == &e.name))
        .cloned()
        .collect();
    println!(
        "[sab_workshop] boot model {outfit} — {} part(s) from {}: {}",
        used.len(),
        cfg.megapack,
        used.iter().map(|e| e.name.as_str()).collect::<Vec<_>>().join(", ")
    );
    Ok(BootModel {
        // The OUTFIT, not `lm.name` — assembly names itself after whichever part happened to carry
        // the richest rig (for Sean, the glove), which is a confusing thing to call the whole figure.
        name: outfit,
        mesh: lm.mesh,
        bones: lm.bones,
        parts: used,
        part_ranges: lm.part_ranges,
    })
}

/// The outfit a part belongs to: everything up to and including the outfit number
/// (`CH_AL_SeanDevlin_01_UB` → `CH_AL_SeanDevlin_01`, and so does `…_01_GR_2`).
///
/// Grouping on "the name minus its last token" instead would split `_GR_2` into its own character and
/// let a numbered variant look like a whole separate outfit.
fn outfit_key(name: &str) -> String {
    let toks: Vec<&str> = name.split('_').collect();
    // The first all-digit token past the prefix is the outfit index; everything after it is a part.
    let cut = toks
        .iter()
        .position(|t| !t.is_empty() && t.bytes().all(|c| c.is_ascii_digit()))
        .map(|i| i + 1)
        .unwrap_or(toks.len().saturating_sub(1).max(1));
    toks[..cut.min(toks.len())].join("_")
}

/// Pick one character's parts to boot with.
///
/// `want` is a case-insensitive name token (`--boot`), else the game's protagonist. Whichever outfit
/// is chosen, the parts run through the SAME slot rules the browser uses for a click
/// ([`crate::assets::Asset::assembly`]): one mesh per body slot, no LODs, and the head variants
/// (`_HD`/`_FM`/`_FX`) collapsed to one — stacking those buries three faces in one skull.
/// Returns the outfit name and its parts.
fn pick_character(list: &[MeshEntry], want: &str) -> Result<(String, Vec<MeshEntry>), String> {
    use std::collections::BTreeMap;

    // Group every character-looking mesh by outfit. `CH_` is the game's own character prefix.
    let mut groups: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, e) in list.iter().enumerate() {
        if e.name.len() > 3 && e.name[..3].eq_ignore_ascii_case("CH_") {
            groups.entry(outfit_key(&e.name)).or_default().push(i);
        }
    }
    if groups.is_empty() {
        return Err("no CH_* character meshes in the megapack".into());
    }
    let lower = want.to_ascii_lowercase();
    let chosen = groups
        .iter()
        .filter(|(k, _)| !lower.is_empty() && k.to_ascii_lowercase().contains(&lower))
        // Among matches, the outfit with the most parts is the assembled figure rather than a
        // one-off; BTreeMap order makes the tie-break deterministic.
        .max_by_key(|(_, v)| v.len())
        // No match (or no token): the richest character in the pack still beats failing to start.
        .or_else(|| groups.iter().max_by_key(|(_, v)| v.len()))
        .map(|(k, v)| (k.clone(), v.clone()))
        .ok_or("no character to boot with")?;

    let (key, idxs) = chosen;
    let parts: Vec<crate::assets::PartRef> = idxs
        .iter()
        .map(|&i| crate::assets::PartRef {
            mesh_index: i,
            name: list[i].name.clone(),
            label: crate::assets::part_label(&list[i].name),
        })
        .collect();
    let asset = crate::assets::Asset { name: key.clone(), category: "Characters", parts };
    let picked: Vec<MeshEntry> =
        asset.assembly().iter().filter_map(|&i| list.get(i).cloned()).collect();
    if picked.is_empty() {
        return Err(format!("{key}: no loadable parts"));
    }
    Ok((key, picked))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outfit_key_cuts_at_the_outfit_number() {
        assert_eq!(outfit_key("CH_AL_SeanDevlin_01_UB"), "CH_AL_SeanDevlin_01");
        assert_eq!(outfit_key("CH_AL_SeanDevlin_01_GR_2"), "CH_AL_SeanDevlin_01");
        assert_eq!(outfit_key("CH_AL_SeanDevlin_01_UB_LOD"), "CH_AL_SeanDevlin_01");
        // No outfit number: fall back to dropping the part token.
        assert_eq!(outfit_key("CH_MB_Generic_UB"), "CH_MB_Generic");
    }

    /// The whole point of this module: with only a game install — no `output/`, no flags — the app
    /// gets a rigged character. Booting must produce Sean, skinned, on the 191-bone biped rig.
    #[test]
    fn boots_from_a_bare_install() {
        let Some(s) = crate::settings::detected_retail() else {
            eprintln!("skip: no Saboteur install detected");
            return;
        };
        let cfg = Config::from_settings(s);
        let b = match from_game(&cfg) {
            Ok(b) => b,
            Err(e) => panic!("boot from install failed: {e}"),
        };
        eprintln!(
            "booted {}: {} verts, {} tris, {} bones, {} parts",
            b.name,
            b.mesh.positions.len(),
            b.mesh.indices.len() / 3,
            b.bones.len(),
            b.parts.len()
        );
        assert!(b.from_pack(), "boot model must know its pack parts");
        assert!(b.mesh.positions.len() > 1000, "expected a whole character, got a stub");
        // The 191-bone biped rig, plus whatever bones only a non-canonical part knows about (see
        // `meshload::assemble` — they are APPENDED, never interleaved).
        assert!(b.bones.len() >= 191, "Sean's biped rig, got {} bones", b.bones.len());
        assert!(
            b.name.to_ascii_lowercase().contains("sean"),
            "the protagonist should win the boot pick, got {}",
            b.name
        );
        // Skinned, and every joint index addressable on the rig it shipped with.
        assert!(b.mesh.weights.iter().any(|w| w[0] + w[1] + w[2] + w[3] > 0.0001), "no skin weights");
        let max_joint = b.mesh.joints.iter().flat_map(|j| j.iter()).copied().max().unwrap_or(0);
        assert!((max_joint as usize) < b.bones.len(), "joint {max_joint} off the rig");
    }
}
