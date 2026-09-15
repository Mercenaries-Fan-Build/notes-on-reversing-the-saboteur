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
    find_nth_object, hash_bytes as ap0l_hash_bytes, map_hk_to_xml_indices,
    offset_static_translation, set_hkparam_in_object, set_static_rotation, set_static_scale,
    set_static_translation, walk_spline_track_layouts, xml_spline_fingerprint, Ap0lPack,
    HkPackfile, StaticEditError, TrackLayout,
};
use sab_formats::spline_edit::{
    offset_frame_translation, set_frame_rotation, set_frame_scale, set_frame_translation, ClipDims,
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
  perturb-clip <pack> --out <pack> --clip <index> <edit-flag ...>
              [--assetcc <AssetCc2.exe>] [--keep-artifacts <dir>]

    Edit flags (any combination, at least one required):
      --set-duration <seconds>                        (requires --assetcc)
      --set-static-translation <track> <x> <y> <z>
      --set-static-rotation    <track> <qx> <qy> <qz> <qw>
      --set-static-scale       <track> <sx> <sy> <sz>
      --offset-static-translation <track> <dx> <dy> <dz>

    Frame flags (each carries its own <clip>; --clip may be omitted when
    only frame flags are used; require --assetcc):
      --set-frame-translation <clip> <track> <frame> <x> <y> <z>
      --set-frame-rotation    <clip> <track> <frame> <qx> <qy> <qz> <qw>
      --set-frame-scale       <clip> <track> <frame> <sx> <sy> <sz>
      --offset-frame-translation <clip> <track> <frame> <dx> <dy> <dz>

    Static-value edits patch the raw compressed data stream directly and do
    NOT need AssetCc2. --set-duration edits an XML hkparam, so it needs the
    AssetCc2 round-trip. Frame edits keyframe-bake the target track (CP-count
    == num_frames, degree-1 knots), which changes the `data` hkArray size, so
    they also need the AssetCc2 round-trip (which handles the packfile-level
    plumbing). Any combination may be given: AssetCc2 round-trip + XML
    edits (duration, frame `data` replacements) run first, then static edits
    on the round-tripped blob.

  verify-splice <baseline.pack> <modified.pack>
  debug-map    <pack> <xml>                                # diagnostic
  probe-clip   <pack> --clip <index>                       # list per-track
                                                             mask + which
                                                             tracks accept
                                                             a static edit");
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

/// One parsed static-value edit request. `track` is a fixup-order track index
/// into the target clip's transform tracks. The value fields carry the new
/// value (for `Set*` variants) or the delta (for `Offset*`).
#[derive(Debug)]
enum StaticEdit {
    SetTranslation { track: usize, xyz: [f32; 3] },
    SetRotation { track: usize, quat: [f32; 4] },
    SetScale { track: usize, sxyz: [f32; 3] },
    OffsetTranslation { track: usize, delta: [f32; 3] },
}

/// Pull every occurrence of `name` from `args` and, for each occurrence,
/// consume the next `arity` positional arguments as the payload. Returns the
/// payloads (each a `Vec<&str>` of length `arity`) or an error naming which
/// occurrence was short.
fn collect_multi<'a>(args: &'a [String], name: &str, arity: usize) -> Result<Vec<Vec<&'a str>>, String> {
    let mut out = Vec::new();
    for (i, a) in args.iter().enumerate() {
        if a == name {
            let end = i + 1 + arity;
            if end > args.len() {
                return Err(format!("{name} expects {arity} args, only {} present", args.len() - i - 1));
            }
            out.push(args[i + 1..end].iter().map(|s| s.as_str()).collect());
        }
    }
    Ok(out)
}

fn parse_f32s<const N: usize>(vals: &[&str]) -> Result<[f32; N], String> {
    if vals.len() != N {
        return Err(format!("expected {N} floats, got {}", vals.len()));
    }
    let mut out = [0f32; N];
    for (i, v) in vals.iter().enumerate() {
        out[i] = v.parse().map_err(|_| format!("bad float {v:?}"))?;
    }
    Ok(out)
}

