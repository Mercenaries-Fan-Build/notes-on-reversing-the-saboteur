//! The 2008 prototype's mesh body (`MSH4` and its sibling revisions), one per [`super::slot_dir`]
//! record of a `meshes` slot. Big-endian throughout, vertex and index buffers included. Layout and
//! evidence: `docs/2008_msh4_format.md`. The engine reader is `ReadMesh` (`WildStar_p.exe`
//! @0x8236def8; the same field sequence in `WildStar_d.exe` @0x82658168).
//!
//! ```text
//! +0      char    magic[4]            one of MAGICS
//! +4      u32     name_len            includes the NUL; at most 0x400
//!         char    name[name_len]
//!         u32     stream_count        fields follow the name directly, unaligned
//!         u32     occluder_count      0 in every body of the build
//!         u32     shadow_count        0 in every body of the build
//!         u32     reserved            read and discarded
//!         u32     shadow_caster       WSModel::IsShadowCaster
//!         Stream  streams[stream_count]
//!         u32     bone_count          ('CLOT' marks a cloth mesh; none in the build)
//!         Bone    bones[bone_count]   77 bytes each
//!         u32     remap_count
//!         Remap   remaps[remap_count] 68 bytes each
//!         f32     centre[3], half[3], radius
//!         tail                        not read by the engine: "MSHC" + u32 (8 bytes), a
//!                                     little-endian 4x4 f32 matrix (64 bytes), or nothing
//!
//! Stream:  u32 format, index_count, vertex_count, bone, stride, prim_count
//!          u8  vertices[vertex_count * stride]     laid out by Layout::of(format)
//!          u16 indices[index_count]
//!          Prim prims[prim_count]
//! Prim:    u32 strip, triangle_count, material, lod, segment_bits, index_start, index_count
//! Bone:    u32 hash; i32 parent; f32 translation[3], rotation[4], scale[3], centre[3], half[4];
//!          u8 export_flags
//! Remap:   u32 bone; f32 matrix[16]
//! ```
//!
//! A variant record (every record that does not start with a mesh magic) is a material binding
//! table, zero bytes to the next 0x800 boundary, then a mesh body ([`Record::Variant`]).

use super::cursor::Cursor;
use crate::bytes;

/// The mesh magics, in on-disk byte order. `ReadMesh` in `WildStar_p.exe` loads only `MSH4` and
/// rejects the rest as "Old Exported Mesh Version"; every body of the other revisions in the build
/// has the `MSH4` layout.
pub const MAGICS: [&[u8; 4]; 7] = [b"MSH0", b"MSH4", b"MSH5", b"MSH6", b"MSH8", b"MSHL", b"MSHR"];
/// `bone_count`'s value for a cloth mesh, which `ReadMesh` does not load.
pub const CLOTH: u32 = 0x434C_4F54;
/// The 8-byte tail's tag.
pub const TAIL_TAG: &[u8; 4] = b"MSHC";
/// Alignment of the mesh body after a variant record's binding table.
pub const VARIANT_ALIGN: usize = 0x800;

/// One draw range of a stream; one engine `OdinSegment`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Prim {
    /// Non-zero selects a triangle strip (`D3DPT 6`) over a list (`4`); 0 in every prim of the build.
    pub strip: u32,
    pub triangle_count: u32,
    /// The material's hash (`pandemic_hash` of its `WSAO` name), looked up by `OdinMaterialManager`.
    pub material: u32,
    /// The segment's LOD level (`OdinSegment::GetLodLevel` = flags & 7).
    pub lod: u32,
    /// Stored at segment flag bits 4..7; no engine code reads them.
    pub segment_bits: u32,
    pub index_start: u32,
    pub index_count: u32,
}

/// One vertex/index buffer pair and its draw ranges.
#[derive(Clone, Debug, PartialEq)]
pub struct Stream {
    pub format: u32,
    /// Index of the bone the stream's segments belong to.
    pub bone: u32,
    pub stride: u32,
    pub vertex_count: u32,
    /// `vertex_count * stride` bytes laid out by [`Layout::of`].
    pub vertices: Vec<u8>,
    pub indices: Vec<u16>,
    pub prims: Vec<Prim>,
}

/// One bone.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bone {
    pub hash: u32,
    /// Parent bone index, -1 for a root.
    pub parent: i32,
    pub translation: [f32; 3],
    /// Quaternion `[x, y, z, w]`.
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
    /// `PclAABB` centre.
    pub centre: [f32; 3],
    /// `PclAABB` half-extent; the fourth lane is stored with it.
    pub half: [f32; 4],
    /// `PclBone::GetExportFlags`, read by `WSHavokModel::Setup` to match bones to physics bodies.
    pub export_flags: u8,
}

/// A skinning remap: a bone index and its stored matrix.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Remap {
    pub bone: u32,
    pub matrix: [f32; 16],
}

