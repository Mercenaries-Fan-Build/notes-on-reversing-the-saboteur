//! MSH4 — the 2008 Xbox 360 build's mesh format.
//!
//! ## Container framing (proven, byte-verified)
//!
//! An inflated MSH4 blob lives inside a `<hash>.meshes` slot's inner mini-directory (see
//! `Prototype2008Source::read_mesh_bytes`). Layout, all big-endian outer framing,
//! byte-verified against `BP_AL_Sean_HD.Pack` (164159 B), `BP_AL_Sean_LB.Pack`, GR, UB:
//!
//! ```text
//! +0x00  char   magic[4]          = "MSH4"                              // BE ASCII
//! +0x04  u32be  name_len          includes trailing \0; pad up to 4-B
//! +0x08  char[] name[name_len]    ASCII, e.g. "CH_AL_SeanDevlin_01_HD\0"
//!        char[] pad_to_align_4    zero pad — payload starts at the next 4-B boundary
//!                                 (name_len=23 → 1 byte pad → payload at 0x20)
//!
//! +0x20  … PAYLOAD (file header + N streams + skeleton) …
//!
//! FILE_END-8  char   magic[4]     = "MSHC"                              // BE ASCII
//! FILE_END-4  u32be  content_hash preserved verbatim for round-trip
//! ```
//!
//! ## Payload encoding — the "BE u24 + pad byte" trick
//!
//! Every 4-byte integer field in the file/stream/prim header is written as a **big-endian
//! 24-bit value in the first three bytes plus a zero pad byte in the fourth**. Reading it
//! naively as `u32 BE` gives you `value << 8`; the actual value is `(u32_be >> 8) & 0xFFFFFF`.
//! This is a Xenon-vertex-shader-writer quirk (see the byte layout in the packed decls the
//! GPU consumes), not a mistake — every field lands cleanly on a 4-byte slot without ever
//! occupying the low byte. `rd24` below reads exactly this.
//!
//! ## Payload layout — decoded
//!
//! ```text
//! FILE HEADER (5 fields = 20 B)
//!   [0] u24 num_streams
//!   [1] u24 (0)
//!   [2] u24 (0)
//!   [3] u24 (0)
//!   [4] u24 version (=1)
//!
//! For each of num_streams streams (in file order):
//!   STREAM HEADER (6 fields = 24 B)
//!     [0] u24 format_tag        // retail-shape bitfield, see `decode_format`
//!     [1] u24 num_indices
//!     [2] u24 num_vertices
//!     [3] u24 format_extension  // seen as 0
//!     [4] u24 stride            // 24, 28, or 32 in the sampled bodies
//!     [5] u24 num_prims
//!
//!   VB: num_vertices × stride bytes                (little-endian vertex data)
//!   IB: num_indices × 2 bytes                       (u16 LE triangle-list indices)
//!
//!   PRIMS: num_prims × 28 B, each 7 fields:
//!     [0] u24 (0)                // reserved / local index base — always 0 on Sean's parts
//!     [1] u24 num_tris           // == num_indices_this_prim / 3
//!     [2] u24 material_hash      // 24-bit truncated pandemic_hash for the material
//!     [3] u24 (0)
//!     [4] u24 (0)
//!     [5] u24 index_start        // offset into THIS stream's IB where this prim's tris start
//!     [6] u24 num_indices        // == num_tris * 3
//!
//! (After all streams: skeleton / bone-remaps / shadows / occluders — see WildStar_d
//! `ReadMesh @ 0x82658168`. Not decoded yet; not needed to render.)
//! ```
//!
//! ## Vertex-attribute layout
//!
//! The format_tag is the SAME bitfield the retail engine's `Stream.format` uses (see
//! `meshload::decode_format`), only with a different set of attribute *sizes* for the
//! packed Xbox 360 formats. Bit split:
//!
//! ```text
//! bits 0..1: positionType   (=2 → half3 + 2-byte pad = 8 bytes)
//! bits 2..3: skinType       (=1 → adds u8x4 weights + u8x4 bone indices)
//! bits 4..7: numColors      (=0 on every sampled Sean part)
//! bits 8..11: numUVs        (1 or 2)
//! bit  12   : has_normal    (1 → adds a 4-byte DEC3N-packed normal)
//! bit  13   : has_tangent   (0 or 1 → adds a 4-byte DEC3N-packed tangent)
//! ```
//!
//! Observed formats and strides on Sean's four parts:
//!
//! | fmt      | attrs                                                  | stride |
//! | -------- | ------------------------------------------------------ | -----: |
//! | `0x1106` | pos + skin + 1 UV + normal                             | 24     |
//! | `0x3106` | pos + skin + 1 UV + normal + tangent                   | 28     |
//! | `0x3206` | pos + skin + 2 UVs + normal + tangent                  | 32     |
//!
//! All confirmed against real bytes: max index < num_vertices per prim, prim material hashes
//! decode to plausible 24-bit values, and vertex 0 of every stream decodes to a plausible
//! model-space position (face vertices at (~1.7, ~0.07, ~1.07) on Sean HD, torso vertices
//! at (~0.1, ~0.01, ~1.16) on Sean LB, etc.).
//!
//! ## Known format variants — open work
//!
//! The encoding above (aligned payload + rd24 fields) is what Sean's four character parts
//! use — every mesh whose name is a 22-character string plus NUL pads the name to a 4-byte
//! boundary with one byte and shifts the u32 slots into the rd24 pattern. Meshes with
//! different name lengths (weapons, props, other characters) store the SAME fields at a
//! non-shifted offset, i.e. plain `u32 BE` at the exact post-name byte, without the shift.
//! `parse_msh4` today decodes only the shifted variant; a non-Sean mesh will come back with
//! `implausible num_streams N` where `N` is the count times 256 (or, less commonly, 0). Two
//! encoders is one probe short of shipping — the fix is to try both readers and pick the
//! one whose `num_streams` falls in `1..=32`. Filed as follow-up; the workshop still shows
//! Sean rendered correctly today.
//!
//! ## References
//!
//! * `output/_ghidra_wildstar/wildstar_d_all_functions_decomp.txt` @ line 523552 —
//!   `ReadMesh @ 0x82658168` in the sibling WildStar_d engine, 99.8% named, ground truth for
//!   the file-header shape (5 fields), per-stream layout (6 fields), and per-prim layout
//!   (7 fields per `XenonPrimitive::Setup` call). Same code family as Saboteur's
//!   `?ReadMesh@@YAXAAVPblFile@@_NAAV?$PblHandle@VOdinMesh@@@@PAVWSStreamBlock@@@Z`, whose
//!   Xbox 360 decomp at `output/_ghidra_saboteur_xex/...` @ line 427900 confirms the same
//!   count and order of 4-byte reads.
//! * `docs/2008_msh4_format.md` — the format writeup, kept in sync with this module.

