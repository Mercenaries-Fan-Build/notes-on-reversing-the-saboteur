//! sab_gametext — command-line **authoring** for `GameText.dlg` (writes edited files). Read-only
//! inspection (info/list/get/hash/roundtrip) lives in `sab_probe gametext`. All format logic is in
//! `sab_formats::gametext`; this is a thin front-end so edits can be scripted/batched/tested without
//! the Workshop GUI.

use sab_formats::gametext::GameText;
use sab_formats::pandemic_hash;
use std::fs;
use std::process::exit;

fn get(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1).cloned())
}
fn hex_u32(s: &str) -> Result<u32, String> {
    u32::from_str_radix(s.trim_start_matches("0x"), 16).map_err(|_| format!("bad hex {s:?}"))
}

fn usage() {
    eprintln!(
        "sab_gametext — GameText.dlg authoring (writes files). Inspect with `sab_probe gametext`.\n\
\n\
  set   <in.dlg> <out.dlg> (--id <DottedID> | --hash 0x..) [--scene 0x..] --text <STRING>\n\
                                       overwrite an existing record's string (base or DNEC subtitle)\n\
  add   <in.dlg> <out.dlg> --id <DottedID> --text <STRING>\n\
                                       append a NEW UI-text record (asset_id=pandemic_hash(id))\n\
  add-dnec <in.dlg> <out.dlg> --scene 0x.. --text <STRING> (--key vo_.. | --hash 0x..)\n\
                                       append a NEW subtitle into an existing DNEC scene group"
    );
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(|s| s.as_str()).unwrap_or("");
    let rest = &args[args.len().min(1)..];
    let inp = || rest.first().ok_or_else(|| "need <in.dlg>".to_string());
    let outp = || rest.get(1).ok_or_else(|| "need <out.dlg>".to_string());
    match cmd {
        "set" => {
            let text = get(rest, "--text").ok_or("need --text <STRING>")?;
            let id = match (get(rest, "--id"), get(rest, "--hash")) {
                (Some(d), _) => pandemic_hash(&d),
                (None, Some(h)) => hex_u32(&h)?,
                _ => return Err("need --id <DottedID> or --hash 0x..".into()),
            };
            let scene = match get(rest, "--scene") {
                Some(s) => Some(hex_u32(&s)?),
                None => None,
            };
            let mut gt = GameText::parse(&fs::read(inp()?).map_err(|e| e.to_string())?)?;
            gt.find_any_mut(id, scene).ok_or_else(|| format!("no record 0x{id:08x}"))?.set_text(&text);
            let out = gt.write();
            GameText::parse(&out)?;
            fs::write(outp()?, &out).map_err(|e| e.to_string())?;
            println!("set 0x{id:08x} -> {} bytes", out.len());
        }
        "add" => {
            let dotted = get(rest, "--id").ok_or("need --id <DottedID>")?;
            let text = get(rest, "--text").ok_or("need --text <STRING>")?;
            let mut gt = GameText::parse(&fs::read(inp()?).map_err(|e| e.to_string())?)?;
            let id = gt.add_ui(&dotted, &text)?;
            let out = gt.write();
            GameText::parse(&out)?;
            fs::write(outp()?, &out).map_err(|e| e.to_string())?;
            println!("added UI {dotted:?} = 0x{id:08x}; {} bytes", out.len());
        }
        "add-dnec" => {
            let scene = hex_u32(&get(rest, "--scene").ok_or("need --scene 0x..")?)?;
            let text = get(rest, "--text").ok_or("need --text <STRING>")?;
            let key = get(rest, "--key").unwrap_or_default();
            let asset_id = match get(rest, "--hash") {
                Some(h) => hex_u32(&h)?,
                None if !key.is_empty() => pandemic_hash(&key),
                None => return Err("need --hash 0x.. or --key vo_..".into()),
            };
            let mut gt = GameText::parse(&fs::read(inp()?).map_err(|e| e.to_string())?)?;
            gt.add_dnec(scene, asset_id, &key, &text)?;
            let out = gt.write();
            GameText::parse(&out)?;
            fs::write(outp()?, &out).map_err(|e| e.to_string())?;
            println!("added subtitle 0x{asset_id:08x} into scene 0x{scene:08x}; {} bytes", out.len());
        }
        _ => usage(),
    }
    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        exit(1);
    }
}
