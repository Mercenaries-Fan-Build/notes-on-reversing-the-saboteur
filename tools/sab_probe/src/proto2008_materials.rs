//! `proto2008-materials <bin360>` — read every `materials` slot of every loose `.Pack` under the 2008
//! prototype's `Bin360` with `sab_formats::proto2008::material`, and tally the facts
//! `docs/2008_materials_format.md` cites: parse results per sub-pack revision, the uid against the
//! name's hash, pass names, flags, shaders, constant and state counts, sampler bindings, whether each
//! texture hash names a `textures` record (same pack, any pack), and whether each mesh primitive's
//! material names a material (same pack, any pack).

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use sab_formats::proto2008::sbl::{self, SlotKind};
use sab_formats::proto2008::{material, mesh, slot_dir};
use sab_formats::{names, pandemic_hash, zlib};

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

fn pass_label(h: u32) -> String {
    match h {
        material::PASS_COLOR => "Color-Pass".into(),
        material::PASS_Z => "Z-Pass".into(),
        material::PASS_CAST_SHADOW => "Cast-Shadow-Pass".into(),
        material::PASS_RECEIVE_SHADOW => "Receive-Shadow-Pass".into(),
        material::PASS_FOLIAGE => "Foliage-Pass".into(),
        other => hash_label(other),
    }
}

/// `D3DSAMPLERSTATETYPE` and value names from `WildStar_p.pdb` for the states the build sets.
fn sampler_state_label(s: u32, v: u32) -> String {
    let address = |v: u32| match v {
        0 => "WRAP".to_string(),
        1 => "MIRROR".into(),
        2 => "CLAMP".into(),
        3 => "MIRRORONCE".into(),
        6 => "BORDER".into(),
        o => format!("{o}"),
    };
    let filter = |v: u32| match v {
        0 => "POINT".to_string(),
        1 => "LINEAR".into(),
        2 => "NONE".into(),
        4 => "ANISOTROPIC".into(),
        o => format!("{o}"),
    };
    match s {
        0x00 => format!("ADDRESSU {}", address(v)),
        0x04 => format!("ADDRESSV {}", address(v)),
        0x10 => format!("MAGFILTER {}", filter(v)),
        0x14 => format!("MINFILTER {}", filter(v)),
        0x18 => format!("MIPFILTER {}", filter(v)),
        o => format!("0x{o:X} = {v}"),
    }
}

/// `D3DRENDERSTATETYPE` names from `WildStar_p.pdb` for the states the build sets.
fn render_state_label(s: u32) -> String {
    match s {
        0x30 => "ZWRITEENABLE".into(),
        0x38 => "CULLMODE".into(),
        0x3C => "ALPHABLENDENABLE".into(),
        0x60 => "ALPHATESTENABLE".into(),
        0x64 => "ALPHAREF".into(),
        0x6C => "STENCILENABLE".into(),
        0x7C => "STENCILPASS".into(),
        0x80 => "STENCILFUNC".into(),
        0x84 => "STENCILREF".into(),
        0x8C => "STENCILWRITEMASK".into(),
        0xD0 => "DEPTHBIAS".into(),
        0xD4 => "COLORWRITEENABLE".into(),
        o => format!("0x{o:X}"),
    }
}

fn hash_label(h: u32) -> String {
    if h == 0 {
        return "0".into();
    }
    match names::name(h) {
        Some(n) => format!("{h:08X} '{n}'"),
        None => format!("{h:08X} (unnamed)"),
    }
}

struct Pack {
    rel: String,
    magic: String,
    materials: Vec<material::Material>,
    textures: HashSet<u32>,
    prim_materials: Vec<u32>,
}

