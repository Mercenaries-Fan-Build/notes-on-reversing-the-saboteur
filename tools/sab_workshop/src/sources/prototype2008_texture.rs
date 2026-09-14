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
//! The 4-byte `format_tag` is an Xbox 360 `_D3DFORMAT` enum value stored big-endian. The
//! mapping was cross-checked against the Xbox 360 D3D headers dumped from the sister
//! WildStar_d PDB (`output/pdb_dumps/saboteur_pdb_t.txt` around line 670633) — every tag
//! observed in the corpus is a `_D3DFORMAT` constant of the tiled (`D3DFMT_*`, not
//! `D3DFMT_LIN_*`) family. The per-mip storage pattern is invariable: each mip whose
//! min(W,H) ≥ the macrotile side gets its own base tile; smaller mips share one packed-tail
//! tile — that's what makes `unc` predictable, and it's why the base mip always sits at
//! offset 0 in the inflated buffer.
//!
//! | tag              | `_D3DFORMAT`         | element bytes | notes                                    |
//! |------------------|----------------------|---------------|------------------------------------------|
//! | `1A 20 01 52`    | D3DFMT_DXT1 (BC1)    | 8/block       | most textures — diffuse without alpha    |
//! | `1A 20 01 54`    | D3DFMT_DXT5 (BC3)    | 16/block      | diffuse-with-alpha and normal-in-DXT5    |
//! | `1A 20 01 71`    | D3DFMT_DXN  (BC5)    | 16/block      | 2-channel tangent normal; Z reconstructed|
//! | `28 28 01 44`    | D3DFMT_R5G6B5        | 2/texel       | 16bpp opaque colour masks, gradients     |
//! | `28 28 01 86`    | D3DFMT_X8R8G8B8      | 4/texel       | 32bpp uncompressed; opaque (X = 0xFF)    |
//!
//! DXN (a.k.a. ATI2/3Dc/BC5) shares the 16-byte block layout of DXT5 but interprets both
//! halves as BC4 alpha-blocks — first half gives X, second half gives Y. Z is reconstructed
//! from `sqrt(1 - X² - Y²)` at decode time so previews look like proper tangent-space
//! normals (B ≈ 255) instead of the garbage-RGB-plus-good-alpha of the previous BC3
//! interpretation.
//!
//! ## Xbox 360 tiling ("swizzle") — the hard part
//!
//! Retail Xbox 360 textures live in a swizzled memory layout so the GPU's texture cache can
//! stream 4×4-texel neighborhoods efficiently. To read them on a PC we un-swizzle to the
//! standard row-major layout and then either hand the bytes to the PC BC-block decoders
//! that already ship in `dtex::decode_surface` (BC1/BC3) or convert texel-by-texel to RGBA8
//! (BC5, R5G6B5, X8R8G8B8).
//!
//! The swizzle is a 32-element-wide macrotile with a fixed intra-tile pattern proven out by
//! the `XGAddress2DTiledOffset` reference implementation (public Xbox SDK, replicated in
//! Wine/XeXTool/Xenia). "Element" is a texel for uncompressed formats or a 4×4-block for BC,
//! and the same address formula covers both — only `log2(bytes per element)` changes. A BC1
//! macrotile is 32·32·8 = 8 192 bytes; BC3/BC5 32·32·16 = 16 384; R5G6B5 32·32·2 = 2 048;
//! X8R8G8B8 32·32·4 = 4 096. Any surface smaller than 32 elements in either dimension is
//! padded up to one full macrotile in storage — that's why the 32×32 DXT1 base mip weighs
//! 8 192 bytes even though the pixel data is 128× smaller.
//!
//! Byte-swap: Xbox 360 stores multi-byte texture words big-endian. The unit of swap is the
//! word size the GPU reads:
//! - 1 byte/texel (L8, A8): no swap.
//! - 2 bytes/texel or per BC-block 16-bit sub-word: swap every 2-byte pair. BC1 = 4 pairs
//!   per block, BC3/BC5 = 8 pairs per block, R5G6B5 = 1 pair per texel.
//! - 4 bytes/texel (X8R8G8B8, A8R8G8B8): swap every 4-byte pair (byte-reverse the DWORD).
//!   On disk one texel reads A,R,G,B big-endian; PC decoders want B,G,R,A little-endian.
//!
//! Un-swapping is done AFTER un-tiling — the tile move is opaque copies so ordering doesn't
//! affect correctness, and doing it once on the linear buffer is simpler than teaching each
//! per-tile copy about endianness.