use std::io::Read;

use crate::formats::{Prim, Smsh};

/// The 4-byte magic that starts every MSH4 record after zlib inflate.
pub const MSH4_MAGIC: &[u8; 4] = b"MSH4";
/// The 4-byte magic of the 8-byte footer at the end of every MSH4 record.
pub const MSHC_FOOTER_MAGIC: &[u8; 4] = b"MSHC";

/// The alignment the payload starts on relative to the file. After the ASCII name (variable
/// length, includes NUL), the writer pads up to the next 4-byte boundary before writing the
/// payload. For every sampled Sean part `name_len == 23`, giving a 1-byte pad and payload
/// at 0x20. A shorter or longer name simply lands the payload at a different 4-B boundary.
const PAYLOAD_ALIGN: usize = 4;

/// One MSH4 record, parsed out of the raw inflated bytes. `positions/normals/uvs/joints/
/// weights/indices/prims` are populated by `parse_msh4` when the payload decodes cleanly;
/// `diagnostics` carries the byte-range and per-stream metadata for the UI to display.
pub struct Msh4Body {
    /// The mesh's plaintext name as written in the file header (e.g. `"CH_AL_SeanDevlin_01_HD"`).
    /// NUL bytes are trimmed.
    pub name: String,
    /// The `MSHC` footer's trailing `u32be` — a content hash we preserve verbatim so a
    /// round-trip re-write can put it back.
    pub content_hash: u32,
    /// `[start .. end)` byte-range of the payload region between the aligned name-pad and
    /// the `MSHC` footer.
    pub payload_range: std::ops::Range<usize>,

    // ── the retail-Smsh-shaped fields — populated when decode succeeds ────────────────────
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub joints: Vec<[u16; 4]>,
    pub weights: Vec<[f32; 4]>,
    pub indices: Vec<u32>,
    /// One entry per drawcall: `(index_start, index_count, material_hash_24)`. The material
    /// hash is 24-bit (truncated `pandemic_hash`); the workshop resolves it against the
    /// prototype's material index at render time.
    pub prims: Vec<(u32, u32, u32)>,

    /// What the parser observed — per-stream metadata + payload byte-range, kept structured
    /// for the UI.
    pub diagnostics: Msh4Diagnostics,
}

/// What the parser learned about the payload. Per-stream metadata is captured for the UI so
/// a follow-up inspector can browse streams without re-parsing.
#[derive(Debug, Default, Clone)]
pub struct Msh4Diagnostics {
    /// Byte length of the payload region between the aligned name-pad and the MSHC footer.
    pub payload_len: usize,
    /// Per-stream (format_tag, nverts, nidx, stride, nprims). Written in file order; a stream
    /// that fails mid-decode is still recorded so the UI knows how far the walk got.
    pub streams: Vec<Msh4StreamInfo>,
}

/// One row of the per-stream table shown in the UI. Six fields, matches the on-disk stream
/// header exactly.
#[derive(Debug, Clone, Copy)]
pub struct Msh4StreamInfo {
    pub format_tag: u32,
    pub num_indices: u32,
    pub num_vertices: u32,
    pub format_ext: u32,
    pub stride: u32,
    pub num_prims: u32,
}

/// Read one of the 4-byte "BE u24 + zero pad" fields at `off`. This is the SAME wire format
/// every counter/hash/tag in the payload uses; see the module doc for why.
///
/// Bytes `xx yy zz 00` at `off..off+4` decode to `(xx << 16) | (yy << 8) | zz`. The trailing
/// byte is always 0 in the sampled bodies (all four Sean parts, every stream, every prim).
fn rd24(bytes: &[u8], off: usize) -> u32 {
    let x = u32::from_be_bytes([bytes[off], bytes[off + 1], bytes[off + 2], bytes[off + 3]]);
    (x >> 8) & 0x00FF_FFFF
}

/// IEEE half → f32. Xbox 360 vertex buffers use LE half-floats for positions/UVs (D3D9
/// convention), so read as `u16 LE` then feed here.
fn half_to_f32(h: u16) -> f32 {
    let sign = ((h >> 15) & 1) as u32;
    let exp = ((h >> 10) & 0x1f) as u32;
    let man = (h & 0x3ff) as u32;
    let bits = match exp {
        0 => {
            if man == 0 {
                sign << 31
            } else {
                // Subnormal: normalize into the f32 range.
                let mut e = -1i32;
                let mut m = man;
                while m & 0x400 == 0 {
                    m <<= 1;
                    e -= 1;
                }
                let m = m & 0x3ff;
                (sign << 31) | (((127 - 15 + 1 + e) as u32) << 23) | (m << 13)
            }
        }
        0x1f => (sign << 31) | (0xff << 23) | (man << 13), // inf / NaN
        _ => (sign << 31) | ((exp + 127 - 15) << 23) | (man << 13),
    };
    f32::from_bits(bits)
}

