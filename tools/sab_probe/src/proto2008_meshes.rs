//! `proto2008-meshes <bin360>` — read every `meshes` slot record of every loose `.Pack` under the 2008
//! prototype's `Bin360` (MP00 entries are byte copies of these, `proto2008-packs`), inflate the zlib
//! ones, parse each with `sab_formats::proto2008::mesh::parse_record`, and tally the facts
//! `docs/2008_msh4_format.md` cites: record kinds and magics, failures, stream formats, prim and bone
//! fields, tails, which half4 position streams are normalised to [0,1], the `UBYTE4N` tangent's byte
//! order against the `DEC3N` normal, each stream's model-space box (`Mesh::positions`) against its
//! bone's box, skin index ranges, whether each remap inverts its bone's bind transform, and how the
//! pack's `wsd` placement nodes relate to the bounds of the meshes they name.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use sab_formats::editnodes::{self, tags, Node};
use sab_formats::proto2008::mesh::{self, Element, Layout, Mesh, Record, Tail};
use sab_formats::proto2008::sbl::{self, SlotKind};
use sab_formats::proto2008::slot_dir;
use sab_formats::zlib;

fn files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
        let p = e.expect("dir entry").path();
        if p.is_dir() {
            files(&p, out);
        } else {
            out.push(p);
        }
    }
}

#[derive(Default)]
struct Tally {
    counts: BTreeMap<String, usize>,
    examples: BTreeMap<String, String>,
}

impl Tally {
    fn note(&mut self, key: String, at: &str) {
        self.examples.entry(key.clone()).or_insert_with(|| at.to_string());
        *self.counts.entry(key).or_default() += 1;
    }
}

fn kind_of(name: &str) -> &'static str {
    if name.contains("_Chunk") {
        "chunk"
    } else if name.contains("_baked_collision") {
        "baked collision"
    } else if name.contains("_baked_") {
        "baked"
    } else {
        "object"
    }
}

fn len3(v: [f32; 3]) -> f32 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

/// Mean |dot(tangent, normal)| over a stream for the two byte orders of the `UBYTE4N` tangent.
fn tangent_orders(s: &mesh::Stream, l: &Layout) -> Option<(f32, f32)> {
    let (Some((Element::Dec3N, no)), Some((Element::UByte4N, to))) = (l.normal, l.tangent) else { return None };
    let (mut fwd, mut rev, mut n) = (0f32, 0f32, 0usize);
    for v in s.vertices.chunks_exact(s.stride as usize) {
        let nn = mesh::floats(Element::Dec3N, v, no);
        let t = mesh::bytes4(v, to);
        let un = |b: u8| b as f32 / 255.0 * 2.0 - 1.0;
        let a = [un(t[0]), un(t[1]), un(t[2])];
        let r = [un(t[3]), un(t[2]), un(t[1])];
        let dot = |x: [f32; 3]| {
            let l = len3(x).max(1e-6);
            ((x[0] * nn[0] + x[1] * nn[1] + x[2] * nn[2]) / l).abs()
        };
        fwd += dot(a);
        rev += dot(r);
        n += 1;
    }
    (n > 0).then(|| (fwd / n as f32, rev / n as f32))
}

