//! `ALBS` (`SBLA`) — the sub-pack inside a megapack entry, read by `FUN_00658870` (directory length
//! `FUN_0065a900`, descriptor `FUN_0065a470`, blob reads `FUN_00660e10`). Layout:
//! `docs/formats/sbla_subpack.md`.
//!
//! ```text
//! +0x00  "ALBS"
//! +0x04  u32 X                  byte length of the descriptor
//! +0x08  Descriptor[X]          only when X != 0
//! 8+X    Record[N]              N = sum of the nine type counts; grouped by type in TYPE_ORDER
//! tbl_end = 8 + X + 24*N
//! tbl_end  blobs, back to back in record order, each `comp` bytes; the sub-pack ends after the last
//!
//! Record (24 bytes): u32 hash; u32 offset; u32 comp; u32 uncomp; u32 f10; u32 f14
//!
//! Descriptor (FUN_0065a470):
//!   [heightfield]  u32 'HEI1' ("1IEH" on disk); u32 w; u32 h; f32 f0; f32 f1; u8[w*h]
//!   Tail           the map6 record tail (lists, the nine counts, A/B/C lists)
//! ```
//!
//! The nine type counts come from the descriptor when `X != 0`, otherwise from the object's record in
//! `global.map` / `France.map` (whose hash is the megapack entry's `index`).
//!
//! The engine reads the heightfield when the owning object's flag word (`+0x108`) has
//! `& 0x1C00 == 0x400` (`0x65a49a`); the heightfield is the one that begins with the `'HEI1'` tag, and
//! [`parse`] selects it by that tag, requiring the descriptor to consume exactly `X` bytes either way.

use crate::bytes;
use crate::map6::{parse_tail_at, Tail};

/// The magic, in on-disk byte order.
pub const MAGIC: &[u8; 4] = b"ALBS";
/// The heightfield tag, the FourCC `'HEI1'` stored as a little-endian u32 (`"1IEH"` on disk).
pub const HEIGHTFIELD_TAG: u32 = 0x4845_4931;
/// Record table order by type (`DAT_01121574`).
pub const TYPE_ORDER: [usize; 9] = [6, 7, 0, 2, 8, 4, 3, 1, 5];
/// The separately streamed type: its records' blobs are stored uncompressed (`comp == uncomp`).
pub const TYPE_STREAMED: usize = 8;

/// A terrain heightfield.
#[derive(Clone, Debug, PartialEq)]
pub struct Heightfield {
    pub w: u32,
    pub h: u32,
    pub f0: f32,
    pub f1: f32,
    pub data: Vec<u8>,
}

/// The descriptor block.
#[derive(Clone, Debug, PartialEq)]
pub struct Descriptor {
    pub heightfield: Option<Heightfield>,
    pub tail: Tail,
}

/// One 24-byte record, with the type its table position gives it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Record {
    pub kind: usize,
    pub hash: u32,
    pub offset: u32,
    pub comp: u32,
    pub uncomp: u32,
    pub f10: u32,
    pub f14: u32,
}

/// A parsed sub-pack; blobs borrow the caller's bytes.
#[derive(Clone, Debug)]
pub struct SubPack<'a> {
    pub descriptor: Option<Descriptor>,
    pub counts: [u32; 9],
    pub records: Vec<Record>,
    pub blobs: Vec<&'a [u8]>,
}

/// The descriptor length `X` at `+4`, after checking the magic.
pub fn descriptor_len(buf: &[u8]) -> Result<u32, String> {
    let magic = bytes::slice(buf, 0, 4, "ALBS magic")?;
    if magic != MAGIC {
        return Err(format!("not an ALBS sub-pack (magic {magic:02X?})"));
    }
    bytes::u32_le(buf, 4, "ALBS descriptor length")
}

fn parse_descriptor(buf: &[u8], x: usize) -> Result<Descriptor, String> {
    let end = 8 + x;
    let d = bytes::slice(buf, 0, end, "ALBS descriptor")?;
    let mut p = 8;
    let heightfield = if bytes::u32_le(d, 8, "descriptor tag")? == HEIGHTFIELD_TAG {
        let w = bytes::u32_le(d, 12, "heightfield w")?;
        let h = bytes::u32_le(d, 16, "heightfield h")?;
        let f0 = bytes::f32_le(d, 20, "heightfield f0")?;
        let f1 = bytes::f32_le(d, 24, "heightfield f1")?;
        let n = (w as usize).checked_mul(h as usize).ok_or("heightfield w*h overflows")?;
        let data = bytes::slice(d, 28, n, "heightfield data")?.to_vec();
        p = 28 + n;
        Some(Heightfield { w, h, f0, f1, data })
    } else {
        None
    };
    let (tail, q) = parse_tail_at(d, p)?;
    if q != end {
        return Err(format!("ALBS descriptor parses to 0x{:X} bytes, X is 0x{x:X}", q - 8));
    }
    Ok(Descriptor { heightfield, tail })
}