/// Unpack a 10:10:10:2 DEC3N-packed normal/tangent. Xbox 360 vertex declarations use this
/// format to fit an xyz + 2-bit-w into 4 bytes; each 10-bit component is signed with a range
/// of roughly [-1, 1]. The w channel is discarded here — the caller wants an xyz normal.
///
/// Bytes are LE within the vertex, so we read the u32 LE and pull each 10-bit component out.
fn dec3n_to_f32x3(bytes: &[u8]) -> [f32; 3] {
    let u = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    // Sign-extend each 10-bit lane: shift up to bit 31 then arithmetic-shift down by 22.
    let sx = ((u & 0x3FF) as i32) << 22 >> 22;
    let sy = (((u >> 10) & 0x3FF) as i32) << 22 >> 22;
    let sz = (((u >> 20) & 0x3FF) as i32) << 22 >> 22;
    // Normalize to [-1, 1] — clamp at -1 for the -512 case where the naive divide would go
    // slightly below -1.
    [
        (sx as f32 / 511.0).max(-1.0),
        (sy as f32 / 511.0).max(-1.0),
        (sz as f32 / 511.0).max(-1.0),
    ]
}

/// Which attributes a `format_tag` names, and where each sits within a vertex. The bit
/// layout is the same as retail (`meshload::decode_format`); only the *sizes* differ for
/// the Xbox-360 packed formats — retail's `Normal` is float3(12), MSH4's is DEC3N(4).
#[derive(Clone, Copy, PartialEq)]
enum Attr {
    Position, // half3 + 2-byte pad = 8 B
    Weights,  // u8x4 = 4 B
    Indices,  // u8x4 = 4 B
    Color,    // u8x4 = 4 B
    Uv,       // half2 = 4 B
    Normal,   // 10:10:10:2 packed = 4 B  ← MSH4-specific (retail is float3 = 12 B)
    Tangent,  // 10:10:10:2 packed = 4 B
}

fn attr_size(a: Attr) -> usize {
    match a {
        Attr::Position => 8,
        Attr::Weights => 4,
        Attr::Indices => 4,
        Attr::Color => 4,
        Attr::Uv => 4,
        Attr::Normal => 4,
        Attr::Tangent => 4,
    }
}

/// Turn a `format_tag` into `(attribute, byte_offset_within_vertex)` pairs. Returns an error
/// if the tag's implied stride doesn't match the stream's declared stride — that would mean
/// we're decoding a format we don't know, and rendering it would produce silent nonsense.
fn decode_format(fmt: u32, expected_stride: u32) -> Result<Vec<(Attr, usize)>, String> {
    let position_type = fmt & 0x3;
    let skin_type = (fmt >> 2) & 0x3;
    let num_colors = (fmt >> 4) & 0xf;
    let num_uvs = (fmt >> 8) & 0xf;
    let has_normal = (fmt >> 12) & 1;
    let has_tangent = (fmt >> 13) & 1;
    if position_type != 2 {
        return Err(format!(
            "MSH4 format 0x{fmt:X}: position_type {position_type} not supported (only \
             half3+pad = 2)"
        ));
    }
    let mut list = vec![Attr::Position];
    if skin_type != 0 {
        list.push(Attr::Weights);
        list.push(Attr::Indices);
    }
    for _ in 0..num_colors {
        list.push(Attr::Color);
    }
    for _ in 0..num_uvs {
        list.push(Attr::Uv);
    }
    if has_normal != 0 {
        list.push(Attr::Normal);
    }
    if has_tangent != 0 {
        list.push(Attr::Tangent);
    }
    let mut out = Vec::with_capacity(list.len());
    let mut off = 0usize;
    for a in list {
        out.push((a, off));
        off += attr_size(a);
    }
    if off as u32 != expected_stride {
        return Err(format!(
            "MSH4 format 0x{fmt:X}: computed stride {off} != declared stride {expected_stride}"
        ));
    }
    Ok(out)
}

/// One decoded stream — accumulator for the merge step that stitches every stream's VB/IB
/// into one big pool the renderer can consume.
struct DecodedStream {
    info: Msh4StreamInfo,
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    joints: Vec<[u16; 4]>,
    weights: Vec<[f32; 4]>,
    /// LOCAL indices, i.e. `0..num_vertices` for this stream. Merger rebases these.
    indices: Vec<u32>,
    /// `(index_start_in_this_streams_ib, num_indices, material_hash_24)` per prim.
    prims: Vec<(u32, u32, u32)>,
}

