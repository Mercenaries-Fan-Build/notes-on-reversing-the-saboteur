//! `SBL*` — the sub-pack in every 2008 `.Pack` file and every [`super::mp00`] entry. Layout:
//! `docs/2008_subpack_format.md`.
//!
//! ```text
//! +0x00  char    magic[4]       one of MAGICS
//! +0x04  u32     count
//! +0x08  Record  records[count]
//! 8+24*count     slot bytes, back to back in record order; the sub-pack ends after the last
//!
//! Record (24 bytes)
//!   +0x00  u32  kind        pandemic_hash of the slot type name (SlotKind)
//!   +0x04  u32  offset      absolute offset of the slot within the sub-pack
//!   +0x08  u32  size        slot byte length
//!   +0x0C  u32  size_dup    equal to size
//!   +0x10  u32  flag        1
//!   +0x14  u32  id          0, or a hash naming the slot (a `wsd` slot's layer, e.g.
//!                           pandemic_hash("HDV_Mission") in France/0.Pack)
//! ```
//!
//! Only `wsd` slots repeat within one sub-pack (one per layer).

use crate::bytes;

/// The sub-pack magics, in on-disk byte order.
pub const MAGICS: [&[u8; 4]; 7] = [b"SBL0", b"SBL4", b"SBL5", b"SBL6", b"SBL8", b"SBLL", b"SBLX"];
/// Directory record length.
pub const RECORD_LEN: usize = 24;
/// The value of every record's `flag` word.
pub const FLAG: u32 = 1;

/// A slot type: its record's `kind` is `pandemic_hash` of [`SlotKind::name`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SlotKind {
    Materials,
    Meshes,
    Physics,
    Textures,
    Wsd,
    PathGraphs,
    AiFences,
    FlashMovies,
}

impl SlotKind {
    /// Every slot type, in declaration order.
    pub const ALL: [SlotKind; 8] = [
        SlotKind::Materials,
        SlotKind::Meshes,
        SlotKind::Physics,
        SlotKind::Textures,
        SlotKind::Wsd,
        SlotKind::PathGraphs,
        SlotKind::AiFences,
        SlotKind::FlashMovies,
    ];

    /// The slot type name whose `pandemic_hash` is the record's `kind`.
    pub fn name(self) -> &'static str {
        match self {
            SlotKind::Materials => "materials",
            SlotKind::Meshes => "meshes",
            SlotKind::Physics => "physics",
            SlotKind::Textures => "textures",
            SlotKind::Wsd => "wsd",
            SlotKind::PathGraphs => "PathGraphs",
            SlotKind::AiFences => "AIFences",
            SlotKind::FlashMovies => "FlashMovies",
        }
    }

    /// The on-disk `kind` word.
    pub fn hash(self) -> u32 {
        match self {
            SlotKind::Materials => 0xD728_29F9,
            SlotKind::Meshes => 0xFE5E_3A56,
            SlotKind::Physics => 0x4445_EA18,
            SlotKind::Textures => 0xA40D_777D,
            SlotKind::Wsd => 0x9AB5_A351,
            SlotKind::PathGraphs => 0xE108_7B27,
            SlotKind::AiFences => 0xD309_8461,
            SlotKind::FlashMovies => 0xB5D2_FE96,
        }
    }

    /// The slot type a `kind` word names.
    pub fn from_hash(h: u32) -> Result<SlotKind, String> {
        SlotKind::ALL
            .into_iter()
            .find(|k| k.hash() == h)
            .ok_or_else(|| format!("SBL record kind 0x{h:08X} is not a known slot type"))
    }

    /// Whether one sub-pack may hold more than one slot of this type.
    pub fn repeats(self) -> bool {
        self == SlotKind::Wsd
    }
}

/// One directory record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Record {
    pub kind: SlotKind,
    pub offset: u32,
    pub size: u32,
    pub id: u32,
}

/// A parsed sub-pack; slots borrow the caller's bytes.
#[derive(Clone, Debug)]
pub struct SubPack<'a> {
    pub magic: [u8; 4],
    pub records: Vec<Record>,
    /// Slot bytes, one per record, in record order.
    pub slots: Vec<&'a [u8]>,
}

