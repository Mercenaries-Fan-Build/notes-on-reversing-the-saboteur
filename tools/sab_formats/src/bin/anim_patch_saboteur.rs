//! `anim_patch_saboteur` — write-side tooling for The Saboteur's animation pack.
//!
//! Uses Havok Content Tools 6.5's `AssetCc2.exe` as a period-correct oracle
//! (packfile serializer / XML round-tripper) and Pandemic's own AP0L container
//! ([`sab_formats::ap0l`]) to author modified `Animations.pack` files without
//! disturbing the ANIM/INTV/SEQC/TRAN/EDGE/BANK/ADD1 metadata (which carries
//! the track→bone binding — Saboteur's equivalent of Mercs 2's `trnm` chunk).
//!
//! Subcommands:
//!
//! * `extract-hkx <pack> <out.hkx>`
//!     Dump the concatenated Havok 6.5 packfile from an AP0L pack.
//!
//! * `replace-hkx <pack> <in.hkx> <out.pack>`
//!     Splice a new Havok packfile back into the pack. Head (AP0L + ANIM)
//!     and tail (INTV..SSP0) are preserved byte-for-byte. `hkSize` is
//!     rewritten to match `in.hkx`.
//!
//! * `roundtrip <pack> --assetcc <exe> --out <pack> [--keep-artifacts <dir>]`
//!     Baseline: `pack -> extract hkx -> AssetCc2 --xml -> AssetCc2 --strip
//!     --rules4101 -> splice`. Proves the toolchain leaves head/tail
//!     byte-identical and the Havok blob decode-equivalent.
//!
//! * `perturb-clip <pack> --assetcc <exe> --out <pack> --clip <index>
//!         --set-duration <seconds> [--keep-artifacts <dir>]`
//!     Round-trip and, before recompiling, rewrite the `duration` hkparam of
//!     the given `hkaSplineCompressedAnimation` object. AssetCc2 recompiles;
//!     splice back.
//!
//! * `verify-splice <baseline.pack> <modified.pack>`
//!     Byte-compare the two packs OUTSIDE the Havok blob. This is the
//!     acceptance test that the AP0L splice preserved every non-Havok byte
//!     (ANIM track→bone maps included).
//!
//! * `debug-map <pack> <xml>`   *(diagnostic)*
//!     Show the fixup→XML clip-index mapping, plus the first few fingerprints
//!     from each side. Handy when a fingerprint-collision report needs to be
//!     understood.
//!
//! `--keep-artifacts <dir>` copies the intermediate `.hkx`/`.xml` files
//! aside so a maintainer can inspect what AssetCc2 saw and produced.
//!
//! Do NOT write to `C:/GOG Games/The Saboteur/`. Emit to a mod slot or the
//! scratchpad. See memory `[[gog-install-is-the-only-target]]` and
//! `[[dlc-slot-mount-rules]]`.

use sab_formats::ap0l::{
    find_nth_object, hash_bytes as ap0l_hash_bytes, map_hk_to_xml_indices, set_hkparam_in_object,
    xml_spline_fingerprint, Ap0lPack, HkPackfile,
};
use std::path::{Path, PathBuf};
use std::process::{exit, Command};
use std::{fs, io};

const CLASS_SPLINE: &str = "hkaSplineCompressedAnimation";

// ------------------------------------------------------------- arg parsing

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1).cloned())
}
fn parse_usize(s: &str) -> Result<usize, String> {
    s.parse::<usize>().map_err(|_| format!("bad integer {s:?}"))
}
fn parse_f32(s: &str) -> Result<f32, String> {
    s.parse::<f32>().map_err(|_| format!("bad float {s:?}"))
}