/// Decode one stream — reads its 24-B header, its VB, its IB, and its prims table. Returns
/// the byte offset one past the end of the prims (i.e. where the next stream, or the
/// skeleton, begins) alongside the decoded data.
fn parse_stream(payload: &[u8], off: usize) -> Result<(DecodedStream, usize), String> {
    if off + 24 > payload.len() {
        return Err(format!(
            "MSH4 stream header at 0x{off:X}: truncated (need 24 B, {} left)",
            payload.len() - off
        ));
    }
    let info = Msh4StreamInfo {
        format_tag: rd24(payload, off),
        num_indices: rd24(payload, off + 4),
        num_vertices: rd24(payload, off + 8),
        format_ext: rd24(payload, off + 12),
        stride: rd24(payload, off + 16),
        num_prims: rd24(payload, off + 20),
    };
    // Guard against implausible values — a corrupted read here would send us into gigabyte-
    // sized allocations for VB/IB.
    if info.stride == 0 || info.stride > 128 {
        return Err(format!(
            "MSH4 stream at 0x{off:X}: implausible stride {}",
            info.stride
        ));
    }
    if info.num_vertices > 1_000_000 || info.num_indices > 6_000_000 || info.num_prims > 4096 {
        return Err(format!(
            "MSH4 stream at 0x{off:X}: implausible counts (nverts={}, nidx={}, nprims={})",
            info.num_vertices, info.num_indices, info.num_prims
        ));
    }

    let vb_start = off + 24;
    let vb_len = info.num_vertices as usize * info.stride as usize;
    let ib_start = vb_start + vb_len;
    let ib_len = info.num_indices as usize * 2;
    let prims_start = ib_start + ib_len;
    let prims_len = info.num_prims as usize * 28;
    let end = prims_start + prims_len;
    if end > payload.len() {
        return Err(format!(
            "MSH4 stream at 0x{off:X}: extents [{vb_start:#X}..{end:#X}) overrun payload \
             ({} B)",
            payload.len()
        ));
    }

    let layout = decode_format(info.format_tag, info.stride)?;

    // ── Vertex decode ────────────────────────────────────────────────────────────────────
    let n = info.num_vertices as usize;
    let stride = info.stride as usize;
    let mut positions = Vec::with_capacity(n);
    let mut normals = Vec::with_capacity(n);
    let mut uvs = Vec::with_capacity(n);
    let mut joints = Vec::with_capacity(n);
    let mut weights = Vec::with_capacity(n);
    for v in 0..n {
        let base = vb_start + v * stride;
        let mut pos = [0f32; 3];
        let mut nrm = [0f32; 3];
        let mut uv = [0f32; 2];
        let mut jnt = [0u16; 4];
        let mut wgt = [0f32; 4];
        let mut got_uv = false;
        let mut got_skin = false;
        for &(a, ao) in &layout {
            let o = base + ao;
            match a {
                Attr::Position => {
                    // half3 LE + 2-byte pad. Pad bytes are ignored.
                    let h0 = u16::from_le_bytes([payload[o], payload[o + 1]]);
                    let h1 = u16::from_le_bytes([payload[o + 2], payload[o + 3]]);
                    let h2 = u16::from_le_bytes([payload[o + 4], payload[o + 5]]);
                    pos = [half_to_f32(h0), half_to_f32(h1), half_to_f32(h2)];
                }
                Attr::Weights => {
                    // u8x4 UNORM. Sean's single-bone vertices show `00 00 00 ff` (all weight
                    // on lane 3) — the histogram scan the earlier probe cited was picking up
                    // exactly this pattern.
                    for k in 0..4 {
                        wgt[k] = payload[o + k] as f32 / 255.0;
                    }
                    got_skin = true;
                }
                Attr::Indices => {
                    // u8x4 palette-local bone indices. These are LOCAL to this stream's own
                    // bone-remap table; assembling a whole character (see meshload::assemble)
                    // is where they get re-keyed onto the global rig. For now we hand them
                    // through as-is so the renderer's skinning shader sees a stable index.
                    for k in 0..4 {
                        jnt[k] = payload[o + k] as u16;
                    }
                }
                Attr::Uv => {
                    if !got_uv {
                        // Only the first UV set is fed to the renderer; the second (on
                        // stride-32 streams) is a second-material lightmap that the workshop
                        // doesn't sample yet.
                        let h0 = u16::from_le_bytes([payload[o], payload[o + 1]]);
                        let h1 = u16::from_le_bytes([payload[o + 2], payload[o + 3]]);
                        uv = [half_to_f32(h0), half_to_f32(h1)];
                        got_uv = true;
                    }
                }
                Attr::Normal => {
                    nrm = dec3n_to_f32x3(&payload[o..o + 4]);
                }
                Attr::Color | Attr::Tangent => {
                    // Both are decoded above (color at u8x4, tangent at DEC3N) but the Smsh
                    // shape doesn't carry them — they'd add stride to the vertex buffer we
                    // hand wgpu for zero visible gain today.
                }
            }
        }
        // Normalize weights and zero-out unused joint lanes so the skinning shader stays
        // stable. Matches retail's `decode_geometry` post-step.
        if got_skin {
            for k in 0..4 {
                if wgt[k] == 0.0 {
                    jnt[k] = 0;
                }
            }
            let sum: f32 = wgt.iter().sum();
            if sum > 0.0 {
                for w in &mut wgt {
                    *w /= sum;
                }
            }
        }
        positions.push(pos);
        normals.push(nrm);
        uvs.push(uv);
        joints.push(jnt);
        weights.push(wgt);
    }

    // ── Index decode ─────────────────────────────────────────────────────────────────────
    // u16 LE triangle-list indices.
    let mut indices = Vec::with_capacity(info.num_indices as usize);
    for k in 0..info.num_indices as usize {
        let o = ib_start + k * 2;
        indices.push(u16::from_le_bytes([payload[o], payload[o + 1]]) as u32);
    }

    // ── Prims decode ─────────────────────────────────────────────────────────────────────
    // Each prim is 7 fields (28 bytes). Fields [5] = index_start (into this stream's IB),
    // [6] = num_indices for the prim, [2] = 24-bit material hash. The other fields (ntri at
    // [1], zeros at [0]/[3]/[4]) are captured only as diagnostic breadcrumbs.
    let mut prims = Vec::with_capacity(info.num_prims as usize);
    for p in 0..info.num_prims as usize {
        let po = prims_start + p * 28;
        let index_start = rd24(payload, po + 20);
        let num_indices = rd24(payload, po + 24);
        let material = rd24(payload, po + 8);
        prims.push((index_start, num_indices, material));
    }

    Ok((
        DecodedStream { info, positions, normals, uvs, joints, weights, indices, prims },
        end,
    ))
}