fn survey_mesh(m: &Mesh, at: &str, t: &mut Tally) {
    let kind = kind_of(&m.name);
    let magic = String::from_utf8_lossy(&m.magic).to_string();
    t.note(format!("bodies parsed: {magic} ({kind})"), at);
    t.note(format!("shadow_caster {} reserved {} ({kind})", m.shadow_caster, m.reserved), at);
    let tail = match m.tail {
        Tail::Hash(_) => "MSHC hash",
        Tail::Matrix(_) => "LE matrix",
        Tail::None => "none",
    };
    t.note(format!("tail {tail} ({kind})"), at);
    for s in &m.streams {
        t.note(format!("stream format {:08X} stride {} ({kind})", s.format, s.stride), &format!("{at} {}", m.name));
        let l = Layout::of(s.format).expect("parse checked the layout");
        if let Some((Element::Half4, o)) = l.position {
            let unit = s.vertices.chunks_exact(s.stride as usize).all(|v| {
                let p = mesh::floats(Element::Half4, v, o);
                p[..3].iter().all(|x| (-1e-3..=1.0 + 1e-3).contains(x))
            });
            t.note(format!("half4 positions all in [0,1]: {unit} ({kind})"), &format!("{at} {}", m.name));
            t.note(format!("half4 positions all in [0,1]: {unit}, stream has a normal: {}", l.normal.is_some()), &format!("{at} {} {:08X}", m.name, s.format));
        }
        if let (Some(b), Ok(ps)) = (m.bones.get(s.bone as usize), m.positions(s)) {
            let class = match l.position {
                Some((Element::UShort2, _)) => "terrain",
                Some((Element::Half4, _)) if l.normal.is_none() => "normalised",
                _ if l.skin.is_some() => "skinned",
                _ => "rigid",
            };
            let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
            for p in &ps {
                for k in 0..3 {
                    lo[k] = lo[k].min(p[k]);
                    hi[k] = hi[k].max(p[k]);
                }
            }
            let tol = |x: f32| 2e-3 + 2e-3 * x.abs();
            let axis = |k: usize| {
                let (bl, bh) = (b.centre[k] - b.half[k], b.centre[k] + b.half[k]);
                (lo[k] - bl).abs() <= tol(bl) && (hi[k] - bh).abs() <= tol(bh)
            };
            let fits = |k: usize| lo[k] >= b.centre[k] - b.half[k] - tol(b.centre[k] - b.half[k]) && hi[k] <= b.centre[k] + b.half[k] + tol(b.centre[k] + b.half[k]);
            let verdict = if (0..3).all(axis) {
                "equals the bone's box"
            } else if (0..3).all(fits) {
                "inside the bone's box"
            } else {
                "outside the bone's box"
            };
            if !ps.is_empty() {
                t.note(format!("model-space vertex box {verdict} ({class})"), &format!("{at} {} bone {} [{lo:?}..{hi:?}] box c {:?} h {:?}", m.name, s.bone, b.centre, &b.half[..3]));
                if verdict.starts_with("outside") {
                    if let Some((e, o)) = l.position.filter(|(e, _)| matches!(e, Element::Half4 | Element::Float3)) {
                        let (mut rlo, mut rhi) = ([f32::MAX; 3], [f32::MIN; 3]);
                        for v in s.vertices.chunks_exact(s.stride as usize) {
                            let p = mesh::floats(e, v, o);
                            for k in 0..3 {
                                rlo[k] = rlo[k].min(p[k]);
                                rhi[k] = rhi[k].max(p[k]);
                            }
                        }
                        let rfits = (0..3).all(|k| rlo[k] >= b.centre[k] - b.half[k] - tol(b.centre[k] - b.half[k]) && rhi[k] <= b.centre[k] + b.half[k] + tol(b.centre[k] + b.half[k]));
                        let excess: f32 = (0..3)
                            .map(|k| ((b.centre[k] - b.half[k]) - lo[k]).max(0.0).max(hi[k] - (b.centre[k] + b.half[k])))
                            .fold(0.0, f32::max);
                        let ex = if excess < 0.05 { "< 5 cm" } else if excess < 0.5 { "< 50 cm" } else { ">= 50 cm" };
                        t.note(
                            format!("outside ({class}): raw positions fit the box: {rfits}; largest excess {ex}; bones {}", if m.bones.len() > 1 { ">1" } else { "1" }),
                            &format!("{at} {} bone {}", m.name, s.bone),
                        );
                    }
                }
            }
        }
        if let Some((wo, io)) = l.skin {
            let (mut max_idx, mut wsum_ok) = (0u8, true);
            for v in s.vertices.chunks_exact(s.stride as usize) {
                let w = mesh::bytes4(v, wo);
                let i = mesh::bytes4(v, io);
                let sum: u32 = w.iter().map(|&x| x as u32).sum();
                if !(250..=260).contains(&sum) {
                    wsum_ok = false;
                }
                for k in 0..4 {
                    if w[k] != 0 {
                        max_idx = max_idx.max(i[k]);
                    }
                }
            }
            let rel = |x: usize| if (max_idx as usize) < x { "<" } else { ">=" };
            t.note(
                format!("skinned stream: weights sum to 255: {wsum_ok}; max weighted index {} remap count, {} bone count", rel(m.remaps.len()), rel(m.bones.len())),
                &format!("{at} {} max {max_idx} remaps {} bones {}", m.name, m.remaps.len(), m.bones.len()),
            );
        }
        if let Some((fwd, rev)) = tangent_orders(s, &l) {
            let better = if fwd < rev { "stored order" } else { "reversed order" };
            t.note(format!("tangent more perpendicular to the normal in {better}"), &format!("{at} {} mean|dot| {fwd:.3} vs {rev:.3}", m.name));
        }
        for p in &s.prims {
            t.note(format!("prim lod {} segment_bits {} ({kind}{})", p.lod, p.segment_bits, if m.name.contains("_DAM") { ", _DAM" } else { "" }), &format!("{at} {}", m.name));
        }
    }
    for b in &m.bones {
        t.note(format!("bone export_flags {} ({kind})", b.export_flags), &format!("{at} {}", m.name));
    }
    for r in &m.remaps {
        let Some(_) = m.bones.get(r.bone as usize) else {
            t.note("remap bone index past the bone list".into(), &format!("{at} {}", m.name));
            continue;
        };
        let o = m.bone_to_model(r.bone as usize, [0.0; 3]);
        let ex = m.bone_to_model(r.bone as usize, [1.0, 0.0, 0.0]);
        let ey = m.bone_to_model(r.bone as usize, [0.0, 1.0, 0.0]);
        let ez = m.bone_to_model(r.bone as usize, [0.0, 0.0, 1.0]);
        let mm = r.matrix;
        let apply_rows = |p: [f32; 3]| [0, 1, 2].map(|i| mm[4 * i] * p[0] + mm[4 * i + 1] * p[1] + mm[4 * i + 2] * p[2] + mm[4 * i + 3]);
        let apply_cols = |p: [f32; 3]| [0, 1, 2].map(|i| mm[i] * p[0] + mm[4 + i] * p[1] + mm[8 + i] * p[2] + mm[12 + i]);
        let close = |a: [f32; 3], b: [f32; 3]| (0..3).all(|k| (a[k] - b[k]).abs() < 2e-3);
        let as_rows = close(apply_rows(o), [0.0; 3]) && close(apply_rows(ex), [1.0, 0.0, 0.0]) && close(apply_rows(ey), [0.0, 1.0, 0.0]) && close(apply_rows(ez), [0.0, 0.0, 1.0]);
        let as_cols = close(apply_cols(o), [0.0; 3]) && close(apply_cols(ex), [1.0, 0.0, 0.0]) && close(apply_cols(ey), [0.0, 1.0, 0.0]) && close(apply_cols(ez), [0.0, 0.0, 1.0]);
        let verdict = match (as_rows, as_cols) {
            (true, true) => "both orders (identity-like)",
            (true, false) => "row-vector order m[4r+c] with translation in m[3,7,11]",
            (false, true) => "column order with translation in m[12..15]",
            (false, false) => "neither order",
        };
        t.note(format!("remap matrix inverts the bone's bind world: {verdict} ({kind})"), &format!("{at} {} bone {} {:?}", m.name, r.bone, mm));
    }
}