fn usage() -> ! {
    eprintln!("\
anim_patch_saboteur — author modded Havok clips in a Saboteur Animations.pack.

  extract-hkx <pack> <out.hkx>
  replace-hkx <pack> <in.hkx> <out.pack>
  roundtrip   <pack> --assetcc <AssetCc2.exe> --out <pack>
              [--keep-artifacts <dir>]
  perturb-clip <pack> --assetcc <AssetCc2.exe> --out <pack> --clip <index>
              --set-duration <seconds> [--keep-artifacts <dir>]
  verify-splice <baseline.pack> <modified.pack>
  debug-map    <pack> <xml>                                # diagnostic");
    exit(2);
}

// ------------------------------------------------------------- AssetCc2

fn find_assetcc(explicit: Option<String>) -> Result<PathBuf, String> {
    if let Some(p) = explicit {
        let pb = PathBuf::from(p);
        if pb.exists() { return Ok(pb); }
        return Err(format!("AssetCc2 not found at {}", pb.display()));
    }
    let candidates = [
        r"C:\Program Files (x86)\Havok\BatchProcess\AssetCc\AssetCc2.exe",
        r"C:\Program Files\Havok\BatchProcess\AssetCc\AssetCc2.exe",
    ];
    for c in candidates {
        if Path::new(c).exists() {
            return Ok(PathBuf::from(c));
        }
    }
    Err("--assetcc <path> not given and AssetCc2 not at default HCT 6.5 install location".into())
}

/// `AssetCc2 --xml <in.hkx> <out.xml>`.
fn assetcc_decompile(exe: &Path, hkx: &Path, xml_out: &Path) -> Result<(), String> {
    let status = Command::new(exe)
        .args([
            "--xml".as_ref(),
            hkx.as_os_str(),
            xml_out.as_os_str(),
        ])
        .status()
        .map_err(|e| format!("run AssetCc2 --xml: {e}"))?;
    if !status.success() {
        return Err(format!("AssetCc2 --xml exited {:?}", status.code()));
    }
    Ok(())
}

/// `AssetCc2 --strip --rules4101 <in.xml> <out.hkx>`.
fn assetcc_compile(exe: &Path, xml: &Path, hkx_out: &Path) -> Result<(), String> {
    let status = Command::new(exe)
        .args([
            "--strip".as_ref(),
            "--rules4101".as_ref(),
            xml.as_os_str(),
            hkx_out.as_os_str(),
        ])
        .status()
        .map_err(|e| format!("run AssetCc2 --strip: {e}"))?;
    if !status.success() {
        return Err(format!("AssetCc2 --strip exited {:?}", status.code()));
    }
    Ok(())
}

// ------------------------------------------------------------- scratch dir

/// Where to drop AssetCc2 inputs/outputs during a round-trip. Prefers the
/// harness scratchpad ($CLAUDE_SCRATCH), falls back to `std::env::temp_dir()`.
fn scratch_dir(prefix: &str) -> io::Result<PathBuf> {
    let base = std::env::var("CLAUDE_SCRATCH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir());
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = base.join(format!("{prefix}_{ts:x}"));
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn copy_artifacts(from: &[&Path], into: &Option<PathBuf>) -> Result<(), String> {
    let Some(dir) = into else { return Ok(()); };
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    for src in from {
        let name = src.file_name().ok_or("artifact has no filename")?;
        fs::copy(src, dir.join(name))
            .map_err(|e| format!("copy {} -> {}: {e}", src.display(), dir.display()))?;
    }
    Ok(())
}

// ------------------------------------------------------------- commands

fn cmd_extract_hkx(args: &[String]) -> Result<(), String> {
    let pack = args.get(1).ok_or("need <pack>")?;
    let out = args.get(2).ok_or("need <out.hkx>")?;
    let bytes = fs::read(pack).map_err(|e| format!("read pack: {e}"))?;
    let ap = Ap0lPack::parse(bytes).map_err(|e| e.to_string())?;
    fs::write(out, ap.hk_blob()).map_err(|e| format!("write hkx: {e}"))?;
    println!(
        "extracted {} bytes ({} clips inside) from AP0L at offset 0x{:x} -> {}",
        ap.hk_size(),
        ap.num_anims(),
        ap.hk_offset(),
        out
    );
    Ok(())
}

fn cmd_replace_hkx(args: &[String]) -> Result<(), String> {
    let pack = args.get(1).ok_or("need <pack>")?;
    let hkx = args.get(2).ok_or("need <in.hkx>")?;
    let out = args.get(3).ok_or("need <out.pack>")?;
    let bytes = fs::read(pack).map_err(|e| format!("read pack: {e}"))?;
    let new_blob = fs::read(hkx).map_err(|e| format!("read hkx: {e}"))?;
    let ap = Ap0lPack::parse(bytes).map_err(|e| e.to_string())?;
    let spliced = ap.splice_hk_blob(&new_blob);
    fs::write(out, &spliced).map_err(|e| format!("write pack: {e}"))?;
    println!(
        "spliced {} bytes of Havok data (Δ={:+}) -> {} ({} bytes total)",
        new_blob.len(),
        new_blob.len() as i64 - ap.hk_size() as i64,
        out,
        spliced.len()
    );
    Ok(())
}

fn round_trip_blob(
    exe: &Path,
    blob: &[u8],
    keep: &Option<PathBuf>,
) -> Result<Vec<u8>, String> {
    let dir = scratch_dir("anim_patch").map_err(|e| e.to_string())?;
    let hkx = dir.join("input.hkx");
    let xml = dir.join("input.xml");
    let rt = dir.join("rt.hkx");
    fs::write(&hkx, blob).map_err(|e| format!("write scratch hkx: {e}"))?;
    assetcc_decompile(exe, &hkx, &xml)?;
    assetcc_compile(exe, &xml, &rt)?;
    let out = fs::read(&rt).map_err(|e| format!("read rt hkx: {e}"))?;
    copy_artifacts(&[&hkx, &xml, &rt], keep)?;
    if keep.is_none() {
        let _ = fs::remove_dir_all(&dir);
    }
    Ok(out)
}

fn cmd_roundtrip(args: &[String]) -> Result<(), String> {
    let pack = args.get(1).ok_or("need <pack>")?;
    let out = arg(args, "--out").ok_or("need --out <pack>")?;
    let exe = find_assetcc(arg(args, "--assetcc"))?;
    let keep = arg(args, "--keep-artifacts").map(PathBuf::from);

    let bytes = fs::read(pack).map_err(|e| format!("read pack: {e}"))?;
    let ap = Ap0lPack::parse(bytes).map_err(|e| e.to_string())?;
    let blob_in = ap.hk_blob().to_vec();
    println!(
        "extract: {} bytes at 0x{:x} ({} clips)",
        blob_in.len(),
        ap.hk_offset(),
        ap.num_anims()
    );
    let blob_out = round_trip_blob(&exe, &blob_in, &keep)?;
    println!(
        "AssetCc2 round-trip: {} bytes -> {} bytes (Δ={:+})",
        blob_in.len(),
        blob_out.len(),
        blob_out.len() as i64 - blob_in.len() as i64
    );
    let spliced = ap.splice_hk_blob(&blob_out);
    fs::write(&out, &spliced).map_err(|e| format!("write out: {e}"))?;
    println!("wrote {} ({} bytes)", out, spliced.len());
    Ok(())
}

fn cmd_perturb_clip(args: &[String]) -> Result<(), String> {
    let pack = args.get(1).ok_or("need <pack>")?;
    let out = arg(args, "--out").ok_or("need --out <pack>")?;
    let clip: usize = parse_usize(&arg(args, "--clip").ok_or("need --clip <index>")?)?;
    let exe = find_assetcc(arg(args, "--assetcc"))?;
    let keep = arg(args, "--keep-artifacts").map(PathBuf::from);

    let set_duration = arg(args, "--set-duration").map(|s| parse_f32(&s)).transpose()?;
    if set_duration.is_none() {
        return Err("need at least one edit flag (e.g. --set-duration <seconds>)".into());
    }

    let bytes = fs::read(pack).map_err(|e| format!("read pack: {e}"))?;
    let ap = Ap0lPack::parse(bytes).map_err(|e| e.to_string())?;
    // Sanity: report the target clip's numTransformTracks so a maintainer can
    // sanity-check they're editing what they think they're editing.
    let hk = HkPackfile::parse(ap.hk_blob()).map_err(|e| e.to_string())?;
    let scas = hk.objects_of_class(CLASS_SPLINE);
    let obj = *scas.get(clip).ok_or_else(|| {
        format!("clip index {clip} out of range (pack has {} spline anims)", scas.len())
    })?;
    let ntt = hk.num_transform_tracks(obj);
    println!(
        "target clip #{clip} @ data+0x{:x}, numTransformTracks={} (Havok {})",
        obj,
        ntt,
        hk.version()
    );

    // Round-trip with XML editing:
    let dir = scratch_dir("anim_perturb").map_err(|e| e.to_string())?;
    let hkx = dir.join("input.hkx");
    let xml = dir.join("input.xml");
    let xml_mod = dir.join("perturbed.xml");
    let rt = dir.join("perturbed.hkx");
    fs::write(&hkx, ap.hk_blob()).map_err(|e| format!("write scratch hkx: {e}"))?;
    assetcc_decompile(&exe, &hkx, &xml)?;
    let xml_text = fs::read_to_string(&xml).map_err(|e| format!("read xml: {e}"))?;
    // AssetCc2 emits objects in a different order than the Havok packfile's
    // virtual-fixup table. sab_havok65 / sab_animmeta both index clips by
    // fixup order, so we translate --clip N (fixup order) into the matching
    // XML declaration index by structural fingerprint.
    let hk_fps: Vec<_> = scas.iter().map(|&s| hk.spline_fingerprint(s)).collect();
    let hk_hashes: Vec<_> = scas.iter().map(|&s| ap0l_hash_bytes(hk.spline_data(s))).collect();
    let hk_to_xml =
        map_hk_to_xml_indices(&xml_text, &hk_fps, &hk_hashes).map_err(|e| e.to_string())?;
    let xml_idx = hk_to_xml[clip];
    if xml_idx != clip {
        println!(
            "note: fixup clip #{clip} = XML object #{xml_idx} (AssetCc2 reordered on decompile)",
        );
    }
    let (obj_start, obj_end) = find_nth_object(&xml_text, CLASS_SPLINE, xml_idx)
        .ok_or_else(|| format!("clip #{clip} (XML idx {xml_idx}) not found in XML"))?;
    let mut mutated = xml_text.clone();
    if let Some(seconds) = set_duration {
        // Match AssetCc2's own %f style (six decimal places) to stay diff-friendly.
        let repl = format!("{seconds:.6}");
        // The `duration` param appears once in each hkaSplineCompressedAnimation object.
        // `set_hkparam_in_object` guards against ambiguity for us.
        mutated = set_hkparam_in_object(&mutated, obj_start, obj_end, "duration", &repl)?;
        println!("set clip #{clip}.duration -> {repl}");
    }
    fs::write(&xml_mod, &mutated).map_err(|e| format!("write perturbed xml: {e}"))?;
    assetcc_compile(&exe, &xml_mod, &rt)?;
    let blob_out = fs::read(&rt).map_err(|e| format!("read perturbed hkx: {e}"))?;

    let spliced = ap.splice_hk_blob(&blob_out);
    fs::write(&out, &spliced).map_err(|e| format!("write out: {e}"))?;
    println!(
        "AssetCc2 recompile: {} bytes (input was {}, Δ={:+}); wrote {} ({} bytes)",
        blob_out.len(),
        ap.hk_blob().len(),
        blob_out.len() as i64 - ap.hk_blob().len() as i64,
        out,
        spliced.len(),
    );
    copy_artifacts(&[&hkx, &xml, &xml_mod, &rt], &keep)?;
    if keep.is_none() {
        let _ = fs::remove_dir_all(&dir);
    }
    Ok(())
}

fn cmd_debug_map(args: &[String]) -> Result<(), String> {
    let pack = args.get(1).ok_or("need <pack>")?;
    let xml = args.get(2).ok_or("need <xml>")?;
    let bytes = fs::read(pack).map_err(|e| e.to_string())?;
    let ap = Ap0lPack::parse(bytes).map_err(|e| e.to_string())?;
    let hk = HkPackfile::parse(ap.hk_blob()).map_err(|e| e.to_string())?;
    let scas = hk.objects_of_class(CLASS_SPLINE);
    let hk_fps: Vec<_> = scas.iter().map(|&s| hk.spline_fingerprint(s)).collect();
    let xml_text = fs::read_to_string(xml).map_err(|e| e.to_string())?;
    let mut xi = 0usize;
    let mut xml_fps = Vec::new();
    while let Some(fp) = xml_spline_fingerprint(&xml_text, xi) {
        xml_fps.push(fp);
        xi += 1;
    }
    println!("packfile spline anims: {}, XML spline anims: {}", hk_fps.len(), xml_fps.len());
    println!("first 3 fixup fingerprints:");
    for (i, fp) in hk_fps.iter().take(3).enumerate() {
        println!("  fx#{i}: {fp:?}");
    }
    println!("first 3 XML fingerprints:");
    for (i, fp) in xml_fps.iter().take(3).enumerate() {
        println!("  xm#{i}: {fp:?}");
    }
    let hk_hashes: Vec<_> = scas.iter().map(|&s| ap0l_hash_bytes(hk.spline_data(s))).collect();
    let map = map_hk_to_xml_indices(&xml_text, &hk_fps, &hk_hashes).map_err(|e| e.to_string())?;
    println!("map[0..5] = {:?}", &map[..5.min(map.len())]);
    // Sanity: how many identity vs shuffled?
    let identity = map.iter().enumerate().filter(|(i, x)| *i == **x).count();
    println!("clips with identity mapping: {}/{}", identity, map.len());
    Ok(())
}

fn cmd_verify_splice(args: &[String]) -> Result<(), String> {
    let a = args.get(1).ok_or("need <baseline.pack>")?;
    let b = args.get(2).ok_or("need <modified.pack>")?;
    let av = fs::read(a).map_err(|e| format!("read {a}: {e}"))?;
    let bv = fs::read(b).map_err(|e| format!("read {b}: {e}"))?;
    let ap = Ap0lPack::parse(av.clone()).map_err(|e| e.to_string())?;
    let bp = Ap0lPack::parse(bv.clone()).map_err(|e| e.to_string())?;

    if ap.head() != bp.head() {
        // Locate diverging bytes (allow only the hkSize field at hk_offset-4 to differ).
        let hk_size_field = ap.hk_offset() - 4;
        let mut diffs = 0usize;
        let mut first_diff = None;
        for (i, (x, y)) in ap.head().iter().zip(bp.head().iter()).enumerate() {
            if x != y {
                if first_diff.is_none() {
                    first_diff = Some(i);
                }
                if !(hk_size_field..hk_size_field + 4).contains(&i) {
                    diffs += 1;
                }
            }
        }
        if diffs > 0 {
            return Err(format!(
                "head differs in {diffs} non-hkSize bytes (first diff @0x{:x})",
                first_diff.unwrap_or(0)
            ));
        }
        println!(
            "head differs ONLY at hkSize field @0x{hk_size_field:x}: {} -> {}",
            ap.hk_size(),
            bp.hk_size()
        );
    } else {
        println!("head byte-identical ({} bytes)", ap.head().len());
    }

    if ap.tail() != bp.tail() {
        let n_diff = ap.tail().iter().zip(bp.tail().iter()).filter(|(x, y)| x != y).count()
            + ap.tail().len().abs_diff(bp.tail().len());
        return Err(format!(
            "tail differs in {n_diff} bytes (baseline={}B, modified={}B) — \
             AP0L splice would have shifted, not modified, the tail",
            ap.tail().len(),
            bp.tail().len()
        ));
    }
    println!("tail byte-identical ({} bytes)", ap.tail().len());

    // Report Havok blob delta.
    let ad = ap.hk_blob();
    let bd = bp.hk_blob();
    let common = ad.len().min(bd.len());
    let diff_bytes = ad.iter().zip(bd.iter()).filter(|(x, y)| x != y).count();
    println!(
        "havok blob: baseline={}B, modified={}B (Δ={:+}), differing bytes in overlap={} ({:.3}%)",
        ad.len(),
        bd.len(),
        bd.len() as i64 - ad.len() as i64,
        diff_bytes,
        100.0 * diff_bytes as f64 / common.max(1) as f64
    );
    Ok(())
}

// ------------------------------------------------------------- main

fn main() {
    let all: Vec<String> = std::env::args().collect();
    if all.len() < 2 {
        usage();
    }
    let cmd = all[1].clone();
    let rest = &all[1..];
    let r = match cmd.as_str() {
        "extract-hkx" => cmd_extract_hkx(rest),
        "replace-hkx" => cmd_replace_hkx(rest),
        "roundtrip" => cmd_roundtrip(rest),
        "perturb-clip" => cmd_perturb_clip(rest),
        "verify-splice" => cmd_verify_splice(rest),
        "debug-map" => cmd_debug_map(rest),
        _ => usage(),
    };
    if let Err(e) = r {
        eprintln!("error: {e}");
        exit(1);
    }
}
