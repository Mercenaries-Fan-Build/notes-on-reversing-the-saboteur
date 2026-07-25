//! In-app MESH extraction — browse and load any skinned model straight out of a megapack.
//!
//! PORTED from `tools/sab_mesh` (the validated extractor; see `docs/formats/mesh_geometry.md`), with
//! the CLI / glTF / SMSH writers stripped and its bespoke Mat4 replaced by glam (the skinning
//! convention is column-vector `world[i] = world[parent] * local[i]`, and `Bone::inv_bind` is stored
//! ROW-MAJOR — see `skinning`). Do not re-derive these offsets.
//!
//! Container path: megapack entry → `MSHA` wrapper (276 B) → blob0 = zlib(MESH body),
//! blob1 = zlib(.dat VB/IB) immediately after blob0.
//!
//! Listing is CHEAP (scan for the `AHSM` magic + read its 276-byte header); the two zlib blobs are
//! only inflated when a model is actually loaded.

#![allow(dead_code)]

use std::io::Read;

use glam::{Mat4, Quat, Vec3};

use crate::formats::{Bone, Prim, Smsh};

fn u16at(b: &[u8], o: usize) -> u16 { u16::from_le_bytes([b[o], b[o + 1]]) }
fn u32at(b: &[u8], o: usize) -> u32 { u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]) }
fn i16at(b: &[u8], o: usize) -> i16 { i16::from_le_bytes([b[o], b[o + 1]]) }
fn i32at(b: &[u8], o: usize) -> i32 { i32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]) }
fn f32at(b: &[u8], o: usize) -> f32 { f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]) }

