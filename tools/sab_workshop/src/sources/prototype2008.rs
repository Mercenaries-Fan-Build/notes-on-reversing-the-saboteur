//! The 2008-05 Xbox 360 prototype (`Bin360/`) as a [`GameSource`], read through
//! `sab_formats::proto2008`: assets assembled from `GameTemplates.wsd` (`BLU2`), parts decoded from the
//! `SBL*` packs' `meshes` slots, textures bound from their `materials` slots, `ANM0` clips, the version
//! 3 `GameText.dlg`, and the build's textures for the Icons page. Formats and evidence:
//! `docs/2008_templates_and_text.md`, `docs/2008_subpack_format.md`, `docs/2008_msh4_format.md`,
//! `docs/2008_texture_format.md`, `docs/2008_materials_format.md`, `docs/2008_animations_format.md`.
//!
//! Meshes: the `Global/` packs hold the dynamic objects templates assemble from — one mesh item per
//! name, or per distinct copy where copies differ. Every other pack is a world block; its meshes are
//! listed under it. A template's reference to a mesh resolves as the engine's `LoadDynamic(template)`
//! does: to the copy in the pack named after the template, else to the only copy in `Global/`.
//!
//! Every pack of the build parses (the `sab_formats` game gate), so a pack that does not fails the
//! open with its path: that copy of the build is damaged.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use glam::{Mat3, Mat4, Quat, Vec3};
use memmap2::Mmap;
use sab_formats::gametemplates::GameTemplates;
use sab_formats::proto2008::material::{self, Material};
use sab_formats::proto2008::sbl::{self, SlotKind};
use sab_formats::proto2008::{mesh, slot_dir, texture};
use sab_formats::{dds, pandemic_hash, pixels, zlib};

use crate::clip::AnimClip;
use crate::dtex::CpuTexture;
use crate::formats::{Bone, Prim, Smsh, SubMesh};
use crate::meshload::LoadedMesh;
use crate::resolve::{ModelTex, Prov, Role, SubmeshBinding, TexAsset};
use crate::source::{ClipRow, GameSource, GameVersion, IconRow, MeshFind, MeshItem, PartRange, RawRecord, SourceFingerprint};
use crate::sources::prototype2008_anim::Catalog;

/// The folder of the packs that hold dynamic objects; every other folder's packs are world blocks.
const DYNAMIC: &str = "Global/";

/// One loose `SBL*` pack.
struct Pack {
    path: PathBuf,
    /// Path relative to `Bin360`, `/`-separated.
    rel: String,
    /// The file stem, lower case: the template a dynamic pack is named after.
    stem: String,
    magic: [u8; 4],
    materials: Vec<Material>,
}

