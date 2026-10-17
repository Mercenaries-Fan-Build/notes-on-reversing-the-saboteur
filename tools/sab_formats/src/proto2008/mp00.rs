//! `MP00` — the 2008 `mega*.pack` archive: a directory of [`super::sbl`] sub-packs.
//!
//! ```text
//! +0x00  char   magic[4] = "MP00"
//! +0x04  u32    count
//! +0x08  Entry  entries[count]       16 bytes each
//! 8+16*count    sub-packs, back to back in entry order; the file ends after the last
//!
//! Entry (16 bytes)
//!   +0x00  u32  hash
//!   +0x04  u32  size     sub-pack byte length
//!   +0x08  u64  offset   absolute offset of the sub-pack
//! ```
//!
//! The archives are up to 2.1 GB, so [`parse_index`] reads only the header and directory and
//! [`check_layout`] checks the entries against the file length.

use crate::bytes;

/// The magic, in on-disk byte order.
pub const MAGIC: &[u8; 4] = b"MP00";
/// Directory entry length.
pub const ENTRY_LEN: usize = 16;

/// One directory entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    pub hash: u32,
    pub size: u32,
    pub offset: u64,
}

/// The number of bytes [`parse_index`] needs: the header and the whole directory. `head` must hold at
/// least the 8-byte header.
pub fn index_len(head: &[u8]) -> Result<usize, String> {
    let magic = bytes::slice(head, 0, 4, "MP00 magic")?;
    if magic != MAGIC {
        return Err(format!("MP00: magic {magic:02X?}, expected {MAGIC:02X?}"));
    }
    let count = bytes::u32_be(head, 4, "MP00 count")? as usize;
    count
        .checked_mul(ENTRY_LEN)
        .and_then(|n| n.checked_add(8))
        .ok_or_else(|| format!("MP00 count {count} overflows"))
}

/// Parse the directory from the first [`index_len`] bytes of the archive.
pub fn parse_index(head: &[u8]) -> Result<Vec<Entry>, String> {
    let end = index_len(head)?;
    bytes::slice(head, 0, end, "MP00 directory")?;
    let count = (end - 8) / ENTRY_LEN;
    let mut entries = Vec::with_capacity(count);
    for i in 0..count {
        let p = 8 + i * ENTRY_LEN;
        let offset = (bytes::u32_be(head, p + 8, "MP00 offset hi")? as u64) << 32
            | bytes::u32_be(head, p + 12, "MP00 offset lo")? as u64;
        entries.push(Entry {
            hash: bytes::u32_be(head, p, "MP00 hash")?,
            size: bytes::u32_be(head, p + 4, "MP00 size")?,
            offset,
        });
    }
    Ok(entries)
}

/// Check that the sub-packs tile `[8 + 16*count, file_len)` in entry order.
pub fn check_layout(entries: &[Entry], file_len: u64) -> Result<(), String> {
    let mut cursor = 8 + (entries.len() * ENTRY_LEN) as u64;
    for (i, e) in entries.iter().enumerate() {
        if e.offset != cursor {
            return Err(format!("MP00 entry {i}: offset 0x{:X}, expected 0x{cursor:X} (sub-packs are contiguous)", e.offset));
        }
        cursor += e.size as u64;
    }
    if cursor != file_len {
        return Err(format!("MP00: sub-packs end at 0x{cursor:X}, the file is 0x{file_len:X} bytes"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(subpacks: &[(u32, &[u8])]) -> Vec<u8> {
        let mut out = MAGIC.to_vec();
        out.extend_from_slice(&(subpacks.len() as u32).to_be_bytes());
        let mut off = (8 + subpacks.len() * ENTRY_LEN) as u64;
        for (h, data) in subpacks {
            out.extend_from_slice(&h.to_be_bytes());
            out.extend_from_slice(&(data.len() as u32).to_be_bytes());
            out.extend_from_slice(&off.to_be_bytes());
            off += data.len() as u64;
        }
        for (_, data) in subpacks {
            out.extend_from_slice(data);
        }
        out
    }

    #[test]
    fn parses_index_and_checks_layout() {
        let buf = build(&[(0xC78A_97C7, b"SBLX...."), (0x7875_8D13, b"SBL8")]);
        assert_eq!(index_len(&buf[..8]), Ok(8 + 2 * 16));
        let es = parse_index(&buf[..40]).unwrap();
        assert_eq!(es[0], Entry { hash: 0xC78A_97C7, size: 8, offset: 40 });
        assert_eq!(es[1], Entry { hash: 0x7875_8D13, size: 4, offset: 48 });
        assert_eq!(check_layout(&es, buf.len() as u64), Ok(()));
        assert!(check_layout(&es, buf.len() as u64 + 1).unwrap_err().contains("end at"));
        let mut gap = es.clone();
        gap[1].offset += 1;
        assert!(check_layout(&gap, buf.len() as u64 + 1).unwrap_err().contains("contiguous"));
        assert!(parse_index(&buf[..39]).is_err());
        assert!(index_len(b"SBL4\0\0\0\x01").unwrap_err().contains("magic"));
    }
}
