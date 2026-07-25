//! sab_probe — ask The Saboteur's assets questions. READ-ONLY: it writes no files and changes
//! nothing; it reports what is actually in the game data.
//!
//! WHY THIS EXISTS. Investigations used to be bolted onto the extraction tools as env vars and
//! one-off flags, which convolutes them and leaves probe code behind in shipping paths. Questions
//! live here; `sab_mesh` / `sab_skeleton` stay about extraction.
//!
//! WHAT IT FOUND (`parts`, the reason it was written):
//!   * A character's parts each carry their OWN skeleton with their own bone count (HD=168,
//!     UB=182, LB=182, GR=191, HAT=105), and bone INDEX N is NOT the same bone in every part. The
//!     pipeline extracts one skeleton — from whichever part has the most bones, i.e. the GLOVE —
//!     and poses the merged mesh against it.
//!   * Every skinned bone ships its own inverse-bind in the mesh's BoneRemaps. Where two parts
//!     skin the same bone those stored matrices AGREE with each other (trustworthy ground truth)
//!     and DISAGREE with the `world[i] = world[parent] · localTMS[i]` chain the tools derive.
//!   * The chain cannot be repaired from what it reads: a character's face bones have local
//!     translations of (0,0,0) on disk, so the chain collapses that subtree onto a single point.
//!     The stored inverse-bind is the only place those offsets exist.
//!
//! None of it is visible at bind pose: `jointMatrix = world · inv_bind` comes out identity for ANY
//! self-consistent-but-wrong chain, so the mesh renders perfectly until a clip plays.
//!
//! The container / MESH / skeleton parsing below is COPIED from `sab_skeleton` — the house pattern
//! (see `sab_workshop`, which copies from `sab_havok65` / `sab_dtex` / `sab_pack`). The probe reads
//! the GAME, not the tools.
//!
//! Pipeline (all empirically validated against `Global/Dynamic0.megapack`, PC/GOG build):
//!
//!   .megapack (magic "00PM", 64-bit index)                                  [container]
//!     -> SBLA sub-pack ("ALBS")                                             [uncompressed container]
//!         -> MSHA wrapper ("AHSM"): 20-byte header + 256-byte ASCII name    [uncompressed]
//!             -> MESH body (zlib, header 0x78 0x01) -> flat MESH + skeleton  [compressed]
//!
//! The MSHA header (magic, sizes, and the 256-byte asset name) is stored *uncompressed*
//! inside the SBLA sub-pack, so this tool locates meshes by scanning the raw megapack for
//! the "AHSM" magic and reading the ASCII name directly — no external string dictionary or
//! `global.map` navigation required. Character meshes are named `CH_AL_<Name>_<part>`.
//!
//! Layout confirmed from PredatorCZ/SaboteurToolset (`mesh/mesh_to_gltf.cpp`,
//! `include/meshpack.hpp`) and cross-checked byte-for-byte against real assets. The engine
//! codename is "WildStar" (WS* classes; e.g. WSSkeletonBone @ decomp VA 0x0132b22c).
//!
//! name_hash == boneName0 == pandemic_hash(bone name). Verified: 0xCBC1EB51="GlobalSRT" (root),
//! 0x24C5009C="Bone_Hips", 0x4C7733ED="Bone_Chest", 0x705C4508="Bone_Head", etc., and these
//! exact hashes are the keys the animation system uses (they appear in Animations.pack's ANIM
//! bone list, e.g. offset ~1807).
// The copied format code documents the whole on-disk MESH layout; a probe reads only part of it.
#![allow(dead_code)]

use std::io::Read;

mod tex;

// ---------------------------------------------------------------------------
// Little-endian scalar readers
// ---------------------------------------------------------------------------
fn u32(b: &[u8], o: usize) -> u32 { u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]) }
fn i16(b: &[u8], o: usize) -> i16 { i16::from_le_bytes([b[o], b[o + 1]]) }
fn u64(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes([b[o], b[o+1], b[o+2], b[o+3], b[o+4], b[o+5], b[o+6], b[o+7]])
}
fn f32a(b: &[u8], o: usize) -> f32 { f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]) }

/// pandemic_hash (FNV-1a variant, Pandemic Studios). Case-folded by `|0x20` on the raw byte.
/// Verified `pandemic_hash("ANY") == 0xED057225`.
fn pandemic_hash(s: &str) -> u32 {
    let mut h: u32 = 0x811C9DC5;
    for c in s.bytes() {
        h = (h ^ ((c | 0x20) as u32)).wrapping_mul(0x0100_0193);
    }
    (h ^ 0x2A).wrapping_mul(0x0100_0193)
}

// ---------------------------------------------------------------------------
// 4x4 matrix math. Storage note: the on-disk `localTMS` is 16 f32 row-major with the
// translation in the LAST ROW (indices 12,13,14). We work in column-vector math convention,
// so we transpose on load: `Mat4.m[r][c] = raw[c*4 + r]` (translation lands in column 3).
// World bind = parent_world * local. inv_bind = inverse(world).
// (Verified: transpose+column-3 translation composes to a standing humanoid; and the
//  independent `transforms` TRS array reproduces these matrices to 1.6e-7.)
// ---------------------------------------------------------------------------
#[derive(Clone, Copy)]
struct Mat4 { m: [[f32; 4]; 4] }