/// The offsets the records' `offset` fields hold: the running sum of `comp` over the preceding
/// records, not counting type-8 records.
pub fn expected_offsets(records: &[Record]) -> Vec<u32> {
    let mut sum = 0u32;
    records
        .iter()
        .map(|r| {
            let o = sum;
            if r.kind != TYPE_STREAMED {
                sum = sum.wrapping_add(r.comp);
            }
            o
        })
        .collect()
}

/// Parse a sub-pack. `map_counts` are the nine type counts from the object's map record; they are
/// required when the descriptor is empty (`X == 0`) and must match the descriptor's when it is not.
/// The records and blobs must account for every byte of `buf`.
pub fn parse<'a>(buf: &'a [u8], map_counts: Option<[u32; 9]>) -> Result<SubPack<'a>, String> {
    let x = descriptor_len(buf)? as usize;
    let descriptor = if x != 0 { Some(parse_descriptor(buf, x)?) } else { None };
    let counts = match (&descriptor, map_counts) {
        (Some(d), Some(m)) if d.tail.counts != m => {
            return Err(format!("descriptor counts {:?} differ from the map's {m:?}", d.tail.counts));
        }
        (Some(d), _) => d.tail.counts,
        (None, Some(m)) => m,
        (None, None) => {
            return Err("ALBS with an empty descriptor: its type counts are in global.map / France.map".into())
        }
    };
    let mut kinds = Vec::new();
    for &k in &TYPE_ORDER {
        kinds.extend(std::iter::repeat(k).take(counts[k] as usize));
    }
    let tbl = 8 + x;
    let mut records = Vec::with_capacity(kinds.len());
    for (i, &kind) in kinds.iter().enumerate() {
        let r = tbl + 24 * i;
        records.push(Record {
            kind,
            hash: bytes::u32_le(buf, r, "record hash")?,
            offset: bytes::u32_le(buf, r + 4, "record offset")?,
            comp: bytes::u32_le(buf, r + 8, "record comp")?,
            uncomp: bytes::u32_le(buf, r + 12, "record uncomp")?,
            f10: bytes::u32_le(buf, r + 16, "record f10")?,
            f14: bytes::u32_le(buf, r + 20, "record f14")?,
        });
    }
    let mut p = tbl + 24 * records.len();
    let mut blobs = Vec::with_capacity(records.len());
    for (i, r) in records.iter().enumerate() {
        blobs.push(bytes::slice(buf, p, r.comp as usize, &format!("record {i} (type {}) blob", r.kind))?);
        p += r.comp as usize;
    }
    if p != buf.len() {
        return Err(format!("ALBS: records and blobs end at 0x{p:X}, the sub-pack is 0x{:X} bytes", buf.len()));
    }
    for (i, (r, want)) in records.iter().zip(expected_offsets(&records)).enumerate() {
        if r.comp != 0 && r.offset != want {
            return Err(format!("record {i} (type {}): offset 0x{:X}, expected 0x{want:X}", r.kind, r.offset));
        }
    }
    Ok(SubPack { descriptor, counts, records, blobs })
}

/// One blob to write, with its record fields.
#[derive(Clone, Copy, Debug)]
pub struct Blob<'a> {
    pub kind: usize,
    pub hash: u32,
    pub uncomp: u32,
    pub f10: u32,
    pub f14: u32,
    /// The stored bytes; `comp` is their length.
    pub data: &'a [u8],
    /// The `offset` field of a record with no bytes, written as given (no rule determines it).
    pub empty_offset: u32,
}

impl<'a> SubPack<'a> {
    /// The records and blobs as [`Blob`]s, in table order.
    pub fn blobs(&self) -> Vec<Blob<'a>> {
        self.records
            .iter()
            .zip(&self.blobs)
            .map(|(r, d)| Blob {
                kind: r.kind,
                hash: r.hash,
                uncomp: r.uncomp,
                f10: r.f10,
                f14: r.f14,
                data: d,
                empty_offset: if r.comp == 0 { r.offset } else { 0 },
            })
            .collect()
    }
}

fn put_u32(o: &mut Vec<u8>, v: u32) {
    o.extend_from_slice(&v.to_le_bytes());
}

