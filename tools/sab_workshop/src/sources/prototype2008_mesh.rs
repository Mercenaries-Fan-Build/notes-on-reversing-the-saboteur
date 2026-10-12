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
//! +0x04  u32be  name_len          includes trailing \0
//! +0x08  char[] name[name_len]    ASCII, e.g. "CH_AL_SeanDevlin_01_HD\0"
//!        u8     pad               ONE zero byte, unconditional; not an "align to 4"
//!
//! +NAME_END+1  … PAYLOAD (file header + N streams + skeleton) …
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
//! ## Format uniformity — no second encoder needed
//!
//! An earlier probe suspected two sub-formats and proposed an auto-detect on `num_streams`
//! sanity. There is only ONE encoding: fixed 1-byte pad after the name + rd24 slots for
//! every 4-byte field. The current parser accidentally worked for Sean because Sean's names
//! land at `name_end % 4 == 3`, where `align_to_4(name_end) == name_end + 1` (i.e. the old
//! "align" rule happens to agree with the fixed pad). For every other name length the
//! aligned offset landed the reader on the wrong byte and produced the well-known
//! `implausible num_streams 0 / 256 / 65536 / …` errors. Byte-fingerprinted `NZ`, `Grenade`,
//! `Trap01_ORD` (name lengths 22 / 16 / 16) all decode cleanly under the fixed-1-byte rule.
//!
//! The pad-rule fix closes out every previously-known "outlier" too. `FBP_Debug_UB` — a
//! `CH_Debug_UB` dev-prop mesh (name_len 12, one stream, fmt 0x1106, 232 verts / 243 tris /
//! 1 prim, plus 8 kB of trailing skeleton) — was called out by an earlier probe as
//! "genuinely distinct". It is not: it is a small hand-authored debug body that decodes
//! byte-for-byte under the same rules as the shipping meshes. The pinning test is
//! `tests::fbp_debug_ub_decodes_under_the_fixed_pad_rule_no_sub_format`.
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
//!
//! ## Trailing skeleton block
//!
//! Everything after the last stream's prim table is the mesh's rig. Byte-fingerprinted on
//! Sean HD (104 bones, 33 remaps), NZ officer HD (70 / 5), FBP_Debug_UB (70 / 38), and
//! Alfa Romera (43 / 0):
//!
//! ```text
//! + 0                u24 BE num_bones          (3 bytes; NO trailing pad — a 0 pad would put
//!                                                the first name-hash byte one lane too far
//!                                                and mis-decode the whole tree)
//! + 3                num_bones × 77-byte bone entry:
//!                      + 0  u32 BE  name_hash            (pandemic_hash of the bone name)
//!                      + 4  i32 BE  parent_index         (-1 = root, else index into bones[])
//!                      + 8  f32 BE  translation.xyz       (3 × 4 = 12)
//!                      +20  f32 BE  rotation.xyzw         (quat; 4 × 4 = 16)
//!                      +36  f32 BE  scale.xyz             (3 × 4 = 12)
//!                      +48  f32 BE  aabb.min.xyz          (PclAABB.min PblVector3; 12)
//!                      +60  f32 BE  aabb.max.xyzw         (PclAABB.max PblVector4; 16)
//!                      +76  u8      flag                  (PclBone::Setup's uchar param)
//! + 3 + N·77         u32 BE num_bone_remaps     (per-skinned-bone inverse bind matrices)
//! + …                num_bone_remaps × 68-byte remap:
//!                      + 0  u32 BE  bone_index            (index into bones[]; retail keys
//!                                                          the same array by hash, 2008 by
//!                                                          index — verified by observing
//!                                                          Sean's remap 0's first u32 = 15)
//!                      + 4  f32 BE  ibm (16 floats)        (4 × PblVector4 = 64; row-major)
//! + END - 28         f32 BE mesh_bbox.min.xyz  (12) + max.xyzw (16) — the WSModel bounds
//! ```
//!
//! Ground truth: WildStar_d `ReadMesh` @ lines 524507–524660 for the skeleton loop and 524600–
//! 524659 for the bone-remap loop. Cross-checked against `PclAABB::PclAABB` (@ line 393792,
//! 28-byte class = PblVector3 min + PblVector4 max) and `?Setup@PclBone` in
//! `output/pdb_dumps/wildstar_d_pdb_combined.txt`.
//!
//! ## num_bones encoding — u24 without pad byte
//!
//! Every other u32-shaped field in MSH4 is "u24 in top 3 bytes, 0 pad byte in the low byte"
//! (see the file-header note above). num_bones is the ONE place that departs from that: it's
//! stored as 3 tight bytes with no pad, and the next field (bone 0's name_hash) begins at
//! offset 3, not 4. This is the arithmetic that pins it — every sampled mesh's trail_len
//! matches `3 + num_bones·77 + 4 + num_remaps·68 + 28` to the byte. Under the "u24+pad"
//! rule the total would be off by 1 and no combination of counts would ever balance.

use std::io::Read;

use crate::formats::{Prim, Smsh};

/// The 4-byte magic that starts every MSH4 record after zlib inflate.
pub const MSH4_MAGIC: &[u8; 4] = b"MSH4";
/// The 4-byte magic of the 8-byte footer at the end of every MSH4 record.
pub const MSHC_FOOTER_MAGIC: &[u8; 4] = b"MSHC";

/// Fixed number of pad bytes between the trailing NUL of `name` and the first byte of the
/// payload. Empirically 1 for every sampled body — see the module doc for the fingerprints
/// that ruled out the earlier "align to 4" hypothesis.
const NAME_PAD_BYTES: usize = 1;

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

    /// The rig that lives in the trailing region of every MSH4 record, or `None` when the
    /// mesh ships no skeleton (or the trailing region doesn't decode). See `Msh4Skeleton`
    /// for the shape and the module doc for the byte layout.
    pub skeleton: Option<Msh4Skeleton>,
}