/// Compare each `wsd` object named like a unit-cube mesh with that mesh's stream bone bounds.
fn survey_placement(wsd: &[&[u8]], meshes: &HashMap<String, Mesh>, at: &str, t: &mut Tally) {
    for slot in wsd {
        let objs = match editnodes::parse_entry_be(slot) {
            Ok(o) => o,
            Err(e) => {
                t.note(format!("FAIL wsd parse: {e}"), at);
                continue;
            }
        };
        t.note("wsd slots parsed".into(), at);
        for o in &objs {
            let Node::Container { children, .. } = o else { continue };
            let param = |tag: u32| {
                children.iter().find_map(|c| match c {
                    Node::Leaf { tag: tg, data } if *tg == tag => Some(data.as_slice()),
                    _ => None,
                })
            };
            let Some(name) = param(tags::CLASS_NAME).and_then(|d| d.split_last()).map(|(_, s)| String::from_utf8_lossy(s).to_string()) else {
                t.note("wsd object without ClassName".into(), at);
                continue;
            };
            let Some(m) = meshes.get(&name) else {
                t.note(format!("wsd object names no mesh of this pack ({})", kind_of(&name)), &format!("{at} {name}"));
                continue;
            };
            let v = |tag| param(tag).and_then(editnodes::vec3_be);
            let (Some(pos), Some(x), Some(y), Some(z)) = (v(tags::POSITION), v(tags::X_AXIS), v(tags::Y_AXIS), v(tags::Z_AXIS)) else {
                t.note(format!("wsd object lacks Position/axes ({})", kind_of(&name)), &format!("{at} {name}"));
                continue;
            };
            let kind = kind_of(&name);
            let bones: Vec<u32> = m.streams.iter().map(|s| s.bone).collect::<std::collections::BTreeSet<_>>().into_iter().collect();
            t.note(format!("placed mesh's streams use {} distinct bones ({kind})", bones.len().min(3)), &format!("{at} {name}"));
            let Some(b) = bones.first().and_then(|&i| m.bones.get(i as usize)) else { continue };
            let mut origin = [0f32; 3];
            let mut k = bones[0] as i32;
            while k >= 0 {
                let bb = &m.bones[k as usize];
                for j in 0..3 {
                    origin[j] += bb.translation[j];
                }
                k = bb.parent;
            }
            let near = |a: [f32; 3], b: [f32; 3]| (0..3).all(|j| (a[j] - b[j]).abs() < 1e-2);
            let min = [b.centre[0] - b.half[0], b.centre[1] - b.half[1], b.centre[2] - b.half[2]];
            let rel = if near(pos, [0.0; 3]) {
                "zero"
            } else if near(pos, origin) {
                "the stream bone's chained translation"
            } else if near(pos, min) {
                "the stream bone's box minimum"
            } else if near(pos, m.bones[0].translation) {
                "bone 0's translation"
            } else {
                "none of zero / bone origin / box minimum"
            };
            t.note(format!("wsd Position == {rel} ({kind})"), &format!("{at} {name} pos {pos:?} origin {origin:?} min {min:?}"));
            let size = [2.0 * b.half[0], 2.0 * b.half[1], 2.0 * b.half[2]];
            let lens = [len3(x), len3(y), len3(z)];
            let axes_match = (0..3).all(|k| (lens[k] - size[k]).abs() <= 1e-3 * size[k].abs().max(1.0));
            let unit = (0..3).all(|k| (lens[k] - 1.0).abs() < 1e-3);
            t.note(format!("axis lengths == bone bound size: {axes_match}, unit axes: {unit} ({kind})"), &format!("{at} {name} lens {lens:?} size {size:?}"));
        }
    }
}

