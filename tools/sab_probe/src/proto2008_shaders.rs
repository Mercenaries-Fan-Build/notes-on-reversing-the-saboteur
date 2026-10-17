//! `proto2008-shaders <bin360>` — read every `*.shaders` file at the 2008 prototype's `Bin360` root
//! with `sab_formats::proto2008::shaders`, and every `materials` slot under it, and tally the facts
//! `docs/2008_shaders_format.md` cites: groups and variants per file, parameter names and values,
//! code heads, and whether each material pass's shader hashes name a group by `key` or by `name`, in
//! which file.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use sab_formats::proto2008::material;
use sab_formats::proto2008::sbl::{self, SlotKind};
use sab_formats::proto2008::{shader_mapping, shaders};

fn pass_label(h: u32) -> String {
    match h {
        material::PASS_COLOR => "Color-Pass".into(),
        material::PASS_Z => "Z-Pass".into(),
        material::PASS_CAST_SHADOW => "Cast-Shadow-Pass".into(),
        material::PASS_RECEIVE_SHADOW => "Receive-Shadow-Pass".into(),
        material::PASS_FOLIAGE => "Foliage-Pass".into(),
        other => format!("{other:08X}"),
    }
}

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

pub fn run(args: &[String]) {
    let [bin360] = args else {
        eprintln!("usage: sab_probe proto2008-shaders <Bin360 dir>");
        std::process::exit(2);
    };
    let root = PathBuf::from(bin360);
    let mut all = Vec::new();
    files(&root, &mut all);
    all.sort();
    let mut t = Tally::default();
    let mut by_key: HashMap<u32, Vec<String>> = HashMap::new();
    let mut by_name: HashMap<u32, Vec<String>> = HashMap::new();
    let mut samplers_of: HashMap<(u32, &str), Vec<usize>> = HashMap::new();
    for p in all.iter().filter(|p| p.extension().and_then(|x| x.to_str()).map(|x| x.eq_ignore_ascii_case("shaders")) == Some(true)) {
        let rel = p.strip_prefix(&root).expect("under root").to_string_lossy().into_owned();
        let buf = std::fs::read(p).unwrap_or_else(|e| panic!("{rel}: {e}"));
        let s = match shaders::parse(&buf) {
            Ok(s) => s,
            Err(e) => {
                t.note(format!("FAIL {rel}: {e}"), &rel);
                continue;
            }
        };
        println!("{rel}: {:?} layout, {} pixel groups, {} vertex groups", s.layout, s.pixel.len(), s.vertex.len());
        for (stage, groups) in [("pixel", &s.pixel), ("vertex", &s.vertex)] {
            for g in groups.iter() {
                let at = format!("{rel} {stage} {:08X}", g.name);
                t.note(format!("{stage} group {}", if g.key.is_some() { "present" } else { "absent" }), &at);
                t.note(format!("{stage} group name word {}", if g.name == 0 { "0" } else { "non-zero" }), &at);
                if let Some(k) = g.key {
                    t.note(format!("{stage} group key {} name", if k == g.name { "==" } else { "!=" }), &format!("{at} key {k:08X}"));
                    by_key.entry(k).or_default().push(format!("{rel} {stage}"));
                }
                by_name.entry(g.name).or_default().push(format!("{rel} {stage}"));
                t.note(format!("{stage} variants per group {}", g.variants.len().min(16)), &at);
                for (vi, v) in g.variants.iter().enumerate() {
                    let head = v.code.get(..4).map(|h| format!("{h:02X?}")).unwrap_or_default();
                    t.note(format!("{stage} code head {head}"), &format!("{at} variant {vi} {} bytes", v.code.len()));
                    match shaders::constant_table(v.code) {
                        Ok((version, cs)) => {
                            t.note(format!("{stage} constant table version 0x{version:08X}"), &format!("{at} variant {vi}"));
                            let mut material_samplers = Vec::new();
                            for c in cs.iter().filter(|c| c.set == shaders::REGISTER_SET_SAMPLER) {
                                match c.name.strip_prefix("g_Sampler").and_then(|n| n.parse::<u16>().ok()) {
                                    Some(n) => {
                                        t.note(format!("{stage} g_Sampler<n> at sampler register {} n", if n == c.index { "==" } else { "!=" }), &format!("{at} variant {vi} {} s{}", c.name, c.index));
                                        material_samplers.push(n);
                                    }
                                    None => t.note(format!("{stage} engine sampler '{}' at s{}", c.name, c.index), &format!("{at} variant {vi}")),
                                }
                            }
                            material_samplers.sort();
                            let dense = material_samplers.iter().enumerate().all(|(i, &n)| i as u16 == n);
                            t.note(format!("{stage} g_Sampler indices dense from 0: {dense}"), &format!("{at} variant {vi} {material_samplers:?}"));
                            if let Some(k) = g.key {
                                samplers_of.entry((k, stage)).or_default().push(material_samplers.len());
                            }
                        }
                        Err(e) => t.note(format!("FAIL {stage} constant table: {e}"), &format!("{at} variant {vi}")),
                    }
                    t.note(format!("{stage} params per variant {}", v.params.len().min(32)), &format!("{at} variant {vi}"));
                    for (n, val) in &v.params {
                        t.note(format!("{stage} param '{n}'"), &format!("{at} variant {vi} value {val}"));
                        t.note(format!("{stage} param '{n}' value {val}"), &format!("{at} variant {vi}"));
                    }
                }
            }
        }
    }
    let mut mappings: Vec<(String, shader_mapping::Mapping)> = Vec::new();
    for p in all.iter().filter(|p| p.extension().and_then(|x| x.to_str()) == Some("ShaderMapping")) {
        let rel = p.strip_prefix(&root).expect("under root").to_string_lossy().into_owned();
        let world = rel.trim_end_matches(".ShaderMapping").to_string();
        let m = shader_mapping::parse(&std::fs::read(p).expect("read mapping")).unwrap_or_else(|e| panic!("{rel}: {e}"));
        for r in &m.records {
            let ps = if r.pixel_shader == 0 { "0" } else if by_key.contains_key(&r.pixel_shader) { "a group key" } else { "no group" };
            let vs = if by_key.contains_key(&r.vertex_shader) { "a group key" } else { "no group" };
            t.note(format!("{world}.ShaderMapping record: pass {}, pixel shader {ps}, vertex shader {vs}", pass_label(r.pass)), &format!("{:08X}", r.material));
        }
        println!("{rel}: {} records", m.records.len());
        mappings.push((world, m));
    }
    let mut pass_shaders: HashSet<(u32, &'static str)> = HashSet::new();
    for p in all.iter().filter(|p| p.extension().and_then(|x| x.to_str()).map(|x| x.eq_ignore_ascii_case("pack")) == Some(true)) {
        let buf = std::fs::read(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
        if !buf.starts_with(b"SBL") {
            continue;
        }
        let folder = p.strip_prefix(&root).expect("under root").components().next().map(|c| c.as_os_str().to_string_lossy().into_owned()).unwrap_or_default();
        let sp = sbl::parse(&buf).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
        let Some(slot) = sp.slot(SlotKind::Materials) else { continue };
        for m in material::parse(slot).unwrap_or_else(|e| panic!("{}: {e}", p.display())) {
            for pass in &m.passes {
                for (world, map) in &mappings {
                    let vs = map.get(m.uid, pass.name, shader_mapping::Stage::Vertex);
                    let ps = map.get(m.uid, pass.name, shader_mapping::Stage::Pixel);
                    let mapped = match (vs, ps) {
                        (None, None) => "not mapped".to_string(),
                        (Some(v), Some(q)) => format!(
                            "mapped, vs {} ps {}",
                            if v == pass.vertex_shader { "same" } else { "replaced" },
                            if q == pass.pixel_shader { "same" } else { "replaced" }
                        ),
                        _ => "mapped for one stage only".into(),
                    };
                    t.note(format!("{folder}/ pass in {world}.ShaderMapping: {mapped}"), &format!("{} {} {:08X}", p.display(), m.name, m.uid));
                    let rv = map.resolve(m.uid, pass.name, shader_mapping::Stage::Vertex, pass.vertex_shader);
                    let rp = map.resolve(m.uid, pass.name, shader_mapping::Stage::Pixel, pass.pixel_shader);
                    let found = by_key.contains_key(&rv) && (rp == 0 || by_key.contains_key(&rp));
                    t.note(format!("{folder}/ pass shaders after {world}.ShaderMapping: {}", if found { "both in a .shaders file" } else { "missing" }), &format!("{} {} {:08X}", p.display(), m.name, m.uid));
                }
                pass_shaders.insert((pass.vertex_shader, "vertex"));
                if pass.pixel_shader != 0 {
                    pass_shaders.insert((pass.pixel_shader, "pixel"));
                }
                let refs = pass.texture_refs.len();
                let vs = samplers_of.get(&(pass.vertex_shader, "vertex"));
                let ps = if pass.pixel_shader == 0 { Some(vec![0]) } else { samplers_of.get(&(pass.pixel_shader, "pixel")).cloned() };
                let verdict = match (vs, ps) {
                    (Some(vs), Some(ps)) => {
                        let used: Vec<usize> = vs.iter().flat_map(|v| ps.iter().map(move |p| (*v).max(*p))).collect();
                        if used.iter().all(|&u| u == refs) {
                            "every variant's g_Sampler count (max of vs, ps) == texture refs".to_string()
                        } else if used.iter().all(|&u| u <= refs) {
                            "g_Sampler count < texture refs in some variant".into()
                        } else {
                            format!("g_Sampler count > texture refs in some variant ({used:?} vs {refs})")
                        }
                    }
                    _ => "a shader of the pass is in no .shaders file".into(),
                };
                t.note(format!("material pass: {}", verdict.split(" (").next().unwrap_or(&verdict)), &format!("{} {} {verdict}", p.display(), m.name));
            }
        }
    }
    for (h, stage) in &pass_shaders {
        let k = by_key.get(h).map(|v| v.join(" + ")).unwrap_or_else(|| "none".into());
        let n = by_name.get(h).map(|v| v.join(" + ")).unwrap_or_else(|| "none".into());
        t.note(format!("material {stage} shader: by key in [{k}], by name in [{n}]"), &format!("{h:08X}"));
    }
    for (k, n) in &t.counts {
        println!("{n:>8}  {k}   e.g. {}", t.examples[k]);
    }
}
