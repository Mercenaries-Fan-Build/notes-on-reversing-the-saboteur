//! 2008-05 Xbox 360 prototype — inner `.textures`-slot texture decoder.
//!
//! ## Where this sits
//!
//! The outer container is already decoded (`prototype2008_subpack.rs` gives us the raw
//! `.textures` slot bytes). Inside that slot is a headless mini-directory of per-texture
//! records (`prototype2008.rs::parse_texslot` mines it). Each record's PAYLOAD is a
//! self-contained texture blob whose bytes this module turns into RGBA8.
//!
//! ## Payload layout (all big-endian; proven against the corpus)
//!
//! ```text
//! +0        u32   name_len
//! +4        char  name[name_len]
//! +4+nl     u8[4] format_tag       // see the table below
//! +8+nl     u32   flags            // varies (mostly small ints, meaning unresolved)
//! +12+nl    u16   width
//! +14+nl    u16   height
//! +16+nl    u16   mips             // 1..log2(max(W,H))+1
//! +18+nl    u32   unc              // total uncompressed size of ALL streams packed
//! +22+nl    u32   ns               // number of zlib streams (usually 1)
//! per stream:
//!            u32   cs              // compressed size
//!            u8[cs] zlib_data
//! +N        u8[8] "MSHC…"          // 8-byte trailer, likely a hash — ignored
//! ```
//!
//! The 22-byte pre-streams header is unaligned reads (retail DTEX has the same shape,
//! LE — this is that shape byte-swapped end-to-end for the 360). Multi-stream splitting
//! isn't observed in the current corpus (every `.textures` sub-record has `ns=1`), but the
//! parser handles `ns>=1` for future-proofing.
//!
//! ## Format tag table
//!
//! The 4-byte `format_tag` is a game-internal enum, not a plain GPUTEXTUREFORMAT. It was
//! derived empirically: every unique tag observed across the 4389 SBL-family `.Pack` files
//! was surveyed, its `unc / (W·H)` ratio computed for many `(W,H,mips)` bins, and the storage
//! pattern (tile count × tile size) matched to a specific Xbox 360 texture format. The
//! per-mip storage pattern is invariable — each mip whose min(W,H) ≥ 32 gets its own base
//! tile; smaller mips share one packed-tail tile. That's what makes the format identifiable
//! from the uncompressed byte-count alone.
//!
//! | tag              | format             | block bytes | notes                                    |
//! |------------------|--------------------|-------------|------------------------------------------|
//! | `1A 20 01 52`    | DXT1 (BC1)         | 8           | most textures — diffuse without alpha    |
//! | `1A 20 01 54`    | DXT5 (BC3)         | 16          | diffuse-with-alpha and normal-in-DXT5    |
//! | `1A 20 01 71`    | DXT5 (BC3)         | 16          | normal-map variant (decoded as BC3)      |
//! | `28 28 01 44`    | 16bpp uncompressed | -           | 2 bytes/texel masks; A8L8/16bpp guess    |
//! | `28 28 01 86`    | A8R8G8B8 (4 bpp)   | -           | uncompressed detail normal maps          |
//!
//! The three DXT variants (`1A 20 01 52` / `54` / `71`) are all wired end-to-end — the
//! `71` variant is empirically the same 16-byte BC3 blocks as `54`, just used for normal
//! maps that the game's shader unpacks with a swizzle (X→alpha, Y→green). The block bytes
//! we emit are correct D3D9 BC3; a future revision can post-swizzle for cleaner previews.
//! Together the three tags cover 24 988 of 47 316 records (53 %). The two uncompressed
//! variants (~48 % of records but overwhelmingly small masks) return a shaped error naming
//! the tag; adding them means a linear un-tile plus a channel-order probe for the 16bpp
//! case — clean follow-up work.
//!
//! ## Xbox 360 tiling ("swizzle") — the hard part
//!
//! Retail Xbox 360 textures live in a swizzled memory layout so the GPU's texture cache can
//! stream 4×4-texel neighborhoods efficiently. To read them on a PC we un-swizzle to the
//! standard row-major layout and then hand the bytes to the PC BC-block decoders that
//! already ship in `dtex::decode_surface`.
//!
//! The swizzle is a 32×32-block macrotile with a fixed intra-tile pattern proven out by the
//! `XGAddress2DTiledOffset` reference implementation (public Xbox SDK, replicated in
//! Wine/XeXTool/Xenia). BC1 blocks are 8 bytes so one macrotile = 32·32·8 = 8 192 bytes;
//! BC3 blocks are 16 bytes so one macrotile = 16 384 bytes. Any texture smaller than 128×128
//! pixels (32×32 blocks) is padded to exactly one macrotile. This is why the uncompressed
//! byte-count for e.g. a 32×32 DXT1 texture with all six mips is `2 × 8192` — one base tile
//! for mip 0 + one packed-tail tile for mips 1..5.
//!
//! Byte-swap: within each BC block, every 16-bit word is byte-swapped on-disk (the GPU is
//! big-endian). BC1 blocks are 4 words; BC3 blocks are 8 words. Un-swapping after un-tiling
//! turns them into the little-endian D3D9 block layout the PC decoder expects.

