//! A `textures` slot record of the 2008 prototype: one texture's header and its zlib streams, which
//! inflate to the Xbox 360 GPU memory image of the texture (tiled, big-endian elements). Layout and
//! evidence: `docs/2008_texture_format.md`. Engine readers: `ReadTextureDeprecated` (`WildStar_d.exe`
//! @0x8265ad48) and the streamed `ReadTexture` / `DecompressTexture` (@0x82663cc0 / @0x82664590), which
//! inflate the streams straight into the texture's locked memory.
//!
//! ```text
//! u32   name_len            at most 0x400
//! char  name[name_len]
//! u32   format              Xbox 360 D3DFORMAT
//! u32   flags               (flags & 0x79) == 0 -> the engine ORs 0x7E00 into format (sRGB)
//! u16   width, height, mips
//! u32   unpacked            bytes of the inflated memory image
//! u32   stream_count        0 means 1
//! { u32 packed; u8 zlib[packed] }[stream_count]
//! ```

use crate::bytes;
use crate::pixels;

/// `D3DFMT_DXT1` (`WildStar_p.pdb`).
pub const DXT1: u32 = 0x1A20_0152;
/// `D3DFMT_DXT5` (and `D3DFMT_DXT4`, the same value).
pub const DXT5: u32 = 0x1A20_0154;
/// `D3DFMT_DXN`.
pub const DXN: u32 = 0x1A20_0171;
/// `D3DFMT_R5G6B5`.
pub const R5G6B5: u32 = 0x2828_0144;
/// `D3DFMT_X8R8G8B8`.
pub const X8R8G8B8: u32 = 0x2828_0186;
/// The flag bits whose absence makes the engine sample the texture as sRGB.
pub const LINEAR_FLAGS: u32 = 0x79;
/// `D3DFORMAT` sign fields X, Y, Z set to `GPUSIGN_GAMMA` (bits 9..14).
pub const GAMMA_SIGNS: u32 = 0x7E00;

/// A parsed texture record; streams borrow the caller's bytes.
#[derive(Clone, Debug)]
pub struct Texture<'a> {
    pub name: String,
    pub format: u32,
    pub flags: u32,
    pub width: u16,
    pub height: u16,
    pub mips: u16,
    pub unpacked: u32,
    pub streams: Vec<&'a [u8]>,
    /// Bytes after the last stream.
    pub trailing: &'a [u8],
}

impl Texture<'_> {
    /// Whether the engine samples the texture as sRGB: it sets the gamma sign fields when none of
    /// [`LINEAR_FLAGS`] is set.
    pub fn srgb(&self) -> bool {
        self.flags & LINEAR_FLAGS == 0
    }

    /// The format the engine creates the texture with (`format` with the gamma fields applied).
    pub fn engine_format(&self) -> u32 {
        if self.srgb() {
            (self.format & 0xFFFF_81FF) | GAMMA_SIGNS
        } else {
            self.format
        }
    }

    /// The inflated memory image: every stream inflated and concatenated; it must be `unpacked`
    /// bytes long.
    pub fn memory(&self) -> Result<Vec<u8>, String> {
        let mut out = Vec::with_capacity(self.unpacked as usize);
        for (i, s) in self.streams.iter().enumerate() {
            let mut dec = flate2::read::ZlibDecoder::new(*s);
            std::io::Read::read_to_end(&mut dec, &mut out)
                .map_err(|e| format!("texture '{}' stream {i}: zlib inflate failed: {e}", self.name))?;
            if dec.total_in() as usize != s.len() {
                return Err(format!("texture '{}' stream {i}: zlib ends after {} of {} bytes", self.name, dec.total_in(), s.len()));
            }
        }
        if out.len() != self.unpacked as usize {
            return Err(format!("texture '{}': streams inflate to {} bytes, header declares {}", self.name, out.len(), self.unpacked));
        }
        Ok(out)
    }
}

