//! Texture surface decoding to straight-alpha RGBA8, for the D3D9 (PC, little-endian) layouts of the
//! block-compressed and uncompressed formats both builds use. Containers (`dtex`, DDS,
//! `proto2008::texture`) hand a surface's bytes here after undoing their own tiling and byte order.
//!
//! * BC1/BC2/BC3 — the Mercenaries 2 workshop's `texpng.rs` decoders (BC2 added for DXT3).
//! * BC4/BC5 — one or two BC3-alpha-style blocks per 4×4 tile; BC5 decodes to R = first block,
//!   G = second, B = 0.
//! * A8R8G8B8 / X8R8G8B8 — bytes B, G, R, A per texel; R5G6B5 — one little-endian u16; A8; L8.

/// A surface format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Bc1,
    Bc2,
    Bc3,
    Bc4,
    Bc5,
    A8r8g8b8,
    X8r8g8b8,
    R5g6b5,
    A8,
    L8,
}

impl Format {
    /// Bytes per 4×4 block (block formats) or per texel.
    pub fn unit_bytes(self) -> usize {
        match self {
            Format::Bc1 | Format::Bc4 => 8,
            Format::Bc2 | Format::Bc3 | Format::Bc5 => 16,
            Format::A8r8g8b8 | Format::X8r8g8b8 => 4,
            Format::R5g6b5 => 2,
            Format::A8 | Format::L8 => 1,
        }
    }

    pub fn is_block(self) -> bool {
        matches!(self, Format::Bc1 | Format::Bc2 | Format::Bc3 | Format::Bc4 | Format::Bc5)
    }

    /// Bytes of a tightly packed `w`×`h` surface.
    pub fn surface_bytes(self, w: usize, h: usize) -> usize {
        if self.is_block() {
            w.div_ceil(4) * h.div_ceil(4) * self.unit_bytes()
        } else {
            w * h * self.unit_bytes()
        }
    }
}

/// Decode a tightly packed `w`×`h` surface. `data` must hold at least
/// [`Format::surface_bytes`]`(w, h)` bytes.
pub fn decode(format: Format, w: usize, h: usize, data: &[u8]) -> Result<Vec<u8>, String> {
    let need = format.surface_bytes(w, h);
    if data.len() < need {
        return Err(format!("{format:?} {w}x{h} surface needs {need} bytes, got {}", data.len()));
    }
    let mut out = vec![0u8; w * h * 4];
    if format.is_block() {
        let (bw, bh) = (w.div_ceil(4), h.div_ceil(4));
        let ub = format.unit_bytes();
        for by in 0..bh {
            for bx in 0..bw {
                let off = (by * bw + bx) * ub;
                let block = &data[off..off + ub];
                let texels = match format {
                    Format::Bc1 => bc1_block(block, true),
                    Format::Bc2 => bc2_block(block),
                    Format::Bc3 => bc3_block(block),
                    Format::Bc4 => {
                        let r = bc4_block(block);
                        std::array::from_fn(|t| [r[t], r[t], r[t], 255])
                    }
                    _ => {
                        let (r, g) = (bc4_block(&block[..8]), bc4_block(&block[8..]));
                        std::array::from_fn(|t| [r[t], g[t], 0, 255])
                    }
                };
                for ty in 0..4 {
                    for tx in 0..4 {
                        let (px, py) = (bx * 4 + tx, by * 4 + ty);
                        if px < w && py < h {
                            let d = (py * w + px) * 4;
                            out[d..d + 4].copy_from_slice(&texels[ty * 4 + tx]);
                        }
                    }
                }
            }
        }
        return Ok(out);
    }
    for i in 0..w * h {
        let px = &mut out[i * 4..i * 4 + 4];
        match format {
            Format::A8r8g8b8 | Format::X8r8g8b8 => {
                let s = &data[i * 4..i * 4 + 4];
                px.copy_from_slice(&[s[2], s[1], s[0], if format == Format::A8r8g8b8 { s[3] } else { 255 }]);
            }
            Format::R5g6b5 => {
                let c = rgb565(u16::from_le_bytes([data[i * 2], data[i * 2 + 1]]));
                px.copy_from_slice(&[c[0], c[1], c[2], 255]);
            }
            Format::A8 => px.copy_from_slice(&[255, 255, 255, data[i]]),
            Format::L8 => px.copy_from_slice(&[data[i], data[i], data[i], 255]),
            _ => unreachable!("block formats returned above"),
        }
    }
    Ok(out)
}

fn rgb565(v: u16) -> [u8; 3] {
    let r = ((v >> 11) & 0x1F) as u32;
    let g = ((v >> 5) & 0x3F) as u32;
    let b = (v & 0x1F) as u32;
    [((r * 255 + 15) / 31) as u8, ((g * 255 + 31) / 63) as u8, ((b * 255 + 15) / 31) as u8]
}

