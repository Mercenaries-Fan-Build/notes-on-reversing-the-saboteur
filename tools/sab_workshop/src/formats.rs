//! SMSH mesh + `.skel` skeleton readers.
//!
//! COPIED from `tools/sab_havok65/src/gltf.rs` (`read_smsh`, `read_skel`, `Bone`, `Smsh`).
//! Do not re-derive these formats.

// Some fields (bone names, UVs) are part of the on-disk format but unused by the viewer.
#![allow(dead_code)]

/// One skeleton bone (rest/bind pose, parent link, inverse-bind for skinning).
#[derive(Clone)]
pub struct Bone {
    pub parent: i32, // -1 for a root
    pub name: String,
    pub t: [f32; 3],
    pub r: [f32; 4], // xyzw
    pub s: [f32; 3],
    pub inv_bind: Option<[f32; 16]>, // row-major inverse bind matrix
    /// The bone's exact LOCAL transform (row-major), when a TRS triple cannot represent it.
    ///
    /// `Mat4::to_scale_rotation_translation` silently loses shear and mis-signs mirrored axes, so a
    /// matrix -> TRS -> matrix round trip is not lossless. Assembly derives locals from corrected
    /// world matrices and must hand the renderer exactly what it computed — measured deviation
    /// through the TRS round trip was 2.7, i.e. the mesh was deformed before any clip played.
    pub local_m: Option<[f32; 16]>,
}

/// One draw-range / sub-mesh: a contiguous slice of the index buffer with the material that
/// authored it. `material_hash` is a `pandemic_hash` of a WSAO material name (resolved to textures
/// by `tools/sab_wsao`). Preserved verbatim from `sab_mesh` through `merge_smsh.py` (which offsets
/// `index_start` across the merged parts), so a merged character mesh keeps one prim per part-material.
#[derive(Clone, Copy)]
pub struct Prim {
    pub index_start: u32,
    pub index_count: u32,
    pub material_hash: u32,
    pub flags: u32,
    /// Rigid-attachment bone for UNSKINNED geometry (hats/props) — `DrawCall.parentBone`. Only
    /// present in SMSH v2; a v1 file reads back 0 and its accessories cannot be placed.
    pub parent_bone: u16,
    /// Which level of detail this range belongs to. `0` is what the engine draws up close. A
    /// character's `LowDetailModel` parts are assembled in at `1` (see `assets::Asset::assembly`);
    /// a mesh's own geometry is `0` (formats with no LOD field carry only their drawn geometry, and
    /// `meshload` refuses a retail drawcall at any other in-mesh level).
    pub lod: u8,
}

/// Decoded SMSH geometry.
pub struct Smsh {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub joints: Vec<[u16; 4]>, // GLOBAL skeleton bone indices
    pub weights: Vec<[f32; 4]>,
    pub indices: Vec<u32>,
    pub prims: Vec<Prim>, // per-material draw ranges (empty for a pre-prims SMSH)
}

/// A non-overlapping draw range with the material(s) that authored it — the unit the renderer binds
/// a texture to and draws. `materials` are candidate material hashes in drawcall order (a range may
/// carry several: LOD / damage / detail passes); the first is the default diffuse pick.
#[derive(Clone)]
pub struct SubMesh {
    pub index_start: u32,
    pub index_count: u32,
    pub materials: Vec<u32>,
    /// The level of detail of every prim in this range (`Prim::lod`). The viewer draws level 0;
    /// the bundle writes one mesh object per level.
    pub lod: u8,
}