/// One `meshes` slot record.
struct MeshRec {
    pack: usize,
    /// The stored record's byte range in the pack file.
    offset: usize,
    stored: usize,
    unpacked: usize,
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

/// One loose `.dds` file.
struct LooseDds {
    name: String,
    rel: String,
    path: PathBuf,
}

pub struct Prototype2008Source {
    bin360: PathBuf,
    fingerprint: SourceFingerprint,
    packs: Vec<Pack>,
    records: Vec<MeshRec>,
    items: Vec<MeshItem>,
    /// Item → a record holding it.
    item_record: Vec<usize>,
    /// Record → its item.
    record_item: Vec<usize>,
    /// Name hash → the dynamic items of that name.
    by_name: HashMap<u32, Vec<usize>>,
    /// A dynamic pack by its stem, lower case.
    dynamic_pack: HashMap<String, usize>,
    /// `(name hash, dynamic pack)` → the record of that name in that pack.
    in_pack: HashMap<(u32, usize), usize>,
    /// World blocks: pack path → its items.
    blocks: Vec<(String, Vec<usize>)>,
    /// Texture hash → every record with that hash, in pack order.
    textures: HashMap<u32, Vec<TexRef>>,
    loose_dds: Vec<LooseDds>,
    templates: GameTemplates,
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

/// FNV-1a 64 of a record, to tell copies apart without holding them.
fn fingerprint64(b: &[u8]) -> u64 {
    b.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &c| (h ^ c as u64).wrapping_mul(0x0000_0100_0000_01B3))
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
        let mut records: Vec<MeshRec> = Vec::new();
        let mut copy_of: Vec<u64> = Vec::new();
        let mut names: Vec<String> = Vec::new();
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
            let dynamic = rel.starts_with(DYNAMIC);
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
                            names.push(mesh::record_name(prefix, compressed).map_err(damaged)?);
                            copy_of.push(if dynamic { fingerprint64(rec) } else { 0 });
                            records.push(MeshRec {
                                pack,
                                offset: r.offset as usize + t.offset as usize,
                                stored: t.stored as usize,
                                unpacked: t.unpacked as usize,
                                hash: t.hash,
                            });
                        }
                    }
                    _ => {}
                }
            }
            let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or_default().to_ascii_lowercase();
            packs.push(Pack { path: p.clone(), rel, stem, magic: sp.magic, materials: mats });
        }

        // Items: a dynamic mesh is one item per distinct copy of its name; a world mesh is one item per
        // record, listed under its block.
        let mut items: Vec<MeshItem> = Vec::new();
        let mut item_record: Vec<usize> = Vec::new();
        let mut record_item = vec![usize::MAX; records.len()];
        let mut by_name: HashMap<u32, Vec<usize>> = HashMap::new();
        let mut copies: HashMap<(u32, u64), usize> = HashMap::new();
        let mut distinct: HashMap<u32, usize> = HashMap::new();
        for (ri, r) in records.iter().enumerate() {
            if packs[r.pack].rel.starts_with(DYNAMIC) && !copies.contains_key(&(r.hash, copy_of[ri])) {
                copies.insert((r.hash, copy_of[ri]), usize::MAX);
                *distinct.entry(r.hash).or_default() += 1;
            }
        }
        let mut blocks: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for (ri, r) in records.iter().enumerate() {
            let pack = &packs[r.pack];
            if pack.rel.starts_with(DYNAMIC) {
                let slot = copies.get_mut(&(r.hash, copy_of[ri])).expect("every dynamic copy was counted");
                if *slot == usize::MAX {
                    *slot = items.len();
                    let name = if distinct[&r.hash] > 1 { format!("{} ({})", names[ri], pack.rel) } else { names[ri].clone() };
                    items.push(MeshItem { name, location: pack.rel.clone(), world: false });
                    item_record.push(ri);
                    by_name.entry(r.hash).or_default().push(*slot);
                }
                record_item[ri] = *slot;
            } else {
                record_item[ri] = items.len();
                blocks.entry(r.pack).or_default().push(items.len());
                items.push(MeshItem { name: names[ri].clone(), location: pack.rel.clone(), world: true });
                item_record.push(ri);
            }
        }
        let blocks: Vec<(String, Vec<usize>)> = blocks.into_iter().map(|(p, v)| (packs[p].rel.clone(), v)).collect();
        let dynamic_pack: HashMap<String, usize> =
            packs.iter().enumerate().filter(|(_, p)| p.rel.starts_with(DYNAMIC)).map(|(i, p)| (p.stem.clone(), i)).collect();
        let mut in_pack: HashMap<(u32, usize), usize> = HashMap::new();
        for (ri, r) in records.iter().enumerate().filter(|(_, r)| packs[r.pack].rel.starts_with(DYNAMIC)) {
            match in_pack.get(&(r.hash, r.pack)) {
                Some(&prev) if copy_of[prev] != copy_of[ri] => {
                    return Err(format!("{}: two different mesh records named {}", packs[r.pack].rel, names[ri]));
                }
                Some(_) => {}
                None => {
                    in_pack.insert((r.hash, r.pack), ri);
                }
            }
        }

        let tpath = bin360.join("GameTemplates.wsd");
        let tb = std::fs::read(&tpath).map_err(|e| format!("{}: {e}", tpath.display()))?;
        let (templates, used) = GameTemplates::parse(&tb).map_err(|e| format!("{}: {e}", tpath.display()))?;
        if used != tb.len() {
            return Err(format!("{}: the templates end at byte {used} of {}", tpath.display(), tb.len()));
        }
        let animations = Catalog::open(&bin360, &all)?;

        let n_textures: usize = textures.values().map(Vec::len).sum();
        let n_materials: usize = packs.iter().map(|p| p.materials.len()).sum();
        fingerprint.matched(format!(
            "{} SBL packs: {} mesh records ({} dynamic items, {} world blocks), {n_textures} texture records ({} distinct), {n_materials} materials",
            packs.len(),
            records.len(),
            items.iter().filter(|i| !i.world).count(),
            blocks.len(),
            textures.len()
        ));
        fingerprint.matched(format!("GameTemplates.wsd: {} templates", templates.templates().count()));
        let (loose_clips, pack_records) = animations.counts();
        fingerprint.matched(format!("animations: {loose_clips} loose ANM0 clips, {pack_records} pack records"));
        fingerprint.matched(format!("{} loose .dds files", loose_dds.len()));
        fingerprint.matched(format!(
            "not indexed: {mega} MP00 archives (byte copies of loose files), {anim_packs} AP04 animation pack (read as clips), {empty} empty .pack files"
        ));
        Ok(Self {
            bin360,
            fingerprint,
            packs,
            records,
            items,
            item_record,
            record_item,
            by_name,
            dynamic_pack,
            in_pack,
            blocks,
            textures,
            loose_dds,
            templates,
            animations,
        })
    }

    fn record(&self, item: usize) -> Result<&MeshRec, String> {
        let ri = *self.item_record.get(item).ok_or_else(|| format!("mesh {item} out of range"))?;
        Ok(&self.records[ri])
    }

    /// The parsed record of a mesh item.
    pub fn read_mesh_record(&self, item: usize) -> Result<mesh::Record, String> {
        let r = self.record(item)?;
        let pack = &self.packs[r.pack];
        let m = map(&pack.path)?;
        let stored = &m[r.offset..r.offset + r.stored];
        let name = &self.items[item].name;
        let body = if r.stored < r.unpacked { zlib::inflate_exact(stored, r.unpacked, name)? } else { stored.to_vec() };
        mesh::parse_record(&body).map_err(|e| format!("{name} in {}: {e}", pack.rel))
    }

    /// A texture record, preferring one in pack `prefer`.
    fn tex_ref(&self, hash: u32, prefer: usize) -> Option<&TexRef> {
        let refs = self.textures.get(&hash)?;
        refs.iter().find(|t| t.pack == prefer).or_else(|| refs.first())
    }

    fn tex_bytes(&self, t: &TexRef) -> Result<Vec<u8>, String> {
        Ok(map(&self.packs[t.pack].path)?[t.offset..t.offset + t.stored].to_vec())
    }

    fn decode_loose_dds(&self, i: usize, preview: Option<u32>) -> Result<CpuTexture, String> {
        let f = self.loose_dds.get(i).ok_or_else(|| format!("loose .dds {i} out of range"))?;
        let b = std::fs::read(&f.path).map_err(|e| format!("{}: {e}", f.path.display()))?;
        let d = dds::parse(&b).map_err(|e| format!("{}: {e}", f.rel))?;
        let level = match preview {
            Some(min) => (0..d.mips.max(1)).rev().find(|&l| (d.width >> l).max(1) >= min && (d.height >> l).max(1) >= min).unwrap_or(0),
            None => 0,
        };
        let (w, h) = ((d.width >> level).max(1), (d.height >> level).max(1));
        let rgba = pixels::decode(d.format, w as usize, h as usize, d.level(0, level)?).map_err(|e| format!("{}: {e}", f.rel))?;
        Ok(CpuTexture { name: f.name.clone(), width: w, height: h, format: d.four_cc, rgba })
    }
}