/// The model's bounding volume (`WSModel` +0x20).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    pub centre: [f32; 3],
    pub half: [f32; 3],
    pub radius: f32,
}

/// The bytes after the bounds, which the engine does not read.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Tail {
    /// `"MSHC"` and a hash.
    Hash(u32),
    /// A little-endian 4x4 matrix, row-vector convention: `world = [p, 1] * M`.
    Matrix([f32; 16]),
    /// Nothing.
    None,
}

/// A parsed body.
#[derive(Clone, Debug, PartialEq)]
pub struct Mesh {
    pub magic: [u8; 4],
    pub name: String,
    pub reserved: u32,
    pub shadow_caster: bool,
    pub streams: Vec<Stream>,
    pub bones: Vec<Bone>,
    pub remaps: Vec<Remap>,
    pub bounds: Bounds,
    pub tail: Tail,
}

/// One entry of a variant record's binding table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Binding {
    /// A material UID of the pack's `materials` slot.
    pub material: u32,
    pub key: u32,
    pub pairs: Vec<(u32, u32)>,
}

/// A `meshes` slot record.
#[derive(Clone, Debug, PartialEq)]
pub enum Record {
    Mesh(Mesh),
    Variant { bindings: Vec<Binding>, mesh: Mesh },
}

impl Record {
    pub fn mesh(&self) -> &Mesh {
        match self {
            Record::Mesh(m) | Record::Variant { mesh: m, .. } => m,
        }
    }
}

/// Parse a record: a mesh body, or a variant record's table, padding and body.
pub fn parse_record(b: &[u8]) -> Result<Record, String> {
    if MAGICS.iter().any(|m| b.starts_with(&m[..])) {
        return parse(b).map(Record::Mesh);
    }
    let mut c = Cursor { b, at: 0 };
    let count = c.u32("binding count")? as usize;
    let mut bindings = Vec::with_capacity(count.min(1 << 12));
    for _ in 0..count {
        let material = c.u32("binding material")?;
        let key = c.u32("binding key")?;
        let n = c.u32("binding pair count")? as usize;
        let mut pairs = Vec::with_capacity(n.min(1 << 12));
        for _ in 0..n {
            pairs.push((c.u32("binding pair")?, c.u32("binding pair")?));
        }
        bindings.push(Binding { material, key, pairs });
    }
    let body = c.at.div_ceil(VARIANT_ALIGN).max(1) * VARIANT_ALIGN;
    let pad = bytes::slice(b, c.at, body.saturating_sub(c.at), "variant padding")?;
    if pad.iter().any(|&x| x != 0) {
        return Err(format!("variant record: non-zero byte between the table end 0x{:X} and 0x{body:X}", c.at));
    }
    let mesh = parse(bytes::slice(b, body, b.len().saturating_sub(body), "variant mesh")?)?;
    Ok(Record::Variant { bindings, mesh })
}