/// Reduce the (possibly overlapping) per-drawcall `prims` to a clean cover of the index buffer.
///
/// `sab_mesh` emits one prim per drawcall, so a mesh part shows up as a coarse "whole" range plus its
/// split children, and several drawcalls (materials) can target the same range (render passes). We
/// keep only **leaf** ranges — those that don't strictly contain another distinct range — and
/// collapse identical ranges into one `SubMesh` carrying every candidate material.
///
/// This is done **per level of detail**: a level's ranges never contain or merge with another
/// level's, so LOD 1 can never be folded into (or stacked onto) LOD 0. Each level must occupy one
/// contiguous span — `meshload::assemble` lays parts out level by level — and the spans must tile
/// `[0, total_indices)`. Within a level whose leaves do not tile its span (unexpected overlap/gap),
/// that level falls back to one whole-span submesh so it still renders (untextured) rather than
/// double-drawing; if the levels themselves do not tile the buffer, each level becomes one
/// untextured submesh over its own run of the buffer, so levels are never merged.
pub fn submesh_cover(prims: &[Prim], total_indices: u32) -> Vec<SubMesh> {
    let whole = || {
        vec![SubMesh { index_start: 0, index_count: total_indices, materials: Vec::new(), lod: 0 }]
    };
    if prims.is_empty() {
        return whole();
    }
    let mut levels: Vec<u8> = prims.iter().filter(|p| p.index_count > 0).map(|p| p.lod).collect();
    levels.sort_unstable();
    levels.dedup();
    let mut out: Vec<SubMesh> = Vec::new();
    for lod in levels {
        // Distinct ranges of this level, first-seen order, accumulating candidate materials.
        let mut ranges: Vec<(u32, u32, Vec<u32>)> = Vec::new(); // (start, count, materials)
        for p in prims.iter().filter(|p| p.lod == lod && p.index_count > 0) {
            match ranges.iter_mut().find(|(s, c, _)| *s == p.index_start && *c == p.index_count) {
                Some((_, _, mats)) => {
                    if !mats.contains(&p.material_hash) {
                        mats.push(p.material_hash);
                    }
                }
                None => ranges.push((p.index_start, p.index_count, vec![p.material_hash])),
            }
        }
        // A leaf strictly contains no other distinct range.
        let contains = |a: &(u32, u32, Vec<u32>), b: &(u32, u32, Vec<u32>)| {
            let (ae, be) = (a.0 + a.1, b.0 + b.1);
            a.0 <= b.0 && ae >= be && (a.0 != b.0 || a.1 != b.1)
        };
        let mut leaves: Vec<SubMesh> = ranges
            .iter()
            .filter(|r| !ranges.iter().any(|o| contains(r, o)))
            .map(|(s, c, m)| SubMesh { index_start: *s, index_count: *c, materials: m.clone(), lod })
            .collect();
        leaves.sort_by_key(|s| s.index_start);
        let span_start = ranges.iter().map(|r| r.0).min().unwrap_or(0);
        let span_end = ranges.iter().map(|r| r.0 + r.1).max().unwrap_or(0);
        // Verify the leaves tile this level's span exactly.
        let mut pos = span_start;
        let tiles = leaves.iter().all(|s| {
            let ok = s.index_start == pos;
            pos = s.index_start + s.index_count;
            ok
        }) && pos == span_end;
        if tiles && !leaves.is_empty() {
            out.extend(leaves);
        } else {
            out.push(SubMesh {
                index_start: span_start,
                index_count: span_end - span_start,
                materials: Vec::new(),
                lod,
            });
        }
    }
    // The levels themselves must tile the buffer: no gap, no overlap between levels.
    out.sort_by_key(|s| s.index_start);
    let mut pos = 0u32;
    let tiles = out.iter().all(|s| {
        let ok = s.index_start == pos;
        pos = s.index_start + s.index_count;
        ok
    }) && pos == total_indices;
    if tiles {
        return out;
    }
    // Not tiled: one untextured submesh per level, each level running from its first index up to
    // the next level's first index (the first from 0, the last to the end). Levels are laid out in
    // order, so this covers the buffer once; a single whole-buffer submesh would stack them.
    let mut starts: Vec<(u32, u8)> = Vec::new();
    for s in &out {
        match starts.iter_mut().find(|(_, l)| *l == s.lod) {
            Some((st, _)) => *st = (*st).min(s.index_start),
            None => starts.push((s.index_start, s.lod)),
        }
    }
    starts.sort_unstable();
    if starts.len() <= 1 {
        return whole();
    }
    starts[0].0 = 0;
    (0..starts.len())
        .map(|k| {
            let end = starts.get(k + 1).map(|n| n.0).unwrap_or(total_indices);
            SubMesh {
                index_start: starts[k].0,
                index_count: end - starts[k].0,
                materials: Vec::new(),
                lod: starts[k].1,
            }
        })
        .collect()
}

/// Rigidly bind UNSKINNED geometry to its prim's `parent_bone`.
///
/// Accessories (hats, props) ship with NO skin weights and are authored around the origin; the engine
/// parents them to `DrawCall.parentBone`. Left alone they render at the world origin — Sean's hat on
/// the floor between his feet. Rather than special-case them in the shader, give each weightless
/// vertex one full-weight influence on its prim's parent bone, so the ordinary skinning path
/// (`jointMatrix[parent] * pos`) places AND animates it for free. Needs SMSH v2 (v1 has no field).
pub fn bind_rigid_attachments(m: &mut Smsh, bind_world: &[glam::Mat4]) {
    let mut done = vec![false; m.positions.len()];
    for prim in &m.prims {
        let pb = prim.parent_bone as usize;
        let Some(bw) = bind_world.get(pb) else { continue };
        let start = prim.index_start as usize;
        let end = (start + prim.index_count as usize).min(m.indices.len());
        for &idx in &m.indices[start..end] {
            let v = idx as usize;
            if v >= m.weights.len() || done[v] {
                continue;
            }
            let w = m.weights[v];
            if w[0] + w[1] + w[2] + w[3] > 0.0001 {
                continue; // genuinely skinned — leave it be
            }
            done[v] = true;
            // The vertex is in the BONE'S LOCAL space; skinning expects BIND space. Lift it:
            //   jointMatrix[pb] * (bindWorld[pb] * p) == world[pb] * inv_bind[pb] * bindWorld[pb] * p
            //                                         == world[pb] * p          (inv_bind = bindWorld^-1)
            // i.e. exactly "rigidly parented to that bone", posed correctly for free.
            let p = glam::Vec3::from_array(m.positions[v]);
            m.positions[v] = bw.transform_point3(p).to_array();
            let n = glam::Vec3::from_array(m.normals[v]);
            m.normals[v] = bw.transform_vector3(n).normalize_or_zero().to_array();
            m.joints[v] = [prim.parent_bone, 0, 0, 0];
            m.weights[v] = [1.0, 0.0, 0.0, 0.0];
        }
    }
}

