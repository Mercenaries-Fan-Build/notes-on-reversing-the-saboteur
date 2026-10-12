//! The Saboteur's resource-name hash — `Saboteur.exe` `FUN_00dc1e20` (GOG build), reproduced exactly.
//!
//! ```text
//! 00dc1e20  mov   edx, [esp+4]        ; name
//!           test  edx, edx / je ret0  ; null  -> 0
//!           mov   cl, [edx]
//!           test  cl, cl  / je ret0   ; ""    -> 0
//!           mov   eax, 0x811C9DC5
//! 00dc1e33  movsx ecx, cl             ; the byte is SIGN-extended
//!           or    ecx, 0x20           ; case fold
//!           xor   ecx, eax
//!           imul  ecx, ecx, 0x01000193
//!           ...                       ; next byte until NUL
//!           xor   eax, 0x2A
//!           imul  eax, eax, 0x01000193
//! ```
//!
//! FNV-1a with a `|0x20` case fold and an `^0x2A` / `*prime` finaliser. Each byte is sign-extended, so
//! a byte `>= 0x80` contributes `0xFFFFFFxx`. [`pandemic_hash_bytes`] hashes raw game bytes;
//! [`pandemic_hash`] hashes a `&str`'s UTF-8 bytes, which equal the game's bytes only for ASCII.

const BASIS: u32 = 0x811C_9DC5;
const PRIME: u32 = 0x0100_0193;

/// `FUN_00dc1e20` over raw bytes. Empty input hashes to 0, as the engine returns 0 for `""`.
pub fn pandemic_hash_bytes(name: &[u8]) -> u32 {
    if name.is_empty() {
        return 0;
    }
    let mut h = BASIS;
    for &b in name {
        let c = (b as i8) as i32 as u32; // movsx
        h = ((c | 0x20) ^ h).wrapping_mul(PRIME);
    }
    (h ^ 0x2A).wrapping_mul(PRIME)
}

/// `FUN_00dc1e20` over a string's UTF-8 bytes. Equal to the engine's hash of the same name when the
/// name is ASCII; for anything else, hash the game's own bytes with [`pandemic_hash_bytes`].
pub fn pandemic_hash(name: &str) -> u32 {
    pandemic_hash_bytes(name.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ground truth pinned in `docs/formats/lua_scripts.md`.
    #[test]
    fn known_values() {
        assert_eq!(pandemic_hash("ANY"), 0xED05_7225);
        assert_eq!(pandemic_hash("any"), 0xED05_7225, "case-folded");
        assert_eq!(pandemic_hash(""), 0, "the engine returns 0 for an empty name");
        assert_eq!(pandemic_hash("P_Keys_KeyRing_A"), 0xF4B7_8198);
    }

    /// A byte >= 0x80 is sign-extended (`movsx` at 0x00dc1e33): the single byte 0xE9, expected value
    /// computed by hand.
    #[test]
    fn high_bytes_are_sign_extended() {
        let signed = ((0xFFFF_FFE9u32 | 0x20) ^ BASIS).wrapping_mul(PRIME);
        let want = (signed ^ 0x2A).wrapping_mul(PRIME);
        assert_eq!(pandemic_hash_bytes(&[0xE9]), want);
        let zero_extended = ((0x0000_00E9u32 | 0x20) ^ BASIS).wrapping_mul(PRIME);
        assert_ne!(pandemic_hash_bytes(&[0xE9]), (zero_extended ^ 0x2A).wrapping_mul(PRIME));
    }
}
