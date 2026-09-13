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
use crate::resolve::{Prov, SubmeshBinding, TexAsset};

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
    /// Per-submesh per-slot binding (0=diffuse, 1=spec, 2=normal, 3=mask). See
    /// `resolve::SubmeshBinding` — this is what `wsao::Wsao::textures` returns for each material.
    pub submesh_tex: Vec<SubmeshBinding>,
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

    // ---- textures/ : every asset bound to any slot, plus name-stem siblings ----
    // The picker/WSAO path fills all four slots when it can, so this iterates the FULL binding, not
    // just the diffuse. The stem-sibling pull-in below is kept as a fallback for the name-suffix
    // heuristic path (`_D` has no WSMA record and its `_NM`/`_MASK` siblings would otherwise miss).
    let mut want: Vec<usize> =
        job.submesh_tex.iter().flat_map(|b| b.iter_bound().map(|(_, a)| a)).collect();
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

    let slot_name_and_file = |ai: Option<usize>| {
        let name = ai.and_then(|x| job.assets.get(x)).map(|t| t.name.clone());
        let file = ai.and_then(|x| file_of.get(&x)).map(|f| format!("textures/{f}"));
        (name, file)
    };

    let submeshes: Vec<_> = job
        .submeshes
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let binding = job.submesh_tex.get(i).copied().unwrap_or_default();
            let (d_name, d_file) = slot_name_and_file(binding.diffuse());
            let (s_name, s_file) = slot_name_and_file(binding.spec());
            let (n_name, n_file) = slot_name_and_file(binding.normal());
            let (m_name, m_file) = slot_name_and_file(binding.mask());
            serde_json::json!({
                "index": i,
                "index_start": s.index_start,
                "index_count": s.index_count,
                "material_hashes": s.materials.iter().map(|m| format!("0x{m:08X}")).collect::<Vec<_>>(),
                // Kept as-is so downstream tools that only knew about the diffuse binding still see
                // the field they expect.
                "texture": d_name.clone(),
                "texture_file": d_file.clone(),
                "slots": {
                    "diffuse":  { "texture": d_name, "file": d_file },
                    "spec":     { "texture": s_name, "file": s_file },
                    "normal":   { "texture": n_name, "file": n_file },
                    "mask":     { "texture": m_name, "file": m_file },
                },
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
    submesh_tex: &[SubmeshBinding],
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
    //
    // One image + one texture per written PNG; one material per DISTINCT SubmeshBinding — the
    // (diffuse, spec, normal, mask) tuple. Keying on the full binding (instead of just the diffuse,
    // as this once did) is what makes normal/spec bindings actually reach glTF: two submeshes that
    // share a diffuse but differ in their normal would otherwise be collapsed into one baseColor-only
    // material. glTF 2.0's core PBR has slots for baseColor and normal — spec and mask have no
    // dedicated slot under metallic-roughness, so they ride in `extras` (a modder or a Blender
    // import script can find them without needing a vendor extension).
    let mut images: Vec<serde_json::Value> = Vec::new();
    let mut textures: Vec<serde_json::Value> = Vec::new();
    let mut tex_index: BTreeMap<usize, usize> = BTreeMap::new();
    for (t, w) in written.iter().enumerate() {
        images.push(serde_json::json!({ "uri": format!("textures/{}", w.file) }));
        textures.push(serde_json::json!({ "source": t }));
        tex_index.insert(w.asset, t);
    }
    let file_of: BTreeMap<usize, &str> = written.iter().map(|w| (w.asset, w.file.as_str())).collect();

    let mut materials: Vec<serde_json::Value> = Vec::new();
    let mut mat_of: BTreeMap<SubmeshBinding, usize> = BTreeMap::new();
    let cover: Vec<SubMesh> = if submeshes.is_empty() {
        vec![SubMesh { index_start: 0, index_count: ni as u32, materials: Vec::new() }]
    } else {
        submeshes.to_vec()
    };
    for (i, _s) in cover.iter().enumerate() {
        let Some(binding) = submesh_tex.get(i).copied() else { continue };
        if !binding.is_bound() {
            continue;
        }
        if mat_of.contains_key(&binding) {
            continue;
        }
        let mut name_parts: Vec<String> = Vec::new();
        let mut pbr = serde_json::json!({ "metallicFactor": 0.0, "roughnessFactor": 1.0 });
        let mut extras = serde_json::Map::new();
        let mut mat = serde_json::json!({ "doubleSided": true });
        if let Some(t) = binding.diffuse().and_then(|a| tex_index.get(&a).copied()) {
            pbr["baseColorTexture"] = serde_json::json!({ "index": t });
            if let Some(f) = binding.diffuse().and_then(|a| file_of.get(&a)) {
                name_parts.push(f.trim_end_matches(".png").into());
            }
        }
        if let Some(t) = binding.normal().and_then(|a| tex_index.get(&a).copied()) {
            mat["normalTexture"] = serde_json::json!({ "index": t });
        }
        if let Some(t) = binding.spec().and_then(|a| tex_index.get(&a).copied()) {
            extras.insert("specularTexture".into(), serde_json::json!({ "index": t }));
        }
        if let Some(t) = binding.mask().and_then(|a| tex_index.get(&a).copied()) {
            extras.insert("maskTexture".into(), serde_json::json!({ "index": t }));
        }
        mat["pbrMetallicRoughness"] = pbr;
        if !extras.is_empty() {
            mat["extras"] = serde_json::Value::Object(extras);
        }
        if name_parts.is_empty() {
            name_parts.push(format!("material_{}", materials.len()));
        }
        mat["name"] = serde_json::json!(name_parts.join("_"));
        mat_of.insert(binding, materials.len());
        materials.push(mat);
    }

    // ---- primitives: one per submesh, sharing the vertex accessors ----
    let mut prims: Vec<serde_json::Value> = Vec::new();
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
        if let Some(m) = submesh_tex.get(i).and_then(|b| mat_of.get(b)) {
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
        // Submesh 0 binds asset 7 (diffuse only), submesh 1 binds nothing.
        let written = vec![WrittenTex { asset: 7, file: "skin.png".into() }];
        let bindings = [SubmeshBinding::diffuse_only(7), SubmeshBinding::default()];
        let glb = write_glb(&lm, "T", &cover, &bindings, &written);
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
        let Some(s) = crate::settings::detected_retail() else {
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

    /// Mimic a user clicking a SINGLE Sean part in the Parts panel: load one MSHA, resolve
    /// its per-submesh WSMA bindings, and report which slots (diffuse/spec/normal/mask) got
    /// filled with a real asset index. This is the empirical test for the user's complaint
    /// that the 3D viewer doesn't shade normal maps for a selected part.
    #[test]
    fn single_part_click_resolves_all_slots() {
        let Some(s) = crate::settings::detected_retail() else {
            eprintln!("skip: no Saboteur install detected");
            return;
        };
        let Ok(mp) = crate::pack::Megapack::open(&s.megapack()) else {
            eprintln!("skip: megapack unavailable");
            return;
        };
        let Ok(wsao) = crate::wsao::Wsao::open(&s.wsao()) else {
            eprintln!("skip: France.materials unavailable");
            return;
        };
        // Pick just Sean's UB (upper body) — one part, mirrors a Parts-panel click.
        let list = crate::meshload::list_meshes(&mp);
        let Some(part) = list.iter().find(|e| e.name == "CH_AL_SeanDevlin_01_UB").cloned() else {
            eprintln!("skip: no CH_AL_SeanDevlin_01_UB in this pack");
            return;
        };
        let picked = vec![part.clone()];
        let lm = crate::meshload::assemble(mp.raw(), &picked).expect("assemble");
        let submeshes: Vec<SubMesh> =
            crate::formats::submesh_cover(&lm.mesh.prims, lm.mesh.indices.len() as u32);

        use std::collections::{HashMap, HashSet};
        let want: Vec<[Option<u32>; SubmeshBinding::NUM_SLOTS]> = submeshes
            .iter()
            .map(|sm| {
                let mut slots = [None; SubmeshBinding::NUM_SLOTS];
                if let Some(mat) = sm.materials.first().copied() {
                    if let Some(hashes) = wsao.textures(mat) {
                        for (i, h) in hashes.iter().take(SubmeshBinding::NUM_SLOTS).enumerate() {
                            if *h != 0 {
                                slots[i] = Some(*h);
                            }
                        }
                    }
                }
                slots
            })
            .collect();
        let mut needed: HashSet<u32> =
            want.iter().flat_map(|s| s.iter().copied()).flatten().collect();
        let own = mp.entry_containing(part.file_off).map(|e| mp.slice(&e).to_vec()).unwrap_or_default();
        let mut found: HashMap<u32, TexAsset> = HashMap::new();
        crate::resolve::take_hashes_from_slices(&[&own], &mut needed, &mut found);
        let after_own = needed.len();
        crate::resolve::take_hashes_from_packs(&[&mp], &mut needed, &mut found);
        let after_sweep = needed.len();

        // Now report: how many submeshes had a normal-slot hash requested, and how many got resolved.
        let normal_requested = want.iter().filter(|s| s[SubmeshBinding::NORMAL].is_some()).count();
        let normal_hashes: Vec<u32> = want
            .iter()
            .filter_map(|s| s[SubmeshBinding::NORMAL])
            .collect();
        let normal_resolved =
            normal_hashes.iter().filter(|h| found.contains_key(h)).count();
        println!(
            "\n=== single-part resolve probe (CH_AL_SeanDevlin_01_UB) ===\n\
             submeshes:            {}\n\
             normal hashes wanted: {} distinct\n\
             normal hashes found:  {} / {}\n\
             hashes unresolved after own-slice:  {}\n\
             hashes unresolved after pack sweep: {}\n\
             assets in pool: {}",
            submeshes.len(),
            normal_hashes.len(),
            normal_resolved,
            normal_hashes.len(),
            after_own,
            after_sweep,
            found.len(),
        );
        // The bug the user reports: if normal_requested > 0 and normal_resolved == 0, the shader
        // never gets a normal-slot upload for a single-part load. Assert one direction of that so
        // the test fails when a regression sneaks in.
        if normal_requested > 0 {
            assert!(
                normal_resolved > 0,
                "WSMA asked for {normal_requested} normal-slot textures, but zero resolved — the \
                 3D viewer can't shade normals it can't find"
            );
        }
    }

    #[test]
    fn sibling_maps_share_a_stem() {
        assert_eq!(stem("Sean_Head_D"), stem("Sean_Head_N"));
        assert_eq!(stem("Sean_Head_D"), stem("SEAN_HEAD_SM"));
        assert_ne!(stem("Sean_Head_D"), stem("Sean_Hand_D"));
    }

    /// End-to-end against the installed game: assemble Sean, bind him via WSAO's full slot
    /// vector, run the bundle export, and prove that normal + spec (when the material has them)
    /// actually land in `textures/` and are wired into the glTF material as `normalTexture` /
    /// `extras.specularTexture`. This is the invariant the earlier diffuse-only export missed —
    /// `_NM` and `_SM` records existed in the pool but never reached the file system.
    #[test]
    fn bundle_exports_normal_and_spec_when_material_has_them() {
        let Some(s) = crate::settings::detected_retail() else {
            eprintln!("skip: no Saboteur install detected");
            return;
        };
        let Ok(mp) = crate::pack::Megapack::open(&s.megapack()) else {
            eprintln!("skip: megapack unavailable");
            return;
        };
        let Ok(wsao) = crate::wsao::Wsao::open(&s.wsao()) else {
            eprintln!("skip: France.materials unavailable");
            return;
        };
        let list = crate::meshload::list_meshes(&mp);
        let parts: Vec<MeshEntry> = list
            .iter()
            .filter(|e| e.name.to_ascii_lowercase().contains("seandevlin"))
            .take(3)
            .cloned()
            .collect();
        if parts.is_empty() {
            eprintln!("skip: no Sean parts");
            return;
        }
        let lm = crate::meshload::assemble(mp.raw(), &parts).expect("assemble");
        let submeshes: Vec<SubMesh> =
            crate::formats::submesh_cover(&lm.mesh.prims, lm.mesh.indices.len() as u32);

        // Resolve every WSMA slot for every submesh (mirrors app::resolve_model_textures).
        use std::collections::{HashMap, HashSet};
        let want: Vec<[Option<u32>; SubmeshBinding::NUM_SLOTS]> = submeshes
            .iter()
            .map(|sm| {
                let mut slots = [None; SubmeshBinding::NUM_SLOTS];
                if let Some(mat) = sm.materials.first().copied() {
                    if let Some(hashes) = wsao.textures(mat) {
                        for (i, h) in hashes.iter().take(SubmeshBinding::NUM_SLOTS).enumerate() {
                            if *h != 0 {
                                slots[i] = Some(*h);
                            }
                        }
                    }
                }
                slots
            })
            .collect();
        let mut needed: HashSet<u32> =
            want.iter().flat_map(|s| s.iter().copied()).flatten().collect();
        if needed.is_empty() {
            eprintln!("skip: Sean's submeshes have no WSMA bindings in this pack");
            return;
        }
        let mut found: HashMap<u32, TexAsset> = HashMap::new();
        let own: Vec<Vec<u8>> = parts
            .iter()
            .filter_map(|p| mp.entry_containing(p.file_off).map(|e| mp.slice(&e).to_vec()))
            .collect();
        let own_ref: Vec<&[u8]> = own.iter().map(|v| v.as_slice()).collect();
        crate::resolve::take_hashes_from_slices(&own_ref, &mut needed, &mut found);
        if !needed.is_empty() {
            crate::resolve::take_hashes_from_packs(&[&mp], &mut needed, &mut found);
        }
        let mut assets: Vec<TexAsset> = Vec::new();
        let mut idx: HashMap<u32, usize> = HashMap::new();
        for (h, a) in found {
            let ai = assets.len();
            idx.insert(h, ai);
            assets.push(a);
        }
        let submesh_tex: Vec<SubmeshBinding> = want
            .iter()
            .map(|slot_hashes| {
                let mut b = SubmeshBinding::default();
                for (s, wh) in slot_hashes.iter().enumerate() {
                    if let Some(ai) = wh.and_then(|h| idx.get(&h).copied()) {
                        b.slots[s] = Some(ai);
                    }
                }
                b
            })
            .collect();

        // Only bother if the pool actually contains a normal or spec — some outfits may not.
        let any_normal =
            submesh_tex.iter().any(|b| b.normal().is_some() || b.spec().is_some());
        if !any_normal {
            eprintln!("skip: no submesh in this outfit has a normal/spec binding — cannot verify");
            return;
        }

        let out = std::env::temp_dir().join("sab_workshop_bundle_slots_test");
        let _ = std::fs::remove_dir_all(&out);
        let submesh_prov = vec![Prov::Bound; submeshes.len()];
        let job = Job {
            megapack: s.megapack(),
            label: "SeanSlotsTest".into(),
            parts: parts.clone(),
            submeshes: submeshes.clone(),
            assets,
            submesh_tex: submesh_tex.clone(),
            submesh_prov,
            outroot: out.clone(),
        };
        let dir = PathBuf::from(run(&job).expect("export"));

        // The textures folder should now hold at least one `_NM` or `_SM` PNG — before the port,
        // only `_D` files ever appeared here.
        let tex_dir = dir.join("textures");
        let files: Vec<String> = std::fs::read_dir(&tex_dir)
            .expect("textures dir")
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        let has_nm = files.iter().any(|f| f.to_lowercase().contains("_nm"));
        let has_sm = files.iter().any(|f| f.to_lowercase().contains("_sm") || f.to_lowercase().contains("_s."));
        assert!(
            has_nm || has_sm,
            "textures/ has no _NM or _SM PNG — the slot 1/2 harvest is not reaching disk. Files: {files:?}"
        );

        // The glTF should carry `normalTexture` OR `extras.specularTexture` on at least one
        // material — proof the widened material writer is wired up, not just the file writer.
        let glb = std::fs::read(dir.join("model.glb")).expect("model.glb");
        let (json, _) = chunks(&glb);
        let mats = json["materials"].as_array().cloned().unwrap_or_default();
        let mat_has_normal = mats.iter().any(|m| m.get("normalTexture").is_some());
        let mat_has_spec = mats
            .iter()
            .any(|m| m.get("extras").and_then(|e| e.get("specularTexture")).is_some());
        assert!(
            mat_has_normal || mat_has_spec,
            "no glTF material carries a normalTexture or extras.specularTexture — the slot 1/2 \
             wiring in write_glb is not hooked up. Materials: {mats:#?}"
        );

        let _ = std::fs::remove_dir_all(&out);
    }
}
