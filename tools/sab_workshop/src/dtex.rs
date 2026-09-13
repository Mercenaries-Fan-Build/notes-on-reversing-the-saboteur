//! DTEX decode → straight-alpha RGBA8, for texturing the viewer.
//!
//! The record parser (`parse`/`payload`/`mips_of`, the format codes, the 0x180000 stream split and
//! the 24-byte MipDesc) is COPIED from `tools/sab_dtex` — the validated reader (12,559 textures,
//! 100% decode). The BC1/BC2/BC3 block decoders are COPIED from the Mercs2 workshop's `texpng.rs`
//! (BC2 added here for DXT3). Do not re-derive either format.
//!
//! A DTEX is a length-prefixed record with NO 4CC magic (it lives inside an ALBS sub-pack). We take
//! the exact bytes of one record — what `sab_dtex carve` produces — and hand back the LARGEST mip as
//! RGBA8 ready to upload as an `Rgba8Unorm`/`…Srgb` texture.

// Some header fields / uncommon-format decoders land ahead of the callers that consume them.
#![allow(dead_code)]

// D3DFMT ints share the on-disk `format` field with ascii 4CCs.
const DXT1: u32 = 0x3154_5844; // BC1
const DXT3: u32 = 0x3354_5844; // BC2
const DXT5: u32 = 0x3554_5844; // BC3
const MIPDESC: usize = 24;

fn u16(b: &[u8], o: usize) -> u16 { u16::from_le_bytes([b[o], b[o + 1]]) }
fn u32(b: &[u8], o: usize) -> u32 { u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]) }

/// One decoded texture surface: the finest (largest) mip, straight-alpha RGBA8.
#[derive(Clone)]
pub struct CpuTexture {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub format: u32,
    pub rgba: Vec<u8>, // width*height*4
}

/// A parsed DTEX record header + its raw (still-zlib) streams.
pub struct Dtex {
    pub name: String,
    pub format: u32,
    pub flags: u32,
    pub width: u16,
    pub height: u16,
    pub mips: u16,
    unc: u32,
    streams: Vec<Vec<u8>>,
    pub total_len: usize,
}

impl Dtex {
    /// Human-readable format tag (DXT1/DXT5/A8R8G8B8/…).
    pub fn format_name(&self) -> String {
        match self.format {
            DXT1 => "DXT1".into(),
            DXT3 => "DXT3".into(),
            DXT5 => "DXT5".into(),
            0x15 => "A8R8G8B8".into(),
            0x14 => "X8R8G8B8".into(),
            0x1c => "A8".into(),
            0x32 => "L8".into(),
            0x16 => "R5G6B5".into(),
            0x28 => "V8U8".into(),
            other => format!("0x{other:08X}"),
        }
    }
}

fn uncompressed_bpp(fmt: u32) -> Option<usize> {
    match fmt {
        0x15 | 0x14 | 0x3f => Some(4),
        0x16 | 0x28 => Some(2),
        0x1c | 0x32 => Some(1),
        _ => None,
    }
}

/// Parse a standalone DTEX record (the bytes `sab_dtex carve` emits).
pub fn parse(buf: &[u8]) -> Result<Dtex, String> {
    if buf.len() < 4 {
        return Err("DTEX too short".into());
    }
    let name_len = u32(buf, 0) as usize;
    if name_len == 0 || name_len > 256 || 4 + name_len + 26 > buf.len() {
        return Err("not a DTEX record (bad name length)".into());
    }
    let name = String::from_utf8_lossy(&buf[4..4 + name_len]).into_owned();
    let f = 4 + name_len;
    let format = u32(buf, f);
    let flags = u32(buf, f + 4);
    let width = u16(buf, f + 8);
    let height = u16(buf, f + 10);
    let mips = u16(buf, f + 12);
    let unc = u32(buf, f + 14);
    let ns = u32(buf, f + 18) as usize;
    let mut o = f + 22;
    let mut streams = Vec::with_capacity(ns);
    for _ in 0..ns {
        if o + 4 > buf.len() {
            return Err("stream length overruns buffer".into());
        }
        let cs = u32(buf, o) as usize;
        o += 4;
        if o + cs > buf.len() {
            return Err("stream overruns buffer".into());
        }
        streams.push(buf[o..o + cs].to_vec());
        o += cs;
    }
    Ok(Dtex { name, format, flags, width, height, mips, unc, streams, total_len: o })
}

