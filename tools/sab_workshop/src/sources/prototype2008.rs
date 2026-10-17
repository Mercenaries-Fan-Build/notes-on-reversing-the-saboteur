//! The 2008-05 Xbox 360 prototype (`Bin360/`) as a [`GameSource`]: the meshes, textures and
//! materials of every loose `SBL*` sub-pack, read through `sab_formats::proto2008`, and the loose
//! `.dds` files. Formats and evidence: `docs/2008_subpack_format.md`, `docs/2008_msh4_format.md`,
//! `docs/2008_texture_format.md`, `docs/2008_materials_format.md`.
//!
//! Opening indexes headers only, over memory-mapped packs: sub-pack directories, slot directories,
//! texture headers, mesh names (`mesh::record_name` on a `mesh::NAME_PREFIX` prefix) and the
//! `materials` slots. Every pack of the build parses (the `sab_formats` game gate), so a pack that
//! does not fails the open with its path: that copy of the build is damaged.
//!
//! `MP00` archives are not indexed: every entry is a byte copy of a loose file. `ANM0` clips and
//! `Animations.pack` (`AP04`) belong to the animation catalog, `prototype2008_anim::Catalog`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use glam::{Mat3, Mat4, Quat, Vec3};
use memmap2::Mmap;
use sab_formats::proto2008::material::{self, Material};
use sab_formats::proto2008::sbl::{self, SlotKind};
use sab_formats::proto2008::{mesh, slot_dir, texture};
use sab_formats::{dds, names, pandemic_hash, pixels, zlib};

use crate::dtex::CpuTexture;
use crate::formats::{Bone, Prim, Smsh};
use crate::source::{AssetHandle, AssetKind, GameSource, GameVersion, SourceFingerprint};
use crate::sources::prototype2008_anim::Catalog;

/// A texture handle's key: the texture hash for a slot texture; this bit plus an index for a loose
/// `.dds` file.
const LOOSE_DDS_KEY: u64 = 1 << 32;

/// One loose `SBL*` pack.
struct Pack {
    path: PathBuf,
    /// Path relative to `Bin360`, `/`-separated.
    rel: String,
    magic: [u8; 4],
    materials: Vec<Material>,
}

/// One `meshes` slot record.
struct MeshRef {
    pack: usize,
    /// The stored record's byte range in the pack file.
    offset: usize,
    stored: usize,
    unpacked: usize,
    name: String,
    hash: u32,
}

/// One `textures` slot record.
#[derive(Clone)]
struct TexRef {
    pack: usize,
    offset: usize,
    stored: usize,
    header: texture::Header,
    /// Slot record `+0x14`: `UnloadNormalMaps` may evict the texture.
    evictable: bool,
}

/// One loose `.dds` file.
struct LooseDds {
    name: String,
    rel: String,
    path: PathBuf,
}

/// What the engine knows about a texture that bears on its role (`docs/2008_materials_format.md`,
/// "Texture kinds per sampler").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextureKind {
    /// Sampled as sRGB.
    Colour,
    /// DXN, or evictable by `UnloadNormalMaps`: every such texture is normal-map-named.
    Normal,
    /// Any other linear texture — specular, mask, emissive or detail; the data does not say which.
    Linear,
}

impl TexRef {
    fn kind(&self) -> TextureKind {
        if self.header.format & !texture::GAMMA_SIGNS == texture::DXN || self.evictable {
            TextureKind::Normal
        } else if self.header.srgb() {
            TextureKind::Colour
        } else {
            TextureKind::Linear
        }
    }
}

/// A material's `Color-Pass` textures, as the workshop shows and binds them.
#[derive(Clone, Debug)]
pub struct MaterialView {
    pub uid: u32,
    pub name: String,
    /// The pack the material record is in.
    pub pack: String,
    /// `Color-Pass` samplers in order: texture hash and kind (`None` when no pack holds it).
    pub samplers: Vec<(u32, Option<TextureKind>)>,
    /// `Color-Pass` sampler 0.
    pub colour: Option<u32>,
    /// The first `Color-Pass` texture of kind [`TextureKind::Normal`].
    pub normal: Option<u32>,
}

/// A mesh record converted for the renderer: geometry in bind space with global bone indices, its
/// rig, and the materials its primitives name.
pub struct LoadedMesh {
    pub name: String,
    pub smsh: Smsh,
    pub rig: Vec<Bone>,
    pub materials: HashMap<u32, MaterialView>,
    /// Index ranges `(start, count)` of primitives whose stream has no texture coordinates.
    pub uvless: Vec<(u32, u32)>,
    /// The pack the record is in, for texture lookups that prefer it.
    pack: usize,
}

pub struct Prototype2008Source {
    bin360: PathBuf,
    fingerprint: SourceFingerprint,
    packs: Vec<Pack>,
    meshes: Vec<MeshRef>,
    /// Texture hash -> every record with that hash, in pack order.
    textures: HashMap<u32, Vec<TexRef>>,
    loose_dds: Vec<LooseDds>,
    tex_cache: Mutex<HashMap<u64, CpuTexture>>,
    animations: Catalog,
}

