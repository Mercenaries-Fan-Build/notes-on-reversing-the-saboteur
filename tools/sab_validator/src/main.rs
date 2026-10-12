//! sab_validator — parse The Saboteur's assets the way the engine's mount path does and report
//! anything the loader would choke on. Guided by the disassembled loaders; every rule cites the
//! decomp function it is derived from.
//!
//! Usage:
//!   sab_validator <asset>            audit a .megapack / .kilopack or an ALBS sub-pack
//!                                    (the DTEX and MSHA records inside are validated as it descends;
//!                                     a bare .dtex file is not a supported top-level input)
//!   sab_validator <asset> --limit N  only descend the first N megapack entries (fast smoke test)
//!   sab_validator <asset> --json out.json      also write the machine report
//!   sab_validator <asset> --max-issues N       cap printed issues (default 60; 0 = all)
//!   sab_validator <asset> --quiet    summary only

mod consume;
mod report;

use report::{Report, Severity};
use std::process::exit;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut path: Option<String> = None;
    let mut json_out: Option<String> = None;
    let mut limit: Option<usize> = None;
    let mut max_issues: usize = 60;
    let mut quiet = false;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--json" => {
                i += 1;
                json_out = args.get(i).cloned();
            }
            "--limit" => {
                i += 1;
                limit = args.get(i).and_then(|s| s.parse().ok());
            }
            "--max-issues" => {
                i += 1;
                max_issues = args.get(i).and_then(|s| s.parse().ok()).unwrap_or(60);
            }
            "--quiet" => quiet = true,
            "-h" | "--help" => usage(),
            s if !s.starts_with('-') => path = Some(s.to_string()),
            other => {
                eprintln!("unknown flag: {other}");
                usage();
            }
        }
        i += 1;
    }

    let path = path.unwrap_or_else(|| {
        eprintln!("error: no asset path given\n");
        usage();
    });

    let buf = std::fs::read(&path).unwrap_or_else(|e| {
        eprintln!("error: cannot read {path}: {e}");
        exit(2);
    });
    let name = std::path::Path::new(&path)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.clone());

    eprintln!("sab_validator: {name} ({} bytes)", buf.len());

    let mut report = Report::default();
    // Dispatch by magic (on-disk byte order).
    match buf.get(0..4) {
        Some(m) if m == sab_formats::megapack::MAGIC => {
            consume::consume_megapack(&buf, &name, &mut report, limit);
        }
        Some(m) if m == sab_formats::sbla::MAGIC => {
            consume::consume_sbla(&buf, &name, &mut report);
        }
        _ => {
            eprintln!(
                "error: unrecognized top-level magic {:02X?} (expected 00PM megapack or ALBS sub-pack)",
                buf.get(0..4).unwrap_or(&[])
            );
            exit(2);
        }
    }

    // --- render ---
    if !quiet {
        let shown = if max_issues == 0 { report.issues.len() } else { max_issues };
        for iss in report.issues.iter().take(shown) {
            println!("{iss}");
        }
        if report.issues.len() > shown {
            println!("… {} more issues (raise --max-issues)", report.issues.len() - shown);
        }
        println!();
    }

    // --- summary ---
    let scanned: u64 = report.scanned.values().sum();
    eprint!("scanned: ");
    for (fmt, n) in &report.scanned {
        eprint!("{fmt}={n} ");
    }
    eprintln!("(total {scanned})");
    if !report.unchecked.is_empty() {
        eprint!("unchecked: ");
        for (check, n) in &report.unchecked {
            eprint!("{check}={n} ");
        }
        eprintln!();
    }
    eprintln!(
        "verdict: {} fatal, {} warn, {} info",
        report.count(Severity::Fatal),
        report.count(Severity::Warning),
        report.count(Severity::Advisory)
    );

    if let Some(out) = json_out {
        if let Err(e) = std::fs::write(&out, report.to_json()) {
            eprintln!("warning: could not write {out}: {e}");
        } else {
            eprintln!("wrote {out}");
        }
    }

    let code = report.exit_code();
    eprintln!("{}", if code == 0 { "RESULT: OK (no fatal findings)" } else { "RESULT: FAIL" });
    exit(code);
}

fn usage() -> ! {
    eprintln!(
        "sab_validator <asset> [--limit N] [--json out.json] [--max-issues N] [--quiet]\n\
         \n\
         Audits a Saboteur .megapack/.kilopack or ALBS sub-pack the way the engine mount path\n\
         (FUN_00e428c0 -> FUN_00658870) would consume it, validating the DTEX and MSHA records\n\
         found inside. <asset> must start with the 00PM or ALBS magic; a carved standalone .dtex\n\
         is not accepted. Exit 0 = no fatal findings."
    );
    exit(2);
}