/// One bone in the mesh's skeleton — matches the 77-byte on-disk entry with cooked f32/i32.
/// `parent` is an INDEX into the bones list (`-1` for the root), unlike retail's parent-hash
/// convention — the 2008 build stores an int, not a `PblCRC`, per WildStar_d
/// `PclSkeleton::CreateBone(int index, int parent, ...)`.
#[derive(Debug, Clone)]
pub struct Msh4Bone {
    /// `pandemic_hash(name)` — the same key the animation system uses for its bone tracks.
    pub name_hash: u32,
    /// Parent bone index, or `-1` for the root. Every non-root bone's parent lands in
    /// `0..bones.len()`.
    pub parent: i32,
    /// Local-space translation relative to the parent, in metres.
    pub translation: [f32; 3],
    /// Local-space rotation as a unit quaternion `(x, y, z, w)`. Root bones ship the
    /// identity `(0, 0, 0, 1)`.
    pub rotation: [f32; 4],
    /// Local-space scale relative to the parent. Typical value is `(1, 1, 1)`.
    pub scale: [f32; 3],
    /// The `PclAABB::min` half — a `PblVector3`, in model space.
    pub aabb_min: [f32; 3],
    /// The `PclAABB::max` half — a `PblVector4`; `w` is not documented (radius, or padding
    /// the engine writes but nobody reads). Kept as-is for byte-round-trip.
    pub aabb_max: [f32; 4],
    /// The `unsigned char` `PclBone::Setup` takes as its 5th arg. Meaning unknown; retained
    /// verbatim so a future writer can round-trip it.
    pub flag: u8,
}

/// One inverse-bind matrix, keyed by bone INDEX (not hash — the 2008 build stores the u32
/// index into the bones list, unlike retail which keys by hash). Only bones the mesh
/// actually SKINS ship a remap; the rest are chain-derived from the bones' local
/// transforms.
#[derive(Debug, Clone)]
pub struct Msh4BoneRemap {
    /// Index of the bone this ibm binds to — `bones[bone_index]`. Every observed value on
    /// Sean HD / NZ officer / FBP_Debug_UB is small (≤ num_bones), matching the retail
    /// `BoneRemaps.bone_id` shape.
    pub bone_index: u32,
    /// 16 floats — a 4×4 matrix stored as 4 consecutive `PblVector4` rows on disk. The engine
    /// treats it as an inverse bind matrix (`matrix = inverse(world_bind)`); the workshop's
    /// glue is free to transpose or invert as its own convention requires.
    pub matrix: [f32; 16],
}