/// Parse one record; every byte up to the end of the last stream is accounted for, and the bytes
/// after it are returned in `trailing`.
pub fn parse(b: &[u8]) -> Result<Texture<'_>, String> {
    let name_len = bytes::u32_be(b, 0, "texture name_len")? as usize;
    if name_len > 0x400 {
        return Err(format!("texture: name_len {name_len} exceeds the reader's 0x400"));
    }
    let raw = bytes::slice(b, 4, name_len, "texture name")?;
    let name = String::from_utf8_lossy(raw.strip_suffix(&[0]).unwrap_or(raw)).into_owned();
    let h = 4 + name_len;
    let format = bytes::u32_be(b, h, "texture format")?;
    let flags = bytes::u32_be(b, h + 4, "texture flags")?;
    let width = bytes::u16_be(b, h + 8, "texture width")?;
    let height = bytes::u16_be(b, h + 10, "texture height")?;
    let mips = bytes::u16_be(b, h + 12, "texture mips")?;
    let unpacked = bytes::u32_be(b, h + 14, "texture unpacked")?;
    let count = bytes::u32_be(b, h + 18, "texture stream_count")?.max(1) as usize;
    let mut at = h + 22;
    let mut streams = Vec::with_capacity(count.min(64));
    for i in 0..count {
        let packed = bytes::u32_be(b, at, "texture stream length")? as usize;
        streams.push(bytes::slice(b, at + 4, packed, &format!("texture '{name}' stream {i}"))?);
        at += 4 + packed;
    }
    Ok(Texture { name, format, flags, width, height, mips, unpacked, streams, trailing: &b[at..] })
}

/// A format's pixel decoder and its GPU element: bytes per element and whether an element is a 4×4
/// block.
pub fn surface_format(format: u32) -> Result<pixels::Format, String> {
    Ok(match format & !GAMMA_SIGNS {
        DXT1 => pixels::Format::Bc1,
        DXT5 => pixels::Format::Bc3,
        DXN => pixels::Format::Bc5,
        R5G6B5 => pixels::Format::R5g6b5,
        X8R8G8B8 => pixels::Format::X8r8g8b8,
        other => return Err(format!("texture format 0x{other:08X} is not one of the build's five")),
    })
}

/// Byte offset of element `(x, y)` in a tiled Xbox 360 surface whose width, in elements, is aligned
/// to 32 (`XGAddress2DTiledOffset`). `log2_bpe` is log2 of the bytes per element.
pub fn tiled_offset(x: u32, y: u32, aligned_width: u32, log2_bpe: u32) -> u32 {
    let macro_ = ((x >> 5) + (y >> 5) * (aligned_width >> 5)) << (log2_bpe + 7);
    let micro = ((x & 7) + ((y & 6) << 2)) << log2_bpe;
    let offset = macro_ + ((micro & !15) << 1) + (micro & 15) + ((y & 8) << (3 + log2_bpe)) + ((y & 1) << 4);
    ((offset & !511) << 3) + ((offset & 448) << 2) + (offset & 63) + ((y & 16) << 7) + (((((y & 8) >> 2) + (x >> 3)) & 3) << 6)
}

/// Where one mip level lives in the memory image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Level {
    pub level: u16,
    /// The level's size in texels.
    pub width: u32,
    pub height: u32,
    /// Byte offset of the storage the level is tiled into.
    pub offset: usize,
    /// That storage's row pitch, in elements (a multiple of 32).
    pub pitch: u32,
    /// The level's element offset inside that storage (non-zero for levels in a packed tail).
    pub x: u32,
    pub y: u32,
}

fn log2_ceil(v: u32) -> u32 {
    32 - v.max(1).saturating_sub(1).leading_zeros()
}

/// Every level's storage is rounded up to 4 KiB (Xenia `kTextureSubresourceAlignmentBytes`).
const SUBRESOURCE_ALIGN: usize = 0x1000;

/// The first level of the packed mip tail: `max(log2_ceil(min(w, h)) - 4, 0)` (Xenia
/// `GetPackedMipLevel`, `src/xenia/gpu/texture_util.h`).
pub fn packed_level(width: u32, height: u32) -> u32 {
    log2_ceil(width.min(height)).saturating_sub(4)
}

/// Texel offset of level `level` inside the packed tail of a `width`×`height` texture (Xenia
/// `GetPackedMipOffset`, `src/xenia/gpu/texture_util.cc`, 2D case). The first three tail levels sit at
/// 16, 8, 4 texels along the short axis' perpendicular; the rest at power-of-two offsets along the
/// long axis.
pub fn packed_offset(width: u32, height: u32, level: u32) -> (u32, u32) {
    let (lw, lh) = (log2_ceil(width), log2_ceil(height));
    let packed = level - packed_level(width, height);
    if packed < 3 {
        if lw > lh {
            (0, 16 >> packed)
        } else {
            (16 >> packed, 0)
        }
    } else if lw > lh {
        ((1 << (lw - packed_level(width, height))) >> (packed - 2), 0)
    } else {
        (0, (1 << (lh - packed_level(width, height))) >> (packed - 2))
    }
}