use std::io::Read;

use crate::dtex::{self, CpuTexture};

/// Format tags observed in the corpus. See the module-doc table for provenance. Kept as
/// bytes rather than a u32 so the four-byte hex reads directly.
pub const TAG_DXT1: [u8; 4] = [0x1A, 0x20, 0x01, 0x52];
pub const TAG_DXT5: [u8; 4] = [0x1A, 0x20, 0x01, 0x54];
pub const TAG_DXT5_NM: [u8; 4] = [0x1A, 0x20, 0x01, 0x71];
pub const TAG_LIN_16BPP: [u8; 4] = [0x28, 0x28, 0x01, 0x44];
pub const TAG_LIN_8888: [u8; 4] = [0x28, 0x28, 0x01, 0x86];

/// The 22-byte header past the format tag. Broken out so the parser and the workshop UI
/// can share a single view of the record shape (a caller that only wants dimensions doesn't
/// want to inflate the payload).
#[derive(Clone, Debug)]
pub struct SlotTexHeader {
    pub name: String,
    pub tag: [u8; 4],
    pub flags: u32,
    pub width: u16,
    pub height: u16,
    pub mips: u16,
    /// Total uncompressed size of every stream concatenated. For a 1-stream record, the
    /// inflated buffer will be exactly `unc` bytes long. The tile-count math elsewhere in
    /// this module is what pins the format down; this field is the sanity check.
    pub unc: u32,
    pub ns: u32,
    /// Offset of the first stream's `cs u32` prefix in the whole record payload.
    pub streams_off: usize,
}

/// Parse the on-disk header of a slot-texture record. Doesn't inflate anything.
pub fn parse_header(payload: &[u8]) -> Result<SlotTexHeader, String> {
    if payload.len() < 4 {
        return Err("slot texture: payload too short for name_len".into());
    }
    let name_len = u32_be(payload, 0)? as usize;
    if name_len == 0 || name_len > 256 {
        return Err(format!("slot texture: implausible name_len {name_len}"));
    }
    let tag_off = 4 + name_len;
    if tag_off + 4 + 22 > payload.len() {
        return Err(format!(
            "slot texture: header truncated (need {}+22 bytes, have {})",
            tag_off + 4,
            payload.len()
        ));
    }
    let name = std::str::from_utf8(&payload[4..tag_off])
        .map_err(|_| "slot texture: non-utf8 name")?
        .trim_end_matches('\0')
        .to_string();
    let tag = [
        payload[tag_off],
        payload[tag_off + 1],
        payload[tag_off + 2],
        payload[tag_off + 3],
    ];
    let h = tag_off + 4;
    let flags = u32_be(payload, h)?;
    let width = u16_be(payload, h + 4)?;
    let height = u16_be(payload, h + 6)?;
    let mips = u16_be(payload, h + 8)?;
    let unc = u32_be(payload, h + 10)?;
    let ns = u32_be(payload, h + 14)?;
    Ok(SlotTexHeader {
        name,
        tag,
        flags,
        width,
        height,
        mips,
        unc,
        ns,
        streams_off: h + 18,
    })
}

