//! HCT XML oracle harness: cross-validate our `hkaSplineCompressedAnimation`
//! reader against Havok Content Tools 6.5's own `AssetCc2 --xml` emit.
//!
//! HCT 6.5 (`Havok-6.5.0-r1`, exact version match with retail Saboteur) is the
//! period-correct reference. We already extracted the AP0L Havok packfile from
//! `Animations.pack` and dumped its XML once via
//!   `AssetCc2.exe --xml retail_anim.hkx retail_anim.xml`
//! yielding 2214 `<hkobject class="hkaSplineCompressedAnimation">` blocks —
//! same count our decoder finds via vfixups.
//!
//! For every clip we compare, field by field:
//!   * scalars: `duration`, `numberOf{Transform,Float}Tracks`, `numFrames`,
//!     `numBlocks`, `maxFramesPerBlock`, `maskAndQuantizationSize`,
//!     `blockDuration`, `blockInverseDuration`, `frameDuration`
//!   * arrays: `blockOffsets`, `floatBlockOffsets`, `transformOffsets`,
//!     `floatOffsets`
//!   * bytes:  `data`
//!
//! Ints/masks/offsets must match EXACTLY; floats are compared with an absolute
//! tolerance of 1e-4 (HCT emits `%f`, i.e. 6 decimal digits, so anything below
//! 1e-6 is quantization noise from the printer, not from our decode).
//!
//! Note the two "float" arrays are not tracked by `super::SplineAnim`; we read
//! them directly from the packfile via the same `Packfile::array` helper.
//!
//! Invoke: `sab_havok65 <pack> validate-hct <retail_anim.xml>`

use crate::{parse_ap0l, read_spline_anim, u32le, Packfile};
use std::collections::HashMap;
use std::fs;

/// FNV-1a 64-bit over the raw data blob. Only needed to pair pack and XML clips
/// (their ID/enumeration orders differ: our vfixup iteration is packfile-order,
/// HCT's XML dump walks the object graph from `toplevelobject`).
fn fnv1a64(b: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &x in b { h ^= x as u64; h = h.wrapping_mul(0x00000100_000001B3); }
    h
}

/// One clip as reported by AssetCc2's XML emit.
struct HctClip {
    duration: f32,
    num_transform_tracks: u32,
    num_float_tracks: u32,
    num_frames: u32,
    num_blocks: u32,
    max_frames_per_block: u32,
    mask_and_quant_size: u32,
    block_duration: f32,
    block_inv_duration: f32,
    frame_duration: f32,
    block_offsets: Vec<u32>,
    float_block_offsets: Vec<u32>,
    transform_offsets: Vec<u32>,
    float_offsets: Vec<u32>,
    data: Vec<u8>,
}

impl HctClip {
    fn new() -> Self {
        HctClip {
            duration: 0.0, num_transform_tracks: 0, num_float_tracks: 0,
            num_frames: 0, num_blocks: 0, max_frames_per_block: 0,
            mask_and_quant_size: 0, block_duration: 0.0, block_inv_duration: 0.0,
            frame_duration: 0.0,
            block_offsets: Vec::new(), float_block_offsets: Vec::new(),
            transform_offsets: Vec::new(), float_offsets: Vec::new(),
            data: Vec::new(),
        }
    }
}

/// Extract an attribute value (`key="value"`) from a `<hkparam ...>` line.
fn attr<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let needle = format!("{key}=\"");
    let i = line.find(&needle)? + needle.len();
    let rest = &line[i..];
    let j = rest.find('"')?;
    Some(&rest[..j])
}

/// Return the text between `>...</hkparam>` on a one-line hkparam.
fn inline_body(line: &str) -> Option<&str> {
    let gt = line.find('>')? + 1;
    let end = line.rfind("</hkparam>")?;
    if end < gt { return None; }
    Some(&line[gt..end])
}

/// Parse an inline scalar hkparam value into f32.
fn parse_f32(s: &str) -> Result<f32, String> {
    s.trim().parse::<f32>().map_err(|e| format!("bad float {s:?}: {e}"))
}
fn parse_u32(s: &str) -> Result<u32, String> {
    s.trim().parse::<u32>().map_err(|e| format!("bad uint {s:?}: {e}"))
}

/// Parse a whitespace-separated array of u32 (used for the four offset arrays).
fn parse_u32_array(s: &str) -> Result<Vec<u32>, String> {
    s.split_ascii_whitespace()
        .map(|t| t.parse::<u32>().map_err(|e| format!("bad u32 {t:?}: {e}")))
        .collect()
}
/// Parse a whitespace-separated array of u8 (the `data` blob).
fn parse_u8_array(s: &str, expected: usize) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(expected);
    for t in s.split_ascii_whitespace() {
        out.push(t.parse::<u8>().map_err(|e| format!("bad u8 {t:?}: {e}"))?);
    }
    Ok(out)
}

