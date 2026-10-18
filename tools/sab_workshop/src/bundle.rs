//! LOSSLESS model-export bundle — the community-editable interchange for a Saboteur asset, the same
//! for every build the workshop reads.
//!
//! Ported from `mercs2_workshop`'s `src/bundle.rs`, which solved this problem first; the rule it
//! encodes is the reason to take it wholesale rather than grow another one-button `.glb` writer.
//!
//! **The preservation rule: nothing is discarded.** An editable format can only carry what it
//! understands, and we have NOT fully reversed everything a mesh record holds (stream/format tables
//! beyond the vertex semantics we decode, per-drawcall damage material passes, the material hashes we
//! cannot resolve to a name). So the bundle keeps BOTH:
//!
//!   * `raw/`          — each source part's ORIGINAL record, verbatim, as the build stores it (retail:
//!                       `.msha`, the 276-byte header plus both still-deflated blobs; 2008: `.zlib` or
//!                       `.variant`). This is the guarantee: whatever we failed to understand is still
//!                       here, byte-exact.
//!   * `model.glb`     — the EDITABLE view: geometry + the real bone hierarchy + skin, one mesh
//!                       object per level of detail, one primitive per submesh, each bound to its
//!                       material. Every DCC reads glTF 2.0.
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
use std::sync::Arc;

use crate::formats::SubMesh;
use crate::meshload::LoadedMesh;
use crate::resolve::{Prov, SubmeshBinding, TexAsset};
use crate::source::GameSource;

/// Everything an export needs, OWNED — so it can be handed to a worker thread.
///
/// The worker re-assembles the parts from the source and touches no GPU state. The texture pool and
/// the submesh bindings are cloned rather than re-resolved because the user may have hand-bound
/// slots on the Materials panel: the bundle has to reflect what is on screen.
pub struct Job {
    pub source: Arc<dyn GameSource>,
    /// Asset name — the bundle directory and the glTF mesh name.
    pub label: String,
    /// The mesh items assembled, each at its level of detail (`assets::Asset::assembly`).
    pub picks: Vec<(usize, u8)>,
    /// What the assembly could not include, written to the manifest so a bundle missing a part says
    /// so.
    pub lod_notes: Vec<String>,
    /// The asset's damage parts (`assets::Asset::damage`).
    pub damage: Vec<(u32, i32)>,
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

/// The submesh cover the GLB is written from: the renderer's, or — when the caller passed none —
/// the model's own.
fn cover(submeshes: &[SubMesh], lm: &LoadedMesh) -> Vec<SubMesh> {
    if !submeshes.is_empty() {
        return submeshes.to_vec();
    }
    crate::formats::submesh_cover(&lm.mesh.prims, lm.mesh.indices.len() as u32)
}

/// One glTF object: a level of detail, in one damage state when the model has damage states.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct View {
    lod: u8,
    damage: Option<i32>,
}

impl View {
    /// Whether a submesh draws in this view: its level, and either no damage state or this one.
    fn shows(self, s: &SubMesh) -> bool {
        s.lod == self.lod && (s.damage.is_none() || s.damage == self.damage)
    }
}

/// The views, ascending: per level, one per damage state its submeshes draw in, else one.
fn views_of(cover: &[SubMesh]) -> Vec<View> {
    let mut levels: Vec<u8> = cover.iter().map(|s| s.lod).collect();
    levels.sort_unstable();
    levels.dedup();
    let mut out = Vec::new();
    for lod in levels {
        let mut states: Vec<i32> = cover.iter().filter(|s| s.lod == lod).filter_map(|s| s.damage).collect();
        states.sort_unstable();
        states.dedup();
        if states.is_empty() {
            out.push(View { lod, damage: None });
        }
        out.extend(states.into_iter().map(|d| View { lod, damage: Some(d) }));
    }
    out
}

