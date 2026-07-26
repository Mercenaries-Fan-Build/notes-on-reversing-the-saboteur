//! `loosefiles_BinPC.pack` — the outer loose-file container.
//!
//! A flat sequence of `{ 128-byte header, payload, pad to 0x10 }`. The retail PC build ships exactly
//! one, `France/loosefiles_BinPC.pack`, holding 7 entries — among them the embedded
//! `France\EditNodes\EditNodes.pack` (see [`crate::editnodes`]) and `GameTemplates.wsd`.
//!
//! ```text
//! Header (128 bytes)
//!   +0x00  u32   name_crc   CRC-32/BZIP2 of the LOWERCASED name  (see `name_crc`)
//!   +0x04  u32   size       payload bytes; 0xFFFFFFFF = "open the real file from disk"
//!   +0x08  char  name[120]  NUL-terminated, backslash-separated, original case
//! payload[size], then pad to the next 0x10 boundary
//! ```
//!
//! **The reader is a sequential cursor, not a directory** (`Saboteur.exe FUN_00dc1060`): it reads the
//! header at the cursor, compares the requested path against `name` (case-insensitively, folding
//! `\` and `/`), and only on a *match* advances the cursor past the payload and its 0x10 padding. A
//! miss does not advance — it falls back to opening the file from disk. Consequences for writers are
//! in `docs/formats/editnodes.md`; the short version is that resizing a payload in place is safe
//! (every position is derived from `size` at read time, so there are no stored offsets to fix).

/// CRC-32/BZIP2: poly `0x04C11DB7`, init/xorout `0xFFFFFFFF`, **non-reflected** (MSB-first).
///
/// The same algorithm Mercenaries 2 uses for its save-game `ProfileHash`, which is how it was
/// identified — Saboteur's is over a name, Mercs 2's over the save body.
fn crc32_bzip2(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in data {
        crc ^= (b as u32) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 != 0 { (crc << 1) ^ 0x04C1_1DB7 } else { crc << 1 };
        }
    }
    !crc
}

/// The `+0x00` field of a loose-file header: `crc32_bzip2(name.to_ascii_lowercase())`.
///
/// Pass the name exactly as it is stored at `+0x08` — a backslash-separated relative path such as
/// `France\EditNodes\EditNodes.pack`. Only case is normalised; separators are hashed as given.
///
/// It is a hash of the **name**, not of the payload, so **editing or resizing an entry's contents
/// never invalidates it**. It only has to be computed when adding an entry or renaming one. The
/// engine never reads the field at all (exactly one reference to it in the whole executable, as the
/// destination of the header read), so a stale value is inert at runtime — but writing the real one
/// costs nothing and keeps the container self-consistent for any other tool.
pub fn name_crc(name: &str) -> u32 {
    crc32_bzip2(name.to_ascii_lowercase().as_bytes())
}

/// Byte size of one loose-file header.
pub const HEADER_SIZE: usize = 128;
/// Payloads are padded so the next header starts on a multiple of this.
pub const ALIGN: usize = 0x10;
/// A `size` of `0xFFFFFFFF` means "skip this header and open the real file from disk".
pub const SIZE_FROM_DISK: u32 = 0xFFFF_FFFF;

/// Offset of the first byte after an entry whose header starts at `header_off`, i.e. where the next
/// header begins: past the 128-byte header and `size` payload bytes, rounded up to [`ALIGN`].
pub fn next_header_offset(header_off: usize, size: usize) -> usize {
    (header_off + HEADER_SIZE + size + ALIGN - 1) & !(ALIGN - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Standard CRC-32/BZIP2 check value, so a refactor of the kernel is caught independently of the
    /// Saboteur data.
    #[test]
    fn matches_the_published_crc32_bzip2_check_value() {
        assert_eq!(crc32_bzip2(b"123456789"), 0xFC89_1918);
    }

    /// Every entry of the retail `France/loosefiles_BinPC.pack`, as (name, stored `+0x00`). These are
    /// the seven equations the algorithm was solved against — inlined so the test needs no game
    /// install.
    #[test]
    fn reproduces_every_retail_loose_file_crc() {
        let retail: [(&str, u32); 7] = [
            ("France.shaders", 0xee2e_6431),
            (r"Cinematics\Conversations\Conversations.cnvpack", 0x4635_0dcf),
            (r"Cinematics\Cinematics.cinpack", 0x0f3c_8f37),
            ("global.map", 0x6b37_a7cb),
            ("France.map", 0x25b9_6f25),
            (r"France\EditNodes\EditNodes.pack", 0xc965_b7ea),
            ("GameTemplates.wsd", 0x12cd_bde3),
        ];
        for (name, want) in retail {
            assert_eq!(name_crc(name), want, "{name}");
        }
    }

    /// Case is folded, separators are not: `global.map` is the only retail name already lowercase,
    /// which is what exposed the normalisation in the first place.
    #[test]
    fn folds_case_but_not_separators() {
        assert_eq!(name_crc("GameTemplates.wsd"), name_crc("gametemplates.wsd"));
        assert_eq!(name_crc("GameTemplates.wsd"), name_crc("GAMETEMPLATES.WSD"));
        assert_ne!(name_crc(r"France\EditNodes\EditNodes.pack"), name_crc("France/EditNodes/EditNodes.pack"));
    }

    #[test]
    fn next_header_offset_rounds_up_to_16() {
        assert_eq!(next_header_offset(0, 0), 128);
        assert_eq!(next_header_offset(0, 1), 144);
        assert_eq!(next_header_offset(0, 16), 144);
        assert_eq!(next_header_offset(0, 17), 160);
        // The retail first entry: France.shaders at 0, 23_066_269 bytes -> the second header at
        // 23_066_400, which is where the walk of the real file actually lands.
        assert_eq!(next_header_offset(0, 23_066_269), 23_066_400);
    }
}