/// Turn a slot-texture record into RGBA8. Returns an error for formats not yet cracked
/// (which stays a small shaped stub — the caller surfaces it to the UI).
///
/// `payload` is the whole sub-record starting at the `name_len` u32 (i.e. what
/// `prototype2008.rs::SlotTex` addresses via `slot_off + data_off .. + data_size`).
pub fn decode_slot_texture(payload: &[u8]) -> Result<CpuTexture, String> {
    let h = parse_header(payload)?;
    let (w, hgt) = (h.width as u32, h.height as u32);
    // Guard: width/height must be positive and (for BC formats) at least 4 in each dimension.
    // Every real texture we've seen is a power of two, but the un-tile / block-decode paths
    // handle non-pow2 correctly too — it's the alignment that matters, not pow2-ness.
    if w == 0 || hgt == 0 {
        return Err(format!("slot texture: zero dimension {w}x{hgt}"));
    }

    // Inflate every stream and concatenate. `unc` is our expected total; we sanity-check
    // against it so a bad stream doesn't silently degrade to garbage.
    let inflated = inflate_streams(payload, &h)?;
    if inflated.len() != h.unc as usize {
        return Err(format!(
            "slot texture {}: inflated {} bytes but header claims unc={}",
            h.name,
            inflated.len(),
            h.unc
        ));
    }

    // Route by format tag. Everything past this point is per-format.
    match h.tag {
        TAG_DXT1 => decode_bc_tiled(&h, &inflated, /*block_bytes=*/ 8, /*fmt=*/ D3DFMT_DXT1),
        TAG_DXT5 | TAG_DXT5_NM => {
            decode_bc_tiled(&h, &inflated, /*block_bytes=*/ 16, /*fmt=*/ D3DFMT_DXT5)
        }
        TAG_LIN_16BPP | TAG_LIN_8888 => Err(format!(
            "prototype 2008: {} ({}x{}) tag {} is uncompressed \
             ({} bpp) — decoder not wired yet, only DXT1/DXT5 supported today",
            h.name,
            w,
            hgt,
            hex4(&h.tag),
            if h.tag == TAG_LIN_16BPP { 16 } else { 32 }
        )),
        _ => Err(format!(
            "prototype 2008: {} ({}x{}) format tag {} unrecognised — this decoder covers \
             DXT1 ({}), DXT5 ({}, {}), 16bpp ({}) and 8888 ({})",
            h.name,
            w,
            hgt,
            hex4(&h.tag),
            hex4(&TAG_DXT1),
            hex4(&TAG_DXT5),
            hex4(&TAG_DXT5_NM),
            hex4(&TAG_LIN_16BPP),
            hex4(&TAG_LIN_8888),
        )),
    }
}

// D3DFMT_ codes shared with `dtex::decode_surface`. Duplicating the two we need here so this
// module doesn't reach into dtex private constants.
const D3DFMT_DXT1: u32 = 0x3154_5844;
const D3DFMT_DXT5: u32 = 0x3554_5844;

/// Inflate the record's zlib stream(s) into one contiguous buffer. Multi-stream records
/// concatenate their inflated payloads in order — matches retail's `dtex::payload` shape.
fn inflate_streams(payload: &[u8], h: &SlotTexHeader) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(h.unc as usize);
    let mut o = h.streams_off;
    for i in 0..h.ns {
        if o + 4 > payload.len() {
            return Err(format!("slot texture {}: stream[{}] cs prefix overruns", h.name, i));
        }
        let cs = u32_be(payload, o)? as usize;
        o += 4;
        if o + cs > payload.len() {
            return Err(format!(
                "slot texture {}: stream[{}] cs={} overruns record",
                h.name, i, cs
            ));
        }
        let mut z = flate2::read::ZlibDecoder::new(&payload[o..o + cs]);
        z.read_to_end(&mut out).map_err(|e| format!(
            "slot texture {}: stream[{}] zlib inflate: {e}", h.name, i
        ))?;
        o += cs;
    }
    Ok(out)
}

