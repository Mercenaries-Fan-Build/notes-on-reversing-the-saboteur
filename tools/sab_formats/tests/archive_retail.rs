//! Retail gate for the archive layer: every retail `.megapack` / `.kiloPack` rebuilds byte for byte from
//! its parsed entries, and `France/loosefiles_BinPC.pack` walks to its end with every stored name CRC
//! reproduced.
//!
//! Game-gated: built by the `retail` feature, reads the install named by the repo-root
//! `.sab-local.toml`, and fails if it is absent.

use std::path::{Path, PathBuf};

use sab_formats::game_paths::local_config_retail_dir;
use sab_formats::{loosefiles, megapack};

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
