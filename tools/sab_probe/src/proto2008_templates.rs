//! `proto2008-templates <bin360>` — read the 2008 prototype's `GameTemplates.wsd` (`BLU2`) with
//! `sab_formats::gametemplates` and every loose `.Pack`'s `meshes` slot directory, and tally what
//! assembling characters and assets from the templates rests on: the parse, the template types, the
//! body-setup streams (slots, variations, part references), where each part template's models are
//! stored and whether every copy of a mesh record is byte-identical, which templates reference a mesh
//! directly by type, and the player templates.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use sab_formats::gametemplates::{GameTemplates, Template};
use sab_formats::pandemic_hash;
use sab_formats::proto2008::sbl::{self, SlotKind};
use sab_formats::proto2008::{mesh, slot_dir};

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

/// FNV-1a 64 over a record, to compare copies without holding them.
fn fnv64(b: &[u8]) -> u64 {
    b.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &c| (h ^ c as u64).wrapping_mul(0x0000_0100_0000_01B3))
}

const SLOT_KEYS: [&str; 6] = ["UpperBody", "LowerBody", "GearA", "GearB", "GearC", ""];
const SLOT5: u32 = 0x37A9_525D;

#[derive(Default)]
struct Sets(BTreeMap<String, BTreeMap<String, usize>>);

impl Sets {
    fn add(&mut self, k: &str, v: impl ToString) {
        *self.0.entry(k.to_string()).or_default().entry(v.to_string()).or_default() += 1;
    }
}