/// Decode mip 0 of a BC-family Xbox-360-tiled texture into RGBA8.
///
/// The inflated buffer contains ALL mip levels tiled, in Xbox 360 storage order:
/// - mip 0 occupies the first `tile_count(mip0) × tile_bytes` bytes
/// - mip 1 padded to its own base-tile allocation follows
/// - …down to the smallest mip whose `min(w,h) >= 32`; below that the mips are packed into
///   ONE trailing tile (with uninit `BAADF00D` padding — the trailer of every real slot).
///
/// We only need mip 0 for a preview. We compute its byte range (base tile count × tile size),
/// carve those bytes, un-tile them into a row-major block stream, byte-swap each block's
/// 16-bit words, and hand off to `dtex::decode_surface`.
fn decode_bc_tiled(
    h: &SlotTexHeader,
    inflated: &[u8],
    block_bytes: usize,
    d3d_fmt: u32,
) -> Result<CpuTexture, String> {
    let (w, hgt) = (h.width as usize, h.height as usize);
    // BC formats are 4×4 blocks. Block-count dimensions:
    let bw = w.div_ceil(4);
    let bh = hgt.div_ceil(4);
    // Storage width in blocks, padded up to a 32-block macrotile boundary. The XG address
    // formula reads the storage width, not the logical block width.
    let storage_bw = bw.max(32);
    let storage_bh = bh.max(32);
    let mip0_bytes = storage_bw * storage_bh * block_bytes;
    if mip0_bytes > inflated.len() {
        return Err(format!(
            "slot texture {}: mip 0 wants {mip0_bytes} bytes but only {} inflated",
            h.name,
            inflated.len()
        ));
    }
    let tiled = &inflated[..mip0_bytes];
    let log2_bpb = match block_bytes {
        8 => 3,
        16 => 4,
        _ => return Err(format!("slot texture {}: unsupported block size {block_bytes}", h.name)),
    };

    // Un-tile into a linear (row-major) block stream sized for the storage dimensions. We
    // over-allocate to storage_bw × storage_bh (may include padding rows/columns for small
    // textures); the block-decoder then reads only the top-left bw × bh rectangle it needs.
    let mut linear = vec![0u8; storage_bw * storage_bh * block_bytes];
    for out_by in 0..storage_bh {
        for out_bx in 0..storage_bw {
            let src_off = xg_address_2d_tiled_offset(out_bx, out_by, storage_bw, log2_bpb);
            if src_off + block_bytes > tiled.len() {
                continue; // padding region past the last real byte — leave zeros
            }
            let dst_off = (out_by * storage_bw + out_bx) * block_bytes;
            linear[dst_off..dst_off + block_bytes]
                .copy_from_slice(&tiled[src_off..src_off + block_bytes]);
        }
    }

    // Byte-swap every 16-bit word inside each block. The GPU is big-endian; the PC BC
    // decoder wants the LE D3D9 layout. Do this on the linear buffer AFTER un-tiling —
    // the un-tile just moves opaque 8/16-byte chunks, so which order we do these two
    // steps in makes no difference, but doing it after keeps the tiled-vs-linear boundary
    // cleaner in the code.
    for chunk in linear.chunks_exact_mut(2) {
        chunk.swap(0, 1);
    }

    // Now decode as a standard D3D9 BC1/BC3 surface. Extract the top-left bw × bh block
    // rectangle out of the storage-sized buffer (skip the padding rows/columns).
    // `decode_surface` reads row-by-row using the STORAGE stride, so hand it a copy that
    // uses `bw` as its stride. Simpler than teaching the decoder about strides.
    let mut trimmed = Vec::with_capacity(bw * bh * block_bytes);
    for by in 0..bh {
        let row_start = by * storage_bw * block_bytes;
        trimmed.extend_from_slice(&linear[row_start..row_start + bw * block_bytes]);
    }
    let rgba = dtex::decode_surface(d3d_fmt, w as u32, hgt as u32, &trimmed)?;
    Ok(CpuTexture {
        name: h.name.clone(),
        width: w as u32,
        height: hgt as u32,
        format: d3d_fmt,
        rgba,
    })
}