impl Mat4 {
    fn identity() -> Mat4 {
        let mut m = [[0.0f32; 4]; 4];
        for i in 0..4 { m[i][i] = 1.0; }
        Mat4 { m }
    }
    /// Load from 16 raw f32 (row-major, translation in last row) -> transpose to column-vector math.
    fn from_raw_transposed(raw: &[f32; 16]) -> Mat4 {
        let mut m = [[0.0f32; 4]; 4];
        for r in 0..4 { for c in 0..4 { m[r][c] = raw[c * 4 + r]; } }
        Mat4 { m }
    }
    fn mul(&self, o: &Mat4) -> Mat4 {
        let mut m = [[0.0f32; 4]; 4];
        for r in 0..4 { for c in 0..4 {
            let mut s = 0.0f32;
            for k in 0..4 { s += self.m[r][k] * o.m[k][c]; }
            m[r][c] = s;
        }}
        Mat4 { m }
    }
    fn flat_rowmajor(&self) -> [f32; 16] {
        let mut o = [0.0f32; 16];
        for r in 0..4 { for c in 0..4 { o[r * 4 + c] = self.m[r][c]; } }
        o
    }
    fn translation(&self) -> [f32; 3] { [self.m[0][3], self.m[1][3], self.m[2][3]] }
    /// Decompose into translation / rotation(quaternion xyzw) / scale (assumes no shear).
    fn decompose(&self) -> ([f32; 3], [f32; 4], [f32; 3]) {
        let t = self.translation();
        let col = |c: usize| [self.m[0][c], self.m[1][c], self.m[2][c]];
        let len = |v: [f32; 3]| (v[0]*v[0] + v[1]*v[1] + v[2]*v[2]).sqrt();
        let (mut sx, mut sy, mut sz) = (len(col(0)), len(col(1)), len(col(2)));
        // preserve handedness
        if self.det3() < 0.0 { sx = -sx; }
        if sx == 0.0 { sx = 1e-8; } if sy == 0.0 { sy = 1e-8; } if sz == 0.0 { sz = 1e-8; }
        let r = [
            [self.m[0][0]/sx, self.m[0][1]/sy, self.m[0][2]/sz],
            [self.m[1][0]/sx, self.m[1][1]/sy, self.m[1][2]/sz],
            [self.m[2][0]/sx, self.m[2][1]/sy, self.m[2][2]/sz],
        ];
        let q = quat_from_rot(&r);
        (t, q, [sx, sy, sz])
    }
    fn det3(&self) -> f32 {
        let m = &self.m;
        m[0][0]*(m[1][1]*m[2][2]-m[1][2]*m[2][1])
        - m[0][1]*(m[1][0]*m[2][2]-m[1][2]*m[2][0])
        + m[0][2]*(m[1][0]*m[2][1]-m[1][1]*m[2][0])
    }
    /// General 4x4 inverse (Cramer's rule).
    fn inverse(&self) -> Mat4 {
        let a: [f32; 16] = self.flat_rowmajor();
        let mut inv = [0.0f32; 16];
        inv[0]  =  a[5]*a[10]*a[15]-a[5]*a[11]*a[14]-a[9]*a[6]*a[15]+a[9]*a[7]*a[14]+a[13]*a[6]*a[11]-a[13]*a[7]*a[10];
        inv[4]  = -a[4]*a[10]*a[15]+a[4]*a[11]*a[14]+a[8]*a[6]*a[15]-a[8]*a[7]*a[14]-a[12]*a[6]*a[11]+a[12]*a[7]*a[10];
        inv[8]  =  a[4]*a[9]*a[15]-a[4]*a[11]*a[13]-a[8]*a[5]*a[15]+a[8]*a[7]*a[13]+a[12]*a[5]*a[11]-a[12]*a[7]*a[9];
        inv[12] = -a[4]*a[9]*a[14]+a[4]*a[10]*a[13]+a[8]*a[5]*a[14]-a[8]*a[6]*a[13]-a[12]*a[5]*a[10]+a[12]*a[6]*a[9];
        inv[1]  = -a[1]*a[10]*a[15]+a[1]*a[11]*a[14]+a[9]*a[2]*a[15]-a[9]*a[3]*a[14]-a[13]*a[2]*a[11]+a[13]*a[3]*a[10];
        inv[5]  =  a[0]*a[10]*a[15]-a[0]*a[11]*a[14]-a[8]*a[2]*a[15]+a[8]*a[3]*a[14]+a[12]*a[2]*a[11]-a[12]*a[3]*a[10];
        inv[9]  = -a[0]*a[9]*a[15]+a[0]*a[11]*a[13]+a[8]*a[1]*a[15]-a[8]*a[3]*a[13]-a[12]*a[1]*a[11]+a[12]*a[3]*a[9];
        inv[13] =  a[0]*a[9]*a[14]-a[0]*a[10]*a[13]-a[8]*a[1]*a[14]+a[8]*a[2]*a[13]+a[12]*a[1]*a[10]-a[12]*a[2]*a[9];
        inv[2]  =  a[1]*a[6]*a[15]-a[1]*a[7]*a[14]-a[5]*a[2]*a[15]+a[5]*a[3]*a[14]+a[13]*a[2]*a[7]-a[13]*a[3]*a[6];
        inv[6]  = -a[0]*a[6]*a[15]+a[0]*a[7]*a[14]+a[4]*a[2]*a[15]-a[4]*a[3]*a[14]-a[12]*a[2]*a[7]+a[12]*a[3]*a[6];
        inv[10] =  a[0]*a[5]*a[15]-a[0]*a[7]*a[13]-a[4]*a[1]*a[15]+a[4]*a[3]*a[13]+a[12]*a[1]*a[7]-a[12]*a[3]*a[5];
        inv[14] = -a[0]*a[5]*a[14]+a[0]*a[6]*a[13]+a[4]*a[1]*a[14]-a[4]*a[2]*a[13]-a[12]*a[1]*a[6]+a[12]*a[2]*a[5];
        inv[3]  = -a[1]*a[6]*a[11]+a[1]*a[7]*a[10]+a[5]*a[2]*a[11]-a[5]*a[3]*a[10]-a[9]*a[2]*a[7]+a[9]*a[3]*a[6];
        inv[7]  =  a[0]*a[6]*a[11]-a[0]*a[7]*a[10]-a[4]*a[2]*a[11]+a[4]*a[3]*a[10]+a[8]*a[2]*a[7]-a[8]*a[3]*a[6];
        inv[11] = -a[0]*a[5]*a[11]+a[0]*a[7]*a[9]+a[4]*a[1]*a[11]-a[4]*a[3]*a[9]-a[8]*a[1]*a[7]+a[8]*a[3]*a[5];
        inv[15] =  a[0]*a[5]*a[10]-a[0]*a[6]*a[9]-a[4]*a[1]*a[10]+a[4]*a[2]*a[9]+a[8]*a[1]*a[6]-a[8]*a[2]*a[5];
        let mut det = a[0]*inv[0]+a[1]*inv[4]+a[2]*inv[8]+a[3]*inv[12];
        if det == 0.0 { det = 1e-12; }
        det = 1.0 / det;
        let mut m = [[0.0f32; 4]; 4];
        for r in 0..4 { for c in 0..4 { m[r][c] = inv[r * 4 + c] * det; } }
        Mat4 { m }
    }
}

fn quat_from_rot(r: &[[f32; 3]; 3]) -> [f32; 4] {
    let trace = r[0][0] + r[1][1] + r[2][2];
    let (x, y, z, w);
    if trace > 0.0 {
        let s = 0.5 / (trace + 1.0).sqrt();
        w = 0.25 / s; x = (r[2][1]-r[1][2])*s; y = (r[0][2]-r[2][0])*s; z = (r[1][0]-r[0][1])*s;
    } else if r[0][0] > r[1][1] && r[0][0] > r[2][2] {
        let s = 2.0 * (1.0 + r[0][0] - r[1][1] - r[2][2]).sqrt();
        w = (r[2][1]-r[1][2])/s; x = 0.25*s; y = (r[0][1]+r[1][0])/s; z = (r[0][2]+r[2][0])/s;
    } else if r[1][1] > r[2][2] {
        let s = 2.0 * (1.0 + r[1][1] - r[0][0] - r[2][2]).sqrt();
        w = (r[0][2]-r[2][0])/s; x = (r[0][1]+r[1][0])/s; y = 0.25*s; z = (r[1][2]+r[2][1])/s;
    } else {
        let s = 2.0 * (1.0 + r[2][2] - r[0][0] - r[1][1]).sqrt();
        w = (r[1][0]-r[0][1])/s; x = (r[0][2]+r[2][0])/s; y = (r[1][2]+r[2][1])/s; z = 0.25*s;
    }
    let n = (x*x + y*y + z*z + w*w).sqrt().max(1e-12);
    [x/n, y/n, z/n, w/n]
}

