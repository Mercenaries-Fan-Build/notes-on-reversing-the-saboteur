//! zlib (RFC 1950) streams.
//!
//! The Saboteur's MSHA blobs, DTEX mip streams and ALBS blobs are standard zlib (`78 01` headers), each
//! with a declared inflated size. [`inflate_exact`] returns an error when a stream inflates to a
//! different length or leaves bytes after its end.
//!
//! [`deflate`] output is valid zlib but not byte-identical to the 2009 encoder's
//! (`docs/formats/dtex_texture.md`).

use std::io::{Read, Write};

use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use flate2::Compression;

/// Inflate one complete zlib stream that must produce exactly `expected` bytes and consume all of
/// `data`. `field` names the stream in errors.
pub fn inflate_exact(data: &[u8], expected: usize, field: &str) -> Result<Vec<u8>, String> {
    let mut dec = ZlibDecoder::new(data);
    let mut out = Vec::with_capacity(expected);
    dec.read_to_end(&mut out).map_err(|e| format!("{field}: zlib inflate failed: {e}"))?;
    if out.len() != expected {
        return Err(format!("{field}: inflated to {} bytes, the format declares {expected}", out.len()));
    }
    let consumed = dec.total_in() as usize;
    if consumed != data.len() {
        return Err(format!(
            "{field}: zlib stream ends after {consumed} of {} bytes ({} trailing)",
            data.len(),
            data.len() - consumed
        ));
    }
    Ok(out)
}

/// Deflate `data` into one zlib stream at `level` (0–9).
pub fn deflate(data: &[u8], level: u32) -> Vec<u8> {
    let mut enc = ZlibEncoder::new(Vec::new(), Compression::new(level));
    enc.write_all(data).expect("writing into a Vec cannot fail");
    enc.finish().expect("finishing into a Vec cannot fail")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_checks_lengths() {
        let data: Vec<u8> = (0..5000u32).map(|i| (i % 251) as u8).collect();
        let z = deflate(&data, 9);
        assert_eq!(inflate_exact(&z, data.len(), "t"), Ok(data.clone()));

        let e = inflate_exact(&z, data.len() + 1, "blob").unwrap_err();
        assert!(e.contains("blob") && e.contains("declares"), "{e}");

        let mut padded = z.clone();
        padded.extend_from_slice(&[0, 0, 0]);
        let e = inflate_exact(&padded, data.len(), "blob").unwrap_err();
        assert!(e.contains("3 trailing"), "{e}");

        assert!(inflate_exact(&[0x78, 0x01, 0xFF], 1, "junk").is_err());
    }
}