pub fn run(args: &[String]) {
    let [bin360] = args else {
        eprintln!("usage: sab_probe proto2008-materials <Bin360 dir>");
        std::process::exit(2);
    };
    let root = PathBuf::from(bin360);
    let mut all = Vec::new();
    files(&root, &mut all);
    all.sort();
    let mut t = Tally::default();
    let mut packs = Vec::new();
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
        let magic = String::from_utf8_lossy(&sp.magic).into_owned();
        let mut pack = Pack { rel: rel.clone(), magic: magic.clone(), materials: Vec::new(), textures: HashSet::new(), prim_materials: Vec::new() };
        if let Some(slot) = sp.slot(SlotKind::Materials) {
            match material::parse(slot) {
                Ok(ms) => {
                    t.note(format!("materials slot parses ({magic})"), &rel);
                    pack.materials = ms;
                }
                Err(e) => t.note(format!("FAIL materials slot ({magic}): {}", e.split(':').next_back().unwrap_or(&e).trim()), &format!("{rel}: {e}")),
            }
        }
        if let Some(slot) = sp.slot(SlotKind::Textures) {
            let recs = slot_dir::parse(slot).unwrap_or_else(|e| panic!("{rel}: {e}"));
            pack.textures.extend(recs.iter().filter(|r| r.stored != 0).map(|r| r.hash));
        }
        if let Some(slot) = sp.slot(SlotKind::Meshes) {
            let recs = slot_dir::parse(slot).unwrap_or_else(|e| panic!("{rel}: {e}"));
            for r in recs.iter().filter(|r| r.stored != 0) {
                let d = &slot[r.offset as usize..(r.offset + r.stored) as usize];
                let body = if r.stored < r.unpacked { zlib::inflate_exact(d, r.unpacked as usize, "mesh").unwrap_or_else(|e| panic!("{rel}: {e}")) } else { d.to_vec() };
                let rec = mesh::parse_record(&body).unwrap_or_else(|e| panic!("{rel}: {e}"));
                for s in &rec.mesh().streams {
                    pack.prim_materials.extend(s.prims.iter().map(|p| p.material));
                }
                if let mesh::Record::Variant { bindings, .. } = &rec {
                    pack.prim_materials.extend(bindings.iter().map(|b| b.material));
                }
            }
        }
        packs.push(pack);
    }
    let all_materials: HashSet<u32> = packs.iter().flat_map(|p| p.materials.iter().map(|m| m.uid)).collect();
    let all_textures: HashSet<u32> = packs.iter().flat_map(|p| p.textures.iter().copied()).collect();
    for pack in &packs {
        let at = &pack.rel;
        let local: HashSet<u32> = pack.materials.iter().map(|m| m.uid).collect();
        for m in &pack.materials {
            let mat = format!("{at} '{}' {:08X}", m.name, m.uid);
            t.note(format!("uid == pandemic_hash(name): {}", pandemic_hash(&m.name) == m.uid), &mat);
            t.note(format!("textures per material {}", m.textures.len().min(8)), &mat);
            t.note(format!("passes per material {}", m.passes.len().min(8)), &mat);
            for &tex in &m.textures {
                let where_ = if pack.textures.contains(&tex) { "this pack" } else if all_textures.contains(&tex) { "another pack only" } else { "no pack" };
                t.note(format!("material texture names a textures record in {where_}"), &format!("{mat} tex {}", hash_label(tex)));
            }
            let unused: Vec<usize> = (0..m.textures.len()).filter(|i| !m.passes.iter().any(|p| p.texture_refs.contains(&(*i as u32)))).collect();
            t.note(format!("material textures no pass binds: {}", unused.len().min(4)), &format!("{mat} {unused:?}"));
            let order: Vec<String> = m.passes.iter().map(|p| pass_label(p.name)).collect();
            t.note(format!("pass order {}", order.join(", ")), &mat);
            for (pi, p) in m.passes.iter().enumerate() {
                let pass = format!("{mat} pass {pi}");
                t.note(format!("pass name {}", pass_label(p.name)), &pass);
                t.note(format!("pass flags 0x{:X}", p.flags), &pass);
                t.note(format!("pass vertex shader {}", if p.vertex_shader == 0 { "0" } else { "set" }), &pass);
                let alpha_test = p.render_states.iter().any(|&(s, v)| s == 0x60 && v != 0);
                t.note(
                    format!("pass pixel shader {}, alpha test {} ({})", if p.pixel_shader == 0 { "0" } else { "set" }, if alpha_test { "on" } else { "off" }, pass_label(p.name)),
                    &pass,
                );
                t.note(format!("pass texture refs {}", p.texture_refs.len()), &format!("{pass} {:?}", p.texture_refs));
                t.note(format!("pass register vectors {}", p.register_vectors.len()), &format!("{pass} {:?}", p.register_vectors));
                for (r, _) in &p.register_vectors {
                    t.note(format!("register vector register {r}"), &pass);
                }
                t.note(format!("pass vs constants f/i/b {}/{}/{}", p.vs.floats.len(), p.vs.ints.len(), p.vs.bools.len()), &pass);
                t.note(format!("pass ps constants f/i/b {}/{}/{}", p.ps.floats.len(), p.ps.ints.len(), p.ps.bools.len()), &pass);
                for (s, v) in &p.render_states {
                    t.note(format!("render state {} ({})", render_state_label(*s), pass_label(p.name)), &format!("{pass} value 0x{v:X}"));
                }
                let samplers = p.samplers();
                t.note(
                    format!("sampler groups {} texture refs", if samplers.is_empty() { "none, with".to_string() } else if samplers.len() == p.texture_refs.len() { "==".into() } else { format!("{} vs", samplers.len()) }),
                    &format!("{pass} {} refs", p.texture_refs.len()),
                );
                for (i, g) in samplers.iter().enumerate() {
                    let states: Vec<String> = g.iter().map(|(s, v)| sampler_state_label(*s, *v)).collect();
                    t.note(format!("sampler group: {}", states.join(", ")), &format!("{pass} sampler {i}"));
                }
            }
        }
        for &pm in &pack.prim_materials {
            let where_ = if local.contains(&pm) { "this pack" } else if all_materials.contains(&pm) { "another pack only" } else { "no pack" };
            t.note(format!("mesh primitive material names a material in {where_} ({})", pack.magic), &format!("{at} {}", hash_label(pm)));
        }
    }
    let shaders: HashSet<u32> = packs.iter().flat_map(|p| p.materials.iter()).flat_map(|m| m.passes.iter()).flat_map(|p| [p.vertex_shader, p.pixel_shader]).filter(|&h| h != 0).collect();
    let named = shaders.iter().filter(|&&h| names::name(h).is_some()).count();
    println!("{} distinct shader hashes, {named} named by data/hash_names.tsv", shaders.len());
    println!("{} distinct material uids, {} distinct texture records", all_materials.len(), all_textures.len());
    for (k, n) in &t.counts {
        println!("{n:>8}  {k}   e.g. {}", t.examples[k]);
    }
}
