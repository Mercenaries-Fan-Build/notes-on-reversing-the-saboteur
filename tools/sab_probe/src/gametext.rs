//! `sab_probe gametext` — read-only inspection of a `GameText.dlg`. Parsing/writing lives in
//! `sab_formats::gametext`; authoring (set/add) is in that crate's `sab_gametext` bin. This asks
//! questions only.

use sab_formats::gametext::{GameText, Record};
use sab_formats::pandemic_hash;

pub fn run(args: &[String]) {
    let sub = args.first().map(|s| s.as_str()).unwrap_or("");
    if sub == "hash" {
        match args.get(1) {
            Some(s) => println!("0x{:08x}  pandemic_hash({s:?})", pandemic_hash(s)),
            None => eprintln!("usage: sab_probe gametext hash <string>"),
        }
        return;
    }
    let Some(path) = args.get(1) else { return usage() };
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("read {path}: {e}");
            std::process::exit(1);
        }
    };
    let gt = match GameText::parse(&bytes) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("parse {path}: {e}");
            std::process::exit(1);
        }
    };
    let rest = &args[2.min(args.len())..];
    match sub {
        "info" => info(&gt),
        "list" => list(&gt, rest),
        "get" => get(&gt, rest),
        "roundtrip" => roundtrip(&gt, &bytes, path),
        _ => usage(),
    }
}

fn usage() {
    eprintln!("sab_probe gametext <info|list|get|hash|roundtrip> <GameText.dlg> [opts]");
    eprintln!("  info                              header + UI/VO + DNEC counts");
    eprintln!("  list  [--ui|--vo|--dnec] [--limit N]");
    eprintln!("  get   (--id <DottedID> | --hash 0x..) [--scene 0x..]");
    eprintln!("  hash  <string>                    pandemic_hash of a text id");
    eprintln!("  roundtrip                         assert re-emit is byte-identical");
}

fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).map(|s| s.as_str())
}
fn has(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}
fn preview(r: &Record) -> String {
    // char-boundary-safe: some strings have multi-byte UTF-8 at byte 60 (String::truncate panics).
    r.text_string().chars().take(60).collect()
}

fn info(gt: &GameText) {
    let ui = gt.records.iter().filter(|r| r.is_ui()).count();
    let vo = gt.records.len() - ui;
    println!("version={}  base_records={}  (UI={ui}, VO={vo})", gt.version, gt.records.len());
    println!("total_string_code_units={}", gt.total_code_units());
    let groups = gt.dnec_groups().len();
    if groups > 0 {
        println!("DNEC: {groups} scene groups, {} sub-records", gt.dnec_record_count());
    } else {
        println!("tail: no DNEC section");
    }
}

fn list(gt: &GameText, rest: &[String]) {
    let limit: usize = flag(rest, "--limit").and_then(|s| s.parse().ok()).unwrap_or(50);
    let mut n = 0;
    if has(rest, "--dnec") {
        'outer: for s in gt.dnec_groups() {
            for r in &s.records {
                println!("scene 0x{:08x}  0x{:08x} {:<40} {:?}", s.scene_hash, r.asset_id, r.key_str(), preview(r));
                n += 1;
                if n >= limit {
                    println!("... (--limit {limit} reached)");
                    break 'outer;
                }
            }
        }
        return;
    }
    let (only_ui, only_vo) = (has(rest, "--ui"), has(rest, "--vo"));
    for r in &gt.records {
        if (only_ui && !r.is_ui()) || (only_vo && r.is_ui()) {
            continue;
        }
        println!("0x{:08x} [{}] {:<40} {:?}", r.asset_id, if r.is_ui() { "UI" } else { "VO" }, r.key_str(), preview(r));
        n += 1;
        if n >= limit {
            println!("... (--limit {limit} reached)");
            break;
        }
    }
}

fn get(gt: &GameText, rest: &[String]) {
    let id = if let Some(d) = flag(rest, "--id") {
        pandemic_hash(d)
    } else if let Some(h) = flag(rest, "--hash") {
        match u32::from_str_radix(h.trim_start_matches("0x"), 16) {
            Ok(v) => v,
            Err(_) => {
                eprintln!("bad --hash {h:?}");
                return;
            }
        }
    } else {
        eprintln!("need --id <DottedID> or --hash 0x..");
        return;
    };
    let scene = flag(rest, "--scene").and_then(|s| u32::from_str_radix(s.trim_start_matches("0x"), 16).ok());
    match gt.find_any(id, scene) {
        Some(r) => println!("0x{:08x}  {:?}", r.asset_id, r.text_string()),
        None => eprintln!("no record with asset_id 0x{id:08x}"),
    }
}

fn roundtrip(gt: &GameText, orig: &[u8], path: &str) {
    let out = gt.write();
    let ok = out == orig;
    println!(
        "roundtrip {path}: base={} dnec_groups={} dnec_records={}  BYTE_IDENTICAL={ok}",
        gt.records.len(),
        gt.dnec_groups().len(),
        gt.dnec_record_count()
    );
    if !ok {
        std::process::exit(1);
    }
}
