// sab_gametext — CLI for The Saboteur (2009) GameText.dlg (the complete localized-text container,
// one file per language under Cinematics/Dialog/<Lang>/).
//
// This is a thin front-end. All parsing/writing/editing lives in `sab_formats::gametext` so the
// Workshop, the validator and this CLI share one implementation. Format spec + confidence table:
// docs/formats/gametext.md.

use sab_formats::gametext::{GameText, Record};
use sab_formats::pandemic_hash;
use std::env;
use std::fs;
use std::process::exit;

// ---------------------------------------------------------------- arg helpers
fn get_flag(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1).cloned())
}
fn has_flag(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}
fn parse_hex_u32(s: &str) -> Result<u32, String> {
    u32::from_str_radix(s.trim_start_matches("0x"), 16).map_err(|_| format!("bad hex {s:?}"))
}

/// Resolve the target asset_id from `--id`/`--hash`, and an optional DNEC `--scene` filter.
fn resolve_id(args: &[String]) -> Result<(u32, Option<u32>), String> {
    let id = if let Some(ids) = get_flag(args, "--id") {
        pandemic_hash(&ids)
    } else if let Some(hs) = get_flag(args, "--hash") {
        parse_hex_u32(&hs)?
    } else {
        return Err("need --id <DottedID> or --hash 0x...".into());
    };
    let scene = match get_flag(args, "--scene") {
        Some(s) => Some(parse_hex_u32(&s)?),
        None => None,
    };
    Ok((id, scene))
}

fn preview(r: &Record) -> String {
    let mut t = r.text_string();
    t.truncate(60);
    t
}

fn usage() {
    eprintln!(
        "sab_gametext — The Saboteur GameText.dlg reader/writer (CLI over sab_formats::gametext)\n\
\n\
  hash  <string>                       print pandemic_hash of a text id\n\
  info  <in.dlg>                       header + UI/VO counts + DNEC group/record counts\n\
  list  <in.dlg> [--ui|--vo|--dnec] [--limit N]   list records (asset_id, key, text preview)\n\
  get   <in.dlg> (--id <DottedID> | --hash 0x..) [--scene 0x..]   read one string (base + DNEC)\n\
  set   <in.dlg> <out.dlg> (--id <DottedID> | --hash 0x..) [--scene 0x..] --text <STRING>\n\
                                       overwrite an existing record's string (base or DNEC;\n\
                                       --scene picks a DNEC group when the id is ambiguous)\n\
  add   <in.dlg> <out.dlg> --id <DottedID> --text <STRING>\n\
                                       append a NEW UI-text record (asset_id=pandemic_hash(id))\n\
  add-dnec <in.dlg> <out.dlg> --scene 0x.. --text <STRING> (--key vo_.. | --hash 0x..)\n\
                                       append a NEW VO subtitle into an existing DNEC scene group\n\
                                       (asset_id = --hash, else pandemic_hash(--key))\n\
  roundtrip <in.dlg>                   parse -> re-emit; assert byte-identical + exact consume\n"
    );
}

fn read(args: &[String]) -> Result<Vec<u8>, String> {
    fs::read(args.first().ok_or("need <in.dlg>")?).map_err(|e| e.to_string())
}