fn files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    for e in std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let p = e.map_err(|e| format!("{}: {e}", dir.display()))?.path();
        if p.is_dir() {
            files(&p, out)?;
        } else {
            out.push(p);
        }
    }
    Ok(())
}

fn has_ext(p: &Path, ext: &str) -> bool {
    p.extension().and_then(|x| x.to_str()).map(|x| x.eq_ignore_ascii_case(ext)) == Some(true)
}

fn map(path: &Path) -> Result<Mmap, String> {
    let f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    // SAFETY: the file is opened read-only and the build's files are not modified while mapped.
    unsafe { Mmap::map(&f) }.map_err(|e| format!("{}: mmap: {e}", path.display()))
}

impl Prototype2008Source {
    pub fn open(root: &Path) -> Result<Self, String> {
        let bin360 = sab_formats::game_paths::proto2008_bin360(root).ok_or_else(|| {
            format!("{} is not the 2008 build: it neither contains a `Bin360/` directory nor holds `Saboteur.xex` itself", root.display())
        })?;
        let mut fingerprint = SourceFingerprint::default();
        fingerprint.matched(format!("Bin360 root: {}", bin360.display()));
        let mut all = Vec::new();
        files(&bin360, &mut all)?;
        all.sort();
        let mut packs = Vec::new();
        let mut meshes = Vec::new();
        let mut textures: HashMap<u32, Vec<TexRef>> = HashMap::new();
        let mut loose_dds = Vec::new();
        let (mut empty, mut mega, mut anim_packs) = (0usize, 0usize, 0usize);
        for p in &all {
            let rel = p.strip_prefix(&bin360).expect("under Bin360").to_string_lossy().replace('\\', "/");
            if has_ext(p, "dds") {
                let name = p.file_stem().and_then(|s| s.to_str()).unwrap_or_default().to_string();
                loose_dds.push(LooseDds { name, rel, path: p.clone() });
                continue;
            }
            if !has_ext(p, "pack") {
                continue;
            }
            let m = map(p)?;
            if m.len() < 4 {
                empty += 1;
                continue;
            }
            match &m[..4] {
                b"MP00" => {
                    mega += 1;
                    continue;
                }
                b"AP04" => {
                    anim_packs += 1;
                    continue;
                }
                head if head.starts_with(b"SBL") => {}
                head => return Err(format!("{rel}: a .pack with magic {head:02X?}, which is none of SBL*, MP00, AP04")),
            }
            let damaged = |e: String| format!("{rel}: {e} — every pack of the 2008 build parses, so this copy is damaged");
            let sp = sbl::parse(&m).map_err(damaged)?;
            let pack = packs.len();
            let mats = match sp.slot(SlotKind::Materials) {
                Some(slot) => material::parse(slot).map_err(damaged)?,
                None => Vec::new(),
            };
            for (r, slot) in sp.records.iter().zip(&sp.slots) {
                match r.kind {
                    SlotKind::Textures => {
                        for t in slot_dir::parse(slot).map_err(damaged)?.into_iter().filter(|t| t.stored != 0) {
                            let rec = &slot[t.offset as usize..(t.offset + t.stored) as usize];
                            let header = texture::parse_header(&rec[..rec.len().min(texture::MAX_HEADER)]).map_err(damaged)?;
                            if pandemic_hash(&header.name) != t.hash {
                                return Err(damaged(format!("texture record {:08X} holds '{}'", t.hash, header.name)));
                            }
                            textures.entry(t.hash).or_default().push(TexRef {
                                pack,
                                offset: r.offset as usize + t.offset as usize,
                                stored: t.stored as usize,
                                header,
                                evictable: t.f14 != 0,
                            });
                        }
                    }
                    SlotKind::Meshes => {
                        for t in slot_dir::parse(slot).map_err(damaged)?.into_iter().filter(|t| t.stored != 0) {
                            let rec = &slot[t.offset as usize..(t.offset + t.stored) as usize];
                            let compressed = t.stored < t.unpacked;
                            let prefix = if compressed { &rec[..rec.len().min(mesh::NAME_PREFIX)] } else { rec };
                            let name = mesh::record_name(prefix, compressed).map_err(damaged)?;
                            meshes.push(MeshRef {
                                pack,
                                offset: r.offset as usize + t.offset as usize,
                                stored: t.stored as usize,
                                unpacked: t.unpacked as usize,
                                name,
                                hash: t.hash,
                            });
                        }
                    }
                    _ => {}
                }
            }
            packs.push(Pack { path: p.clone(), rel, magic: sp.magic, materials: mats });
        }
        let n_textures: usize = textures.values().map(Vec::len).sum();
        let n_materials: usize = packs.iter().map(|p| p.materials.len()).sum();
        fingerprint.matched(format!(
            "{} SBL packs: {} mesh records, {n_textures} texture records ({} distinct), {n_materials} materials",
            packs.len(),
            meshes.len(),
            textures.len()
        ));
        fingerprint.matched(format!("{} loose .dds files", loose_dds.len()));
        fingerprint.matched(format!(
            "not indexed: {mega} MP00 archives (byte copies of loose files), {anim_packs} AP04 animation pack, {empty} empty .pack files"
        ));
        let animations = Catalog::open(&bin360, &all)?;
        let (loose_clips, pack_records) = animations.counts();
        fingerprint.matched(format!("animations: {loose_clips} loose ANM0 clips, {pack_records} pack records"));
        Ok(Self {
            bin360,
            fingerprint,
            packs,
            meshes,
            textures,
            loose_dds,
            tex_cache: Mutex::new(HashMap::new()),
            animations,
        })
    }

