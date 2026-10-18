//! `xma2-oracle <Bin360> <ffmpeg> <work dir> <c|default> [every-nth]` — decode every 2008 package
//! stream and whole bank media with `sab_formats::xma2` and with FFmpeg (`-cpuflags 0` for `c`, the
//! build's own SIMD for `default`; output `f32le`), and compare them sample by sample: equal bits,
//! equal values, lengths, the first difference and the largest. docs/formats/xma2_decoder.md.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use sab_formats::proto2008::sound_pack;

struct Item {
    name: String,
    bytes: Vec<u8>,
}

#[derive(Default)]
struct Tally {
    bit_identical: usize,
    value_identical: usize,
    length_differs: usize,
    ours_failed: usize,
    ffmpeg_failed: usize,
    ffmpeg_stderr: usize,
    /// FFmpeg's warning lines with the context address removed, and how many media gave each.
    warnings: BTreeMap<String, usize>,
    max_abs: f32,
    samples: usize,
    examples: Vec<String>,
}

fn ffmpeg_decode(ffmpeg: &Path, input: &Path, output: &Path, c_only: bool) -> Result<(Vec<f32>, String), String> {
    let mut cmd = Command::new(ffmpeg);
    cmd.args(["-hide_banner", "-nostdin", "-loglevel", "warning"]);
    if c_only {
        cmd.args(["-cpuflags", "0"]);
    }
    cmd.arg("-i").arg(input).args(["-f", "f32le", "-y"]).arg(output);
    let out = cmd.output().map_err(|e| format!("{}: {e}", ffmpeg.display()))?;
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    if !out.status.success() {
        return Err(format!("ffmpeg exit {:?}: {stderr}", out.status.code()));
    }
    let raw = std::fs::read(output).map_err(|e| format!("{}: {e}", output.display()))?;
    Ok((raw.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect(), stderr))
}

fn check(item: &Item, ffmpeg: &Path, work: &Path, c_only: bool, t: &mut Tally) {
    let input = work.join(format!("{}.xma", item.name));
    let output = work.join(format!("{}.f32", item.name));
    std::fs::write(&input, &item.bytes).unwrap_or_else(|e| panic!("{}: {e}", input.display()));
    let theirs = match ffmpeg_decode(ffmpeg, &input, &output, c_only) {
        Ok((s, err)) => {
            if !err.is_empty() {
                t.ffmpeg_stderr += 1;
                for line in err.lines() {
                    let general: String = line.split(' ').filter(|w| !w.starts_with("0x")).collect::<Vec<_>>().join(" ");
                    *t.warnings.entry(general).or_default() += 1;
                }
            }
            s
        }
        Err(e) => {
            t.ffmpeg_failed += 1;
            t.examples.push(format!("{}: {e}", item.name));
            return;
        }
    };
    let _ = std::fs::remove_file(&input);
    let _ = std::fs::remove_file(&output);
    let ours = match sound_pack::decode_media(&item.bytes) {
        Ok(p) => p.samples,
        Err(e) => {
            t.ours_failed += 1;
            if t.examples.len() < 12 {
                t.examples.push(format!("{}: ours failed: {e}", item.name));
            }
            return;
        }
    };
    t.samples += ours.len();
    if ours.len() != theirs.len() {
        t.length_differs += 1;
        if t.examples.len() < 12 {
            t.examples.push(format!("{}: {} samples, ffmpeg {}", item.name, ours.len(), theirs.len()));
        }
    }
    let n = ours.len().min(theirs.len());
    let bits = (0..n).all(|i| ours[i].to_bits() == theirs[i].to_bits());
    let values = (0..n).all(|i| ours[i] == theirs[i]);
    if ours.len() == theirs.len() && bits {
        t.bit_identical += 1;
    }
    if ours.len() == theirs.len() && values {
        t.value_identical += 1;
    }
    if !values {
        let first = (0..n).find(|&i| ours[i] != theirs[i]).expect("a difference");
        let max = (0..n).map(|i| (ours[i] - theirs[i]).abs()).fold(0f32, f32::max);
        t.max_abs = t.max_abs.max(max);
        if t.examples.len() < 12 {
            t.examples.push(format!("{}: first difference at sample {first} ({} vs {}), largest {max:e}", item.name, ours[first], theirs[first]));
        }
    }
}