/// 8-byte BC1 colour block. `allow_1bit_alpha`: BC1's `c0 <= c1` punch-through mode (BC2/BC3's
/// embedded colour block is always four-colour).
fn bc1_block(b: &[u8], allow_1bit_alpha: bool) -> [[u8; 4]; 16] {
    let c0 = u16::from_le_bytes([b[0], b[1]]);
    let c1 = u16::from_le_bytes([b[2], b[3]]);
    let p0 = rgb565(c0);
    let p1 = rgb565(c1);
    let mut pal = [[0u8; 4]; 4];
    pal[0] = [p0[0], p0[1], p0[2], 255];
    pal[1] = [p1[0], p1[1], p1[2], 255];
    if c0 > c1 || !allow_1bit_alpha {
        for k in 0..3 {
            pal[2][k] = ((2 * p0[k] as u32 + p1[k] as u32) / 3) as u8;
            pal[3][k] = ((p0[k] as u32 + 2 * p1[k] as u32) / 3) as u8;
        }
        pal[2][3] = 255;
        pal[3][3] = 255;
    } else {
        for k in 0..3 {
            pal[2][k] = ((p0[k] as u32 + p1[k] as u32) / 2) as u8;
        }
        pal[2][3] = 255;
        pal[3] = [0, 0, 0, 0];
    }
    let idx = u32::from_le_bytes([b[4], b[5], b[6], b[7]]);
    std::array::from_fn(|t| pal[((idx >> (t * 2)) & 3) as usize])
}

/// 16-byte BC2 block: 8 bytes of explicit 4-bit alpha, then a four-colour BC1 block.
fn bc2_block(b: &[u8]) -> [[u8; 4]; 16] {
    let alpha = u64::from_le_bytes(b[0..8].try_into().expect("8 bytes"));
    let mut out = bc1_block(&b[8..16], false);
    for (t, texel) in out.iter_mut().enumerate() {
        let a4 = ((alpha >> (t * 4)) & 0xF) as u8;
        texel[3] = (a4 << 4) | a4;
    }
    out
}

/// 16-byte BC3 block: an 8-byte BC4 alpha block, then a four-colour BC1 block.
fn bc3_block(b: &[u8]) -> [[u8; 4]; 16] {
    let a = bc4_block(&b[0..8]);
    let mut out = bc1_block(&b[8..16], false);
    for (t, texel) in out.iter_mut().enumerate() {
        texel[3] = a[t];
    }
    out
}

/// 8-byte BC4 block (BC3's alpha half): two endpoints and 16 three-bit indices.
fn bc4_block(b: &[u8]) -> [u8; 16] {
    let (a0, a1) = (b[0] as u32, b[1] as u32);
    let mut pal = [0u8; 8];
    pal[0] = a0 as u8;
    pal[1] = a1 as u8;
    if a0 > a1 {
        for k in 1..7u32 {
            pal[(k + 1) as usize] = (((7 - k) * a0 + k * a1) / 7) as u8;
        }
    } else {
        for k in 1..5u32 {
            pal[(k + 1) as usize] = (((5 - k) * a0 + k * a1) / 5) as u8;
        }
        pal[6] = 0;
        pal[7] = 255;
    }
    let mut bits = 0u64;
    for (i, &byte) in b[2..8].iter().enumerate() {
        bits |= (byte as u64) << (8 * i);
    }
    std::array::from_fn(|t| pal[((bits >> (t * 3)) & 7) as usize])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bc1_flat_block_decodes_opaque() {
        let c = ((128u16 >> 3) << 11) | ((128u16 >> 2) << 5) | (132u16 >> 3);
        let mut block = [0u8; 8];
        block[0..2].copy_from_slice(&c.to_le_bytes());
        block[2..4].copy_from_slice(&c.to_le_bytes());
        let px = decode(Format::Bc1, 4, 4, &block).unwrap();
        for t in px.chunks_exact(4) {
            assert_eq!(t[3], 255);
            assert!((t[0] as i32 - 128).abs() <= 8, "R ~128, got {}", t[0]);
        }
    }

    #[test]
    fn bc5_maps_blocks_to_red_and_green() {
        let block = [200u8, 200, 0, 0, 0, 0, 0, 0, 50, 50, 0, 0, 0, 0, 0, 0];
        let px = decode(Format::Bc5, 4, 4, &block).unwrap();
        assert_eq!(&px[..4], &[200, 50, 0, 255]);
    }

    #[test]
    fn uncompressed_layouts_and_short_data() {
        assert_eq!(decode(Format::A8r8g8b8, 1, 1, &[1, 2, 3, 4]).unwrap(), vec![3, 2, 1, 4]);
        assert_eq!(decode(Format::X8r8g8b8, 1, 1, &[1, 2, 3, 4]).unwrap(), vec![3, 2, 1, 255]);
        assert_eq!(decode(Format::R5g6b5, 1, 1, &0xF800u16.to_le_bytes()).unwrap(), vec![255, 0, 0, 255]);
        assert!(decode(Format::Bc3, 8, 4, &[0; 16]).unwrap_err().contains("needs 32"));
    }
}