    /// The pack (relative to `Bin360`) a mesh handle's record is in.
    pub fn mesh_pack(&self, handle: &AssetHandle) -> Result<&str, String> {
        Ok(&self.packs[self.mesh_ref(handle)?.pack].rel)
    }

    fn mesh_ref(&self, handle: &AssetHandle) -> Result<&MeshRef, String> {
        if handle.kind != AssetKind::Mesh {
            return Err(format!("handle {:?} '{}' is not a mesh", handle.kind, handle.name));
        }
        self.meshes.get(handle.key as usize).ok_or_else(|| format!("mesh handle {} out of range", handle.key))
    }

    /// The parsed record of a mesh handle.
    pub fn read_mesh_record(&self, handle: &AssetHandle) -> Result<mesh::Record, String> {
        let r = self.mesh_ref(handle)?;
        let pack = &self.packs[r.pack];
        let m = map(&pack.path)?;
        let stored = &m[r.offset..r.offset + r.stored];
        let body = if r.stored < r.unpacked { zlib::inflate_exact(stored, r.unpacked, &r.name)? } else { stored.to_vec() };
        mesh::parse_record(&body).map_err(|e| format!("{} in {}: {e}", r.name, pack.rel))
    }

    /// A texture record, preferring one in pack `prefer`.
    fn tex_ref(&self, hash: u32, prefer: Option<usize>) -> Option<&TexRef> {
        let refs = self.textures.get(&hash)?;
        prefer.and_then(|p| refs.iter().find(|t| t.pack == p)).or_else(|| refs.first())
    }

    fn material_view(&self, pack: usize, m: &Material) -> MaterialView {
        let colour_pass = m.passes.iter().find(|p| p.name == material::PASS_COLOR);
        let samplers: Vec<(u32, Option<TextureKind>)> = colour_pass
            .map(|p| p.sampler_textures(m).map(|h| (h, self.tex_ref(h, Some(pack)).map(TexRef::kind))).collect())
            .unwrap_or_default();
        MaterialView {
            uid: m.uid,
            name: m.name.clone(),
            pack: self.packs[pack].rel.clone(),
            colour: samplers.first().map(|s| s.0),
            normal: samplers.iter().find(|s| s.1 == Some(TextureKind::Normal)).map(|s| s.0),
            samplers,
        }
    }

    /// Load a mesh for the renderer: see [`convert`].
    pub fn load_mesh(&self, handle: &AssetHandle) -> Result<LoadedMesh, String> {
        let r = self.mesh_ref(handle)?;
        let record = self.read_mesh_record(handle)?;
        let m = record.mesh();
        let (smsh, rig, uvless) = convert(m)?;
        let pack = &self.packs[r.pack];
        let mut materials = HashMap::new();
        for p in &smsh.prims {
            if materials.contains_key(&p.material_hash) {
                continue;
            }
            let mat = pack.materials.iter().find(|x| x.uid == p.material_hash).ok_or_else(|| {
                format!("{} in {}: primitive material {:08X} is not in the pack's materials slot", r.name, pack.rel, p.material_hash)
            })?;
            materials.insert(p.material_hash, self.material_view(r.pack, mat));
        }
        Ok(LoadedMesh { name: m.name.clone(), smsh, rig, materials, uvless, pack: r.pack })
    }