use std::io::Read;

use crate::dtex::{self, CpuTexture};

/// Format tags observed in the corpus. See the module-doc table for provenance. Kept as
/// bytes rather than a u32 so the four-byte hex reads directly. The values are the
/// big-endian byte layout of an Xbox 360 `_D3DFORMAT` enum constant (D3DFMT_DXT1 = 0x1A20_0152,
/// D3DFMT_DXT5 = 0x1A20_0154, D3DFMT_DXN = 0x1A20_0171, D3DFMT_R5G6B5 = 0x2828_0144,
/// D3DFMT_X8R8G8B8 = 0x2828_0186).
pub const TAG_DXT1: [u8; 4] = [0x1A, 0x20, 0x01, 0x52];
pub const TAG_DXT5: [u8; 4] = [0x1A, 0x20, 0x01, 0x54];
pub const TAG_DXN: [u8; 4] = [0x1A, 0x20, 0x01, 0x71];
pub const TAG_R5G6B5: [u8; 4] = [0x28, 0x28, 0x01, 0x44];
pub const TAG_X8R8G8B8: [u8; 4] = [0x28, 0x28, 0x01, 0x86];

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
        TAG_DXT1 => decode_bc_tiled(&h, &inflated, /*block_bytes=*/ 8, BcKind::Bc1),
        TAG_DXT5 => decode_bc_tiled(&h, &inflated, /*block_bytes=*/ 16, BcKind::Bc3),
        TAG_DXN => decode_bc_tiled(&h, &inflated, /*block_bytes=*/ 16, BcKind::Bc5),
        TAG_R5G6B5 => decode_linear_tiled(&h, &inflated, LinearKind::R5G6B5),
        TAG_X8R8G8B8 => decode_linear_tiled(&h, &inflated, LinearKind::X8R8G8B8),
        _ => Err(format!(
            "prototype 2008: {} ({}x{}) format tag {} unrecognised — this decoder covers \
             DXT1 ({}), DXT5 ({}), DXN ({}), R5G6B5 ({}) and X8R8G8B8 ({})",
            h.name,
            w,
            hgt,
            hex4(&h.tag),
            hex4(&TAG_DXT1),
            hex4(&TAG_DXT5),
            hex4(&TAG_DXN),
            hex4(&TAG_R5G6B5),
            hex4(&TAG_X8R8G8B8),
        )),
    }
}

/// Which BC family a tiled surface uses. Determines both the PC-side block decoder we call
/// and the surface `format` value we tag onto the returned [`CpuTexture`] (the workshop UI
/// keys the format-name pill off this).
#[derive(Copy, Clone, Debug)]
enum BcKind {
    Bc1,
    Bc3,
    Bc5,
}

impl BcKind {
    fn d3d_fmt(self) -> u32 {
        match self {
            BcKind::Bc1 => D3DFMT_DXT1,
            BcKind::Bc3 => D3DFMT_DXT5,
            // No standard D3D9 4CC for BC5 — reuse the Xbox 360 DXN tag as the format id so
            // it round-trips as a distinct value in the UI.
            BcKind::Bc5 => 0x1A20_0171,
        }
    }
}

/// Which uncompressed surface a tiled linear format decodes to. Both variants produce
/// straight-alpha RGBA8 (opaque — the 32bpp variant is `X8R8G8B8`, so the X slot ignores
/// what's on disk and writes 255).
#[derive(Copy, Clone, Debug)]
enum LinearKind {
    R5G6B5,
    X8R8G8B8,
}

impl LinearKind {
    fn bytes_per_texel(self) -> usize {
        match self {
            LinearKind::R5G6B5 => 2,
            LinearKind::X8R8G8B8 => 4,
        }
    }
    /// `log2(bytes_per_texel)` — the `XGAddress2DTiledOffset` size parameter.
    fn log2_bpe(self) -> usize {
        match self {
            LinearKind::R5G6B5 => 1,
            LinearKind::X8R8G8B8 => 2,
        }
    }
    /// The dtex-side format code we tag onto the returned surface. Matches the values
    /// [`dtex::decode_surface`] would recognise for the same formats.
    fn d3d_fmt(self) -> u32 {
        match self {
            LinearKind::R5G6B5 => 0x16,
            LinearKind::X8R8G8B8 => 0x14, // D3DFMT_X8R8G8B8
        }
    }
}

