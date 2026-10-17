//! The directory at the start of a `meshes` or `textures` [`super::sbl`] slot.
//!
//! ```text
//! +0x00  u32     count
//! +0x04  Record  records[count]
//!
//! Record (24 bytes)
//!   +0x00  u32  hash
//!   +0x04  u32  offset     relative to the slot start
//!   +0x08  u32  stored     byte length in the slot
//!   +0x0C  u32  unpacked
//!   +0x10  u32  f10
//!   +0x14  u32  f14
//! ```

use crate::bytes;

/// Directory record length.
pub const RECORD_LEN: usize = 24;

/// One directory record, as stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Record {
    pub hash: u32,
    pub offset: u32,
    pub stored: u32,
    pub unpacked: u32,
    pub f10: u32,
    pub f14: u32,
}

/// Read the directory. Each record's `[offset, offset + stored)` must lie inside the slot.
pub fn parse(slot: &[u8]) -> Result<Vec<Record>, String> {
    let count = bytes::u32_be(slot, 0, "slot directory count")? as usize;
    let end = count
        .checked_mul(RECORD_LEN)
        .and_then(|n| n.checked_add(4))
        .ok_or_else(|| format!("slot directory count {count} overflows"))?;
    bytes::slice(slot, 0, end, &format!("slot directory ({count} records)"))?;
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let p = 4 + i * RECORD_LEN;
        let w = |k: usize, f: &str| bytes::u32_be(slot, p + 4 * k, f);
        let r = Record {
            hash: w(0, "slot record hash")?,
            offset: w(1, "slot record offset")?,
            stored: w(2, "slot record stored")?,
            unpacked: w(3, "slot record unpacked")?,
            f10: w(4, "slot record f10")?,
            f14: w(5, "slot record f14")?,
        };
        bytes::slice(slot, r.offset as usize, r.stored as usize, &format!("slot record {i} data"))?;
        out.push(r);
    }
    Ok(out)
}