    /// Everything a bundle export of a mesh handle writes, decoded: the stored record, the converted
    /// mesh and rig, each submesh's colour and normal textures, and a manifest of the record's pack,
    /// materials (every `Color-Pass` sampler with its texture and kind) and texture failures.
    pub fn bundle_job(&self, handle: &AssetHandle, outroot: PathBuf) -> Result<crate::bundle::Job2008, String> {
        use crate::resolve::SubmeshBinding;
        let r = self.mesh_ref(handle)?;
        let lm = self.load_mesh(handle)?;
        let raw = map(&self.packs[r.pack].path)?[r.offset..r.offset + r.stored].to_vec();
        let ni = lm.smsh.indices.len() as u32;
        let submeshes = crate::formats::submesh_cover(&lm.smsh.prims, ni);
        let mut textures: Vec<(String, CpuTexture)> = Vec::new();
        let mut index_of: HashMap<(u32, bool), usize> = HashMap::new();
        let mut errors: Vec<String> = Vec::new();
        let mut bindings = vec![SubmeshBinding::default(); submeshes.len()];
        let mut rows = Vec::new();
        for (i, sm) in submeshes.iter().enumerate() {
            let (s0, s1) = (sm.index_start, sm.index_start + sm.index_count);
            let textured = !lm.uvless.iter().any(|&(a, n)| a < s1 && s0 < a + n);
            let views: Vec<&MaterialView> = sm.materials.iter().filter_map(|u| lm.materials.get(u)).collect();
            if let (true, Some(v)) = (textured, views.first()) {
                for (hash, role, slot) in [(v.colour, TextureKind::Colour, SubmeshBinding::DIFFUSE), (v.normal, TextureKind::Normal, SubmeshBinding::NORMAL)] {
                    let Some(h) = hash else { continue };
                    let key = (h, role == TextureKind::Normal);
                    let idx = match index_of.get(&key) {
                        Some(&k) => Some(k),
                        None => match self.load_mesh_texture(&lm, h, role) {
                            Ok(t) => {
                                let name = if role == TextureKind::Normal { format!("{}_xyz", t.name) } else { t.name.clone() };
                                textures.push((name, t));
                                index_of.insert(key, textures.len() - 1);
                                Some(textures.len() - 1)
                            }
                            Err(e) => {
                                errors.push(format!("submesh {i} {role:?} {h:08X}: {e}"));
                                None
                            }
                        },
                    };
                    bindings[i].set(slot, idx);
                }
            }
            let materials: Vec<serde_json::Value> = views
                .iter()
                .map(|v| {
                    serde_json::json!({
                        "uid": format!("0x{:08X}", v.uid),
                        "name": v.name,
                        "pack": v.pack,
                        "color_pass_samplers": v.samplers.iter().map(|(h, k)| serde_json::json!({
                            "texture": self.name_of_hash(*h),
                            "hash": format!("0x{h:08X}"),
                            "kind": k.map(|k| format!("{k:?}")),
                        })).collect::<Vec<_>>(),
                    })
                })
                .collect();
            rows.push(serde_json::json!({
                "index": i, "lod": sm.lod, "index_start": sm.index_start, "index_count": sm.index_count,
                "has_texture_coordinates": textured, "materials": materials,
            }));
        }
        let record = self.read_mesh_record(handle)?;
        let m = record.mesh();
        let manifest = serde_json::json!({
            "build": "2008-05 Xbox 360 prototype",
            "record": {
                "pack": self.packs[r.pack].rel,
                "name": r.name,
                "hash": format!("0x{:08X}", r.hash),
                "magic": String::from_utf8_lossy(&m.magic),
                "stored": r.stored,
                "unpacked": r.unpacked,
            },
            "bones": m.bones.iter().enumerate().map(|(i, b)| serde_json::json!({
                "index": i,
                "name": lm.rig[i].name,
                "hash": format!("0x{:08X}", b.hash),
                "parent": b.parent,
                "has_stored_inverse_bind": m.remaps.iter().any(|x| x.bone as usize == i),
            })).collect::<Vec<_>>(),
            "submeshes": rows,
            "texture_errors": errors,
        });
        let raw_file = format!("{}.{}", r.name, if r.stored < r.unpacked { "zlib" } else { "variant" });
        Ok(crate::bundle::Job2008 {
            label: r.name.clone(),
            raw,
            raw_file,
            mesh: crate::meshload::LoadedMesh {
                name: lm.name.clone(),
                prim_parent_bone: lm.smsh.prims.iter().map(|p| p.parent_bone).collect(),
                bone_hashes: m.bones.iter().map(|b| b.hash).collect(),
                stored_ibm: HashMap::new(),
                part_ranges: Vec::new(),
                bones: lm.rig.clone(),
                mesh: lm.smsh,
            },
            submeshes,
            textures,
            submesh_tex: bindings,
            manifest,
            outroot,
        })
    }

    /// Decode a texture by hash for a mesh, preferring the mesh's own pack, prepared for its role.
    pub fn load_mesh_texture(&self, mesh: &LoadedMesh, hash: u32, role: TextureKind) -> Result<CpuTexture, String> {
        let t = self.tex_ref(hash, Some(mesh.pack)).ok_or_else(|| format!("no pack holds texture {hash:08X}"))?;
        let mut tex = self.decode_tex(t)?;
        if role == TextureKind::Normal {
            to_xyz_normal(&mut tex, t.header.format & !texture::GAMMA_SIGNS);
        }
        Ok(tex)
    }

    fn decode_tex(&self, t: &TexRef) -> Result<CpuTexture, String> {
        let m = map(&self.packs[t.pack].path)?;
        let x = texture::parse(&m[t.offset..t.offset + t.stored])?;
        let rgba = texture::decode_base(&x)?;
        Ok(CpuTexture { name: x.name, width: x.width as u32, height: x.height as u32, format: x.format, rgba })
    }

    fn decode_loose_dds(&self, i: usize) -> Result<CpuTexture, String> {
        let f = &self.loose_dds[i];
        let b = std::fs::read(&f.path).map_err(|e| format!("{}: {e}", f.path.display()))?;
        let d = dds::parse(&b).map_err(|e| format!("{}: {e}", f.rel))?;
        let rgba = pixels::decode(d.format, d.width as usize, d.height as usize, d.level(0, 0)?).map_err(|e| format!("{}: {e}", f.rel))?;
        Ok(CpuTexture { name: f.name.clone(), width: d.width, height: d.height, format: d.four_cc, rgba })
    }
}

