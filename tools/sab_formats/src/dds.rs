//! `DDS ` files (D3D9 `DDS_HEADER`, little-endian), as the 2008 prototype ships loose under `Bin360/`.
//!
//! ```text
//! +0    char  magic[4] = "DDS "
//! +4    u32   header size = 124
//! +8    u32   flags                   0x20000 = mip count valid
//! +12   u32   height
//! +16   u32   width
//! +28   u32   mip count
//! +80   u32   pixel-format flags      0x4 = four-cc
//! +84   u32   four-cc                 DXT1..DXT5, ATI2
//! +88   u32   RGB bit count; +92.. R, G, B, A masks
//! +112  u32   caps2                   0x200 = cube map (faces 0xFC00), 0x200000 = volume
//! +128        surfaces: per face, its mip chain largest first, no padding
//! ```

use crate::bytes;
use crate::pixels::Format;

const DDSD_MIPMAPCOUNT: u32 = 0x2_0000;
const DDSCAPS2_CUBEMAP: u32 = 0x200;
const DDSCAPS2_CUBEMAP_FACES: u32 = 0xFC00;
const DDSCAPS2_VOLUME: u32 = 0x20_0000;

/// A parsed file; `base` borrows the caller's bytes.
#[derive(Clone, Debug)]
pub struct Dds<'a> {
    pub width: u32,
    pub height: u32,
    pub mips: u32,
    /// 6 for a cube map, otherwise 1.
    pub faces: u32,
    pub format: Format,
    /// The four-cc, or 0 for an uncompressed file.
    pub four_cc: u32,
    /// Face 0's largest surface.
    pub base: &'a [u8],
    /// Every byte after the header.
    pub surfaces: &'a [u8],
}

impl<'a> Dds<'a> {
    /// The size, in texels, of mip level `level`.
    pub fn level_size(&self, level: u32) -> (u32, u32) {
        ((self.width >> level).max(1), (self.height >> level).max(1))
    }

    fn level_bytes(&self, level: u32) -> usize {
        let (w, h) = self.level_size(level);
        self.format.surface_bytes(w as usize, h as usize)
    }

    /// Bytes of every face's whole mip chain: what follows the header in a well-formed file.
    pub fn surfaces_len(&self) -> usize {
        self.faces as usize * (0..self.mips).map(|l| self.level_bytes(l)).sum::<usize>()
    }

    /// Mip level `level` of face `face`.
    pub fn level(&self, face: u32, level: u32) -> Result<&'a [u8], String> {
        if level >= self.mips || face >= self.faces {
            return Err(format!("DDS has {} faces of {} mips, face {face} level {level} asked for", self.faces, self.mips));
        }
        let chain: usize = (0..self.mips).map(|l| self.level_bytes(l)).sum();
        let at = face as usize * chain + (0..level).map(|l| self.level_bytes(l)).sum::<usize>();
        bytes::slice(self.surfaces, at, self.level_bytes(level), "DDS mip surface")
    }
}

const fn cc(s: &[u8; 4]) -> u32 {
    u32::from_le_bytes(*s)
}