/// One parsed per-frame edit request. `clip` and `track` are fixup-order
/// indices; `frame` is a 0-based frame index bounded by the clip's numFrames.
///
/// See the `spline_edit` module's docs for the keyframe-baking approach
/// these ops all take.
#[derive(Debug)]
enum FrameEdit {
    SetTranslation { clip: usize, track: usize, frame: usize, xyz: [f32; 3] },
    SetRotation { clip: usize, track: usize, frame: usize, quat: [f32; 4] },
    SetScale { clip: usize, track: usize, frame: usize, sxyz: [f32; 3] },
    OffsetTranslation { clip: usize, track: usize, frame: usize, delta: [f32; 3] },
}

impl FrameEdit {
    fn clip(&self) -> usize {
        match *self {
            FrameEdit::SetTranslation { clip, .. }
            | FrameEdit::SetRotation { clip, .. }
            | FrameEdit::SetScale { clip, .. }
            | FrameEdit::OffsetTranslation { clip, .. } => clip,
        }
    }
}

fn collect_frame_edits(args: &[String]) -> Result<Vec<FrameEdit>, String> {
    let mut out = Vec::new();
    for v in collect_multi(args, "--set-frame-translation", 6)? {
        let clip = parse_usize(v[0])?;
        let track = parse_usize(v[1])?;
        let frame = parse_usize(v[2])?;
        let xyz = parse_f32s::<3>(&v[3..])?;
        out.push(FrameEdit::SetTranslation { clip, track, frame, xyz });
    }
    for v in collect_multi(args, "--set-frame-rotation", 7)? {
        let clip = parse_usize(v[0])?;
        let track = parse_usize(v[1])?;
        let frame = parse_usize(v[2])?;
        let quat = parse_f32s::<4>(&v[3..])?;
        out.push(FrameEdit::SetRotation { clip, track, frame, quat });
    }
    for v in collect_multi(args, "--set-frame-scale", 6)? {
        let clip = parse_usize(v[0])?;
        let track = parse_usize(v[1])?;
        let frame = parse_usize(v[2])?;
        let sxyz = parse_f32s::<3>(&v[3..])?;
        out.push(FrameEdit::SetScale { clip, track, frame, sxyz });
    }
    for v in collect_multi(args, "--offset-frame-translation", 6)? {
        let clip = parse_usize(v[0])?;
        let track = parse_usize(v[1])?;
        let frame = parse_usize(v[2])?;
        let delta = parse_f32s::<3>(&v[3..])?;
        out.push(FrameEdit::OffsetTranslation { clip, track, frame, delta });
    }
    Ok(out)
}

fn collect_static_edits(args: &[String]) -> Result<Vec<StaticEdit>, String> {
    let mut out = Vec::new();
    for v in collect_multi(args, "--set-static-translation", 4)? {
        let track = parse_usize(v[0])?;
        let xyz = parse_f32s::<3>(&v[1..])?;
        out.push(StaticEdit::SetTranslation { track, xyz });
    }
    for v in collect_multi(args, "--set-static-rotation", 5)? {
        let track = parse_usize(v[0])?;
        let quat = parse_f32s::<4>(&v[1..])?;
        out.push(StaticEdit::SetRotation { track, quat });
    }
    for v in collect_multi(args, "--set-static-scale", 4)? {
        let track = parse_usize(v[0])?;
        let sxyz = parse_f32s::<3>(&v[1..])?;
        out.push(StaticEdit::SetScale { track, sxyz });
    }
    for v in collect_multi(args, "--offset-static-translation", 4)? {
        let track = parse_usize(v[0])?;
        let delta = parse_f32s::<3>(&v[1..])?;
        out.push(StaticEdit::OffsetTranslation { track, delta });
    }
    Ok(out)
}

