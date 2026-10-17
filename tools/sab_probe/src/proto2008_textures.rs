//! `proto2008-textures <bin360>` — read every `textures` slot record of every loose `.Pack` under the
//! 2008 prototype's `Bin360` with `sab_formats::proto2008::texture`, and tally the facts
//! `docs/2008_texture_format.md` cites: formats, flags, the sRGB rule's split, mip counts, the slot
//! record's words against the header, trailing bytes, inflated length against `unpacked`, the mip
//! layout's extent against `unpacked`, each decoded mip against the box-filtered decoded base (scored
//! against a wrongly placed control read), and — for every texture with a loose `.dds` twin of the
//! same name — how each decoded level compares with the twin's.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use sab_formats::proto2008::sbl::{self, SlotKind};
use sab_formats::proto2008::{slot_dir, texture};
use sab_formats::{dds, pixels};

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
    counts: BTreeMap<String, usize>,
    examples: BTreeMap<String, String>,
}

impl Tally {
    fn note(&mut self, key: String, at: &str) {
        self.examples.entry(key.clone()).or_insert_with(|| at.to_string());
        *self.counts.entry(key).or_default() += 1;
    }
}

fn fmt_name(f: u32) -> String {
    match f {
        texture::DXT1 => "DXT1".into(),
        texture::DXT5 => "DXT5".into(),
        texture::DXN => "DXN".into(),
        texture::R5G6B5 => "R5G6B5".into(),
        texture::X8R8G8B8 => "X8R8G8B8".into(),
        o => format!("0x{o:08X}"),
    }
}

/// Level `l` read without untiling: element `(x, y)` at `offset + (y * pitch + x) * bpe`, swapped
/// as [`texture::level_bytes`] swaps; the control the tiled read is scored against.
fn linear_level(mem: &[u8], sf: pixels::Format, l: &texture::Level) -> Option<Vec<u8>> {
    let block = if sf.is_block() { 4 } else { 1 };
    let (ew, eh) = (l.width.div_ceil(block) as usize, l.height.div_ceil(block) as usize);
    let bpe = sf.unit_bytes();
    let mut lin = vec![0u8; ew * eh * bpe];
    for y in 0..eh {
        let src = l.offset + y * l.pitch as usize * bpe;
        lin[y * ew * bpe..(y + 1) * ew * bpe].copy_from_slice(mem.get(src..src + ew * bpe)?);
    }
    match sf {
        pixels::Format::X8r8g8b8 => lin.chunks_exact_mut(4).for_each(|c| c.reverse()),
        _ => lin.chunks_exact_mut(2).for_each(|c| c.swap(0, 1)),
    }
    pixels::decode(sf, l.width as usize, l.height as usize, &lin).ok()
}

/// Box-filter `base` (`w`×`h` RGBA8) down to `lw`×`lh`.
fn downsample(base: &[u8], w: usize, h: usize, lw: usize, lh: usize) -> Vec<u8> {
    let mut out = vec![0u8; lw * lh * 4];
    for y in 0..lh {
        for x in 0..lw {
            let (x0, x1) = (x * w / lw, ((x + 1) * w / lw).max(x * w / lw + 1));
            let (y0, y1) = (y * h / lh, ((y + 1) * h / lh).max(y * h / lh + 1));
            for c in 0..4 {
                let mut s = 0u64;
                for yy in y0..y1 {
                    for xx in x0..x1 {
                        s += base[(yy * w + xx) * 4 + c] as u64;
                    }
                }
                out[(y * lw + x) * 4 + c] = (s / ((x1 - x0) * (y1 - y0)) as u64) as u8;
            }
        }
    }
    out
}

/// Mean absolute RGB difference of two equal-sized RGBA8 images.
fn mad(a: &[u8], b: &[u8]) -> f64 {
    let mut s = 0u64;
    for (p, q) in a.chunks_exact(4).zip(b.chunks_exact(4)) {
        for c in 0..3 {
            s += (p[c] as i32 - q[c] as i32).unsigned_abs() as u64;
        }
    }
    s as f64 / (a.len() / 4 * 3).max(1) as f64
}

