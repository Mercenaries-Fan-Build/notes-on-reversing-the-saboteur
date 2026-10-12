//! `albs-keys <retail_dir>` — for every ALBS sub-pack with an empty descriptor (`X == 0`) in a
//! megapack, find the map objects whose pack-path hash equals the entry's `crc`, under each candidate
//! path form, and whether their counts make `sbla::parse` succeed. Alongside, the same for the map
//! object whose record hash equals the entry's `index`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use sab_formats::{loosefiles, map6, megapack, pandemic_hash, sbla};

/// One map object: where it came from, its name, the region of its map (empty for a global map),
/// its record hash and counts.
struct Obj {
    source: String,
    name: String,
    region: String,
    hash: u32,
    counts: [u32; 9],
}

fn objects(retail: &Path) -> Vec<Obj> {
    let lf = std::fs::read(retail.join("France").join("loosefiles_BinPC.pack")).expect("loosefiles_BinPC.pack");
    let es = loosefiles::entries(&lf).expect("loosefiles walk");
    let get = |n: &str| lf[loosefiles::find(&es, n).unwrap_or_else(|| panic!("{n}")).data.clone()].to_vec();
    let globals = [
        ("global.map", map6::parse_global(&get("global.map")).expect("global.map")),
        (
            "DLC Global.map",
            map6::parse_global(&std::fs::read(retail.join("DLC/01/Global.map")).expect("DLC Global.map"))
                .expect("DLC Global.map"),
        ),
    ];
    let regions = [
        ("France.map", map6::parse_region_main(&get("France.map")).expect("France.map")),
        (
            "DLC FRANCE.map",
            map6::parse_region_dlc(&std::fs::read(retail.join("DLC/01/FRANCE.map")).expect("DLC FRANCE.map"))
                .expect("DLC FRANCE.map"),
        ),
    ];
    let mut out = Vec::new();
    for (src, g) in &globals {
        for (list, rs) in [("list1", &g.list1), ("list2", &g.list2), ("dynamics", &g.dynamics)] {
            for r in rs {
                out.push(Obj {
                    source: format!("{src} {list}"),
                    name: r.name(),
                    region: String::new(),
                    hash: r.hash,
                    counts: r.tail.counts,
                });
            }
        }
    }
    for (src, rm) in &regions {
        let region = String::from_utf8_lossy(&rm.region).into_owned();
        for (list, rs) in [("list2", &rm.list2), ("list3", &rm.list3)] {
            for r in rs {
                out.push(Obj {
                    source: format!("{src} {list}"),
                    name: r.name(),
                    region: region.clone(),
                    hash: r.hash,
                    counts: r.tail.counts,
                });
            }
        }
        for t in &rm.tiles {
            if let Some(tail) = &t.tail {
                let end = t.name.iter().position(|&c| c == 0).unwrap_or(t.name.len());
                out.push(Obj {
                    source: format!("{src} tile"),
                    name: String::from_utf8_lossy(&t.name[..end]).into_owned(),
                    region: region.clone(),
                    hash: t.hash,
                    counts: tail.counts,
                });
            }
        }
    }
    out
}

/// The candidate path forms, by label.
fn forms(o: &Obj) -> Vec<(&'static str, String)> {
    let mut v = vec![
        ("name.pack", format!("{}.pack", o.name)),
        ("name.dynpack", format!("{}.dynpack", o.name)),
        ("name.palettepack", format!("{}.palettepack", o.name)),
        ("Global\\name.pack", format!("Global\\{}.pack", o.name)),
        ("Global\\name.dynpack", format!("Global\\{}.dynpack", o.name)),
        ("name", o.name.clone()),
    ];
    if !o.region.is_empty() {
        v.push(("region\\name.pack", format!("{}\\{}.pack", o.region, o.name)));
    }
    for (dl, d) in [("Global\\", "Global\\"), ("global\\", "global\\"), ("", "")] {
        for (nl, n) in [("hash as %d", (o.hash as i32).to_string()), ("hash as %u", o.hash.to_string())] {
            for ext in ["pack", "palettepack", "dynpack"] {
                let label: &'static str = Box::leak(format!("{dl}name+{nl}.{ext}").into_boxed_str());
                v.push((label, format!("{d}{}{n}.{ext}", o.name)));
            }
        }
    }
    v
}

/// CRC-32 (IEEE, reflected, init and final xor 0xFFFFFFFF).
fn crc32(b: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for &x in b {
        c ^= x as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { (c >> 1) ^ 0xEDB8_8320 } else { c >> 1 };
        }
    }
    !c
}