/// Inflate + concatenate every stream into the full mip payload. Streams were split at 0x180000
/// uncompressed-byte boundaries on disk; inflating each and concatenating rejoins the payload.
fn payload(d: &Dtex) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let mut out = Vec::with_capacity(d.unc as usize);
    for s in &d.streams {
        let mut z = flate2::read::ZlibDecoder::new(&s[..]);
        let mut buf = Vec::new();
        z.read_to_end(&mut buf).map_err(|e| format!("zlib inflate: {e}"))?;
        out.extend_from_slice(&buf);
    }
    Ok(out)
}

/// Walk the mip descriptors → (mipIndex, w, h, dataSlice), largest first.
fn mips_of<'a>(d: &Dtex, pl: &'a [u8]) -> Vec<(u32, u32, u32, &'a [u8])> {
    let mut v = Vec::new();
    let mut o = 0usize;
    for _ in 0..d.mips {
        if o + MIPDESC > pl.len() {
            break;
        }
        let w = u32(pl, o + 4);
        let h = u32(pl, o + 8);
        let sz = u32(pl, o + 20) as usize;
        let idx = u32(pl, o);
        o += MIPDESC;
        if o + sz > pl.len() {
            break;
        }
        v.push((idx, w, h, &pl[o..o + sz]));
        o += sz;
    }
    v
}

/// A DTEX record located inside an ALBS sub-pack: (byte offset, record length, plaintext name).
pub type Record = (usize, usize, String);

/// Find every DTEX record in an ALBS sub-pack — COPIED from `sab_dtex::find_records`. Scans for a
/// known format code, then back-scans for the `u32 nameLen + ascii name` that must precede it.
pub fn find_records(sub: &[u8]) -> Vec<Record> {
    let known = |f: u32| {
        matches!(f, DXT1 | DXT3 | DXT5) || uncompressed_bpp(f).is_some()
    };
    let mut v = Vec::new();
    let mut i = 0usize;
    while i + 26 < sub.len() {
        let f = u32(sub, i);
        if known(f) {
            for nl in 1..80usize {
                if i < 4 + nl {
                    break;
                }
                let lo = i - 4 - nl;
                if u32(sub, lo) as usize == nl
                    && sub[lo + 4..lo + 4 + nl].iter().all(|&b| (0x20..0x7f).contains(&b))
                    && lo + 4 + nl == i
                {
                    if let Ok(d) = parse(&sub[lo..]) {
                        v.push((lo, d.total_len, d.name.clone()));
                    }
                    break;
                }
            }
        }
        i += 1;
    }
    v
}

/// Decode a whole DTEX record's finest mip to RGBA8.
pub fn decode(buf: &[u8]) -> Result<CpuTexture, String> {
    let d = parse(buf)?;
    let pl = payload(&d)?;
    let mips = mips_of(&d, &pl);
    let (_, w, h, data) = *mips.first().ok_or("DTEX has no mips")?;
    let rgba = decode_surface(d.format, w, h, data)?;
    Ok(CpuTexture { name: d.name, width: w, height: h, format: d.format, rgba })
}

