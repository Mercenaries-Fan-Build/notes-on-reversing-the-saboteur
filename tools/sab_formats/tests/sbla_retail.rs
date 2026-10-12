//! Retail gate for `sbla` and `mesh`: every ALBS sub-pack in the install parses exactly and writes
//! back byte for byte, and every type-0 blob is one MSHA record whose body and `.dat` decode to their
//! declared sizes. A pack with an empty descriptor takes its counts from its map object: for a `.megapack`
//! entry, the object whose pack path hashes to the entry's `crc` (`map6::pack_counts`); for a
//! `.kiloPack` entry, the object whose hash is the entry's `index`; for a DLC loose pack, the object
//! whose hash is `pandemic_hash(file stem)` (both `map6::object_counts`). Every streamblock entry of a
//! region megapack has `crc == pandemic_hash(map6::streamblock_path(region, index) + ".pack")`.
//!
//! Game-gated: built by the `retail` feature, reads the install named by the repo-root
//! `.sab-local.toml`, and fails if it is absent.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use sab_formats::game_paths::local_config_retail_dir;
use sab_formats::{loosefiles, map6, megapack, mesh, pandemic_hash, sbla};

fn retail() -> PathBuf {
    local_config_retail_dir(Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap_or_else(|e| panic!("{e}"))
}

struct Maps {
    by_path: BTreeMap<u32, [u32; 9]>,
    by_hash: BTreeMap<u32, [u32; 9]>,
    /// Region megapack file name prefix (lowercase) -> region name, from the region maps.
    regions: Vec<(String, String)>,
}

fn maps(retail: &Path) -> Maps {
    let lf = std::fs::read(retail.join("France/loosefiles_BinPC.pack")).expect("loosefiles_BinPC.pack");
    let es = loosefiles::entries(&lf).expect("loosefiles walk");
    let get = |n: &str| lf[loosefiles::find(&es, n).unwrap_or_else(|| panic!("{n}")).data.clone()].to_vec();
    let g = map6::parse_global(&get("global.map")).expect("global.map");
    let f = map6::parse_region_main(&get("France.map")).expect("France.map");
    let dg = map6::parse_global(&std::fs::read(retail.join("DLC/01/Global.map")).expect("DLC Global.map")).expect("DLC Global.map");
    let df = map6::parse_region_dlc(&std::fs::read(retail.join("DLC/01/FRANCE.map")).expect("DLC FRANCE.map")).expect("DLC FRANCE.map");
    let region = |m: &map6::RegionMap| String::from_utf8(m.region.clone()).expect("region name");
    Maps {
        by_path: map6::pack_counts(&[&g, &dg], &[&f, &df]).expect("pack counts"),
        by_hash: map6::object_counts(&[&g, &dg], &[&f, &df]).expect("object counts"),
        regions: vec![(format!("{}/mega", region(&f)).to_lowercase(), region(&f)), ("dlc/01/dlc01mega".into(), region(&df))],
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

/// Parse and write back one sub-pack, and decode every type-0 blob as one MSHA record; the number of
/// meshes, or `Err` naming what failed.
fn check(buf: &[u8], key: u32, counts: &BTreeMap<u32, [u32; 9]>) -> Result<usize, String> {
    let x = sbla::descriptor_len(buf)?;
    let map = if x == 0 { Some(*counts.get(&key).ok_or(format!("no map object 0x{key:08X}"))?) } else { None };
    let sp = sbla::parse(buf, map)?;
    let rebuilt = sbla::write(sp.descriptor.as_ref(), &sp.blobs())?;
    if rebuilt != buf {
        return Err("rebuild differs".into());
    }
    let mut meshes = 0;
    for (r, blob) in sp.records.iter().zip(&sp.blobs) {
        if r.kind == 0 {
            let m = mesh::parse_msha(blob)?;
            let (body, dat) = mesh::read_body_and_dat(blob, &m)?;
            if body.len() != m.unc0 as usize || dat.len() != m.unc1 as usize {
                return Err(format!("MSHA '{}' decoded to {} + {} bytes", m.name, body.len(), dat.len()));
            }
            meshes += 1;
        }
    }
    Ok(meshes)
}

#[test]
fn every_retail_albs_parses_exactly_and_writes_back_byte_identically() {
    let retail = retail();
    let maps = maps(&retail);
    let mut all = Vec::new();
    files(&retail, &mut all);
    all.sort();
    let (mut ok, mut meshes, mut streamblocks, mut failures) = (0usize, 0usize, 0usize, Vec::new());
    for p in &all {
        let rel = p.strip_prefix(&retail).expect("under retail").to_string_lossy().replace('\\', "/");
        let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
        if ext == "megapack" || ext == "kiloPack" {
            let region = maps.regions.iter().find(|(pre, _)| rel.to_lowercase().starts_with(pre)).map(|(_, r)| r);
            let buf = std::fs::read(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
            let mp = megapack::parse(&buf).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
            for (i, e) in mp.entries.iter().enumerate() {
                let sub = megapack::entry_slice(&buf, e).unwrap_or_else(|err| panic!("{err}"));
                if sub.get(0..4) != Some(sbla::MAGIC) {
                    continue;
                }
                let r = if ext == "kiloPack" { check(sub, e.index, &maps.by_hash) } else { check(sub, e.crc, &maps.by_path) };
                match r {
                    Ok(n) => (ok, meshes) = (ok + 1, meshes + n),
                    Err(err) => failures.push(format!("{rel}#{i}: {err}")),
                }
                if let Some(region) = region {
                    if sbla::descriptor_len(sub).expect("descriptor_len") != 0 {
                        let path = format!("{}.pack", map6::streamblock_path(region, e.index));
                        if pandemic_hash(&path) == e.crc {
                            streamblocks += 1;
                        } else {
                            failures.push(format!("{rel}#{i}: crc 0x{:08X} is not pandemic_hash(\"{path}\")", e.crc));
                        }
                    }
                }
            }
        } else if (ext == "dynpack" || ext == "pack") && rel.starts_with("DLC/") {
            let buf = std::fs::read(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
            if buf.get(0..4) != Some(sbla::MAGIC) {
                continue;
            }
            let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            match check(&buf, pandemic_hash(stem), &maps.by_hash) {
                Ok(n) => (ok, meshes) = (ok + 1, meshes + n),
                Err(err) => failures.push(format!("{rel}: {err}")),
            }
        }
    }
    eprintln!(
        "{ok} ALBS sub-packs parse exactly and rebuild byte-identically; {meshes} MSHA records decode; \
         {streamblocks} streamblock keys match"
    );
    assert!(failures.is_empty(), "{} failures:\n{}", failures.len(), failures.join("\n"));
    assert_eq!(ok, 9210);
    assert_eq!(meshes, 208_681);
    assert_eq!(streamblocks, 7914);
}
