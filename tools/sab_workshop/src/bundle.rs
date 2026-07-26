//! LOSSLESS model-export bundle — the community-editable interchange for a Saboteur asset.
//!
//! Ported from `mercs2_workshop`'s `src/bundle.rs`, which solved this problem first; the rule it
//! encodes is the reason to take it wholesale rather than grow another one-button `.glb` writer.
//!
//! **The preservation rule: nothing is discarded.** An editable format can only carry what it
//! understands, and we have NOT fully reversed everything a Saboteur MESH record holds (the SMSH
//! stream/format table beyond the vertex semantics we decode, `parentBone` rigid attachments on
//! pre-v2 files, per-drawcall LOD/damage material passes, the material hashes we cannot resolve to
//! a name). So the bundle keeps BOTH:
//!
//!   * `raw/*.msha`    — each source part's ORIGINAL on-disk record, verbatim (the 276-byte header
//!                       plus both still-deflated blobs). This is the guarantee: whatever we failed
//!                       to understand is still here, byte-exact.
//!   * `model.glb`     — the EDITABLE view: geometry + the real bone hierarchy + skin + one
//!                       primitive per submesh, each bound to its material. Every DCC reads glTF 2.0.
//!   * `textures/`     — the bound skins decoded to PNG, editable, named in the manifest.
//!   * `manifest.json` — the reassembly map: every submesh's binding (range -> material hashes ->
//!                       texture -> how we knew), the bone table with name hashes, the source part
//!                       list, and the FULL texture pool the character offered so nothing that was
//!                       available is silently invisible.
//!
//! Textures are referenced from the GLB by relative `uri` (`textures/Foo.png`) rather than embedded.
//! glTF 2.0 permits this in a GLB container, it keeps the binary small, and — the point — it means
//! repainting a skin is editing a PNG in place, not a round trip through a re-export.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::formats::SubMesh;
use crate::meshload::{self, LoadedMesh, MeshEntry};
use crate::resolve::{Prov, TexAsset};

/// Everything an export needs, OWNED — so it can be handed to a worker thread.
///
/// The worker opens its OWN megapack mmap (mirroring `spawn_tex_resolve`) instead of borrowing the
/// app's, so the export touches neither the live pack handle nor the GPU. The texture pool and the
/// submesh bindings are cloned rather than re-resolved because the user may have hand-bound slots on
/// the Materials page: the bundle has to reflect what is on screen, not what the auto-seed would
/// have guessed.
pub struct Job {
    pub megapack: String,
    /// Asset name — the bundle directory and the glTF mesh name.
    pub label: String,
    pub parts: Vec<MeshEntry>,
    /// The submesh cover as the renderer built it, so `submesh_tex` indexes line up exactly.
    pub submeshes: Vec<SubMesh>,
    pub assets: Vec<TexAsset>,
    pub submesh_tex: Vec<Option<usize>>,
    pub submesh_prov: Vec<Prov>,
    pub outroot: PathBuf,
}

/// Make a string safe as a path component.
fn sanitize(s: &str) -> String {
    let c: String = s
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.') { c } else { '_' })
        .collect();
    if c.is_empty() { "export".into() } else { c }
}

/// Row-major 4x4 (flat) to column-major flat — a transpose of the 16-float layout.
///
/// `Bone::inv_bind` is stored ROW-major here and glTF matrices are COLUMN-major, so each
/// inverse-bind is transposed on the way out.
fn to_colmajor(m: &[f32; 16]) -> [f32; 16] {
    [
        m[0], m[4], m[8], m[12], m[1], m[5], m[9], m[13], m[2], m[6], m[10], m[14], m[3], m[7],
        m[11], m[15],
    ]
}

/// The stem a texture's variants share — `Sean_Head_D` / `Sean_Head_N` -> `Sean_Head`.
///
/// Used to pull a bound diffuse's normal/spec/mask siblings into the bundle. Decoding the WHOLE pool
/// would be hundreds of MB of RGBA for a couple hundred records (see `TexAsset`'s note on why decode
/// is lazy); decoding the siblings of what is actually bound is the targeted version of the same
/// preservation intent, and those maps are half the look of these assets.
fn stem(name: &str) -> String {
    let n = name.trim_end_matches(|c: char| c == '\0');
    let lower = n.to_ascii_lowercase();
    for suf in ["_d", "_n", "_nm", "_s", "_sm", "_m", "_mask"] {
        if let Some(base) = lower.strip_suffix(suf) {
            return base.to_string();
        }
    }
    lower
}

