//! Retail gate for the archive layer: every retail `.megapack` / `.kiloPack` rebuilds byte for byte from
//! its parsed entries, and `France/loosefiles_BinPC.pack` walks to its end with every stored name CRC
//! reproduced.
//!
//! Game-gated: built by the `retail` feature, reads the install named by the repo-root
//! `.sab-local.toml`, and fails if it is absent.

use std::path::{Path, PathBuf};

use sab_formats::game_paths::local_config_retail_dir;
use sab_formats::{loosefiles, map6, megapack};

fn retail() -> PathBuf {
    local_config_retail_dir(Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap_or_else(|e| panic!("{e}"))
}

/// Every file under `dir` whose extension is `.megapack` or `.kiloPack`, sorted.
fn packs(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap_or_else(|e| panic!("{}: {e}", d.display())) {
            let p = e.expect("dir entry").path();
            if p.is_dir() {
                stack.push(p);
            } else if matches!(p.extension().and_then(|x| x.to_str()), Some("megapack" | "kiloPack")) {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

#[test]
fn every_retail_megapack_rebuilds_byte_identically() {
    let all = packs(&retail());
    assert!(all.len() >= 10, "expected the 10 retail packs, found {}: {all:?}", all.len());
    for path in &all {
        let buf = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let mp = megapack::parse(&buf).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let items: Vec<megapack::Item> = mp
            .entries
            .iter()
            .map(|e| megapack::Item {
                crc: e.crc,
                index: e.index,
                data: megapack::entry_slice(&buf, e).unwrap_or_else(|err| panic!("{}: {err}", path.display())),
            })
            .collect();
        let rebuilt = megapack::write(&items);
        assert!(rebuilt == buf, "{}: rebuild differs ({} vs {} bytes)", path.display(), rebuilt.len(), buf.len());
        eprintln!("{}: {} entries, byte-identical", path.display(), mp.entries.len());
    }
}

#[test]
fn loosefiles_walks_to_its_end_and_every_name_crc_matches() {
    let path = retail().join("France").join("loosefiles_BinPC.pack");
    let buf = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let es = loosefiles::entries(&buf).unwrap_or_else(|e| panic!("{e}"));
    let names: Vec<&str> = es.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "France.shaders",
            r"Cinematics\Conversations\Conversations.cnvpack",
            r"Cinematics\Cinematics.cinpack",
            "global.map",
            "France.map",
            r"France\EditNodes\EditNodes.pack",
            "GameTemplates.wsd",
        ]
    );
    for e in &es {
        assert_eq!(e.name_crc, loosefiles::name_crc(&e.name), "{}", e.name);
    }
    assert_eq!(loosefiles::find(&es, "GLOBAL.MAP").map(|e| e.data.len()), Some(284_388));
    assert_eq!(loosefiles::find(&es, "france.map").map(|e| e.data.len()), Some(2_817_849));
}

/// The bytes of a `loosefiles_BinPC.pack` entry.
fn loose(name: &str) -> Vec<u8> {
    let path = retail().join("France").join("loosefiles_BinPC.pack");
    let buf = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let es = loosefiles::entries(&buf).unwrap_or_else(|e| panic!("{e}"));
    let e = loosefiles::find(&es, name).unwrap_or_else(|| panic!("{name} not in loosefiles_BinPC.pack"));
    buf[e.data.clone()].to_vec()
}

#[test]
fn global_maps_parse_exactly_and_write_back_byte_identically() {
    let main = loose("global.map");
    let g = map6::parse_global(&main).unwrap_or_else(|e| panic!("global.map: {e}"));
    assert_eq!((g.dynamics.len(), g.list1.len(), g.list2.len()), (761, 325, 325));
    assert_eq!(g.write().unwrap(), main);

    let dlc = std::fs::read(retail().join("DLC").join("01").join("Global.map")).expect("DLC Global.map");
    let d = map6::parse_global(&dlc).unwrap_or_else(|e| panic!("DLC Global.map: {e}"));
    let names: Vec<String> = d.dynamics.iter().map(|r| r.name()).collect();
    assert_eq!(names, ["MM_Belle_VIP", "KnifeThrow"]);
    for r in &d.dynamics {
        assert_eq!(r.hash, sab_formats::pandemic_hash(&r.name()));
    }
    assert_eq!(d.write().unwrap(), dlc);
}

#[test]
fn region_maps_parse_exactly_and_write_back_byte_identically() {
    let main = loose("France.map");
    let m = map6::parse_region_main(&main).unwrap_or_else(|e| panic!("France.map: {e}"));
    let h = m.header.as_ref().expect("main header");
    let extended = m.tiles.iter().filter(|t| t.tail.is_some()).count();
    assert_eq!((h.paths.len(), m.tiles.len(), extended, m.list2.len(), m.list3.len()), (1733, 29_572, 21_669, 13, 61));
    assert_eq!(m.write().unwrap(), main);

    let dlc = std::fs::read(retail().join("DLC").join("01").join("FRANCE.map")).expect("DLC FRANCE.map");
    let d = map6::parse_region_dlc(&dlc).unwrap_or_else(|e| panic!("DLC FRANCE.map: {e}"));
    let extended = d.tiles.iter().filter(|t| t.tail.is_some()).count();
    assert_eq!((d.tiles.len(), extended, d.list2.len(), d.list3.len()), (20, 9, 1, 4));
    assert_eq!(d.write().unwrap(), dlc);
}