/// Apply `edits` to the compressed `data` slice of a clip that lives at
/// `[data_off .. data_off + data_len]` inside `blob`. Caller supplies the
/// clip's `num_transform_tracks` and `mask_and_quantization_size`.
fn apply_static_edits_at(
    blob: &mut [u8],
    data_off: usize,
    data_len: usize,
    ntt: usize,
    mq: usize,
    edits: &[StaticEdit],
) -> Result<usize, String> {
    let layouts: Vec<TrackLayout> = {
        let slice = &blob[data_off..data_off + data_len];
        walk_spline_track_layouts(slice, ntt, mq).map_err(|e| e.to_string())?
    };
    let data_mut = &mut blob[data_off..data_off + data_len];
    for e in edits {
        match *e {
            StaticEdit::SetTranslation { track, xyz } => {
                let old = set_static_translation(data_mut, &layouts, track, xyz)
                    .map_err(|e| e.to_string())?;
                println!(
                    "  trk{track}: static translation ({:.4}, {:.4}, {:.4}) -> ({:.4}, {:.4}, {:.4})",
                    old[0], old[1], old[2], xyz[0], xyz[1], xyz[2]
                );
            }
            StaticEdit::SetRotation { track, quat } => {
                let old = set_static_rotation(data_mut, &layouts, track, quat)
                    .map_err(|e| e.to_string())?;
                println!(
                    "  trk{track}: static rotation ({:.4}, {:.4}, {:.4}, {:.4}) -> ({:.4}, {:.4}, {:.4}, {:.4})",
                    old[0], old[1], old[2], old[3], quat[0], quat[1], quat[2], quat[3]
                );
            }
            StaticEdit::SetScale { track, sxyz } => {
                let old = set_static_scale(data_mut, &layouts, track, sxyz)
                    .map_err(|e| e.to_string())?;
                println!(
                    "  trk{track}: static scale ({:.4}, {:.4}, {:.4}) -> ({:.4}, {:.4}, {:.4})",
                    old[0], old[1], old[2], sxyz[0], sxyz[1], sxyz[2]
                );
            }
            StaticEdit::OffsetTranslation { track, delta } => {
                let (old, new) = offset_static_translation(data_mut, &layouts, track, delta)
                    .map_err(|e| e.to_string())?;
                println!(
                    "  trk{track}: static translation ({:.4}, {:.4}, {:.4}) + ({:+.4}, {:+.4}, {:+.4}) = ({:.4}, {:.4}, {:.4})",
                    old[0], old[1], old[2], delta[0], delta[1], delta[2], new[0], new[1], new[2]
                );
            }
        }
    }
    Ok(edits.len())
}