pub fn run(args: &[String]) {
    let [bin360] = args else {
        eprintln!("usage: sab_probe proto2008-templates <Bin360 dir>");
        std::process::exit(2);
    };
    let root = PathBuf::from(bin360);
    let path = root.join("GameTemplates.wsd");
    let b = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let (gt, used) = GameTemplates::parse(&b).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let templates: Vec<&Template> = gt.templates().map(|(_, t)| t).collect();
    println!(
        "GameTemplates.wsd: {:?}, {} bytes read of {}, {} entries, {} templates, {} markers",
        gt.order,
        used,
        b.len(),
        gt.entries.len(),
        templates.len(),
        gt.entries.len() - templates.len()
    );
    let mut s = Sets::default();
    for t in &templates {
        s.add("template type", &t.ttype);
        s.add("(unk1, unk2)", format!("({}, {})", t.unk1, t.unk2));
    }
    let by_hash: HashMap<u32, &Template> = templates.iter().map(|t| (pandemic_hash(&t.name), *t)).collect();

    // mesh name hash -> (pack rel, stem, record fingerprint)
    let mut all = Vec::new();
    files(&root, &mut all);
    all.sort();
    let mut copies: HashMap<u32, Vec<(String, String, u64)>> = HashMap::new();
    let mut names: HashMap<u32, String> = HashMap::new();
    for p in all.iter().filter(|p| p.extension().and_then(|x| x.to_str()).is_some_and(|x| x.eq_ignore_ascii_case("pack"))) {
        let buf = std::fs::read(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
        if !buf.starts_with(b"SBL") {
            continue;
        }
        let rel = p.strip_prefix(&root).expect("under root").to_string_lossy().replace('\\', "/");
        let stem = p.file_stem().expect("stem").to_string_lossy().to_ascii_lowercase();
        let sp = sbl::parse(&buf).unwrap_or_else(|e| panic!("{rel}: {e}"));
        let Some(slot) = sp.slot(SlotKind::Meshes) else { continue };
        for r in slot_dir::parse(slot).unwrap_or_else(|e| panic!("{rel}: {e}")).into_iter().filter(|r| r.stored != 0) {
            let d = &slot[r.offset as usize..(r.offset + r.stored) as usize];
            let compressed = r.stored < r.unpacked;
            let name = mesh::record_name(&d[..d.len().min(if compressed { mesh::NAME_PREFIX } else { d.len() })], compressed)
                .unwrap_or_else(|e| panic!("{rel}: {e}"));
            names.entry(r.hash).or_insert(name);
            copies.entry(r.hash).or_default().push((rel.clone(), stem.clone(), fnv64(d)));
        }
    }
    let identical = copies.values().filter(|v| v.iter().all(|c| c.2 == v[0].2)).count();
    println!(
        "mesh records: {} distinct names in {} records; names whose every copy is byte-identical: {identical}",
        copies.len(),
        copies.values().map(Vec::len).sum::<usize>()
    );
    let mut differing: Vec<String> = copies
        .iter()
        .filter(|(_, v)| !v.iter().all(|c| c.2 == v[0].2))
        .map(|(h, v)| format!("{} in {} packs", names[h], v.len()))
        .collect();
    differing.sort();
    println!("names with differing copies: {}", differing.len());
    for d in differing.iter().take(40) {
        println!("  {d}");
    }

    // Part templates' models.
    let (high, low) = (pandemic_hash("HighDetailModel"), pandemic_hash("LowDetailModel"));
    for t in templates.iter().filter(|t| matches!(t.ttype.as_str(), "FxHumanBodyPart" | "FxHumanHead")) {
        for (key, label) in [(high, "HighDetailModel"), (low, "LowDetailModel")] {
            let Some(v) = t.pair(key).and_then(|p| p.as_u32()) else {
                s.add(&format!("{} {label}", t.ttype), "absent");
                continue;
            };
            let Some(c) = copies.get(&v) else {
                s.add(&format!("{} {label}", t.ttype), "names no mesh record");
                continue;
            };
            let own = c.iter().any(|x| x.1 == t.name.to_ascii_lowercase());
            let same = c.iter().all(|x| x.2 == c[0].2);
            s.add(&format!("{} {label}", t.ttype), format!("mesh record (in the pack named after the template: {own}; copies identical: {same})"));
        }
    }

    // Body setups, replayed as assets.rs replays them.
    let slot_hashes: Vec<u32> = SLOT_KEYS.iter().map(|k| if k.is_empty() { SLOT5 } else { pandemic_hash(k) }).collect();
    let (head, bpv, hv, bp, face) = (
        pandemic_hash("Head"),
        pandemic_hash("FxHumanBodyPartVariation"),
        pandemic_hash("FxHumanHeadVariation"),
        pandemic_hash("BodyPart"),
        pandemic_hash("Face"),
    );
    for t in templates.iter().filter(|t| t.ttype == "FxHumanBodySetup") {
        let (mut slot, mut var, mut hvar) = (None::<usize>, None::<u16>, None::<u16>);
        for p in &t.pairs {
            if let Some(k) = slot_hashes.iter().position(|h| *h == p.hash) {
                (slot, var) = (Some(k), None);
            } else if p.hash == head {
                hvar = None;
            } else if p.hash == bpv {
                var = Some(var.map_or(0, |v| v + 1));
            } else if p.hash == hv {
                hvar = Some(hvar.map_or(0, |v| v + 1));
            } else if p.hash == bp || p.hash == face {
                let inside = if p.hash == bp { slot.is_some() && var.is_some() } else { hvar.is_some() };
                let target = p.as_u32().and_then(|v| by_hash.get(&v));
                s.add(
                    "body setup part reference (key, inside a variation, target type)",
                    format!(
                        "({}, {inside}, {})",
                        if p.hash == bp { "BodyPart" } else { "Face" },
                        target.map(|t| t.ttype.as_str()).unwrap_or(if p.as_u32() == Some(0) { "0" } else { "no template" })
                    ),
                );
            }
        }
    }

    // Templates whose 4-byte values name mesh records directly, by type, and where those records are.
    for t in &templates {
        let refs: Vec<u32> = t.pairs.iter().filter_map(|p| p.as_u32()).filter(|v| copies.contains_key(v)).collect();
        if refs.is_empty() {
            continue;
        }
        s.add("type of templates naming a mesh record directly", &t.ttype);
        for v in refs {
            let c = &copies[&v];
            let own = c.iter().any(|x| x.1 == t.name.to_ascii_lowercase());
            let same = c.iter().all(|x| x.2 == c[0].2);
            let own_same = c.iter().filter(|x| x.1 == t.name.to_ascii_lowercase()).all(|x| x.2 == c[0].2);
            s.add(
                "direct mesh reference (in the pack named after the template, every copy identical, that pack's copy equals the first)",
                format!("({own}, {same}, {own_same})"),
            );
            let global: Vec<&(String, String, u64)> = c.iter().filter(|x| x.0.starts_with("Global/")).collect();
            let g_same = global.iter().all(|x| x.2 == global.first().map_or(0, |f| f.2));
            s.add(
                "direct mesh reference, Global/ copies (in the own pack, any Global copy, Global copies identical)",
                format!("({own}, {}, {g_same})", !global.is_empty()),
            );
        }
    }
    let global_names: Vec<&Vec<(String, String, u64)>> =
        copies.values().filter(|v| v.iter().any(|x| x.0.starts_with("Global/"))).collect();
    let global_differ = global_names
        .iter()
        .filter(|v| {
            let g: Vec<u64> = v.iter().filter(|x| x.0.starts_with("Global/")).map(|x| x.2).collect();
            !g.iter().all(|h| *h == g[0])
        })
        .count();
    println!("mesh names with a copy in Global/: {}; whose Global/ copies differ: {global_differ}", global_names.len());

    // Player templates.
    let (model, fx) = (pandemic_hash("Model"), pandemic_hash("FxBodySetup"));
    for t in templates.iter().filter(|t| t.ttype == "Player") {
        let name_of = |k: u32| {
            t.pair(k).and_then(|p| p.as_u32()).map(|v| {
                by_hash.get(&v).map(|x| format!("{} ({})", x.name, x.ttype)).or_else(|| names.get(&v).map(|n| format!("mesh {n}"))).unwrap_or(format!("{v:08X}"))
            })
        };
        println!("Player template {}: Model {:?}, FxBodySetup {:?}", t.name, name_of(model), name_of(fx));
    }
    let mut types: BTreeSet<&str> = BTreeSet::new();
    for t in &templates {
        types.insert(&t.ttype);
    }
    for (k, v) in &s.0 {
        println!("{k}: {v:?}");
    }
}