/// Scan the XML for every `hkaSplineCompressedAnimation` block in the order
/// they appear (matches vfixup order in `__data__`) and lift the fields we care
/// about. Naive line-based state machine — no XML parser, no allocations per
/// scalar, and array payloads that span lines are accumulated cheaply.
fn parse_xml(xml: &str) -> Result<Vec<HctClip>, String> {
    let mut clips = Vec::new();
    let mut cur = HctClip::new();
    let mut in_sca = false;
    let mut collecting: Option<(String, usize)> = None; // (field, expected numelts)
    let mut accum = String::new();

    for line in xml.lines() {
        let t = line.trim_start();
        if !in_sca {
            if t.starts_with("<hkobject")
                && t.contains("class=\"hkaSplineCompressedAnimation\"")
            {
                in_sca = true;
                cur = HctClip::new();
            }
            continue;
        }
        // inside a SCA block
        if let Some((field, _n)) = &collecting {
            if t.starts_with("</hkparam>") {
                let field = field.clone();
                let body = std::mem::take(&mut accum);
                assign_array(&mut cur, &field, &body)?;
                collecting = None;
            } else {
                accum.push_str(t);
                accum.push(' ');
            }
            continue;
        }
        if t.starts_with("</hkobject>") {
            clips.push(std::mem::replace(&mut cur, HctClip::new()));
            in_sca = false;
            continue;
        }
        if !t.starts_with("<hkparam") {
            continue;
        }
        let name = match attr(t, "name") { Some(n) => n.to_string(), None => continue };
        let is_array = t.contains("numelements=");
        if is_array {
            // Two shapes: single-line `<hkparam name="X" numelements="N"> ... </hkparam>`
            // and multi-line where the closing tag is on its own line.
            let numelts = attr(t, "numelements").and_then(|s| s.parse::<usize>().ok())
                .ok_or_else(|| format!("bad numelements on {t:?}"))?;
            if t.contains("</hkparam>") {
                let body = inline_body(t).unwrap_or("");
                assign_array(&mut cur, &name, body)?;
            } else {
                collecting = Some((name, numelts));
                accum.clear();
            }
        } else if let Some(body) = inline_body(t) {
            assign_scalar(&mut cur, &name, body)?;
        }
    }
    Ok(clips)
}

fn assign_scalar(c: &mut HctClip, name: &str, body: &str) -> Result<(), String> {
    match name {
        "duration"                  => c.duration = parse_f32(body)?,
        "numberOfTransformTracks"   => c.num_transform_tracks = parse_u32(body)?,
        "numberOfFloatTracks"       => c.num_float_tracks = parse_u32(body)?,
        "numFrames"                 => c.num_frames = parse_u32(body)?,
        "numBlocks"                 => c.num_blocks = parse_u32(body)?,
        "maxFramesPerBlock"         => c.max_frames_per_block = parse_u32(body)?,
        "maskAndQuantizationSize"   => c.mask_and_quant_size = parse_u32(body)?,
        "blockDuration"             => c.block_duration = parse_f32(body)?,
        "blockInverseDuration"      => c.block_inv_duration = parse_f32(body)?,
        "frameDuration"             => c.frame_duration = parse_f32(body)?,
        _ => {} // "type", "extractedMotion", "annotationTracks" (empty), "endian" — not compared
    }
    Ok(())
}

fn assign_array(c: &mut HctClip, name: &str, body: &str) -> Result<(), String> {
    match name {
        "blockOffsets"       => c.block_offsets       = parse_u32_array(body)?,
        "floatBlockOffsets"  => c.float_block_offsets = parse_u32_array(body)?,
        "transformOffsets"   => c.transform_offsets   = parse_u32_array(body)?,
        "floatOffsets"       => c.float_offsets       = parse_u32_array(body)?,
        "data"               => c.data                = parse_u8_array(body, 0)?,
        _ => {} // "annotationTracks" (always empty in retail)
    }
    Ok(())
}

/// One divergence.
#[derive(Debug)]
struct Diff {
    field: &'static str,
    ours: String,
    hct: String,
}

/// Absolute-tolerance float compare. HCT prints 6 digits; 1e-4 gives 3 orders
/// of headroom without letting a genuine mismatch through.
const F_TOL: f32 = 1e-4;