/// A wide set of (label, path) candidates for one object found in the pack file `file`.
fn wide_forms(o: &Obj, file: &str) -> Vec<(String, String)> {
    let stem = Path::new(file).file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
    let dir = Path::new(file).parent().and_then(|s| s.to_str()).unwrap_or("").to_string();
    let prefixes = [
        String::new(),
        "Global\\".into(),
        "global\\".into(),
        "France\\".into(),
        "Global\\global\\".into(),
        "France\\global\\".into(),
        format!("{stem}\\"),
        format!("{dir}\\"),
        format!("{dir}\\{stem}\\"),
        format!("{dir}\\{stem}.kiloPack\\"),
        "France\\Global\\".into(),
        "France\\France\\".into(),
    ];
    let mut names = vec![
        ("name".to_string(), o.name.clone()),
        ("name+hash%d".into(), format!("{}{}", o.name, o.hash as i32)),
        ("name+hash%u".into(), format!("{}{}", o.name, o.hash)),
    ];
    for d in 0..10 {
        names.push((format!("name+{d}"), format!("{}{d}", o.name)));
    }
    let mut v = Vec::new();
    for p in &prefixes {
        for (nl, n) in &names {
            for ext in ["", ".pack", ".palettepack", ".dynpack", ".kilopack"] {
                v.push((format!("'{p}'+{nl}+'{ext}'"), format!("{p}{n}{ext}")));
            }
        }
    }
    v
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

pub fn run(args: &[String]) {
    let Some(retail) = args.first().map(PathBuf::from) else {
        eprintln!("usage: sab_probe albs-keys <retail_dir>");
        std::process::exit(2);
    };
    let objs = objects(&retail);
    println!("map objects: {}", objs.len());
    let mut named: BTreeMap<&str, (usize, usize, usize, usize)> = BTreeMap::new();
    for o in &objs {
        let e = named.entry(o.source.as_str()).or_default();
        e.0 += 1;
        if o.hash == pandemic_hash(&o.name) {
            e.1 += 1;
        }
        if o.hash == pandemic_hash(&format!("{}0", o.name)) {
            e.2 += 1;
        }
        if o.hash == pandemic_hash(&format!("{}1", o.name)) {
            e.3 += 1;
        }
    }
    println!("record hash == pandemic_hash of name / name+\"0\" / name+\"1\", by source -> (objects, equal each):");
    for (k, v) in &named {
        println!("  {k} -> {v:?}");
    }
    let mut by_path: BTreeMap<u32, Vec<(usize, &'static str)>> = BTreeMap::new();
    let mut by_hash: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
    for (i, o) in objs.iter().enumerate() {
        for (label, p) in forms(o) {
            by_path.entry(pandemic_hash(&p)).or_default().push((i, label));
        }
        by_hash.entry(o.hash).or_default().push(i);
    }
    let mut all = Vec::new();
    files(&retail, &mut all);
    all.sort();

    // (crc outcome, index outcome) -> packs
    let mut tally: BTreeMap<(String, String), usize> = BTreeMap::new();
    let mut examples: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    // which source/form gave the counts that parsed under the crc rule
    let mut crc_forms: BTreeMap<String, usize> = BTreeMap::new();
    // packs with no crc match: (file, source of the index-rule object) -> packs
    let mut no_crc: BTreeMap<(String, String), usize> = BTreeMap::new();
    // of those, which wide path form of the index-rule object hashes to the crc: (file, form) -> packs
    let mut wide: BTreeMap<(String, String), usize> = BTreeMap::new();
    // X != 0 packs: (file, X == 0x3C, prefix for which crc == hash(prefix + streamblock path + ".pack")) -> packs
    let mut streamblocks: BTreeMap<(String, bool, String), usize> = BTreeMap::new();
    let mut packs = 0usize;
    for p in &all {
        let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
        if ext != "megapack" && ext != "kiloPack" {
            continue;
        }
        let buf = std::fs::read(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
        let mp = megapack::parse(&buf).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
        for (i, e) in mp.entries.iter().enumerate() {
            let sub = megapack::entry_slice(&buf, e).unwrap_or_else(|err| panic!("{err}"));
            if sub.get(0..4) != Some(sbla::MAGIC) {
                continue;
            }
            let x = sbla::descriptor_len(sub).expect("descriptor_len");
            if x != 0 {
                let file = p.strip_prefix(&retail).unwrap_or(p).display().to_string();
                let id = e.index.to_string();
                let path = if e.index < 10 { id.clone() } else { format!("{}\\{id}", &id[..2]) };
                let hit = ["France\\", "France\\France\\", "DLC\\01\\France\\", ""]
                    .iter()
                    .find(|pre| pandemic_hash(&format!("{pre}{path}.pack")) == e.crc);
                let k = (file, x == 0x3C, hit.map(|s| s.to_string()).unwrap_or_else(|| "none".into()));
                *streamblocks.entry(k).or_default() += 1;
                continue;
            }
            packs += 1;
            let label = format!("{}#{i} crc 0x{:08X} index 0x{:X}", p.display(), e.crc, e.index);
            let parses = |c: [u32; 9]| sbla::parse(sub, Some(c)).is_ok();

            let crc_hits = by_path.get(&e.crc).cloned().unwrap_or_default();
            let crc_outcome = outcome(crc_hits.iter().map(|&(j, _)| objs[j].counts), parses);
            for &(j, form) in &crc_hits {
                if parses(objs[j].counts) {
                    *crc_forms.entry(format!("{} via {form}", objs[j].source)).or_default() += 1;
                }
            }
            let idx_hits = by_hash.get(&e.index).cloned().unwrap_or_default();
            let idx_outcome = outcome(idx_hits.iter().map(|&j| objs[j].counts), parses);

            let detail = format!(
                "{label}: crc -> [{}]; index -> [{}]",
                crc_hits
                    .iter()
                    .map(|&(j, f)| format!("{} '{}' ({f}) {:?}", objs[j].source, objs[j].name, objs[j].counts))
                    .collect::<Vec<_>>()
                    .join(", "),
                idx_hits
                    .iter()
                    .map(|&j| format!("{} '{}' {:?}", objs[j].source, objs[j].name, objs[j].counts))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            if crc_hits.is_empty() {
                let src = idx_hits.iter().map(|&j| objs[j].source.as_str()).collect::<Vec<_>>().join(" + ");
                let file = p.strip_prefix(&retail).unwrap_or(p).display().to_string();
                for &j in &idx_hits {
                    for form in wide_forms(&objs[j], &file) {
                        if pandemic_hash(&form.1) == e.crc {
                            *wide.entry((file.clone(), form.0.clone())).or_default() += 1;
                        }
                        if crc32(form.1.as_bytes()) == e.crc || crc32(form.1.to_lowercase().as_bytes()) == e.crc {
                            *wide.entry((file.clone(), format!("crc32 {}", form.0))).or_default() += 1;
                        }
                    }
                    if crc32(objs[j].name.as_bytes()) == e.crc {
                        *wide.entry((file.clone(), "crc32 name".into())).or_default() += 1;
                    }
                }
                *no_crc.entry((file, src)).or_default() += 1;
            }
            let k = (crc_outcome, idx_outcome);
            *tally.entry(k.clone()).or_default() += 1;
            let ex = examples.entry(k).or_default();
            if ex.len() < 8 {
                ex.push(detail);
            }
        }
    }
    println!("X=0 ALBS sub-packs in megapacks: {packs}");
    println!("(crc rule, index rule) -> packs:");
    for (k, v) in &tally {
        println!("  {k:?} -> {v}");
        for d in &examples[k] {
            println!("      {d}");
        }
    }
    println!("X != 0 packs by (file, X == 0x3C, prefix of the matching streamblock path):");
    for (k, v) in &streamblocks {
        println!("  {k:?} -> {v}");
    }
    println!("packs with no crc match, by (file, index-rule object source):");
    for (k, v) in &no_crc {
        println!("  {k:?} -> {v}");
    }
    println!("of those, wide path forms of the index-rule object that hash to the crc, by (file, form):");
    for (k, v) in &wide {
        println!("  {k:?} -> {v}");
    }
    println!("crc-rule matches whose counts parse, by source and path form:");
    for (k, v) in &crc_forms {
        println!("  {k} -> {v}");
    }
}

/// "none" with no candidate; else "N candidates, D distinct counts, P parse".
fn outcome(cands: impl Iterator<Item = [u32; 9]>, parses: impl Fn([u32; 9]) -> bool) -> String {
    let c: Vec<[u32; 9]> = cands.collect();
    if c.is_empty() {
        return "none".into();
    }
    let mut distinct = c.clone();
    distinct.sort();
    distinct.dedup();
    let ok = distinct.iter().filter(|&&d| parses(d)).count();
    format!("{} cand, {} distinct, {} parse", c.len(), distinct.len(), ok)
}