fn cmd_perturb_clip(args: &[String]) -> Result<(), String> {
    let pack = args.get(1).ok_or("need <pack>")?;
    let out = arg(args, "--out").ok_or("need --out <pack>")?;
    let keep = arg(args, "--keep-artifacts").map(PathBuf::from);

    let set_duration = arg(args, "--set-duration").map(|s| parse_f32(&s)).transpose()?;
    let static_edits = collect_static_edits(args)?;
    let frame_edits = collect_frame_edits(args)?;

    // --clip is required for static / duration edits (which are per-clip
    // and take the clip from the flag). Frame edits carry clip inline; if
    // ONLY frame edits are present --clip may be omitted.
    let clip_opt = arg(args, "--clip").map(|s| parse_usize(&s)).transpose()?;
    let need_static_clip = !static_edits.is_empty() || set_duration.is_some();
    let clip: Option<usize> = match (clip_opt, need_static_clip) {
        (Some(c), _) => Some(c),
        (None, true) => return Err("need --clip <index> for --set-duration and --set-static-* edits".into()),
        (None, false) => None,
    };
    let need_assetcc = set_duration.is_some() || !frame_edits.is_empty();
    if !need_assetcc && static_edits.is_empty() {
        return Err(
            "need at least one edit flag (e.g. --set-duration <seconds>, \
             --set-static-translation <track> <x> <y> <z>, or \
             --set-frame-translation <clip> <track> <frame> <x> <y> <z>)".into()
        );
    }

    let bytes = fs::read(pack).map_err(|e| format!("read pack: {e}"))?;
    let ap = Ap0lPack::parse(bytes).map_err(|e| e.to_string())?;
    // Sanity: report each involved clip's numTransformTracks so a maintainer
    // can sanity-check they're editing what they think they're editing.
    let hk = HkPackfile::parse(ap.hk_blob()).map_err(|e| e.to_string())?;
    let scas = hk.objects_of_class(CLASS_SPLINE);
    let mut involved_clips: Vec<usize> = Vec::new();
    if let Some(c) = clip {
        involved_clips.push(c);
    }
    for fe in &frame_edits {
        if !involved_clips.contains(&fe.clip()) {
            involved_clips.push(fe.clip());
        }
    }
    for &c in &involved_clips {
        let obj = *scas.get(c).ok_or_else(|| {
            format!("clip index {c} out of range (pack has {} spline anims)", scas.len())
        })?;
        let ntt = hk.num_transform_tracks(obj);
        println!(
            "target clip #{c} @ data+0x{:x}, numTransformTracks={} (Havok {})",
            obj,
            ntt,
            hk.version()
        );
    }

    // ------- Phase 0: bake frame-edited `data` blobs, per affected clip ---
    // Each affected clip may have multiple frame edits (e.g., two translation
    // edits on different tracks or the same track at different frames).
    // We apply them in order to a per-clip `data` blob copy; the final copy
    // becomes the replacement bytes we splice into XML in Phase 1.
    let baked_by_clip: std::collections::HashMap<usize, Vec<u8>> = if frame_edits.is_empty() {
        std::collections::HashMap::new()
    } else {
        let mut m = std::collections::HashMap::new();
        for &c in &involved_clips {
            let obj = scas[c];
            if hk.num_blocks(obj) != 1 {
                return Err(format!(
                    "clip #{c} has numBlocks={}; frame editing only supports single-block clips",
                    hk.num_blocks(obj)
                ));
            }
            let dims = ClipDims {
                num_transform_tracks: hk.num_transform_tracks(obj) as usize,
                num_frames: hk.num_frames(obj) as usize,
                max_frames_per_block: hk.max_frames_per_block(obj) as usize,
                mask_and_quant_size: hk.mask_and_quantization_size(obj) as usize,
                frame_duration: hk.frame_duration(obj),
                block_duration: hk.block_duration(obj),
                block_inv_duration: hk.block_inv_duration(obj),
            };
            let cur = hk.spline_data(obj).to_vec();
            let mut acc = cur;
            for fe in frame_edits.iter().filter(|fe| fe.clip() == c) {
                acc = apply_one_frame_edit(&acc, &dims, fe)?;
            }
            let orig_len = hk.spline_data(obj).len();
            println!(
                "  frame-bake clip #{c}: data {} B -> {} B (Δ={:+})",
                orig_len,
                acc.len(),
                acc.len() as i64 - orig_len as i64
            );
            m.insert(c, acc);
        }
        m
    };

    // -------- Phase 1: AssetCc2 round-trip (if any XML-level edit is needed) --------
    let blob_after_xml_edits: Vec<u8> = if need_assetcc {
        let exe = find_assetcc(arg(args, "--assetcc"))?;
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
        // fixup order, so we translate fixup-order indices to XML declaration
        // indices by structural fingerprint.
        let hk_fps: Vec<_> = scas.iter().map(|&s| hk.spline_fingerprint(s)).collect();
        let hk_hashes: Vec<_> = scas.iter().map(|&s| ap0l_hash_bytes(hk.spline_data(s))).collect();
        let hk_to_xml =
            map_hk_to_xml_indices(&xml_text, &hk_fps, &hk_hashes).map_err(|e| e.to_string())?;
        let mut mutated = xml_text.clone();
        // Duration edit (single clip, given by --clip).
        if let Some(seconds) = set_duration {
            let target_clip = clip.expect("--clip guaranteed present when --set-duration is set");
            let xml_idx = hk_to_xml[target_clip];
            if xml_idx != target_clip {
                println!(
                    "note: fixup clip #{target_clip} = XML object #{xml_idx} (AssetCc2 reordered on decompile)",
                );
            }
            let (obj_start, obj_end) = find_nth_object(&mutated, CLASS_SPLINE, xml_idx)
                .ok_or_else(|| format!("clip #{target_clip} (XML idx {xml_idx}) not found in XML"))?;
            let repl = format!("{seconds:.6}");
            mutated = set_hkparam_in_object(&mutated, obj_start, obj_end, "duration", &repl)?;
            println!("set clip #{target_clip}.duration -> {repl}");
        }
        // Frame edits (one XML `data` rewrite per affected clip).
        for &c in &involved_clips {
            let Some(new_bytes) = baked_by_clip.get(&c) else { continue };
            let xml_idx = hk_to_xml[c];
            let (obj_start, obj_end) = find_nth_object(&mutated, CLASS_SPLINE, xml_idx)
                .ok_or_else(|| format!("clip #{c} (XML idx {xml_idx}) not found in XML"))?;
            mutated = replace_hkarray_bytes(&mutated, obj_start, obj_end, "data", new_bytes)?;
            println!(
                "rewrote clip #{c}.data with {} bytes of freshly baked spline stream",
                new_bytes.len()
            );
        }
        fs::write(&xml_mod, &mutated).map_err(|e| format!("write perturbed xml: {e}"))?;
        assetcc_compile(&exe, &xml_mod, &rt)?;
        let blob_out = fs::read(&rt).map_err(|e| format!("read perturbed hkx: {e}"))?;
        copy_artifacts(&[&hkx, &xml, &xml_mod, &rt], &keep)?;
        if keep.is_none() {
            let _ = fs::remove_dir_all(&dir);
        }
        println!(
            "AssetCc2 recompile: {} bytes (input was {}, Δ={:+})",
            blob_out.len(),
            ap.hk_blob().len(),
            blob_out.len() as i64 - ap.hk_blob().len() as i64
        );
        blob_out
    } else {
        ap.hk_blob().to_vec()
    };

    // -------- Phase 2: static-value edits (if any) directly in the blob --------
    let mut final_blob = blob_after_xml_edits;
    if !static_edits.is_empty() {
        let target_clip = clip.expect("--clip guaranteed present when static edits are set");
        // The blob after AssetCc2 round-trip may have shifted object offsets, so
        // re-parse the packfile to locate clip N and extract just the offsets we
        // need — then drop the HkPackfile borrow before mutating the blob.
        let target = {
            let hk2 = HkPackfile::parse(&final_blob).map_err(|e| e.to_string())?;
            let scas2 = hk2.objects_of_class(CLASS_SPLINE);
            let obj2 = *scas2.get(target_clip).ok_or_else(|| {
                format!("clip index {target_clip} vanished after AssetCc2 round-trip (post-rt count = {})", scas2.len())
            })?;
            let ntt2 = hk2.num_transform_tracks(obj2) as usize;
            let mq2 = hk2.mask_and_quantization_size(obj2) as usize;
            let nb2 = hk2.num_blocks(obj2);
            let (data_off, data_len) = hk2
                .spline_data_span(obj2)
                .ok_or("clip has no `data` hkArray local-fixup — malformed packfile")?;
            println!(
                "  static-edit target: {} track(s), maskAndQuantizationSize={}, numBlocks={}",
                ntt2, mq2, nb2
            );
            if nb2 != 1 {
                return Err(format!(
                    "clip has numBlocks={nb2}; static-value editing only supports single-block clips \
                     (entire retail corpus is single-block)"
                ));
            }
            (data_off, data_len, ntt2, mq2)
        };
        apply_static_edits_at(&mut final_blob, target.0, target.1, target.2, target.3, &static_edits)?;
    }

    // -------- Splice back into AP0L and write out --------
    let spliced = ap.splice_hk_blob(&final_blob);
    fs::write(&out, &spliced).map_err(|e| format!("write out: {e}"))?;
    println!(
        "final blob {} bytes (baseline {}, Δ={:+}); wrote {} ({} bytes)",
        final_blob.len(),
        ap.hk_blob().len(),
        final_blob.len() as i64 - ap.hk_blob().len() as i64,
        out,
        spliced.len(),
    );
    let _ = StaticEditError::ZeroQuaternion; // keep the type re-exported for downstream tools
    Ok(())
}