/// Parse a whole body; every byte must be accounted for.
pub fn parse(b: &[u8]) -> Result<Mesh, String> {
    let magic: [u8; 4] = bytes::slice(b, 0, 4, "mesh magic")?.try_into().expect("4 bytes");
    if !MAGICS.iter().any(|m| **m == magic) {
        return Err(format!("mesh: magic {magic:02X?} is not one of {MAGICS:?}"));
    }
    let name_len = bytes::u32_be(b, 4, "mesh name_len")? as usize;
    if name_len > 0x400 {
        return Err(format!("mesh: name_len {name_len} exceeds ReadMesh's 0x400"));
    }
    let raw = bytes::slice(b, 8, name_len, "mesh name")?;
    let name = match raw.split_last() {
        Some((0, s)) if !s.contains(&0) => String::from_utf8_lossy(s).into_owned(),
        _ => return Err(format!("mesh: name of {name_len} bytes is not NUL-terminated text")),
    };
    let mut c = Cursor { b, at: 8 + name_len };
    let stream_count = c.u32("mesh stream_count")? as usize;
    let occluder_count = c.u32("mesh occluder_count")?;
    let shadow_count = c.u32("mesh shadow_count")?;
    let reserved = c.u32("mesh reserved")?;
    let shadow_caster = c.u32("mesh shadow_caster")? != 0;
    if occluder_count != 0 || shadow_count != 0 {
        return Err(format!(
            "mesh '{name}': {occluder_count} occluders and {shadow_count} shadow volumes; no body of the build has \
             either, so this reader does not decode them"
        ));
    }
    let mut streams = Vec::with_capacity(stream_count.min(1024));
    for si in 0..stream_count {
        let format = c.u32("stream format")?;
        let index_count = c.u32("stream index_count")? as usize;
        let vertex_count = c.u32("stream vertex_count")?;
        let bone = c.u32("stream bone")?;
        let stride = c.u32("stream stride")?;
        let prim_count = c.u32("stream prim_count")? as usize;
        let layout = Layout::of(format).map_err(|e| format!("mesh '{name}' stream {si}: {e}"))?;
        if layout.stride != stride {
            return Err(format!(
                "mesh '{name}' stream {si}: format {format:08X} declares a {}-byte vertex, the stream stores {stride}",
                layout.stride
            ));
        }
        let vb_len = (vertex_count as usize)
            .checked_mul(stride as usize)
            .ok_or_else(|| format!("stream {si}: {vertex_count} x {stride} overflows"))?;
        let vertices = c.take(vb_len, &format!("stream {si} vertices"))?.to_vec();
        let ib = c.take(index_count * 2, &format!("stream {si} indices"))?;
        let indices = ib.chunks_exact(2).map(|w| u16::from_be_bytes([w[0], w[1]])).collect();
        let mut prims = Vec::with_capacity(prim_count.min(10_000));
        for _ in 0..prim_count {
            prims.push(Prim {
                strip: c.u32("prim strip")?,
                triangle_count: c.u32("prim triangle_count")?,
                material: c.u32("prim material")?,
                lod: c.u32("prim lod")?,
                segment_bits: c.u32("prim segment_bits")?,
                index_start: c.u32("prim index_start")?,
                index_count: c.u32("prim index_count")?,
            });
        }
        streams.push(Stream { format, bone, stride, vertex_count, vertices, indices, prims });
    }
    let bone_count = c.u32("mesh bone_count")?;
    if bone_count == CLOTH {
        return Err(format!("mesh '{name}': a cloth mesh ('CLOT'); ReadMesh does not load these and the build has none"));
    }
    let mut bones = Vec::with_capacity((bone_count as usize).min(1 << 16));
    for _ in 0..bone_count {
        let hash = c.u32("bone hash")?;
        let parent = c.u32("bone parent")? as i32;
        let translation = c.f32s("bone translation")?;
        let rotation = c.f32s("bone rotation")?;
        let scale = c.f32s("bone scale")?;
        let centre = c.f32s("bone centre")?;
        let half = c.f32s("bone half")?;
        let export_flags = c.take(1, "bone export_flags")?[0];
        bones.push(Bone { hash, parent, translation, rotation, scale, centre, half, export_flags });
    }
    let remap_count = c.u32("mesh remap_count")? as usize;
    let mut remaps = Vec::with_capacity(remap_count.min(1 << 16));
    for _ in 0..remap_count {
        remaps.push(Remap { bone: c.u32("remap bone")?, matrix: c.f32s("remap matrix")? });
    }
    let bounds = Bounds { centre: c.f32s("bounds centre")?, half: c.f32s("bounds half")?, radius: c.f32("bounds radius")? };
    let rest = &b[c.at..];
    let tail = match rest.len() {
        0 => Tail::None,
        8 if &rest[..4] == TAIL_TAG => Tail::Hash(u32::from_be_bytes(rest[4..8].try_into().expect("4 bytes"))),
        64 => {
            let mut m = [0f32; 16];
            for (k, x) in m.iter_mut().enumerate() {
                *x = bytes::f32_le(rest, 4 * k, "tail matrix")?;
            }
            Tail::Matrix(m)
        }
        n => return Err(format!("mesh '{name}': {n} bytes after the bounds are none of the three tails")),
    };
    let mesh = Mesh { magic, name, reserved, shadow_caster, streams, bones, remaps, bounds, tail };
    check(&mesh)?;
    Ok(mesh)
}

/// Reference invariants every body of the build satisfies.
fn check(m: &Mesh) -> Result<(), String> {
    for (si, s) in m.streams.iter().enumerate() {
        if !m.bones.is_empty() && s.bone as usize >= m.bones.len() {
            return Err(format!("mesh '{}' stream {si}: bone {} of {}", m.name, s.bone, m.bones.len()));
        }
        for (pi, p) in s.prims.iter().enumerate() {
            if p.strip != 0 {
                return Err(format!("mesh '{}' stream {si} prim {pi}: a triangle strip; the build has none", m.name));
            }
            let end = p.index_start as usize + p.index_count as usize;
            if p.index_count == 0 || end > s.indices.len() || p.index_count != 3 * p.triangle_count {
                return Err(format!(
                    "mesh '{}' stream {si} prim {pi}: indices [{}, {end}) of {}, {} triangles",
                    m.name,
                    p.index_start,
                    s.indices.len(),
                    p.triangle_count
                ));
            }
        }
        if let Some(&i) = s.indices.iter().find(|&&i| i as u32 >= s.vertex_count) {
            return Err(format!("mesh '{}' stream {si}: index {i} of {} vertices", m.name, s.vertex_count));
        }
    }
    for (bi, b) in m.bones.iter().enumerate() {
        if b.parent < -1 || b.parent >= bi as i32 {
            return Err(format!("mesh '{}' bone {bi}: parent {}", m.name, b.parent));
        }
    }
    Ok(())
}