/// A texture that made it into `textures/`.
struct WrittenTex {
    /// Index into `Job::assets`.
    asset: usize,
    file: String,
}

/// Write a decoded texture as an 8-bit RGBA PNG.
fn write_png(path: &Path, w: u32, h: u32, rgba: &[u8]) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut wr = enc.write_header().map_err(|e| e.to_string())?;
    wr.write_image_data(rgba).map_err(|e| e.to_string())
}

/// Run a full bundle export. Returns the directory written.
///
/// Safe to call from a worker thread: it opens its own megapack and touches no GPU state.
pub fn run(job: &Job) -> Result<String, String> {
    let mp = crate::pack::Megapack::open(&job.megapack)?;
    let lm = meshload::assemble(mp.raw(), &job.parts)?;

    let dir = job.outroot.join(sanitize(&job.label));
    std::fs::create_dir_all(dir.join("raw")).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(dir.join("textures")).map_err(|e| e.to_string())?;

    // ---- raw/ : the byte-exact source records ----
    let mut raw_files: Vec<(String, String, usize)> = Vec::new(); // (part, file, bytes)
    for p in &job.parts {
        let Some(bytes) = meshload::raw_record(mp.raw(), p) else { continue };
        let file = format!("{}.msha", sanitize(&p.name));
        std::fs::write(dir.join("raw").join(&file), bytes).map_err(|e| e.to_string())?;
        raw_files.push((p.name.clone(), format!("raw/{file}"), bytes.len()));
    }

    // ---- textures/ : every bound skin, plus its normal/spec siblings ----
    let mut want: Vec<usize> = job.submesh_tex.iter().flatten().copied().collect();
    want.sort_unstable();
    want.dedup();
    let bound_stems: Vec<String> = want.iter().filter_map(|i| job.assets.get(*i)).map(|a| stem(&a.name)).collect();
    for (i, a) in job.assets.iter().enumerate() {
        if !want.contains(&i) && bound_stems.contains(&stem(&a.name)) {
            want.push(i);
        }
    }
    want.sort_unstable();
    want.dedup();

    let mut written: Vec<WrittenTex> = Vec::new();
    let mut tex_errs: Vec<String> = Vec::new();
    for &i in &want {
        let Some(a) = job.assets.get(i) else { continue };
        match a.decode() {
            Ok(t) => {
                let file = format!("{}.png", sanitize(&a.name));
                match write_png(&dir.join("textures").join(&file), t.width, t.height, &t.rgba) {
                    Ok(()) => written.push(WrittenTex { asset: i, file }),
                    Err(e) => tex_errs.push(format!("{}: {e}", a.name)),
                }
            }
            // A texture that will not decode is reported in the manifest, not fatal: a bundle with
            // one missing skin is still worth having, and silence would look like it had none.
            Err(e) => tex_errs.push(format!("{}: {e}", a.name)),
        }
    }

    // ---- model.glb ----
    let glb = write_glb(&lm, &job.label, &job.submeshes, &job.submesh_tex, &written);
    std::fs::write(dir.join("model.glb"), &glb).map_err(|e| e.to_string())?;

    // ---- manifest.json ----
    let manifest = manifest(job, &lm, &written, &raw_files, &tex_errs, glb.len());
    std::fs::write(
        dir.join("manifest.json"),
        serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;

    Ok(dir.to_string_lossy().into_owned())
}

/// The reassembly map. Everything the glTF cannot say, said here.
fn manifest(
    job: &Job,
    lm: &LoadedMesh,
    written: &[WrittenTex],
    raw_files: &[(String, String, usize)],
    tex_errs: &[String],
    glb_len: usize,
) -> serde_json::Value {
    let mat_of: BTreeMap<usize, usize> = written.iter().enumerate().map(|(m, w)| (w.asset, m)).collect();
    let file_of: BTreeMap<usize, &str> = written.iter().map(|w| (w.asset, w.file.as_str())).collect();

    let ranges: BTreeMap<&str, (u32, u32)> =
        lm.part_ranges.iter().map(|(n, s, c)| (n.as_str(), (*s, *c))).collect();

    let parts: Vec<_> = raw_files
        .iter()
        .map(|(name, file, bytes)| {
            let (s, c) = ranges.get(name.as_str()).copied().unwrap_or((0, 0));
            serde_json::json!({
                "name": name, "raw": file, "raw_bytes": bytes,
                "index_start": s, "index_count": c,
            })
        })
        .collect();

    let bones: Vec<_> = lm
        .bones
        .iter()
        .enumerate()
        .map(|(i, b)| {
            serde_json::json!({
                "index": i,
                "name": b.name,
                "hash": format!("0x{:08X}", lm.bone_hashes.get(i).copied().unwrap_or(0)),
                "parent": b.parent,
                "has_stored_inverse_bind": b.inv_bind.is_some(),
            })
        })
        .collect();

    let submeshes: Vec<_> = job
        .submeshes
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let a = job.submesh_tex.get(i).copied().flatten();
            serde_json::json!({
                "index": i,
                "index_start": s.index_start,
                "index_count": s.index_count,
                "material_hashes": s.materials.iter().map(|m| format!("0x{m:08X}")).collect::<Vec<_>>(),
                "texture": a.and_then(|x| job.assets.get(x)).map(|t| t.name.clone()),
                "texture_file": a.and_then(|x| file_of.get(&x)).map(|f| format!("textures/{f}")),
                "gltf_material": a.and_then(|x| mat_of.get(&x)).copied(),
                "provenance": match job.submesh_prov.get(i) {
                    Some(Prov::Bound) => "bound",
                    Some(Prov::Seeded) => "seeded",
                    _ => "unresolved",
                },
            })
        })
        .collect();

    // The FULL pool, exported or not — what the character offered, so a modder can see what we had
    // and chose not to decode rather than assuming it did not exist.
    let pool: Vec<_> = job
        .assets
        .iter()
        .enumerate()
        .map(|(i, a)| {
            serde_json::json!({
                "name": a.name, "role": a.role.label(),
                "width": a.width, "height": a.height, "format": a.format,
                "exported": file_of.get(&i).map(|f| format!("textures/{f}")),
            })
        })
        .collect();

    serde_json::json!({
        "generator": "sab_workshop",
        "bundle_version": 1,
        "label": job.label,
        "source": { "megapack": job.megapack, "parts": parts },
        "counts": {
            "vertices": lm.mesh.positions.len(),
            "triangles": lm.mesh.indices.len() / 3,
            "bones": lm.bones.len(),
            "submeshes": job.submeshes.len(),
            "textures_exported": written.len(),
            "glb_bytes": glb_len,
        },
        "model": "model.glb",
        "bones": bones,
        "submeshes": submeshes,
        "texture_pool": pool,
        "texture_errors": tex_errs,
    })
}

