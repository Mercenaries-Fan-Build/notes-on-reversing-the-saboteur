//! `proto2008-packs <bin360>` — read every `.pack` under the 2008 prototype's `Bin360` and tally the
//! container facts `docs/2008_subpack_format.md` cites: files by magic, SBL slot-type sets by magic,
//! `block_hash` against names of the file, MP00 entries against the loose `.Pack` files, and the
//! `meshes` / `textures` slot directories' record words.

use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use sab_formats::pandemic_hash;
use sab_formats::proto2008::sbl::{self, SlotKind};
use sab_formats::proto2008::{mp00, slot_dir};

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
    magic_files: BTreeMap<String, usize>,
    kind_sets: BTreeMap<(String, String), usize>,
    block_hash_consistent: BTreeMap<bool, usize>,
    block_hash_names: BTreeMap<&'static str, usize>,
    heads: BTreeMap<(SlotKind, String), usize>,
    ids: BTreeMap<(SlotKind, &'static str), usize>,
    dir_examples: BTreeMap<(SlotKind, String), String>,
    id_examples: BTreeMap<SlotKind, String>,
    dir: BTreeMap<(SlotKind, String), usize>,
    failures: Vec<String>,
}

fn magic_str(m: &[u8]) -> String {
    m.iter().map(|&c| if c.is_ascii_graphic() { c as char } else { '.' }).collect()
}

/// The slot's first four bytes when they are letters or digits, else the first big-endian u32's size
/// class.
fn head(b: &[u8]) -> String {
    match b.get(..4) {
        Some(m) if m.iter().all(|c| c.is_ascii_alphanumeric()) => magic_str(m),
        Some(m) => {
            let w = u32::from_be_bytes(m.try_into().expect("4 bytes"));
            format!("u32 {}", if w < 0x1_0000 { "< 0x10000" } else { ">= 0x10000" })
        }
        None => format!("<{} bytes>", b.len()),
    }
}

/// A failure message with its numbers replaced by `#`, for grouping.
fn shape(msg: &str) -> String {
    let mut out = String::new();
    let mut prev_digit = false;
    for c in msg.chars() {
        let digit = c.is_ascii_hexdigit() && (prev_digit || c.is_ascii_digit());
        if digit {
            if !prev_digit {
                out.push('#');
            }
        } else {
            out.push(c);
        }
        prev_digit = digit;
    }
    out
}

const ZERO: slot_dir::Record = slot_dir::Record { hash: 0, offset: 0, stored: 0, unpacked: 0, f10: 0, f14: 0 };

/// Slot directory facts for one `meshes` / `textures` slot.
fn survey_dir(kind: SlotKind, slot: &[u8], t: &mut Tally, at: &str) {
    let recs = match slot_dir::parse(slot) {
        Ok(r) => r,
        Err(e) => {
            t.failures.push(format!("{at} {}: {e}", kind.name()));
            return;
        }
    };
    let mut note = |s: String| {
        t.dir_examples.entry((kind, s.clone())).or_insert_with(|| at.to_string());
        *t.dir.entry((kind, s)).or_default() += 1
    };
    let dir_end = 4 + recs.len() * slot_dir::RECORD_LEN;
    let live: Vec<&slot_dir::Record> = recs.iter().filter(|r| **r != ZERO).collect();
    note(format!("all-zero records present: {}", live.len() != recs.len()));
    let zero_pos: Vec<usize> = recs.iter().enumerate().filter(|(_, r)| **r == ZERO).map(|(i, _)| i).collect();
    if !zero_pos.is_empty() {
        let trailing = zero_pos.iter().rev().enumerate().all(|(k, &i)| i == recs.len() - 1 - k);
        note(format!("all-zero records are trailing: {trailing}"));
    }
    let mut cursor = dir_end as u64;
    let mut contiguous = true;
    for r in &live {
        note(format!("f10={} f14={}", r.f10, r.f14));
        let rel = if r.stored == r.unpacked { "==" } else if r.stored < r.unpacked { "<" } else { ">" };
        note(format!("stored {rel} unpacked"));
        if r.offset as u64 != cursor {
            contiguous = false;
            note(format!("gap before record: {}", r.offset as i64 - cursor as i64));
        }
        cursor = r.offset as u64 + r.stored as u64;
        let d = &slot[r.offset as usize..(r.offset + r.stored) as usize];
        note(format!("data head {:02X?} stored {rel} unpacked", &d[..d.len().min(2)]));
    }
    note(format!("contiguous from dir end: {contiguous}"));
    note(format!("tail after last record: {}", slot.len() as i64 - cursor as i64));
}

