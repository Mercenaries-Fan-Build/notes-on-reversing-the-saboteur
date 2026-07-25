//! `sab_probe audio` — read-only: resolve GameText VO/subtitle keys to their Wwise `.wem` in a
//! `1KCP` sound pack, and report coverage. Resolution lives in `sab_formats::wwise`.

use sab_formats::gametext::GameText;
use sab_formats::wwise::SoundPack;

pub fn run(args: &[String]) {
    let (Some(pck), Some(dlg)) = (args.first(), args.get(1)) else { return usage() };
    let pack = match SoundPack::open(pck) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{pck}: {e}");
            std::process::exit(1);
        }
    };
    let (streams, objs) = pack.stats();
    let gt = match std::fs::read(dlg).map_err(|e| e.to_string()).and_then(|b| GameText::parse(&b)) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("{dlg}: {e}");
            std::process::exit(1);
        }
    };
    println!("pack: {streams} streams, {objs} HIRC objects");

    // Every keyed VO/subtitle record (base VO + DNEC subs).
    let mut keys: Vec<String> = gt.records.iter().filter(|r| !r.is_ui()).map(|r| r.key_str()).collect();
    for s in gt.dnec_groups() {
        keys.extend(s.records.iter().filter(|r| !r.key_str().is_empty()).map(|r| r.key_str()));
    }

    let single = flag(args, "--key");
    if let Some(k) = single {
        match pack.wem_for_key(k) {
            Some(w) => println!("{k} -> wem {w:08x}"),
            None => println!("{k} -> unresolved"),
        }
        return;
    }

    let mut resolved = 0usize;
    let mut examples = Vec::new();
    for k in &keys {
        match pack.wem_for_key(k) {
            Some(w) => {
                resolved += 1;
                if examples.len() < 8 {
                    examples.push(format!("  {:<46} -> wem {w:08x}", trunc(k, 46)));
                }
            }
            None => {}
        }
    }
    let n = keys.len().max(1);
    println!("resolved {resolved}/{} VO+sub keys ({:.1}%)", keys.len(), 100.0 * resolved as f64 / n as f64);
    for e in examples {
        println!("{e}");
    }
}

fn usage() {
    eprintln!("sab_probe audio <Sound/<Lang>.pck> <GameText.dlg> [--key <vo_key>]");
    eprintln!("  resolve VO/subtitle keys to their Wwise .wem source id + report coverage");
}

fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).map(|s| s.as_str())
}

fn trunc(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}