// ------------------------------------------------------------------ vertex layout

/// A vertex element's storage, as `XenonSharedBuffers::FindDecl` declares it (`D3DDECLTYPE_*` from the
/// `WildStar_p.pdb` enum).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Element {
    /// `FLOAT16_4` (0x1A2360), 8 bytes.
    Half4,
    /// `FLOAT16_2` (0x2C235F), 4 bytes.
    Half2,
    /// `FLOAT2` (0x2C23A5), 8 bytes.
    Float2,
    /// `FLOAT3` (0x2A23B9), 12 bytes.
    Float3,
    /// `FLOAT4` (0x1A23A6), 16 bytes.
    Float4,
    /// `USHORT2` (0x2C2259), 4 bytes.
    UShort2,
    /// `UBYTE4N` (0x1A2086), 4 bytes.
    UByte4N,
    /// `UBYTE4` (0x1A2286), 4 bytes.
    UByte4,
    /// `DEC3N` (0x2A2187), 4 bytes.
    Dec3N,
}

impl Element {
    pub fn size(self) -> u32 {
        match self {
            Element::Half2 | Element::UShort2 | Element::UByte4N | Element::UByte4 | Element::Dec3N => 4,
            Element::Half4 | Element::Float2 => 8,
            Element::Float3 => 12,
            Element::Float4 => 16,
        }
    }
}

/// A vertex's elements in declaration order, with byte offsets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    pub position: Option<(Element, u32)>,
    /// `BLENDWEIGHT` then `BLENDINDICES`.
    pub skin: Option<(u32, u32)>,
    pub colors: Vec<u32>,
    pub uvs: Vec<(Element, u32)>,
    pub normal: Option<(Element, u32)>,
    pub tangent: Option<(Element, u32)>,
    pub binormal: Option<(Element, u32)>,
    pub stride: u32,
}

impl Layout {
    /// `FindDecl`'s element list for a stream format (`WildStar_p.exe` @0x829a7f10, `WildStar_d.exe`
    /// @0x83157798). `ReadMesh` passes a zero texture-coordinate type word, so every coordinate set is
    /// type 0. Format bits `FindDecl` does not test (bit 29 on baked tiles among them) add no element.
    pub fn of(format: u32) -> Result<Layout, String> {
        let bit = |b: u32| format & (1 << b) != 0;
        let mut at = 0u32;
        let mut put = |e: Element| {
            let o = at;
            at += e.size();
            (e, o)
        };
        let position = match format & 3 {
            0 => None,
            1 if bit(31) => Some(put(Element::UShort2)),
            1 => Some(put(Element::Float2)),
            2 if bit(24) => Some(put(Element::Half4)),
            2 if bit(31) => Some(put(Element::UShort2)),
            2 => Some(put(Element::Float3)),
            _ => Some(put(Element::Float4)),
        };
        let skin = bit(2).then(|| (put(Element::UByte4N).1, put(Element::UByte4).1));
        let colors = [4, 5].into_iter().filter(|&b| bit(b)).map(|_| put(Element::UByte4N).1).collect();
        let uv_element = if bit(25) { Element::Half2 } else { Element::Float2 };
        let uvs = (0..(format >> 8 & 0xF)).map(|_| put(uv_element)).collect();
        let (normal, tangent, binormal) = if bit(30) {
            (
                bit(12).then(|| put(Element::Float3)),
                bit(13).then(|| put(Element::Float3)),
                bit(14).then(|| put(Element::Float3)),
            )
        } else {
            (
                bit(12).then(|| put(if bit(26) { Element::Dec3N } else { Element::Float3 })),
                bit(13).then(|| put(Element::UByte4N)),
                bit(14).then(|| put(if bit(28) { Element::Dec3N } else { Element::Float3 })),
            )
        };
        Ok(Layout { position, skin, colors, uvs, normal, tangent, binormal, stride: at })
    }
}

// ------------------------------------------------------------------ model space

/// Terrain lattice spacing in metres (`8F000001` streams): 37.5 m chunks of 16 cells.
pub const TERRAIN_STEP: f32 = 37.5 / 16.0;
/// Terrain lattice width: 17 points per row.
pub const TERRAIN_ROW: u16 = 17;

fn rotate(q: [f32; 4], v: [f32; 3]) -> [f32; 3] {
    let (x, y, z, w) = (q[0], q[1], q[2], q[3]);
    let t = [2.0 * (y * v[2] - z * v[1]), 2.0 * (z * v[0] - x * v[2]), 2.0 * (x * v[1] - y * v[0])];
    [v[0] + w * t[0] + (y * t[2] - z * t[1]), v[1] + w * t[1] + (z * t[0] - x * t[2]), v[2] + w * t[2] + (x * t[1] - y * t[0])]
}