pub fn run(args: &[String]) {
    let (bin360, ffmpeg, work, mode, nth) = match args {
        [b, f, w, m] => (b, f, w, m, 1usize),
        [b, f, w, m, n] => (b, f, w, m, n.parse().expect("every-nth is a number")),
        _ => {
            eprintln!("usage: sab_probe xma2-oracle <Bin360> <ffmpeg> <work dir> <c|default> [every-nth]");
            std::process::exit(2);
        }
    };
    let c_only = match mode.as_str() {
        "c" => true,
        "default" => false,
        m => panic!("mode {m:?}: expected c or default"),
    };
    let ffmpeg = PathBuf::from(ffmpeg);
    let work = PathBuf::from(work);
    std::fs::create_dir_all(&work).unwrap_or_else(|e| panic!("{}: {e}", work.display()));
    let path = Path::new(bin360).join("sound/Saboteur.pck");
    let b = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let p = sound_pack::parse(&b, b.len() as u64).unwrap_or_else(|e| panic!("{}: {e}", path.display()));

    let mut items = Vec::new();
    for r in &p.streams {
        items.push(Item { name: format!("s_{:08X}", r.id), bytes: r.data(&b).expect("bounds checked").to_vec() });
    }
    for r in &p.banks {
        let d = r.data(&b).expect("bounds checked");
        let cs = sound_pack::chunks(d).unwrap_or_else(|e| panic!("bank {:08X}: {e}", r.id));
        let body = |t: &[u8; 4]| cs.iter().find(|c| &c.tag == t).map(|c| &d[c.start..c.start + c.len]);
        let (Some(idx), Some(data)) = (body(b"DIDX"), body(b"DATA")) else { continue };
        for e in idx.chunks(12) {
            let w = |i: usize| u32::from_be_bytes(e[4 * i..4 * i + 4].try_into().expect("4 bytes"));
            let m = &data[w(1) as usize..(w(1) + w(2)) as usize];
            if sound_pack::riff_chunks(m).is_ok() {
                items.push(Item { name: format!("b_{:08X}_{:08X}", r.id, w(0)), bytes: m.to_vec() });
            }
        }
    }
    let items: Vec<Item> = items.into_iter().enumerate().filter(|(i, _)| i % nth == 0).map(|(_, it)| it).collect();
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    let chunk = items.len().div_ceil(threads).max(1);
    let tallies: Vec<Tally> = std::thread::scope(|s| {
        let handles: Vec<_> = items
            .chunks(chunk)
            .map(|part| {
                let (ffmpeg, work) = (&ffmpeg, &work);
                s.spawn(move || {
                    let mut t = Tally::default();
                    for it in part {
                        check(it, ffmpeg, work, c_only, &mut t);
                    }
                    t
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().expect("worker")).collect()
    });
    let mut all = Tally::default();
    let mut examples = BTreeMap::new();
    for t in tallies {
        all.bit_identical += t.bit_identical;
        all.value_identical += t.value_identical;
        all.length_differs += t.length_differs;
        all.ours_failed += t.ours_failed;
        all.ffmpeg_failed += t.ffmpeg_failed;
        all.ffmpeg_stderr += t.ffmpeg_stderr;
        for (w, n) in t.warnings {
            *all.warnings.entry(w).or_default() += n;
        }
        all.max_abs = all.max_abs.max(t.max_abs);
        all.samples += t.samples;
        for e in t.examples {
            examples.insert(e.clone(), ());
        }
    }
    println!("{} media ({mode}): bit-identical {}, value-identical {}, length differs {}, ours failed {}, ffmpeg failed {}, ffmpeg warned {}", items.len(), all.bit_identical, all.value_identical, all.length_differs, all.ours_failed, all.ffmpeg_failed, all.ffmpeg_stderr);
    println!("samples compared {}, largest difference {:e}", all.samples, all.max_abs);
    println!("ffmpeg warnings: {:?}", all.warnings);
    for e in examples.keys().take(20) {
        println!("  {e}");
    }
}