/// IEEE half → f32 (positions are half4, UVs half2).
fn half_to_f32(h: u16) -> f32 {
    let sign = ((h >> 15) & 1) as u32;
    let exp = ((h >> 10) & 0x1f) as u32;
    let man = (h & 0x3ff) as u32;
    let bits = match exp {
        0 => {
            if man == 0 {
                sign << 31
            } else {
                // subnormal → normalize
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
fn half_at(b: &[u8], o: usize) -> f32 { half_to_f32(u16at(b, o)) }

/// One model found in a megapack — enough to list it without inflating anything.
#[derive(Clone)]
pub struct MeshEntry {
    pub name: String,
    pub file_off: usize, // offset of the MSHA magic within the megapack
    comp0: u32,
    unc0: u32,
    comp1: u32,
    unc1: u32,
}

/// A model decoded and ready to render.
pub struct LoadedMesh {
    pub name: String,
    pub mesh: Smsh,
    pub bones: Vec<Bone>,
    /// Per-drawcall `parentBone` (rigid attachment target), same order as `mesh.prims`.
    pub prim_parent_bone: Vec<u16>,
    /// Per-bone name hash, for matching an attachment bone across parts.
    pub bone_hashes: Vec<u32>,
    /// The asset's OWN inverse-bind for each bone this part SKINS, keyed by bone name-hash
    /// (row-major, as stored). This is ground truth: the derived `world[i] = world[parent] · local[i]`
    /// chain disagrees with it — a character's face bones have on-disk local translations of
    /// (0,0,0), so the chain collapses that subtree onto one point and only these matrices know
    /// where the face really is. Pooled across parts during assembly (see `assemble`).
    pub stored_ibm: std::collections::HashMap<u32, [f32; 16]>,
    /// `(part name, index_start, index_count)` per source part, in merge order.
    ///
    /// Each part ships its OWN texture bundle, so a character cannot be textured from one pool —
    /// this is what lets the resolver seed each part's submeshes from that part's own textures
    /// instead of smearing the first bundle it found across the whole body.
    pub part_ranges: Vec<(String, u32, u32)>,
}

/// First offset of `needle` in `buf` at or after `from` (small linear scan — used only over a
/// sub-pack's ALBS header/directory, a few KB).
fn find_subseq(buf: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if from >= buf.len() {
        return None;
    }
    buf[from..].windows(needle.len()).position(|w| w == needle).map(|i| from + i)
}

/// A `Matrix44` as the MESH stores it: 16 f32, **column-major** — the translation sits in the last
/// four (`m[12..15]`), the last row is `(0,0,0,1)`, which is glam's own `from_cols_array` layout.
///
/// Getting this backwards is undetectable at bind pose. `jointMatrix = world · inv_bind` comes out
/// identity for ANY self-consistent pair of world and inverse-bind, so a transposed read renders a
/// flawless bind pose and only falls apart once a clip drives the rig. `matrices_are_column_major`
/// pins the layout against the real bytes rather than against our belief about them.
fn mat44(raw: &[f32; 16]) -> Mat4 {
    Mat4::from_cols_array(raw)
}

/// The inverse of [`row_major`]: a `Bone`'s stored row-major matrix back to a `Mat4`.
fn from_row_major(rm: &[f32; 16]) -> Mat4 {
    let mut cm = [0f32; 16];
    for c in 0..4 {
        for r in 0..4 {
            cm[c * 4 + r] = rm[r * 4 + c];
        }
    }
    Mat4::from_cols_array(&cm)
}

/// The row-major form `formats::Bone` stores (`skinning` transposes it back).
fn row_major(m: Mat4) -> [f32; 16] {
    let cm = m.to_cols_array();
    let mut rm = [0f32; 16];
    for c in 0..4 {
        for r in 0..4 {
            rm[r * 4 + c] = cm[c * 4 + r];
        }
    }
    rm
}

fn parse_msha_header(buf: &[u8], off: usize) -> Option<(String, u32, u32, u32, u32)> {
    if off + 276 > buf.len() {
        return None;
    }
    // MSHA: id(4) uncompressedSize0(4) uncompressedSize1(4) compressedSize0(4) compressedSize1(4) name[0x100]
    let unc0 = u32at(buf, off + 4);
    let unc1 = u32at(buf, off + 8);
    let c0 = u32at(buf, off + 12);
    let c1 = u32at(buf, off + 16);
    let name_bytes = &buf[off + 20..off + 276];
    let end = name_bytes.iter().position(|&b| b == 0).unwrap_or(0);
    if end == 0 {
        return None;
    }
    let nm = &name_bytes[..end];
    if !nm.iter().all(|&b| (0x20..0x7f).contains(&b)) {
        return None;
    }
    Some((String::from_utf8_lossy(nm).into_owned(), c0, unc0, c1, unc1))
}

fn zlib_inflate(data: &[u8], expected: usize) -> Option<Vec<u8>> {
    let mut d = flate2::read::ZlibDecoder::new(data);
    let mut out = Vec::with_capacity(expected);
    d.read_to_end(&mut out).ok()?;
    Some(out)
}

/// List every MSHA model in a megapack — **near-instant**, by walking the MSHA chain rather than
/// scanning the 715 MB.
///
/// The MSHA wrappers in a bundle are **contiguous**: the first sits right after the ALBS directory,
/// and `next = pos + 276 + compressedSize0 + compressedSize1` reaches the next one exactly (verified
/// bit-for-bit: chain-walk == full-file scan, 6807 == 6807 meshes across all 759 Dynamic0 bundles,
/// zero mismatches). So we read only the 276-byte headers (~2 MB total for 6807 meshes) and skip every
/// compressed blob — the ALBS layout is the earmark. Header-only; no inflate.
pub fn list_meshes(mp: &crate::pack::Megapack) -> Vec<MeshEntry> {
    let mut out = Vec::new();
    for e in mp.entries() {
        let sub = mp.slice(e);
        let base = e.offset as usize;
        // The first MSHA sits just past the ALBS header + directory (a few KB); find it, then chain.
        let Some(first) = find_subseq(sub, b"AHSM", 0x20) else { continue };
        let mut p = first;
        while p + 276 <= sub.len() && &sub[p..p + 4] == b"AHSM" {
            // A garbage/non-printable header ends the chain (we've walked past the MSHA region).
            let Some((name, c0, unc0, c1, unc1)) = parse_msha_header(sub, p) else { break };
            if c0 > 0 && unc0 > 0 {
                out.push(MeshEntry { name, file_off: base + p, comp0: c0, unc0, comp1: c1, unc1 });
            }
            // Advance by the true record stride regardless, so the chain stays aligned.
            p += 276 + c0 as usize + c1 as usize;
        }
    }
    out
}

/// Inflate + decode one listed model.
pub fn load(buf: &[u8], e: &MeshEntry) -> Result<LoadedMesh, String> {
    let start0 = e.file_off + 276;
    let end0 = start0 + e.comp0 as usize;
    if end0 > buf.len() {
        return Err("MESH body blob out of range".into());
    }
    let body = zlib_inflate(&buf[start0..end0], e.unc0 as usize).ok_or("inflate MESH body failed")?;
    if body.len() != e.unc0 as usize {
        return Err(format!("MESH body {} != declared {}", body.len(), e.unc0));
    }
    // blob1 (.dat VB/IB) sits immediately after blob0.
    let start1 = end0;
    let dat = if e.comp1 > 0 && start1 + e.comp1 as usize <= buf.len() {
        zlib_inflate(&buf[start1..start1 + e.comp1 as usize], e.unc1 as usize).unwrap_or_default()
    } else {
        Vec::new()
    };
    if dat.is_empty() {
        return Err("no .dat (VB/IB) blob".into());
    }
    let tail = parse_mesh(&body)?;
    let mut mesh = decode_geometry(&tail, &dat)?;
    crate::formats::bind_rigid_attachments(&mut mesh, &crate::skinning::bind_world(&tail.bones));
    let prim_parent_bone = tail.draws.iter().map(|d| d.parent_bone).collect();
    let bone_hashes = tail.bone_hashes.clone();
    let stored_ibm = tail.stored_ibm.clone();
    let part_ranges = vec![(e.name.clone(), 0u32, mesh.indices.len() as u32)];
    Ok(LoadedMesh { name: e.name.clone(), mesh, bones: tail.bones, prim_parent_bone, bone_hashes, stored_ibm, part_ranges })
}

struct Stream {
    num_vertices: u32,
    format: u32,
    vb_offset: u32,
    vb_stride: u32,
    ib_offset: u32,
    face_type: u32,
    num_indices: u32,
}
struct Primitive { stream_index: u32, index_offset: u32, num_indices: u32 }
struct DrawCall { primitive_index: u32, material: u32, parent_bone: u16 }

struct MeshTail {
    bones: Vec<Bone>,
    bone_hashes: Vec<u32>,
    /// The `localTMS` array, raw and in file order — the asset's OWN local matrix per bone. The
    /// bind chain is built from these, not from the RTS triple (see `parse_mesh`).
    local_tms: Vec<[f32; 16]>,
    /// bone name-hash -> the asset's own inverse-bind, for the bones this part skins.
    stored_ibm: std::collections::HashMap<u32, [f32; 16]>,
    bone_ids: Vec<u8>,
    remaps: Vec<u32>, // boneRemap[i].boneId
    streams: Vec<Stream>,
    prims: Vec<Primitive>,
    draws: Vec<DrawCall>,
}

/// MESH header + skeleton + tail. Offsets per `sab_mesh` / `docs/formats/mesh_geometry.md`.
fn parse_mesh(body: &[u8]) -> Result<MeshTail, String> {
    if body.len() < 244 {
        return Err("MESH body too short".into());
    }
    let num_bones0 = u32at(body, 204) as usize;
    let num_bone_remaps = u32at(body, 208) as usize;
    let num_streams = u16at(body, 216) as usize;
    let num_primitives = u16at(body, 218) as usize;
    let num_draw_calls = u32at(body, 232) as usize;
    if num_bones0 <= 1 {
        return Err(format!("mesh not skinned (numBones0={num_bones0})"));
    }

    // MESHSkeleton header @244 (11 u32)
    let mut p = 244usize;
    let num_unk_bones0 = u32at(body, p);
    let num_bones = u32at(body, p + 12) as usize; // numBones2
    let num_unk_bones1 = u32at(body, p + 16);
    let num_bones3 = u32at(body, p + 20) as usize;
    let num_bones4 = u32at(body, p + 28) as usize;
    if num_bones != num_bones3 || num_bones != num_bones4 {
        return Err(format!("bone count mismatch: {num_bones}/{num_bones3}/{num_bones4}"));
    }
    if num_bones != num_bones0 {
        return Err(format!("numBones0({num_bones0}) != numBones2({num_bones})"));
    }
    p += 44;

    // boneIds: numBones u8 + numUnkBones0 pad
    if p + num_bones > body.len() {
        return Err("boneIds out of range".into());
    }
    let bone_ids = body[p..p + num_bones].to_vec();
    p += num_bones + num_unk_bones0 as usize;

    // localTMS: numBones * Matrix44 — the bone's own LOCAL matrix.
    //
    // This used to be skipped on the grounds that the RTS triple below says the same thing, and it
    // does (`the_matrix_and_the_rts_triple_state_the_same_local`, worst deviation ~1e-7). We read it
    // anyway because it is EXACT: composing a matrix from a TRS triple and decomposing it back is
    // not lossless, and the rig hands `Bone::local_m` straight to the renderer. It also gives the
    // column-major layout a second witness — the two encodings only agree if the matrix is read the
    // right way round.
    let mut local_tms = Vec::with_capacity(num_bones);
    for i in 0..num_bones {
        let o = p + i * 64;
        if o + 64 > body.len() {
            return Err("localTMS out of range".into());
        }
        let mut m = [0f32; 16];
        for (k, slot) in m.iter_mut().enumerate() {
            *slot = f32at(body, o + k * 4);
        }
        local_tms.push(m);
    }
    p += num_bones * 64;

    // bones: numBones * Bone(64) — boneName0 (hash) @0
    let mut name_hashes = Vec::with_capacity(num_bones);
    for _ in 0..num_bones {
        name_hashes.push(u32at(body, p));
        p += 64;
    }
    // transforms: numBones * RTSValue(48)
    let mut trs = Vec::with_capacity(num_bones);
    for _ in 0..num_bones {
        let t = [f32at(body, p), f32at(body, p + 4), f32at(body, p + 8)];
        let r = [f32at(body, p + 16), f32at(body, p + 20), f32at(body, p + 24), f32at(body, p + 28)];
        let s = [f32at(body, p + 32), f32at(body, p + 36), f32at(body, p + 40)];
        trs.push((t, r, s));
        p += 48;
    }
    // parentIds: numBones i16
    let mut parents = Vec::with_capacity(num_bones);
    for _ in 0..num_bones {
        parents.push(i16at(body, p) as i32);
        p += 2;
    }
    p += 4 * num_bones; // numBones * null32
    if num_unk_bones1 != 0 {
        p += 2;
    }

    // Compose bind world (column-vector, parent * local), then inv_bind ROW-MAJOR for `Bone`.
    // The local comes from `localTMS`; the RTS triple is kept only for callers that read `Bone::t`.
    let locals: Vec<Mat4> = local_tms.iter().map(mat44).collect();
    let mut world = vec![Mat4::IDENTITY; num_bones];
    let mut done = vec![false; num_bones];
    let mut progress = true;
    while progress {
        progress = false;
        for i in 0..num_bones {
            if done[i] {
                continue;
            }
            let pp = parents[i];
            if pp < 0 || pp as usize >= num_bones {
                world[i] = locals[i];
                done[i] = true;
                progress = true;
            } else if done[pp as usize] {
                world[i] = world[pp as usize] * locals[i];
                done[i] = true;
                progress = true;
            }
        }
    }
    if done.iter().any(|&d| !d) {
        return Err("bone hierarchy cycle / dangling parent".into());
    }
    let bones: Vec<Bone> = (0..num_bones)
        .map(|i| {
            let (t, r, s) = trs[i];
            Bone {
                parent: parents[i],
                // A MESH stores only the hash; resolve it through the recovered dictionary so a
                // pack-loaded rig reads like the `.skel` one (and so anything that filters bones BY
                // NAME — the inspector tree, `anim_sweep`'s root chain — still works).
                name: crate::bone_names::name_for(name_hashes[i]),
                t,
                r,
                s,
                inv_bind: Some(row_major(world[i].inverse())),
                // Hand on the EXACT local matrix: a TRS round trip is not lossless, and this is the
                // transform the whole bind chain was composed from.
                local_m: Some(row_major(locals[i])),
            }
        })
        .collect();

    // ---- tail ----
    let mut stored_ibm: std::collections::HashMap<u32, [f32; 16]> = Default::default();
    let mut remaps = Vec::with_capacity(num_bone_remaps);
    if num_bone_remaps > 0 {
        let guard = u32at(body, p) as usize;
        if guard != num_bone_remaps {
            return Err(format!("boneRemap guard {guard} != {num_bone_remaps}"));
        }
        p += 8; // guard + null32
        for _ in 0..num_bone_remaps {
            let bone_id = u32at(body, p + 64); // after the 64-byte ibm
            // Keep the ibm rather than skipping it — it is the only correct source for this bone's
            // bind pose, and assembly pools these across a character's parts.
            if let Some(&h) = name_hashes.get(bone_id as usize) {
                let mut raw = [0f32; 16];
                for k in 0..16 {
                    raw[k] = f32at(body, p + k * 4);
                }
                stored_ibm.insert(h, raw);
            }
            remaps.push(bone_id);
            p += 68;
        }
    }
    let mut streams = Vec::with_capacity(num_streams);
    for _ in 0..num_streams {
        streams.push(Stream {
            num_vertices: u32at(body, p + 24),
            format: u32at(body, p + 40),
            vb_offset: u32at(body, p + 88),
            vb_stride: u32at(body, p + 120),
            ib_offset: u32at(body, p + 128),
            face_type: u32at(body, p + 140),
            num_indices: u32at(body, p + 144),
        });
        p += 152;
    }
    let mut prims = Vec::with_capacity(num_primitives);
    for _ in 0..num_primitives {
        if i32at(body, p + 4) != -1 {
            return Err("primitive const0 != -1".into());
        }
        prims.push(Primitive {
            stream_index: u32at(body, p + 80),
            index_offset: u32at(body, p + 88),
            num_indices: u32at(body, p + 96),
        });
        p += 100;
    }
    let mut draws = Vec::with_capacity(num_draw_calls);
    for _ in 0..num_draw_calls {
        draws.push(DrawCall {
            primitive_index: u32at(body, p),
            material: u32at(body, p + 4),
            // The rigid-attachment bone for UNSKINNED drawcall geometry (hats/props).
            parent_bone: u16at(body, p + 12),
        });
        p += 16;
    }

    Ok(MeshTail { bones, bone_hashes: name_hashes, local_tms, stored_ibm, bone_ids, remaps, streams, prims, draws })
}

#[derive(Clone, Copy, PartialEq)]
enum Attr { Position, BoneWeights, BoneIndices, Color, Uv, Normal, Tangent }

fn attr_size(a: Attr) -> usize {
    match a {
        Attr::Position => 8,     // R16G16B16A16 FLOAT (half4)
        Attr::BoneWeights => 4,  // R8G8B8A8 UNORM
        Attr::BoneIndices => 4,  // R8G8B8A8 UINT
        Attr::Color => 4,        // R8G8B8A8 UNORM
        Attr::Uv => 4,           // R16G16 FLOAT (half2)
        Attr::Normal => 12,      // R32G32B32 FLOAT
        Attr::Tangent => 4,      // R8G8B8A8 UNORM
    }
}

/// format = positionType:2 | skinType:2 | numColors:4 | numUVs:4 | normal:1 | tangent:1 | .. | tag:8(0x1B)
fn decode_format(fmt: u32) -> Result<Vec<(Attr, usize)>, String> {
    if (fmt >> 24) & 0xff != 0x1b {
        return Err(format!("unexpected constTag in format 0x{fmt:08x}"));
    }
    let position_type = fmt & 0x3;
    let skin_type = (fmt >> 2) & 0x3;
    let num_colors = (fmt >> 4) & 0xf;
    let num_uvs = (fmt >> 8) & 0xf;
    let has_normal = (fmt >> 12) & 1;
    let has_tangent = (fmt >> 13) & 1;
    if position_type != 2 {
        return Err(format!("unsupported positionType {position_type} in 0x{fmt:08x}"));
    }
    let mut list = vec![Attr::Position];
    if skin_type != 0 {
        list.push(Attr::BoneWeights);
        list.push(Attr::BoneIndices);
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
    Ok(out)
}

/// Decode every stream into one interleaved pool, resolving per-vertex bone indices to GLOBAL
/// skeleton bones via `global = boneIds[ boneRemaps[local] ]`, and flatten drawcalls into prims.
fn decode_geometry(tail: &MeshTail, dat: &[u8]) -> Result<Smsh, String> {
    let mut m = Smsh {
        positions: Vec::new(),
        normals: Vec::new(),
        uvs: Vec::new(),
        joints: Vec::new(),
        weights: Vec::new(),
        indices: Vec::new(),
        prims: Vec::new(),
    };
    let mut stream_ibase = Vec::with_capacity(tail.streams.len());

    for s in &tail.streams {
        if s.face_type != 1 {
            return Err(format!("unexpected faceType {} (only triangle-list)", s.face_type));
        }
        let layout = decode_format(s.format)?;
        let stride = s.vb_stride as usize;
        let nverts = s.num_vertices as usize;
        let vb = s.vb_offset as usize;
        if vb + nverts * stride > dat.len() {
            return Err("vertex buffer out of range".into());
        }
        let vbase = m.positions.len() as u32;

        for v in 0..nverts {
            let base = vb + v * stride;
            let (mut pos, mut nrm, mut uv) = ([0f32; 3], [0f32; 3], [0f32; 2]);
            let (mut jnt, mut wgt) = ([0u16; 4], [0f32; 4]);
            let mut got_uv = false;
            let mut has_skin = false;
            for &(a, ao) in &layout {
                let o = base + ao;
                match a {
                    Attr::Position => pos = [half_at(dat, o), half_at(dat, o + 2), half_at(dat, o + 4)],
                    Attr::Normal => nrm = [f32at(dat, o), f32at(dat, o + 4), f32at(dat, o + 8)],
                    Attr::Uv => {
                        if !got_uv {
                            uv = [half_at(dat, o), half_at(dat, o + 2)];
                            got_uv = true;
                        }
                    }
                    Attr::BoneWeights => {
                        for k in 0..4 {
                            wgt[k] = dat[o + k] as f32 / 255.0;
                        }
                        has_skin = true;
                    }
                    Attr::BoneIndices => {
                        for k in 0..4 {
                            let local = dat[o + k] as usize;
                            jnt[k] = if local < tail.remaps.len() {
                                let bone_id = tail.remaps[local] as usize;
                                *tail.bone_ids.get(bone_id).unwrap_or(&0) as u16
                            } else {
                                0
                            };
                        }
                    }
                    Attr::Color | Attr::Tangent => {}
                }
            }
            if has_skin {
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
            m.positions.push(pos);
            m.normals.push(nrm);
            m.uvs.push(uv);
            m.joints.push(jnt);
            m.weights.push(wgt);
        }

        // u16 triangle-list indices, rebased onto the global vertex pool.
        let ibase = m.indices.len() as u32;
        stream_ibase.push(ibase);
        let ib = s.ib_offset as usize;
        let nidx = s.num_indices as usize;
        if ib + nidx * 2 > dat.len() {
            return Err("index buffer out of range".into());
        }
        for k in 0..nidx {
            m.indices.push(u16at(dat, ib + k * 2) as u32 + vbase);
        }
    }

    for d in &tail.draws {
        let prim = tail
            .prims
            .get(d.primitive_index as usize)
            .ok_or_else(|| format!("drawcall references primitive {}", d.primitive_index))?;
        let ibase = stream_ibase.get(prim.stream_index as usize).copied().unwrap_or(0);
        m.prims.push(Prim {
            index_start: ibase + prim.index_offset,
            index_count: prim.num_indices,
            material_hash: d.material,
            flags: d.primitive_index,
            parent_bone: d.parent_bone,
        });
    }
    Ok(m)
}

#[cfg(test)]
mod bbox_tests {
    use super::*;

    /// Compute the correct MESH bounding volume (@76 size.xyz / @88 radius=½·|size| / @92 center.xyz)
    /// from decoded geometry, for Sean's Mattias parts in the mod pack — the fix for the port's
    /// zero/wrong bbox (cutscene frustum-culls the body → invisible). Validates the method on the
    /// vanilla parts first (their stored bbox is known-good), then prints values to bake into the port.
    #[test]
    fn sean_bbox_from_geometry() {
        let parts = [
            "CH_AL_SeanDevlin_01_LB",
            "CH_AL_SeanDevlin_01_UB",
            "CH_AL_SeanDevlin_01_GR",
            "CH_AL_SeanDevlin_01_HD",
            "CH_AL_SeanDevlin_01_HAT",
        ];
        let Some(s) = crate::settings::detected() else {
            eprintln!("skip: no Saboteur install detected");
            return;
        };
        for pk in ["patchdynamic0.megapack", "Dynamic0.megapack"] {
            let path = format!("{}/Global/{pk}", s.game_dir);
            if !std::path::Path::new(&path).exists() {
                eprintln!("skip: {path} not present");
                continue;
            }
            let pack = crate::pack::Megapack::open(&path).expect("open megapack");
            let list = list_meshes(&pack);
            let buf = pack.raw();
            eprintln!("\n=== {pk} ===");
            for part in parts {
                let Some(e) = list.iter().find(|e| e.name.eq_ignore_ascii_case(part)) else {
                    eprintln!("  {part:32} <not in pack>");
                    continue;
                };
                let lm = match load(&buf, e) {
                    Ok(l) => l,
                    Err(err) => {
                        eprintln!("  {part:32} load err: {err}");
                        continue;
                    }
                };
                let ps = &lm.mesh.positions;
                if ps.is_empty() {
                    eprintln!("  {part:32} 0 verts");
                    continue;
                }
                let (mut mn, mut mx) = ([f32::MAX; 3], [f32::MIN; 3]);
                for p in ps {
                    for k in 0..3 {
                        mn[k] = mn[k].min(p[k]);
                        mx[k] = mx[k].max(p[k]);
                    }
                }
                let size = [mx[0] - mn[0], mx[1] - mn[1], mx[2] - mn[2]];
                let ctr = [(mx[0] + mn[0]) * 0.5, (mx[1] + mn[1]) * 0.5, (mx[2] + mn[2]) * 0.5];
                let r = 0.5 * (size[0] * size[0] + size[1] * size[1] + size[2] * size[2]).sqrt();
                eprintln!(
                    "  {part:32} verts={:5} size=({:.4},{:.4},{:.4}) r={:.4} center=({:.4},{:.4},{:.4})",
                    ps.len(), size[0], size[1], size[2], r, ctr[0], ctr[1], ctr[2]
                );
            }
        }
    }
}

#[cfg(test)]
mod layout_tests {
    use super::*;

    /// Open a megapack and hand back the raw MESH body of the first model matching `token`.
    fn a_real_mesh_body(token: &str) -> Option<Vec<u8>> {
        let s = crate::settings::detected()?;
        let pack = crate::pack::Megapack::open(&s.megapack()).ok()?;
        let list = list_meshes(&pack);
        let buf = pack.raw();
        let e = list.iter().find(|e| e.name.to_ascii_lowercase().contains(token))?;
        let start = e.file_off + 276;
        zlib_inflate(&buf[start..start + e.comp0 as usize], e.unc0 as usize)
    }

    /// **Which way round are the MESH's 4×4 matrices stored?** Answered from the bytes, not from
    /// belief: an affine transform has a last row of `(0,0,0,1)`, and where those four values sit
    /// tells you the layout. Column-major puts the translation in `raw[12..14]` and the zeros at
    /// `raw[3]`, `raw[7]`, `raw[11]`; row-major is the transpose of that.
    ///
    /// This has to be asserted because getting it wrong is SILENT. `jointMatrix = world · inv_bind`
    /// is identity for any self-consistent pair, so a transposed read renders a perfect bind pose and
    /// only tears once a clip plays — which, until the app booted from the pack, nothing here did.
    #[test]
    fn matrices_are_column_major() {
        let Some(body) = a_real_mesh_body("seandevlin") else {
            eprintln!("skip: no Saboteur install detected");
            return;
        };
        let tail = parse_mesh(&body).expect("parse MESH");
        assert!(!tail.local_tms.is_empty(), "no localTMS array");

        let mut col_major = 0;
        let mut row_major_count = 0;
        let check = |m: &[f32; 16]| -> (bool, bool) {
            let z = |v: f32| v.abs() < 1e-6;
            let one = (m[15] - 1.0).abs() < 1e-6;
            (one && z(m[3]) && z(m[7]) && z(m[11]), one && z(m[12]) && z(m[13]) && z(m[14]))
        };
        // Every localTMS and every stored inverse-bind, over one real character part.
        let all: Vec<&[f32; 16]> = tail.local_tms.iter().chain(tail.stored_ibm.values()).collect();
        for m in &all {
            let (c, r) = check(m);
            // A matrix with no translation at all satisfies both; it says nothing either way.
            if c && !r {
                col_major += 1;
            } else if r && !c {
                row_major_count += 1;
            }
        }
        eprintln!(
            "{} matrices: {col_major} decisively column-major, {row_major_count} decisively row-major",
            all.len()
        );
        assert!(col_major > 0, "no matrix carried a translation — inconclusive sample");
        assert_eq!(
            row_major_count, 0,
            "MESH matrices are read as column-major ({col_major} agree), but {row_major_count} say otherwise"
        );
    }

    /// A MESH states each bone's local transform TWICE — as a `localTMS` matrix and as the RTS triple
    /// after it — and the two must say the same thing. We chain the matrix (it is exact, where a TRS
    /// round trip is not), so this is the check that doing so cannot change the rig.
    ///
    /// It is also the control for [`matrices_are_column_major`]: read the matrix the wrong way round
    /// and it stops agreeing with the triple, which is a second, independent witness to the layout.
    #[test]
    fn the_matrix_and_the_rts_triple_state_the_same_local() {
        let Some(body) = a_real_mesh_body("seandevlin") else {
            eprintln!("skip: no Saboteur install detected");
            return;
        };
        let tail = parse_mesh(&body).expect("parse MESH");
        let n = tail.bones.len();
        let mut worst = 0f32;
        let mut worst_bone = 0usize;
        for i in 0..n {
            let b = &tail.bones[i];
            let from_trs = Mat4::from_scale_rotation_translation(
                Vec3::from_array(b.s),
                Quat::from_xyzw(b.r[0], b.r[1], b.r[2], b.r[3]),
                Vec3::from_array(b.t),
            );
            let d = mat44(&tail.local_tms[i])
                .to_cols_array()
                .iter()
                .zip(from_trs.to_cols_array().iter())
                .fold(0f32, |a, (x, y)| a.max((x - y).abs()));
            if d > worst {
                worst = d;
                worst_bone = i;
            }
        }
        eprintln!(
            "{n} bones: worst localTMS vs RTS deviation {worst:.6} (bone {worst_bone} '{}')",
            tail.bones[worst_bone].name
        );
        assert!(worst < 1e-3, "localTMS and the RTS triple disagree by {worst} on bone {worst_bone}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Against the REAL install: list models in Dynamic0 and load Sean's `_GR` part, checking it
    /// reproduces what `sab_mesh` wrote into `output/skeletons/parts/sean_GR.smsh`
    /// (3389 verts / 10044 indices / 5 prims, per docs/formats/mesh_geometry.md).
    #[test]
    fn load_sean_gr_from_megapack() {
        let Some(s) = crate::settings::detected() else {
            eprintln!("skip: no Saboteur install detected");
            return;
        };
        let mp = &s.megapack();
        if !std::path::Path::new(mp).exists() {
            eprintln!("skip: {mp} not present");
            return;
        }
        let pack = crate::pack::Megapack::open(mp).expect("open megapack");
        let list = list_meshes(&pack);
        let buf = pack.raw();
        eprintln!("listed {} models", list.len());
        assert!(list.len() > 10, "expected many models");
        let e = list
            .iter()
            .find(|e| e.name.eq_ignore_ascii_case("CH_AL_SeanDevlin_01_GR"))
            .expect("Sean _GR present");
        let lm = load(&buf, e).expect("load Sean _GR");
        eprintln!(
            "loaded {}: {} verts, {} idx, {} prims, {} bones",
            lm.name,
            lm.mesh.positions.len(),
            lm.mesh.indices.len(),
            lm.mesh.prims.len(),
            lm.bones.len()
        );
        assert_eq!(lm.mesh.positions.len(), 3389, "vertex count vs sab_mesh");
        assert_eq!(lm.mesh.indices.len(), 10044, "index count vs sab_mesh");
        assert_eq!(lm.mesh.prims.len(), 5, "drawcall count vs sab_mesh");
        assert_eq!(lm.bones.len(), 191, "bone count");
        assert!(lm.mesh.indices.iter().all(|&i| (i as usize) < lm.mesh.positions.len()));
        // The bind pose must compose to ~identity joint matrices (proves TRS + inv_bind agree).
        let worst = crate::skinning::bind_pose(&lm.bones)
            .iter()
            .flat_map(|m| (*m - glam::Mat4::IDENTITY).to_cols_array())
            .fold(0f32, |a, c| a.max(c.abs()));
        eprintln!("bind-pose max deviation from identity: {worst:.6}");
        assert!(worst < 1e-3, "bind pose should be identity, got {worst}");
    }
}

#[cfg(test)]
mod hat_tests {
    use super::*;

    /// The hat sits on the floor because its geometry is UNSKINNED and authored at the origin — it is
    /// rigid attachment geometry whose target lives in the drawcall's `parentBone`, a field SMSH
    /// never stored. Prove it: Sean's HAT part should have zero skin influence and a non-zero
    /// parentBone naming a head bone.
    #[test]
    fn hat_is_rigid_attachment_with_parent_bone() {
        let Some(s) = crate::settings::detected() else {
            eprintln!("skip: no Saboteur install detected");
            return;
        };
        let mp = &s.megapack();
        if !std::path::Path::new(mp).exists() {
            eprintln!("skip: no megapack");
            return;
        }
        let pack = crate::pack::Megapack::open(mp).expect("open megapack");
        let list = list_meshes(&pack);
        let buf = pack.raw();
        for e in list.iter().filter(|e| e.name.to_ascii_lowercase().contains("seandevlin")) {
            let Ok(lm) = load(buf, e) else { continue };
            let skinned = lm
                .mesh
                .weights
                .iter()
                .any(|w| w[0] + w[1] + w[2] + w[3] > 0.0001);
            let ymin = lm.mesh.positions.iter().map(|p| p[1]).fold(f32::MAX, f32::min);
            let ymax = lm.mesh.positions.iter().map(|p| p[1]).fold(f32::MIN, f32::max);
            let pbs: Vec<u16> = {
                let mut v = lm.prim_parent_bone.clone();
                v.sort_unstable();
                v.dedup();
                v
            };
            eprintln!(
                "{:34} verts={:5} skinned={:5} Y[{:6.2},{:6.2}] parentBone(s)={:?}",
                lm.name, lm.mesh.positions.len(), skinned, ymin, ymax, pbs
            );
            // For an unskinned part, resolve what its parentBone actually IS on its own rig.
            if !skinned {
                for &pb in &pbs {
                    let hash = lm.bone_hashes.get(pb as usize).copied().unwrap_or(0);
                    let world_y = lm
                        .bones
                        .get(pb as usize)
                        .map(|b| b.t[1])
                        .unwrap_or(0.0);
                    eprintln!(
                        "     -> parentBone {pb} name_hash=0x{hash:08X} local_t.y={world_y:.3}"
                    );
                }
            }
        }
    }
}

/// Assemble a whole character from its parts, the way the game does.
///
/// A character is not one mesh: `assets::Asset` resolves it — from the game's own GameTemplates —
/// into an `FxHumanHead` plus several `FxHumanBodyPart` meshes (HD, UB, LB, GR, HAT…). Showing one
/// of those is showing a floating torso; this stitches them into the figure.
///
/// Two things make it non-trivial, both learned the hard way:
///
/// 1. **Bone INDEX is not portable between parts.** Each part carries its own PRUNED rig with its
///    own ordering — for Sean, 175 of 191 indices name a different bone in a different part. The
///    name-hash IS stable (it is the key the animation system itself uses), so every part's joints
///    are re-keyed onto one canonical skeleton by hash. Concatenating without that silently rigs the
///    head to the glove's bones, and looks perfect at bind pose.
/// 2. **The derived bind pose is wrong where it matters most.** `world[i] = world[parent] · local[i]`
///    disagrees with the asset's own inverse-bind matrices, because a character's face bones store a
///    local translation of (0,0,0) and the chain collapses that whole subtree onto one point. Every
///    part ships correct matrices for the bones IT skins, so assembling — which loads every part —
///    is exactly when the union becomes available. We pool them and fix the canonical skeleton.
pub fn assemble(buf: &[u8], parts: &[MeshEntry]) -> Result<LoadedMesh, String> {
    if parts.is_empty() {
        return Err("no parts".into());
    }
    let mut loaded: Vec<LoadedMesh> = Vec::new();
    let mut errs: Vec<String> = Vec::new();
    for e in parts {
        match load(buf, e) {
            Ok(l) => loaded.push(l),
            // A part that fails is skipped, not fatal: a character with a missing hat is still worth
            // looking at, and the status line reports what was dropped.
            Err(err) => errs.push(format!("{}: {err}", e.name)),
        }
    }
    if loaded.is_empty() {
        return Err(errs.join("; "));
    }

    // ---- one UNION skeleton, keyed by bone name-hash ----
    //
    // No single part holds every bone: each ships a PRUNED rig with only what it needs, so the
    // richest part is still missing others' bones. Picking it and hoping meant vertices whose bone
    // was absent had to be pinned to the root (hundreds of them), and the same character assembled
    // differently depending on which part happened to win. So take the richest part as the SPINE —
    // it has the deepest correct hierarchy — then graft in every bone the other parts know about.
    //
    // A grafted bone keeps its parent by HASH; if that parent is itself unknown it attaches to the
    // root, which is the honest fallback (a bone with nowhere to hang belongs at the origin).
    let canon = (0..loaded.len()).max_by_key(|i| loaded[*i].bones.len()).unwrap();
    let mut bones = loaded[canon].bones.clone();
    let mut hashes = loaded[canon].bone_hashes.clone();
    let mut index_of: std::collections::HashMap<u32, u16> =
        hashes.iter().enumerate().map(|(i, h)| (*h, i as u16)).collect();

    // Pass 1 — append every unknown bone, remembering its parent's HASH (the index is meaningless
    // here and the parent may itself still be ungrafted).
    let mut want_parent: std::collections::HashMap<u16, u32> = Default::default();
    let mut grafted = 0usize;
    for l in &loaded {
        for (bi, h) in l.bone_hashes.iter().enumerate() {
            if index_of.contains_key(h) {
                continue;
            }
            let Some(src) = l.bones.get(bi) else { continue };
            let idx = bones.len() as u16;
            if src.parent >= 0 {
                if let Some(ph) = l.bone_hashes.get(src.parent as usize) {
                    want_parent.insert(idx, *ph);
                }
            }
            let mut b = src.clone();
            b.parent = -1; // provisional; resolved below
            bones.push(b);
            hashes.push(*h);
            index_of.insert(*h, idx);
            grafted += 1;
        }
    }
    // Pass 2 — now that every bone exists, hook each graft to its real parent.
    for (idx, ph) in &want_parent {
        bones[*idx as usize].parent = index_of.get(ph).map(|i| *i as i32).unwrap_or(0);
    }

    // ---- correct the bind pose from the pooled ground truth ----
    let mut truth: std::collections::HashMap<u32, [f32; 16]> = Default::default();
    for l in &loaded {
        for (h, m) in &l.stored_ibm {
            truth.entry(*h).or_insert(*m);
        }
    }
    // Correcting the bind is BOTH halves or neither. `jointMatrix = world · inv_bind`, so swapping
    // in a true inv_bind while `world` still comes from the bad chain makes the two disagree and the
    // mesh deforms AT BIND POSE — worse than leaving both consistently wrong. So: rebuild each
    // bone's WORLD from the truth, then re-derive its local TRS and inv_bind from that world, and
    // the hierarchy reproduces the real bind pose.
    // A bone's own local, exactly as the file gave it (`parse_mesh` put `localTMS` here). Falling
    // back to the TRS triple would reintroduce the collapse it exists to avoid.
    let local_of = |b: &Bone| match &b.local_m {
        Some(rm) => from_row_major(rm),
        None => glam::Mat4::from_scale_rotation_translation(
            glam::Vec3::from_array(b.s),
            glam::Quat::from_xyzw(b.r[0], b.r[1], b.r[2], b.r[3]),
            glam::Vec3::from_array(b.t),
        ),
    };

    let n = bones.len();
    let mut world = vec![glam::Mat4::IDENTITY; n];
    let mut done = vec![false; n];
    let mut corrected = 0usize;
    // A bone with ground truth resolves immediately, whatever its position in the list.
    for i in 0..n {
        if let Some(m) = truth.get(&hashes[i]) {
            world[i] = mat44(m).inverse();
            done[i] = true;
            corrected += 1;
        }
    }
    // The rest chain from their parent. Grafted bones break the parent-before-child ordering the
    // file guarantees, so iterate to a fixed point rather than assuming one pass suffices.
    let mut guard = 0;
    while done.iter().any(|d| !d) && guard <= n {
        guard += 1;
        let mut progress = false;
        for i in 0..n {
            if done[i] {
                continue;
            }
            let p = bones[i].parent;
            if p < 0 {
                world[i] = local_of(&bones[i]);
                done[i] = true;
                progress = true;
            } else if done[p as usize] {
                world[i] = world[p as usize] * local_of(&bones[i]);
                done[i] = true;
                progress = true;
            }
        }
        if !progress {
            break; // a cycle or dangling parent: leave the remainder at their local
        }
    }
    for i in 0..n {
        if !done[i] {
            world[i] = local_of(&bones[i]);
        }
    }
    for i in 0..n {
        let p = bones[i].parent;
        let local = if p < 0 {
            world[i]
        } else {
            world[p as usize].inverse() * world[i]
        };
        // Keep the TRS for anything that reads it, but hand the renderer the EXACT matrix —
        // the decomposition is not lossless and the round trip is what deformed the mesh.
        let (sc, rot, tr) = local.to_scale_rotation_translation();
        bones[i].t = tr.to_array();
        bones[i].r = [rot.x, rot.y, rot.z, rot.w];
        bones[i].s = sc.to_array();
        bones[i].local_m = Some(row_major(local));
        bones[i].inv_bind = Some(row_major(world[i].inverse()));
    }

    // ---- merge geometry, re-keying every part's joints onto the canonical rig ----
    let mut out = crate::formats::Smsh {
        positions: Vec::new(),
        normals: Vec::new(),
        uvs: Vec::new(),
        joints: Vec::new(),
        weights: Vec::new(),
        indices: Vec::new(),
        prims: Vec::new(),
    };
    let mut prim_parent_bone: Vec<u16> = Vec::new();
    let mut dropped = 0usize;

    let mut part_ranges: Vec<(String, u32, u32)> = Vec::new();
    for l in &loaded {
        let base_v = out.positions.len() as u32;
        let base_i = out.indices.len() as u32;
        // this part's local bone index -> canonical index, by name hash
        let map: Vec<Option<u16>> =
            l.bone_hashes.iter().map(|h| index_of.get(h).copied()).collect();

        out.positions.extend_from_slice(&l.mesh.positions);
        out.normals.extend_from_slice(&l.mesh.normals);
        out.uvs.extend_from_slice(&l.mesh.uvs);
        out.weights.extend_from_slice(&l.mesh.weights);
        for j4 in &l.mesh.joints {
            let mut o = [0u16; 4];
            for k in 0..4 {
                match map.get(j4[k] as usize).copied().flatten() {
                    Some(c) => o[k] = c,
                    // A bone this part uses that the canonical rig lacks: pin to the root rather
                    // than to a WRONG bone — a vertex that does not move beats one that flies.
                    None => {
                        o[k] = 0;
                        dropped += 1;
                    }
                }
            }
            out.joints.push(o);
        }
        out.indices.extend(l.mesh.indices.iter().map(|i| i + base_v));
        for (pi, pr) in l.mesh.prims.iter().enumerate() {
            let mut q = pr.clone();
            q.index_start += base_i;
            // the rigid-attachment bone has to travel through the same remap
            q.parent_bone = l
                .prim_parent_bone
                .get(pi)
                .and_then(|b| map.get(*b as usize).copied().flatten())
                .unwrap_or(0);
            prim_parent_bone.push(q.parent_bone);
            out.prims.push(q);
        }
        part_ranges.push((l.name.clone(), base_i, out.indices.len() as u32 - base_i));
    }

    // The invariant that makes a bind pose correct: world · inv_bind == identity for every bone.
    // It is what silently held while the chain was wrong-but-self-consistent, and what broke the
    // moment only half the correction was applied — so assert it rather than trust it.
    // Check the RENDER PATH, not our own bookkeeping. The renderer never sees `world` — it
    // recomposes it from the t/r/s we just wrote (`skinning::bind_world`), and that round trip goes
    // through `to_scale_rotation_translation`, which cannot represent shear and mis-signs mirrored
    // axes. Validating our own `world` against our own `inv_bind` would pass by construction and
    // prove nothing.
    // mirrors skinning::bind_local, so the check exercises what the renderer will actually do
    let exact_local = |b: &Bone| match &b.local_m {
        Some(rm) => from_row_major(rm),
        None => local_of(b),
    };
    let mut recomposed = vec![glam::Mat4::IDENTITY; n];
    let mut rdone = vec![false; n];
    let mut guard2 = 0;
    while rdone.iter().any(|d| !d) && guard2 <= n {
        guard2 += 1;
        let mut prog = false;
        for i in 0..n {
            if rdone[i] { continue; }
            let p = bones[i].parent;
            if p < 0 {
                recomposed[i] = exact_local(&bones[i]);
                rdone[i] = true; prog = true;
            } else if rdone[p as usize] {
                recomposed[i] = recomposed[p as usize] * exact_local(&bones[i]);
                rdone[i] = true; prog = true;
            }
        }
        if !prog { break; }
    }
    let mut worst_dev = 0f32;
    let mut worst_bone = 0usize;
    for i in 0..n {
        if let Some(ib) = &bones[i].inv_bind {
            let m = recomposed[i] * from_row_major(ib);
            let mut d = 0f32;
            for (a, b) in m.to_cols_array().iter().zip(glam::Mat4::IDENTITY.to_cols_array().iter()) {
                d = d.max((a - b).abs());
            }
            if d > worst_dev { worst_dev = d; worst_bone = i; }
        }
    }

    let name = parts[canon].name.clone();
    eprintln!(
        "[sab_workshop] assembled {} from {} part(s): {} verts, {} tris, {} bones \
         ({corrected} bind-corrected from the asset's own matrices{})",
        name,
        loaded.len(),
        out.positions.len(),
        out.indices.len() / 3,
        bones.len(),
        if dropped > 0 { format!(", {dropped} joint refs pinned to root") } else { String::new() }
    );
    if grafted > 0 {
        eprintln!("[sab_workshop]   union skeleton: {grafted} bone(s) grafted from other parts");
    }
    if worst_dev > 1e-3 {
        eprintln!(
            "[sab_workshop]   WARNING bind pose is not identity: worst |world·invBind - I| = {worst_dev:.4} at bone {worst_bone}              — the TRS round-trip lost the transform; the mesh is deformed before any clip plays"
        );
    }
    for e in &errs {
        eprintln!("[sab_workshop]   skipped part {e}");
    }
    Ok(LoadedMesh { name, mesh: out, bones, prim_parent_bone, bone_hashes: hashes, stored_ibm: truth, part_ranges })
}

// ---------------------------------------------------------------------------
// glTF (.glb) export — skinned bind-pose mesh + skeleton. Ported from sab_mesh's proven writer
// (same structure): JOINTS_0 carry global bone indices, skin.joints is the identity list [0..N), so
// nodeWorld[b] * inverseBindMatrices[b] = identity at bind pose. The one convention that matters:
// Bone::inv_bind is stored ROW-MAJOR here and glTF matrices are COLUMN-MAJOR, so each inverse-bind
// is transposed on the way out.
// ---------------------------------------------------------------------------

/// Row-major 4x4 (flat) to column-major flat — a transpose of the 16-float layout.
fn to_colmajor(m: &[f32; 16]) -> [f32; 16] {
    [
        m[0], m[4], m[8], m[12], m[1], m[5], m[9], m[13], m[2], m[6], m[10], m[14], m[3], m[7],
        m[11], m[15],
    ]
}

/// Serialize an assembled character as a standalone skinned bind-pose `.glb` (glTF 2.0 binary):
/// mesh + skeleton + skin. Untextured. One primitive over the full merged index buffer.
pub fn write_glb(lm: &LoadedMesh) -> Vec<u8> {
    let g = &lm.mesh;
    let nv = g.positions.len();
    let ni = g.indices.len();
    let nb = lm.bones.len();
    let has_skin = nb > 0 && g.joints.len() == nv && g.weights.len() == nv;

    let mut bin: Vec<u8> = Vec::new();
    let align = |b: &mut Vec<u8>| while b.len() % 4 != 0 { b.push(0) };

    let pos_off = bin.len();
    let (mut pmin, mut pmax) = ([f32::MAX; 3], [f32::MIN; 3]);
    for p in &g.positions {
        for k in 0..3 {
            pmin[k] = pmin[k].min(p[k]);
            pmax[k] = pmax[k].max(p[k]);
            bin.extend_from_slice(&p[k].to_le_bytes());
        }
    }
    align(&mut bin);
    let nrm_off = bin.len();
    for n in &g.normals {
        for k in 0..3 {
            bin.extend_from_slice(&n[k].to_le_bytes());
        }
    }
    align(&mut bin);
    let uv_off = bin.len();
    for u in &g.uvs {
        for k in 0..2 {
            bin.extend_from_slice(&u[k].to_le_bytes());
        }
    }
    align(&mut bin);
    let jnt_off = bin.len();
    for j in &g.joints {
        for k in 0..4 {
            bin.extend_from_slice(&j[k].to_le_bytes());
        }
    }
    align(&mut bin);
    let wgt_off = bin.len();
    for w in &g.weights {
        for k in 0..4 {
            bin.extend_from_slice(&w[k].to_le_bytes());
        }
    }
    align(&mut bin);
    let idx_off = bin.len();
    for &i in &g.indices {
        bin.extend_from_slice(&i.to_le_bytes());
    }
    align(&mut bin);
    let ibm_off = bin.len();
    const IDENT: [f32; 16] = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];
    for b in &lm.bones {
        for c in to_colmajor(&b.inv_bind.unwrap_or(IDENT)) {
            bin.extend_from_slice(&c.to_le_bytes());
        }
    }
    align(&mut bin);

    let f = |x: f32| if x == 0.0 { "0".to_string() } else { format!("{x}") };
    let mut bv = String::new();
    let push_bv = |s: &mut String, off: usize, len: usize, target: Option<u32>| {
        if !s.is_empty() {
            s.push(',');
        }
        s.push_str(&format!("{{\"buffer\":0,\"byteOffset\":{off},\"byteLength\":{len}"));
        if let Some(t) = target {
            s.push_str(&format!(",\"target\":{t}"));
        }
        s.push('}');
    };
    push_bv(&mut bv, pos_off, nv * 12, Some(34962));
    push_bv(&mut bv, nrm_off, nv * 12, Some(34962));
    push_bv(&mut bv, uv_off, nv * 8, Some(34962));
    push_bv(&mut bv, jnt_off, nv * 8, Some(34962));
    push_bv(&mut bv, wgt_off, nv * 16, Some(34962));
    push_bv(&mut bv, idx_off, ni * 4, Some(34963));
    push_bv(&mut bv, ibm_off, nb * 64, None);

    let mut acc = format!(
        "{{\"bufferView\":0,\"componentType\":5126,\"count\":{nv},\"type\":\"VEC3\",\"min\":[{},{},{}],\"max\":[{},{},{}]}}",
        f(pmin[0]), f(pmin[1]), f(pmin[2]), f(pmax[0]), f(pmax[1]), f(pmax[2])
    );
    acc.push_str(&format!(",{{\"bufferView\":1,\"componentType\":5126,\"count\":{nv},\"type\":\"VEC3\"}}"));
    acc.push_str(&format!(",{{\"bufferView\":2,\"componentType\":5126,\"count\":{nv},\"type\":\"VEC2\"}}"));
    acc.push_str(&format!(",{{\"bufferView\":3,\"componentType\":5123,\"count\":{nv},\"type\":\"VEC4\"}}"));
    acc.push_str(&format!(",{{\"bufferView\":4,\"componentType\":5126,\"count\":{nv},\"type\":\"VEC4\"}}"));
    acc.push_str(&format!(",{{\"bufferView\":5,\"componentType\":5125,\"count\":{ni},\"type\":\"SCALAR\"}}"));
    acc.push_str(&format!(",{{\"bufferView\":6,\"componentType\":5126,\"count\":{nb},\"type\":\"MAT4\"}}"));

    let attrs = if has_skin {
        "\"POSITION\":0,\"NORMAL\":1,\"TEXCOORD_0\":2,\"JOINTS_0\":3,\"WEIGHTS_0\":4"
    } else {
        "\"POSITION\":0,\"NORMAL\":1,\"TEXCOORD_0\":2"
    };
    let prim_json = format!("{{\"attributes\":{{{attrs}}},\"indices\":5}}");

    let mut children: Vec<Vec<usize>> = vec![Vec::new(); nb];
    let mut roots: Vec<usize> = Vec::new();
    for (b, bone) in lm.bones.iter().enumerate() {
        if bone.parent < 0 {
            roots.push(b);
        } else {
            children[bone.parent as usize].push(b);
        }
    }
    let mut nodes = String::new();
    for (b, bone) in lm.bones.iter().enumerate() {
        if b > 0 {
            nodes.push(',');
        }
        nodes.push_str(&format!(
            "{{\"translation\":[{},{},{}],\"rotation\":[{},{},{},{}],\"scale\":[{},{},{}]",
            f(bone.t[0]), f(bone.t[1]), f(bone.t[2]),
            f(bone.r[0]), f(bone.r[1]), f(bone.r[2]), f(bone.r[3]),
            f(bone.s[0]), f(bone.s[1]), f(bone.s[2])
        ));
        if !children[b].is_empty() {
            let cs: Vec<String> = children[b].iter().map(|c| c.to_string()).collect();
            nodes.push_str(&format!(",\"children\":[{}]", cs.join(",")));
        }
        nodes.push('}');
    }
    let mesh_node = nb;
    if nb > 0 {
        nodes.push(',');
    }
    if has_skin {
        nodes.push_str(&format!("{{\"name\":\"{}\",\"mesh\":0,\"skin\":0}}", lm.name));
    } else {
        nodes.push_str(&format!("{{\"name\":\"{}\",\"mesh\":0}}", lm.name));
    }

    let joints_list: Vec<String> = (0..nb).map(|b| b.to_string()).collect();
    let skins = if has_skin {
        format!(
            ",\"skins\":[{{\"inverseBindMatrices\":6,\"skeleton\":{},\"joints\":[{}]}}]",
            roots.first().copied().unwrap_or(0),
            joints_list.join(",")
        )
    } else {
        String::new()
    };
    let mut scene_nodes: Vec<String> = roots.iter().map(|r| r.to_string()).collect();
    scene_nodes.push(mesh_node.to_string());

    let json = format!(
        "{{\"asset\":{{\"version\":\"2.0\",\"generator\":\"sab_workshop\"}},\
\"scene\":0,\"scenes\":[{{\"nodes\":[{}]}}],\
\"nodes\":[{}],\
\"meshes\":[{{\"name\":\"{}\",\"primitives\":[{}]}}]{},\
\"accessors\":[{}],\
\"bufferViews\":[{}],\
\"buffers\":[{{\"byteLength\":{}}}]}}",
        scene_nodes.join(","), nodes, lm.name, prim_json, skins, acc, bv, bin.len()
    );

    let mut json_bytes = json.into_bytes();
    while json_bytes.len() % 4 != 0 {
        json_bytes.push(b' ');
    }
    while bin.len() % 4 != 0 {
        bin.push(0);
    }
    let total = 12 + 8 + json_bytes.len() + 8 + bin.len();
    let mut glb = Vec::with_capacity(total);
    glb.extend_from_slice(&0x4654_6C67u32.to_le_bytes()); // "glTF"
    glb.extend_from_slice(&2u32.to_le_bytes());
    glb.extend_from_slice(&(total as u32).to_le_bytes());
    glb.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
    glb.extend_from_slice(&0x4E4F_534Au32.to_le_bytes()); // "JSON"
    glb.extend_from_slice(&json_bytes);
    glb.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    glb.extend_from_slice(&0x004E_4942u32.to_le_bytes()); // "BIN\0"
    glb.extend_from_slice(&bin);
    glb
}

#[cfg(test)]
mod glb_tests {
    use super::*;
    use crate::formats::{Bone, Smsh};

    fn bone(parent: i32) -> Bone {
        Bone {
            parent,
            name: "b".into(),
            t: [0.0, 0.0, 0.0],
            r: [0.0, 0.0, 0.0, 1.0],
            s: [1.0, 1.0, 1.0],
            inv_bind: Some([1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0]),
            local_m: None,
        }
    }

    #[test]
    fn glb_container_is_valid() {
        let lm = LoadedMesh {
            name: "T".into(),
            mesh: Smsh {
                positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
                normals: vec![[0.0, 0.0, 1.0]; 3],
                uvs: vec![[0.0, 0.0]; 3],
                joints: vec![[0, 0, 0, 0]; 3],
                weights: vec![[1.0, 0.0, 0.0, 0.0]; 3],
                indices: vec![0, 1, 2],
                prims: vec![],
            },
            bones: vec![bone(-1), bone(0)],
            prim_parent_bone: vec![],
            bone_hashes: vec![0, 0],
            stored_ibm: std::collections::HashMap::new(),
            part_ranges: vec![],
        };
        let glb = write_glb(&lm);
        // GLB header: magic "glTF", version 2, total length == bytes.
        assert_eq!(&glb[0..4], b"glTF");
        assert_eq!(u32::from_le_bytes(glb[4..8].try_into().unwrap()), 2);
        assert_eq!(u32::from_le_bytes(glb[8..12].try_into().unwrap()) as usize, glb.len());
        // JSON chunk header.
        let json_len = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
        assert_eq!(&glb[16..20], b"JSON");
        let json = std::str::from_utf8(&glb[20..20 + json_len]).unwrap();
        // structure: one mesh, a skin (skinned), 7 base accessors, 7 bufferViews.
        assert!(json.contains("\"meshes\""));
        assert!(json.contains("\"skins\""));
        assert!(json.contains("\"JOINTS_0\":3"));
        assert!(json.contains("\"count\":3,\"type\":\"VEC3\"")); // positions/normals
        // BIN chunk follows and is 4-aligned.
        let bin_off = 20 + json_len;
        assert_eq!(&glb[bin_off + 4..bin_off + 8], b"BIN\0");
        assert_eq!(glb.len() % 4, 0);
    }
}