/// Rebuild a normal map's texels as `(x, y, z)` in RGB for the renderer: DXN and X8R8G8B8 normal
/// maps carry the normal in R and G only (B is 0 in DXN; in X8R8G8B8 it varies and `|RGB|` is not
/// unit), so `z = sqrt(1 - x² - y²)`; DXT5 normal maps store a unit `|RGB|` and are used as stored.
/// Measured by `sab_probe proto2008-textures`; the shaders' own tangent frame is not established.
fn to_xyz_normal(t: &mut CpuTexture, format: u32) {
    if format != texture::DXN && format != texture::X8R8G8B8 {
        return;
    }
    for p in t.rgba.chunks_exact_mut(4) {
        let s = |v: u8| v as f32 / 127.5 - 1.0;
        let (x, y) = (s(p[0]), s(p[1]));
        let z = (1.0 - x * x - y * y).max(0.0).sqrt();
        p[2] = ((z + 1.0) * 127.5).round() as u8;
        p[3] = 255;
    }
}

fn row_major(m: Mat4) -> [f32; 16] {
    m.transpose().to_cols_array()
}

/// A mesh's rig and its geometry in bind space, for the renderer's `jointMatrix = world · inv_bind`.
///
/// * Rig: each bone's TRS; `inv_bind` is the bone's stored remap matrix (row-vector convention,
///   transposed here) when it has one, else the inverse of its bind world.
/// * Skinned streams: model space as stored, bone indices through the remap palette.
/// * Bone-space streams: a vertex `p` of bone `b` becomes `inverse(inv_bind[b]) · p` with full weight
///   on `b`, so `world[b] · inv_bind[b]` places it at `world[b] · p`.
/// * Other model-space streams (terrain, normalised boxes): weighted to their stream bone, carried so
///   the bind pose draws them where they are; with no bones, unweighted.
/// * Streams without normals get area-weighted face normals; streams without texture coordinates get
///   zeros and their primitives are listed in the returned ranges.
pub fn convert(m: &mesh::Mesh) -> Result<(Smsh, Vec<Bone>, Vec<(u32, u32)>), String> {
    let mut stored: HashMap<u32, [f32; 16]> = HashMap::new();
    for r in &m.remaps {
        if let Some(prev) = stored.insert(r.bone, r.matrix) {
            if prev != r.matrix {
                return Err(format!("mesh '{}': bone {} has two different remap matrices", m.name, r.bone));
            }
        }
    }
    let mut world: Vec<Mat4> = Vec::with_capacity(m.bones.len());
    for (i, b) in m.bones.iter().enumerate() {
        let local = Mat4::from_scale_rotation_translation(Vec3::from_array(b.scale), Quat::from_array(b.rotation), Vec3::from_array(b.translation));
        let w = match b.parent {
            -1 => local,
            p if p >= 0 && (p as usize) < i => world[p as usize] * local,
            p => return Err(format!("mesh '{}': bone {i} has parent {p}, not an earlier bone", m.name)),
        };
        world.push(w);
    }
    let inv_bind: Vec<Mat4> = (0..m.bones.len())
        .map(|i| match stored.get(&(i as u32)) {
            Some(rm) => Mat4::from_cols_array(rm),
            None => world[i].inverse(),
        })
        .collect();
    let rig: Vec<Bone> = m
        .bones
        .iter()
        .enumerate()
        .map(|(i, b)| Bone {
            parent: b.parent,
            name: names::name(b.hash).map(str::to_string).unwrap_or_else(|| format!("bone_{:08X}", b.hash)),
            t: b.translation,
            r: b.rotation,
            s: b.scale,
            inv_bind: Some(row_major(inv_bind[i])),
            local_m: None,
        })
        .collect();
    let mut out = Smsh { positions: Vec::new(), normals: Vec::new(), uvs: Vec::new(), joints: Vec::new(), weights: Vec::new(), indices: Vec::new(), prims: Vec::new() };
    let mut uvless = Vec::new();
    for (si, s) in m.streams.iter().enumerate() {
        let at = |e: String| format!("mesh '{}' stream {si}: {e}", m.name);
        let base = out.positions.len() as u32;
        let ibase = out.indices.len() as u32;
        let (space, mut ps) = m.stream_positions(s).map_err(at)?;
        let mut ns = m.stream_normals(s).map_err(at)?;
        let skin = m.stream_skin(s).map_err(at)?;
        let bone = |b: u32| -> Result<u16, String> {
            if (b as usize) < m.bones.len() {
                Ok(b as u16)
            } else {
                Err(at(format!("bone {b} past {} bones", m.bones.len())))
            }
        };
        let place = |to_bind: Mat4, ps: &mut Vec<[f32; 3]>, ns: &mut Option<Vec<[f32; 3]>>| {
            for p in ps.iter_mut() {
                *p = to_bind.transform_point3(Vec3::from_array(*p)).to_array();
            }
            if let Some(ns) = ns {
                let nm = Mat3::from_mat4(to_bind).inverse().transpose();
                for n in ns.iter_mut() {
                    *n = (nm * Vec3::from_array(*n)).normalize_or_zero().to_array();
                }
            }
        };
        let n = ps.len();
        let (joints, weights): (Vec<[u16; 4]>, Vec<[f32; 4]>) = match (skin, space) {
            (Some(sk), _) => {
                let mut j = Vec::with_capacity(n);
                let mut w = Vec::with_capacity(n);
                for (bs, ws) in sk {
                    j.push([bone(bs[0])?, bone(bs[1])?, bone(bs[2])?, bone(bs[3])?]);
                    w.push(ws);
                }
                (j, w)
            }
            (None, mesh::Space::Bone(b)) => {
                let b16 = bone(b)?;
                place(inv_bind[b as usize].inverse(), &mut ps, &mut ns);
                (vec![[b16, 0, 0, 0]; n], vec![[1.0, 0.0, 0.0, 0.0]; n])
            }
            (None, mesh::Space::Model) if m.bones.is_empty() => (vec![[0; 4]; n], vec![[0.0; 4]; n]),
            (None, mesh::Space::Model) => {
                let b16 = bone(s.bone)?;
                let b = s.bone as usize;
                place((world[b] * inv_bind[b]).inverse(), &mut ps, &mut ns);
                (vec![[b16, 0, 0, 0]; n], vec![[1.0, 0.0, 0.0, 0.0]; n])
            }
        };
        if s.indices.len() % 3 != 0 {
            return Err(at(format!("{} indices is not a triangle list", s.indices.len())));
        }
        if let Some(&i) = s.indices.iter().find(|&&i| i as usize >= n) {
            return Err(at(format!("index {i} past {n} vertices")));
        }
        let ns = match ns {
            Some(ns) => ns,
            None => face_normals(&ps, &s.indices),
        };
        let uvs = m.stream_uvs(s, 0).map_err(at)?;
        out.uvs.extend(uvs.clone().unwrap_or_else(|| vec![[0.0, 0.0]; n]));
        out.positions.extend(ps);
        out.normals.extend(ns);
        out.joints.extend(joints);
        out.weights.extend(weights);
        out.indices.extend(s.indices.iter().map(|&i| base + i as u32));
        for p in &s.prims {
            if p.index_start as usize + p.index_count as usize > s.indices.len() {
                return Err(at(format!("primitive {}+{} past {} indices", p.index_start, p.index_count, s.indices.len())));
            }
            let lod = u8::try_from(p.lod).map_err(|_| at(format!("LOD level {}", p.lod)))?;
            out.prims.push(Prim {
                index_start: ibase + p.index_start,
                index_count: p.index_count,
                material_hash: p.material,
                flags: 0,
                parent_bone: if m.bones.is_empty() { 0 } else { bone(s.bone)? },
                lod,
            });
            if uvs.is_none() {
                uvless.push((ibase + p.index_start, p.index_count));
            }
        }
    }
    Ok((out, rig, uvless))
}