/// Apply one frame edit to a clip's `data` blob, returning the new blob.
fn apply_one_frame_edit(
    data: &[u8],
    dims: &ClipDims,
    fe: &FrameEdit,
) -> Result<Vec<u8>, String> {
    match *fe {
        FrameEdit::SetTranslation { clip, track, frame, xyz } => {
            let (new, old) = set_frame_translation(data, dims, track, frame, xyz)
                .map_err(|e| e.to_string())?;
            println!(
                "  clip#{clip} trk{track} frame{frame}: translation ({:.4},{:.4},{:.4}) -> ({:.4},{:.4},{:.4})",
                old[0], old[1], old[2], xyz[0], xyz[1], xyz[2]
            );
            Ok(new)
        }
        FrameEdit::SetRotation { clip, track, frame, quat } => {
            let (new, old) = set_frame_rotation(data, dims, track, frame, quat)
                .map_err(|e| e.to_string())?;
            println!(
                "  clip#{clip} trk{track} frame{frame}: rotation ({:.4},{:.4},{:.4},{:.4}) -> ({:.4},{:.4},{:.4},{:.4})",
                old[0], old[1], old[2], old[3], quat[0], quat[1], quat[2], quat[3]
            );
            Ok(new)
        }
        FrameEdit::SetScale { clip, track, frame, sxyz } => {
            let (new, old) = set_frame_scale(data, dims, track, frame, sxyz)
                .map_err(|e| e.to_string())?;
            println!(
                "  clip#{clip} trk{track} frame{frame}: scale ({:.4},{:.4},{:.4}) -> ({:.4},{:.4},{:.4})",
                old[0], old[1], old[2], sxyz[0], sxyz[1], sxyz[2]
            );
            Ok(new)
        }
        FrameEdit::OffsetTranslation { clip, track, frame, delta } => {
            let (new, old, new_val) =
                offset_frame_translation(data, dims, track, frame, delta).map_err(|e| e.to_string())?;
            println!(
                "  clip#{clip} trk{track} frame{frame}: translation ({:.4},{:.4},{:.4}) + \
                 ({:+.4},{:+.4},{:+.4}) = ({:.4},{:.4},{:.4})",
                old[0], old[1], old[2], delta[0], delta[1], delta[2], new_val[0], new_val[1], new_val[2]
            );
            Ok(new)
        }
    }
}