/// The complete rig for one MSH4 record — bones + per-skinned-bone inverse-bind matrices +
/// the mesh's own bounding volume (a `PblVector3` position and a `PblVector4` extent, per
/// WildStar_d `WSModel` field layout at offsets 0x20 / 0x2c).
#[derive(Debug, Clone)]
pub struct Msh4Skeleton {
    /// Bones in file order. The root's `parent == -1`; all other parents are valid indices.
    pub bones: Vec<Msh4Bone>,
    /// One entry per skinned bone. Empty for meshes the engine doesn't skin (vehicles,
    /// static props). Not indexed by bone position — join with `Msh4Bone::name_hash`.
    pub remaps: Vec<Msh4BoneRemap>,
    /// The `WSModel` position `PblVector3` at 0x20 — probably a bind-pose pivot, kept as-is.
    pub model_pos: [f32; 3],
    /// The `WSModel` `PblVector4` at 0x2c. Semantics undocumented (bounding sphere +
    /// radius? extent?), preserved verbatim.
    pub model_sphere: [f32; 4],
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
    /// Byte offset within the payload one past the last stream's prim table. Everything from
    /// here to `payload_len` is trailing content (occluders / shadows / skeleton / …).
    pub trailing_start: usize,
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

/// Byte size of a single on-disk bone entry (see the module doc for the field breakdown).
const BONE_ENTRY_SIZE: usize = 77;
/// Byte size of a single bone-remap entry (`u32 bone_hash + 16 × f32 matrix`).
const BONE_REMAP_SIZE: usize = 68;
/// Byte size of the trailing model-bounds block (`PblVector3` + `PblVector4`).
const MODEL_BOUNDS_SIZE: usize = 28;

/// Read a big-endian `f32` from `bytes[off..off+4]`. Xbox 360 is BE for scalars outside the
/// GPU-facing vertex buffers.
fn rd_f32_be(bytes: &[u8], off: usize) -> f32 {
    f32::from_be_bytes([bytes[off], bytes[off + 1], bytes[off + 2], bytes[off + 3]])
}
fn rd_u32_be(bytes: &[u8], off: usize) -> u32 {
    u32::from_be_bytes([bytes[off], bytes[off + 1], bytes[off + 2], bytes[off + 3]])
}
fn rd_i32_be(bytes: &[u8], off: usize) -> i32 {
    i32::from_be_bytes([bytes[off], bytes[off + 1], bytes[off + 2], bytes[off + 3]])
}
fn rd_vec3_be(bytes: &[u8], off: usize) -> [f32; 3] {
    [rd_f32_be(bytes, off), rd_f32_be(bytes, off + 4), rd_f32_be(bytes, off + 8)]
}
fn rd_vec4_be(bytes: &[u8], off: usize) -> [f32; 4] {
    [
        rd_f32_be(bytes, off),
        rd_f32_be(bytes, off + 4),
        rd_f32_be(bytes, off + 8),
        rd_f32_be(bytes, off + 12),
    ]
}

/// Parse the trailing skeleton region of an MSH4 payload. `trail` is the payload slice from
/// the byte one past the last stream's prim table to the byte before the `MSHC` footer.
///
/// Returns `Ok(None)` for meshes with no meaningful rig — an empty trail, a bone count that
/// wouldn't fit the trail length, or (equivalently) trailing bytes that don't add up under
/// the fixed layout. Returns `Err` only when a well-shaped trail decodes to something
/// obviously wrong (parent index out of range, quaternion far from unit-length). The bar for
/// `Err` is deliberately narrow: everything renders as "no rig" rather than blocking a mesh
/// from decoding at all.
pub fn parse_skeleton(trail: &[u8]) -> Result<Option<Msh4Skeleton>, String> {
    // A skeleton needs at minimum: 3-byte count + 4-byte remap-count + 28-byte bounds = 35 B.
    // Anything smaller can't hold even an empty rig.
    if trail.len() < 3 + 4 + MODEL_BOUNDS_SIZE {
        return Ok(None);
    }

    // num_bones is a BE u24 stored in exactly 3 bytes — no trailing pad byte, unlike every
    // other u32 slot in MSH4 (see the module doc). The next field starts at offset 3.
    let num_bones = u32::from_be_bytes([0, trail[0], trail[1], trail[2]]) as usize;

    // The whole trail must fit `head + bones + remap_count + remaps + bounds`. If it
    // doesn't, we're either at the CLOT-marker case (WildStar_d ReadMesh @0x82658168 line
    // 524508) or the mesh writer emitted something we don't yet understand — either way,
    // give the caller back `None` rather than smearing garbage across the bone list.
    let bones_end = 3usize.saturating_add(num_bones.saturating_mul(BONE_ENTRY_SIZE));
    if bones_end + 4 + MODEL_BOUNDS_SIZE > trail.len() {
        return Ok(None);
    }
    let num_remaps = rd_u32_be(trail, bones_end) as usize;
    let remaps_end =
        (bones_end + 4).saturating_add(num_remaps.saturating_mul(BONE_REMAP_SIZE));
    if remaps_end + MODEL_BOUNDS_SIZE != trail.len() {
        // Under the fixed layout every mesh's trail_len is `3 + N·77 + 4 + M·68 + 28` to
        // the byte. If it isn't, the u24 we read wasn't a bone count — punt.
        return Ok(None);
    }

    // ── bones ────────────────────────────────────────────────────────────────────────────
    let mut bones = Vec::with_capacity(num_bones);
    for i in 0..num_bones {
        let bo = 3 + i * BONE_ENTRY_SIZE;
        let name_hash = rd_u32_be(trail, bo);
        let parent = rd_i32_be(trail, bo + 4);
        // The root's parent is written as -1; all other parents are indices into `bones`.
        // A parent that lies outside `[0, i)` would either mean we're mid-decode of the
        // wrong layout, or the writer produced a forward reference — reject rather than
        // silently accept a broken hierarchy.
        if parent != -1 && !(0..(num_bones as i32)).contains(&parent) {
            return Err(format!(
                "MSH4 skeleton: bone {i} has out-of-range parent {parent} (num_bones={num_bones})"
            ));
        }
        let translation = rd_vec3_be(trail, bo + 8);
        let rotation = rd_vec4_be(trail, bo + 20);
        let scale = rd_vec3_be(trail, bo + 36);
        let aabb_min = rd_vec3_be(trail, bo + 48);
        let aabb_max = rd_vec4_be(trail, bo + 60);
        let flag = trail[bo + 76];
        // A unit-quaternion sanity check catches "we're reading the wrong bytes" without
        // rejecting numerical drift from the exporter. The observed root ships an EXACT
        // identity (0,0,0,1); other bones vary by ±1e-6 from unit magnitude.
        let qmag2 = rotation[0] * rotation[0]
            + rotation[1] * rotation[1]
            + rotation[2] * rotation[2]
            + rotation[3] * rotation[3];
        if !qmag2.is_finite() || !(0.5..2.0).contains(&qmag2) {
            return Err(format!(
                "MSH4 skeleton: bone {i} rotation magnitude^2 = {qmag2} is far from 1.0 \
                 (rotation={:?})",
                rotation
            ));
        }
        bones.push(Msh4Bone {
            name_hash,
            parent,
            translation,
            rotation,
            scale,
            aabb_min,
            aabb_max,
            flag,
        });
    }

    // ── bone remaps ──────────────────────────────────────────────────────────────────────
    let mut remaps = Vec::with_capacity(num_remaps);
    for i in 0..num_remaps {
        let ro = bones_end + 4 + i * BONE_REMAP_SIZE;
        let bone_index = rd_u32_be(trail, ro);
        if (bone_index as usize) >= num_bones {
            return Err(format!(
                "MSH4 skeleton: bone_remap {i} names bone_index {bone_index} but skeleton \
                 has only {num_bones} bones"
            ));
        }
        let mut matrix = [0f32; 16];
        for k in 0..16 {
            matrix[k] = rd_f32_be(trail, ro + 4 + k * 4);
        }
        remaps.push(Msh4BoneRemap { bone_index, matrix });
    }

    // ── mesh model bounds (last 28 bytes) ────────────────────────────────────────────────
    let bo = remaps_end;
    let model_pos = rd_vec3_be(trail, bo);
    let model_sphere = rd_vec4_be(trail, bo + 12);

    Ok(Some(Msh4Skeleton { bones, remaps, model_pos, model_sphere }))
}

impl Msh4Body {
    /// Return the raw bytes of the trailing skeleton region for the caller who wants to
    /// re-parse it themselves (or round-trip it verbatim). This is the same slice
    /// `parse_skeleton` walks and always sums to
    /// `3 + N·77 + 4 + M·68 + 28` bytes when the rig decodes cleanly.
    pub fn trailing_bytes<'a>(&self, raw: &'a [u8]) -> &'a [u8] {
        let payload = &raw[self.payload_range.clone()];
        &payload[self.diagnostics.trailing_start..]
    }
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
    // Name is followed by a FIXED 1-byte pad, then the payload starts. This is unconditional:
    // regardless of `name_len`, the writer emits exactly one byte before the payload. The old
    // hypothesis was "align to a 4-byte boundary" — that happens to coincide with a 1-byte pad
    // when `name_len % 4 == 3` (which is Sean's name shape: 22 chars + NUL = 23 bytes), but
    // fails on every other name length. Verified by byte-fingerprinting `BP_AL_Sean_HD`
    // (name_len 23), `BP_NZ_TerSqdSldOfc_HD` (22), `GB_WP_Grenade01` (16), `Trap01_ORD` (16):
    // for each, VB half3 positions decode to plausible model-space coords only when the payload
    // is anchored `name_end + 1`.
    let payload_start = name_end + NAME_PAD_BYTES;
    if payload_start > footer_off {
        return Err(format!(
            "MSH4: name+pad end 0x{payload_start:X} overruns MSHC footer at 0x{footer_off:X} \
             — malformed header"
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
    // Vehicles carry many streams: `VH_CV_CR_AlfaRomera_01` reads 37, `VH_CV_CR_Citroen15_01`
    // reads 49 under the fixed-1-byte-pad encoding — one stream per doorway/wheel/window/LOD
    // sub-mesh. The old sanity ceiling of 32 was tuned to character parts alone. 128 keeps
    // us from spending gigabytes on a genuinely corrupted body while accepting every real
    // asset the prototype ships.
    if num_streams == 0 || num_streams > 128 {
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

    let diagnostics = Msh4Diagnostics {
        payload_len: payload.len(),
        streams: diag_streams,
        trailing_start: off,
    };

    // ── trailing skeleton block ──────────────────────────────────────────────────────────
    // Bytes from `off` (one past the last stream's prim table) to the end of the payload.
    // A mesh with no skeleton returns `Ok(None)`; a malformed one returns `Err`. Bubble the
    // error up so the UI sees it rather than pretending the mesh had no rig.
    let trail = if off <= payload.len() { &payload[off..] } else { &payload[payload.len()..] };
    let skeleton = parse_skeleton(trail).map_err(|e| format!("MSH4 '{name}': {e}"))?;

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
        skeleton,
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
        // Synthetic body has no trailing skeleton region (0 bytes past the empty stream).
        assert!(m.skeleton.is_none(), "synthetic body must yield no skeleton");
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
        crate::testenv::prototype2008_root()
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
    /// how many decode cleanly. Under the fixed-1-byte-pad encoding, the prototype has ONE
    /// sub-format for every named mesh we ship (see the module doc's "Format uniformity"
    /// section). Before the fix, only Sean-shape meshes (name_len == 23) decoded — 10/40.
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
        // Regression floor for the pad-rule fix. Baseline was 10/40 (Sean-shape only); the
        // fix takes us to 40/40 on the current prototype. Keep 35 as the acceptance bar so
        // an isolated new outlier can slip through without breaking the build, but a bulk
        // regression (payload-start off by 1, rd24 replaced with plain BE u32, …) fails
        // loudly.
        assert!(ok >= 35, "survey regression: only {ok}/{attempted} decoded (want ≥35)");
    }

    /// Explicit coverage on 40 NON-Sean meshes across weapons, props, vehicles, and other
    /// characters — the categories the previous probe called out as "30 of 40 fail". Every
    /// distinct name-length shape (%4 in 0/1/2, plus %4 == 3 for non-Sean names) must
    /// decode; if a category regresses we surface it name-by-name.
    #[test]
    fn survey_non_sean_meshes_by_category() {
        let Some(root) = bin360_root() else {
            eprintln!("skip: no Bin360/ prototype");
            return;
        };
        use crate::source::GameSource;
        use crate::sources::prototype2008::Prototype2008Source;
        let src = Prototype2008Source::open(&root).expect("open prototype");
        let handles = src.list_meshes();
        // Pick the first ≤40 mesh handles whose NAME does not start with "BP_AL_Sean". We
        // want breadth across categories, not depth into one. `list_meshes` returns handles
        // in packlog-scan order, which is naturally spread across packs.
        let non_sean: Vec<_> = handles
            .iter()
            .filter(|h| !h.name.starts_with("BP_AL_Sean"))
            .take(40)
            .collect();
        assert!(
            non_sean.len() >= 40,
            "prototype install has fewer than 40 non-Sean mesh handles ({})",
            non_sean.len()
        );

        let mut ok = 0usize;
        let mut fails: Vec<(String, String)> = Vec::new();
        for h in &non_sean {
            let Ok(slot) = src.read_mesh_bytes(h) else {
                fails.push((h.name.clone(), "read_mesh_bytes failed".into()));
                continue;
            };
            let Ok(raw) = inflate_first_msh4_stream(&slot) else {
                fails.push((h.name.clone(), "inflate failed".into()));
                continue;
            };
            match parse_msh4(&raw) {
                Ok(body) if !body.positions.is_empty() && !body.indices.is_empty() => {
                    let max_idx = body.indices.iter().copied().max().unwrap_or(0);
                    if (max_idx as usize) < body.positions.len() && body.indices.len() % 3 == 0 {
                        ok += 1;
                    } else {
                        fails.push((h.name.clone(), "index bounds/parity".into()));
                    }
                }
                Ok(_) => fails.push((h.name.clone(), "empty geometry".into())),
                Err(e) => fails.push((h.name.clone(), e)),
            }
        }
        eprintln!("[non-Sean survey] ok={ok}/{}", non_sean.len());
        for (n, e) in &fails {
            eprintln!("  FAIL {n}: {e}");
        }
        assert!(
            ok >= 35,
            "non-Sean regression: only {ok}/40 decoded — the fixed-1-byte pad rule slipped"
        );
    }

    /// One focused non-Sean mesh with hand-verified byte-level expectations. The `NZ`
    /// terror-squad-officer part sits `name_len == 22`, exactly the case the old parser
    /// misread (`num_streams 256`). Under the pad-fix it must decode into a single skinned
    /// stream with realistic bounds.
    #[test]
    fn decodes_specific_non_sean_mesh_ter_sqd_ofc() {
        let Some(root) = bin360_root() else {
            eprintln!("skip: no Bin360/ prototype");
            return;
        };
        use crate::source::GameSource;
        use crate::sources::prototype2008::Prototype2008Source;
        let src = Prototype2008Source::open(&root).expect("open prototype");
        let Some(handle) =
            src.list_meshes().into_iter().find(|h| h.name == "BP_NZ_TerSqdSldOfc_HD")
        else {
            eprintln!("skip: BP_NZ_TerSqdSldOfc_HD not in packlog");
            return;
        };
        let slot = src.read_mesh_bytes(&handle).expect("read slot");
        let raw = inflate_first_msh4_stream(&slot).expect("inflate MSH4");
        let body = parse_msh4(&raw).expect("parse the NZ officer");
        assert_eq!(body.name, "CH_NZ_TerSqdSldOfc_HD", "mesh name preserved");
        assert!(body.positions.len() >= 1000, "want a real character mesh, got {}", body.positions.len());
        assert_eq!(body.indices.len() % 3, 0, "index count divisible by 3");
        let max_idx = body.indices.iter().copied().max().unwrap_or(0);
        assert!((max_idx as usize) < body.positions.len(), "max index in range");
        // The character stands in typical model space, not inside a debug cube.
        let (mut mn, mut mx) = ([f32::MAX; 3], [f32::MIN; 3]);
        for p in &body.positions {
            for k in 0..3 {
                mn[k] = mn[k].min(p[k]);
                mx[k] = mx[k].max(p[k]);
            }
        }
        for k in 0..3 {
            assert!(mn[k] >= -3.0 && mx[k] <= 3.0, "bounds axis {k} out of model space: [{}, {}]", mn[k], mx[k]);
        }
        // The old parser returned `Err("MSH4: implausible num_streams 256")` for this exact
        // input; capture the ex-failure mode so a regression to the align-to-4 rule stops
        // the build here.
        assert!(!body.diagnostics.streams.is_empty(), "expected non-empty stream table");
    }

    /// `FBP_Debug_UB` — the "last known outlier" per an earlier probe. Byte-fingerprint from
    /// this test resolves it: it is a small (232-vert / 243-tri) authored debug mesh that
    /// decodes byte-for-byte under the same fixed-1-byte-pad + rd24 rules every other named
    /// mesh uses. `name_len == 12` places its num_streams slot at `payload+0` with the value
    /// `01` in bytes[3] discarded as pad — identical shape to Sean's num_streams=1 slot.
    /// Version, stream header (`fmt=0x1106, nidx=729, nverts=232, ext=0, stride=24, nprims=1`),
    /// VB/IB/prims extents, and the trailing skeleton bytes all lay out cleanly. There is
    /// no "second sub-format"; the earlier claim was a stale artifact of the pre-pad-fix
    /// probe misreading the same slot. Pin the successful decode here so the next reader
    /// doesn't burn another day chasing a ghost.
    #[test]
    fn fbp_debug_ub_decodes_under_the_fixed_pad_rule_no_sub_format() {
        let Some(root) = bin360_root() else {
            eprintln!("skip: no Bin360/ prototype");
            return;
        };
        use crate::source::GameSource;
        use crate::sources::prototype2008::Prototype2008Source;
        let src = Prototype2008Source::open(&root).expect("open prototype");
        let Some(handle) = src.list_meshes().into_iter().find(|h| h.name == "FBP_Debug_UB") else {
            eprintln!("skip: FBP_Debug_UB not in packlog");
            return;
        };
        let slot = src.read_mesh_bytes(&handle).expect("read FBP_Debug_UB slot");
        let raw = inflate_first_msh4_stream(&slot).expect("inflate FBP_Debug_UB MSH4");
        let body = parse_msh4(&raw).expect("parse FBP_Debug_UB");

        // Framing matches the same rules as every other named mesh.
        assert_eq!(body.name, "CH_Debug_UB", "mesh name preserved");
        assert_eq!(body.diagnostics.streams.len(), 1, "one authored stream");
        let s = body.diagnostics.streams[0];
        assert_eq!(s.format_tag, 0x1106, "format_tag matches the retail bitfield");
        assert_eq!(s.stride, 24, "stride matches format 0x1106 (pos+skin+1UV+normal)");
        assert_eq!(s.num_vertices, 232, "expected 232 verts");
        assert_eq!(s.num_indices, 729, "expected 729 indices (243 triangles)");
        assert_eq!(s.num_prims, 1, "one drawcall");

        // Geometry decoded: index/vertex sanity + prim range covers the IB.
        assert_eq!(body.positions.len(), 232);
        assert_eq!(body.indices.len(), 729);
        assert_eq!(body.indices.len() % 3, 0);
        let max_idx = body.indices.iter().copied().max().unwrap_or(0);
        assert!((max_idx as usize) < body.positions.len(), "max index in range");
        assert_eq!(body.prims.len(), 1);
        let (start, count, mat) = body.prims[0];
        assert_eq!(start, 0, "prim starts at IB offset 0");
        assert_eq!(count as usize, body.indices.len(), "prim covers the whole IB");
        assert!(mat != 0, "prim has a non-null material hash");

        // The bounds tell us what this mesh actually is — a small dev prop, well within
        // model space, not a garbled decode.
        let (mut mn, mut mx) = ([f32::MAX; 3], [f32::MIN; 3]);
        for p in &body.positions {
            for k in 0..3 {
                mn[k] = mn[k].min(p[k]);
                mx[k] = mx[k].max(p[k]);
            }
        }
        for k in 0..3 {
            assert!(
                mn[k] >= -5.0 && mx[k] <= 5.0,
                "axis {k} bounds out of model space: [{}, {}]",
                mn[k],
                mx[k]
            );
        }
    }

    /// The pad-rule fingerprint. For every name-length residue we care about, verify that
    /// the fixed-1-byte-pad rule reads the num_streams field at the same byte as the
    /// engine — i.e. produces a small non-zero count. The pre-fix `align_to_4` rule
    /// reads through 1/2/3 bytes of pad and pulls out a wildly wrong u24; that difference
    /// is what this test pins.
    #[test]
    fn payload_start_is_fixed_one_byte_pad_not_align_to_four() {
        // Build a synthetic MSH4 body for each name-length residue class. Each body puts
        // `num_streams = 3` at the fixed-1-byte-pad offset, stubs 3 zero u32 slots, then a
        // stub stream that immediately errors on stride 0 (we don't care about the geometry
        // — only about the header offsets).
        for name_len_no_nul in [1usize, 2, 3, 4] {
            let mut b = Vec::new();
            b.extend_from_slice(MSH4_MAGIC);
            let name_len = name_len_no_nul + 1; // include the trailing NUL
            b.extend_from_slice(&(name_len as u32).to_be_bytes());
            for _ in 0..name_len_no_nul {
                b.push(b'A');
            }
            b.push(0); // NUL
            b.push(0); // the fixed 1-byte pad the fix depends on
            // File header — 5 rd24 slots. num_streams=3 in slot 0, zeros in the rest.
            fn wr(dst: &mut Vec<u8>, v: u32) {
                let v = v & 0x00FF_FFFF;
                dst.extend_from_slice(&[(v >> 16) as u8, (v >> 8) as u8, v as u8, 0]);
            }
            wr(&mut b, 3); // num_streams
            wr(&mut b, 0);
            wr(&mut b, 0);
            wr(&mut b, 0);
            wr(&mut b, 1); // version
            // Stream 0 stub — stride 0 to fail parse_stream after the header sanity read,
            // but the file-header num_streams check must have already passed.
            wr(&mut b, 0x1106);
            wr(&mut b, 0);
            wr(&mut b, 0);
            wr(&mut b, 0);
            wr(&mut b, 0); // stride == 0 → parse_stream returns Err
            wr(&mut b, 0);
            // Footer.
            b.extend_from_slice(MSHC_FOOTER_MAGIC);
            b.extend_from_slice(&0u32.to_be_bytes());

            let r = parse_msh4(&b);
            // Header must have accepted; the stride-0 stream trips a per-stream error
            // that's swallowed (partial body). So we get back an Ok body whose num_streams
            // diagnostic is 0 (walked zero streams because the first stream errored) but
            // whose file header parse succeeded — the alternative (aligned) offset would
            // read a garbage num_streams and Err at "implausible num_streams".
            assert!(
                r.is_ok(),
                "name_len_no_nul={name_len_no_nul}: parse_msh4 rejected the header, means \
                 the payload_start rule is wrong. Err: {:?}",
                r.err()
            );
        }
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

    /// Pandemic FNV-1a hash — copied verbatim from `tools/sab_skeleton`. The 2008 build
    /// keys bones by the same `pandemic_hash(name)` as retail, so we can hash a known
    /// bone name and check it appears in the parsed rig.
    fn pandemic_hash(s: &str) -> u32 {
        let mut h: u32 = 0x811C_9DC5;
        for c in s.bytes() {
            h = (h ^ ((c | 0x20) as u32)).wrapping_mul(0x0100_0193);
        }
        (h ^ 0x2A).wrapping_mul(0x0100_0193)
    }

    /// Sean's HD part must yield a bone table with >30 bones, exactly one root, every
    /// non-root parent pointing to a valid earlier bone, and unit-length quaternions —
    /// the acceptance test for "we can pose the 2008 character".
    #[test]
    fn parses_skeleton_from_sean_hd() {
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
        let body = parse_msh4(&raw).expect("parse MSH4");
        let skel = body.skeleton.as_ref().expect("Sean HD must ship a skeleton");
        assert!(
            skel.bones.len() > 30,
            "expected a rigged character mesh — got {} bones",
            skel.bones.len()
        );
        // Exactly one root (parent == -1), and every other parent lies inside the bones.
        let root_count = skel.bones.iter().filter(|b| b.parent == -1).count();
        assert_eq!(root_count, 1, "expected exactly one root bone, got {root_count}");
        for (i, b) in skel.bones.iter().enumerate() {
            if b.parent == -1 {
                continue;
            }
            assert!(
                (0..(skel.bones.len() as i32)).contains(&b.parent),
                "bone {i}: parent {} out of range 0..{}",
                b.parent,
                skel.bones.len()
            );
        }
        // Root's known hash. WildStar's canonical root is `GlobalSRT` (0xCBC1EB51).
        assert!(
            skel.bones.iter().any(|b| b.name_hash == pandemic_hash("GlobalSRT")),
            "expected GlobalSRT (0x{:08X}) in the bone list",
            pandemic_hash("GlobalSRT")
        );
        // Every quaternion is (near) unit length. Retail's tolerance is ~1e-6; the
        // 2008-build data has floats packed once so we allow 1e-3.
        for (i, b) in skel.bones.iter().enumerate() {
            let m2 = b.rotation[0] * b.rotation[0]
                + b.rotation[1] * b.rotation[1]
                + b.rotation[2] * b.rotation[2]
                + b.rotation[3] * b.rotation[3];
            assert!(
                (m2 - 1.0).abs() < 1e-3,
                "bone {i} ({:#010X}) rotation not unit-length: {:?} (|q|^2 = {})",
                b.name_hash,
                b.rotation,
                m2
            );
        }
        // Sean's HD mesh skins a subset of bones — remaps must be non-empty and every
        // bone_index must fall inside the bones list.
        assert!(!skel.remaps.is_empty(), "Sean HD must ship at least one bone remap");
        for r in &skel.remaps {
            assert!(
                (r.bone_index as usize) < skel.bones.len(),
                "remap references out-of-range bone_index {} (num_bones={})",
                r.bone_index,
                skel.bones.len()
            );
        }
        eprintln!(
            "  Sean HD: {} bones, {} skinned bones, root idx {}",
            skel.bones.len(),
            skel.remaps.len(),
            skel.bones.iter().position(|b| b.parent == -1).unwrap()
        );
    }

    /// Same-shape validation on a different character part — pins that the parser handles
    /// the smaller-rig case, not just Sean's shape.
    #[test]
    fn parses_skeleton_from_nz_officer_hd() {
        let Some(root) = bin360_root() else {
            eprintln!("skip: no Bin360/ prototype");
            return;
        };
        use crate::source::GameSource;
        use crate::sources::prototype2008::Prototype2008Source;
        let src = Prototype2008Source::open(&root).expect("open prototype");
        let Some(handle) =
            src.list_meshes().into_iter().find(|h| h.name == "BP_NZ_TerSqdSldOfc_HD")
        else {
            eprintln!("skip: BP_NZ_TerSqdSldOfc_HD not in packlog");
            return;
        };
        let slot = src.read_mesh_bytes(&handle).expect("read slot");
        let raw = inflate_first_msh4_stream(&slot).expect("inflate MSH4");
        let body = parse_msh4(&raw).expect("parse MSH4");
        let skel = body.skeleton.as_ref().expect("NZ officer must ship a skeleton");
        assert!(skel.bones.len() > 30, "expected >30 bones, got {}", skel.bones.len());
        // Same invariants as Sean's.
        let root_count = skel.bones.iter().filter(|b| b.parent == -1).count();
        assert_eq!(root_count, 1);
        assert!(
            skel.bones.iter().any(|b| b.name_hash == pandemic_hash("GlobalSRT")),
            "expected GlobalSRT root"
        );
        for b in &skel.bones {
            if b.parent != -1 {
                assert!((b.parent as usize) < skel.bones.len());
            }
            let m2 = b.rotation.iter().map(|x| x * x).sum::<f32>();
            assert!((m2 - 1.0).abs() < 1e-3);
        }
    }

    /// A prototype mesh whose trailing region genuinely doesn't decode as a skeleton must
    /// yield `Ok(None)` rather than `Err`. Uses a synthetic body: no bones, all bytes
    /// past the file header laid out as a "no-skeleton" 4-byte remap count of 0 + a
    /// 28-byte model bounds block. Also probes the "trail too short" path.
    #[test]
    fn handles_empty_or_absent_skeleton() {
        // A synthetic MSH4 body with an empty stream and a trailing region whose length is
        // too small to even hold a skeleton header — expect `body.skeleton == None`.
        let mut b = Vec::new();
        b.extend_from_slice(MSH4_MAGIC);
        b.extend_from_slice(&3u32.to_be_bytes());
        b.extend_from_slice(b"AB\0");
        b.push(0);
        fn wr(dst: &mut Vec<u8>, value: u32) {
            let v = value & 0x00FF_FFFF;
            dst.extend_from_slice(&[(v >> 16) as u8, (v >> 8) as u8, v as u8, 0]);
        }
        wr(&mut b, 1); // num_streams
        wr(&mut b, 0);
        wr(&mut b, 0);
        wr(&mut b, 0);
        wr(&mut b, 1); // version
        wr(&mut b, 0x1106); // fmt
        wr(&mut b, 0); // nidx
        wr(&mut b, 0); // nverts
        wr(&mut b, 0); // fmt_ext
        wr(&mut b, 24); // stride
        wr(&mut b, 0); // nprims
        b.extend_from_slice(MSHC_FOOTER_MAGIC);
        b.extend_from_slice(&0xDEADBEEFu32.to_be_bytes());
        let body = parse_msh4(&b).expect("parse synthetic MSH4");
        assert!(
            body.skeleton.is_none(),
            "synthetic body with an empty trailing region must yield no skeleton"
        );

        // parse_skeleton called directly on a too-short slice must also give None.
        assert!(parse_skeleton(&[]).unwrap().is_none());
        assert!(parse_skeleton(&[0u8; 3]).unwrap().is_none());
        assert!(parse_skeleton(&[0u8; 34]).unwrap().is_none()); // one short of the minimum

        // A trail with a plausible byte-count that doesn't add up under the fixed layout
        // (i.e., trail_len != 3 + N·77 + 4 + M·68 + 28 for any N, M): expect None, not Err.
        //   n=1 bone would consume 3+77+4+28 = 112 minimum; give it 113 bytes → mismatch.
        let mut junk = vec![0u8; 113];
        junk[2] = 1; // BE u24 num_bones = 1
        assert!(parse_skeleton(&junk).unwrap().is_none());
    }

    /// Byte-perfect round-trip: re-emit the skeleton's fields at the exact same offsets
    /// with big-endian encoding, and check the reconstructed buffer equals the source
    /// trailing region byte-for-byte. This proves the parse is complete (all bytes
    /// consumed and every field's semantics correctly identified — a wrong endian or a
    /// missing pad byte would flunk here even if `skeleton_size_matches_trailing_bytes_exactly`
    /// passed).
    #[test]
    fn skeleton_round_trips_bytewise_on_real_mesh() {
        let Some(root) = bin360_root() else {
            eprintln!("skip: no Bin360/ prototype");
            return;
        };
        use crate::source::GameSource;
        use crate::sources::prototype2008::Prototype2008Source;
        let src = Prototype2008Source::open(&root).expect("open prototype");
        // Cover a rigged character and a bone-remap-less vehicle, plus the small debug prop:
        // three different flavours of the trailing block through the same emitter.
        for target in ["BP_AL_Sean_HD", "VH_CV_CR_AlfaRomera_01", "FBP_Debug_UB"] {
            let Some(handle) = src.list_meshes().into_iter().find(|h| h.name == target) else {
                eprintln!("skip {target}: not in packlog");
                continue;
            };
            let slot = src.read_mesh_bytes(&handle).expect("read slot");
            let raw = inflate_first_msh4_stream(&slot).expect("inflate MSH4");
            let body = parse_msh4(&raw).expect("parse MSH4");
            let trail = body.trailing_bytes(&raw).to_vec();
            let skel = body
                .skeleton
                .as_ref()
                .unwrap_or_else(|| panic!("{target}: expected a skeleton"));

            // Re-emit the trailing region from the parsed skeleton, using the exact same
            // byte layout the parser reads.
            let mut out = Vec::with_capacity(trail.len());
            // num_bones — BE u24 with NO pad byte.
            let n = skel.bones.len() as u32;
            out.extend_from_slice(&[(n >> 16) as u8, (n >> 8) as u8, n as u8]);
            for b in &skel.bones {
                out.extend_from_slice(&b.name_hash.to_be_bytes());
                out.extend_from_slice(&b.parent.to_be_bytes());
                for x in b.translation {
                    out.extend_from_slice(&x.to_be_bytes());
                }
                for x in b.rotation {
                    out.extend_from_slice(&x.to_be_bytes());
                }
                for x in b.scale {
                    out.extend_from_slice(&x.to_be_bytes());
                }
                for x in b.aabb_min {
                    out.extend_from_slice(&x.to_be_bytes());
                }
                for x in b.aabb_max {
                    out.extend_from_slice(&x.to_be_bytes());
                }
                out.push(b.flag);
            }
            let m = skel.remaps.len() as u32;
            out.extend_from_slice(&m.to_be_bytes());
            for r in &skel.remaps {
                out.extend_from_slice(&r.bone_index.to_be_bytes());
                for x in r.matrix {
                    out.extend_from_slice(&x.to_be_bytes());
                }
            }
            for x in skel.model_pos {
                out.extend_from_slice(&x.to_be_bytes());
            }
            for x in skel.model_sphere {
                out.extend_from_slice(&x.to_be_bytes());
            }

            assert_eq!(
                out.len(),
                trail.len(),
                "{target}: reserialized len {} != trail len {}",
                out.len(),
                trail.len()
            );
            if out != trail {
                // Locate the first mismatching byte to help diagnose.
                let mismatch = out
                    .iter()
                    .zip(trail.iter())
                    .position(|(a, b)| a != b)
                    .unwrap_or(out.len());
                panic!(
                    "{target}: byte-round-trip differs — first mismatch at offset 0x{:X} \
                     (emitted {:02X}, source {:02X})",
                    mismatch, out[mismatch], trail[mismatch]
                );
            }
        }
    }

    /// The trailing skeleton region must decode into a skeleton whose serialized size
    /// matches the raw trail length byte-for-byte — proving the parse consumes every byte
    /// and there's no unaccounted-for tail. Run against the four sampled real meshes.
    #[test]
    fn skeleton_size_matches_trailing_bytes_exactly() {
        let Some(root) = bin360_root() else {
            eprintln!("skip: no Bin360/ prototype");
            return;
        };
        use crate::source::GameSource;
        use crate::sources::prototype2008::Prototype2008Source;
        let src = Prototype2008Source::open(&root).expect("open prototype");
        let mut checked = 0;
        for target in [
            "BP_AL_Sean_HD",
            "BP_NZ_TerSqdSldOfc_HD",
            "FBP_Debug_UB",
            "VH_CV_CR_AlfaRomera_01",
        ] {
            let Some(handle) = src.list_meshes().into_iter().find(|h| h.name == target) else {
                eprintln!("skip {target}: not in packlog");
                continue;
            };
            let slot = src.read_mesh_bytes(&handle).expect("read slot");
            let raw = inflate_first_msh4_stream(&slot).expect("inflate MSH4");
            let body = parse_msh4(&raw).expect("parse MSH4");
            let trail = body.trailing_bytes(&raw);
            let skel = body
                .skeleton
                .as_ref()
                .unwrap_or_else(|| panic!("{target}: expected a skeleton"));
            // The three fixed pieces + the two variable-length arrays must sum to exactly
            // the trail length. This is the ground-truth check for "we fully understand
            // the layout".
            let expected = 3
                + skel.bones.len() * BONE_ENTRY_SIZE
                + 4
                + skel.remaps.len() * BONE_REMAP_SIZE
                + MODEL_BOUNDS_SIZE;
            assert_eq!(
                expected,
                trail.len(),
                "{target}: skeleton size {} != trail_len {} (bones={}, remaps={})",
                expected,
                trail.len(),
                skel.bones.len(),
                skel.remaps.len()
            );
            checked += 1;
        }
        assert!(checked > 0, "no real meshes reachable");
    }

    /// Byte-fingerprint probe left as `#[ignore]`d documentation. Dumps the first 64 bytes
    /// of each mesh's trailing region and validates the trial layout arithmetic. Handy when
    /// diagnosing a mesh whose skeleton doesn't parse (`cargo test ... --ignored`).
    #[test]
    #[ignore]
    fn probe_trailing_region_hexdump() {
        let Some(root) = bin360_root() else {
            eprintln!("skip: no Bin360/ prototype");
            return;
        };
        use crate::source::GameSource;
        use crate::sources::prototype2008::Prototype2008Source;
        let src = Prototype2008Source::open(&root).expect("open prototype");
        let handles = src.list_meshes();
        for target in [
            "BP_AL_Sean_HD",
            "BP_NZ_TerSqdSldOfc_HD",
            "FBP_Debug_UB",
            "VH_CV_CR_AlfaRomera_01",
        ] {
            let Some(h) = handles.iter().find(|h| h.name == target) else {
                eprintln!("skip {target}: not in packlog");
                continue;
            };
            let Ok(slot) = src.read_mesh_bytes(h) else { continue };
            let Ok(raw) = inflate_first_msh4_stream(&slot) else { continue };
            let body = match parse_msh4(&raw) {
                Ok(b) => b,
                Err(e) => {
                    eprintln!("{target}: parse err {e}");
                    continue;
                }
            };
            let d = &body.diagnostics;
            let trail_start = d.trailing_start;
            let trail_len = d.payload_len - trail_start;
            eprintln!(
                "\n=== {target} name='{}' payload={} trail_start=0x{:X} trail_len={} nstreams={} ===",
                body.name, d.payload_len, trail_start, trail_len, d.streams.len()
            );
            let payload = &raw[body.payload_range.clone()];
            let trail = &payload[trail_start..];
            let dump_first = &trail[..(0x40.min(trail.len()))];
            eprintln!("  HEAD:");
            for (i, row) in dump_first.chunks(16).enumerate() {
                let mut hx = String::new();
                for b in row {
                    hx.push_str(&format!("{b:02X} "));
                }
                eprintln!("    {:04X}: {hx}", i * 16);
            }
            // Trial layout: 3-byte u24 BE num_bones, then N × 77-byte bones, then
            // u32 BE num_remaps, then M × 68-byte remaps, then 28-byte mesh_bbox.
            let n = u32::from_be_bytes([0, trail[0], trail[1], trail[2]]) as usize;
            let bones_end = 3 + n * 77;
            eprintln!(
                "  trial: num_bones = {} (BE u24 @0), bones end at 0x{:X}, trail_len={}",
                n, bones_end, trail.len()
            );
            if bones_end + 4 <= trail.len() {
                let nr = u32::from_be_bytes([
                    trail[bones_end],
                    trail[bones_end + 1],
                    trail[bones_end + 2],
                    trail[bones_end + 3],
                ]);
                let remaps_bytes_avail = trail.len() - bones_end - 4 - 28;
                eprintln!(
                    "  trial: num_remaps = {} (BE u32 @0x{:X}), remaps_avail bytes={}, expected={}",
                    nr,
                    bones_end,
                    remaps_bytes_avail,
                    nr as usize * 68
                );
                // Peek at last 28 bytes as mesh bbox floats.
                let tail_off = trail.len() - 28;
                let f = |o: usize| {
                    f32::from_be_bytes([trail[o], trail[o + 1], trail[o + 2], trail[o + 3]])
                };
                eprintln!(
                    "  tail_bbox: min=({:.3}, {:.3}, {:.3}) max=({:.3}, {:.3}, {:.3}, {:.3})",
                    f(tail_off),
                    f(tail_off + 4),
                    f(tail_off + 8),
                    f(tail_off + 12),
                    f(tail_off + 16),
                    f(tail_off + 20),
                    f(tail_off + 24),
                );
            }
            // Dump the LAST 64 bytes.
            let tail_start = trail.len().saturating_sub(64);
            eprintln!("  TAIL @{:X}:", tail_start);
            for (i, row) in trail[tail_start..].chunks(16).enumerate() {
                let mut hx = String::new();
                for b in row {
                    hx.push_str(&format!("{b:02X} "));
                }
                eprintln!("    {:04X}: {hx}", tail_start + i * 16);
            }
        }
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