/// The glTF node + mesh name for one view — `<label>_LOD0`, or `<label>_LOD0_state1` for a model
/// with damage states. Every view is a separate object on the same skin, so a DCC shows them as
/// siblings under one armature.
fn object_name(label: &str, v: View) -> String {
    match v.damage {
        None => format!("{label}_LOD{}", v.lod),
        Some(d) => format!("{label}_LOD{}_state{d}", v.lod),
    }
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

/// One `raw/` file: the part it holds, its path in the bundle, its size and where the build stores it.
struct RawFile {
    mesh: usize,
    file: String,
    bytes: usize,
    location: String,
}

/// Run a full bundle export. Returns the directory written.
///
/// Safe to call from a worker thread: it re-assembles from the source and touches no GPU state.
pub fn run(job: &Job) -> Result<String, String> {
    let src = job.source.as_ref();
    // The same assembly the viewer shows: a part that does not decode is missing from both, and the
    // manifest names it.
    let asm = crate::assets::Assembly { picks: job.picks.clone(), notes: job.lod_notes.clone(), damage: job.damage.clone() };
    let model = crate::boot::assemble(src, &job.label, &asm)?;
    let (lm, notes) = (model.lm, model.notes);

    let dir = job.outroot.join(sanitize(&job.label));
    std::fs::create_dir_all(dir.join("raw")).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(dir.join("textures")).map_err(|e| e.to_string())?;

    // ---- raw/ : the byte-exact source records ----
    // One file per MESH: a template whose `LowDetailModel` is its `HighDetailModel` (Sean's) picks
    // the same record at both levels, and it is the same bytes either way.
    let mut raw_files: Vec<RawFile> = Vec::new();
    for &(mesh, _) in &job.picks {
        if raw_files.iter().any(|r| r.mesh == mesh) {
            continue;
        }
        let rec = src.raw_part(mesh)?;
        let file = sanitize(&rec.file);
        std::fs::write(dir.join("raw").join(&file), &rec.bytes).map_err(|e| e.to_string())?;
        raw_files.push(RawFile { mesh, file: format!("raw/{file}"), bytes: rec.bytes.len(), location: rec.location });
    }

    // ---- textures/ : every asset bound to any slot ----
    let mut want: Vec<usize> =
        job.submesh_tex.iter().flat_map(|b| b.iter_bound().map(|(_, a)| a)).collect();
    want.sort_unstable();
    want.dedup();

    let mut written: Vec<WrittenTex> = Vec::new();
    let mut tex_errs: Vec<String> = Vec::new();
    for &i in &want {
        let Some(a) = job.assets.get(i) else { continue };
        match a.decode() {
            Ok(t) => {
                // Two different records can share a name (2008: copies of a name in different
                // packs); the second is filed under its pool index too.
                let stem = sanitize(&a.name);
                let file = if written.iter().any(|w| w.file == format!("{stem}.png")) { format!("{stem}~{i}.png") } else { format!("{stem}.png") };
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
    let manifest = manifest(job, &lm, &written, &raw_files, &tex_errs, &notes, glb.len());
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
    raw_files: &[RawFile],
    tex_errs: &[String],
    notes: &[String],
    glb_len: usize,
) -> serde_json::Value {
    let file_of: BTreeMap<usize, &str> = written.iter().map(|w| (w.asset, w.file.as_str())).collect();

    // One entry per part AS ASSEMBLED — a mesh used at two levels is listed once per range.
    let parts: Vec<_> = lm
        .part_ranges
        .iter()
        .map(|r| {
            let raw = raw_files.iter().find(|f| f.mesh == r.mesh);
            serde_json::json!({
                "name": r.name, "lod": r.lod,
                "location": raw.map(|f| f.location.clone()),
                "raw": raw.map(|f| f.file.clone()), "raw_bytes": raw.map(|f| f.bytes),
                "index_start": r.index_start, "index_count": r.index_count,
            })
        })
        .collect();

    // Per level and damage state: the glTF mesh object it became and its size.
    let cov = cover(&job.submeshes, lm);
    let mut lods: Vec<_> = Vec::new();
    for v in views_of(&cov) {
        let tris: u32 = cov.iter().filter(|s| v.shows(s)).map(|s| s.index_count).sum::<u32>() / 3;
        let names: Vec<&str> =
            lm.part_ranges.iter().filter(|r| r.lod == v.lod).map(|r| r.name.as_str()).collect();
        lods.push(serde_json::json!({
            "lod": v.lod, "damage_state": v.damage, "object": object_name(&job.label, v), "triangles": tris, "parts": names,
        }));
    }

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
                "lod": s.lod,
                "damage_state": s.damage,
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
        // 3: `source` names the build and each part's location, one entry per part and level.
        "bundle_version": 3,
        "label": job.label,
        "source": { "build": job.source.version().label(), "root": job.source.label(), "parts": parts },
        "lods": lods,
        "lod_notes": notes,
        "damage_parts": job.damage.iter().map(|(b, s)| serde_json::json!({ "bone": format!("0x{b:08X}"), "state": s })).collect::<Vec<_>>(),
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

/// Serialize the assembled character as a skinned bind-pose `.glb` (glTF 2.0 binary): skeleton +
/// skin + ONE MESH OBJECT PER LEVEL OF DETAIL (`<label>_LOD<n>`, all on skin 0) + ONE PRIMITIVE PER
/// SUBMESH, each bound to the material its texture came from.
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
    if nv == 0 {
        // A skeleton-only record: the bone hierarchy and no mesh (glTF has no empty buffer views).
        let roots: Vec<usize> = (0..nb).filter(|&b| lm.bones[b].parent < 0).collect();
        let nodes: Vec<serde_json::Value> = (0..nb)
            .map(|b| {
                let bone = &lm.bones[b];
                let children: Vec<usize> = (0..nb).filter(|&c| lm.bones[c].parent == b as i32).collect();
                let mut n = serde_json::json!({ "name": bone.name, "translation": bone.t.to_vec(), "rotation": bone.r.to_vec(), "scale": bone.s.to_vec() });
                if !children.is_empty() {
                    n["children"] = serde_json::json!(children);
                }
                n
            })
            .collect();
        let root = serde_json::json!({
            "asset": { "version": "2.0", "generator": "sab_workshop" },
            "scene": 0,
            "scenes": [{ "name": label, "nodes": roots }],
            "nodes": nodes,
        });
        return container(&root, Vec::new());
    }

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
    let cover: Vec<SubMesh> = cover(submeshes, lm);
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

    // ---- meshes: one per view (level of detail, damage state); primitives: one per submesh ----
    //
    // All views share the vertex accessors, but a view's primitives go in ITS mesh, so LOD1 is a
    // separate object rather than a second skin drawn over LOD0, and a damaged state is a separate
    // object rather than wreckage drawn over the intact model.
    let levels = views_of(&cover);
    let mut meshes: Vec<serde_json::Value> = Vec::new();
    for &v in &levels {
        let mut prims: Vec<serde_json::Value> = Vec::new();
        for (i, s) in cover.iter().enumerate() {
            if s.index_count == 0 || !v.shows(s) {
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
        meshes.push(serde_json::json!({ "name": object_name(label, v), "primitives": prims }));
    }

    // ---- nodes: the bone hierarchy, then one mesh node per level ----
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
    // ---- scenes: one per view, each the skeleton's roots + that view's mesh node; scene 0 (LOD0,
    // intact) is the default ----
    let mut scenes: Vec<serde_json::Value> = Vec::new();
    for (m, &v) in levels.iter().enumerate() {
        let mut mn = serde_json::json!({ "name": object_name(label, v), "mesh": m });
        if has_skin {
            mn["skin"] = serde_json::json!(0);
        }
        let mut scene_nodes: Vec<usize> = roots.clone();
        scene_nodes.push(nodes.len());
        nodes.push(mn);
        let name = match v.damage {
            None => format!("LOD{}", v.lod),
            Some(d) => format!("LOD{} state {d}", v.lod),
        };
        scenes.push(serde_json::json!({ "name": name, "nodes": scene_nodes }));
    }

    let mut root = serde_json::json!({
        "asset": { "version": "2.0", "generator": "sab_workshop" },
        "scene": 0,
        "scenes": scenes,
        "nodes": nodes,
        "meshes": meshes,
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

    container(&root, bin)
}

/// The GLB container: the JSON chunk, then the BIN chunk when there is one.
fn container(root: &serde_json::Value, mut bin: Vec<u8>) -> Vec<u8> {
    let mut json_bytes = root.to_string().into_bytes();
    while json_bytes.len() % 4 != 0 {
        json_bytes.push(b' ');
    }
    while bin.len() % 4 != 0 {
        bin.push(0);
    }
    let bin_chunk = if bin.is_empty() { 0 } else { 8 + bin.len() };
    let total = 12 + 8 + json_bytes.len() + bin_chunk;
    let mut glb = Vec::with_capacity(total);
    glb.extend_from_slice(&0x4654_6C67u32.to_le_bytes()); // "glTF"
    glb.extend_from_slice(&2u32.to_le_bytes());
    glb.extend_from_slice(&(total as u32).to_le_bytes());
    glb.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
    glb.extend_from_slice(&0x4E4F_534Au32.to_le_bytes()); // "JSON"
    glb.extend_from_slice(&json_bytes);
    if !bin.is_empty() {
        glb.extend_from_slice(&(bin.len() as u32).to_le_bytes());
        glb.extend_from_slice(&0x004E_4942u32.to_le_bytes()); // "BIN\0"
        glb.extend_from_slice(&bin);
    }
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
            part_ranges: vec![range("T", 0, 6, 0)],
            uvless: Vec::new(),
        }
    }

    fn range(name: &str, index_start: u32, index_count: u32, lod: u8) -> crate::source::PartRange {
        crate::source::PartRange { mesh: 0, name: name.into(), index_start, index_count, lod }
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
            SubMesh { index_start: 0, index_count: 3, materials: vec![0xAA], lod: 0, damage: None },
            SubMesh { index_start: 3, index_count: 3, materials: vec![0xBB], lod: 0, damage: None },
        ];
        // Submesh 0 binds asset 7 (diffuse only), submesh 1 binds nothing.
        let written = vec![WrittenTex { asset: 7, file: "skin.png".into() }];
        let bindings = [SubmeshBinding { slots: [Some(7), None, None, None] }, SubmeshBinding::default()];
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

    /// End-to-end on both builds: export a real character with both levels of detail and its bound
    /// textures, and read the bundle back.
    ///
    /// The unit tests above prove the writer's shape from synthetic input; only this proves the
    /// thing a modder actually receives — that the parts assemble, the raw records round-trip, the
    /// textures land, the glTF's accessors stay inside the buffer they point at, and each level is
    /// its own object. The character is the first one whose templates name a separate
    /// `LowDetailModel` mesh, assembled exactly as a click assembles it.
    #[cfg(feature = "retail")]
    #[test]
    fn a_character_bundle_round_trips_on_both_builds() {
        for src in crate::testenv::both_shared() {
            character_bundle_round_trips(src);
        }
    }

    #[cfg(feature = "retail")]
    fn character_bundle_round_trips(source: Arc<dyn GameSource>) {
        let src = source.as_ref();
        let build = src.label();
        let assets = crate::assets::build(src);
        let (name, asm) = assets
            .iter()
            .filter(|a| a.category == "Characters")
            .map(|a| (a.name.clone(), a.assembly()))
            .find(|(_, asm)| {
                asm.picks.iter().any(|&(i, l)| l == 1 && !asm.picks.contains(&(i, 0)))
            })
            .unwrap_or_else(|| panic!("{build}: a character whose LowDetailModel is a separate mesh"));
        let model = crate::boot::assemble(src, &name, &asm).unwrap_or_else(|e| panic!("{build} {name}: {e}"));
        let submeshes = crate::formats::submesh_cover(&model.lm.mesh.prims, model.lm.mesh.indices.len() as u32);
        let mt = src.resolve_textures(&model.lm.part_ranges, &submeshes, &model.lm.uvless).unwrap_or_else(|e| panic!("{build} {name}: {e}"));
        assert!(mt.submesh_tex.iter().any(|b| b.diffuse().is_some()), "{build} {name}: no submesh binds a diffuse");

        let out = std::env::temp_dir().join(format!("sab_workshop_bundle_test_{:?}", src.version()));
        let _ = std::fs::remove_dir_all(&out);
        let job = Job {
            source: source.clone(),
            label: "LodTest".into(),
            picks: asm.picks.clone(),
            lod_notes: asm.notes.clone(),
            damage: asm.damage.clone(),
            submeshes: submeshes.clone(),
            assets: mt.assets.clone(),
            submesh_tex: mt.submesh_tex.clone(),
            submesh_prov: mt.submesh_prov.clone(),
            outroot: out.clone(),
        };
        let dir = PathBuf::from(run(&job).unwrap_or_else(|e| panic!("{build}: export {name}: {e}")));

        // The raw records are the preservation promise: byte-identical to the build.
        for &(mesh, _) in &asm.picks {
            let want = src.raw_part(mesh).expect("raw record");
            let got = std::fs::read(dir.join("raw").join(sanitize(&want.file))).expect("raw file");
            assert!(got == want.bytes, "{build}: raw/{} is not byte-exact", want.file);
        }
        // Every bound texture is a PNG in textures/.
        let man: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join("manifest.json")).unwrap()).unwrap();
        assert_eq!(man["texture_errors"].as_array().map(Vec::len), Some(0), "{build}: {}", man["texture_errors"]);
        let bound: std::collections::BTreeSet<usize> = mt.submesh_tex.iter().flat_map(|b| b.iter_bound().map(|(_, a)| a)).collect();
        let pngs = std::fs::read_dir(dir.join("textures")).expect("textures dir").count();
        assert_eq!(pngs, bound.len(), "{build}: one PNG per bound texture");

        // The GLB parses, and every accessor lands inside the BIN chunk it indexes.
        let glb = std::fs::read(dir.join("model.glb")).expect("model.glb");
        let (json, blen) = chunks(&glb);
        // A bound normal map reaches the glTF material.
        if mt.submesh_tex.iter().any(|b| b.normal().is_some()) {
            let mats = json["materials"].as_array().cloned().unwrap_or_default();
            assert!(mats.iter().any(|m| m.get("normalTexture").is_some()), "{build}: no glTF material carries the bound normal map");
        }
        let views = json["bufferViews"].as_array().unwrap();
        for v in views {
            let off = v["byteOffset"].as_u64().unwrap() as usize;
            let len = v["byteLength"].as_u64().unwrap() as usize;
            assert!(off + len <= blen, "bufferView {off}+{len} overruns BIN ({blen})");
        }

        assert_eq!(man["source"]["parts"].as_array().unwrap().len(), model.lm.part_ranges.len(), "{build} {name}");
        assert!(man["counts"]["vertices"].as_u64().unwrap() > 0);

        // One skinned mesh object per level, named for it, and each holding exactly its level's
        // triangles — LOD1 is beside LOD0, not inside it.
        let meshes = json["meshes"].as_array().unwrap();
        let lods = man["lods"].as_array().unwrap();
        assert!(meshes.len() >= 2, "{build} {name}: expected at least LOD0 and LOD1 objects");
        assert_eq!(meshes.len(), lods.len(), "{build} {name}: one object per level");
        let nodes = json["nodes"].as_array().unwrap();
        for (m, lod) in meshes.iter().zip(lods) {
            let obj = lod["object"].as_str().unwrap();
            assert_eq!(m["name"].as_str(), Some(obj));
            let node = nodes.iter().find(|n| n["name"].as_str() == Some(obj)).expect("mesh node");
            assert_eq!(node["skin"].as_u64(), Some(0), "{obj} must ride the one shared skin");
            let tris: u64 = m["primitives"]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| json["accessors"][p["indices"].as_u64().unwrap() as usize]["count"].as_u64().unwrap())
                .sum::<u64>()
                / 3;
            assert_eq!(tris, lod["triangles"].as_u64().unwrap(), "{name} {obj}");
            assert!(tris > 0, "{name} {obj} is empty");
        }
        assert_eq!(
            man["counts"]["bones"].as_u64().unwrap() as usize,
            nodes.len() - meshes.len(),
            "manifest bone count must match the glTF node tree (bones + one node per level)"
        );
        eprintln!("{name}: {}", serde_json::to_string(&man["lods"]).unwrap());

        let _ = std::fs::remove_dir_all(&out);
    }

    /// A model with damage states becomes one object per state, each holding the geometry that draws
    /// in every state plus its own state's — never the other state's.
    #[test]
    fn each_damage_state_is_its_own_mesh_object() {
        let mut lm = demo();
        lm.mesh.indices = vec![0, 1, 2, 1, 3, 2, 0, 1, 3];
        let cover = vec![
            SubMesh { index_start: 0, index_count: 3, materials: vec![0xAA], lod: 0, damage: None },
            SubMesh { index_start: 3, index_count: 3, materials: vec![0xBB], lod: 0, damage: Some(0) },
            SubMesh { index_start: 6, index_count: 3, materials: vec![0xCC], lod: 0, damage: Some(1) },
        ];
        let glb = write_glb(&lm, "T", &cover, &[], &[]);
        let (json, _) = chunks(&glb);
        let meshes = json["meshes"].as_array().unwrap();
        assert_eq!(meshes.len(), 2);
        assert_eq!(meshes[0]["name"].as_str(), Some("T_LOD0_state0"));
        assert_eq!(meshes[1]["name"].as_str(), Some("T_LOD0_state1"));
        for (m, own) in meshes.iter().zip([3u64, 6]) {
            let offs: Vec<u64> = m["primitives"]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| json["accessors"][p["indices"].as_u64().unwrap() as usize]["byteOffset"].as_u64().unwrap() / 4)
                .collect();
            assert_eq!(offs, [0, own], "{}", m["name"]);
        }
    }

    /// Two levels in one assembly become two mesh objects, and neither draws the other's indices.
    #[test]
    fn each_level_is_its_own_mesh_object() {
        let mut lm = demo();
        lm.part_ranges = vec![range("T", 0, 3, 0), range("T_LOD", 3, 3, 1)];
        let cover = vec![
            SubMesh { index_start: 0, index_count: 3, materials: vec![0xAA], lod: 0, damage: None },
            SubMesh { index_start: 3, index_count: 3, materials: vec![0xBB], lod: 1, damage: None },
        ];
        let glb = write_glb(&lm, "T", &cover, &[], &[]);
        let (json, _) = chunks(&glb);
        let meshes = json["meshes"].as_array().unwrap();
        assert_eq!(meshes.len(), 2);
        assert_eq!(meshes[0]["name"].as_str(), Some("T_LOD0"));
        assert_eq!(meshes[1]["name"].as_str(), Some("T_LOD1"));
        for (m, want_off) in meshes.iter().zip([0u64, 12]) {
            let prims = m["primitives"].as_array().unwrap();
            assert_eq!(prims.len(), 1);
            let a = prims[0]["indices"].as_u64().unwrap() as usize;
            assert_eq!(json["accessors"][a]["byteOffset"].as_u64(), Some(want_off));
            assert_eq!(json["accessors"][a]["count"].as_u64(), Some(3));
        }
        // One scene per level: the skeleton root plus that level's mesh node and not the other's,
        // both on skin 0; LOD0's scene is the default.
        let nodes = json["nodes"].as_array().unwrap();
        let scenes = json["scenes"].as_array().unwrap();
        assert_eq!(scenes.len(), 2);
        assert_eq!(json["scene"].as_u64(), Some(0));
        let node_of = |name: &str| nodes.iter().position(|n| n["name"].as_str() == Some(name)).unwrap() as u64;
        for (k, (own, other)) in [("T_LOD0", "T_LOD1"), ("T_LOD1", "T_LOD0")].into_iter().enumerate() {
            let sc: Vec<u64> = scenes[k]["nodes"].as_array().unwrap().iter().map(|n| n.as_u64().unwrap()).collect();
            assert_eq!(scenes[k]["name"].as_str(), Some(["LOD0", "LOD1"][k]));
            assert!(sc.contains(&0), "scene {k} lacks the skeleton root");
            assert!(sc.contains(&node_of(own)), "{own} not in scene {k}");
            assert!(!sc.contains(&node_of(other)), "{other} leaked into scene {k}");
            assert_eq!(nodes[node_of(own) as usize]["skin"].as_u64(), Some(0));
        }
    }

}
