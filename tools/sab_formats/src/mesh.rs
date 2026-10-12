//! MSHA — the record wrapping a MESH resource (on-disk 4CC `AHSM`). Every type-0 blob of an ALBS
//! sub-pack ([`crate::sbla`]) is exactly one MSHA record.
//!
//! ```text
//! +0x00  char AHSM
//! +0x04  u32  unc0         MESH body inflated size
//! +0x08  u32  unc1         .dat (VB/IB) inflated size
//! +0x0C  u32  c0           MESH body stored size
//! +0x10  u32  c1           .dat stored size
//! +0x14  char name[256]    NUL-terminated
//! +0x114 body[c0], then dat[c1]; a blob is zlib unless its stored size equals its inflated size,
//!        in which case it is stored raw
//! ```

use crate::{bytes, zlib};

pub const MAGIC: &[u8; 4] = b"AHSM";
pub const HEADER: usize = 276;

/// One MSHA record, located in some buffer at `off`.
#[derive(Clone, Debug)]
pub struct Msha {
    /// Offset of the record (its `AHSM` magic) in the buffer it was found in.
    pub off: usize,
    pub name: String,
    pub unc0: u32,
    pub unc1: u32,
    pub c0: u32,
    pub c1: u32,
}

impl Msha {
    /// The record's total size: header, body and `.dat`.
    pub fn len(&self) -> usize {
        HEADER + self.c0 as usize + self.c1 as usize
    }
}

/// The MSHA record at `off` in `buf`. Its body and `.dat` must lie within `buf`.
pub fn parse_msha_at(buf: &[u8], off: usize) -> Result<Msha, String> {
    let h = bytes::slice(buf, off, HEADER, "MSHA header")?;
    if &h[0..4] != MAGIC {
        return Err(format!("MSHA at 0x{off:X}: magic {:02X?}", &h[0..4]));
    }
    let unc0 = bytes::u32_le(h, 4, "MSHA unc0")?;
    let unc1 = bytes::u32_le(h, 8, "MSHA unc1")?;
    let c0 = bytes::u32_le(h, 12, "MSHA c0")?;
    let c1 = bytes::u32_le(h, 16, "MSHA c1")?;
    let raw = &h[20..HEADER];
    let n = raw.iter().position(|&b| b == 0).ok_or_else(|| format!("MSHA at 0x{off:X}: name has no NUL"))?;
    let name = String::from_utf8(raw[..n].to_vec()).map_err(|_| format!("MSHA at 0x{off:X}: name is not UTF-8"))?;
    let m = Msha { off, name, unc0, unc1, c0, c1 };
    bytes::slice(buf, off, m.len(), &format!("MSHA '{}' body + .dat", m.name))?;
    Ok(m)
}

/// The MSHA record that is all of `blob` (a type-0 ALBS blob).
pub fn parse_msha(blob: &[u8]) -> Result<Msha, String> {
    let m = parse_msha_at(blob, 0)?;
    if m.len() != blob.len() {
        return Err(format!("MSHA '{}' is {} bytes, the blob is {}", m.name, m.len(), blob.len()));
    }
    Ok(m)
}

/// Every offset in `buf` where [`parse_msha_at`] accepts a record with a printable, non-empty name.
/// For buffers whose ALBS record table cannot be read (an object pack without its map counts); a
/// parsed ALBS gives the records directly as its type-0 blobs.
pub fn scan_msha(buf: &[u8]) -> Vec<Msha> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + HEADER <= buf.len() {
        if &buf[i..i + 4] == MAGIC {
            if let Ok(m) = parse_msha_at(buf, i) {
                if !m.name.is_empty() && m.name.bytes().all(|b| (0x20..0x7f).contains(&b)) {
                    out.push(m);
                }
            }
        }
        i += 1;
    }
    out
}

/// A blob stored `comp` bytes long that inflates to `unc`: raw when the two are equal, else zlib.
fn decode(src: &[u8], unc: usize, field: &str) -> Result<Vec<u8>, String> {
    if src.len() == unc {
        return Ok(src.to_vec());
    }
    zlib::inflate_exact(src, unc, field)
}

/// The MESH body and `.dat` of the record `m` found in `buf`, each exactly its declared size.
pub fn read_body_and_dat(buf: &[u8], m: &Msha) -> Result<(Vec<u8>, Vec<u8>), String> {
    let s0 = m.off + HEADER;
    let body_src = bytes::slice(buf, s0, m.c0 as usize, &format!("MSHA '{}' body", m.name))?;
    let dat_src = bytes::slice(buf, s0 + m.c0 as usize, m.c1 as usize, &format!("MSHA '{}' .dat", m.name))?;
    let body = decode(body_src, m.unc0 as usize, &format!("MSHA '{}' body", m.name))?;
    let dat = decode(dat_src, m.unc1 as usize, &format!("MSHA '{}' .dat", m.name))?;
    Ok((body, dat))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(name: &str, body: &[u8], unc0: u32, dat: &[u8], unc1: u32) -> Vec<u8> {
        let mut b = Vec::from(*MAGIC);
        for v in [unc0, unc1, body.len() as u32, dat.len() as u32] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        let mut n = [0u8; 256];
        n[..name.len()].copy_from_slice(name.as_bytes());
        b.extend_from_slice(&n);
        b.extend_from_slice(body);
        b.extend_from_slice(dat);
        b
    }

    #[test]
    fn stored_and_zlib_blobs_decode() {
        let body = b"the quick brown fox".repeat(20);
        let z = zlib::deflate(&body, 9);
        let rec = record("Mesh", &z, body.len() as u32, &[1, 2, 3, 4], 4);
        let m = parse_msha(&rec).unwrap();
        assert_eq!(m.name, "Mesh");
        let (b, d) = read_body_and_dat(&rec, &m).unwrap();
        assert_eq!(b, body);
        assert_eq!(d, [1, 2, 3, 4]);
    }

    #[test]
    fn a_blob_longer_than_its_record_or_a_size_mismatch_is_an_error() {
        let mut rec = record("M", &[9; 4], 4, &[], 0);
        rec.push(0);
        assert!(parse_msha(&rec).unwrap_err().contains("the blob is"));
        let rec = record("M", &[9; 4], 4, &[], 7);
        let m = parse_msha(&rec).unwrap();
        assert!(read_body_and_dat(&rec, &m).is_err(), ".dat declared 7 bytes, stored 0");
    }

    #[test]
    fn scan_finds_records_inside_a_larger_buffer() {
        let mut buf = vec![0u8; 10];
        buf.extend_from_slice(&record("Prop_A", &[1, 2], 2, &[], 0));
        buf.extend_from_slice(b"AHSM junk");
        let found = scan_msha(&buf);
        assert_eq!(found.len(), 1);
        assert_eq!((found[0].off, found[0].name.as_str()), (10, "Prop_A"));
    }
}