impl Mesh {
    /// A point in bone `bone`'s space carried to model space through the bind pose: each bone's
    /// `translation + rotation · (scale ⊙ p)`, from `bone` up to the root.
    pub fn bone_to_model(&self, bone: usize, p: [f32; 3]) -> [f32; 3] {
        let mut q = p;
        let mut k = bone as i32;
        while k >= 0 {
            let b = &self.bones[k as usize];
            let r = rotate(b.rotation, [q[0] * b.scale[0], q[1] * b.scale[1], q[2] * b.scale[2]]);
            q = [r[0] + b.translation[0], r[1] + b.translation[1], r[2] + b.translation[2]];
            k = b.parent;
        }
        q
    }

    /// A stream's vertex positions in model space: [`Mesh::stream_positions`], with bone-space streams
    /// carried by [`Mesh::bone_to_model`].
    pub fn positions(&self, s: &Stream) -> Result<Vec<[f32; 3]>, String> {
        let (space, ps) = self.stream_positions(s)?;
        Ok(match space {
            Space::Model => ps,
            Space::Bone(b) => ps.into_iter().map(|p| self.bone_to_model(b as usize, p)).collect(),
        })
    }

    /// A stream's vertex positions and the space they are in:
    ///
    /// * terrain (`USHORT2` position): `[height, lattice]`, lattice = `17·Zq + Xq`;
    ///   `x/z = origin + (Xq, Zq) · TERRAIN_STEP`, `y = box_min.y + height / 65535 · box_size.y`, with
    ///   `origin` the stream bone carried to model space — model space;
    /// * a `FLOAT16_4` stream without a normal: normalised, `box_min + q · box_size` of the stream bone's
    ///   box — model space;
    /// * a skinned stream, or a stream of a body without bones: as stored, model space;
    /// * any other stream: as stored, in the stream bone's space.
    ///
    /// Evidence for each rule: `docs/2008_msh4_format.md` ("Coordinate spaces").
    pub fn stream_positions(&self, s: &Stream) -> Result<(Space, Vec<[f32; 3]>), String> {
        let l = Layout::of(s.format)?;
        let ps = self.decode_positions(s, &l)?;
        let normalised = matches!(l.position, Some((Element::UShort2, _)))
            || (matches!(l.position, Some((Element::Half4, _))) && l.normal.is_none());
        let space = if normalised || l.skin.is_some() || self.bones.is_empty() { Space::Model } else { Space::Bone(s.bone) };
        Ok((space, ps))
    }

    /// A stream's normals in the space of its positions ([`Mesh::stream_positions`]), or `None` when
    /// the format has no normal.
    pub fn stream_normals(&self, s: &Stream) -> Result<Option<Vec<[f32; 3]>>, String> {
        let l = Layout::of(s.format)?;
        Ok(l.normal.map(|(e, o)| {
            s.vertices
                .chunks_exact(s.stride as usize)
                .map(|v| {
                    let n = floats(e, v, o);
                    [n[0], n[1], n[2]]
                })
                .collect()
        }))
    }

    /// Texture coordinate set `set` of a stream, or `None` when the format has fewer sets.
    pub fn stream_uvs(&self, s: &Stream, set: usize) -> Result<Option<Vec<[f32; 2]>>, String> {
        let l = Layout::of(s.format)?;
        Ok(l.uvs.get(set).map(|&(e, o)| {
            s.vertices
                .chunks_exact(s.stride as usize)
                .map(|v| {
                    let t = floats(e, v, o);
                    [t[0], t[1]]
                })
                .collect()
        }))
    }

    /// A skinned stream's influences: bone indices (through the remap palette) and weights in [0,1],
    /// x component first. `None` when the format is not skinned. An influence with weight 0 gets bone 0.
    pub fn stream_skin(&self, s: &Stream) -> Result<Option<Vec<([u32; 4], [f32; 4])>>, String> {
        let l = Layout::of(s.format)?;
        let Some((wo, io)) = l.skin else { return Ok(None) };
        let mut out = Vec::with_capacity(s.vertex_count as usize);
        for v in s.vertices.chunks_exact(s.stride as usize) {
            let w = ubyte4(v, wo);
            let i = ubyte4(v, io);
            let mut bones = [0u32; 4];
            let mut weights = [0f32; 4];
            for k in 0..4 {
                if w[k] == 0 {
                    continue;
                }
                let r = self.remaps.get(i[k] as usize).ok_or_else(|| {
                    format!("mesh '{}': skin index {} past {} remaps", self.name, i[k], self.remaps.len())
                })?;
                bones[k] = r.bone;
                weights[k] = w[k] as f32 / 255.0;
            }
            out.push((bones, weights));
        }
        Ok(Some(out))
    }