/// Serialize the assembled character as a skinned bind-pose `.glb` (glTF 2.0 binary): mesh +
/// skeleton + skin + ONE PRIMITIVE PER SUBMESH, each bound to the material its texture came from.
///
/// The per-submesh split is the difference that matters versus a single merged primitive: it is what
/// carries the material binding into Blender at all, and it is the seam a modder needs to swap one
/// body part without touching the rest.
///
/// JOINTS_0 carry global bone indices and `skin.joints` is the identity list `[0..N)`, so
/// `nodeWorld[b] * inverseBindMatrices[b]` is identity at bind pose.
fn write_glb(
    lm: &LoadedMesh,
    label: &str,
    submeshes: &[SubMesh],
    submesh_tex: &[Option<usize>],
    written: &[WrittenTex],
) -> Vec<u8> {
    let g = &lm.mesh;
    let nv = g.positions.len();
    let ni = g.indices.len();
    let nb = lm.bones.len();
    let has_skin = nb > 0 && g.joints.len() == nv && g.weights.len() == nv;

    // ---- BIN chunk ----
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
    if nv == 0 {
        (pmin, pmax) = ([0.0; 3], [0.0; 3]);
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
    const IDENT: [f32; 16] =
        [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];
    for b in &lm.bones {
        for c in to_colmajor(&b.inv_bind.unwrap_or(IDENT)) {
            bin.extend_from_slice(&c.to_le_bytes());
        }
    }
    align(&mut bin);

    // ---- bufferViews ----
    let mut views: Vec<serde_json::Value> = Vec::new();
    let mut view = |off: usize, len: usize, target: Option<u32>| -> usize {
        let mut v = serde_json::json!({ "buffer": 0, "byteOffset": off, "byteLength": len });
        if let Some(t) = target {
            v["target"] = serde_json::json!(t);
        }
        views.push(v);
        views.len() - 1
    };
    let v_pos = view(pos_off, nv * 12, Some(34962));
    let v_nrm = view(nrm_off, nv * 12, Some(34962));
    let v_uv = view(uv_off, nv * 8, Some(34962));
    let v_jnt = view(jnt_off, nv * 8, Some(34962));
    let v_wgt = view(wgt_off, nv * 16, Some(34962));
    let v_idx = view(idx_off, ni * 4, Some(34963));
    let v_ibm = view(ibm_off, nb * 64, None);

    // ---- accessors ----
    let mut accs: Vec<serde_json::Value> = Vec::new();
    let a_pos = {
        accs.push(serde_json::json!({
            "bufferView": v_pos, "componentType": 5126, "count": nv, "type": "VEC3",
            "min": pmin.to_vec(), "max": pmax.to_vec(),
        }));
        accs.len() - 1
    };
    let mut acc = |view: usize, ctype: u32, count: usize, ty: &str, byte_off: usize| -> usize {
        accs.push(serde_json::json!({
            "bufferView": view, "byteOffset": byte_off,
            "componentType": ctype, "count": count, "type": ty,
        }));
        accs.len() - 1
    };
    let a_nrm = acc(v_nrm, 5126, nv, "VEC3", 0);
    let a_uv = acc(v_uv, 5126, nv, "VEC2", 0);
    let a_jnt = acc(v_jnt, 5123, nv, "VEC4", 0);
    let a_wgt = acc(v_wgt, 5126, nv, "VEC4", 0);
    let a_ibm = acc(v_ibm, 5126, nb, "MAT4", 0);

    // ---- materials / images / textures ----
    // One material per exported skin. A submesh with no binding gets no `material` at all, which is
    // glTF's own way of saying "default" — better than inventing a grey material that a modder would
    // then have to tell apart from a real one.
    let mut images: Vec<serde_json::Value> = Vec::new();
    let mut textures: Vec<serde_json::Value> = Vec::new();
    let mut materials: Vec<serde_json::Value> = Vec::new();
    let mut mat_of: BTreeMap<usize, usize> = BTreeMap::new();
    for (m, w) in written.iter().enumerate() {
        images.push(serde_json::json!({ "uri": format!("textures/{}", w.file) }));
        textures.push(serde_json::json!({ "source": m }));
        materials.push(serde_json::json!({
            "name": w.file.trim_end_matches(".png"),
            "pbrMetallicRoughness": {
                "baseColorTexture": { "index": m },
                "metallicFactor": 0.0,
                "roughnessFactor": 1.0,
            },
            "doubleSided": true,
        }));
        mat_of.insert(w.asset, m);
    }

    // ---- primitives: one per submesh, sharing the vertex accessors ----
    let mut prims: Vec<serde_json::Value> = Vec::new();
    let cover: Vec<SubMesh> = if submeshes.is_empty() {
        vec![SubMesh { index_start: 0, index_count: ni as u32, materials: Vec::new() }]
    } else {
        submeshes.to_vec()
    };
    for (i, s) in cover.iter().enumerate() {
        if s.index_count == 0 {
            continue;
        }
        let a_idx = acc(v_idx, 5125, s.index_count as usize, "SCALAR", s.index_start as usize * 4);
        let mut attrs = serde_json::json!({ "POSITION": a_pos, "NORMAL": a_nrm, "TEXCOORD_0": a_uv });
        if has_skin {
            attrs["JOINTS_0"] = serde_json::json!(a_jnt);
            attrs["WEIGHTS_0"] = serde_json::json!(a_wgt);
        }
        let mut p = serde_json::json!({ "attributes": attrs, "indices": a_idx });
        if let Some(m) = submesh_tex.get(i).copied().flatten().and_then(|x| mat_of.get(&x)) {
            p["material"] = serde_json::json!(m);
        }
        prims.push(p);
    }

    // ---- nodes: the bone hierarchy, then the mesh ----
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); nb];
    let mut roots: Vec<usize> = Vec::new();
    for (b, bone) in lm.bones.iter().enumerate() {
        if bone.parent < 0 {
            roots.push(b);
        } else if let Some(c) = children.get_mut(bone.parent as usize) {
            c.push(b);
        }
    }
    let mut nodes: Vec<serde_json::Value> = Vec::new();
    for (b, bone) in lm.bones.iter().enumerate() {
        let mut n = serde_json::json!({
            "name": bone.name,
            "translation": bone.t.to_vec(),
            "rotation": bone.r.to_vec(),
            "scale": bone.s.to_vec(),
        });
        if !children[b].is_empty() {
            n["children"] = serde_json::json!(children[b]);
        }
        nodes.push(n);
    }
    let mesh_node = nodes.len();
    let mut mn = serde_json::json!({ "name": label, "mesh": 0 });
    if has_skin {
        mn["skin"] = serde_json::json!(0);
    }
    nodes.push(mn);

    let mut scene_nodes: Vec<usize> = roots.clone();
    scene_nodes.push(mesh_node);

    let mut root = serde_json::json!({
        "asset": { "version": "2.0", "generator": "sab_workshop" },
        "scene": 0,
        "scenes": [{ "nodes": scene_nodes }],
        "nodes": nodes,
        "meshes": [{ "name": label, "primitives": prims }],
        "accessors": accs,
        "bufferViews": views,
        "buffers": [{ "byteLength": bin.len() }],
    });
    if has_skin {
        root["skins"] = serde_json::json!([{
            "inverseBindMatrices": a_ibm,
            "skeleton": roots.first().copied().unwrap_or(0),
            "joints": (0..nb).collect::<Vec<_>>(),
        }]);
    }
    if !materials.is_empty() {
        root["images"] = serde_json::json!(images);
        root["textures"] = serde_json::json!(textures);
        root["materials"] = serde_json::json!(materials);
        root["samplers"] = serde_json::json!([{ "wrapS": 10497, "wrapT": 10497 }]);
        for t in root["textures"].as_array_mut().unwrap() {
            t["sampler"] = serde_json::json!(0);
        }
    }

    // ---- GLB container ----
    let mut json_bytes = root.to_string().into_bytes();
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
mod tests {
    use super::*;
    use crate::formats::{Bone, Smsh};