pub fn run(args: &[String]) {
    let [bin360] = args else {
        eprintln!("usage: sab_probe proto2008-meshes <Bin360 dir>");
        std::process::exit(2);
    };
    let root = PathBuf::from(bin360);
    let mut all = Vec::new();
    files(&root, &mut all);
    all.sort();
    let mut t = Tally::default();
    for p in &all {
        let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if !name.to_ascii_lowercase().ends_with(".pack") {
            continue;
        }
        let rel = p.strip_prefix(&root).expect("under root").to_string_lossy().replace('\\', "/");
        let buf = std::fs::read(p).unwrap_or_else(|e| panic!("{rel}: {e}"));
        if !buf.starts_with(b"SBL") {
            continue;
        }
        let sp = sbl::parse(&buf).unwrap_or_else(|e| panic!("{rel}: {e}"));
        let sbl_magic = String::from_utf8_lossy(&sp.magic).to_string();
        let mut meshes = HashMap::new();
        if let Some(slot) = sp.slot(SlotKind::Meshes) {
            let recs = slot_dir::parse(slot).unwrap_or_else(|e| panic!("{rel}: {e}"));
            for (ri, r) in recs.iter().enumerate() {
                if r.stored == 0 {
                    continue;
                }
                let at = format!("{rel}#{ri}");
                let data = &slot[r.offset as usize..(r.offset + r.stored) as usize];
                let body = if r.stored < r.unpacked {
                    match zlib::inflate_exact(data, r.unpacked as usize, &at) {
                        Ok(b) => b,
                        Err(e) => {
                            t.note(format!("FAIL inflate: {e}"), &at);
                            continue;
                        }
                    }
                } else {
                    data.to_vec()
                };
                let rec = match mesh::parse_record(&body) {
                    Ok(r) => r,
                    Err(e) => {
                        let msg = e.split(':').next_back().unwrap_or(&e).trim().to_string();
                        t.note(format!("FAIL parse: {msg}"), &format!("{at}: {e}"));
                        continue;
                    }
                };
                let m = rec.mesh();
                let rmagic = String::from_utf8_lossy(&m.magic).to_string();
                match &rec {
                    Record::Mesh(_) => t.note(format!("record: mesh {rmagic} in {sbl_magic} pack, stored {}", if r.stored < r.unpacked { "zlib" } else { "raw" }), &at),
                    Record::Variant { bindings, .. } => {
                        t.note(format!("record: variant ({} bindings) + {rmagic} in {sbl_magic} pack", bindings.len().min(4)), &at);
                        let mats_in_pack = bindings.len();
                        t.note(format!("variant bindings {mats_in_pack}"), &at);
                    }
                }
                let named = sab_formats::pandemic_hash(&m.name) == r.hash;
                t.note(format!("record hash == pandemic_hash(mesh name): {named} ({rmagic})"), &format!("{at} {} {:08X}", m.name, r.hash));
                if let Tail::Hash(h) = m.tail {
                    t.note(format!("record hash == MSHC tail hash: {} ({rmagic})", h == r.hash), &format!("{at} {} {:08X} {h:08X}", m.name, r.hash));
                }
                survey_mesh(m, &at, &mut t);
                meshes.insert(m.name.clone(), rec.mesh().clone());
            }
        }
        let wsd: Vec<&[u8]> = sp.slots_of(SlotKind::Wsd).map(|(_, s)| s).collect();
        if !wsd.is_empty() {
            survey_placement(&wsd, &meshes, &rel, &mut t);
        }
    }
    for (k, n) in &t.counts {
        println!("{n:>8}  {k}   e.g. {}", t.examples[k]);
    }
}