/// Parse the header and slice the base surface.
pub fn parse(b: &[u8]) -> Result<Dds<'_>, String> {
    if bytes::slice(b, 0, 4, "DDS magic")? != b"DDS " {
        return Err("not a DDS file (no `DDS ` magic)".into());
    }
    let size = bytes::u32_le(b, 4, "DDS header size")?;
    if size != 124 {
        return Err(format!("DDS header size {size}, expected 124"));
    }
    let flags = bytes::u32_le(b, 8, "DDS flags")?;
    let height = bytes::u32_le(b, 12, "DDS height")?;
    let width = bytes::u32_le(b, 16, "DDS width")?;
    let mips = if flags & DDSD_MIPMAPCOUNT != 0 { bytes::u32_le(b, 28, "DDS mip count")?.max(1) } else { 1 };
    let caps2 = bytes::u32_le(b, 112, "DDS caps2")?;
    if caps2 & DDSCAPS2_VOLUME != 0 {
        return Err("DDS volume textures are not read".into());
    }
    let faces = if caps2 & DDSCAPS2_CUBEMAP != 0 {
        match caps2 & DDSCAPS2_CUBEMAP_FACES {
            DDSCAPS2_CUBEMAP_FACES => 6,
            partial => return Err(format!("DDS cube map with face bits 0x{partial:04X}; only all six faces are read")),
        }
    } else {
        1
    };
    let pf_flags = bytes::u32_le(b, 80, "DDS pixel-format flags")?;
    let four_cc = bytes::u32_le(b, 84, "DDS four-cc")?;
    let format = if pf_flags & 0x4 != 0 {
        match four_cc {
            x if x == cc(b"DXT1") => Format::Bc1,
            x if x == cc(b"DXT2") || x == cc(b"DXT3") => Format::Bc2,
            x if x == cc(b"DXT4") || x == cc(b"DXT5") => Format::Bc3,
            x if x == cc(b"ATI2") => Format::Bc5,
            other => return Err(format!("DDS four-cc 0x{other:08X} is not one this reader decodes")),
        }
    } else {
        let bits = bytes::u32_le(b, 88, "DDS bit count")?;
        let masks = [bytes::u32_le(b, 92, "DDS R mask")?, bytes::u32_le(b, 96, "DDS G mask")?, bytes::u32_le(b, 100, "DDS B mask")?, bytes::u32_le(b, 104, "DDS A mask")?];
        match (bits, masks) {
            (32, [0x00FF_0000, 0x0000_FF00, 0x0000_00FF, 0xFF00_0000]) => Format::A8r8g8b8,
            (32, [0x00FF_0000, 0x0000_FF00, 0x0000_00FF, 0]) => Format::X8r8g8b8,
            (16, [0xF800, 0x07E0, 0x001F, 0]) => Format::R5g6b5,
            (8, [0, 0, 0, 0xFF]) => Format::A8,
            (8, [0xFF, 0, 0, 0]) => Format::L8,
            other => return Err(format!("DDS uncompressed format {other:?} is not one this reader decodes")),
        }
    };
    let n = format.surface_bytes(width as usize, height as usize);
    let base = bytes::slice(b, 128, n, "DDS base surface")?;
    Ok(Dds { width, height, mips, faces, format, four_cc: if pf_flags & 0x4 != 0 { four_cc } else { 0 }, base, surfaces: &b[128..] })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(w: u32, h: u32, pf_flags: u32, four_cc: u32) -> Vec<u8> {
        let mut b = vec![0u8; 128];
        b[0..4].copy_from_slice(b"DDS ");
        b[4..8].copy_from_slice(&124u32.to_le_bytes());
        b[12..16].copy_from_slice(&h.to_le_bytes());
        b[16..20].copy_from_slice(&w.to_le_bytes());
        b[80..84].copy_from_slice(&pf_flags.to_le_bytes());
        b[84..88].copy_from_slice(&four_cc.to_le_bytes());
        b
    }

    #[test]
    fn parses_dxt5_and_rejects_short_files() {
        let mut b = header(8, 4, 4, cc(b"DXT5"));
        b.extend_from_slice(&[0u8; 32]);
        let d = parse(&b).unwrap();
        assert_eq!((d.width, d.height, d.format, d.base.len()), (8, 4, Format::Bc3, 32));
        b.truncate(b.len() - 1);
        assert!(parse(&b).is_err());
        assert!(parse(b"junk").is_err());
    }

    #[test]
    fn slices_cube_faces_and_mips() {
        let mut b = header(8, 8, 4, cc(b"DXT1"));
        b[8..12].copy_from_slice(&DDSD_MIPMAPCOUNT.to_le_bytes());
        b[28..32].copy_from_slice(&4u32.to_le_bytes());
        b[112..116].copy_from_slice(&(DDSCAPS2_CUBEMAP | DDSCAPS2_CUBEMAP_FACES).to_le_bytes());
        // Per face: 8x8 = 4 blocks, 4x4, 2x2, 1x1 = 1 block each -> 7 blocks of 8 bytes.
        for face in 0..6u8 {
            for level in 0..7u8 {
                b.extend_from_slice(&[face * 16 + level; 8]);
            }
        }
        let d = parse(&b).unwrap();
        assert_eq!((d.faces, d.mips, d.surfaces_len()), (6, 4, 6 * 56));
        let face2_base: Vec<u8> = (32..36u8).flat_map(|v| [v; 8]).collect();
        assert_eq!(d.level(2, 0).unwrap(), &face2_base[..]);
        assert_eq!(d.level(5, 3).unwrap(), &[86u8; 8][..]);
        assert!(d.level(6, 0).is_err());
    }
}