// ---------------------------------------------------------------------------
// Parsed structures
// ---------------------------------------------------------------------------
struct Bone {
    index: usize,
    name_hash: u32, // boneName0 = pandemic_hash(bone name); the key the anim system uses
    parent: i32,    // -1 for root
    local_t: [f32; 3],
    local_r: [f32; 4], // xyzw quaternion
    local_s: [f32; 3],
    world: Mat4,       // bind: parent_world * local
    inv_bind: Mat4,    // inverse(world)
    /// The asset's OWN inverse-bind for this bone, when the mesh skins it (from BoneRemaps).
    /// Ground truth: `stored_ibm.inverse()` is the bone's real bind world. `None` for a bone with
    /// no skin on it, which is also a bone whose position cannot affect the render.
    stored_ibm: Option<Mat4>,
}

struct MeshHit {
    name: String,
    file_off: usize,   // offset of "AHSM" in the megapack
    unc0: u32,
    comp0: u32,
    num_bones0: u32,
    body: Vec<u8>,     // decompressed MESH body
}

/// Parse one MSHA at `off` if valid; returns (name, comp0, unc0) header fields.
fn parse_msha_header(buf: &[u8], off: usize) -> Option<(String, u32, u32, u32, u32)> {
    if off + 276 > buf.len() { return None; }
    // MSHA: id(4) uncompressedSize0(4) uncompressedSize1(4) compressedSize0(4) compressedSize1(4) name[0x100]
    let u_unc0 = u32(buf, off + 4);
    let u_unc1 = u32(buf, off + 8);
    let u_c0 = u32(buf, off + 12);
    let u_c1 = u32(buf, off + 16);
    let name_bytes = &buf[off + 20..off + 276];
    let end = name_bytes.iter().position(|&b| b == 0).unwrap_or(0);
    if end == 0 { return None; }
    let nm = &name_bytes[..end];
    if !nm.iter().all(|&b| (0x20..0x7f).contains(&b)) { return None; }
    Some((String::from_utf8_lossy(nm).into_owned(), u_c0, u_unc0, u_c1, u_unc1))
}

fn zlib_inflate(data: &[u8], expected: usize) -> Option<Vec<u8>> {
    let mut d = flate2::read::ZlibDecoder::new(data);
    let mut out = Vec::with_capacity(expected);
    match d.read_to_end(&mut out) {
        Ok(_) => Some(out),
        Err(_) => None,
    }
}

/// numBones0 sits at a fixed offset (204) in the decompressed MESH body — see MESH::Read.
fn mesh_num_bones0(body: &[u8]) -> Option<u32> {
    if body.len() < 212 { return None; }
    Some(u32(body, 204))
}