/// Xbox 360 2D-tiled texture address function. Given un-tiled block coordinates `(x, y)`
/// (block coords for BC, texel coords for uncompressed — either way the same code), the
/// storage `pitch_aligned` in blocks (rounded up to a 32-block macrotile boundary), and
/// `log2_bpb` = log2 of bytes per block (3 for BC1, 4 for BC3/BC5), returns the byte
/// offset within the tiled surface where that block lives.
///
/// **Provenance**: this is Xenia's `texture_address::Tiled2D` (public GitHub, GPL-permissive
/// header), which is the reference the Xbox 360 emulator community has settled on. Two
/// other candidate formulas (the "Xbox SDK" one that appears in old forum posts, and the
/// "Wine XG" copy) were surveyed — they both produce out-of-range offsets for small
/// (single-macrotile) surfaces, and neither round-trips a known input. Xenia's version is
/// the one that gives contiguous coverage of `[0, pitch*height*bpb)` for every corpus size
/// checked (verified by exhaustive enumeration of a 32×32 BC1 tile — see the scratchpad
/// verifier). If a decode ever regresses, re-check the formula against Xenia HEAD rather
/// than tweaking individual bit ops.
fn xg_address_2d_tiled_offset(x: usize, y: usize, pitch_aligned_blocks: usize, log2_bpb: usize) -> usize {
    // Macro-tile: which 32×32-block cell this (x,y) sits in, shifted to become the
    // upper-bits base. `<< 6` = 64 blocks per macrotile group before we mix in inner bits.
    let outer_blocks = ((y >> 5) * (pitch_aligned_blocks >> 5) + (x >> 5)) << 6;
    // Inner-tile position within the 32×32 macrotile: y bits 1-3 give the row group, x bits
    // 0-2 give the column within the 8-wide micro-row.
    let inner_blocks = (((y >> 1) & 0b111) << 3) | (x & 0b111);
    let outer_inner_bytes = (outer_blocks | inner_blocks) << log2_bpb;
    // "Bank" and "pipe" are the Xbox 360 GPU's memory-bank/DRAM-channel bits interleaved
    // into the address for parallelism; they scramble the low bits of the final offset.
    let bank = (y >> 4) & 1;
    let pipe = ((x >> 3) & 3) ^ (((y >> 3) & 1) << 1);
    let y_lsb = y & 1;
    // TiledCombine: fold bank/pipe into the address, and shuffle outer_inner_bytes bits
    // into the final layout. This is the invertible bijection the GPU uses internally.
    ((y_lsb << 4) | (pipe << 6) | (bank << 11))
        | (outer_inner_bytes & 0xF)
        | (((outer_inner_bytes >> 4) & 1) << 5)
        | (((outer_inner_bytes >> 5) & 0b111) << 8)
        | ((outer_inner_bytes >> 8) << 12)
}

fn u16_be(b: &[u8], o: usize) -> Result<u16, String> {
    if o + 2 > b.len() {
        return Err(format!("u16 read at 0x{o:X} overruns"));
    }
    Ok(u16::from_be_bytes([b[o], b[o + 1]]))
}

fn u32_be(b: &[u8], o: usize) -> Result<u32, String> {
    if o + 4 > b.len() {
        return Err(format!("u32 read at 0x{o:X} overruns"));
    }
    Ok(u32::from_be_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]))
}