fn cmp_f(field: &'static str, ours: f32, hct: f32, out: &mut Vec<Diff>) {
    let diff = (ours - hct).abs();
    let bad = if !ours.is_finite() || !hct.is_finite() {
        ours.to_bits() != hct.to_bits()
    } else {
        diff > F_TOL
    };
    if bad {
        out.push(Diff { field, ours: format!("{ours}"), hct: format!("{hct}") });
    }
}
fn cmp_u<T: PartialEq + std::fmt::Debug>(field: &'static str, ours: T, hct: T, out: &mut Vec<Diff>) {
    if ours != hct {
        out.push(Diff { field, ours: format!("{ours:?}"), hct: format!("{hct:?}") });
    }
}
fn cmp_arr_u32(field: &'static str, ours: &[u32], hct: &[u32], out: &mut Vec<Diff>) {
    if ours != hct {
        out.push(Diff {
            field,
            ours: format!("len={} {:?}", ours.len(), &ours[..ours.len().min(8)]),
            hct:  format!("len={} {:?}", hct.len(),  &hct[..hct.len().min(8)]),
        });
    }
}
/// Byte-compare the data blob. Report the first differing offset if any.
fn cmp_data(ours: &[u8], hct: &[u8], out: &mut Vec<Diff>) {
    if ours.len() != hct.len() {
        out.push(Diff {
            field: "data.len",
            ours: format!("{}", ours.len()),
            hct: format!("{}", hct.len()),
        });
    }
    let n = ours.len().min(hct.len());
    if let Some(i) = (0..n).find(|&i| ours[i] != hct[i]) {
        // Also count total mismatched bytes so we know the scope.
        let mut mism = 0usize;
        for k in 0..n { if ours[k] != hct[k] { mism += 1; } }
        let show = |b: &[u8], off: usize| {
            let e = (off + 8).min(b.len());
            b[off..e].iter().map(|x| format!("{x}")).collect::<Vec<_>>().join(" ")
        };
        out.push(Diff {
            field: "data.bytes",
            ours: format!("@0x{i:x} [{}] ({} mism)", show(ours, i), mism),
            hct:  format!("@0x{i:x} [{}]", show(hct, i)),
        });
    }
}