/// Area-weighted vertex normals of a triangle list.
fn face_normals(ps: &[[f32; 3]], indices: &[u16]) -> Vec<[f32; 3]> {
    let mut acc = vec![Vec3::ZERO; ps.len()];
    for t in indices.chunks_exact(3) {
        let [a, b, c] = [t[0] as usize, t[1] as usize, t[2] as usize];
        let (pa, pb, pc) = (Vec3::from_array(ps[a]), Vec3::from_array(ps[b]), Vec3::from_array(ps[c]));
        let n = (pb - pa).cross(pc - pa);
        acc[a] += n;
        acc[b] += n;
        acc[c] += n;
    }
    acc.into_iter().map(|n| n.normalize_or_zero().to_array()).collect()
}

impl GameSource for Prototype2008Source {
    fn version(&self) -> GameVersion {
        GameVersion::Prototype2008Xbox360
    }

    fn label(&self) -> String {
        format!("Prototype 2008 Xbox 360 — {}", self.bin360.display())
    }

    fn fingerprint(&self) -> &SourceFingerprint {
        &self.fingerprint
    }

    fn list_meshes(&self) -> Vec<AssetHandle> {
        self.meshes
            .iter()
            .enumerate()
            .map(|(i, r)| AssetHandle { key: i as u64, name: r.name.clone(), kind: AssetKind::Mesh, hash: Some(r.hash) })
            .collect()
    }

    fn list_textures(&self) -> Vec<AssetHandle> {
        let mut out: Vec<AssetHandle> = self
            .textures
            .iter()
            .map(|(h, refs)| AssetHandle { key: *h as u64, name: refs[0].header.name.clone(), kind: AssetKind::Texture, hash: Some(*h) })
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out.extend(self.loose_dds.iter().enumerate().map(|(i, f)| AssetHandle {
            key: LOOSE_DDS_KEY | i as u64,
            name: f.rel.clone(),
            kind: AssetKind::Texture,
            hash: None,
        }));
        out
    }