    fn decode_positions(&self, s: &Stream, l: &Layout) -> Result<Vec<[f32; 3]>, String> {
        let Some((e, o)) = l.position else { return Err(format!("mesh '{}': stream format {:08X} has no position", self.name, s.format)) };
        let bone = self.bones.get(s.bone as usize);
        let boxed = |b: &Bone| {
            let min = [b.centre[0] - b.half[0], b.centre[1] - b.half[1], b.centre[2] - b.half[2]];
            let size = [2.0 * b.half[0], 2.0 * b.half[1], 2.0 * b.half[2]];
            (min, size)
        };
        let verts = s.vertices.chunks_exact(s.stride as usize);
        match e {
            Element::UShort2 => {
                let b = bone.ok_or_else(|| format!("mesh '{}': a terrain stream needs its bone", self.name))?;
                let (min, size) = boxed(b);
                let origin = self.bone_to_model(s.bone as usize, [0.0; 3]);
                verts
                    .map(|v| {
                        let [h, lat] = ushort2(v, o);
                        let (zq, xq) = (lat / TERRAIN_ROW, lat % TERRAIN_ROW);
                        if zq >= TERRAIN_ROW {
                            return Err(format!("mesh '{}': terrain lattice index {lat} past 17x17", self.name));
                        }
                        Ok([
                            origin[0] + xq as f32 * TERRAIN_STEP,
                            min[1] + h as f32 / 65535.0 * size[1],
                            origin[2] + zq as f32 * TERRAIN_STEP,
                        ])
                    })
                    .collect()
            }
            Element::Half4 if l.normal.is_none() => {
                let b = bone.ok_or_else(|| format!("mesh '{}': a normalised stream needs its bone", self.name))?;
                let (min, size) = boxed(b);
                Ok(verts
                    .map(|v| {
                        let q = floats(e, v, o);
                        [min[0] + q[0] * size[0], min[1] + q[1] * size[1], min[2] + q[2] * size[2]]
                    })
                    .collect())
            }
            Element::Half4 | Element::Float2 | Element::Float3 | Element::Float4 => Ok(verts
                .map(|v| {
                    let p = floats(e, v, o);
                    [p[0], p[1], p[2]]
                })
                .collect()),
            other => Err(format!("mesh '{}': position element {other:?}", self.name)),
        }
    }
}

/// The space a stream's stored positions are in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Space {
    Model,
    /// The space of this bone, carried to model space through the bind pose.
    Bone(u32),
}

// ------------------------------------------------------------------ element decoding

/// An IEEE half from its big-endian bits.
pub fn half(x: u16) -> f32 {
    let sign = if x & 0x8000 != 0 { -1.0 } else { 1.0 };
    let e = ((x >> 10) & 0x1f) as i32;
    let m = (x & 0x3ff) as f32;
    match e {
        0 => sign * m * 2f32.powi(-24),
        31 => sign * if m == 0.0 { f32::INFINITY } else { f32::NAN },
        _ => sign * (1.0 + m / 1024.0) * 2f32.powi(e - 15),
    }
}

fn be16(v: &[u8], o: usize) -> u16 {
    u16::from_be_bytes([v[o], v[o + 1]])
}

fn be32(v: &[u8], o: usize) -> u32 {
    u32::from_be_bytes([v[o], v[o + 1], v[o + 2], v[o + 3]])
}

/// A float element's components (`w` = 1 for fewer than four), from one vertex's bytes.
pub fn floats(e: Element, v: &[u8], o: u32) -> [f32; 4] {
    let o = o as usize;
    let f = |k: usize| f32::from_bits(be32(v, o + 4 * k));
    match e {
        Element::Half4 => [half(be16(v, o)), half(be16(v, o + 2)), half(be16(v, o + 4)), half(be16(v, o + 6))],
        Element::Half2 => [half(be16(v, o)), half(be16(v, o + 2)), 0.0, 1.0],
        Element::Float2 => [f(0), f(1), 0.0, 1.0],
        Element::Float3 => [f(0), f(1), f(2), 1.0],
        Element::Float4 => [f(0), f(1), f(2), f(3)],
        Element::Dec3N => {
            let w = be32(v, o);
            let c = |s: u32| {
                let x = ((w >> s) & 0x3ff) as i32;
                let x = if x >= 512 { x - 1024 } else { x };
                (x as f32 / 511.0).max(-1.0)
            };
            [c(0), c(10), c(20), 1.0]
        }
        Element::UShort2 | Element::UByte4N | Element::UByte4 => {
            unreachable!("floats() called on integer element {e:?}")
        }
    }
}