pub fn read_smsh(b: &[u8]) -> Result<Smsh, String> {
    if b.len() < 20 || &b[0..4] != b"SMSH" {
        return Err("not an SMSH blob".into());
    }
    let rd_u32 = |o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
    let rd_f32 = |o: usize| f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
    let rd_u16 = |o: usize| u16::from_le_bytes([b[o], b[o + 1]]);
    let nv = rd_u32(8) as usize;
    let ni = rd_u32(12) as usize;
    let version = rd_u32(4);
    let np = rd_u32(16) as usize; // num prims (0 for a pre-prims SMSH)
    // v1 prims are 16 B {start,count,material,flags}; v2 appends u16 parentBone (+u16 pad) = 20 B.
    let prim_stride = if version >= 2 { 20 } else { 16 };
    // Bounds check: header claims nv verts + ni indices in the fixed-stride layout below, then
    // np * 16-byte prim records at the tail.
    let need = 20 + nv * (12 + 12 + 8 + 8 + 16) + ni * 4 + np * prim_stride;
    if b.len() < need {
        return Err(format!("SMSH truncated: need {need} bytes, have {}", b.len()));
    }
    let mut p = 20usize;
    let mut positions = Vec::with_capacity(nv);
    for i in 0..nv { let o = p + i * 12; positions.push([rd_f32(o), rd_f32(o + 4), rd_f32(o + 8)]); }
    p += nv * 12;
    let mut normals = Vec::with_capacity(nv);
    for i in 0..nv { let o = p + i * 12; normals.push([rd_f32(o), rd_f32(o + 4), rd_f32(o + 8)]); }
    p += nv * 12;
    let mut uvs = Vec::with_capacity(nv);
    for i in 0..nv { let o = p + i * 8; uvs.push([rd_f32(o), rd_f32(o + 4)]); }
    p += nv * 8;
    let mut joints = Vec::with_capacity(nv);
    for i in 0..nv { let o = p + i * 8; joints.push([rd_u16(o), rd_u16(o + 2), rd_u16(o + 4), rd_u16(o + 6)]); }
    p += nv * 8;
    let mut weights = Vec::with_capacity(nv);
    for i in 0..nv { let o = p + i * 16; weights.push([rd_f32(o), rd_f32(o + 4), rd_f32(o + 8), rd_f32(o + 12)]); }
    p += nv * 16;
    let mut indices = Vec::with_capacity(ni);
    for i in 0..ni { indices.push(rd_u32(p + i * 4)); }
    p += ni * 4;
    let mut prims = Vec::with_capacity(np);
    for i in 0..np {
        let o = p + i * prim_stride;
        prims.push(Prim {
            index_start: rd_u32(o),
            index_count: rd_u32(o + 4),
            material_hash: rd_u32(o + 8),
            flags: rd_u32(o + 12),
            parent_bone: if version >= 2 { rd_u16(o + 16) } else { 0 },
            // SMSH has no LOD field: a file holds one mesh's drawn geometry.
            lod: 0,
        });
    }
    // NOTE: rigid attachments need the skeleton (bind world), so callers must run
    // `bind_rigid_attachments` once they have it.
    Ok(Smsh { positions, normals, uvs, joints, weights, indices, prims })
}

/// Read a whitespace `.skel` file: one bone per line
/// `parent name tx ty tz rx ry rz rw sx sy sz [invbind m0..m15 (row-major)]`.
pub fn read_skel(text: &str) -> Vec<Bone> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 12 {
            continue;
        }
        let p = |i: usize| f[i].parse::<f32>().unwrap_or(0.0);
        let inv_bind = if f.len() >= 28 {
            let mut m = [0.0f32; 16];
            for (k, slot) in m.iter_mut().enumerate() {
                *slot = p(12 + k);
            }
            Some(m)
        } else {
            None
        };
        out.push(Bone {
            local_m: None,
            parent: f[0].parse().unwrap_or(-1),
            name: f[1].to_string(),
            t: [p(2), p(3), p(4)],
            r: [p(5), p(6), p(7), p(8)],
            s: [p(9), p(10), p(11)],
            inv_bind,
        });
    }
    out
}