    fn list_materials(&self) -> Vec<AssetHandle> {
        self.packs
            .iter()
            .enumerate()
            .flat_map(|(pi, p)| {
                p.materials.iter().enumerate().map(move |(mi, m)| AssetHandle {
                    key: ((pi as u64) << 32) | mi as u64,
                    name: m.name.clone(),
                    kind: AssetKind::Material,
                    hash: Some(m.uid),
                })
            })
            .collect()
    }

    fn list_animations(&self) -> Vec<AssetHandle> {
        self.animations.handles()
    }

    fn load_texture(&self, hash: u32) -> Result<CpuTexture, String> {
        if let Some(t) = self.tex_cache.lock().expect("texture cache").get(&(hash as u64)) {
            return Ok(t.clone());
        }
        let t = self.tex_ref(hash, None).ok_or_else(|| format!("no pack holds texture {hash:08X}"))?;
        let tex = self.decode_tex(t)?;
        self.tex_cache.lock().expect("texture cache").insert(hash as u64, tex.clone());
        Ok(tex)
    }

    fn load_texture_handle(&self, handle: &AssetHandle) -> Result<CpuTexture, String> {
        if handle.key & LOOSE_DDS_KEY == 0 {
            return self.load_texture(handle.key as u32);
        }
        if let Some(t) = self.tex_cache.lock().expect("texture cache").get(&handle.key) {
            return Ok(t.clone());
        }
        let tex = self.decode_loose_dds((handle.key & 0xFFFF_FFFF) as usize)?;
        self.tex_cache.lock().expect("texture cache").insert(handle.key, tex.clone());
        Ok(tex)
    }

    fn name_of_hash(&self, hash: u32) -> Option<String> {
        if let Some(refs) = self.textures.get(&hash) {
            return Some(refs[0].header.name.clone());
        }
        names::name(hash).map(str::to_string)
    }

    fn describe(&self, handle: &AssetHandle) -> Result<Vec<(String, String)>, String> {
        let mut kv: Vec<(String, String)> = Vec::new();
        match handle.kind {
            AssetKind::Mesh => {
                let r = self.mesh_ref(handle)?;
                let p = &self.packs[r.pack];
                kv.push(("pack".into(), format!("{} ({})", p.rel, String::from_utf8_lossy(&p.magic))));
                kv.push(("stored".into(), format!("{} bytes{}", r.stored, if r.stored < r.unpacked { format!(", zlib of {}", r.unpacked) } else { " raw (variant record)".into() })));
            }
            AssetKind::Texture if handle.key & LOOSE_DDS_KEY != 0 => {
                let f = self.loose_dds.get((handle.key & 0xFFFF_FFFF) as usize).ok_or("loose .dds handle out of range")?;
                kv.push(("file".into(), f.rel.clone()));
            }
            AssetKind::Texture => {
                let refs = self.textures.get(&(handle.key as u32)).ok_or("unknown texture handle")?;
                let t = &refs[0];
                kv.push(("format".into(), format!("0x{:08X}", t.header.format)));
                kv.push(("size".into(), format!("{}×{}, {} mips", t.header.width, t.header.height, t.header.mips)));
                kv.push(("kind".into(), format!("{:?}", t.kind())));
                kv.push(("in packs".into(), refs.len().to_string()));
                kv.push(("first pack".into(), self.packs[t.pack].rel.clone()));
            }
            AssetKind::Material => {
                let (pi, mi) = ((handle.key >> 32) as usize, (handle.key & 0xFFFF_FFFF) as usize);
                let m = self.packs.get(pi).and_then(|p| p.materials.get(mi)).ok_or("material handle out of range")?;
                let v = self.material_view(pi, m);
                kv.push(("pack".into(), v.pack.clone()));
                kv.push(("uid".into(), format!("{:08X}", m.uid)));
                let passes: Vec<String> = m
                    .passes
                    .iter()
                    .map(|p| match p.name {
                        material::PASS_COLOR => "Color".to_string(),
                        material::PASS_Z => "Z".into(),
                        material::PASS_CAST_SHADOW => "Cast-Shadow".into(),
                        material::PASS_RECEIVE_SHADOW => "Receive-Shadow".into(),
                        material::PASS_FOLIAGE => "Foliage".into(),
                        other => format!("{other:08X}"),
                    })
                    .collect();
                kv.push(("passes".into(), passes.join(", ")));
                for (i, (h, k)) in v.samplers.iter().enumerate() {
                    let name = self.name_of_hash(*h).unwrap_or_else(|| format!("{h:08X}"));
                    let kind = k.map(|k| format!("{k:?}")).unwrap_or_else(|| "in no pack".into());
                    kv.push((format!("sampler {i}"), format!("{name} ({kind})")));
                }
            }
            AssetKind::Animation => kv.extend(self.animations.describe(handle)?),
            other => kv.push(("kind".into(), format!("{other:?}"))),
        }
        Ok(kv)
    }

