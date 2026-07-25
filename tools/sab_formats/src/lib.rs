//! sab_formats — The Saboteur (2009) asset-format parsing library.
//!
//! The Saboteur ships on a Pandemic Studios engine that shares low-level primitives with
//! Mercenaries 2. Rather than duplicate them, we depend on the published `mercs2_formats`
//! crate for the pieces that are provably identical between the two games, and implement
//! everything Saboteur-specific here.
//!
//! ## Reused from `mercs2_formats` (proven identical)
//! * **Resource-name hash** — Saboteur's `pandemic_hash` (`Saboteur.exe FUN_00dc1e20`:
//!   FNV-1a/32, basis `0x811C9DC5`, prime `0x01000193`, per-byte `|0x20` case-fold,
//!   finalizer `^0x2A` then `*prime`) is byte-for-byte `mercs2_formats::hash::pandemic_hash_m2`.
//!   Re-exported below as [`pandemic_hash`]. Sanity: `pandemic_hash("ANY") == 0xED057225`.
//! * **SGES block codec** — `docs/formats/archive_and_models.md` records SGES as "same as
//!   Mercs 2, byte-identical"; reuse [`mercs2_formats::sges::decompress_sges`].
//! * **Bounds-checked cursor** — `mercs2_formats::safe_slice::SafeSlice` models engine pointer
//!   dereferences and returns an `AccessViolation` instead of panicking, which is exactly the
//!   failure a validator wants to surface. Re-exported as [`SafeSlice`] / [`AccessViolation`].
//!
//! ## Implemented here (Saboteur-specific containers/assets)
//! * [`megapack`] — outer `.megapack` archive (on-disk magic `00PM`; mounter `FUN_00e428c0`).
//! * [`sbla`]     — `SBLA`/`ALBS` per-asset sub-pack directory (loader `FUN_00658870`).
//! * [`dtex`]     — DTEX texture record (validated on 12,559 retail textures).
//!
//! Each module exposes a pure parser over `&[u8]` plus the structural facts a validator needs.

pub mod dtex;
pub mod editnodes;
pub mod gametemplates;
pub mod gametext;
pub mod megapack;
pub mod mesh;
pub mod sbla;
pub mod wwise;

/// Saboteur resource-name hash. Identical to `mercs2_formats::hash::pandemic_hash_m2`
/// (`Saboteur.exe FUN_00dc1e20`). Empty string hashes to 0.
pub use mercs2_formats::hash::pandemic_hash_m2 as pandemic_hash;

/// SGES segmented-deflate block decompressor. The Saboteur's SGES framing is byte-identical
/// to Mercenaries 2 (see `docs/formats/archive_and_models.md`).
pub use mercs2_formats::sges::decompress_sges;

/// Bounds-checked byte cursor that models engine pointer dereferences: OOB reads return an
/// [`AccessViolation`] rather than panicking.
pub use mercs2_formats::safe_slice::{AccessViolation, SafeSlice};

/// Little-endian scalar readers reused from `mercs2_formats` (panic on OOB; use [`SafeSlice`]
/// when you need the fallible, engine-faithful variant).
pub use mercs2_formats::ffcs::{read_f32_le, read_u16_le, read_u32_le};

#[cfg(test)]
mod foundation_tests {
    use super::*;

    /// The load-bearing reuse claim: `mercs2_formats`'s M2 hash reproduces Saboteur ground
    /// truth. `hash("ANY") == 0xED057225` is the sanity value pinned in
    /// `docs/formats/megapack_key_derivation.md`.
    #[test]
    fn pandemic_hash_matches_saboteur_ground_truth() {
        assert_eq!(pandemic_hash("ANY"), 0xED05_7225);
        assert_eq!(pandemic_hash(""), 0);
    }

    /// A real `Global\Dynamic0.megapack` key pair (resource "Act1_IntKey"): the engine's
    /// `path_crc = pandemic_hash("global\\<name>.dynpack")` and `name_crc = pandemic_hash(name)`.
    /// Values lifted from `tools/sab_megapack_key` (verified against the retail pack).
    #[test]
    fn megapack_key_derivation_reproduces() {
        assert_eq!(pandemic_hash("Act1_IntKey"), 0xB333_DA43); // name_crc
        assert_eq!(pandemic_hash("global\\Act1_IntKey.dynpack"), 0xD3EF_69E0); // path_crc
    }
}

#[cfg(test)]
mod codec_tests {
    use super::{gametemplates::GameTemplates, gametext::GameText, pandemic_hash};

    #[test]
    fn gametext_addui_hash() {
        // The load-bearing modding fact: a new UI id is keyed by pandemic_hash(dottedID).
        assert_eq!(pandemic_hash("A1M0_Text.TASK_RaceJavier"), 0xafc7_fd9c);
    }

    #[test]
    fn gametext_roundtrip_synthetic() {
        // Minimal hand-built file: header + 1 UI record + no DNEC; must round-trip byte-identical.
        let mut b = Vec::new();
        b.extend_from_slice(&5u32.to_le_bytes()); // version
        b.extend_from_slice(&1u32.to_le_bytes()); // count
        b.extend_from_slice(&3u32.to_le_bytes()); // total CU = "Hi\0" = 3
        b.extend_from_slice(b"TXTD");
        b.extend_from_slice(&pandemic_hash("X_Text.Hi").to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes()); // key_len (bare NUL)
        b.push(0);
        b.extend_from_slice(&3u16.to_le_bytes()); // str_len
        for u in "Hi".encode_utf16() { b.extend_from_slice(&u.to_le_bytes()); }
        b.extend_from_slice(&0u16.to_le_bytes()); // NUL terminator
        let gt = GameText::parse(&b).unwrap();
        assert_eq!(gt.records.len(), 1);
        assert!(gt.records[0].is_ui());
        assert_eq!(gt.records[0].text_string(), "Hi");
        assert_eq!(gt.write(), b);
    }

    #[test]
    fn gametemplates_roundtrip_synthetic() {
        let mut inner = Vec::new();
        // one template: unk1=0, unk2=1, name "T", type "Prop", 1 pair {Texture, hash}
        let name = b"T\0"; let ty = b"Prop\0";
        let mut body = Vec::new();
        body.extend_from_slice(&0u32.to_le_bytes());
        body.extend_from_slice(&1u32.to_le_bytes());
        body.extend_from_slice(&(name.len() as u32).to_le_bytes()); body.extend_from_slice(name);
        body.extend_from_slice(&(ty.len() as u32).to_le_bytes()); body.extend_from_slice(ty);
        body.extend_from_slice(&1u32.to_le_bytes()); // pair_count
        body.extend_from_slice(&pandemic_hash("Texture").to_le_bytes());
        body.extend_from_slice(&4u32.to_le_bytes());
        body.extend_from_slice(&pandemic_hash("FO_PT_Oak01_AB").to_le_bytes());
        inner.extend_from_slice(b"AULB");
        inner.extend_from_slice(&1u32.to_le_bytes());
        inner.extend_from_slice(&(body.len() as u32).to_le_bytes());
        inner.extend_from_slice(&body);
        let (gt, consumed) = GameTemplates::parse(&inner).unwrap();
        assert_eq!(consumed, inner.len());
        assert_eq!(gt.write(), inner);
        let (_, t) = gt.find("T").unwrap();
        assert_eq!(t.pair(pandemic_hash("Texture")).unwrap().as_u32(), Some(pandemic_hash("FO_PT_Oak01_AB")));
    }
}
