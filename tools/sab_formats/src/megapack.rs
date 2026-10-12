//! `.megapack` / `.kiloPack` — the outer Saboteur archive (on-disk magic `00PM`), read by the streaming
//! mounter `FUN_00e428c0`.
//!
//! ```text
//! +0x00  char   magic[4] = "00PM"
//! +0x04  u32    count
//! +0x08  Entry  index[count]           20 bytes each
//!  ...   Pair   table2[count]          8 bytes each: {crc, index} of index[i], same order
//!  ...   0xCB to the next 2048 boundary
//!  ...   sub-pack i at index[i].offset, in index order, each followed by 0xCB to the next 2048
//!        boundary (the file ends on one)
//!
//! Entry (20 bytes)
//!   +0x00  u32  crc      pandemic_hash of the asset's pack path (`map6::pack_counts`,
//!                        `map6::streamblock_path`); the lookup key
//!   +0x04  u32  index    the id of a streamblock; for an object, its map record hash, except the
//!                        France cinematic packs, whose name's leading number it is
//!   +0x08  u32  size     sub-pack byte length
//!   +0x0C  u64  offset   absolute offset of the sub-pack
//! ```
//!
//! [`write`] lays a pack out exactly this way; it reproduces every retail `.megapack` / `.kiloPack`
//! byte for byte from its parsed entries and sub-packs.

use crate::bytes;

/// The magic, in on-disk byte order.
pub const MAGIC: &[u8; 4] = b"00PM";
/// Alignment of the data area and of every sub-pack.
pub const SECTOR: u64 = 2048;
/// The padding byte.
pub const PAD: u8 = 0xCB;

/// One index entry (20 bytes on disk).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    /// `pandemic_hash` of the pack path — the key the mounter looks entries up by.
    pub crc: u32,
    /// The id of a streamblock; for an object, its map record hash (see the module docs).
    pub index: u32,
    /// Sub-pack byte length.
    pub size: u32,
    /// Absolute file offset of the sub-pack.
    pub offset: u64,
}

/// A parsed megapack index (the sub-pack bytes stay in the caller's buffer).
#[derive(Clone, Debug)]
pub struct Megapack {
    pub entries: Vec<Entry>,
}

impl Megapack {
    /// Largest `offset + size` across entries.
    pub fn implied_end(&self) -> u64 {
        self.entries.iter().map(|e| e.offset + e.size as u64).max().unwrap_or(0)
    }
}

/// Parse the header, index and `table2`. A `table2` pair that is not its index entry's `{crc, index}`
/// is an error. Entry offsets and sizes are not checked against `buf` here; [`entry_slice`] does that.
pub fn parse(buf: &[u8]) -> Result<Megapack, String> {
    let magic = bytes::slice(buf, 0, 4, "megapack magic")?;
    if magic != MAGIC {
        return Err(format!("bad magic {magic:02X?} (expected {MAGIC:02X?} \"00PM\")"));
    }
    let count = bytes::u32_le(buf, 4, "megapack count")? as usize;
    let t2 = count
        .checked_mul(20)
        .and_then(|n| n.checked_add(8))
        .ok_or_else(|| format!("megapack count {count} overflows"))?;
    bytes::slice(buf, 8, count * 20 + count * 8, &format!("megapack index + table2 ({count} entries)"))?;
    let mut entries = Vec::with_capacity(count);
    for i in 0..count {
        let p = 8 + i * 20;
        entries.push(Entry {
            crc: bytes::u32_le(buf, p, "entry crc")?,
            index: bytes::u32_le(buf, p + 4, "entry index")?,
            size: bytes::u32_le(buf, p + 8, "entry size")?,
            offset: bytes::u64_le(buf, p + 12, "entry offset")?,
        });
    }
    for (i, e) in entries.iter().enumerate() {
        let p = t2 + i * 8;
        let pair = (bytes::u32_le(buf, p, "table2 crc")?, bytes::u32_le(buf, p + 4, "table2 index")?);
        if pair != (e.crc, e.index) {
            return Err(format!(
                "table2[{i}] = {{0x{:08X}, 0x{:08X}}}, index[{i}] = {{0x{:08X}, 0x{:08X}}}",
                pair.0, pair.1, e.crc, e.index
            ));
        }
    }
    Ok(Megapack { entries })
}