    fn load_animation_clip(&self, handle: &AssetHandle) -> Result<Box<dyn crate::clip::AnimClip>, String> {
        Ok(Box::new(self.animations.load(handle)?))
    }

    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sab_formats::proto2008::mesh::{Bounds, Mesh, Prim as MPrim, Remap, Stream, Tail};

    fn bone(parent: i32, t: [f32; 3]) -> mesh::Bone {
        mesh::Bone { hash: 1, parent, translation: t, rotation: [0.0, 0.0, 0.0, 1.0], scale: [1.0; 3], centre: [0.0; 3], half: [1.0; 4], export_flags: 0 }
    }

    /// One triangle in a bone-space stream (format 2: a float3 position only) of bone 1, which sits
    /// at x = 5 under the root.
    fn rigid_mesh(remap: Option<[f32; 16]>) -> Mesh {
        let format = 0x0000_0002;
        let l = mesh::Layout::of(format).unwrap();
        let mut vertices = Vec::new();
        for p in [[0.0f32, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]] {
            let mut v = vec![0u8; l.stride as usize];
            let (_, o) = l.position.unwrap();
            for (k, c) in p.iter().enumerate() {
                v[o as usize + 4 * k..o as usize + 4 * k + 4].copy_from_slice(&c.to_be_bytes());
            }
            vertices.extend(v);
        }
        Mesh {
            magic: *b"MSH4",
            name: "t".into(),
            reserved: 0,
            shadow_caster: false,
            streams: vec![Stream {
                format,
                bone: 1,
                stride: l.stride,
                vertex_count: 3,
                vertices,
                indices: vec![0, 1, 2],
                prims: vec![MPrim { strip: 0, triangle_count: 1, material: 7, lod: 0, segment_bits: 0, index_start: 0, index_count: 3 }],
            }],
            bones: vec![bone(-1, [0.0, 0.0, 0.0]), bone(0, [5.0, 0.0, 0.0])],
            remaps: remap.map(|m| vec![Remap { bone: 1, matrix: m }]).unwrap_or_default(),
            bounds: Bounds { centre: [0.0; 3], half: [1.0; 3], radius: 1.0 },
            tail: Tail::None,
        }
    }

    fn skinned(smsh: &Smsh, rig: &[Bone]) -> Vec<Vec3> {
        let joints = crate::skinning::bind_pose(rig);
        smsh.positions
            .iter()
            .zip(&smsh.joints)
            .zip(&smsh.weights)
            .map(|((p, j), w)| {
                let m = joints[j[0] as usize] * w[0];
                m.transform_point3(Vec3::from_array(*p))
            })
            .collect()
    }

    #[test]
    fn a_rigid_stream_draws_at_its_bone_at_bind_pose() {
        let m = rigid_mesh(None);
        let (smsh, rig, uvless) = convert(&m).unwrap();
        assert_eq!(uvless, vec![(0, 3)]);
        let got = skinned(&smsh, &rig);
        assert!((got[1] - Vec3::new(6.0, 0.0, 0.0)).length() < 1e-5, "{got:?}");
        // A stored inverse bind (row-vector convention: translation in m[12..14]) that differs from
        // the inverse bind world still draws the vertex at world · p.
        let ib = Mat4::from_translation(Vec3::new(-4.0, 1.0, 0.0)).to_cols_array();
        let (smsh, rig, _) = convert(&rigid_mesh(Some(ib))).unwrap();
        let got = skinned(&smsh, &rig);
        assert!((got[1] - Vec3::new(6.0, 0.0, 0.0)).length() < 1e-4, "{got:?}");
        assert_eq!(smsh.prims[0].material_hash, 7);
        assert!(smsh.normals.iter().all(|n| (Vec3::from_array(*n).length() - 1.0).abs() < 1e-5));
    }

    #[test]
    fn dxn_normals_get_z() {
        let mut t = CpuTexture { name: String::new(), width: 1, height: 1, format: 0, rgba: vec![128, 128, 0, 255] };
        to_xyz_normal(&mut t, texture::DXN);
        assert_eq!(t.rgba[2], 255);
        let mut dxt5 = CpuTexture { name: String::new(), width: 1, height: 1, format: 0, rgba: vec![128, 128, 200, 255] };
        to_xyz_normal(&mut dxt5, texture::DXT5);
        assert_eq!(dxt5.rgba[2], 200);
    }

    /// Opens the build named by `proto2008_dir` in the repo-root `.sab-local.toml` and loads every
    /// mesh record: decoded, converted, and every primitive's material found in its pack.
    #[cfg(feature = "retail")]
    #[test]
    fn every_mesh_of_the_build_loads() {
        let bin360 = sab_formats::game_paths::local_config_proto2008_bin360(Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap_or_else(|e| panic!("{e}"));
        let src = Prototype2008Source::open(&bin360).unwrap_or_else(|e| panic!("{e}"));
        let handles = src.list_meshes();
        let failures: Vec<String> = handles.iter().filter_map(|h| src.load_mesh(h).err()).collect();
        assert!(failures.is_empty(), "{} of {} meshes failed:\n{}", failures.len(), handles.len(), failures.join("\n"));
        assert_eq!(handles.len(), 137_861);
    }
}