/// Lay out a sub-pack. `blobs` must be grouped by type in [`TYPE_ORDER`]; their per-type counts must
/// equal the descriptor's when there is one (with no descriptor, the counts belong in the object's map
/// record). `comp` is each blob's length and `offset` follows [`expected_offsets`] for every record
/// with bytes.
pub fn write(descriptor: Option<&Descriptor>, blobs: &[Blob]) -> Result<Vec<u8>, String> {
    let mut counts = [0u32; 9];
    let mut order_pos = 0usize;
    for (i, b) in blobs.iter().enumerate() {
        if b.kind >= 9 {
            return Err(format!("blob {i}: type {} out of range", b.kind));
        }
        let pos = TYPE_ORDER.iter().position(|&k| k == b.kind).expect("every type is in TYPE_ORDER");
        if pos < order_pos {
            return Err(format!("blob {i}: type {} after a later type in TYPE_ORDER", b.kind));
        }
        order_pos = pos;
        counts[b.kind] += 1;
    }
    let mut desc = Vec::new();
    if let Some(d) = descriptor {
        if d.tail.counts != counts {
            return Err(format!("blob counts {counts:?} differ from the descriptor's {:?}", d.tail.counts));
        }
        if let Some(h) = &d.heightfield {
            if h.data.len() != h.w as usize * h.h as usize {
                return Err(format!("heightfield {}x{} with {} bytes", h.w, h.h, h.data.len()));
            }
            put_u32(&mut desc, HEIGHTFIELD_TAG);
            put_u32(&mut desc, h.w);
            put_u32(&mut desc, h.h);
            desc.extend_from_slice(&h.f0.to_le_bytes());
            desc.extend_from_slice(&h.f1.to_le_bytes());
            desc.extend_from_slice(&h.data);
        }
        crate::map6::write_tail(&mut desc, &d.tail)?;
    }
    let records: Vec<Record> = blobs
        .iter()
        .map(|b| Record {
            kind: b.kind,
            hash: b.hash,
            offset: 0,
            comp: b.data.len() as u32,
            uncomp: b.uncomp,
            f10: b.f10,
            f14: b.f14,
        })
        .collect();
    let offsets = expected_offsets(&records);
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    put_u32(&mut out, desc.len() as u32);
    out.extend_from_slice(&desc);
    for ((r, b), off) in records.iter().zip(blobs).zip(offsets) {
        put_u32(&mut out, r.hash);
        put_u32(&mut out, if r.comp == 0 { b.empty_offset } else { off });
        put_u32(&mut out, r.comp);
        put_u32(&mut out, r.uncomp);
        put_u32(&mut out, r.f10);
        put_u32(&mut out, r.f14);
    }
    for b in blobs {
        out.extend_from_slice(b.data);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tail(counts: [u32; 9]) -> Tail {
        Tail { list1: vec![], list2: vec![], x: 0, counts, a: vec![], nb: 0, b: vec![], c: vec![] }
    }

    fn blob(kind: usize, data: &[u8]) -> Blob<'_> {
        Blob { kind, hash: 0x100 + kind as u32, uncomp: data.len() as u32 * 2, f10: 1, f14: 2, data, empty_offset: 0 }
    }

    #[test]
    fn round_trips_with_a_heightfield_descriptor() {
        let d = Descriptor {
            heightfield: Some(Heightfield { w: 2, h: 3, f0: 1.0, f1: -1.0, data: vec![9; 6] }),
            tail: tail([1, 0, 0, 0, 0, 0, 0, 1, 1]),
        };
        let blobs = [blob(7, b"ui"), blob(0, b"mesh!"), blob(8, b"raw")];
        let bytes = write(Some(&d), &blobs).unwrap();
        let sp = parse(&bytes, None).unwrap();
        assert_eq!(sp.descriptor.as_ref(), Some(&d));
        assert_eq!(sp.records.iter().map(|r| r.kind).collect::<Vec<_>>(), [7, 0, 8]);
        assert_eq!(sp.blobs, [&b"ui"[..], b"mesh!", b"raw"]);
        // The type-8 blob is not counted in the offsets that follow it; the first two are.
        assert_eq!(sp.records.iter().map(|r| r.offset).collect::<Vec<_>>(), [0, 2, 7]);
        assert_eq!(write(sp.descriptor.as_ref(), &sp.blobs()).unwrap(), bytes);
    }

    #[test]
    fn empty_descriptor_needs_map_counts() {
        let blobs = [blob(0, b"m"), blob(1, b"tex")];
        let bytes = write(None, &blobs).unwrap();
        assert!(parse(&bytes, None).unwrap_err().contains("global.map"));
        let sp = parse(&bytes, Some([1, 1, 0, 0, 0, 0, 0, 0, 0])).unwrap();
        assert_eq!(sp.blobs, [&b"m"[..], b"tex"]);
        assert!(parse(&bytes, Some([1, 0, 0, 0, 0, 0, 0, 0, 0])).unwrap_err().contains("end at"));
    }

    #[test]
    fn out_of_order_types_and_a_wrong_offset_are_errors() {
        assert!(write(None, &[blob(1, b"t"), blob(0, b"m")]).unwrap_err().contains("TYPE_ORDER"));
        let mut bytes = write(None, &[blob(0, b"m"), blob(1, b"tex")]).unwrap();
        bytes[8 + 24 + 4] = 9;
        assert!(parse(&bytes, Some([1, 1, 0, 0, 0, 0, 0, 0, 0])).unwrap_err().contains("expected 0x1"));
    }
}
