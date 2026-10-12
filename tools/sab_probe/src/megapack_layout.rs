//! `megapack-layout <megapack>...` — the byte-level layout of each megapack beyond its index: what
//! `table2` holds, how the data is ordered and aligned, which byte values fill the header padding,
//! the gaps between sub-packs and the tail, and whether a canonical rebuild reproduces the file.

use std::collections::BTreeMap;

use sab_formats::megapack;

const SECTOR: u64 = 2048;

fn align_up(v: u64) -> u64 {
    v.div_ceil(SECTOR) * SECTOR
}

/// Histogram of the byte values in `b`, as `{value: count}`.
fn byte_set(b: &[u8]) -> BTreeMap<u8, usize> {
    let mut m = BTreeMap::new();
    for &x in b {
        *m.entry(x).or_default() += 1;
    }
    m
}

fn fmt_set(m: &BTreeMap<u8, usize>) -> String {
    if m.is_empty() {
        return "(none)".into();
    }
    m.iter().map(|(k, v)| format!("0x{k:02X}x{v}")).collect::<Vec<_>>().join(" ")
}

pub fn run(args: &[String]) {
    if args.is_empty() {
        eprintln!("usage: sab_probe megapack-layout <megapack>...");
        std::process::exit(2);
    }
    for path in args {
        let buf = std::fs::read(path).unwrap_or_else(|e| {
            eprintln!("read {path}: {e}");
            std::process::exit(1)
        });
        let mp = megapack::parse(&buf).unwrap_or_else(|e| {
            eprintln!("{path}: {e}");
            std::process::exit(1)
        });
        let n = mp.entries.len();
        println!("== {path}");
        println!("  bytes {}  entries {n}", buf.len());

        // table2: n x {u32, u32} right after the 20-byte index entries.
        let t2 = 8 + n * 20;
        let header_end = t2 + n * 8;
        let mut t2_mirrors = 0usize;
        for (i, e) in mp.entries.iter().enumerate() {
            let o = t2 + i * 8;
            let a = u32::from_le_bytes(buf[o..o + 4].try_into().unwrap());
            let b = u32::from_le_bytes(buf[o + 4..o + 8].try_into().unwrap());
            if a == e.crc && b == e.index {
                t2_mirrors += 1;
            }
        }
        println!("  table2 == (crc,index) of the same entry: {t2_mirrors}/{n}");

        // Data order and alignment.
        let mut order: Vec<usize> = (0..n).collect();
        order.sort_by_key(|&i| mp.entries[i].offset);
        let in_index_order = order.iter().enumerate().all(|(k, &i)| k == i);
        let aligned = mp.entries.iter().filter(|e| e.offset % SECTOR == 0).count();
        println!("  data in index order: {in_index_order}   2048-aligned offsets: {aligned}/{n}");
        let first = mp.entries.iter().map(|e| e.offset).min().unwrap_or(header_end as u64);
        println!(
            "  header ends 0x{header_end:X}; first data at 0x{first:X} (align_up(header) = 0x{:X})",
            align_up(header_end as u64)
        );

        // Fill bytes: header pad, inter-entry gaps (in offset order), and the tail.
        let header_pad = byte_set(&buf[header_end..first as usize]);
        let mut gaps: BTreeMap<u8, usize> = BTreeMap::new();
        let mut gap_lens: BTreeMap<String, usize> = BTreeMap::new();
        for w in order.windows(2) {
            let (a, b) = (&mp.entries[w[0]], &mp.entries[w[1]]);
            let end = a.offset + a.size as u64;
            let kind = if b.offset == align_up(end) { "align_up(end)" } else { "other" };
            *gap_lens.entry(kind.into()).or_default() += 1;
            for (k, v) in byte_set(&buf[end as usize..b.offset as usize]) {
                *gaps.entry(k).or_default() += v;
            }
        }
        let last = order.last().map(|&i| &mp.entries[i]);
        let tail_start = last.map(|e| e.offset + e.size as u64).unwrap_or(first) as usize;
        let tail = byte_set(&buf[tail_start..]);
        println!("  header pad bytes: {}", fmt_set(&header_pad));
        println!("  gaps between sub-packs: next offset = {gap_lens:?}; bytes: {}", fmt_set(&gaps));
        println!(
            "  tail after last sub-pack: {} bytes, file length 2048-aligned: {}; bytes: {}",
            buf.len() - tail_start,
            buf.len() as u64 % SECTOR == 0,
            fmt_set(&tail)
        );

        // Canonical rebuild with each fill byte seen: does it reproduce the file exactly?
        let mut fills: Vec<u8> = header_pad.keys().chain(gaps.keys()).chain(tail.keys()).copied().collect();
        fills.sort_unstable();
        fills.dedup();
        for (hp, gp) in fills.iter().flat_map(|&h| fills.iter().map(move |&g| (h, g))) {
            let rebuilt = rebuild(&buf, &mp, &order, header_end, hp, gp);
            println!(
                "  rebuild (header pad 0x{hp:02X}, data pad 0x{gp:02X}): {}",
                if rebuilt == buf { "BYTE-IDENTICAL".to_string() } else { first_diff(&rebuilt, &buf) }
            );
        }
    }
}

/// Index + table2 from the parsed entries, header padded with `hp` to the first sector, sub-packs at
/// their original offsets in offset order, each padded with `gp` to the next sector.
fn rebuild(
    buf: &[u8],
    mp: &megapack::Megapack,
    order: &[usize],
    header_end: usize,
    hp: u8,
    gp: u8,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(buf.len());
    out.extend_from_slice(megapack::MAGIC);
    out.extend_from_slice(&(mp.entries.len() as u32).to_le_bytes());
    for e in &mp.entries {
        out.extend_from_slice(&e.crc.to_le_bytes());
        out.extend_from_slice(&e.index.to_le_bytes());
        out.extend_from_slice(&e.size.to_le_bytes());
        out.extend_from_slice(&e.offset.to_le_bytes());
    }
    for e in &mp.entries {
        out.extend_from_slice(&e.crc.to_le_bytes());
        out.extend_from_slice(&e.index.to_le_bytes());
    }
    assert_eq!(out.len(), header_end);
    out.resize(align_up(header_end as u64) as usize, hp);
    for &i in order {
        let e = &mp.entries[i];
        out.resize(e.offset as usize, gp);
        out.extend_from_slice(&buf[e.offset as usize..e.offset as usize + e.size as usize]);
    }
    out.resize(align_up(out.len() as u64) as usize, gp);
    out
}

fn first_diff(a: &[u8], b: &[u8]) -> String {
    match a.iter().zip(b).position(|(x, y)| x != y) {
        Some(p) => format!("differs first at 0x{p:X} (rebuilt 0x{:02X}, file 0x{:02X})", a[p], b[p]),
        None => format!("length differs (rebuilt {}, file {})", a.len(), b.len()),
    }
}