/// The sub-pack bytes of `e`, or an error naming the entry and the range.
pub fn entry_slice<'a>(buf: &'a [u8], e: &Entry) -> Result<&'a [u8], String> {
    let start = usize::try_from(e.offset).map_err(|_| format!("entry crc 0x{:08X}: offset overflows", e.crc))?;
    bytes::slice(buf, start, e.size as usize, &format!("sub-pack crc 0x{:08X}", e.crc))
}

/// One sub-pack to write: its `crc`, `index` and bytes.
pub struct Item<'a> {
    pub crc: u32,
    pub index: u32,
    pub data: &'a [u8],
}

fn align_up(v: u64) -> u64 {
    v.div_ceil(SECTOR) * SECTOR
}

/// Lay out a megapack: index and `table2` in `items` order, the data area at the first 2048 boundary,
/// each sub-pack 2048-aligned in `items` order, every gap and the tail filled with [`PAD`].
pub fn write(items: &[Item]) -> Vec<u8> {
    let header_end = 8 + items.len() as u64 * 28;
    let mut offsets = Vec::with_capacity(items.len());
    let mut cursor = align_up(header_end);
    for it in items {
        offsets.push(cursor);
        cursor = align_up(cursor + it.data.len() as u64);
    }
    let mut out = Vec::with_capacity(cursor as usize);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(items.len() as u32).to_le_bytes());
    for (it, off) in items.iter().zip(&offsets) {
        out.extend_from_slice(&it.crc.to_le_bytes());
        out.extend_from_slice(&it.index.to_le_bytes());
        out.extend_from_slice(&(it.data.len() as u32).to_le_bytes());
        out.extend_from_slice(&off.to_le_bytes());
    }
    for it in items {
        out.extend_from_slice(&it.crc.to_le_bytes());
        out.extend_from_slice(&it.index.to_le_bytes());
    }
    for (it, off) in items.iter().zip(&offsets) {
        out.resize(*off as usize, PAD);
        out.extend_from_slice(it.data);
    }
    out.resize(cursor as usize, PAD);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_bad_magic() {
        assert!(parse(b"XXXX\0\0\0\0").is_err());
    }

    #[test]
    fn rejects_truncated_index() {
        let mut b = Vec::from(*MAGIC);
        b.extend_from_slice(&100u32.to_le_bytes());
        assert!(parse(&b).unwrap_err().contains("100 entries"));
    }

    #[test]
    fn write_then_parse_round_trips() {
        let a = vec![1u8; 3000];
        let b = vec![2u8; 10];
        let pack = write(&[Item { crc: 0xAA, index: 0xBB, data: &a }, Item { crc: 0xCC, index: 0xDD, data: &b }]);
        assert_eq!(pack.len() as u64 % SECTOR, 0);
        let mp = parse(&pack).unwrap();
        assert_eq!(mp.entries.len(), 2);
        assert_eq!(mp.entries[0].offset, SECTOR);
        assert_eq!(mp.entries[1].offset, 3 * SECTOR);
        assert_eq!(entry_slice(&pack, &mp.entries[0]).unwrap(), &a[..]);
        assert_eq!(entry_slice(&pack, &mp.entries[1]).unwrap(), &b[..]);
        assert!(pack[8 + 2 * 28..SECTOR as usize].iter().all(|&x| x == PAD));
    }

    #[test]
    fn a_table2_that_does_not_mirror_the_index_is_an_error() {
        let mut pack = write(&[Item { crc: 0xAA, index: 0xBB, data: &[0; 4] }]);
        pack[8 + 20] ^= 1;
        assert!(parse(&pack).unwrap_err().contains("table2[0]"));
    }

    #[test]
    fn an_entry_past_the_end_is_an_error() {
        let pack = write(&[Item { crc: 0xAA, index: 0xBB, data: &[0; 4] }]);
        let e = Entry { crc: 1, index: 2, size: 10, offset: pack.len() as u64 - 4 };
        assert!(entry_slice(&pack, &e).unwrap_err().contains("0x00000001"));
    }
}
