//! Game gate for `proto2008` and `xex` over the 2008 prototype named by `proto2008_dir` in the repo-root
//! `.sab-local.toml`: every `.pack` is an MP00 archive whose entries tile it and are byte copies of the
//! loose files they hash the paths of, an `SBL*` sub-pack, `AP04`, or empty; every `meshes` and
//! `textures` slot directory reads; every `meshes` record parses; every `textures` record parses, is
//! keyed by its name's hash, inflates to `unpacked` bytes that its mip layout spans exactly, and every
//! level reads; every `wsd` slot is a big-endian EditNodes entry; every `.xex` unwraps.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use sab_formats::game_paths::local_config_proto2008_bin360;
use sab_formats::proto2008::sbl::{self, SlotKind};
use sab_formats::proto2008::{mesh, mp00, slot_dir, texture};
use sab_formats::{editnodes, pandemic_hash, xex, zlib};

fn bin360() -> PathBuf {
    local_config_proto2008_bin360(Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap_or_else(|e| panic!("{e}"))
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
struct Counts {
    sbl: usize,
    mesh_records: usize,
    variant_records: usize,
    wsd_slots: usize,
    textures: usize,
}

fn check_sbl(buf: &[u8], at: &str, c: &mut Counts, failures: &mut Vec<String>) {
    let sp = match sbl::parse(buf) {
        Ok(sp) => sp,
        Err(e) => return failures.push(format!("{at}: {e}")),
    };
    c.sbl += 1;
    for (r, slot) in sp.records.iter().zip(&sp.slots) {
        match r.kind {
            SlotKind::Textures => {
                let recs = match slot_dir::parse(slot) {
                    Ok(r) => r,
                    Err(e) => return failures.push(format!("{at} textures: {e}")),
                };
                for (i, rec) in recs.iter().filter(|r| r.stored != 0).enumerate() {
                    let d = &slot[rec.offset as usize..(rec.offset + rec.stored) as usize];
                    let checked = texture::parse(d).and_then(|t| {
                        if pandemic_hash(&t.name) != rec.hash {
                            return Err(format!("record hash {:08X} is not pandemic_hash('{}')", rec.hash, t.name));
                        }
                        let memory = t.memory()?;
                        let (levels, end) = texture::levels(&t)?;
                        if end != memory.len() {
                            return Err(format!("'{}': mip layout spans {end} bytes, image is {}", t.name, memory.len()));
                        }
                        levels.iter().try_for_each(|l| texture::level_bytes(&t, &memory, l).map(drop))
                    });
                    match checked {
                        Ok(()) => c.textures += 1,
                        Err(e) => failures.push(format!("{at} texture {i}: {e}")),
                    }
                }
            }
            SlotKind::Meshes => {
                let recs = match slot_dir::parse(slot) {
                    Ok(r) => r,
                    Err(e) => return failures.push(format!("{at} meshes: {e}")),
                };
                for (i, rec) in recs.iter().filter(|r| r.stored != 0).enumerate() {
                    let d = &slot[rec.offset as usize..(rec.offset + rec.stored) as usize];
                    let body = if rec.stored < rec.unpacked {
                        match zlib::inflate_exact(d, rec.unpacked as usize, "mesh") {
                            Ok(b) => b,
                            Err(e) => {
                                failures.push(format!("{at} mesh {i}: {e}"));
                                continue;
                            }
                        }
                    } else {
                        d.to_vec()
                    };
                    match mesh::parse_record(&body) {
                        Ok(mesh::Record::Mesh(_)) => c.mesh_records += 1,
                        Ok(mesh::Record::Variant { .. }) => c.variant_records += 1,
                        Err(e) => failures.push(format!("{at} mesh {i}: {e}")),
                    }
                }
            }
            SlotKind::Wsd => match editnodes::parse_entry_be(slot) {
                Ok(_) => c.wsd_slots += 1,
                Err(e) => failures.push(format!("{at} wsd: {e}")),
            },
            _ => {}
        }
    }
}

#[test]
fn every_2008_pack_and_record_parses() {
    let root = bin360();
    let mut all = Vec::new();
    files(&root, &mut all);
    all.sort();
    let mut c = Counts::default();
    let mut failures = Vec::new();
    let mut loose: HashMap<String, Vec<u8>> = HashMap::new();
    let mut megas = Vec::new();
    let (mut empty, mut ap04) = (0usize, 0usize);
    for p in &all {
        let rel = p.strip_prefix(&root).expect("under root").to_string_lossy().replace('/', "\\");
        if !rel.to_ascii_lowercase().ends_with(".pack") {
            continue;
        }
        let mut f = std::fs::File::open(p).unwrap_or_else(|e| panic!("{rel}: {e}"));
        let mut head = [0u8; 4];
        if f.read(&mut head).expect("read head") < 4 {
            empty += 1;
            continue;
        }
        match &head {
            m if m == mp00::MAGIC => megas.push((p.clone(), rel)),
            b"AP04" => ap04 += 1,
            m if m.starts_with(b"SBL") => {
                let buf = std::fs::read(p).unwrap_or_else(|e| panic!("{rel}: {e}"));
                check_sbl(&buf, &rel, &mut c, &mut failures);
                loose.insert(rel.to_ascii_lowercase(), buf);
            }
            m => failures.push(format!("{rel}: unknown magic {m:02X?}")),
        }
    }
    let (mut entries, mut copies) = (0usize, 0usize);
    for (p, rel) in &megas {
        let mut f = std::fs::File::open(p).expect("open mega");
        let len = f.metadata().expect("metadata").len();
        let mut hdr = [0u8; 8];
        f.read_exact(&mut hdr).expect("mega header");
        let mut dir = vec![0u8; mp00::index_len(&hdr).unwrap_or_else(|e| panic!("{rel}: {e}"))];
        f.seek(SeekFrom::Start(0)).expect("seek");
        f.read_exact(&mut dir).expect("mega directory");
        let es = mp00::parse_index(&dir).unwrap_or_else(|e| panic!("{rel}: {e}"));
        if let Err(e) = mp00::check_layout(&es, len) {
            failures.push(format!("{rel}: {e}"));
        }
        for e in &es {
            entries += 1;
            let mut buf = vec![0u8; e.size as usize];
            f.seek(SeekFrom::Start(e.offset)).expect("seek entry");
            f.read_exact(&mut buf).expect("read entry");
            if loose.iter().any(|(path, b)| pandemic_hash(path) == e.hash && *b == buf) {
                copies += 1;
            }
        }
    }
    let mut unwrapped = 0;
    for p in all.iter().filter(|p| p.extension().and_then(|x| x.to_str()) == Some("xex")) {
        let buf = std::fs::read(p).expect("read xex");
        let r = xex::parse(&buf).and_then(|h| xex::unwrap(&buf, &h, &xex::DEVKIT_KEK));
        match r {
            Ok(_) => unwrapped += 1,
            Err(e) => failures.push(format!("{}: {e}", p.display())),
        }
    }
    eprintln!(
        "{} SBL sub-packs, {} mesh records + {} variant records, {} textures, {} wsd slots; {entries} MP00 \
         entries, {copies} byte copies of a loose SBL .pack keyed by its path; {unwrapped} .xex unwrapped",
        c.sbl, c.mesh_records, c.variant_records, c.textures, c.wsd_slots
    );
    assert!(failures.is_empty(), "{} failures:\n{}", failures.len(), failures.join("\n"));
    assert_eq!((empty, ap04, megas.len()), (2, 1, 4));
    assert_eq!(c.sbl, 4389);
    assert_eq!(c.mesh_records + c.variant_records, 137_861);
    assert_eq!(c.variant_records, 215);
    assert_eq!(c.textures, 47_313);
    assert_eq!(entries, 4960);
    assert_eq!(copies, 4385);
    assert_eq!(unwrapped, 6);
}
