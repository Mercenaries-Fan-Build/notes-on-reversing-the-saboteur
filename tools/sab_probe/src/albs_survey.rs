//! `albs-survey <retail_dir>` — parse every ALBS sub-pack in the install with `sab_formats::sbla`,
//! taking each object pack's type counts from its map record (`global.map`, `France.map`, and the DLC
//! maps; the record hash is the megapack entry's `index`, or `pandemic_hash(file stem)` for a DLC loose
//! pack), and tally parse failures and how each record's stored `offset` compares with
//! `sbla::expected_offsets`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use sab_formats::{loosefiles, map6, megapack, pandemic_hash, sbla};

/// hash -> counts from every object record and extended tile in the given maps, and the main
/// France.map tiles' counts by tile ordinal (`None` for a tile without a tail).
fn map_counts(retail: &Path) -> (BTreeMap<u32, [u32; 9]>, Vec<Option<[u32; 9]>>) {
    let lf = std::fs::read(retail.join("France").join("loosefiles_BinPC.pack")).expect("loosefiles_BinPC.pack");
    let es = loosefiles::entries(&lf).expect("loosefiles walk");
    let get = |n: &str| lf[loosefiles::find(&es, n).unwrap_or_else(|| panic!("{n}")).data.clone()].to_vec();
    let g = map6::parse_global(&get("global.map")).expect("global.map");
    let f = map6::parse_region_main(&get("France.map")).expect("France.map");
    let dg = map6::parse_global(&std::fs::read(retail.join("DLC/01/Global.map")).expect("DLC Global.map"))
        .expect("DLC Global.map");
    let df = map6::parse_region_dlc(&std::fs::read(retail.join("DLC/01/FRANCE.map")).expect("DLC FRANCE.map"))
        .expect("DLC FRANCE.map");
    let m = map6::object_counts(&[&g, &dg], &[&f, &df]).expect("object counts");
    println!("map objects with counts: {}", m.len());
    let by_ordinal = f.tiles.iter().map(|t| t.tail.as_ref().map(|tl| tl.counts)).collect();
    (m, by_ordinal)
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
    packs: usize,
    ok: usize,
    no_counts: usize,
    /// X=0 packs with no map record by hash: (label, index, result with the tile at ordinal `index`).
    unmatched: Vec<(String, u32, String)>,
    errors: BTreeMap<String, usize>,
    error_examples: Vec<String>,
    /// (type 8?, comp == 0?) -> (records, stored offset == expected)
    offsets: BTreeMap<(bool, bool), (usize, usize)>,
    offset_examples: Vec<String>,
}

fn survey(
    label: &str,
    buf: &[u8],
    key: u32,
    maps: &BTreeMap<u32, [u32; 9]>,
    tiles: &[Option<[u32; 9]>],
    t: &mut Tally,
) {
    t.packs += 1;
    let x = match sbla::descriptor_len(buf) {
        Ok(x) => x,
        Err(e) => {
            *t.errors.entry(e).or_default() += 1;
            return;
        }
    };
    let counts = maps.get(&key).copied();
    if x == 0 && counts.is_none() {
        t.no_counts += 1;
        let attempt = |c: [u32; 9]| match sbla::parse(buf, Some(c)) {
            Ok(sp) => format!("parse exactly ({} records)", sp.records.len()),
            Err(e) => format!("fail: {e}"),
        };
        let by_tile = match tiles.get(key as usize) {
            None => "no tile at that ordinal".to_string(),
            Some(None) => "tile has no tail".to_string(),
            Some(Some(c)) => format!("tile counts {}", attempt(*c)),
        };
        let by_text = match maps.get(&pandemic_hash(&key.to_string())) {
            Some(c) => format!("hash(\"{key}\") counts {}", attempt(*c)),
            None => format!("no record hash(\"{key}\")"),
        };
        let by_tile = format!("{by_tile}; {by_text}");
        t.unmatched.push((label.to_string(), key, by_tile));
        return;
    }
    match sbla::parse(buf, if x == 0 { counts } else { None }) {
        Ok(sp) => {
            t.ok += 1;
            let want = sbla::expected_offsets(&sp.records);
            for (r, w) in sp.records.iter().zip(want) {
                let e = t.offsets.entry((r.kind == sbla::TYPE_STREAMED, r.comp == 0)).or_default();
                e.0 += 1;
                if r.offset == w {
                    e.1 += 1;
                } else if t.offset_examples.len() < 20 {
                    t.offset_examples.push(format!(
                        "{label}: type {} hash 0x{:08X} comp {} stored offset 0x{:X} expected 0x{w:X}",
                        r.kind, r.hash, r.comp, r.offset
                    ));
                }
            }
        }
        Err(e) => {
            let key = e.split(':').next().unwrap_or("").to_string();
            *t.errors.entry(key).or_default() += 1;
            if t.error_examples.len() < 20 {
                t.error_examples.push(format!("{label} X=0x{x:X}: {e}"));
            }
        }
    }
}

pub fn run(args: &[String]) {
    let Some(retail) = args.first().map(PathBuf::from) else {
        eprintln!("usage: sab_probe albs-survey <retail_dir>");
        std::process::exit(2);
    };
    let (maps, tiles) = map_counts(&retail);
    let mut all = Vec::new();
    files(&retail, &mut all);
    all.sort();
    let mut t = Tally::default();
    for p in &all {
        let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
        let buf = match ext {
            "megapack" | "kiloPack" => std::fs::read(p).unwrap_or_else(|e| panic!("{}: {e}", p.display())),
            "dynpack" | "pack" if p.components().any(|c| c.as_os_str() == "DLC") => {
                std::fs::read(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
            }
            _ => continue,
        };
        if ext == "megapack" || ext == "kiloPack" {
            let mp = megapack::parse(&buf).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
            for (i, e) in mp.entries.iter().enumerate() {
                let sub = megapack::entry_slice(&buf, e).unwrap_or_else(|err| panic!("{err}"));
                if sub.get(0..4) != Some(sbla::MAGIC) {
                    continue;
                }
                survey(&format!("{}#{i}", p.display()), sub, e.index, &maps, &tiles, &mut t);
            }
        } else if buf.get(0..4) == Some(sbla::MAGIC) {
            let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            survey(&p.display().to_string(), &buf, pandemic_hash(stem), &maps, &tiles, &mut t);
        }
    }
    println!("ALBS sub-packs: {}  parsed: {}  X=0 without a map record: {}", t.packs, t.ok, t.no_counts);
    println!("X=0 packs with no map record by hash ({}):", t.unmatched.len());
    for (label, key, r) in &t.unmatched {
        println!("  {label}  index 0x{key:X} ({key}): {r}");
    }
    println!("parse errors: {:?}", t.errors);
    for e in &t.error_examples {
        println!("  {e}");
    }
    println!("offset field vs expected_offsets, by (type 8, comp 0) -> (records, equal):");
    for (k, v) in &t.offsets {
        println!("  {k:?} -> {v:?}");
    }
    for e in &t.offset_examples {
        println!("  {e}");
    }
}