/// A 4-byte integer element's bytes in stored order.
pub fn bytes4(v: &[u8], o: u32) -> [u8; 4] {
    let o = o as usize;
    [v[o], v[o + 1], v[o + 2], v[o + 3]]
}

/// A `UBYTE4` / `UBYTE4N` element's components x, y, z, w: the element is one big-endian u32 with x in
/// its low byte, so x is the last byte stored.
pub fn ubyte4(v: &[u8], o: u32) -> [u8; 4] {
    let b = bytes4(v, o);
    [b[3], b[2], b[1], b[0]]
}

/// A `USHORT2` element's two big-endian halves in stored order.
pub fn ushort2(v: &[u8], o: u32) -> [u16; 2] {
    [be16(v, o as usize), be16(v, o as usize + 2)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layouts_match_the_stored_strides() {
        for (format, stride) in [
            (0x0F00_0002, 8),
            (0x0E00_0002, 12),
            (0x0E00_1102, 20),
            (0x0F00_1106, 24),
            (0x0F00_3106, 28),
            (0x0F00_3206, 32),
            (0x0F00_3512, 40),
            (0x2F00_0102, 12),
            (0x2F00_2302, 24),
            (0x8F00_0001, 4),
        ] {
            assert_eq!(Layout::of(format).unwrap().stride, stride, "{format:08X}");
        }
        let l = Layout::of(0x0F00_3116).unwrap();
        assert_eq!(l.position, Some((Element::Half4, 0)));
        assert_eq!(l.skin, Some((8, 12)));
        assert_eq!(l.colors, vec![16]);
        assert_eq!(l.uvs, vec![(Element::Half2, 20)]);
        assert_eq!(l.normal, Some((Element::Dec3N, 24)));
        assert_eq!(l.tangent, Some((Element::UByte4N, 28)));
        assert_eq!(Layout::of(0x8F00_0001).unwrap().position, Some((Element::UShort2, 0)));
        assert_eq!(Layout::of(0x0E00_0002).unwrap().position, Some((Element::Float3, 0)));
    }

    #[test]
    fn decodes_halves_and_dec3n() {
        assert_eq!(half(0x3C00), 1.0);
        assert_eq!(half(0x3800), 0.5);
        assert_eq!(half(0xC000), -2.0);
        let v = [0x3C, 0x00, 0x38, 0x00, 0x00, 0x00, 0x3C, 0x00];
        assert_eq!(floats(Element::Half4, &v, 0), [1.0, 0.5, 0.0, 1.0]);
        let n = (511u32 << 20) | (1u32.wrapping_neg() & 0x3ff) << 10;
        assert_eq!(floats(Element::Dec3N, &n.to_be_bytes(), 0), [0.0, -1.0 / 511.0, 1.0, 1.0]);
    }

    fn bone(t: [f32; 3], r: [f32; 4], parent: i32, centre: [f32; 3], half: [f32; 3]) -> Bone {
        Bone { hash: 0, parent, translation: t, rotation: r, scale: [1.0; 3], centre, half: [half[0], half[1], half[2], 0.0], export_flags: 0 }
    }

    fn mesh_with(streams: Vec<Stream>, bones: Vec<Bone>) -> Mesh {
        Mesh {
            magic: *b"MSH4",
            name: "t".into(),
            reserved: 0,
            shadow_caster: false,
            streams,
            bones,
            remaps: vec![Remap { bone: 1, matrix: [0.0; 16] }],
            bounds: Bounds { centre: [0.0; 3], half: [1.0; 3], radius: 1.0 },
            tail: Tail::None,
        }
    }

    #[test]
    fn positions_follow_each_space_rule() {
        let id = [0.0, 0.0, 0.0, 1.0];
        let bones = vec![
            bone([10.0, 0.0, 0.0], id, -1, [10.0, 1.0, 0.0], [18.75, 1.0, 18.75]),
            bone([0.0, 2.0, 0.0], [0.0, 0.0, std::f32::consts::FRAC_1_SQRT_2, std::f32::consts::FRAC_1_SQRT_2], 0, [0.0; 3], [1.0; 3]),
        ];
        // Terrain: lattice 18 = (Xq 1, Zq 1), full height.
        let terrain = Stream {
            format: 0x8F00_0001,
            bone: 0,
            stride: 4,
            vertex_count: 1,
            vertices: [0xFFFFu16.to_be_bytes(), 18u16.to_be_bytes()].concat(),
            indices: vec![],
            prims: vec![],
        };
        // Normalised half4 (no normal): q = (0.5, 1, 0) in bone 0's box.
        let unit = Stream {
            format: 0x0F00_0002,
            bone: 0,
            stride: 8,
            vertex_count: 1,
            vertices: vec![0x38, 0, 0x3C, 0, 0, 0, 0x3C, 0],
            indices: vec![],
            prims: vec![],
        };
        // Rigid float3 + normal under bone 1 (90 degrees about z, 2 up, under bone 0 at x = 10).
        let mut rigid_v = Vec::new();
        for f in [1.0f32, 0.0, 0.0] {
            rigid_v.extend_from_slice(&f.to_be_bytes());
        }
        rigid_v.extend_from_slice(&(511u32 << 20).to_be_bytes());
        let rigid = Stream { format: 0x0E00_1002, bone: 1, stride: 16, vertex_count: 1, vertices: rigid_v, indices: vec![], prims: vec![] };
        let m = mesh_with(vec![terrain, unit, rigid], bones);
        let t = m.positions(&m.streams[0]).unwrap()[0];
        assert!((t[0] - (10.0 + TERRAIN_STEP)).abs() < 1e-4 && (t[1] - 2.0).abs() < 1e-4 && (t[2] - TERRAIN_STEP).abs() < 1e-4, "{t:?}");
        let u = m.positions(&m.streams[1]).unwrap()[0];
        assert!((u[0] - 10.0).abs() < 1e-4 && (u[1] - 2.0).abs() < 1e-4 && (u[2] + 18.75).abs() < 1e-4, "{u:?}");
        assert_eq!(m.stream_positions(&m.streams[2]).unwrap().0, Space::Bone(1));
        let r = m.positions(&m.streams[2]).unwrap()[0];
        assert!((r[0] - 10.0).abs() < 1e-4 && (r[1] - 3.0).abs() < 1e-4 && r[2].abs() < 1e-4, "{r:?}");
        assert_eq!(m.stream_normals(&m.streams[2]).unwrap(), Some(vec![[0.0, 0.0, 1.0]]));
        assert_eq!(ubyte4(&[1, 2, 3, 4], 0), [4, 3, 2, 1]);
    }

    fn body(magic: &[u8; 4], tail: &[u8]) -> Vec<u8> {
        let be = |w: u32| w.to_be_bytes();
        let mut b = magic.to_vec();
        b.extend_from_slice(&be(3));
        b.extend_from_slice(b"ab\0");
        for w in [1, 0, 0, 0, 1] {
            b.extend_from_slice(&be(w));
        }
        for w in [0x0F00_0002, 3, 3, 0, 8, 1] {
            b.extend_from_slice(&be(w));
        }
        for _ in 0..3 {
            b.extend_from_slice(&[0x3C, 0, 0x3C, 0, 0x3C, 0, 0x3C, 0]);
        }
        for i in [0u16, 1, 2] {
            b.extend_from_slice(&i.to_be_bytes());
        }
        for w in [0, 1, 0xABCD_1234, 2, 1, 0, 3] {
            b.extend_from_slice(&be(w));
        }
        b.extend_from_slice(&be(1));
        b.extend_from_slice(&be(0x1111_2222));
        b.extend_from_slice(&be(u32::MAX));
        for _ in 0..17 {
            b.extend_from_slice(&1f32.to_be_bytes());
        }
        b.push(4);
        b.extend_from_slice(&be(0));
        for _ in 0..7 {
            b.extend_from_slice(&2f32.to_be_bytes());
        }
        b.extend_from_slice(tail);
        b
    }

    #[test]
    fn parses_bodies_of_every_revision_and_variant_records() {
        let m = parse(&body(b"MSHL", b"MSHC\x12\x34\x56\x78")).unwrap();
        assert_eq!(m.name, "ab");
        assert!(m.shadow_caster);
        assert_eq!(m.streams[0].prims[0], Prim {
            strip: 0,
            triangle_count: 1,
            material: 0xABCD_1234,
            lod: 2,
            segment_bits: 1,
            index_start: 0,
            index_count: 3
        });
        assert_eq!(m.bones[0].export_flags, 4);
        assert_eq!(m.tail, Tail::Hash(0x1234_5678));
        assert!(parse(&body(b"MSH9", b"")).unwrap_err().contains("magic"));
        assert!(parse(&body(b"MSH4", b"xy")).unwrap_err().contains("tails"));

        let mut v = Vec::new();
        for w in [1u32, 0x45F3_5344, 0x67DB_AA59, 1, 0x4CDC_F012, 0x4249_7B0E] {
            v.extend_from_slice(&w.to_be_bytes());
        }
        v.resize(VARIANT_ALIGN, 0);
        v.extend_from_slice(&body(b"MSHR", b""));
        let Record::Variant { bindings, mesh } = parse_record(&v).unwrap() else { panic!("expected a variant") };
        assert_eq!(bindings, vec![Binding { material: 0x45F3_5344, key: 0x67DB_AA59, pairs: vec![(0x4CDC_F012, 0x4249_7B0E)] }]);
        assert_eq!(&mesh.magic, b"MSHR");
        v[30] = 1;
        assert!(parse_record(&v).unwrap_err().contains("non-zero"));
    }
}