/// Every level's placement, and the image's total size, following Xenia's `GetGuestTextureLayout`
/// for a tiled 2D texture whose mips follow the base. The base level's storage is its width and
/// height in elements, each aligned to 32. Mip level `L`'s storage is sized from the power-of-two
/// dimensions, `max(1, next_pow2(w) >> L)` × `max(1, next_pow2(h) >> L)`, aligned likewise; the mip
/// levels follow the base one after the other. From [`packed_level`] down, levels share that level's
/// storage at [`packed_offset`]; when the packed level is 0, the base is itself at its packed offset
/// in the base storage and the mips share one level-0-sized storage.
pub fn levels(t: &Texture) -> Result<(Vec<Level>, usize), String> {
    let f = surface_format(t.format)?;
    let block = if f.is_block() { 4 } else { 1 };
    let bpe = f.unit_bytes();
    let (w, h) = (t.width as u32, t.height as u32);
    if w == 0 || h == 0 {
        return Err(format!("texture '{}': {w}x{h}", t.name));
    }
    let max_mips = 32 - w.max(h).leading_zeros();
    if t.mips as u32 > max_mips {
        return Err(format!("texture '{}': {} mips exceed the {max_mips} a {w}x{h} texture has", t.name, t.mips));
    }
    let elems = |texels: u32| texels.div_ceil(block).max(1);
    let storage = |sw: u32, sh: u32| {
        let pitch = elems(sw).next_multiple_of(32);
        (pitch, (pitch as usize * elems(sh).next_multiple_of(32) as usize * bpe).next_multiple_of(SUBRESOURCE_ALIGN))
    };
    let packed = packed_level(w, h);
    let placed = |level: u32| if level >= packed { packed_offset(w, h, level) } else { (0, 0) };
    let mut out = Vec::with_capacity(t.mips.max(1) as usize);
    let (bp, base_bytes) = storage(w, h);
    let (bx, by) = placed(0);
    out.push(Level { level: 0, width: w, height: h, offset: 0, pitch: bp, x: bx / block, y: by / block });
    let mut end = base_bytes;
    let mut tail: Option<(usize, u32)> = None;
    for level in 1..t.mips.max(1) as u32 {
        let storage_level = level.min(packed);
        let (pitch, bytes) = storage((w.next_power_of_two() >> storage_level).max(1), (h.next_power_of_two() >> storage_level).max(1));
        let offset = if level < packed {
            let o = end;
            end += bytes;
            o
        } else {
            tail.get_or_insert_with(|| {
                let o = end;
                end += bytes;
                (o, pitch)
            })
            .0
        };
        let (x, y) = placed(level);
        out.push(Level { level: level as u16, width: (w >> level).max(1), height: (h >> level).max(1), offset, pitch, x: x / block, y: y / block });
    }
    Ok((out, end))
}

/// One level of a tiled memory image, untiled and byte-swapped to the PC (little-endian) layout
/// [`pixels::decode`] reads: elements are 4×4 blocks for block formats and texels otherwise; block
/// formats swap every 16-bit word (endian 8in16), 32-bit texels reverse every 32-bit word (8in32),
/// 16-bit texels swap every 16-bit word.
pub fn level_bytes(t: &Texture, memory: &[u8], l: &Level) -> Result<Vec<u8>, String> {
    let f = surface_format(t.format)?;
    let block = if f.is_block() { 4 } else { 1 };
    let (ew, eh) = (l.width.div_ceil(block), l.height.div_ceil(block));
    let bpe = f.unit_bytes() as u32;
    let log2_bpe = bpe.trailing_zeros();
    let mut out = vec![0u8; (ew * eh * bpe) as usize];
    for y in 0..eh {
        for x in 0..ew {
            let src = l.offset + tiled_offset(x + l.x, y + l.y, l.pitch, log2_bpe) as usize;
            let s = memory.get(src..src + bpe as usize).ok_or_else(|| {
                format!("texture '{}' level {}: element ({x},{y}) at {src} past the {}-byte image", t.name, l.level, memory.len())
            })?;
            let dst = ((y * ew + x) * bpe) as usize;
            out[dst..dst + bpe as usize].copy_from_slice(s);
        }
    }
    match f {
        pixels::Format::X8r8g8b8 => out.chunks_exact_mut(4).for_each(|c| c.reverse()),
        _ => out.chunks_exact_mut(2).for_each(|c| c.swap(0, 1)),
    }
    Ok(out)
}