/// Decode a 2008 `textures` slot record: its base level, or for `preview` the smallest level at least
/// that size; a normal map's z rebuilt for display.
pub fn decode_texture(record: &[u8], normal: bool, preview: Option<u32>) -> Result<CpuTexture, String> {
    let t = texture::parse(record)?;
    let memory = t.memory()?;
    let (levels, _) = texture::levels(&t)?;
    let level = match preview {
        Some(min) => levels.iter().rev().find(|l| l.width >= min && l.height >= min).unwrap_or(&levels[0]),
        None => &levels[0],
    };
    let rgba = texture::decode_level(&t, &memory, level)?;
    let mut tex = CpuTexture { name: t.name.clone(), width: level.width, height: level.height, format: t.format, rgba };
    if normal {
        to_xyz_normal(&mut tex, t.format & !texture::GAMMA_SIGNS);
    }
    Ok(tex)
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
            name: crate::bone_names::name_for(b.hash),
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
                damage: None,
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

    fn templates(&self) -> &GameTemplates {
        &self.templates
    }

    fn meshes(&self) -> &[MeshItem] {
        &self.items
    }

    fn find_mesh(&self, hash: u32, template: &str) -> MeshFind {
        let Some(items) = self.by_name.get(&hash) else { return MeshFind::Absent };
        let own = self.dynamic_pack.get(&template.to_ascii_lowercase()).and_then(|&p| self.in_pack.get(&(hash, p)));
        if let Some(&ri) = own {
            return MeshFind::Found(self.record_item[ri]);
        }
        match items.as_slice() {
            [one] => MeshFind::Found(*one),
            many => MeshFind::Ambiguous(many.iter().map(|&i| self.items[i].location.clone()).collect()),
        }
    }

    fn load_part(&self, item: usize) -> Result<LoadedMesh, String> {
        let record = self.read_mesh_record(item)?;
        let m = record.mesh();
        let (smsh, rig, uvless) = convert(m)?;
        let name = self.items[item].name.clone();
        let stored_ibm = m.remaps.iter().map(|r| (m.bones[r.bone as usize].hash, r.matrix)).collect();
        Ok(LoadedMesh {
            prim_parent_bone: smsh.prims.iter().map(|p| p.parent_bone).collect(),
            bone_hashes: m.bones.iter().map(|b| b.hash).collect(),
            stored_ibm,
            part_ranges: Vec::new(),
            uvless,
            bones: rig,
            mesh: smsh,
            name,
        })
    }

    fn raw_part(&self, item: usize) -> Result<RawRecord, String> {
        let r = self.record(item)?;
        let pack = &self.packs[r.pack];
        let bytes = map(&pack.path)?[r.offset..r.offset + r.stored].to_vec();
        let kind = if r.stored < r.unpacked { "zlib" } else { "variant" };
        Ok(RawRecord { file: format!("{}.{kind}", self.items[item].name), bytes, location: pack.rel.clone() })
    }

    fn resolve_textures(&self, parts: &[PartRange], submeshes: &[SubMesh], uvless: &[(u32, u32)]) -> Result<ModelTex, String> {
        let mut assets: Vec<TexAsset> = Vec::new();
        // (hash, pack) → asset; identical copies in different packs are one asset, by content.
        let mut asset_of: HashMap<(u32, usize), usize> = HashMap::new();
        let mut by_content: HashMap<(u32, u64), usize> = HashMap::new();
        let mut submesh_tex = vec![SubmeshBinding::default(); submeshes.len()];
        let mut submesh_prov = vec![Prov::Unresolved; submeshes.len()];
        for (i, sm) in submeshes.iter().enumerate() {
            let (s0, s1) = (sm.index_start, sm.index_start + sm.index_count);
            if uvless.iter().any(|&(a, n)| a < s1 && s0 < a + n) {
                continue;
            }
            let part = parts
                .iter()
                .find(|p| s0 >= p.index_start && s0 < p.index_start + p.index_count)
                .ok_or_else(|| format!("submesh {i} at index {s0} is in no part"))?;
            let pack_index = self.record(part.mesh)?.pack;
            let pack = &self.packs[pack_index];
            let Some(&uid) = sm.materials.first() else { continue };
            let mat = pack
                .materials
                .iter()
                .find(|m| m.uid == uid)
                .ok_or_else(|| format!("{}: material {uid:08X} is not in the materials slot of {}", self.items[part.mesh].name, pack.rel))?;
            let Some(pass) = mat.passes.iter().find(|p| p.name == material::PASS_COLOR) else { continue };
            let mut normal_done = false;
            for (sampler, hash) in pass.sampler_textures(mat).enumerate() {
                let Some(t) = self.tex_ref(hash, pack_index) else { continue };
                let kind = t.kind();
                let ai = match asset_of.get(&(hash, t.pack)) {
                    Some(&a) => a,
                    None => {
                        let bytes = self.tex_bytes(t)?;
                        let content = (hash, fingerprint64(&bytes));
                        let a = match by_content.get(&content) {
                            Some(&a) => a,
                            None => {
                                let role = match kind {
                                    TextureKind::Colour => Role::Diffuse,
                                    TextureKind::Normal => Role::Normal,
                                    TextureKind::Linear => Role::Other,
                                };
                                let format = format!("0x{:08X}", t.header.format);
                                let record = Arc::new(bytes);
                                assets.push(TexAsset::proto2008(t.header.name.clone(), role, t.header.width as u32, t.header.height as u32, format, record, kind == TextureKind::Normal));
                                by_content.insert(content, assets.len() - 1);
                                assets.len() - 1
                            }
                        };
                        asset_of.insert((hash, t.pack), a);
                        a
                    }
                };
                if sampler == 0 {
                    submesh_tex[i].set(SubmeshBinding::DIFFUSE, Some(ai));
                } else if kind == TextureKind::Normal && !normal_done {
                    submesh_tex[i].set(SubmeshBinding::NORMAL, Some(ai));
                    normal_done = true;
                }
            }
            if submesh_tex[i].is_bound() {
                submesh_prov[i] = Prov::Bound;
            }
        }
        let mut cache: HashMap<usize, CpuTexture> = HashMap::new();
        let mut decoded = Vec::new();
        for (i, b) in submesh_tex.iter().enumerate() {
            for (slot, ai) in b.iter_bound() {
                let tex = match cache.get(&ai) {
                    Some(t) => t.clone(),
                    None => {
                        let t = assets[ai].decode().map_err(|e| format!("texture {}: {e}", assets[ai].name))?;
                        cache.insert(ai, t.clone());
                        t
                    }
                };
                decoded.push((i, slot, tex));
            }
        }
        Ok(ModelTex { assets, submesh_tex, submesh_prov, decoded })
    }

    fn describe_mesh(&self, item: usize) -> Vec<(String, String)> {
        let Ok(r) = self.record(item) else { return Vec::new() };
        let p = &self.packs[r.pack];
        let copies = self.records.iter().filter(|x| x.hash == r.hash).count();
        vec![
            ("pack".into(), format!("{} ({})", p.rel, String::from_utf8_lossy(&p.magic))),
            ("hash".into(), format!("{:08X}", r.hash)),
            (
                "stored".into(),
                format!("{} bytes{}", r.stored, if r.stored < r.unpacked { format!(", zlib of {}", r.unpacked) } else { " raw (variant record)".into() }),
            ),
            ("records of this name".into(), copies.to_string()),
        ]
    }

    fn world_blocks(&self) -> &[(String, Vec<usize>)] {
        &self.blocks
    }

    fn clips(&self) -> &[ClipRow] {
        self.animations.rows()
    }

    fn load_clip(&self, index: usize) -> Result<Box<dyn AnimClip>, String> {
        Ok(Box::new(self.animations.load(index)?))
    }

    fn game_text_files(&self) -> Vec<(String, PathBuf)> {
        let dir = self.bin360.join("Cinematics/Dialog");
        let mut out: Vec<(String, PathBuf)> = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.join("GameText.dlg").is_file())
            .map(|p| (p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), p.join("GameText.dlg")))
            .collect();
        out.sort();
        out
    }

    fn icon_textures(&self, progress: &dyn Fn(usize, usize)) -> Result<Vec<IconRow>, String> {
        let total = self.textures.len() + self.loose_dds.len();
        let mut out: Vec<IconRow> = Vec::with_capacity(total);
        for (i, (hash, refs)) in self.textures.iter().enumerate() {
            let t = &refs[0];
            out.push(IconRow { name: t.header.name.clone(), hash: *hash, key: (t.pack, t.offset, t.stored) });
            if i % 4096 == 0 {
                progress(i, total);
            }
        }
        for (i, f) in self.loose_dds.iter().enumerate() {
            out.push(IconRow { name: f.rel.clone(), hash: pandemic_hash(&f.rel), key: (usize::MAX, i, 0) });
        }
        out.sort_by(|a, b| a.name.to_ascii_lowercase().cmp(&b.name.to_ascii_lowercase()));
        progress(total, total);
        Ok(out)
    }

    fn decode_icon(&self, row: &IconRow, preview: Option<u32>) -> Result<CpuTexture, String> {
        let (pack, offset, stored) = row.key;
        if pack == usize::MAX {
            return self.decode_loose_dds(offset, preview);
        }
        let p = self.packs.get(pack).ok_or_else(|| format!("{}: pack {pack} out of range", row.name))?;
        let record = &map(&p.path)?[offset..offset + stored];
        let normal = self.textures.get(&row.hash).and_then(|r| r.iter().find(|t| t.pack == pack)).map(TexRef::kind) == Some(TextureKind::Normal);
        decode_texture(record, normal, preview).map_err(|e| format!("{}: {e}", row.name))
    }

    fn publishable(&self) -> bool {
        false
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
}