fn run() -> Result<(), String> {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() {
        usage();
        return Ok(());
    }
    let cmd = args[0].as_str();
    let rest = &args[1..];
    match cmd {
        "hash" => {
            let s = rest.first().ok_or("need <string>")?;
            println!("0x{:08x}  pandemic_hash({s:?})", pandemic_hash(s));
        }
        "info" => {
            let gt = GameText::parse(&read(rest)?)?;
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
        "list" => {
            let gt = GameText::parse(&read(rest)?)?;
            let only_ui = has_flag(rest, "--ui");
            let only_vo = has_flag(rest, "--vo");
            let only_dnec = has_flag(rest, "--dnec");
            let limit: usize = get_flag(rest, "--limit").and_then(|s| s.parse().ok()).unwrap_or(50);
            let mut n = 0;
            if only_dnec {
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
            } else {
                for r in &gt.records {
                    if (only_ui && !r.is_ui()) || (only_vo && r.is_ui()) {
                        continue;
                    }
                    let kind = if r.is_ui() { "UI" } else { "VO" };
                    println!("0x{:08x} [{kind}] {:<40} {:?}", r.asset_id, r.key_str(), preview(r));
                    n += 1;
                    if n >= limit {
                        println!("... (--limit {limit} reached)");
                        break;
                    }
                }
            }
        }
        "get" => {
            let gt = GameText::parse(&read(rest)?)?;
            let (id, scene) = resolve_id(rest)?;
            let r = gt.find_any(id, scene).ok_or_else(|| not_found(id, scene))?;
            println!("0x{:08x}  {:?}", r.asset_id, r.text_string());
        }
        "set" => {
            let inp = rest.first().ok_or("need <in>")?;
            let outp = rest.get(1).ok_or("need <out>")?;
            let text = get_flag(rest, "--text").ok_or("need --text <STRING>")?;
            let (id, scene) = resolve_id(rest)?;
            let mut gt = GameText::parse(&fs::read(inp).map_err(|e| e.to_string())?)?;
            {
                let r = gt.find_any_mut(id, scene).ok_or_else(|| not_found(id, scene))?;
                r.set_text(&text);
            }
            let out = gt.write();
            GameText::parse(&out)?; // prove validity
            fs::write(outp, &out).map_err(|e| e.to_string())?;
            println!("set 0x{id:08x} -> {} bytes; re-parsed OK", out.len());
        }
        "add" => {
            let inp = rest.first().ok_or("need <in>")?;
            let outp = rest.get(1).ok_or("need <out>")?;
            let dotted = get_flag(rest, "--id").ok_or("need --id <DottedID>")?;
            let text = get_flag(rest, "--text").ok_or("need --text <STRING>")?;
            let mut gt = GameText::parse(&fs::read(inp).map_err(|e| e.to_string())?)?;
            let asset_id = gt.add_ui(&dotted, &text)?;
            let out = gt.write();
            let gt2 = GameText::parse(&out)?;
            fs::write(outp, &out).map_err(|e| e.to_string())?;
            println!("added UI id {dotted:?} = 0x{asset_id:08x}; now {} base records, {} bytes", gt2.records.len(), out.len());
        }
        "add-dnec" => {
            let inp = rest.first().ok_or("need <in>")?;
            let outp = rest.get(1).ok_or("need <out>")?;
            let scene = parse_hex_u32(&get_flag(rest, "--scene").ok_or("need --scene 0x<sceneHash>")?)?;
            let text = get_flag(rest, "--text").ok_or("need --text <STRING>")?;
            let key = get_flag(rest, "--key").unwrap_or_default();
            let asset_id = match get_flag(rest, "--hash") {
                Some(h) => parse_hex_u32(&h)?,
                None if !key.is_empty() => pandemic_hash(&key),
                None => return Err("need --hash 0x.. or --key vo_.. (to derive the asset_id)".into()),
            };
            let mut gt = GameText::parse(&fs::read(inp).map_err(|e| e.to_string())?)?;
            gt.add_dnec(scene, asset_id, &key, &text)?;
            let out = gt.write();
            let gt2 = GameText::parse(&out)?;
            fs::write(outp, &out).map_err(|e| e.to_string())?;
            println!(
                "added VO subtitle 0x{asset_id:08x} ({key:?}) into scene 0x{scene:08x}; now {} DNEC records, {} bytes",
                gt2.dnec_record_count(),
                out.len()
            );
        }
        "roundtrip" => {
            let inp = rest.first().ok_or("need <in.dlg>")?;
            let b = fs::read(inp).map_err(|e| e.to_string())?;
            let gt = GameText::parse(&b)?;
            let out = gt.write();
            let byte_ident = out == b;
            println!(
                "roundtrip {inp}: base_records={} dnec_groups={} dnec_records={}  exact_consume={}  BYTE_IDENTICAL={}",
                gt.records.len(),
                gt.dnec_groups().len(),
                gt.dnec_record_count(),
                out.len() == b.len(),
                byte_ident
            );
            if !byte_ident {
                return Err("round-trip NOT byte-identical".into());
            }
        }
        _ => usage(),
    }
    Ok(())
}

fn not_found(id: u32, scene: Option<u32>) -> String {
    match scene {
        Some(sc) => format!("no record with asset_id 0x{id:08x} in DNEC scene 0x{sc:08x}"),
        None => format!("no record with asset_id 0x{id:08x} (base or DNEC)"),
    }
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        exit(1);
    }
}
