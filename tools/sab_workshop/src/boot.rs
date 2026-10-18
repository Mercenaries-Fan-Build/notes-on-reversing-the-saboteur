//! The models the app shows: assembled from a build's parts, the build's player on startup, or a
//! mesh from loose files (`--mesh`, with `--skel` or rigged on the player).

use crate::assets::{self, Asset, Assembly};
use crate::formats::{self, Bone};
use crate::meshload::{self, LoadedMesh};
use crate::source::GameSource;
use crate::Config;

/// A model ready to show and export.
pub struct Model {
    pub name: String,
    pub lm: LoadedMesh,
    /// The mesh items assembled, with their levels of detail; empty for a model from loose files.
    pub picks: Vec<(usize, u8)>,
    /// What the assembly could not include: model references the build does not have, and parts that
    /// did not decode.
    pub notes: Vec<String>,
    /// The asset's damage parts, which an export re-applies.
    pub damage: Vec<(u32, i32)>,
}

/// Assemble `asm` from `src`'s parts.
pub fn assemble(src: &dyn GameSource, name: &str, asm: &Assembly) -> Result<Model, String> {
    let meshes = src.meshes();
    let (mut lm, skipped) = meshload::assemble_with(&asm.picks, |i| src.load_part(i), |i| meshes[i].name.clone())?;
    meshload::apply_damage(&mut lm, &asm.damage);
    let mut notes = asm.notes.clone();
    notes.extend(skipped.into_iter().map(|s| format!("part not decoded: {s}")));
    Ok(Model { name: name.to_string(), lm, picks: asm.picks.clone(), notes, damage: asm.damage.clone() })
}

/// The bone name hash of a rig bone read from a `.skel`: the hash form `bone_XXXXXXXX` names its hash,
/// any other name is hashed.
fn hash_of(name: &str) -> u32 {
    match name.strip_prefix("bone_").filter(|h| h.len() == 8).map(|h| u32::from_str_radix(h, 16)) {
        Some(Ok(h)) => h,
        _ => sab_formats::pandemic_hash(name),
    }
}

/// The startup model: `--mesh` (with `--skel`, or rigged on the build's player), else the asset
/// `--boot` names, else the build's player.
pub fn load(cfg: &Config, src: &dyn GameSource, all: &[Asset]) -> Result<Model, String> {
    let character = |want: &str| -> Result<Model, String> {
        let i = if want.is_empty() {
            assets::player(src.templates(), all)?
        } else {
            let lower = want.to_ascii_lowercase();
            all.iter()
                .position(|a| a.name.to_ascii_lowercase().contains(&lower))
                .ok_or_else(|| format!("--boot {want}: no asset of this build is named like that"))?
        };
        assemble(src, &all[i].name, &all[i].assembly())
    };
    let Some(mesh_path) = &cfg.mesh else {
        if cfg.skel.is_some() {
            return Err("--skel names a rig for --mesh, which was not given".into());
        }
        return character(&cfg.boot_model);
    };
    let mut mesh = std::fs::read(mesh_path).map_err(|e| format!("read {mesh_path}: {e}")).and_then(|b| formats::read_smsh(&b))?;
    let name = std::path::Path::new(mesh_path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "model".into());
    let (bones, bone_hashes): (Vec<Bone>, Vec<u32>) = match &cfg.skel {
        Some(skel_path) => {
            let text = std::fs::read_to_string(skel_path).map_err(|e| format!("read {skel_path}: {e}"))?;
            let bones = formats::read_skel(&text);
            if bones.is_empty() {
                return Err(format!("{skel_path} parsed to 0 bones"));
            }
            let hashes = bones.iter().map(|b| hash_of(&b.name)).collect();
            (bones, hashes)
        }
        // A ported character is skinned to the game's own skeleton, so the player's rig is its rig.
        None => {
            let rig = character("")?;
            println!("[sab_workshop] {name}: rigged on {} from the build", rig.name);
            (rig.lm.bones, rig.lm.bone_hashes)
        }
    };
    // Accessories are authored at the origin and parented by drawcall; place them on the rig.
    formats::bind_rigid_attachments(&mut mesh, &crate::skinning::bind_world(&bones));
    let lm = LoadedMesh {
        name: name.clone(),
        prim_parent_bone: mesh.prims.iter().map(|p| p.parent_bone).collect(),
        mesh,
        bones,
        bone_hashes,
        stored_ibm: Default::default(),
        part_ranges: Vec::new(),
        uvless: Vec::new(),
    };
    Ok(Model { name, lm, picks: Vec::new(), notes: Vec::new(), damage: Vec::new() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_skel_bone_hashes_by_its_name_or_names_its_hash() {
        assert_eq!(hash_of("bone_1A2B3C4D"), 0x1A2B_3C4D);
        assert_eq!(hash_of("Bone_Head"), sab_formats::pandemic_hash("Bone_Head"));
    }

    /// Both builds boot their player, skinned, with every joint on the rig and every part decoded.
    #[cfg(feature = "retail")]
    #[test]
    fn both_builds_boot_their_player() {
        for src in crate::testenv::both_builds() {
            let all = assets::build(src);
            let cfg = Config::from_settings(crate::settings::Settings::default());
            let b = load(&cfg, src, &all).unwrap_or_else(|e| panic!("{}: {e}", src.label()));
            assert_eq!(b.name, "FBS_RS_Sean", "{}", src.label());
            assert!(b.notes.is_empty(), "{}: {:?}", src.label(), b.notes);
            assert!(b.lm.mesh.positions.len() > 1000, "{}: a whole character, got {} vertices", src.label(), b.lm.mesh.positions.len());
            assert!(b.lm.mesh.weights.iter().any(|w| w[0] + w[1] + w[2] + w[3] > 0.0001), "{}: no skin weights", src.label());
            let max_joint = b.lm.mesh.joints.iter().flat_map(|j| j.iter()).copied().max().unwrap_or(0);
            assert!((max_joint as usize) < b.lm.bones.len(), "{}: joint {max_joint} off the rig", src.label());
        }
    }
}