/// Decode the SMALLEST mip whose long edge still covers `min_dim` — a cheap preview.
///
/// The contact sheet shows a couple of hundred records at ~112px. Decoding each one's finest mip
/// (routinely 1024²) would cost seconds and hundreds of MB of RGBA to produce pixels that are then
/// scaled away. Mip N is the artist's own downsample, so picking it is both cheaper AND better
/// looking than box-filtering mip 0 ourselves. Falls back to the finest mip when every mip is
/// already smaller than `min_dim` (a tiny texture has nothing to spare).
pub fn decode_preview(buf: &[u8], min_dim: u32) -> Result<CpuTexture, String> {
    let d = parse(buf)?;
    let pl = payload(&d)?;
    let mips = mips_of(&d, &pl);
    // `mips_of` is largest-first, so scanning in reverse finds the smallest adequate mip first.
    let pick = mips
        .iter()
        .rev()
        .find(|(_, w, h, _)| (*w).max(*h) >= min_dim)
        .or_else(|| mips.first())
        .ok_or("DTEX has no mips")?;
    let (_, w, h, data) = *pick;
    let rgba = decode_surface(d.format, w, h, data)?;
    Ok(CpuTexture { name: d.name, width: w, height: h, format: d.format, rgba })
}

/// Decode one surface of `fmt` at `w`x`h` from `data` to RGBA8.
fn decode_surface(fmt: u32, w: u32, h: u32, data: &[u8]) -> Result<Vec<u8>, String> {
    let (w, h) = (w as usize, h as usize);
    match fmt {
        DXT1 | DXT3 | DXT5 => Ok(decode_bc(fmt, w, h, data)),
        0x15 | 0x14 => Ok(decode_argb8(w, h, data, fmt == 0x15)),
        0x1c => Ok(decode_a8(w, h, data)),
        0x32 => Ok(decode_l8(w, h, data)),
        0x16 => Ok(decode_r5g6b5(w, h, data)),
        other => Err(format!("unsupported texture format 0x{other:08X} for preview")),
    }
}

// ---- block-compressed (BC1/BC2/BC3) --------------------------------------------------------------

fn decode_bc(fmt: u32, w: usize, h: usize, data: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; w * h * 4];
    let (bw, bh) = (w.div_ceil(4), h.div_ceil(4));
    let block_bytes = if fmt == DXT1 { 8 } else { 16 };
    for by in 0..bh {
        for bx in 0..bw {
            let off = (by * bw + bx) * block_bytes;
            if off + block_bytes > data.len() {
                continue;
            }
            let block = &data[off..off + block_bytes];
            let texels = match fmt {
                DXT1 => decode_bc1_block(block, true),
                DXT3 => decode_bc2_block(block),
                _ => decode_bc3_block(block),
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
    out
}

fn rgb565(v: u16) -> [u8; 3] {
    let r = ((v >> 11) & 0x1F) as u32;
    let g = ((v >> 5) & 0x3F) as u32;
    let b = (v & 0x1F) as u32;
    [((r * 255 + 15) / 31) as u8, ((g * 255 + 31) / 63) as u8, ((b * 255 + 15) / 31) as u8]
}

/// 8-byte BC1 color block → 16 RGBA texels. `allow_1bit_alpha`: BC1's c0<=c1 punch-through mode
/// (BC2/BC3's embedded color block is always 4-color).
fn decode_bc1_block(b: &[u8], allow_1bit_alpha: bool) -> [[u8; 4]; 16] {
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
    let mut out = [[0u8; 4]; 16];
    for t in 0..16 {
        out[t] = pal[((idx >> (t * 2)) & 3) as usize];
    }
    out
}

/// 16-byte BC2 (DXT3) block: 8 bytes explicit 4-bit alpha + a 4-color BC1 color block.
fn decode_bc2_block(b: &[u8]) -> [[u8; 4]; 16] {
    let alpha = u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]);
    let mut out = decode_bc1_block(&b[8..16], false);
    for (t, texel) in out.iter_mut().enumerate() {
        let a4 = ((alpha >> (t * 4)) & 0xF) as u8;
        texel[3] = (a4 << 4) | a4; // 4-bit → 8-bit
    }
    out
}