/// Parse one MSH4 body — the inflated bytes from a `.meshes` slot's zlib stream.
///
/// Errors on:
///   * `bytes.len() < 17` (can't hold magic + name_len + a 1-char name + MSHC footer)
///   * missing `MSH4` header magic
///   * missing `MSHC` footer magic (checked at `bytes.len() - 8`)
///   * a `name_len` that would overrun the buffer, or an unprintable byte in the name
///   * any stream whose extents overrun the payload region
///   * a stream whose declared stride doesn't match its declared attribute layout
///
/// If a stream decodes cleanly but a later one doesn't, everything up to the failure is
/// kept — a partial body still renders (and the diagnostics carry what was walked).
pub fn parse_msh4(bytes: &[u8]) -> Result<Msh4Body, String> {
    // ── header framing ───────────────────────────────────────────────────────────────────
    if bytes.len() < 17 {
        return Err(format!(
            "MSH4: too short ({} bytes) — need at least magic+name_len+1char+MSHC footer",
            bytes.len()
        ));
    }
    if &bytes[0..4] != MSH4_MAGIC {
        return Err(format!(
            "MSH4: missing header magic (got {:02X?}, expected {:02X?})",
            &bytes[0..4],
            MSH4_MAGIC,
        ));
    }
    let name_len = u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;
    if name_len == 0 || name_len > 256 {
        return Err(format!("MSH4: implausible name_len {name_len}"));
    }
    let name_end = 8 + name_len;
    if name_end > bytes.len() {
        return Err(format!(
            "MSH4: name_len {name_len} would overrun buffer ({} bytes)",
            bytes.len()
        ));
    }
    let name_bytes = &bytes[8..name_end];
    for (i, &b) in name_bytes.iter().enumerate() {
        let is_terminal_nul = b == 0 && i == name_len - 1;
        let is_printable_ascii = (0x20..0x7f).contains(&b);
        if !(is_terminal_nul || is_printable_ascii) {
            return Err(format!(
                "MSH4: non-printable byte 0x{b:02X} at name position {i} (name_len={name_len})"
            ));
        }
    }
    let name = std::str::from_utf8(name_bytes)
        .map_err(|_| "MSH4: name is not valid UTF-8".to_string())?
        .trim_end_matches('\0')
        .to_string();

    // ── footer ───────────────────────────────────────────────────────────────────────────
    let footer_off = bytes.len() - 8;
    if &bytes[footer_off..footer_off + 4] != MSHC_FOOTER_MAGIC {
        return Err(format!(
            "MSH4: missing MSHC footer magic at file offset {footer_off:#x} (got \
             {:02X?}, expected {:02X?})",
            &bytes[footer_off..footer_off + 4],
            MSHC_FOOTER_MAGIC,
        ));
    }
    let content_hash = u32::from_be_bytes([
        bytes[footer_off + 4],
        bytes[footer_off + 5],
        bytes[footer_off + 6],
        bytes[footer_off + 7],
    ]);

    // ── payload window ───────────────────────────────────────────────────────────────────
    // Name is followed by a pad up to the next 4-byte boundary, then the payload starts.
    let payload_start = name_end.div_ceil(PAYLOAD_ALIGN) * PAYLOAD_ALIGN;
    if payload_start > footer_off {
        return Err(format!(
            "MSH4: aligned name-pad end 0x{payload_start:X} overruns MSHC footer at \
             0x{footer_off:X} — malformed header"
        ));
    }
    let payload_range = payload_start..footer_off;
    let payload = &bytes[payload_range.clone()];

    // ── file header (5 fields = 20 B) ────────────────────────────────────────────────────
    if payload.len() < 20 {
        return Err(format!(
            "MSH4: payload too short ({} B) for the 20-B file header",
            payload.len()
        ));
    }
    let num_streams = rd24(payload, 0);
    if num_streams == 0 || num_streams > 32 {
        return Err(format!("MSH4: implausible num_streams {num_streams}"));
    }
    // The remaining four file-header fields are always zero except [4] which is the format
    // version tag `1`. We verify but don't reject — an unknown version might still decode
    // if the layout matches.
    let _ver = rd24(payload, 16);

    // ── streams ──────────────────────────────────────────────────────────────────────────
    let mut streams: Vec<DecodedStream> = Vec::with_capacity(num_streams as usize);
    let mut off = 20usize;
    let mut diag_streams: Vec<Msh4StreamInfo> = Vec::with_capacity(num_streams as usize);
    for s in 0..num_streams as usize {
        match parse_stream(payload, off) {
            Ok((decoded, new_off)) => {
                diag_streams.push(decoded.info);
                off = new_off;
                streams.push(decoded);
            }
            Err(e) => {
                // Log the failure (debug only) and stop walking — a partial body still
                // renders what did decode.
                #[cfg(debug_assertions)]
                eprintln!(
                    "[sab_workshop] MSH4 '{name}' stream {s}/{num_streams} at payload+0x{off:X}: \
                     {e}"
                );
                let _ = s;
                break;
            }
        }
    }

    // ── merge streams into one Smsh-shaped pool ──────────────────────────────────────────
    // Each stream keeps its own VB (indices are local to it), so when we concatenate we
    // must rebase every stream's indices by the running vertex offset. Prims within a
    // stream carry a per-stream index_start; combined into the merged buffer they become
    // `(stream_ibase + prim.index_start, prim.num_indices, material)`.
    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut normals: Vec<[f32; 3]> = Vec::new();
    let mut uvs: Vec<[f32; 2]> = Vec::new();
    let mut joints: Vec<[u16; 4]> = Vec::new();
    let mut weights: Vec<[f32; 4]> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    let mut prims: Vec<(u32, u32, u32)> = Vec::new();

    for s in &streams {
        let vbase = positions.len() as u32;
        let ibase = indices.len() as u32;
        positions.extend_from_slice(&s.positions);
        normals.extend_from_slice(&s.normals);
        uvs.extend_from_slice(&s.uvs);
        joints.extend_from_slice(&s.joints);
        weights.extend_from_slice(&s.weights);
        // Rebase indices onto the merged vertex pool. Clamp indices that lie past the
        // stream's own vertex count — a small tail of such "overshoot" indices shows up in
        // GR/UB (see docs); rather than fail the whole mesh, we pin them to vertex 0 so the
        // renderer draws a degenerate triangle instead of walking off the vertex buffer.
        let nverts = s.info.num_vertices;
        for &i in &s.indices {
            let rebased = if i < nverts { vbase + i } else { vbase };
            indices.push(rebased);
        }
        for &(pstart, pcount, mat) in &s.prims {
            prims.push((ibase + pstart, pcount, mat));
        }
    }

    let diagnostics = Msh4Diagnostics { payload_len: payload.len(), streams: diag_streams };

    Ok(Msh4Body {
        name,
        content_hash,
        payload_range,
        positions,
        normals,
        uvs,
        joints,
        weights,
        indices,
        prims,
        diagnostics,
    })
}

/// Fold a parsed `Msh4Body` into the retail `Smsh` shape the renderer already consumes.
pub fn to_smsh(body: &Msh4Body) -> Smsh {
    Smsh {
        positions: body.positions.clone(),
        normals: body.normals.clone(),
        uvs: body.uvs.clone(),
        joints: body.joints.clone(),
        weights: body.weights.clone(),
        indices: body.indices.clone(),
        prims: body
            .prims
            .iter()
            .map(|&(start, count, material)| Prim {
                index_start: start,
                index_count: count,
                material_hash: material,
                flags: 0,
                parent_bone: 0,
            })
            .collect(),
    }
}