fn survey_sbl(buf: &[u8], at: &str, stem: &str, rel: &str, t: &mut Tally) {
    let sp = match sbl::parse(buf) {
        Ok(sp) => sp,
        Err(e) => {
            t.failures.push(format!("{at}: {e}"));
            return;
        }
    };
    let magic = magic_str(&sp.magic);
    let set: Vec<&str> = sp.records.iter().map(|r| r.kind.name()).collect();
    let source = if rel.is_empty() { "MP00 entry" } else { "loose file" };
    *t.kind_sets.entry((format!("{source} {magic}"), set.join("+"))).or_default() += 1;
    let bh = sp.records[0].id;
    *t.block_hash_consistent.entry(sp.records.iter().all(|r| r.id == bh)).or_default() += 1;
    let cands: [(&'static str, String); 4] = [
        ("stem", stem.to_string()),
        ("stem.pack", format!("{stem}.pack")),
        ("rel path", rel.to_string()),
        ("rel path no ext", rel.rsplit_once('.').map(|(a, _)| a.to_string()).unwrap_or_default()),
    ];
    let hit = cands.iter().find(|(_, s)| !s.is_empty() && sp.records.iter().any(|r| r.id == pandemic_hash(s)));
    let label = match hit {
        Some((n, _)) => *n,
        None if bh == 0 => "zero",
        None => "none",
    };
    *t.block_hash_names.entry(label).or_default() += 1;
    for (r, slot) in sp.records.iter().zip(&sp.slots) {
        let id = match r.id {
            0 => "0",
            h => match sab_formats::names::name(h) {
                Some(n) => {
                    t.id_examples.entry(r.kind).or_insert_with(|| n.to_string());
                    "named in hash_names.tsv"
                }
                None => "not in hash_names.tsv",
            },
        };
        *t.ids.entry((r.kind, id)).or_default() += 1;
        *t.heads.entry((r.kind, head(slot))).or_default() += 1;
        if matches!(r.kind, SlotKind::Meshes | SlotKind::Textures) {
            survey_dir(r.kind, slot, t, at);
        }
    }
}

pub fn run(args: &[String]) {
    let [bin360] = args else {
        eprintln!("usage: sab_probe proto2008-packs <Bin360 dir>");
        std::process::exit(2);
    };
    let root = PathBuf::from(bin360);
    let mut all = Vec::new();
    files(&root, &mut all);
    all.sort();
    let mut t = Tally::default();
    let mut loose_by_bytes: HashMap<(u64, u32), Vec<String>> = HashMap::new();
    let mut megas = Vec::new();
    for p in &all {
        let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if !name.to_ascii_lowercase().ends_with(".pack") {
            continue;
        }
        let rel = p.strip_prefix(&root).expect("under root").to_string_lossy().replace('\\', "/");
        let mut f = std::fs::File::open(p).unwrap_or_else(|e| panic!("{rel}: {e}"));
        let len = f.metadata().expect("metadata").len();
        if len < 4 {
            *t.magic_files.entry(format!("<{len} bytes>")).or_default() += 1;
            continue;
        }
        let mut m = [0u8; 4];
        f.read_exact(&mut m).expect("magic");
        *t.magic_files.entry(magic_str(&m)).or_default() += 1;
        if &m == mp00::MAGIC {
            megas.push((p.clone(), rel));
            continue;
        }
        if m.starts_with(b"SBL") {
            let buf = std::fs::read(p).unwrap_or_else(|e| panic!("{rel}: {e}"));
            let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            survey_sbl(&buf, &rel, stem, &rel, &mut t);
        }
    }
    for p in &all {
        let rel = p.strip_prefix(&root).expect("under root").to_string_lossy().replace('\\', "/");
        if megas.iter().any(|(_, r)| *r == rel) {
            continue;
        }
        let buf = std::fs::read(p).unwrap_or_else(|e| panic!("{rel}: {e}"));
        loose_by_bytes.entry((buf.len() as u64, crc32(&buf))).or_default().push(rel);
    }
    let mut mega_match: BTreeMap<String, usize> = BTreeMap::new();
    for (p, rel) in &megas {
        let mut f = std::fs::File::open(p).expect("open mega");
        let len = f.metadata().expect("metadata").len();
        let mut hdr = [0u8; 8];
        f.read_exact(&mut hdr).expect("mega header");
        let n = mp00::index_len(&hdr).unwrap_or_else(|e| panic!("{rel}: {e}"));
        let mut head = vec![0u8; n];
        f.seek(SeekFrom::Start(0)).expect("seek");
        f.read_exact(&mut head).expect("mega directory");
        let es = mp00::parse_index(&head).unwrap_or_else(|e| panic!("{rel}: {e}"));
        if let Err(e) = mp00::check_layout(&es, len) {
            t.failures.push(format!("{rel}: {e}"));
        }
        println!("{rel}: {} entries, {len} bytes", es.len());
        let dir = rel.rsplit_once('/').map(|(d, _)| d).unwrap_or("").to_string();
        for (i, e) in es.iter().enumerate() {
            let mut buf = vec![0u8; e.size as usize];
            f.seek(SeekFrom::Start(e.offset)).expect("seek entry");
            f.read_exact(&mut buf).unwrap_or_else(|err| panic!("{rel}#{i}: {err}"));
            let kind = if buf.is_empty() {
                "empty".to_string()
            } else if buf.starts_with(b"SBL") {
                survey_sbl(&buf, &format!("{rel}#{i}"), "", "", &mut t);
                "SBL".to_string()
            } else {
                format!("head {:?}", magic_str(&buf[..buf.len().min(4)]))
            };
            let key = match loose_by_bytes.get(&(buf.len() as u64, crc32(&buf))) {
                Some(v) => {
                    let forms = |r: &str| -> Vec<(&'static str, String)> {
                        let file = r.rsplit_once('/').map(|(_, s)| s).unwrap_or(r).to_string();
                        let stem = file.rsplit_once('.').map(|(a, _)| a.to_string()).unwrap_or(file.clone());
                        let no_ext = r.rsplit_once('.').map(|(a, _)| a.to_string()).unwrap_or(r.to_string());
                        vec![
                            ("rel/", r.to_string()),
                            ("rel\\", r.replace('/', "\\")),
                            ("rel no ext /", no_ext.clone()),
                            ("rel no ext \\", no_ext.replace('/', "\\")),
                            ("file", file),
                            ("stem", stem),
                        ]
                    };
                    let form = v.iter().flat_map(|r| forms(r)).find(|(_, s)| pandemic_hash(s) == e.hash).map(|(n, _)| n);
                    let exts: Vec<String> = v
                        .iter()
                        .map(|r| r.rsplit_once('.').map(|(_, x)| x.to_ascii_lowercase()).unwrap_or_default())
                        .collect::<std::collections::BTreeSet<_>>()
                        .into_iter()
                        .collect();
                    format!("{dir} {kind}: identical loose file .{}; hash = ph({})", exts.join("/."), form.unwrap_or("none of the path forms"))
                }
                None => format!("{dir} {kind}: no identical loose file"),
            };
            *mega_match.entry(key).or_default() += 1;
        }
    }

    println!("\nfiles by magic");
    for (m, n) in &t.magic_files {
        println!("  {m:<12} {n}");
    }
    println!("\nslot-type sets by source and magic");
    for ((m, s), n) in &t.kind_sets {
        println!("  {m} {n:>6}  {s}");
    }
    println!("\nblock_hash same in every record: {:?}", t.block_hash_consistent);
    println!("block_hash equals pandemic_hash of: {:?}", t.block_hash_names);
    println!("\nrecord word +0x14 by slot type");
    for ((k, s), n) in &t.ids {
        println!("  {:<12} {s:<24} {n}", k.name());
    }
    for (k, n) in &t.id_examples {
        println!("  {:<12} e.g. {n}", k.name());
    }
    println!("\nslot heads");
    for ((k, h), n) in &t.heads {
        println!("  {:<12} {h:<6} {n}", k.name());
    }
    println!("\nslot directories");
    for ((k, s), n) in &t.dir {
        println!("  {:<9} {s:<40} {n:>7}  e.g. {}", k.name(), t.dir_examples[&(*k, s.clone())]);
    }
    println!("\nMP00 entries");
    for (k, n) in &mega_match {
        println!("  {k:<80} {n}");
    }
    println!("\n{} failures", t.failures.len());
    let mut groups: BTreeMap<String, Vec<&String>> = BTreeMap::new();
    for f in &t.failures {
        let msg = f.split_once(": ").map(|(_, m)| m).unwrap_or(f);
        groups.entry(shape(msg)).or_default().push(f);
    }
    for (g, fs) in &groups {
        println!("  {:>6}  {g}", fs.len());
        for f in fs.iter().take(3) {
            println!("            e.g. {f}");
        }
    }
}

/// CRC-32, to key the loose packs by content.
fn crc32(b: &[u8]) -> u32 {
    let mut c = flate2::Crc::new();
    c.update(b);
    c.sum()
}