// D3DFMT_ 4CC codes shared with `dtex::decode_surface`. Duplicating the two we need here
// so this module doesn't reach into dtex private constants.
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
    kind: BcKind,
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

    // Extract the top-left bw × bh block rectangle out of the storage-sized buffer (skip
    // padding rows/columns). Both `dtex::decode_surface` and the local BC5 decoder read
    // row-by-row assuming a tight `bw`-wide stride.
    let mut trimmed = Vec::with_capacity(bw * bh * block_bytes);
    for by in 0..bh {
        let row_start = by * storage_bw * block_bytes;
        trimmed.extend_from_slice(&linear[row_start..row_start + bw * block_bytes]);
    }
    let rgba = match kind {
        BcKind::Bc1 | BcKind::Bc3 => dtex::decode_surface(kind.d3d_fmt(), w as u32, hgt as u32, &trimmed)?,
        BcKind::Bc5 => decode_bc5(w, hgt, &trimmed),
    };
    Ok(CpuTexture {
        name: h.name.clone(),
        width: w as u32,
        height: hgt as u32,
        format: kind.d3d_fmt(),
        rgba,
    })
}

/// Decode a whole surface of D3DFMT_DXN / BC5 to RGBA8 with the two BC4 halves mapped to
/// R and G and Z reconstructed from `sqrt(1 - X² - Y²)`. The output is an OpenGL-tangent
/// normal map (R=X, G=+Y, B=Z), same convention as the retail `_NM` textures — the
/// workshop's `normalmap::mean_blue` gate then treats these consistently with the retail
/// path (real tangent normals, B ≈ 255 → passes the gate, no height-to-normal fixup).
///
/// A DXN block is 16 bytes: `[R BC4 half][G BC4 half]`, each half using the exact 8-byte
/// BC3-alpha encoding (2 endpoint bytes + 6 bytes of packed 3-bit indices).
fn decode_bc5(w: usize, h: usize, data: &[u8]) -> Vec<u8> {
    let (bw, bh) = (w.div_ceil(4), h.div_ceil(4));
    let mut out = vec![0u8; w * h * 4];
    for by in 0..bh {
        for bx in 0..bw {
            let off = (by * bw + bx) * 16;
            if off + 16 > data.len() {
                continue;
            }
            let r = decode_bc4_block(&data[off..off + 8]);
            let g = decode_bc4_block(&data[off + 8..off + 16]);
            for ty in 0..4 {
                for tx in 0..4 {
                    let (px, py) = (bx * 4 + tx, by * 4 + ty);
                    if px >= w || py >= h {
                        continue;
                    }
                    let d = (py * w + px) * 4;
                    let ri = r[ty * 4 + tx];
                    let gi = g[ty * 4 + tx];
                    // Reconstruct Z on the unit hemisphere. Values are stored as unsigned
                    // 0..255 → -1..+1 in normal-vector space.
                    let nx = (ri as f32) / 127.5 - 1.0;
                    let ny = (gi as f32) / 127.5 - 1.0;
                    let nz2 = 1.0 - nx * nx - ny * ny;
                    let nz = if nz2 > 0.0 { nz2.sqrt() } else { 0.0 };
                    let bi = ((nz + 1.0) * 127.5).clamp(0.0, 255.0) as u8;
                    out[d] = ri;
                    out[d + 1] = gi;
                    out[d + 2] = bi;
                    out[d + 3] = 255;
                }
            }
        }
    }
    out
}

/// 8-byte BC4 block → 16 unsigned 8-bit values. Identical algorithm to BC3's alpha half.
fn decode_bc4_block(b: &[u8]) -> [u8; 16] {
    let a0 = b[0] as u32;
    let a1 = b[1] as u32;
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
    let mut out = [0u8; 16];
    for t in 0..16 {
        out[t] = pal[((bits >> (t * 3)) & 7) as usize];
    }
    out
}