/// Run the harness. Reads `<pack>` (AP0L), parses the XML, and diffs every
/// SCA clip. Prints aggregate + up to `MAX_REPORT` divergent clips.
pub fn run(pack_path: &str, xml_path: &str) {
    const MAX_REPORT: usize = 10;

    let pack = fs::read(pack_path).unwrap_or_else(|e| {
        eprintln!("read pack {pack_path}: {e}"); std::process::exit(1)
    });
    let ap = parse_ap0l(&pack).unwrap_or_else(|e| {
        eprintln!("parse AP0L: {e}"); std::process::exit(1)
    });
    let blob = &pack[ap.blob_off..ap.blob_off + ap.hk_size];
    let pk = Packfile::parse(blob).unwrap_or_else(|e| {
        eprintln!("parse packfile: {e}"); std::process::exit(1)
    });
    let scas: Vec<usize> = pk.vfixups.iter()
        .filter(|(_, c)| c == "hkaSplineCompressedAnimation")
        .map(|(s, _)| *s).collect();
    println!(
        "pack:  {} SCA instances (Havok {})",
        scas.len(), pk.version
    );

    let xml_txt = fs::read_to_string(xml_path).unwrap_or_else(|e| {
        eprintln!("read xml {xml_path}: {e}"); std::process::exit(1)
    });
    let hct = parse_xml(&xml_txt).unwrap_or_else(|e| {
        eprintln!("parse xml: {e}"); std::process::exit(1)
    });
    println!("xml:   {} SCA blocks", hct.len());

    if hct.len() != scas.len() {
        eprintln!("!! count mismatch: pack has {}, xml has {} — cannot align",
                  scas.len(), hct.len());
        std::process::exit(2);
    }

    // Pair pack and XML clips by content fingerprint (data length + FNV-1a of
    // the data blob). Our vfixup iteration order is packfile write-order, HCT
    // numbers objects by graph traversal, so index-to-index alignment is wrong.
    let mut hct_by_fp: HashMap<(u32, u64), Vec<usize>> = HashMap::new();
    for (i, h) in hct.iter().enumerate() {
        hct_by_fp.entry((h.data.len() as u32, fnv1a64(&h.data))).or_default().push(i);
    }

    let mut clean = 0usize;
    let mut dirty: Vec<(usize, usize, Vec<Diff>)> = Vec::new();
    let mut unpaired_pack: Vec<usize> = Vec::new();
    let mut used_hct = vec![false; hct.len()];
    for (i, &src) in scas.iter().enumerate() {
        let ours = read_spline_anim(&pk, src);
        // Read the two float* arrays that SplineAnim does not track.
        let (fbo_dst, fbo_n) = pk.array(src, 0x4c);
        let mut float_block_offsets = Vec::with_capacity(fbo_n);
        if let Some(d) = fbo_dst {
            for k in 0..fbo_n { float_block_offsets.push(u32le(blob, d + k * 4)); }
        }
        let (fo_dst, fo_n) = pk.array(src, 0x64);
        let mut float_offsets = Vec::with_capacity(fo_n);
        if let Some(d) = fo_dst {
            for k in 0..fo_n { float_offsets.push(u32le(blob, d + k * 4)); }
        }
        let ours_data = &blob[ours.data_off..ours.data_off + ours.data_len];

        // Pair by fingerprint. If it's unique, take that HCT index. If
        // several XML clips share the fingerprint (unlikely but possible for
        // near-identical clips), take the first unused one.
        let fp = (ours.data_len as u32, fnv1a64(ours_data));
        let hct_idx = match hct_by_fp.get(&fp) {
            None => { unpaired_pack.push(i); continue; }
            Some(v) => {
                match v.iter().find(|&&j| !used_hct[j]).copied() {
                    Some(j) => j,
                    None => { unpaired_pack.push(i); continue; }
                }
            }
        };
        used_hct[hct_idx] = true;
        let h = &hct[hct_idx];
        let mut diffs = Vec::new();
        cmp_f("duration",             ours.duration,               h.duration,             &mut diffs);
        cmp_u("numTransformTracks",   ours.num_transform_tracks as u32, h.num_transform_tracks, &mut diffs);
        cmp_u("numFloatTracks",       ours.num_float_tracks as u32,     h.num_float_tracks,     &mut diffs);
        cmp_u("numFrames",            ours.num_frames as u32,           h.num_frames,           &mut diffs);
        cmp_u("numBlocks",            ours.num_blocks as u32,           h.num_blocks,           &mut diffs);
        cmp_u("maxFramesPerBlock",    ours.max_frames_per_block as u32, h.max_frames_per_block, &mut diffs);
        cmp_u("maskAndQuantizationSize",
                                      ours.mask_and_quant_size as u32,  h.mask_and_quant_size,  &mut diffs);
        cmp_f("blockDuration",        ours.block_duration,          h.block_duration,       &mut diffs);
        cmp_f("blockInverseDuration", ours.block_inv_duration,      h.block_inv_duration,   &mut diffs);
        cmp_f("frameDuration",        ours.frame_duration,          h.frame_duration,       &mut diffs);
        cmp_arr_u32("blockOffsets",       &ours.block_offsets,     &h.block_offsets,       &mut diffs);
        cmp_arr_u32("floatBlockOffsets",  &float_block_offsets,    &h.float_block_offsets, &mut diffs);
        cmp_arr_u32("transformOffsets",   &ours.transform_offsets, &h.transform_offsets,   &mut diffs);
        cmp_arr_u32("floatOffsets",       &float_offsets,          &h.float_offsets,       &mut diffs);
        cmp_data(ours_data, &h.data, &mut diffs);

        if diffs.is_empty() { clean += 1; } else { dirty.push((i, hct_idx, diffs)); }
    }

    let total = scas.len();
    println!("\n=== HCT vs decoder ===");
    println!("clean: {clean}/{total}");
    println!("dirty: {}/{total}", dirty.len());
    if !unpaired_pack.is_empty() {
        println!("unpaired pack clips (no matching XML fingerprint): {}", unpaired_pack.len());
        let n = unpaired_pack.len().min(5);
        for i in &unpaired_pack[..n] { println!("  pack #{i}"); }
    }
    let n_unused_hct = used_hct.iter().filter(|u| !**u).count();
    if n_unused_hct > 0 {
        println!("unused XML clips (no matching pack fingerprint): {}", n_unused_hct);
    }
    if dirty.is_empty() {
        println!("All fields match to <=1e-4 (floats) / exact (ints/bytes/masks).");
        return;
    }
    let n_show = dirty.len().min(MAX_REPORT);
    println!("first {n_show} divergent clip(s):");
    for (i, hi, diffs) in dirty.iter().take(MAX_REPORT) {
        println!("  pack #{i}  <->  xml #{hi}:  {} diffs", diffs.len());
        for d in diffs.iter().take(6) {
            println!("    {:<24}  ours={}  hct={}", d.field, d.ours, d.hct);
        }
        if diffs.len() > 6 { println!("    ... +{} more", diffs.len() - 6); }
    }
}