/// Scan the whole megapack for MSHA ("AHSM") headers and decode any whose name matches
/// `name_filter` (substring) into a full MeshHit (with decompressed body). If `list_only`,
/// records every skinned candidate lightly (no body retained beyond header parse).
fn scan_meshes(buf: &[u8], name_filter: &str) -> Vec<MeshHit> {
    let magic = b"AHSM";
    let mut hits = Vec::new();
    let mut i = 0usize;
    while i + 276 <= buf.len() {
        if &buf[i..i + 4] == magic {
            if let Some((name, c0, unc0, _c1, _unc1)) = parse_msha_header(buf, i) {
                if c0 > 0 && unc0 > 0 && c0 as usize <= buf.len()
                    && (name_filter.is_empty() || name.contains(name_filter))
                {
                    let start = i + 276;
                    if start + c0 as usize <= buf.len() {
                        if let Some(body) = zlib_inflate(&buf[start..start + c0 as usize], unc0 as usize) {
                            if body.len() == unc0 as usize {
                                if let Some(nb0) = mesh_num_bones0(&body) {
                                    hits.push(MeshHit {
                                        name, file_off: i, unc0, comp0: c0,
                                        num_bones0: nb0, body,
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }
        i += 1; // MSHA headers are byte-packed within the SBLA pack (not aligned)
    }
    hits
}

/// Parse the flat MESH skeleton from a decompressed body. Returns the bone list.
fn parse_skeleton(body: &[u8]) -> Result<Vec<Bone>, String> {
    // ---- MESH header (244 bytes) ----
    // 19*u32 null | BBOX_(Vector min 12 + Vector4 max 16 = 28) | 11*u32 null | name u32 |
    // 8*u32 null | unk0 u32 | 4*u32 null | numBones0 u32@204 | numBoneRemaps u32@208 |
    // null u32 | numStreams u16@216 numPrimitives u16@218 | 3*u32 null | numDrawCalls u32@232 | 2*u32 null
    let num_bones0 = u32(body, 204);
    if num_bones0 <= 1 { return Err(format!("mesh is not skinned (numBones0={num_bones0})")); }

    // ---- MESHSkeleton header @244 (11 u32) ----
    let mut p = 244usize;
    let num_unk_bones0 = u32(body, p); // count of trailing null8 pad after boneIds
    // p+4,p+8 = null
    let num_bones = u32(body, p + 12) as usize; // numBones2  == numBones0
    let num_unk_bones1 = u32(body, p + 16);     // if !=0, one trailing null16
    let num_bones3 = u32(body, p + 20) as usize;
    // p+24 = null
    let num_bones4 = u32(body, p + 28) as usize;
    // p+32,36,40 = null
    if num_bones != num_bones3 || num_bones != num_bones4 {
        return Err(format!("bone count mismatch: {num_bones}/{num_bones3}/{num_bones4}"));
    }
    p += 44;

    // ---- boneIds: numBones u8 ----
    let _bone_ids = &body[p..p + num_bones]; p += num_bones;
    // ---- numUnkBones0 * null8 ----
    p += num_unk_bones0 as usize;

    // ---- localTMS: numBones * Matrix44 (64 bytes each) ----
    let mut local_mats = Vec::with_capacity(num_bones);
    for _ in 0..num_bones {
        let mut raw = [0.0f32; 16];
        for k in 0..16 { raw[k] = f32a(body, p + k * 4); }
        local_mats.push(Mat4::from_raw_transposed(&raw));
        p += 64;
    }

    // ---- bones: numBones * Bone (64 bytes) ----
    //   name0 u32@0 | 4*u32 null | name1 u32@20 | null u32@24 | unk0 u32@28 | bbox(8 f32)@32
    let mut name_hashes = Vec::with_capacity(num_bones);
    for _ in 0..num_bones {
        name_hashes.push(u32(body, p)); // boneName0
        p += 64;
    }

    // ---- transforms: numBones * RTSValue (translation Vec4 + rotation Vec4 + scale Vec4 = 48) ----
    let mut trs = Vec::with_capacity(num_bones);
    for _ in 0..num_bones {
        let t = [f32a(body, p),      f32a(body, p + 4),  f32a(body, p + 8)];   // + w @12 (unused)
        let r = [f32a(body, p + 16), f32a(body, p + 20), f32a(body, p + 24), f32a(body, p + 28)]; // xyzw
        let s = [f32a(body, p + 32), f32a(body, p + 36), f32a(body, p + 40)];  // + w @44 (unused)
        trs.push((t, r, s));
        p += 48;
    }

    // ---- parentIds: numBones * int16 ----
    let mut parents = Vec::with_capacity(num_bones);
    for _ in 0..num_bones {
        parents.push(i16(body, p) as i32);
        p += 2;
    }
    // ---- numBones * null32, then optional null16 ----
    p += 4 * num_bones;
    if num_unk_bones1 != 0 { p += 2; }

    // ---- BoneRemaps: the asset's OWN inverse-bind per SKINNED bone ----
    //
    // These used to be skipped as "not needed", which was the single most expensive assumption in
    // this tool. `world[i] = world[parent] · localTMS[i]` silently disagrees with them — the on-disk
    // local translations for a character's face bones are all (0,0,0), so the chain collapses that
    // whole subtree onto one point. The chain cannot be repaired from the data it reads; the ibm is
    // the only place the truth exists. Read it.
    //
    // Layout (mirrors sab_mesh): u32 unk0(==numBoneRemaps) + null32, then [ ibm(64) + boneId(4) ].
    let num_bone_remaps = u32(body, 208) as usize;
    let mut stored_ibm: Vec<(usize, Mat4)> = Vec::new();
    if num_bone_remaps > 0 {
        let guard = u32(body, p) as usize;
        if guard != num_bone_remaps {
            return Err(format!("boneRemap count guard mismatch: {guard} != {num_bone_remaps}"));
        }
        p += 8;
        for _ in 0..num_bone_remaps {
            if p + 68 > body.len() { return Err("truncated BoneRemap array".into()); }
            let mut raw = [0.0f32; 16];
            for k in 0..16 { raw[k] = f32a(body, p + k * 4); }
            let bone_id = u32(body, p + 64) as usize;
            if bone_id < num_bones {
                stored_ibm.push((bone_id, Mat4::from_raw_transposed(&raw)));
            }
            p += 68;
        }
    }
    let _ = p; // the rest is Streams/Primitives/DrawCalls (not needed here)

    // ---- compose world (bind) matrices: world[i] = world[parent] * local[i] ----
    let mut world = vec![Mat4::identity(); num_bones];
    let mut done = vec![false; num_bones];
    let mut progress = true;
    while progress {
        progress = false;
        for i in 0..num_bones {
            if done[i] { continue; }
            let pp = parents[i];
            if pp < 0 {
                world[i] = local_mats[i]; done[i] = true; progress = true;
            } else if done[pp as usize] {
                world[i] = world[pp as usize].mul(&local_mats[i]); done[i] = true; progress = true;
            }
        }
    }
    if done.iter().any(|&d| !d) {
        return Err("bone hierarchy has a cycle or dangling parent".into());
    }

    let ibm_of: std::collections::HashMap<usize, Mat4> = stored_ibm.into_iter().collect();

    let mut bones = Vec::with_capacity(num_bones);
    for i in 0..num_bones {
        let (t, r, s) = trs[i];
        bones.push(Bone {
            index: i,
            name_hash: name_hashes[i],
            parent: parents[i],
            local_t: t, local_r: r, local_s: s,
            world: world[i],
            inv_bind: world[i].inverse(),
            stored_ibm: ibm_of.get(&i).copied(),
        });
    }
    Ok(bones)
}


// ---------------------------------------------------------------------------
// Minimal built-in bone-name dictionary (pandemic_hash -> name) for a readable report.
// These are the canonical Saboteur biped bone names; full resolution needs the game's
// string dictionary, but this covers the major joints for validation.
// ---------------------------------------------------------------------------
fn known_bone_names() -> Vec<&'static str> {
    let mut v = vec![
        "GlobalSRT", "Bone_Attach_Root", "Bone_Root",
        "Bone_Hips", "Bone_Spine", "Bone_Spine1", "Bone_Spine2", "Bone_Chest",
        "Bone_Neck", "Bone_Neck1", "Bone_Head",
        "Bone_LThigh", "Bone_LShin", "Bone_LFoot", "Bone_LToe",
        "Bone_RThigh", "Bone_RShin", "Bone_RFoot", "Bone_RToe",
        "Bone_LClav", "Bone_LBicep", "Bone_LForearm", "Bone_LHand",
        "Bone_RClav", "Bone_RBicep", "Bone_RForearm", "Bone_RHand",
        "bone_attach_lhand", "bone_attach_rhand",
        "bone_cheek_left", "bone_cheek_right", "bone_jaw", "bone_tongue",
        "bone_eye_left", "bone_eye_right", "bone_eyelid_left", "bone_eyelid_right",
    ];
    // finger/thumb variants
    for side in ["L", "R"] {
        for f in ["Finger", "Thumb", "Index", "Middle", "Ring", "Pinky"] {
            for n in 0..4 {
                // leak a static string via Box (fine for a short-lived CLI)
                let s: &'static str = Box::leak(format!("Bone_{side}{f}{n}").into_boxed_str());
                v.push(s);
            }
        }
    }
    v
}

fn build_name_map() -> std::collections::HashMap<u32, &'static str> {
    let mut m = std::collections::HashMap::new();
    for n in known_bone_names() { m.entry(pandemic_hash(n)).or_insert(n); }
    m
}

// ---------------------------------------------------------------------------
// JSON emit (manual; no serde)
// ---------------------------------------------------------------------------
fn f(x: f32) -> String {
    if x == 0.0 { "0".into() } else { format!("{:.7}", x) }
}
fn arr3(a: [f32; 3]) -> String { format!("[{},{},{}]", f(a[0]), f(a[1]), f(a[2])) }
fn arr4(a: [f32; 4]) -> String { format!("[{},{},{},{}]", f(a[0]), f(a[1]), f(a[2]), f(a[3])) }
fn arr16(a: [f32; 16]) -> String {
    let parts: Vec<String> = a.iter().map(|&x| f(x)).collect();
    format!("[{}]", parts.join(","))
}

fn emit_json(
    source: &str, hit: &MeshHit, entry: Option<(u32, u32, u64, u32)>,
    bones: &[Bone], root_count: usize,
) -> String {
    let names = build_name_map();
    let mut s = String::new();
    s.push_str("{\n");
    s.push_str(&format!("  \"source\": {:?},\n", source));
    s.push_str(&format!("  \"mesh_name\": {:?},\n", hit.name));
    s.push_str(&format!("  \"msha_file_offset\": {},\n", hit.file_off));
    s.push_str(&format!("  \"mesh_body_compressed\": {}, \"mesh_body_uncompressed\": {},\n", hit.comp0, hit.unc0));
    if let Some((crc, index, off, size)) = entry {
        s.push_str(&format!(
            "  \"megapack_entry\": {{ \"crc\": \"0x{:08X}\", \"index\": {}, \"index_hash\": \"0x{:08X}\", \"offset\": {}, \"size\": {} }},\n",
            crc, index, index, off, size));
    }
    s.push_str(&format!("  \"num_bones0\": {},\n", hit.num_bones0));
    s.push_str(&format!("  \"root_count\": {},\n", root_count));
    s.push_str("  \"bone_naming\": \"name_hash = boneName0 = pandemic_hash(name); matches Animations.pack ANIM bone-track hashes\",\n");
    s.push_str("  \"matrix_convention\": \"world/inv_bind are 4x4 row-major, column-vector math (translation in column 3 = m[3],m[7],m[11]); world[i]=world[parent]*local[i]\",\n");
    s.push_str("  \"bones\": [\n");
    for (bi, b) in bones.iter().enumerate() {
        let resolved = names.get(&b.name_hash).copied();
        s.push_str("    {");
        s.push_str(&format!(" \"index\": {},", b.index));
        s.push_str(&format!(" \"name_hash\": {},", b.name_hash));
        s.push_str(&format!(" \"name_hash_hex\": \"0x{:08X}\",", b.name_hash));
        if let Some(n) = resolved { s.push_str(&format!(" \"name\": {:?},", n)); }
        s.push_str(&format!(" \"parent\": {},", b.parent));
        s.push_str(&format!(" \"local\": {{ \"t\": {}, \"r\": {}, \"s\": {} }},",
            arr3(b.local_t), arr4(b.local_r), arr3(b.local_s)));
        let (wt, wr, ws) = b.world.decompose();
        s.push_str(&format!(" \"world\": {{ \"t\": {}, \"r\": {}, \"s\": {}, \"m\": {} }},",
            arr3(wt), arr4(wr), arr3(ws), arr16(b.world.flat_rowmajor())));
        s.push_str(&format!(" \"inv_bind\": {{ \"m\": {} }}", arr16(b.inv_bind.flat_rowmajor())));
        s.push_str(" }");
        s.push_str(if bi + 1 < bones.len() { ",\n" } else { "\n" });
    }
    s.push_str("  ]\n}\n");
    s
}

// ---------------------------------------------------------------------------
// Megapack index (for reporting which container entry holds the chosen mesh)
// ---------------------------------------------------------------------------
struct Entry { crc: u32, index: u32, size: u32, offset: u64 }

fn read_megapack_index(buf: &[u8]) -> Result<Vec<Entry>, String> {
    if buf.len() < 8 || &buf[0..4] != b"00PM" {
        return Err(format!("not a megapack (magic {:02X?})", &buf[0..4.min(buf.len())]));
    }
    let count = u32(buf, 4) as usize;
    let mut v = Vec::with_capacity(count);
    let mut p = 8usize;
    for _ in 0..count {
        if p + 20 > buf.len() { break; }
        v.push(Entry { crc: u32(buf, p), index: u32(buf, p + 4), size: u32(buf, p + 8), offset: u64(buf, p + 12) });
        p += 20;
    }
    Ok(v)
}

fn containing_entry(entries: &[Entry], off: usize) -> Option<(u32, u32, u64, u32)> {
    for e in entries {
        let start = e.offset as usize;
        let end = start + e.size as usize;
        if (start..end).contains(&off) {
            return Some((e.crc, e.index, e.offset, e.size));
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Probes
// ---------------------------------------------------------------------------

fn label(names: &std::collections::HashMap<u32, &'static str>, h: u32) -> String {
    names.get(&h).map(|s| (*s).to_string()).unwrap_or_else(|| format!("{h:08X}"))
}

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// Do a character's parts agree on what each bone IS and where it SITS?
///
/// Each part carries its own skeleton, but the pipeline extracts ONE and poses the merged mesh
/// against it — so if index N names a different bone in different parts, the merge is unsound
/// however good the bind pose is. Separately, every skinned bone ships its own inverse-bind; this
/// checks that ground truth against itself (across parts) and against the derived chain.
fn probe_parts(hits: &[MeshHit]) {
    use std::collections::{BTreeMap, HashMap, HashSet};
    let names = build_name_map();

    // The megapack stores each mesh more than once (per sub-pack), so the same part name comes back
    // repeatedly. Keep one entry per NAME — the richest — or the comparison below counts the same
    // asset against itself.
    //
    // Scope matters just as much: "CH_AL_SeanDevlin" also matches his DISGUISES
    // (…_Nazi_General_SS_UB, …_Nazi_KM_UB, …_Hair_FLAT_GR_2). Those are different meshes with
    // legitimately different skeletons, and comparing bone indices across them is meaningless.
    // Pass a filter that names one outfit, e.g. "CH_AL_SeanDevlin_01_".
    let mut best: BTreeMap<&str, &MeshHit> = BTreeMap::new();
    for h in hits {
        let e = best.entry(h.name.as_str()).or_insert(h);
        if h.num_bones0 > e.num_bones0 {
            *e = h;
        }
    }
    let mut parsed: Vec<(String, Vec<Bone>)> = Vec::new();
    for h in best.values() {
        match parse_skeleton(&h.body) {
            Ok(b) => parsed.push((h.name.clone(), b)),
            Err(e) => eprintln!("  {}: parse failed ({e})", h.name),
        }
    }
    if parsed.is_empty() {
        eprintln!("nothing parsed");
        return;
    }
    eprintln!("[*] {} distinct part name(s) after dedupe\n", parsed.len());

    println!("=== PART SKELETONS ===");
    for (n, b) in &parsed {
        let skinned = b.iter().filter(|x| x.stored_ibm.is_some()).count();
        println!("  {n:34} {:4} bones, {skinned:3} skinned (ship a stored inverse-bind)", b.len());
    }

    // 1. Does bone INDEX N mean the same bone in every part that has it?
    let mut idx_hashes: BTreeMap<usize, HashSet<u32>> = BTreeMap::new();
    for (_, b) in &parsed {
        for bone in b {
            idx_hashes.entry(bone.index).or_default().insert(bone.name_hash);
        }
    }
    let conflicts: Vec<(usize, Vec<u32>)> = idx_hashes
        .iter()
        .filter(|(_, hs)| hs.len() > 1)
        .map(|(i, hs)| (*i, hs.iter().copied().collect()))
        .collect();
    println!("\n=== INDEX AGREEMENT  (is index N the same bone everywhere?) ===");
    println!("  distinct bone indices seen : {}", idx_hashes.len());
    println!("  indices naming DIFFERENT bones in different parts: {}", conflicts.len());
    for (i, hs) in conflicts.iter().take(12) {
        let l: Vec<String> = hs.iter().map(|h| label(&names, *h)).collect();
        println!("    idx {i:3} -> {}", l.join("  |  "));
    }

    // 2. Where two parts skin the same bone, do their stored ibms agree on where it is?
    let mut pos: HashMap<u32, Vec<[f32; 3]>> = HashMap::new();
    for (_, b) in &parsed {
        for bone in b {
            if let Some(ibm) = bone.stored_ibm {
                pos.entry(bone.name_hash).or_default().push(ibm.inverse().translation());
            }
        }
    }
    let (mut shared, mut disagree) = (0usize, 0usize);
    for (h, v) in &pos {
        if v.len() < 2 {
            continue;
        }
        shared += 1;
        let mut worst = 0f32;
        for a in v {
            for b in v {
                worst = worst.max(dist(*a, *b));
            }
        }
        if worst > 0.01 {
            disagree += 1;
            if disagree <= 5 {
                println!("    DISAGREE {} spread={worst:.3} m", label(&names, *h));
            }
        }
    }
    println!("\n=== STORED INVERSE-BIND  (is the ground truth self-consistent?) ===");
    println!("  bones skinned by >1 part   : {shared}");
    println!("  ...where the parts disagree: {disagree}");
    println!("  UNION of skinned bones     : {} (by name-hash)", pos.len());

    // Can one canonical, hash-keyed skeleton be built for the whole character?
    //
    // Three things have to hold: some part must contain every bone (or we union them); the parts
    // must agree on each bone's PARENT (by hash, since indices don't survive the trip); and the
    // union must cover every bone any part skins.
    println!("\n=== CANONICAL SKELETON FEASIBILITY ===");
    let all: HashSet<u32> = parsed.iter().flat_map(|(_, b)| b.iter().map(|x| x.name_hash)).collect();
    println!("  UNION of ALL bones         : {} (by name-hash)", all.len());
    let mut superset: Option<&str> = None;
    for (n, b) in &parsed {
        let set: HashSet<u32> = b.iter().map(|x| x.name_hash).collect();
        if set.len() == all.len() && all.iter().all(|h| set.contains(h)) {
            superset = Some(n);
            break;
        }
    }
    match superset {
        Some(n) => println!("  a part containing EVERY bone: {n}  (usable as the canonical order)"),
        None => println!("  a part containing EVERY bone: NONE — the canonical set must be a union"),
    }
    // Parent agreement, keyed by hash on both sides.
    let mut parent_of: HashMap<u32, HashSet<Option<u32>>> = HashMap::new();
    for (_, b) in &parsed {
        for bone in b {
            let ph = if bone.parent >= 0 { b.get(bone.parent as usize).map(|p| p.name_hash) } else { None };
            parent_of.entry(bone.name_hash).or_default().insert(ph);
        }
    }
    let pconf: Vec<u32> = parent_of.iter().filter(|(_, v)| v.len() > 1).map(|(h, _)| *h).collect();
    println!("  bones whose PARENT differs between parts (by hash): {}", pconf.len());
    for h in pconf.iter().take(8) {
        let ps: Vec<String> = parent_of[h]
            .iter()
            .map(|p| p.map(|x| label(&names, x)).unwrap_or_else(|| "ROOT".into()))
            .collect();
        println!("    {} -> {}", label(&names, *h), ps.join("  |  "));
    }
    let unskinned = all.len() - pos.len();
    println!("  bones with NO stored ibm anywhere: {unskinned} (no skin on them => cannot affect the render)");

    // 3. Chain vs truth, and whether a corrected bind is reconstructable. A skinned bone needs
    //    truth for its WHOLE ancestor chain or its derived local is relative to a wrong parent —
    //    harmless at bind, wrong the moment it animates.
    println!("\n=== CHAIN vs TRUTH (per part) ===");
    for (n, b) in &parsed {
        let have: HashSet<usize> =
            b.iter().filter(|x| x.stored_ibm.is_some()).map(|x| x.index).collect();
        if have.is_empty() {
            continue;
        }
        let (mut wrong, mut worst, mut worst_i) = (0usize, 0f32, 0usize);
        for bone in b.iter().filter(|x| x.stored_ibm.is_some()) {
            let d = dist(bone.stored_ibm.unwrap().inverse().translation(), bone.world.translation());
            if d > 0.01 {
                wrong += 1;
            }
            if d > worst {
                worst = d;
                worst_i = bone.index;
            }
        }
        let mut full = 0usize;
        for bone in b.iter().filter(|x| x.stored_ibm.is_some()) {
            let mut p = bone.parent;
            let mut ok = true;
            while p >= 0 {
                if !have.contains(&(p as usize)) {
                    ok = false;
                    break;
                }
                p = b[p as usize].parent;
            }
            if ok {
                full += 1
            }
        }
        println!("  {n:34}");
        println!(
            "      chain disagrees with truth : {wrong:3}/{:3} skinned  (worst {worst:.3} m at {})",
            have.len(),
            label(&names, b[worst_i].name_hash)
        );
        println!("      truth covers whole ancestry: {full:3}/{:3}", have.len());
    }
}

/// Per-bone detail for the richest matching part: what the chain says, what the asset says, and
/// the local the truth requires vs what is actually on disk.
fn probe_bones(hit: &MeshHit) {
    let names = build_name_map();
    let b = match parse_skeleton(&hit.body) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("parse failed: {e}");
            return;
        }
    };
    println!("=== {} — {} bones ===", hit.name, b.len());
    println!("(only bones whose chain disagrees with the asset's own inverse-bind)\n");
    for bone in b.iter().filter(|x| x.stored_ibm.is_some()) {
        let truth = bone.stored_ibm.unwrap().inverse();
        let (t, c) = (truth.translation(), bone.world.translation());
        let d = dist(t, c);
        if d <= 0.01 {
            continue;
        }
        println!(
            "{:>3} {:22} parent={:<4} TRUE=({:7.3},{:7.3},{:7.3})  chain=({:7.3},{:7.3},{:7.3})  off {d:.3} m",
            bone.index, label(&names, bone.name_hash), bone.parent,
            t[0], t[1], t[2], c[0], c[1], c[2]
        );
        // With the parent's truth known too, the required local is fully determined — compare it
        // to the local actually on disk.
        if bone.parent >= 0 {
            if let Some(p) = b.get(bone.parent as usize) {
                if let Some(pibm) = p.stored_ibm {
                    let need = pibm.mul(&truth); // inverse(parent_world) * world  ==  ibm_parent * world
                    let nt = need.translation();
                    println!(
                        "     local REQUIRED=({:7.3},{:7.3},{:7.3})   on-disk=({:7.3},{:7.3},{:7.3})   d={:.3}",
                        nt[0], nt[1], nt[2],
                        bone.local_t[0], bone.local_t[1], bone.local_t[2],
                        dist(nt, bone.local_t)
                    );
                }
            }
        }
    }
}

/// Does each animation track really drive the bone the trackmap claims?
///
/// `anim_bone_map.json` gives every clip a `bone_ids` array (indices into the 191-bone rig) AND a
/// parallel `bone_hashes` array (the pandemic_hash of the bone each track drives). Those are two
/// independent statements of the same fact, so they can be checked against each other — but only
/// with a skeleton that knows every bone's hash.
///
/// This matters because the obvious check is blind exactly where it counts: the `.skel` only spells
/// a hash into the NAME of bones the dictionary can't name (`bone_74_0x9B1CAB9F`), so the named
/// core — GlobalSRT, Bone_Hips, Bone_Spine1, Bone_Chest, Bone_Neck, the whole spine — carries no
/// hash to compare and silently passes. The skeleton JSON emits `name_hash` for EVERY bone, so
/// pass that.
fn probe_anim(anim_json: &str, skel_json: &str) {
    let names = build_name_map();
    // skeleton JSON: bone index -> name_hash, in file order.
    let skel: Vec<u32> = {
        let b = skel_json.as_bytes();
        let key = b"\"name_hash\"";
        let mut v = Vec::new();
        let mut i = 0usize;
        while let Some(rel) = b[i..].windows(key.len()).position(|w| w == key) {
            let mut p = i + rel + key.len();
            while p < b.len() && !(b[p] as char).is_ascii_digit() { p += 1; }
            let mut val: u64 = 0;
            let mut any = false;
            while p < b.len() && (b[p] as char).is_ascii_digit() {
                val = val * 10 + (b[p] - b'0') as u64;
                p += 1;
                any = true;
            }
            i = p;
            if any { v.push(val as u32); }
        }
        v
    };
    println!("skeleton: {} bones (with a name_hash each)", skel.len());
    if skel.is_empty() {
        eprintln!("no \"name_hash\" in the skeleton json — pass sab_skeleton's .json, not the .skel");
        return;
    }

    // Walk every clip's bone_ids / bone_hashes pair.
    let b = anim_json.as_bytes();
    let grab = |from: usize, key: &[u8]| -> Option<(Vec<u64>, usize)> {
        let rel = b[from..].windows(key.len()).position(|w| w == key)?;
        let mut p = from + rel + key.len();
        while p < b.len() && b[p] != b'[' { p += 1; }
        p += 1;
        let mut out = Vec::new();
        let (mut cur, mut any) = (0u64, false);
        while p < b.len() && b[p] != b']' {
            if (b[p] as char).is_ascii_digit() {
                cur = cur * 10 + (b[p] - b'0') as u64;
                any = true;
            } else if any {
                out.push(cur);
                cur = 0;
                any = false;
            }
            p += 1;
        }
        if any { out.push(cur); }
        Some((out, p))
    };

    let (mut clips, mut ok, mut bad, mut unchecked) = (0usize, 0usize, 0usize, 0usize);
    let mut worst: Vec<(usize, u32, u32)> = Vec::new(); // (track, claimed bone, wanted hash)
    let mut i = 0usize;
    while let Some((ids, p1)) = grab(i, b"\"bone_ids\"") {
        let Some((hashes, p2)) = grab(p1, b"\"bone_hashes\"") else { break };
        i = p2;
        clips += 1;
        for (k, (id, want)) in ids.iter().zip(&hashes).enumerate() {
            if *id == 0xFFFF_FFFF { continue; }
            match skel.get(*id as usize) {
                None => unchecked += 1,
                Some(got) if *got as u64 == *want => ok += 1,
                Some(got) => {
                    bad += 1;
                    if worst.len() < 10 { worst.push((k, *id as u32, *got)); }
                }
            }
        }
    }
    println!("clips checked            : {clips}");
    println!("track->bone entries AGREE: {ok}");
    println!("track->bone entries WRONG: {bad}");
    println!("entries with no such bone: {unchecked}");
    for (k, id, got) in worst.iter().take(10) {
        println!("    track {k:2} claims bone {id:3} ({}) — but the clip's hash says otherwise",
            label(&names, *got));
    }
}

/// Recover bone NAMES by brute-forcing `pandemic_hash` against the rig's unknown hashes.
///
/// Every bone is named; the mesh just stores `pandemic_hash(name)` rather than the string, and we
/// have no string table — so 136 of Sean's 191 bones read as `bone_74_0x9B1CAB9F`, which makes a
/// diagnosis unreadable (is bone 11 a spine bone or a coat flap?).
///
/// The hash is FNV-1a over `c | 0x20`, so it is CASE-INSENSITIVE and one multiply per byte: running
/// it forwards over a few hundred thousand plausible names costs nothing. The hierarchy tells us
/// what to guess — a child of `Bone_RShin` is a foot, a bone between `Bone_Spine1` and `Bone_Chest`
/// is a spine joint — so this is a dictionary attack with very good priors.
///
/// A hit is a real find: a 32-bit hash collision on a name that also fits the bone's position in
/// the tree is not a coincidence worth worrying about.
fn probe_names(skel_json: &str) {
    use std::collections::HashMap;

    // hash -> index, from the skeleton report.
    let mut want: HashMap<u32, usize> = HashMap::new();
    {
        let b = skel_json.as_bytes();
        let key = b"\"name_hash\"";
        let (mut i, mut idx) = (0usize, 0usize);
        while let Some(rel) = b[i..].windows(key.len()).position(|w| w == key) {
            let mut p = i + rel + key.len();
            while p < b.len() && !(b[p] as char).is_ascii_digit() { p += 1; }
            let (mut v, mut any) = (0u64, false);
            while p < b.len() && (b[p] as char).is_ascii_digit() {
                v = v * 10 + (b[p] - b'0') as u64;
                p += 1;
                any = true;
            }
            i = p;
            if any { want.insert(v as u32, idx); idx += 1; }
        }
    }
    println!("rig: {} bones", want.len());

    // ---- candidate space, built from what a biped rig actually contains ----
    let joints = [
        "root", "hips", "pelvis", "spine", "chest", "neck", "head", "jaw", "tongue", "teeth",
        "thigh", "shin", "calf", "knee", "foot", "ankle", "toe", "toebase", "ball", "heel",
        "clav", "clavicle", "shoulder", "bicep", "upperarm", "forearm", "elbow", "hand", "wrist",
        "finger", "thumb", "index", "middle", "ring", "pinky", "palm",
        "eye", "eyelid", "brow", "eyebrow", "cheek", "lip", "mouth", "nose", "nostril", "ear",
        "chin", "temple", "forehead", "skull", "face",
        "coat", "cloth", "skirt", "flap", "tail", "hair", "hat", "cap", "collar", "belt",
        "strap", "pouch", "holster", "prop", "attach", "weapon", "cig", "cigarette",
        "breast", "gut", "belly", "twist", "roll", "muscle", "helper",
    ];
    let sides = ["", "l", "r", "left", "right", "_l", "_r", "_left", "_right", "l_", "r_"];
    let prefixes = ["bone_", "bone", "", "b_", "bip01_", "bip01 "];
    let seps = ["", "_"];

    let mut found: Vec<(usize, String)> = Vec::new();
    let mut hit = |name: &str, found: &mut Vec<(usize, String)>| {
        if let Some(&i) = want.get(&pandemic_hash(name)) {
            found.push((i, name.to_string()));
        }
    };
    for p in prefixes {
        for j in joints {
            for s in sides {
                for sep in seps {
                    // side before and after the joint, with and without a separator
                    hit(&format!("{p}{s}{sep}{j}"), &mut found);
                    hit(&format!("{p}{j}{sep}{s}"), &mut found);
                    for n in 0..=6 {
                        hit(&format!("{p}{s}{sep}{j}{n}"), &mut found);
                        hit(&format!("{p}{j}{sep}{s}{n}"), &mut found);
                        hit(&format!("{p}{s}{sep}{j}{sep}{n}"), &mut found);
                        hit(&format!("{p}{s}{sep}{j}0{n}"), &mut found);
                        hit(&format!("{p}{j}{n}{sep}{s}"), &mut found);
                    }
                }
            }
        }
    }
    found.sort();
    found.dedup();
    println!("recovered {} name(s) by brute force:\n", found.len());
    for (i, n) in &found {
        println!("  bone {i:3}  {n}");
    }
}

fn usage() -> ! {
    eprintln!("sab_probe — read-only questions about The Saboteur's assets (writes nothing)");
    eprintln!();
    eprintln!("usage:");
    eprintln!("  sab_probe parts <megapack> <name_substr>   cross-part bone identity; stored ibm vs chain");
    eprintln!("  sab_probe bones <megapack> <name_substr>   per-bone truth vs chain, and the local it implies");
    eprintln!("  sab_probe names <skeleton.json> -              recover bone names by hashing candidates");
    eprintln!("  sab_probe anim  <anim_bone_map.json> <skeleton.json>");
    eprintln!("                                            does each track drive the bone it claims?");
    eprintln!("  sab_probe editnodes <info|list|tree> <EditNodes.pack>   dynamic-object DB");
    eprintln!("  sab_probe gametext  <info|list|get|hash|roundtrip> <GameText.dlg>");
    eprintln!();
    eprintln!("e.g. sab_probe parts \"C:/GOG Games/The Saboteur/Global/Dynamic0.megapack\" CH_AL_SeanDevlin");
    std::process::exit(2)
}

mod audio;
mod editnodes;
mod gametext;
mod texbind;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    // These have their own sub-dispatch (variable arity).
    if args.len() > 1 && args[1] == "texbind" {
        texbind::run(&args[2..]);
        return;
    }
    if args.len() > 1 && args[1] == "editnodes" {
        editnodes::run(&args[2..]);
        return;
    }
    if args.len() > 1 && args[1] == "gametext" {
        gametext::run(&args[2..]);
        return;
    }
    if args.len() > 1 && args[1] == "audio" {
        audio::run(&args[2..]);
        return;
    }
    if args.len() < 4 {
        usage();
    }
    let (cmd, path, filter) = (args[1].as_str(), args[2].as_str(), args[3].as_str());

    match cmd {
        "tex-find" => { tex::cmd_find(path, filter); return; }
        "tex-mesh" => { tex::cmd_mesh(path, filter); return; }
        "tex-wsma" => { tex::cmd_wsma(path, filter); return; }
        "tex-cover" => { tex::cmd_cover(path, filter); return; }
        "tex-names" => { tex::cmd_names(path, filter); return; }
        "tex-raw" => { tex::cmd_raw(path, filter); return; }
        "tex-alt" => { tex::cmd_alt(path, filter); return; }
        "tex-key" => { tex::cmd_key(path, filter); return; }
        "tex-hash" => { tex::cmd_hash(path, filter); return; }
        "tex-prim" => { tex::cmd_prim(path, filter); return; }
        _ => {}
    }

    // `anim` reads two JSON reports rather than the megapack.
    if cmd == "names" {
        let s = std::fs::read_to_string(path).unwrap_or_else(|e| { eprintln!("read {path}: {e}"); std::process::exit(1) });
        probe_names(&s);
        return;
    }
    if cmd == "anim" {
        let a = std::fs::read_to_string(path).unwrap_or_else(|e| { eprintln!("read {path}: {e}"); std::process::exit(1) });
        let s = std::fs::read_to_string(filter).unwrap_or_else(|e| { eprintln!("read {filter}: {e}"); std::process::exit(1) });
        probe_anim(&a, &s);
        return;
    }

    let buf = std::fs::read(path).unwrap_or_else(|e| {
        eprintln!("read {path}: {e}");
        std::process::exit(1);
    });
    eprintln!("[*] {path} — {} bytes", buf.len());
    let hits = scan_meshes(&buf, filter);
    if hits.is_empty() {
        eprintln!("no MSHA mesh matched \"{filter}\"");
        std::process::exit(1);
    }
    eprintln!("[*] {} part(s) matched \"{filter}\"\n", hits.len());

    match cmd {
        "parts" => probe_parts(&hits),
        "bones" => probe_bones(hits.iter().max_by_key(|h| h.num_bones0).unwrap()),
        _ => usage(),
    }
}

