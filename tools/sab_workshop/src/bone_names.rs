//! Bone name-hash → name, for rigs read straight out of the game.
//!
//! A MESH stores only `pandemic_hash(boneName)`, never the string, so a rig loaded from a megapack
//! can only call its bones `bone_XXXXXXXX`. The `.skel` files in `output/` had real names because
//! `sab_skeleton` resolved them through a recovered dictionary — that dictionary is the only thing
//! those files carried that the pack does not, so it lives here now and the in-app path gets the
//! same names. COPIED from `tools/sab_skeleton/src/main.rs` (`known_bone_names` / `build_name_map`);
//! the misses are documented there and kept verbatim rather than re-guessed.
//!
//! Anything not in the dictionary keeps its hash form: the hash IS the identity the animation system
//! keys on, so `bone_XXXXXXXX` is a complete name, not a placeholder for a missing one.

use std::collections::HashMap;
use std::sync::OnceLock;

use sab_formats::pandemic_hash;

/// The recovered dictionary. Names were confirmed by hashing candidates against the real rig
/// (`sab_probe names`), not guessed; `Bone_LClav`/`Bone_RClav` are known misses (the rig calls them
/// shoulders) kept in case another character uses them.
fn known_bone_names() -> Vec<String> {
    let mut v: Vec<String> = [
        "GlobalSRT", "Bone_Attach_Root", "Bone_Root",
        "Bone_Hips", "Bone_Spine", "Bone_Spine1", "Bone_Spine2", "Bone_Chest",
        "Bone_Neck", "Bone_Neck1", "Bone_Head",
        "Bone_LThigh", "Bone_LShin", "Bone_LFoot", "Bone_LToe",
        "Bone_RThigh", "Bone_RShin", "Bone_RFoot", "Bone_RToe",
        "Bone_LClav", "Bone_LBicep", "Bone_LForearm", "Bone_LHand",
        "Bone_RClav", "Bone_RBicep", "Bone_RForearm", "Bone_RHand",
        "bone_attach_lhand", "bone_attach_rhand",
        "bone_cheek_left", "bone_cheek_right", "bone_jaw", "bone_tongue",
        "bone_eye_left", "bone_eye_right", "bone_eyelid_left", "bone_eyelid_right",
        "bone_lshoulder", "bone_rshoulder",
        "bone_chin", "bone_nostrilL", "bone_nostrilR", "bone_cheekL", "bone_cheekR",
        "bone_eyeL", "bone_eyeR", "bone_brow_left", "bone_brow_right",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    for side in ["L", "R"] {
        for f in ["Finger", "Thumb", "Index", "Middle", "Ring", "Pinky"] {
            for n in 0..4 {
                v.push(format!("Bone_{side}{f}{n}"));
            }
        }
    }
    v
}

fn map() -> &'static HashMap<u32, String> {
    static MAP: OnceLock<HashMap<u32, String>> = OnceLock::new();
    MAP.get_or_init(|| {
        let mut m = HashMap::new();
        for n in known_bone_names() {
            m.entry(pandemic_hash(&n)).or_insert(n);
        }
        m
    })
}

/// The bone's name from the dictionary, else from the hash → name table (`sab_formats::names`), else
/// its hash form (`bone_1A2B3C4D`).
pub fn name_for(hash: u32) -> String {
    map()
        .get(&hash)
        .cloned()
        .or_else(|| sab_formats::names::name(hash).map(str::to_string))
        .unwrap_or_else(|| format!("bone_{hash:08X}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The three root-chain bones every biped clip drives must resolve — `anim_sweep` filters the
    /// legitimately-travelling bones BY NAME, so a broken dictionary silently turns every clip bad.
    #[test]
    fn root_chain_resolves() {
        for n in ["GlobalSRT", "Bone_Root", "Bone_Hips"] {
            assert_eq!(name_for(pandemic_hash(n)), n, "dictionary lost {n}");
        }
    }

    #[test]
    fn unknown_hash_keeps_its_identity() {
        assert_eq!(name_for(0x1A2B_3C4D), "bone_1A2B3C4D");
    }
}