pub fn run(args: &[String]) {
    let [bin360] = args else {
        eprintln!("usage: sab_probe proto2008-textures <Bin360 dir>");
        std::process::exit(2);
    };
    let root = PathBuf::from(bin360);
    let mut all = Vec::new();
    files(&root, &mut all);
    all.sort();
    let mut t = Tally::default();
    let mut twins: HashMap<String, PathBuf> = HashMap::new();
    for p in &all {
        if p.extension().and_then(|x| x.to_str()).map(|x| x.eq_ignore_ascii_case("dds")) == Some(true) {
            let rel = p.strip_prefix(&root).expect("under root").to_string_lossy().into_owned();
            let b = std::fs::read(p).unwrap_or_else(|e| panic!("{rel}: {e}"));
            match dds::parse(&b) {
                Ok(d) => {
                    let cc = if d.four_cc == 0 { "uncompressed".to_string() } else { String::from_utf8_lossy(&d.four_cc.to_le_bytes()).into_owned() };
                    t.note(format!("loose dds {cc} ({:?}), {}", d.format, if d.faces == 6 { "cube" } else { "2D" }), &format!("{rel} {}x{} mips {}", d.width, d.height, d.mips));
                    let fit = if b.len() - 128 == d.surfaces_len() { "== header surfaces" } else if b.len() - 128 > d.surfaces_len() { "> header surfaces" } else { "< header surfaces" };
                    t.note(format!("loose dds bytes after header {fit}"), &format!("{rel} {} vs {}", b.len() - 128, d.surfaces_len()));
                    let mut levels_ok = true;
                    for face in 0..d.faces {
                        for l in 0..d.mips {
                            let (w, h) = d.level_size(l);
                            if let Err(e) = d.level(face, l).and_then(|s| pixels::decode(d.format, w as usize, h as usize, s)) {
                                t.note("FAIL loose dds level decode".into(), &format!("{rel} face {face} level {l}: {e}"));
                                levels_ok = false;
                            }
                        }
                    }
                    if levels_ok {
                        t.note("loose dds every face and level decodes".into(), &rel);
                    }
                }
                Err(e) => t.note(format!("FAIL loose dds parse: {e}"), &rel),
            }
            let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_ascii_lowercase();
            twins.entry(stem).or_insert_with(|| p.clone());
        }
    }
    for p in &all {
        let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if !name.to_ascii_lowercase().ends_with(".pack") {
            continue;
        }
        let rel = p.strip_prefix(&root).expect("under root").to_string_lossy().replace('\\', "/");
        let buf = std::fs::read(p).unwrap_or_else(|e| panic!("{rel}: {e}"));
        if !buf.starts_with(b"SBL") {
            continue;
        }
        let sp = sbl::parse(&buf).unwrap_or_else(|e| panic!("{rel}: {e}"));
        let Some(slot) = sp.slot(SlotKind::Textures) else { continue };
        let recs = slot_dir::parse(slot).unwrap_or_else(|e| panic!("{rel}: {e}"));
        for (ri, r) in recs.iter().enumerate().filter(|(_, r)| r.stored != 0) {
            let at = format!("{rel}#{ri}");
            let data = &slot[r.offset as usize..(r.offset + r.stored) as usize];
            let tx = match texture::parse(data) {
                Ok(tx) => tx,
                Err(e) => {
                    t.note(format!("FAIL parse: {}", e.split(':').next_back().unwrap_or(&e).trim()), &format!("{at}: {e}"));
                    continue;
                }
            };
            let f = fmt_name(tx.format);
            t.note(format!("format {f}"), &format!("{at} {}", tx.name));
            t.note(format!("format {f}: flags {:#X} -> srgb {}", tx.flags, tx.srgb()), &format!("{at} {}", tx.name));
            t.note(format!("record +0x14 = {} ({f})", r.f14), &format!("{at} {}", tx.name));
            t.note(format!("record name hash == pandemic_hash(name): {}", sab_formats::pandemic_hash(&tx.name) == r.hash), &format!("{at} {} {:08X}", tx.name, r.hash));
            let rec_unpacked = if r.unpacked == tx.unpacked {
                "== header unpacked"
            } else if r.unpacked == r.stored {
                "== record stored"
            } else {
                "!= header unpacked, != record stored"
            };
            t.note(format!("record unpacked {rec_unpacked} ({})", String::from_utf8_lossy(&sp.magic)),&format!("{at} {} rec {} stored {} hdr {} f10 {} f14 {}", tx.name, r.unpacked, r.stored, tx.unpacked, r.f10, r.f14));
            t.note(format!("streams {}", tx.streams.len().min(3)), &at);
            t.note(format!("trailing {} bytes{}", tx.trailing.len(), if tx.trailing.len() >= 4 { format!(" head {:?}", String::from_utf8_lossy(&tx.trailing[..4])) } else { String::new() }), &format!("{at} {}", tx.name));
            let mem = match tx.memory() {
                Ok(m) => m,
                Err(e) => {
                    t.note(format!("FAIL memory: {}", e.split(':').next_back().unwrap_or(&e).trim()), &format!("{at}: {e}"));
                    continue;
                }
            };
            let Ok(sf) = texture::surface_format(tx.format) else {
                t.note(format!("unknown format {f}"), &at);
                continue;
            };
            let (w, h) = (tx.width as usize, tx.height as usize);
            let (levels, end) = match texture::levels(&tx) {
                Ok(l) => l,
                Err(e) => {
                    t.note(format!("FAIL levels ({f})"), &format!("{at}: {e}"));
                    continue;
                }
            };
            let shape = if texture::packed_level(tx.width as u32, tx.height as u32) == 0 { "packed base" } else if tx.mips as u32 > texture::packed_level(tx.width as u32, tx.height as u32) { "packed tail" } else { "no packed level" };
            t.note(
                format!("{f} mips {} ({shape}): layout extent {} unpacked", if tx.mips > 1 { ">1" } else { "1" }, if end == mem.len() { "==" } else { "!=" }),
                &format!("{at} {} {w}x{h} mips {} unpacked {} layout {end}", tx.name, tx.mips, mem.len()),
            );
            if let Some(e) = levels.iter().find_map(|l| texture::level_bytes(&tx, &mem, l).err()) {
                t.note(format!("FAIL level read ({f})"), &format!("{at}: {e}"));
                continue;
            }
            if levels.len() > 1 {
                let decoded: Vec<Vec<u8>> = levels.iter().map(|l| texture::decode_level(&tx, &mem, l).expect("level read checked above")).collect();
                let base_linear = linear_level(&mem, sf, &levels[0]).expect("base storage inside the image");
                let packed = texture::packed_level(tx.width as u32, tx.height as u32);
                let mut judge = |kind: &str, score: f64, control: f64, l: &texture::Level| {
                    let verdict = if score + 0.5 < control {
                        "layout closer"
                    } else if control + 0.5 < score {
                        if l.width.min(l.height) < 4 { "control closer, level under 4 texels" } else { "CONTROL CLOSER" }
                    } else {
                        "tie within 0.5"
                    };
                    let bucket = if score < 4.0 { "< 4" } else if score < 12.0 { "< 12" } else { ">= 12" };
                    t.note(format!("{kind} ({f}): {verdict}"), &format!("{at} {} level {} {}x{} layout {score:.2} control {control:.2}", tx.name, l.level, l.width, l.height));
                    t.note(format!("{kind} ({f}): layout |diff| {bucket}"), &format!("{at} {} level {} {}x{} layout {score:.2}", tx.name, l.level, l.width, l.height));
                };
                let l1 = &levels[1];
                let ds = |img: &[u8], l: &texture::Level| downsample(img, w, h, l.width as usize, l.height as usize);
                judge("base vs mip 1, control = base read linear", mad(&decoded[1], &ds(&decoded[0], l1)), mad(&decoded[1], &ds(&base_linear, l1)), l1);
                for (l, img) in levels.iter().zip(&decoded).skip(1) {
                    let reference = ds(&decoded[0], l);
                    if (l.level as u32) < packed {
                        let control = linear_level(&mem, sf, l).expect("mip storage inside the image");
                        judge("mip vs base, control = mip read linear", mad(img, &reference), mad(&control, &reference), l);
                    } else {
                        let at_origin = texture::Level { x: 0, y: 0, ..*l };
                        let control = texture::decode_level(&tx, &mem, &at_origin).expect("tail origin inside the image");
                        judge("packed mip vs base, control = tail origin", mad(img, &reference), mad(&control, &reference), l);
                    }
                }
            }
            let Some(twin) = twins.get(&tx.name.to_ascii_lowercase()) else { continue };
            let tb = std::fs::read(twin).expect("read twin");
            let dd = match dds::parse(&tb) {
                Ok(d) => d,
                Err(e) => {
                    t.note(format!("twin not decodable: {e}"), &format!("{at} {}", twin.display()));
                    continue;
                }
            };
            if (dd.width, dd.height) != (w as u32, h as u32) {
                t.note(format!("twin size differs ({f})"), &format!("{at} {} {w}x{h} vs {}x{}", tx.name, dd.width, dd.height));
                continue;
            }
            if dd.mips != tx.mips as u32 {
                t.note(format!("twin mip count differs ({f})"), &format!("{at} {} {} vs {}", tx.name, tx.mips, dd.mips));
            }
            for l in levels.iter().take(dd.mips as usize) {
                let which = if l.level == 0 { "base" } else if (l.level as u32) < texture::packed_level(tx.width as u32, tx.height as u32) { "mip" } else { "packed mip" };
                let (dw, dh) = dd.level_size(l.level as u32);
                let (Ok(a), Ok(b)) = (texture::decode_level(&tx, &mem, l), dd.level(0, l.level as u32).and_then(|s| pixels::decode(dd.format, dw as usize, dh as usize, s))) else {
                    t.note(format!("twin {which}: a side failed to decode ({f})"), &format!("{at} {} level {}", tx.name, l.level));
                    continue;
                };
                let diff: u64 = a.iter().zip(&b).map(|(x, y)| (*x as i32 - *y as i32).unsigned_abs() as u64).sum();
                let mean = diff as f64 / a.len() as f64;
                let verdict = if diff == 0 { "identical" } else if mean < 2.0 { "mean |diff| < 2" } else if mean < 16.0 { "mean |diff| < 16" } else { "differ" };
                t.note(format!("twin {which} {verdict} ({f} vs DDS {:?})", dd.format), &format!("{at} {} level {} {}x{} mean {mean:.2}", tx.name, l.level, l.width, l.height));
            }
        }
    }
    for (k, n) in &t.counts {
        println!("{n:>8}  {k}   e.g. {}", t.examples[k]);
    }
}