/// Decode one level of a memory image to straight-alpha RGBA8.
pub fn decode_level(t: &Texture, memory: &[u8], l: &Level) -> Result<Vec<u8>, String> {
    let linear = level_bytes(t, memory, l)?;
    pixels::decode(surface_format(t.format)?, l.width as usize, l.height as usize, &linear)
}

/// Decode a record's base level to straight-alpha RGBA8.
pub fn decode_base(t: &Texture) -> Result<Vec<u8>, String> {
    let memory = t.memory()?;
    let (ls, _) = levels(t)?;
    decode_level(t, &memory, &ls[0])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zlib;

    fn record(format: u32, flags: u32, w: u16, h: u16, image: &[u8]) -> Vec<u8> {
        let z = zlib::deflate(image, 6);
        let mut b = Vec::new();
        b.extend_from_slice(&5u32.to_be_bytes());
        b.extend_from_slice(b"tex1\0");
        b.extend_from_slice(&format.to_be_bytes());
        b.extend_from_slice(&flags.to_be_bytes());
        for v in [w, h, 1] {
            b.extend_from_slice(&v.to_be_bytes());
        }
        b.extend_from_slice(&(image.len() as u32).to_be_bytes());
        b.extend_from_slice(&1u32.to_be_bytes());
        b.extend_from_slice(&(z.len() as u32).to_be_bytes());
        b.extend_from_slice(&z);
        b
    }

    /// For 4-, 8- and 16-byte elements a 32×32 tile maps onto exactly its own `1024 << log2` bytes.
    #[test]
    fn tiled_offsets_cover_a_macro_tile_exactly_once() {
        for log2 in [2, 3, 4] {
            let mut seen = vec![false; 32 * 32];
            for y in 0..32 {
                for x in 0..32 {
                    let o = tiled_offset(x, y, 32, log2) >> log2;
                    assert!(!seen[o as usize], "log2 {log2}: ({x},{y}) collides");
                    seen[o as usize] = true;
                }
            }
        }
    }

    /// `AnimationTest/0.Pack` record 0: 512×512 DXT1, 10 mips, `unpacked` 196608.
    #[test]
    fn mip_layout_of_a_square_dxt1() {
        let rec = record(DXT1, 6, 512, 512, &[]);
        let mut t = parse(&rec).unwrap();
        t.mips = 10;
        let (ls, end) = levels(&t).unwrap();
        assert_eq!(end, 196_608);
        assert_eq!(packed_level(512, 512), 5);
        assert_eq!((ls[1].offset, ls[1].pitch), (131_072, 64));
        assert_eq!((ls[5].offset, ls[5].x, ls[5].y), (ls[6].offset, 4, 0));
        assert_eq!((ls[6].x, ls[6].y, ls[7].x, ls[7].y), (2, 0, 1, 0));
        assert_eq!((ls[8].x, ls[8].y, ls[9].x, ls[9].y), (0, 2, 0, 1));
    }

    #[test]
    fn parses_header_and_untiles_x8r8g8b8() {
        let mut image = vec![0u8; 64 * 64 * 4];
        // Texel (1, 0) = A,R,G,B = 0x80, 0x10, 0x20, 0x30 as a big-endian u32.
        let o = tiled_offset(1, 0, 64, 2) as usize;
        image[o..o + 4].copy_from_slice(&[0x80, 0x10, 0x20, 0x30]);
        let rec = record(X8R8G8B8, 0, 64, 64, &image);
        let t = parse(&rec).unwrap();
        assert_eq!(t.name, "tex1");
        assert!(t.srgb());
        assert_eq!(t.engine_format(), X8R8G8B8 | GAMMA_SIGNS);
        assert!(t.trailing.is_empty());
        let rgba = decode_base(&t).unwrap();
        assert_eq!(&rgba[4..8], &[0x10, 0x20, 0x30, 255]);
        let mut bad = rec.clone();
        let n = bad.len();
        bad.truncate(n - 1);
        assert!(parse(&bad).is_err());
    }
}
