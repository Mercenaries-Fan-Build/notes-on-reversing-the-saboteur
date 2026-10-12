//! Retail gate for `sbla`: every ALBS sub-pack in the install parses exactly — counts from its
//! descriptor, or from its map object (`map6::object_counts`, keyed by the megapack entry's `index`, or
//! by `pandemic_hash(file stem)` for a DLC loose pack) — and writes back byte for byte.
//!
//! 39 `France/Mega0.megapack` object packs have an empty descriptor and no map object with their
//! `index`; they are listed in `UNKEYED` and must be exactly those.
//!
//! Game-gated: built by the `retail` feature, reads the install named by the repo-root
//! `.sab-local.toml`, and fails if it is absent.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use sab_formats::game_paths::local_config_retail_dir;
use sab_formats::{loosefiles, map6, megapack, pandemic_hash, sbla};

/// `France/Mega0.megapack` entries whose type counts are not in any map.
const UNKEYED: [usize; 39] = [
    10, 11, 12, 21, 22, 23, 28, 29, 30, 31, 41, 42, 43, 44, 45, 46, 61, 62, 63, 64, 65, 66, 67, 68, 69, 70, 71,
    72, 86, 87, 88, 89, 90, 91, 92, 93, 94, 95, 96,
];

fn retail() -> PathBuf {
    local_config_retail_dir(Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap_or_else(|e| panic!("{e}"))
}

fn counts(retail: &Path) -> BTreeMap<u32, [u32; 9]> {
    let lf = std::fs::read(retail.join("France/loosefiles_BinPC.pack")).expect("loosefiles_BinPC.pack");
    let es = loosefiles::entries(&lf).expect("loosefiles walk");
    let get = |n: &str| lf[loosefiles::find(&es, n).unwrap_or_else(|| panic!("{n}")).data.clone()].to_vec();
    let g = map6::parse_global(&get("global.map")).expect("global.map");
    let f = map6::parse_region_main(&get("France.map")).expect("France.map");
    let dg = map6::parse_global(&std::fs::read(retail.join("DLC/01/Global.map")).expect("DLC Global.map")).expect("DLC Global.map");
    let df = map6::parse_region_dlc(&std::fs::read(retail.join("DLC/01/FRANCE.map")).expect("DLC FRANCE.map")).expect("DLC FRANCE.map");
    map6::object_counts(&[&g, &dg], &[&f, &df]).expect("object counts")
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

/// Parse and write back one sub-pack; `Err` names what failed.
fn check(buf: &[u8], key: u32, counts: &BTreeMap<u32, [u32; 9]>) -> Result<(), String> {
    let x = sbla::descriptor_len(buf)?;
    let map = if x == 0 { Some(*counts.get(&key).ok_or("no map object")?) } else { None };
    let sp = sbla::parse(buf, map)?;
    let rebuilt = sbla::write(sp.descriptor.as_ref(), &sp.blobs())?;
    if rebuilt != buf {
        return Err("rebuild differs".into());
    }
    Ok(())
}

#[test]
fn every_retail_albs_parses_exactly_and_writes_back_byte_identically() {
    let retail = retail();
    let counts = counts(&retail);
    let mut all = Vec::new();
    files(&retail, &mut all);
    all.sort();
    let (mut ok, mut failures, mut unkeyed) = (0usize, Vec::new(), Vec::new());
    for p in &all {
        let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
        if ext == "megapack" || ext == "kiloPack" {
            let buf = std::fs::read(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
            let mp = megapack::parse(&buf).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
            for (i, e) in mp.entries.iter().enumerate() {
                let sub = megapack::entry_slice(&buf, e).unwrap_or_else(|err| panic!("{err}"));
                if sub.get(0..4) != Some(sbla::MAGIC) {
                    continue;
                }
                match check(sub, e.index, &counts) {
                    Ok(()) => ok += 1,
                    Err(err) if err == "no map object" && p.ends_with("France/Mega0.megapack") => unkeyed.push(i),
                    Err(err) => failures.push(format!("{}#{i}: {err}", p.display())),
                }
            }
        } else if (ext == "dynpack" || ext == "pack") && p.components().any(|c| c.as_os_str() == "DLC") {
            let buf = std::fs::read(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
            if buf.get(0..4) != Some(sbla::MAGIC) {
                continue;
            }
            let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            match check(&buf, pandemic_hash(stem), &counts) {
                Ok(()) => ok += 1,
                Err(err) => failures.push(format!("{}: {err}", p.display())),
            }
        }
    }
    eprintln!("{ok} ALBS sub-packs parse exactly and rebuild byte-identically");
    assert!(failures.is_empty(), "{} failures:\n{}", failures.len(), failures.join("\n"));
    assert_eq!(unkeyed, UNKEYED);
    assert_eq!(ok, 9171);
}