fn hex4(t: &[u8; 4]) -> String {
    format!("{:02X} {:02X} {:02X} {:02X}", t[0], t[1], t[2], t[3])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pack::pandemic_hash;
    use crate::sources::prototype2008_subpack::SubPack;
    use std::path::PathBuf;

    fn bin360() -> Option<PathBuf> {
        let p = PathBuf::from("c:/Users/Shadow/Desktop/notes-on-reversing-the-sabetour/game-files/Bin360");
        if p.is_dir() { Some(p) } else { None }
    }

    /// Walk .Pack files under Bin360 and hand each texture record's payload to `visit`.
    /// Skips packs that aren't SBL-family or don't have a textures slot. Bails once
    /// `visit` returns Some to keep test-runtime bounded.
    fn walk_texture_payloads<T>(mut visit: impl FnMut(&str, &[u8]) -> Option<T>) -> Option<T> {
        let root = bin360()?;
        walk_dir(&root, &mut |path| {
            if !path.extension().and_then(|s| s.to_str())
                .map(|s| s.eq_ignore_ascii_case("pack"))
                .unwrap_or(false)
            { return None }
            let buf = std::fs::read(path).ok()?;
            let sbl = SubPack::parse(&buf).ok()?;
            let rec = sbl.find("textures")?;
            let slot = sbl.slot_bytes(rec, &buf)?;
            let n = u32::from_be_bytes([slot[0], slot[1], slot[2], slot[3]]) as usize;
            if n == 0 || n > 256 || 4 + n*24 > slot.len() { return None }
            for i in 0..n {
                let ro = 4 + i*24;
                let data_off = u32::from_be_bytes([slot[ro+4], slot[ro+5], slot[ro+6], slot[ro+7]]) as usize;
                let data_size = u32::from_be_bytes([slot[ro+8], slot[ro+9], slot[ro+10], slot[ro+11]]) as usize;
                if data_off + data_size > slot.len() { continue }
                let payload = &slot[data_off..data_off+data_size];
                if let Ok(h) = parse_header(payload) {
                    if let Some(r) = visit(&h.name, payload) { return Some(r) }
                }
            }
            None
        })
    }

    fn walk_dir<T>(dir: &std::path::Path, f: &mut dyn FnMut(&std::path::Path) -> Option<T>) -> Option<T> {
        let rd = std::fs::read_dir(dir).ok()?;
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                if let Some(r) = walk_dir(&p, f) { return Some(r) }
            } else if let Some(r) = f(&p) { return Some(r) }
        }
        None
    }

    /// Header parses cleanly on real data + names decode as printable ASCII.
    #[test]
    fn parses_header_on_real_records() {
        if bin360().is_none() { eprintln!("skip"); return }
        let (name, tag, w, h) = walk_texture_payloads(|_name, payload| {
            let h = parse_header(payload).ok()?;
            Some((h.name.clone(), h.tag, h.width, h.height))
        }).expect("no textures found");
        assert!(w.is_power_of_two() && h.is_power_of_two(), "dims {w}x{h} not pow2");
        assert!(!name.is_empty() && name.chars().all(|c| c.is_ascii_graphic() || c == ' '),
            "bad name {name:?}");
        assert!(matches!(tag, TAG_DXT1 | TAG_DXT5 | TAG_DXT5_NM | TAG_LIN_16BPP | TAG_LIN_8888),
            "unexpected tag {tag:?} for {name}");
    }

    /// Full end-to-end on `DET_Rough128_NM` — the small named-verified texture called out
    /// in the probe brief. It's a `1A 20 01 54` (BC3) 128×128 record. Must produce an
    /// RGBA8 buffer of exactly the right size, non-uniform pixel content, and mean RGB in
    /// the plausible-image range. A wrong un-tile scatters the correct pixel content
    /// throughout the buffer; the "at least K distinct RGB values" guard catches an
    /// all-zero / stuck-at-max output that would slip past the mean-RGB check alone.
    #[test]
    fn decode_det_rough128_nm() {
        if bin360().is_none() { eprintln!("skip"); return }
        let tex = walk_texture_payloads(|name, payload| {
            if name == "DET_Rough128_NM" {
                decode_slot_texture(payload).ok()
            } else { None }
        }).expect("DET_Rough128_NM not found or failed to decode");
        assert_eq!(tex.width, 128);
        assert_eq!(tex.height, 128);
        assert_eq!(tex.rgba.len(), 128*128*4);
        // Non-degenerate: at least some pixels must vary. All-zero and all-max-value are
        // both failures (uninit / stuck-at-max).
        let mean_r: u64 = tex.rgba.chunks_exact(4).map(|p| p[0] as u64).sum::<u64>() / (128*128);
        let mean_g: u64 = tex.rgba.chunks_exact(4).map(|p| p[1] as u64).sum::<u64>() / (128*128);
        let mean_b: u64 = tex.rgba.chunks_exact(4).map(|p| p[2] as u64).sum::<u64>() / (128*128);
        assert!(mean_r > 4 && mean_g > 4 && mean_b > 4,
            "output looks all-zero: mean RGB = ({mean_r}, {mean_g}, {mean_b})");
        assert!(mean_r < 251 || mean_g < 251 || mean_b < 251,
            "output looks stuck-at-max: mean RGB = ({mean_r}, {mean_g}, {mean_b})");
        // Distinct-RGB probe: a real texture has hundreds of distinct pixel colours. If
        // the un-tile is wrong the output is often a uniform grey/black/white. A DXT-compressed
        // 128×128 image should hit at least 64 distinct RGB triples in normal art.
        use std::collections::HashSet;
        let distinct_rgb: HashSet<[u8; 3]> = tex.rgba
            .chunks_exact(4)
            .map(|p| [p[0], p[1], p[2]])
            .collect();
        assert!(distinct_rgb.len() >= 64,
            "expected at least 64 distinct RGB triples in a real 128×128 texture, got {}",
            distinct_rgb.len());
        let _ = pandemic_hash("DET_Rough128_NM"); // sanity call; the actual hash-lookup path
                                                  // is in prototype2008.rs
    }

    /// For every format tag observed, count decode success rate across a bounded sample.
    /// This is our regression guard: if a tag stops decoding, its count drops.
    #[test]
    fn decode_success_by_tag() {
        if bin360().is_none() { eprintln!("skip"); return }
        use std::collections::HashMap;
        let mut total: HashMap<[u8; 4], u32> = HashMap::new();
        let mut ok: HashMap<[u8; 4], u32> = HashMap::new();
        // Bounded probe: at least 5 of each tag, up to ~50 records per tag total to keep
        // wall-clock reasonable.
        walk_texture_payloads::<()>(|_name, payload| {
            let h = match parse_header(payload) { Ok(h) => h, Err(_) => return None };
            let t = h.tag;
            if *total.get(&t).unwrap_or(&0) >= 50 { return None }
            *total.entry(t).or_default() += 1;
            if let Ok(tex) = decode_slot_texture(payload) {
                if tex.rgba.len() == (tex.width as usize) * (tex.height as usize) * 4 {
                    *ok.entry(t).or_default() += 1;
                }
            }
            None
        });
        // Print the split — visible with `cargo test -- --nocapture`. Assert only that the
        // BC formats (which we do decode) hit ≥5 successes each. The uncompressed formats
        // are expected to fail cleanly with a shaped error.
        for (t, n) in &total {
            let k = ok.get(t).copied().unwrap_or(0);
            eprintln!("  tag {}  ok={k}/{n}", hex4(t));
        }
        assert!(ok.get(&TAG_DXT1).copied().unwrap_or(0) >= 5, "DXT1 decode should mostly succeed");
        assert!(ok.get(&TAG_DXT5).copied().unwrap_or(0) >= 5, "DXT5 decode should mostly succeed");
    }

    /// Exhaustive sanity check on `xg_address_2d_tiled_offset`: for a 32×32-block BC1
    /// surface (128×128 pixels — the minimum single-macrotile allocation), enumerate every
    /// (x, y) and require:
    /// - all 1024 offsets are unique (formula is a bijection);
    /// - every offset falls inside `[0, 32*32*8) = [0, 8192)` (the buffer size).
    ///
    /// A regression here (usually a mis-copied bit twiddle) makes the un-tile scramble
    /// half the image, which a mean-RGB probe on a real decode would NOT catch. This test
    /// is what turned up the initial "XG SDK" formula being wrong — it produced offsets
    /// up to 22 528 for a 32×32 surface, which is the empirical fingerprint of overflow.
    #[test]
    fn xg_address_bijection_on_smallest_bc1_surface() {
        use std::collections::HashSet;
        let mut seen = HashSet::new();
        for y in 0..32 {
            for x in 0..32 {
                let off = xg_address_2d_tiled_offset(x, y, 32, 3);
                assert!(off < 8192, "off {off} out of range at ({x},{y})");
                assert!(seen.insert(off), "duplicate off {off} at ({x},{y})");
            }
        }
        assert_eq!(seen.len(), 1024);
        // Same check for BC3 (16-byte blocks) — budget is 16384.
        let mut seen16 = HashSet::new();
        for y in 0..32 {
            for x in 0..32 {
                let off = xg_address_2d_tiled_offset(x, y, 32, 4);
                assert!(off < 16384);
                assert!(seen16.insert(off));
            }
        }
        assert_eq!(seen16.len(), 1024);
        // The very first block must land at offset 0.
        assert_eq!(xg_address_2d_tiled_offset(0, 0, 32, 3), 0);
        assert_eq!(xg_address_2d_tiled_offset(0, 0, 128, 4), 0);
    }
}