/// Rewrite an hkArray hkparam (both the `numelements` attribute and the
/// space-separated decimal-byte payload) inside a single `<hkobject>` scope.
/// Fails if the param is absent or ambiguous inside that scope.
fn replace_hkarray_bytes(
    xml: &str,
    start: usize,
    end: usize,
    param: &str,
    new_bytes: &[u8],
) -> Result<String, String> {
    let scope = &xml[start..end];
    // hkArray hkparams look like: <hkparam name="data" numelements="N"> ... </hkparam>
    let anchor = format!("<hkparam name=\"{param}\"");
    let anchor_rel = scope
        .find(&anchor)
        .ok_or_else(|| format!("hkparam name={param:?} not found in <hkobject>"))?;
    // Make sure it appears exactly once.
    let mut cursor = anchor_rel + anchor.len();
    if scope[cursor..].find(&anchor).is_some() {
        return Err(format!("hkparam name={param:?} occurs more than once in <hkobject>"));
    }
    // Find `numelements="..."`.
    let key = "numelements=\"";
    let ne_key = scope[anchor_rel..]
        .find(key)
        .ok_or_else(|| format!("hkparam name={param:?} has no numelements attribute"))?
        + anchor_rel
        + key.len();
    let ne_end_rel = scope[ne_key..]
        .find('"')
        .ok_or_else(|| format!("malformed numelements on {param:?}"))?;
    let ne_end = ne_key + ne_end_rel;
    // Payload = between the first '>' after the anchor and the matching '</hkparam>'.
    let open_gt = scope[anchor_rel..]
        .find('>')
        .ok_or_else(|| format!("hkparam name={param:?} opening tag never closes"))?
        + anchor_rel
        + 1;
    let close_rel = scope[open_gt..]
        .find("</hkparam>")
        .ok_or_else(|| format!("hkparam name={param:?} has no closing tag"))?;
    let close = open_gt + close_rel;
    // Build new payload — one byte per whitespace-separated decimal, wrapped
    // conservatively at 32 bytes per line for git-diff sanity. AssetCc2's
    // parser is whitespace-agnostic here.
    let mut payload = String::with_capacity(new_bytes.len() * 4 + new_bytes.len() / 32 + 8);
    payload.push('\n');
    for (i, b) in new_bytes.iter().enumerate() {
        if i > 0 {
            if i % 32 == 0 {
                payload.push('\n');
            } else {
                payload.push(' ');
            }
        }
        use std::fmt::Write;
        let _ = write!(payload, "{b}");
    }
    payload.push('\n');
    cursor = 0; // no more scope reuse — build out into abs offsets.
    let abs_ne_start = start + ne_key;
    let abs_ne_end = start + ne_end;
    let abs_payload_start = start + open_gt;
    let abs_payload_end = start + close;
    let mut out = String::with_capacity(xml.len() + payload.len());
    out.push_str(&xml[..abs_ne_start]);
    let _ = cursor;
    let ne_new = format!("{}", new_bytes.len());
    out.push_str(&ne_new);
    out.push_str(&xml[abs_ne_end..abs_payload_start]);
    out.push_str(&payload);
    out.push_str(&xml[abs_payload_end..]);
    Ok(out)
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

fn cmd_probe_clip(args: &[String]) -> Result<(), String> {
    let pack = args.get(1).ok_or("need <pack>")?;
    let clip: usize = parse_usize(&arg(args, "--clip").ok_or("need --clip <index>")?)?;
    let bytes = fs::read(pack).map_err(|e| format!("read pack: {e}"))?;
    let ap = Ap0lPack::parse(bytes).map_err(|e| e.to_string())?;
    let hk = HkPackfile::parse(ap.hk_blob()).map_err(|e| e.to_string())?;
    let scas = hk.objects_of_class(CLASS_SPLINE);
    let obj = *scas.get(clip).ok_or_else(|| {
        format!("clip index {clip} out of range (pack has {} spline anims)", scas.len())
    })?;
    let ntt = hk.num_transform_tracks(obj) as usize;
    let mq = hk.mask_and_quantization_size(obj) as usize;
    let nb = hk.num_blocks(obj);
    println!(
        "clip #{clip} @ data+0x{:x}: {} transform tracks, mask_and_quant_size={}, numBlocks={}",
        obj, ntt, mq, nb
    );
    if nb != 1 {
        return Err(format!("multi-block clip (numBlocks={nb}); walker requires 1"));
    }
    let (data_off, data_len) = hk
        .spline_data_span(obj)
        .ok_or("clip has no `data` hkArray")?;
    let data = &ap.hk_blob()[data_off..data_off + data_len];
    let layouts = walk_spline_track_layouts(data, ntt, mq).map_err(|e| e.to_string())?;
    let mut n_full_trn = 0;
    let mut n_static_rot = 0;
    let mut n_full_scl = 0;
    println!(
        "  {:>4}  ctrl trn_m rot_m scl_m  {:>10} {:>10} {:>10}   T-static  R-static  S-static",
        "trk", "trn_off", "rot_off", "scl_off"
    );
    for (i, l) in layouts.iter().enumerate() {
        let t_full = l.is_fully_static_vec3_trn();
        let r_stat = l.is_static_quat();
        let s_full = l.is_fully_static_vec3_scl();
        if t_full { n_full_trn += 1; }
        if r_stat { n_static_rot += 1; }
        if s_full { n_full_scl += 1; }
        println!(
            "  {:>4}  0x{:02x} 0x{:02x} 0x{:02x} 0x{:02x}  {:>10} {:>10} {:>10}   {}         {}         {}",
            i, l.ctrl, l.trn_mask, l.rot_mask, l.scl_mask,
            l.trn_static_off, l.rot_payload_off, l.scl_static_off,
            if t_full { "yes" } else { " no" },
            if r_stat { "yes" } else { " no" },
            if s_full { "yes" } else { " no" },
        );
    }
    println!(
        "\nsummary: {}/{} tracks fully-static translation, {}/{} static rotation, {}/{} fully-static scale",
        n_full_trn, ntt, n_static_rot, ntt, n_full_scl, ntt
    );
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
        "probe-clip" => cmd_probe_clip(rest),
        _ => usage(),
    };
    if let Err(e) = r {
        eprintln!("error: {e}");
        exit(1);
    }
}