impl<'a> SubPack<'a> {
    /// The bytes of the slot of type `kind`, or `None` when the sub-pack has none. `kind` must be a
    /// type that does not [repeat](SlotKind::repeats); [`SubPack::slots_of`] reads those.
    pub fn slot(&self, kind: SlotKind) -> Option<&'a [u8]> {
        assert!(!kind.repeats(), "SubPack::slot({}) on a repeating slot type; use slots_of", kind.name());
        self.records.iter().position(|r| r.kind == kind).map(|i| self.slots[i])
    }

    /// Every slot of type `kind`, with its record, in record order.
    pub fn slots_of(&self, kind: SlotKind) -> impl Iterator<Item = (&Record, &'a [u8])> + '_ {
        self.records.iter().zip(self.slots.iter().copied()).filter(move |(r, _)| r.kind == kind)
    }
}

/// A sub-pack's directory, read without its slots.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Directory {
    pub magic: [u8; 4],
    pub records: Vec<Record>,
}

/// The length of a sub-pack's header and directory, from its first 8 bytes.
pub fn directory_len(head: &[u8]) -> Result<usize, String> {
    let magic = bytes::slice(head, 0, 4, "SBL magic")?;
    if !MAGICS.iter().any(|m| m[..] == *magic) {
        return Err(format!("SBL: magic {magic:02X?} is not one of {MAGICS:?}"));
    }
    let count = bytes::u32_be(head, 4, "SBL count")? as usize;
    count
        .checked_mul(RECORD_LEN)
        .and_then(|n| n.checked_add(8))
        .ok_or_else(|| format!("SBL count {count} overflows"))
}

/// Parse the header and directory (the first [`directory_len`] bytes) of a sub-pack of `total`
/// bytes. Every record must have `size == size_dup` and `flag == 1`, the slots must tile
/// `[8 + 24*count, total)` in record order, and only a [repeating](SlotKind::repeats) slot type may
/// occur twice.
pub fn parse_directory(dir: &[u8], total: u64) -> Result<Directory, String> {
    let dir_end = directory_len(dir)?;
    let count = (dir_end - 8) / RECORD_LEN;
    bytes::slice(dir, 8, dir_end - 8, &format!("SBL directory ({count} records)"))?;
    let magic: [u8; 4] = dir[..4].try_into().expect("4 bytes");
    let mut records = Vec::with_capacity(count);
    let mut cursor = dir_end as u64;
    for i in 0..count {
        let p = 8 + i * RECORD_LEN;
        let kind = SlotKind::from_hash(bytes::u32_be(dir, p, "SBL kind")?)?;
        let offset = bytes::u32_be(dir, p + 4, "SBL offset")?;
        let size = bytes::u32_be(dir, p + 8, "SBL size")?;
        let size_dup = bytes::u32_be(dir, p + 12, "SBL size_dup")?;
        let flag = bytes::u32_be(dir, p + 16, "SBL flag")?;
        let id = bytes::u32_be(dir, p + 20, "SBL id")?;
        if size != size_dup {
            return Err(format!("SBL record {i}: size 0x{size:X} != size_dup 0x{size_dup:X}"));
        }
        if flag != FLAG {
            return Err(format!("SBL record {i}: flag {flag}, expected {FLAG}"));
        }
        if offset as u64 != cursor {
            return Err(format!("SBL record {i}: offset 0x{offset:X}, expected 0x{cursor:X} (slots are contiguous)"));
        }
        if !kind.repeats() && records.iter().any(|r: &Record| r.kind == kind) {
            return Err(format!("SBL record {i}: a second {} slot", kind.name()));
        }
        records.push(Record { kind, offset, size, id });
        cursor += size as u64;
    }
    if cursor != total {
        return Err(format!("SBL: slots end at 0x{cursor:X}, the sub-pack is 0x{total:X} bytes"));
    }
    Ok(Directory { magic, records })
}