/// 16-byte BC3 (DXT5) block: 8 bytes interpolated alpha + a 4-color BC1 color block.
fn decode_bc3_block(b: &[u8]) -> [[u8; 4]; 16] {
    let a0 = b[0] as u32;
    let a1 = b[1] as u32;
    let mut apal = [0u8; 8];
    apal[0] = a0 as u8;
    apal[1] = a1 as u8;
    if a0 > a1 {
        for k in 1..7u32 {
            apal[(k + 1) as usize] = (((7 - k) * a0 + k * a1) / 7) as u8;
        }
    } else {
        for k in 1..5u32 {
            apal[(k + 1) as usize] = (((5 - k) * a0 + k * a1) / 5) as u8;
        }
        apal[6] = 0;
        apal[7] = 255;
    }
    let mut abits = 0u64;
    for (i, &byte) in b[2..8].iter().enumerate() {
        abits |= (byte as u64) << (8 * i);
    }
    let mut out = decode_bc1_block(&b[8..16], false);
    for (t, texel) in out.iter_mut().enumerate() {
        texel[3] = apal[((abits >> (t * 3)) & 7) as usize];
    }
    out
}

// ---- uncompressed --------------------------------------------------------------------------------

/// D3DFMT_A8R8G8B8 / X8R8G8B8: on-disk bytes are B,G,R,A (little-endian 0xAARRGGBB).
fn decode_argb8(w: usize, h: usize, data: &[u8], has_alpha: bool) -> Vec<u8> {
    let mut out = vec![0u8; w * h * 4];
    for i in 0..(w * h) {
        let s = i * 4;
        if s + 4 > data.len() {
            break;
        }
        out[i * 4] = data[s + 2]; // R
        out[i * 4 + 1] = data[s + 1]; // G
        out[i * 4 + 2] = data[s]; // B
        out[i * 4 + 3] = if has_alpha { data[s + 3] } else { 255 };
    }
    out
}

fn decode_a8(w: usize, h: usize, data: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; w * h * 4];
    for i in 0..(w * h).min(data.len()) {
        out[i * 4] = 255;
        out[i * 4 + 1] = 255;
        out[i * 4 + 2] = 255;
        out[i * 4 + 3] = data[i];
    }
    out
}

fn decode_l8(w: usize, h: usize, data: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; w * h * 4];
    for i in 0..(w * h).min(data.len()) {
        let l = data[i];
        out[i * 4] = l;
        out[i * 4 + 1] = l;
        out[i * 4 + 2] = l;
        out[i * 4 + 3] = 255;
    }
    out
}

fn decode_r5g6b5(w: usize, h: usize, data: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; w * h * 4];
    for i in 0..(w * h) {
        let s = i * 2;
        if s + 2 > data.len() {
            break;
        }
        let v = u16::from_le_bytes([data[s], data[s + 1]]);
        let rgb = rgb565(v);
        out[i * 4] = rgb[0];
        out[i * 4 + 1] = rgb[1];
        out[i * 4 + 2] = rgb[2];
        out[i * 4 + 3] = 255;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// BC1 decode sanity on a hand-built 4x4 block (flat mid-grey): every texel opaque, RGB near the
    /// endpoint. Guards the block decoder without needing game files (the real-data path is covered by
    /// `resolve::tests::resolve_sean_diffuse_from_megapack`, which decodes Sean's actual DTEX).
    #[test]
    fn bc1_flat_block_decodes_opaque() {
        // c0 == c1 == rgb565(128,128,132), all indices 0 → a flat block of that colour.
        let c = ((128u16 >> 3) << 11) | ((128u16 >> 2) << 5) | (132u16 >> 3);
        let mut block = [0u8; 8];
        block[0..2].copy_from_slice(&c.to_le_bytes());
        block[2..4].copy_from_slice(&c.to_le_bytes());
        let texels = decode_bc1_block(&block, false);
        for t in texels {
            assert_eq!(t[3], 255, "opaque");
            assert!((t[0] as i32 - 128).abs() <= 8, "R ~128, got {}", t[0]);
            assert!(t[2] >= t[0], "B channel preserved");
        }
    }
}