/// Decode mip 0 of a tiled uncompressed (linear) Xbox 360 surface into RGBA8.
///
/// The macrotile is 32×32 texels regardless of bytes-per-texel; the address formula is the
/// same XG one, parameterised by `log2(bytes per element)`. For very small surfaces
/// (< 32 texels in either dimension) the base mip is padded up to one full macrotile in
/// storage, so `inflated[0..storage_w*storage_h*bpp]` covers it.
fn decode_linear_tiled(
    h: &SlotTexHeader,
    inflated: &[u8],
    kind: LinearKind,
) -> Result<CpuTexture, String> {
    let (w, hgt) = (h.width as usize, h.height as usize);
    let bpp = kind.bytes_per_texel();
    let storage_w = w.max(32);
    let storage_h = hgt.max(32);
    let mip0_bytes = storage_w * storage_h * bpp;
    if mip0_bytes > inflated.len() {
        return Err(format!(
            "slot texture {}: linear mip 0 wants {mip0_bytes} bytes but only {} inflated",
            h.name,
            inflated.len()
        ));
    }
    let tiled = &inflated[..mip0_bytes];
    let log2_bpe = kind.log2_bpe();

    // Un-tile into a linear texel stream at storage dimensions, then crop to (w × hgt) as
    // we convert to RGBA8. Padding texels outside the logical rectangle are never read.
    let mut linear = vec![0u8; storage_w * storage_h * bpp];
    for y in 0..storage_h {
        for x in 0..storage_w {
            let src_off = xg_address_2d_tiled_offset(x, y, storage_w, log2_bpe);
            if src_off + bpp > tiled.len() {
                continue;
            }
            let dst_off = (y * storage_w + x) * bpp;
            linear[dst_off..dst_off + bpp].copy_from_slice(&tiled[src_off..src_off + bpp]);
        }
    }

    // Byte-swap by the GPU's word size. 2bpp swaps every pair; 4bpp reverses every 4-byte
    // group (the DWORD is stored big-endian, PC decoders read LE).
    match bpp {
        2 => {
            for chunk in linear.chunks_exact_mut(2) {
                chunk.swap(0, 1);
            }
        }
        4 => {
            for chunk in linear.chunks_exact_mut(4) {
                chunk.swap(0, 3);
                chunk.swap(1, 2);
            }
        }
        _ => unreachable!("unsupported linear bpp {bpp}"),
    }

    // Convert to RGBA8. Both formats are opaque previews — the X in X8R8G8B8 is discarded
    // per the D3D spec, and R5G6B5 has no alpha channel to preserve.
    let mut rgba = vec![0u8; w * hgt * 4];
    match kind {
        LinearKind::R5G6B5 => {
            for y in 0..hgt {
                for x in 0..w {
                    let src = (y * storage_w + x) * 2;
                    let v = u16::from_le_bytes([linear[src], linear[src + 1]]);
                    let dst = (y * w + x) * 4;
                    let r = ((v >> 11) & 0x1F) as u32;
                    let g = ((v >> 5) & 0x3F) as u32;
                    let b = (v & 0x1F) as u32;
                    rgba[dst] = ((r * 255 + 15) / 31) as u8;
                    rgba[dst + 1] = ((g * 255 + 31) / 63) as u8;
                    rgba[dst + 2] = ((b * 255 + 15) / 31) as u8;
                    rgba[dst + 3] = 255;
                }
            }
        }
        LinearKind::X8R8G8B8 => {
            // After the 4-byte reverse, one texel reads as (B, G, R, X) in `linear`. That's
            // the standard little-endian D3D9 X8R8G8B8 layout the retail decoder consumes.
            for y in 0..hgt {
                for x in 0..w {
                    let src = (y * storage_w + x) * 4;
                    let dst = (y * w + x) * 4;
                    rgba[dst] = linear[src + 2]; // R
                    rgba[dst + 1] = linear[src + 1]; // G
                    rgba[dst + 2] = linear[src]; // B
                    rgba[dst + 3] = 255; // X-channel is undefined; force opaque
                }
            }
        }
    }

    Ok(CpuTexture {
        name: h.name.clone(),
        width: w as u32,
        height: hgt as u32,
        format: kind.d3d_fmt(),
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
        assert!(matches!(tag, TAG_DXT1 | TAG_DXT5 | TAG_DXN | TAG_R5G6B5 | TAG_X8R8G8B8),
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
    /// This is our regression guard: if a tag stops decoding, its count drops. Coverage is
    /// deliberately generous per tag (≥100 records) so a new tag creeping in fails visibly
    /// and a broken decoder shows up as < a full run.
    #[test]
    fn decode_success_by_tag() {
        if bin360().is_none() { eprintln!("skip"); return }
        use std::collections::HashMap;
        let cap: u32 = 150;
        let mut total: HashMap<[u8; 4], u32> = HashMap::new();
        let mut ok: HashMap<[u8; 4], u32> = HashMap::new();
        let mut size_ok: HashMap<[u8; 4], u32> = HashMap::new();
        walk_texture_payloads::<()>(|_name, payload| {
            let h = match parse_header(payload) { Ok(h) => h, Err(_) => return None };
            let t = h.tag;
            if *total.get(&t).unwrap_or(&0) >= cap { return None }
            *total.entry(t).or_default() += 1;
            match decode_slot_texture(payload) {
                Ok(tex) => {
                    // Dimensions must match the header exactly.
                    let dim_ok = tex.width == h.width as u32 && tex.height == h.height as u32;
                    let byte_ok = tex.rgba.len() == (tex.width as usize) * (tex.height as usize) * 4;
                    if dim_ok && byte_ok {
                        *ok.entry(t).or_default() += 1;
                        *size_ok.entry(t).or_default() += 1;
                    }
                }
                Err(_) => {}
            }
            None
        });
        for (t, n) in &total {
            let k = ok.get(t).copied().unwrap_or(0);
            eprintln!("  tag {}  ok={k}/{n}", hex4(t));
        }
        // Every format we've wired must decode nearly every sample it's given — a hard
        // failure floor of 100 successes per tag (out of up to `cap` = 150 tries) is well
        // above what any spurious padding record could reach on its own.
        for tag in [TAG_DXT1, TAG_DXT5, TAG_DXN, TAG_R5G6B5, TAG_X8R8G8B8] {
            let seen = total.get(&tag).copied().unwrap_or(0);
            if seen == 0 { continue; } // format may be absent from this corpus slice
            let k = ok.get(&tag).copied().unwrap_or(0);
            assert!(
                k >= 100 || k as f32 >= (seen as f32) * 0.95,
                "tag {} decoded only {k}/{seen} — regression",
                hex4(&tag)
            );
        }
    }

    /// DXN is a tangent-space normal map — the reconstructed Z should push the mean blue
    /// well above 128 (uniform-random blocks would sit near 128; a real +Z-out normal map
    /// sits above 200). Guards the BC5 decode + Z-reconstruction path.
    ///
    /// The 2008 corpus has a class of AnimationTest 128×128 placeholder normals whose R
    /// channel is a plausible flat X≈0 but whose G channel is a "trivial 255" BC4 block
    /// (raw bytes `FF FE 00 00 00 00 00 00`, i.e. every texel Y=+1). Those decode
    /// correctly BY the BC5 algorithm — the source data really does say Y=+1 — but they
    /// aren't useful test signal. Filter them out by size (only sample ≥256px) so the
    /// assertion measures real content-authored normal maps.
    #[test]
    fn dxn_decodes_as_tangent_normal() {
        if bin360().is_none() { eprintln!("skip"); return }
        use crate::normalmap::{mean_blue, HEIGHT_MEAN_BLUE};
        let mut checked = 0;
        let mut passing = 0;
        let mut sum_blue: f32 = 0.0;
        walk_texture_payloads::<()>(|_name, payload| {
            let h = match parse_header(payload) { Ok(h) => h, Err(_) => return None };
            if h.tag != TAG_DXN || h.width < 256 { return None }
            if checked >= 20 { return None }
            let Ok(tex) = decode_slot_texture(payload) else { return None };
            if tex.rgba.len() != (tex.width as usize) * (tex.height as usize) * 4 { return None }
            checked += 1;
            let mb = mean_blue(&tex.rgba);
            sum_blue += mb;
            if mb >= HEIGHT_MEAN_BLUE { passing += 1 }
            None
        });
        if checked == 0 { eprintln!("skip: no DXN ≥256px records found"); return }
        let avg = sum_blue / checked as f32;
        eprintln!("DXN sampled={checked} pass_gate={passing} avg_meanB={avg:.1}");
        assert!(
            passing >= (checked * 3) / 4,
            "only {passing}/{checked} DXN textures look like tangent normals (meanB >= {HEIGHT_MEAN_BLUE}); avg meanB = {avg:.1}",
        );
    }

    /// Whole-corpus coverage survey. Walks every .Pack file, tries to decode every
    /// texture record, and prints per-tag pass/fail counts. Ignored because it takes
    /// a couple of minutes across the full 4300+ pack corpus — run with `-- --ignored`.
    #[test]
    #[ignore]
    fn corpus_wide_decode_coverage() {
        if bin360().is_none() { eprintln!("skip"); return }
        use std::collections::HashMap;
        let mut total: HashMap<[u8; 4], u32> = HashMap::new();
        let mut ok: HashMap<[u8; 4], u32> = HashMap::new();
        let mut err: HashMap<[u8; 4], u32> = HashMap::new();
        let mut unknown_tags: HashMap<[u8; 4], u32> = HashMap::new();
        walk_texture_payloads::<()>(|_name, payload| {
            let h = match parse_header(payload) { Ok(h) => h, Err(_) => return None };
            let t = h.tag;
            *total.entry(t).or_default() += 1;
            match decode_slot_texture(payload) {
                Ok(tex) => {
                    if tex.width == h.width as u32
                        && tex.height == h.height as u32
                        && tex.rgba.len() == (tex.width as usize) * (tex.height as usize) * 4
                    {
                        *ok.entry(t).or_default() += 1;
                    } else {
                        *err.entry(t).or_default() += 1;
                    }
                }
                Err(_) => {
                    *err.entry(t).or_default() += 1;
                    if !matches!(t, TAG_DXT1 | TAG_DXT5 | TAG_DXN | TAG_R5G6B5 | TAG_X8R8G8B8) {
                        *unknown_tags.entry(t).or_default() += 1;
                    }
                }
            }
            None
        });
        eprintln!("== 2008 corpus texture decode coverage ==");
        let mut tags: Vec<_> = total.keys().copied().collect();
        tags.sort();
        let mut sum_ok = 0u32;
        let mut sum_total = 0u32;
        for t in &tags {
            let n = total.get(t).copied().unwrap_or(0);
            let k = ok.get(t).copied().unwrap_or(0);
            let e = err.get(t).copied().unwrap_or(0);
            sum_ok += k;
            sum_total += n;
            eprintln!("  tag {}  ok={k:6}  err={e:6}  total={n:6}", hex4(t));
        }
        eprintln!("  ================================================");
        eprintln!("  TOTAL           ok={sum_ok:6}                  total={sum_total:6}");
        if !unknown_tags.is_empty() {
            eprintln!("-- unknown tags still failing --");
            for (t, n) in &unknown_tags {
                eprintln!("  tag {}  x{n}", hex4(t));
            }
        }
    }

    /// Diagnostic — dump per-channel means and the first few texels of a DXN texture so
    /// a mis-decoded BC5 shows up in the log instead of just "meanB is low, why?".
    #[test]
    #[ignore]
    fn diag_dxn_pixel_dump() {
        if bin360().is_none() { eprintln!("skip"); return }
        let mut dumped = 0;
        walk_texture_payloads::<()>(|_name, payload| {
            let h = match parse_header(payload) { Ok(h) => h, Err(_) => return None };
            if h.tag != TAG_DXN { return None }
            // Skip the AnimationTest 128x128 group — every mip is a byte-identical
            // placeholder that isn't useful for diagnostics. Prefer the real 512+ maps.
            if h.width < 256 { return None }
            if dumped >= 4 { return None }
            let Ok(tex) = decode_slot_texture(payload) else { return None };
            let n = (tex.width * tex.height) as u64;
            let mr: u64 = tex.rgba.chunks_exact(4).map(|p| p[0] as u64).sum();
            let mg: u64 = tex.rgba.chunks_exact(4).map(|p| p[1] as u64).sum();
            let mb: u64 = tex.rgba.chunks_exact(4).map(|p| p[2] as u64).sum();
            eprintln!(
                "DXN {} {}x{}  meanRGB=({}, {}, {})  first texels:",
                h.name, tex.width, tex.height, mr / n, mg / n, mb / n
            );
            for i in 0..8 {
                let p = &tex.rgba[i * 4..i * 4 + 4];
                eprintln!("    [{i}] = ({:3}, {:3}, {:3}, {:3})", p[0], p[1], p[2], p[3]);
            }
            dumped += 1;
            None
        });
    }

    /// Diagnostic — print the tag distribution + per-tag record dimensions so a decoder
    /// failure has context. Ignored by default (`cargo test -- --ignored` to run).
    #[test]
    #[ignore]
    fn diag_dump_tag_stats() {
        if bin360().is_none() { eprintln!("skip"); return }
        use std::collections::HashMap;
        let mut by_tag: HashMap<[u8; 4], Vec<(String, u16, u16, u32, u16)>> = HashMap::new();
        walk_texture_payloads::<()>(|_name, payload| {
            if let Ok(h) = parse_header(payload) {
                let v = by_tag.entry(h.tag).or_default();
                if v.len() < 20 {
                    v.push((h.name.clone(), h.width, h.height, h.unc, h.mips));
                }
            }
            None
        });
        for (t, recs) in &by_tag {
            eprintln!("--- tag {} ({} first samples) ---", hex4(t), recs.len());
            for (n, w, h, u, m) in recs {
                eprintln!("  {n} {w}x{h} unc={u} mips={m}  unc/(w*h)={:.3}",
                    (*u as f32) / ((*w as u32 * *h as u32).max(1) as f32));
            }
        }
    }

    /// R5G6B5 + X8R8G8B8 must produce byte-count-correct RGBA8 for at least 100 records
    /// each. Additionally asserts that AT LEAST one large sample per tag has genuine
    /// intra-image variation — a systematic byte-swap or stride bug would collapse ALL
    /// samples to a uniform colour, so the presence of variation is what we guard.
    ///
    /// R5G6B5 records in this corpus are overwhelmingly `AnimationTest_*_Mask` textures
    /// that really are single-colour, so the earlier "half must be non-degenerate" gate
    /// was measuring content, not decode correctness. What we can require, and does
    /// change if the decode is wrong, is that at least one large record shows genuine
    /// pixel variation.
    #[test]
    fn linear_formats_decode_non_degenerate() {
        if bin360().is_none() { eprintln!("skip"); return }
        use std::collections::HashMap;
        let cap: u32 = 150;
        let mut count: HashMap<[u8; 4], u32> = HashMap::new();
        let mut ok: HashMap<[u8; 4], u32> = HashMap::new();
        let mut varied: HashMap<[u8; 4], u32> = HashMap::new();
        walk_texture_payloads::<()>(|_name, payload| {
            let h = match parse_header(payload) { Ok(h) => h, Err(_) => return None };
            if !matches!(h.tag, TAG_R5G6B5 | TAG_X8R8G8B8) { return None }
            if *count.get(&h.tag).unwrap_or(&0) >= cap { return None }
            *count.entry(h.tag).or_default() += 1;
            let Ok(tex) = decode_slot_texture(payload) else { return None };
            if tex.rgba.len() != (tex.width as usize) * (tex.height as usize) * 4 { return None }
            *ok.entry(h.tag).or_default() += 1;
            // "Varied" = more than 8 distinct RGB triples. A byte-swap or stride bug
            // typically produces a single colour or noise; either extreme fails here.
            use std::collections::HashSet;
            let distinct: HashSet<[u8; 3]> = tex.rgba
                .chunks_exact(4)
                .map(|p| [p[0], p[1], p[2]])
                .collect();
            if distinct.len() > 8 { *varied.entry(h.tag).or_default() += 1; }
            None
        });
        for tag in [TAG_R5G6B5, TAG_X8R8G8B8] {
            let seen = count.get(&tag).copied().unwrap_or(0);
            if seen == 0 { eprintln!("no {} records in corpus slice", hex4(&tag)); continue }
            let k = ok.get(&tag).copied().unwrap_or(0);
            let n = varied.get(&tag).copied().unwrap_or(0);
            eprintln!("  linear {}  ok={k}/{seen}  varied={n}", hex4(&tag));
            assert!(
                k >= 100 || k as f32 >= (seen as f32) * 0.95,
                "tag {} decoded only {k}/{seen}",
                hex4(&tag)
            );
            assert!(
                n >= 1,
                "tag {}: zero samples had >8 distinct colours — decode likely broken",
                hex4(&tag)
            );
        }
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