/// Parse a whole sub-pack: [`parse_directory`], then each record's slot bytes.
pub fn parse(buf: &[u8]) -> Result<SubPack<'_>, String> {
    let n = directory_len(buf)?;
    let dir = parse_directory(bytes::slice(buf, 0, n, "SBL directory")?, buf.len() as u64)?;
    let slots = dir.records.iter().map(|r| &buf[r.offset as usize..r.offset as usize + r.size as usize]).collect();
    Ok(SubPack { magic: dir.magic, records: dir.records, slots })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::pandemic_hash;

    #[test]
    fn slot_kind_hashes_are_the_hashes_of_their_names() {
        for k in SlotKind::ALL {
            assert_eq!(pandemic_hash(k.name()), k.hash(), "{}", k.name());
            assert_eq!(SlotKind::from_hash(k.hash()), Ok(k));
        }
        assert!(SlotKind::from_hash(0x1234_5678).is_err());
    }

    fn build(magic: &[u8; 4], slots: &[(SlotKind, &[u8])], id: u32) -> Vec<u8> {
        let mut out = magic.to_vec();
        out.extend_from_slice(&(slots.len() as u32).to_be_bytes());
        let mut off = 8 + slots.len() * RECORD_LEN;
        for (k, data) in slots {
            for w in [k.hash(), off as u32, data.len() as u32, data.len() as u32, FLAG, id] {
                out.extend_from_slice(&w.to_be_bytes());
            }
            off += data.len();
        }
        for (_, data) in slots {
            out.extend_from_slice(data);
        }
        out
    }

    #[test]
    fn parses_and_finds_slots() {
        let buf = build(b"SBL4", &[(SlotKind::Materials, b"WSAO...."), (SlotKind::Meshes, b"mesh")], 0xDEAD_BEEF);
        let sp = parse(&buf).unwrap();
        assert_eq!(&sp.magic, b"SBL4");
        assert_eq!(sp.slot(SlotKind::Materials), Some(&b"WSAO...."[..]));
        assert_eq!(sp.slot(SlotKind::Meshes), Some(&b"mesh"[..]));
        assert_eq!(sp.slot(SlotKind::Textures), None);
        assert_eq!(sp.records[1].offset, 8 + 2 * 24 + 8);
        assert!(sp.records.iter().all(|r| r.id == 0xDEAD_BEEF));
    }

    #[test]
    fn wsd_slots_repeat() {
        let buf = build(b"SBL4", &[(SlotKind::Wsd, b"base"), (SlotKind::Meshes, b"m"), (SlotKind::Wsd, b"layer")], 0);
        let sp = parse(&buf).unwrap();
        let wsd: Vec<&[u8]> = sp.slots_of(SlotKind::Wsd).map(|(_, s)| s).collect();
        assert_eq!(wsd, [&b"base"[..], &b"layer"[..]]);
    }

    #[test]
    fn rejects_each_broken_invariant() {
        let good = build(b"SBLL", &[(SlotKind::Wsd, &[0u8; 8])], 0);
        assert!(parse(&good).is_ok());
        assert!(parse(&[]).is_err());
        assert!(parse(b"MP00\0\0\0\x01").unwrap_err().contains("magic"));

        let mut trailing = good.clone();
        trailing.push(0);
        assert!(parse(&trailing).unwrap_err().contains("slots end"));

        let mut dup = good.clone();
        dup[8 + 12..8 + 16].copy_from_slice(&9u32.to_be_bytes());
        assert!(parse(&dup).unwrap_err().contains("size_dup"));

        let mut flag = good.clone();
        flag[8 + 16..8 + 20].copy_from_slice(&0u32.to_be_bytes());
        assert!(parse(&flag).unwrap_err().contains("flag"));

        let mut gap = good.clone();
        gap[8 + 4..8 + 8].copy_from_slice(&0x21u32.to_be_bytes());
        assert!(parse(&gap).unwrap_err().contains("contiguous"));

        let twice = build(b"SBL4", &[(SlotKind::Meshes, b"a"), (SlotKind::Meshes, b"b")], 0);
        assert!(parse(&twice).unwrap_err().contains("second meshes"));
    }

    #[test]
    fn the_directory_reads_from_a_prefix() {
        let buf = build(b"SBL6", &[(SlotKind::Textures, b"tex"), (SlotKind::Materials, b"mat!")], 0);
        let n = directory_len(&buf[..8]).unwrap();
        assert_eq!(n, 8 + 2 * RECORD_LEN);
        let d = parse_directory(&buf[..n], buf.len() as u64).unwrap();
        assert_eq!(d.records, parse(&buf).unwrap().records);
        assert!(parse_directory(&buf[..n], buf.len() as u64 + 1).unwrap_err().contains("slots end"));
    }
}