    fn bone(parent: i32) -> Bone {
        Bone {
            parent,
            name: "b".into(),
            t: [0.0, 0.0, 0.0],
            r: [0.0, 0.0, 0.0, 1.0],
            s: [1.0, 1.0, 1.0],
            inv_bind: Some([
                1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
            ]),
            local_m: None,
        }
    }

    fn demo() -> LoadedMesh {
        LoadedMesh {
            name: "T".into(),
            mesh: Smsh {
                positions: vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [1.0, 1.0, 0.0]],
                normals: vec![[0.0, 0.0, 1.0]; 4],
                uvs: vec![[0.0, 0.0]; 4],
                joints: vec![[0, 0, 0, 0]; 4],
                weights: vec![[1.0, 0.0, 0.0, 0.0]; 4],
                indices: vec![0, 1, 2, 1, 3, 2],
                prims: Vec::new(),
            },
            bones: vec![bone(-1), bone(0)],
            prim_parent_bone: Vec::new(),
            bone_hashes: vec![1, 2],
            stored_ibm: Default::default(),
            part_ranges: vec![("T".into(), 0, 6)],
        }
    }

    /// Parse the GLB chunk framing back out — the container has to be readable before any of the
    /// JSON inside it is worth arguing about.
    fn chunks(glb: &[u8]) -> (serde_json::Value, usize) {
        let rd = |o: usize| u32::from_le_bytes(glb[o..o + 4].try_into().unwrap()) as usize;
        assert_eq!(&glb[0..4], b"glTF");
        assert_eq!(rd(4), 2);
        assert_eq!(rd(8), glb.len());
        let jlen = rd(12);
        assert_eq!(&glb[16..20], b"JSON");
        let json: serde_json::Value = serde_json::from_slice(&glb[20..20 + jlen]).unwrap();
        let blen = rd(20 + jlen);
        assert_eq!(&glb[24 + jlen..28 + jlen], b"BIN\0");
        (json, blen)
    }

    #[test]
    fn glb_container_is_valid() {
        let lm = demo();
        let glb = write_glb(&lm, "T", &[], &[], &[]);
        let (json, blen) = chunks(&glb);
        assert_eq!(json["buffers"][0]["byteLength"].as_u64().unwrap() as usize, blen);
        // No submeshes -> one primitive covering the whole index buffer.
        assert_eq!(json["meshes"][0]["primitives"].as_array().unwrap().len(), 1);
        assert!(json["skins"][0]["joints"].as_array().unwrap().len() == 2);
    }

    #[test]
    fn one_primitive_per_submesh_with_its_material() {
        let lm = demo();
        let cover = vec![
            SubMesh { index_start: 0, index_count: 3, materials: vec![0xAA] },
            SubMesh { index_start: 3, index_count: 3, materials: vec![0xBB] },
        ];
        // Submesh 0 binds asset 7, submesh 1 binds nothing.
        let written = vec![WrittenTex { asset: 7, file: "skin.png".into() }];
        let glb = write_glb(&lm, "T", &cover, &[Some(7), None], &written);
        let (json, _) = chunks(&glb);
        let prims = json["meshes"][0]["primitives"].as_array().unwrap();
        assert_eq!(prims.len(), 2);
        assert_eq!(prims[0]["material"].as_u64(), Some(0));
        assert!(prims[1].get("material").is_none(), "unbound submesh must carry no material");
        // Each primitive reads its own slice of the shared index bufferView.
        let a0 = prims[0]["indices"].as_u64().unwrap() as usize;
        let a1 = prims[1]["indices"].as_u64().unwrap() as usize;
        assert_eq!(json["accessors"][a0]["byteOffset"].as_u64(), Some(0));
        assert_eq!(json["accessors"][a1]["byteOffset"].as_u64(), Some(12));
        assert_eq!(json["accessors"][a0]["bufferView"], json["accessors"][a1]["bufferView"]);
        // The skin is referenced by relative URI, not embedded.
        assert_eq!(json["images"][0]["uri"].as_str(), Some("textures/skin.png"));
    }

    /// End-to-end against the installed game: export a real character and read the bundle back.
    ///
    /// The unit tests above prove the writer's shape from synthetic input; only this proves the
    /// thing a modder actually receives — that the parts assemble, the raw records round-trip, and
    /// the glTF's accessors stay inside the buffer they point at. Skips when no install is present.
    #[test]
    fn real_character_bundle_round_trips() {
        let Some(s) = crate::settings::detected() else {
            eprintln!("skip: no Saboteur install detected");
            return;
        };
        let Ok(pack) = crate::pack::Megapack::open(&s.megapack()) else {
            eprintln!("skip: megapack unavailable");
            return;
        };
        let list = crate::meshload::list_meshes(&pack);
        let parts: Vec<MeshEntry> = list
            .iter()
            .filter(|e| e.name.to_ascii_lowercase().contains("seandevlin"))
            .take(3)
            .cloned()
            .collect();
        if parts.is_empty() {
            eprintln!("skip: no Sean parts in this pack");
            return;
        }

        let out = std::env::temp_dir().join("sab_workshop_bundle_test");
        let _ = std::fs::remove_dir_all(&out);
        let job = Job {
            megapack: s.megapack(),
            label: "SeanTest".into(),
            parts: parts.clone(),
            submeshes: Vec::new(),
            assets: Vec::new(),
            submesh_tex: Vec::new(),
            submesh_prov: Vec::new(),
            outroot: out.clone(),
        };
        let dir = PathBuf::from(run(&job).expect("export"));

        // The raw records are the preservation promise: byte-identical to the pack.
        for p in &parts {
            let want = meshload::raw_record(pack.raw(), p).expect("raw slice");
            let got = std::fs::read(dir.join("raw").join(format!("{}.msha", sanitize(&p.name))))
                .expect("raw file");
            assert_eq!(got, want, "raw/{} is not byte-exact", p.name);
        }

        // The GLB parses, and every accessor lands inside the BIN chunk it indexes.
        let glb = std::fs::read(dir.join("model.glb")).expect("model.glb");
        let (json, blen) = chunks(&glb);
        let views = json["bufferViews"].as_array().unwrap();
        for v in views {
            let off = v["byteOffset"].as_u64().unwrap() as usize;
            let len = v["byteLength"].as_u64().unwrap() as usize;
            assert!(off + len <= blen, "bufferView {off}+{len} overruns BIN ({blen})");
        }
        assert!(!json["meshes"][0]["primitives"].as_array().unwrap().is_empty());

        let man: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join("manifest.json")).unwrap()).unwrap();
        assert_eq!(man["source"]["parts"].as_array().unwrap().len(), parts.len());
        assert!(man["counts"]["vertices"].as_u64().unwrap() > 0);
        assert_eq!(
            man["counts"]["bones"].as_u64().unwrap() as usize,
            json["nodes"].as_array().unwrap().len() - 1,
            "manifest bone count must match the glTF node tree (bones + the mesh node)"
        );

        let _ = std::fs::remove_dir_all(&out);
    }

    #[test]
    fn sibling_maps_share_a_stem() {
        assert_eq!(stem("Sean_Head_D"), stem("Sean_Head_N"));
        assert_eq!(stem("Sean_Head_D"), stem("SEAN_HEAD_SM"));
        assert_ne!(stem("Sean_Head_D"), stem("Sean_Hand_D"));
    }
}
