//! `albs-survey <retail_dir>` — parse every ALBS sub-pack in the install with `sab_formats::sbla`,
//! taking each object pack's type counts from its map object (`map6::pack_counts` by the megapack
//! entry's `crc`; `map6::object_counts` by a kiloPack entry's `index` or a DLC loose pack's
//! `pandemic_hash(file stem)`), and tally parse failures, blob heads, MSHA record sizes, and how each
//! record's stored `offset` compares with `sbla::expected_offsets`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use sab_formats::{loosefiles, map6, megapack, pandemic_hash, sbla};

/// (counts by pack-path hash, counts by object hash).
fn map_counts(retail: &Path) -> (BTreeMap<u32, [u32; 9]>, BTreeMap<u32, [u32; 9]>) {
    let lf = std::fs::read(retail.join("France").join("loosefiles_BinPC.pack")).expect("loosefiles_BinPC.pack");
    let es = loosefiles::entries(&lf).expect("loosefiles walk");
    let get = |n: &str| lf[loosefiles::find(&es, n).unwrap_or_else(|| panic!("{n}")).data.clone()].to_vec();
    let g = map6::parse_global(&get("global.map")).expect("global.map");
    let f = map6::parse_region_main(&get("France.map")).expect("France.map");
    let dg = map6::parse_global(&std::fs::read(retail.join("DLC/01/Global.map")).expect("DLC Global.map"))
        .expect("DLC Global.map");
    let df = map6::parse_region_dlc(&std::fs::read(retail.join("DLC/01/FRANCE.map")).expect("DLC FRANCE.map"))
        .expect("DLC FRANCE.map");
    let by_path = map6::pack_counts(&[&g, &dg], &[&f, &df]).expect("pack counts");
    let by_hash = map6::object_counts(&[&g, &dg], &[&f, &df]).expect("object counts");
    println!("map objects with counts: {} by pack path, {} by hash", by_path.len(), by_hash.len());
    (by_path, by_hash)
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
    no_counts: Vec<String>,
    errors: BTreeMap<String, usize>,
    error_examples: Vec<String>,
    /// (type 8?, comp == 0?) -> (records, stored offset == expected)
    offsets: BTreeMap<(bool, bool), (usize, usize)>,
    offset_examples: Vec<String>,
    /// (type, first four bytes as text, comp == uncomp) -> blobs
    heads: BTreeMap<(usize, String, bool), usize>,
    /// type-0 blobs that start with AHSM: (exactly 276 + c0 + c1 bytes, not)
    msha_fit: (usize, usize),
    msha_examples: Vec<String>,
}

fn head(b: &[u8]) -> String {
    match b.get(0..4) {
        Some(h) if h.iter().all(|c| c.is_ascii_alphanumeric()) => String::from_utf8_lossy(h).into_owned(),
        Some(h) => format!("{h:02X?}"),
        None => format!("<{} bytes>", b.len()),
    }
}

fn survey(label: &str, buf: &[u8], counts: Option<[u32; 9]>, t: &mut Tally) {
    t.packs += 1;
    let x = match sbla::descriptor_len(buf) {
        Ok(x) => x,
        Err(e) => {
            *t.errors.entry(e).or_default() += 1;
            return;
        }
    };
    if x == 0 && counts.is_none() {
        t.no_counts.push(label.to_string());
        return;
    }
    match sbla::parse(buf, if x == 0 { counts } else { None }) {
        Ok(sp) => {
            t.ok += 1;
            for (r, b) in sp.records.iter().zip(&sp.blobs) {
                *t.heads.entry((r.kind, head(b), r.comp == r.uncomp)).or_default() += 1;
                if r.kind == 0 && b.get(0..4) == Some(b"AHSM") && b.len() >= 20 {
                    let u = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap()) as usize;
                    if b.len() == 276 + u(12) + u(16) {
                        t.msha_fit.0 += 1;
                    } else {
                        t.msha_fit.1 += 1;
                        if t.msha_examples.len() < 10 {
                            t.msha_examples.push(format!(
                                "{label}: blob {} bytes, 276 + c0 {} + c1 {}",
                                b.len(),
                                u(12),
                                u(16)
                            ));
                        }
                    }
                }
            }
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
    let (by_path, by_hash) = map_counts(&retail);
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
                let counts = if ext == "kiloPack" { by_hash.get(&e.index) } else { by_path.get(&e.crc) };
                survey(&format!("{}#{i}", p.display()), sub, counts.copied(), &mut t);
            }
        } else if buf.get(0..4) == Some(sbla::MAGIC) {
            let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            survey(&p.display().to_string(), &buf, by_hash.get(&pandemic_hash(stem)).copied(), &mut t);
        }
    }
    println!("ALBS sub-packs: {}  parsed: {}  X=0 without map counts: {}", t.packs, t.ok, t.no_counts.len());
    for label in &t.no_counts {
        println!("  {label}");
    }
    println!("parse errors: {:?}", t.errors);
    for e in &t.error_examples {
        println!("  {e}");
    }
    println!("blob heads by (type, first 4 bytes, comp == uncomp) -> blobs:");
    for (k, v) in &t.heads {
        println!("  {k:?} -> {v}");
    }
    println!("type-0 AHSM blobs exactly 276 + c0 + c1 bytes: {} (others: {})", t.msha_fit.0, t.msha_fit.1);
    for e in &t.msha_examples {
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