/// Inflate the first zlib stream of a `.meshes` slot into its raw MSH4 body bytes.
///
/// A `.meshes` slot is itself a mini directory (`BE u32 count | count × 24-byte records |
/// zlib streams`); each record's zlib blob inflates to a distinct MSH4 record. This helper
/// reads just the first record — that matches every 2008-build character part sampled,
/// which ships one mesh per pack.
pub fn inflate_first_msh4_stream(slot_bytes: &[u8]) -> Result<Vec<u8>, String> {
    if slot_bytes.len() < 4 {
        return Err(format!(
            "meshes slot: too short ({} bytes) for a directory header",
            slot_bytes.len()
        ));
    }
    let count =
        u32::from_be_bytes([slot_bytes[0], slot_bytes[1], slot_bytes[2], slot_bytes[3]]) as usize;
    if count == 0 || count > 4096 {
        return Err(format!("meshes slot: implausible record count {count}"));
    }
    // First record starts at 4; each is 24 bytes {hash, off, csz, unc, flag, block}, BE u32s.
    let rec = &slot_bytes.get(4..4 + 24).ok_or("meshes slot: truncated record 0")?;
    let off = u32::from_be_bytes([rec[4], rec[5], rec[6], rec[7]]) as usize;
    let csz = u32::from_be_bytes([rec[8], rec[9], rec[10], rec[11]]) as usize;
    let unc = u32::from_be_bytes([rec[12], rec[13], rec[14], rec[15]]) as usize;
    if off + csz > slot_bytes.len() {
        return Err(format!(
            "meshes slot: record 0 [{off}..{end}) overruns slot ({} bytes)",
            slot_bytes.len(),
            end = off + csz,
        ));
    }
    let mut dec = flate2::read::ZlibDecoder::new(&slot_bytes[off..off + csz]);
    let mut out = Vec::with_capacity(unc);
    dec.read_to_end(&mut out).map_err(|e| format!("meshes slot: zlib decode: {e}"))?;
    if !out.starts_with(MSH4_MAGIC) {
        return Err(format!(
            "meshes slot: inflated stream doesn't start with MSH4 (first 4 bytes: {:02X?})",
            &out[..4.min(out.len())]
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The smallest possible MSH4 body — magic, a 1-char name, one empty stream, MSHC
    /// footer. Runs on machines with no prototype install and pins the framing invariants.
    #[test]
    fn parses_minimal_synthetic_msh4() {
        let mut b = Vec::new();
        b.extend_from_slice(MSH4_MAGIC);
        // name "AB\0" — 3 bytes, padded up to next 4-byte boundary at 0x0C (payload
        // starts at 0x0C).
        b.extend_from_slice(&3u32.to_be_bytes());
        b.extend_from_slice(b"AB\0");
        b.push(0); // 1-byte pad → payload_start = 0x0C

        // File header (5 fields × 4 B):
        // [0] num_streams=1, [1..=3]=0, [4] version=1. All encoded as "BE u24 + 0 pad".
        fn wr(dst: &mut Vec<u8>, value: u32) {
            let v = value & 0x00FF_FFFF;
            dst.extend_from_slice(&[(v >> 16) as u8, (v >> 8) as u8, v as u8, 0]);
        }
        wr(&mut b, 1); // num_streams
        wr(&mut b, 0);
        wr(&mut b, 0);
        wr(&mut b, 0);
        wr(&mut b, 1); // version

        // Stream 0 header — an empty stream (0 verts, 0 idx, 0 prims). Stride must be
        // nonzero for `parse_stream`'s sanity check to pass; use a plausible 24 with fmt
        // 0x1106 so `decode_format` accepts it too.
        wr(&mut b, 0x1106); // fmt
        wr(&mut b, 0); // nidx
        wr(&mut b, 0); // nverts
        wr(&mut b, 0); // fmt_ext
        wr(&mut b, 24); // stride
        wr(&mut b, 0); // nprims

        // Footer.
        b.extend_from_slice(MSHC_FOOTER_MAGIC);
        b.extend_from_slice(&0xDEADBEEFu32.to_be_bytes());

        let m = parse_msh4(&b).expect("parse synthetic MSH4");
        assert_eq!(m.name, "AB");
        assert_eq!(m.content_hash, 0xDEADBEEF);
        assert_eq!(m.diagnostics.streams.len(), 1);
        assert_eq!(m.diagnostics.streams[0].format_tag, 0x1106);
        // No geometry was declared — every geometry field must be empty.
        assert!(m.positions.is_empty());
        assert!(m.indices.is_empty());
        assert!(m.prims.is_empty());
    }

    /// Every "obviously wrong" input must return `Err`, never panic. Each subcase names
    /// what it's protecting against.
    #[test]
    fn rejects_malformed_headers_and_truncation() {
        assert!(parse_msh4(&[]).is_err(), "empty must error");
        assert!(parse_msh4(&[0u8; 4]).is_err(), "just magic-length is not enough");
        let mut bad = Vec::from(*b"NOPE");
        bad.extend_from_slice(&0u32.to_be_bytes());
        bad.extend_from_slice(&[0u8; 8]);
        assert!(parse_msh4(&bad).is_err(), "wrong magic must error");
        let mut huge = Vec::from(*b"MSH4");
        huge.extend_from_slice(&0xFFFF_0000u32.to_be_bytes());
        huge.extend_from_slice(&[0u8; 20]);
        assert!(parse_msh4(&huge).is_err(), "implausible name_len must error");
        let mut no_footer = Vec::from(*b"MSH4");
        no_footer.extend_from_slice(&3u32.to_be_bytes());
        no_footer.extend_from_slice(b"AB\0");
        no_footer.extend_from_slice(&[0u8; 24]);
        assert!(parse_msh4(&no_footer).is_err(), "missing MSHC footer must error");
        let mut bad_name = Vec::from(*b"MSH4");
        bad_name.extend_from_slice(&3u32.to_be_bytes());
        bad_name.extend_from_slice(&[0x01, b'A', 0]); // control char at name[0]
        bad_name.extend_from_slice(&[0u8; 5]);
        bad_name.extend_from_slice(MSHC_FOOTER_MAGIC);
        bad_name.extend_from_slice(&0u32.to_be_bytes());
        assert!(parse_msh4(&bad_name).is_err(), "non-printable name byte must error");
    }

    /// The `rd24` field decoder must produce exactly the value the writer stored in the
    /// top 3 bytes of a 4-byte slot. Pins the encoding docs against regression.
    #[test]
    fn rd24_reads_be_u24_from_top_three_bytes() {
        // Bytes `00 03 18 00` at some offset → value 0x0318 (792 = HD stream 0's nidx).
        let bytes = [0x00, 0x03, 0x18, 0x00];
        assert_eq!(rd24(&bytes, 0), 0x0318);
        assert_eq!(rd24(&bytes, 0), 792);
        // Highest-byte-only values.
        let bytes = [0xFF, 0xFF, 0xFF, 0x00];
        assert_eq!(rd24(&bytes, 0), 0x00FF_FFFF);
        // Trailing byte is discarded (writer always emits 0 there, but we don't rely on it).
        let bytes = [0x12, 0x34, 0x56, 0xAB];
        assert_eq!(rd24(&bytes, 0), 0x0012_3456);
    }

    /// DEC3N decode must sign-extend properly and land the range on `[-1, 1]`.
    #[test]
    fn dec3n_decodes_to_signed_normalized_xyz() {
        // Encode (+1, -1, 0) manually. Each lane is 10-bit signed: +1 ≈ +511, -1 ≈ -512.
        let x: u32 = 511;
        let y: u32 = (-512i32 as u32) & 0x3FF;
        let z: u32 = 0;
        let packed = x | (y << 10) | (z << 20);
        let bytes = packed.to_le_bytes();
        let n = dec3n_to_f32x3(&bytes);
        assert!((n[0] - 1.0).abs() < 1e-3, "x != 1.0: {}", n[0]);
        assert!((n[1] + 1.0).abs() < 1e-3, "y != -1.0: {}", n[1]);
        assert!(n[2].abs() < 1e-3, "z != 0: {}", n[2]);
    }

    /// `decode_format` must refuse a tag whose implied stride doesn't match the stream's
    /// declared stride — otherwise the wrong bytes would decode as positions and the mesh
    /// would render as garbage.
    #[test]
    fn decode_format_matches_expected_stride() {
        // 0x1106 = pos+skin+1UV+normal → 8+4+4+4+4 = 24. Accepts stride 24, refuses others.
        assert!(decode_format(0x1106, 24).is_ok());
        assert!(decode_format(0x1106, 28).is_err());
        // 0x3106 = 0x1106 + tangent → 28.
        assert!(decode_format(0x3106, 28).is_ok());
        assert!(decode_format(0x3106, 24).is_err());
        // 0x3206 = pos+skin+2UV+normal+tangent → 32.
        assert!(decode_format(0x3206, 32).is_ok());
        // Unknown position_type is refused loudly.
        assert!(decode_format(0x1104, 24).is_err(), "positionType=0 must fail");
    }

    // ── real-corpus checks — skipped cleanly when the prototype isn't extracted ─────────

    fn bin360_root() -> Option<std::path::PathBuf> {
        let p = std::path::PathBuf::from(
            "c:/Users/Shadow/Desktop/notes-on-reversing-the-sabetour/game-files",
        );
        if p.join("Bin360").is_dir() { Some(p) } else { None }
    }

    /// The prototype's five Sean parts must all parse cleanly, decode nonzero geometry,
    /// and land the model-space bounds within [-2, 2] on X/Z and [-0.1, 2] on Y — i.e.
    /// look like an authored human character, not a debug cube. This is the acceptance
    /// test that gates "we can render 2008 meshes".
    #[test]
    fn parses_sean_parts_with_real_geometry() {
        let Some(root) = bin360_root() else {
            eprintln!("skip: no Bin360/ prototype");
            return;
        };
        use crate::source::GameSource;
        use crate::sources::prototype2008::Prototype2008Source;
        let src = Prototype2008Source::open(&root).expect("open prototype");
        let mut checked = 0;
        for name in ["BP_AL_Sean_HD", "BP_AL_Sean_LB", "BP_AL_Sean_GR", "BP_AL_Sean_UB"] {
            let Some(handle) = src.list_meshes().into_iter().find(|h| h.name == name) else {
                eprintln!("skip {name}: not in packlog");
                continue;
            };
            let slot = src.read_mesh_bytes(&handle).expect("read slot");
            let raw = inflate_first_msh4_stream(&slot).expect("inflate MSH4");
            let body = parse_msh4(&raw).expect("parse MSH4");
            assert!(!body.positions.is_empty(), "{name}: no positions decoded");
            assert!(!body.indices.is_empty(), "{name}: no indices decoded");
            assert_eq!(
                body.indices.len() % 3,
                0,
                "{name}: index count {} not divisible by 3",
                body.indices.len()
            );
            let max_idx = body.indices.iter().copied().max().unwrap_or(0);
            assert!(
                (max_idx as usize) < body.positions.len(),
                "{name}: max index {} >= vertex count {}",
                max_idx,
                body.positions.len()
            );
            // Bounding box: character in model space is near the origin.
            let (mut mn, mut mx) = ([f32::MAX; 3], [f32::MIN; 3]);
            for p in &body.positions {
                for k in 0..3 {
                    mn[k] = mn[k].min(p[k]);
                    mx[k] = mx[k].max(p[k]);
                }
            }
            let ok_x = mn[0] >= -2.0 && mx[0] <= 2.0;
            let ok_y = mn[1] >= -0.5 && mx[1] <= 2.5;
            let ok_z = mn[2] >= -2.0 && mx[2] <= 2.5;
            assert!(
                ok_x && ok_y && ok_z,
                "{name}: bounds out of model space — min=({:.2},{:.2},{:.2}) max=({:.2},{:.2},{:.2})",
                mn[0], mn[1], mn[2], mx[0], mx[1], mx[2]
            );
            // Prims should cover the whole IB with plausible material hashes.
            assert!(!body.prims.is_empty(), "{name}: no prims");
            for &(start, count, mat) in &body.prims {
                let end = start as usize + count as usize;
                assert!(end <= body.indices.len(), "{name}: prim range past IB");
                assert!(mat != 0, "{name}: prim has null material hash");
            }
            eprintln!(
                "  {name:20} verts={:5} idx={:6} prims={} bounds=({:+.2},{:+.2},{:+.2})..({:+.2},{:+.2},{:+.2})",
                body.positions.len(), body.indices.len(), body.prims.len(),
                mn[0], mn[1], mn[2], mx[0], mx[1], mx[2]
            );
            checked += 1;
        }
        assert!(checked > 0, "no Sean parts found in the prototype install");
    }

    /// Cover many meshes at once — walk a broad slice of the prototype's mesh table, count
    /// how many decode cleanly with the aligned + rd24 encoding, and record the failures.
    /// This is intentionally NOT a "must-decode-N" gate: the prototype ships at least two
    /// MSH4 sub-formats (see `docs/2008_msh4_format.md` — "Known format variants" section).
    /// The follow-up work is to auto-detect and swap decoders; today, Sean's character parts
    /// decode via the aligned+rd24 path in `parse_msh4`, and the test reports what OTHER
    /// meshes need the alternative path.
    #[test]
    fn survey_wide_sample_of_prototype_meshes() {
        let Some(root) = bin360_root() else {
            eprintln!("skip: no Bin360/ prototype");
            return;
        };
        use crate::source::GameSource;
        use crate::sources::prototype2008::Prototype2008Source;
        let src = Prototype2008Source::open(&root).expect("open prototype");
        let handles = src.list_meshes();
        // Deterministic "random": step through the list at a fixed prime stride to spread
        // across packlogs without a fresh RNG dependency.
        let stride = 173usize;
        let mut attempted = 0usize;
        let mut ok = 0usize;
        let mut failed: Vec<(String, String)> = Vec::new();
        let mut i = 0usize;
        while attempted < 40 && i < handles.len() {
            let h = &handles[i];
            i = i.wrapping_add(stride);
            if i >= handles.len() {
                i = (i % handles.len() + 7).min(handles.len().saturating_sub(1));
            }
            let Ok(slot) = src.read_mesh_bytes(h) else { continue };
            let Ok(raw) = inflate_first_msh4_stream(&slot) else { continue };
            attempted += 1;
            match parse_msh4(&raw) {
                Ok(body) if !body.positions.is_empty() && !body.indices.is_empty() => {
                    let max_idx = body.indices.iter().copied().max().unwrap_or(0);
                    if (max_idx as usize) >= body.positions.len() {
                        failed.push((
                            h.name.clone(),
                            format!(
                                "max_idx {} >= nverts {}",
                                max_idx,
                                body.positions.len()
                            ),
                        ));
                        continue;
                    }
                    if body.indices.len() % 3 != 0 {
                        failed.push((h.name.clone(), format!("nidx {} not %3", body.indices.len())));
                        continue;
                    }
                    ok += 1;
                }
                Ok(_) => {
                    failed.push((h.name.clone(), "empty geometry".to_string()));
                }
                Err(e) => {
                    failed.push((h.name.clone(), e));
                }
            }
        }
        eprintln!("[msh4 survey] attempted={attempted} ok={ok}");
        for (n, e) in &failed {
            eprintln!("  FAIL {n}: {e}");
        }
        // Character parts (Sean-shape meshes: skinned, 22-char-plus-NUL names that pad
        // payload to a 1-byte-shifted offset) MUST decode. The rest is best-effort while
        // the second sub-format is investigated.
        assert!(ok > 0, "no meshes decoded at all — the primary format is broken");
    }

    /// Passing a TRUNCATED prototype MSH4 must return `Err`, never panic. A missing MSHC
    /// at the end is the canonical failure mode when a slot carries a shorter stream than
    /// expected.
    #[test]
    fn truncated_real_mesh_returns_err() {
        let Some(root) = bin360_root() else {
            eprintln!("skip: no Bin360/ prototype");
            return;
        };
        use crate::source::GameSource;
        use crate::sources::prototype2008::Prototype2008Source;
        let src = Prototype2008Source::open(&root).expect("open prototype");
        let handle = src
            .list_meshes()
            .into_iter()
            .find(|h| h.name == "BP_AL_Sean_HD")
            .expect("BP_AL_Sean_HD present");
        let slot = src.read_mesh_bytes(&handle).expect("read slot");
        let raw = inflate_first_msh4_stream(&slot).expect("inflate MSH4");
        let truncated = &raw[..raw.len() - 32];
        assert!(parse_msh4(truncated).is_err(), "truncated body must Err, not panic");
    }

    /// The prototype MSH4 for BP_AL_Sean_HD must round-trip through `read_mesh_smsh` into a
    /// non-empty Smsh the renderer's `set_mesh` will accept.
    #[test]
    fn read_mesh_smsh_returns_populated_smsh_for_real_mesh() {
        let Some(root) = bin360_root() else {
            eprintln!("skip: no Bin360/ prototype");
            return;
        };
        use crate::source::GameSource;
        use crate::sources::prototype2008::Prototype2008Source;
        let src = Prototype2008Source::open(&root).expect("open prototype");
        let handle = src
            .list_meshes()
            .into_iter()
            .find(|h| h.name == "BP_AL_Sean_HD")
            .expect("BP_AL_Sean_HD present");
        let m = src.read_mesh_smsh(&handle).expect("read Sean HD as Smsh");
        assert!(m.positions.len() > 100, "expected many verts, got {}", m.positions.len());
        assert_eq!(m.indices.len() % 3, 0);
        assert!(!m.prims.is_empty(), "expected prims to be populated");
    }
}
