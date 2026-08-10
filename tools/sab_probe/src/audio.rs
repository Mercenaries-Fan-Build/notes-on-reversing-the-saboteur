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

    // `--csv <path>`: emit a sidecar keyed by wem id, to join against a bulk extraction's
    // manifest.csv (`saboteur_audio`) and give the ID-named .wem files their meaning back.
    // Keys are deduped by wem id: several subtitle keys can resolve to the same stream (a line
    // reused across missions), so the extra keys are kept in `also_keys` rather than dropped.
    let csv_out = flag(args, "--csv");

    let mut resolved = 0usize;
    let mut examples = Vec::new();
    let mut by_wem: std::collections::BTreeMap<u32, Vec<String>> = std::collections::BTreeMap::new();
    for k in &keys {
        match pack.wem_for_key(k) {
            Some(w) => {
                resolved += 1;
                by_wem.entry(w).or_default().push(k.clone());
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

    if let Some(path) = csv_out {
        let mut out = String::from("wem_id,key,speaker,event,also_keys\n");
        for (w, ks) in &by_wem {
            let key = &ks[0];
            let spk = sab_formats::gametext::speaker(key).unwrap_or_default();
            let also = ks[1..].join(" ");
            out.push_str(&format!(
                "{:08x},{},{},{},{}\n",
                w, csv_esc(key), csv_esc(&spk), csv_esc(&sab_formats::wwise::event_name(key)), csv_esc(&also)
            ));
        }
        match std::fs::write(path, out) {
            Ok(()) => println!("sidecar -> {path} ({} distinct wems)", by_wem.len()),
            Err(e) => eprintln!("write {path}: {e}"),
        }
    }
}

fn usage() {
    eprintln!("sab_probe audio <Sound/<Lang>.pck> <GameText.dlg> [--key <vo_key>] [--csv <out.csv>]");
    eprintln!("  resolve VO/subtitle keys to their Wwise .wem source id + report coverage");
    eprintln!("  --csv  write a wem_id -> key/speaker sidecar to join against a manifest.csv");
}

/// Minimal RFC4180 quoting — keys and speakers are tame, but never let one corrupt the row.
fn csv_esc(s: &str) -> String {
    if s.contains([',', '"', '\n']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).map(|s| s.as_str())
}

fn trunc(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}
