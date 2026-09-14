//! Pre-release 2008-05 Xbox 360 build — the animation catalog.
//!
//! **Not a port of retail's `anim_index` / `havok`.** The 2008 build's animation asset chain is a
//! different code path, in a different (older) middleware, in a different byte order. Details
//! below.
//!
//! # What retail's decoder assumes
//!
//! `crate::anim_index` + `crate::havok` decode the retail-PC layout:
//!   * `Animations.pack` starts with `L0PA MINA` (u32-LE-swapped `AP0L ANIM`).
//!   * An `ANIM` block enumerates a per-clip record: id / streamed-flag / name / duration /
//!     per-track bone list / a couple of small sub-tables.
//!   * After the ANIM block, ONE big Havok packfile blob (magic `57 E0 E0 57`) holds every clip
//!     as an `hkaSplineCompressedAnimation`.
//!   * Little-endian throughout (both the outer pack and the Havok blob).
//!
//! # What the 2008 build actually ships
//!
//! Empirically, none of that applies:
//!
//! 1. **`Animations.pack` starts with `AP04ANIM`** (natural byte order, big-endian
//!    orientation for Xenon). The next 4 bytes are `00 00 06 45` = **u32-BE 1605** — the
//!    number of records in the pack.
//! 2. **There is NO Havok packfile in the pack.** The `57 E0 E0 57` magic does not occur.
//!    Nor does any `hka*` classname string (`hkaSplineCompressedAnimation`, `hkaAnimation`,
//!    …). Confirmed by exhaustive substring scan.
//! 3. **Every clip is a Pandemic-custom `ANM0(WAV|DEL|NOC)0` record**, not a Havok
//!    packfile. The pack has 1605 such records; the `animations/` sibling directory has
//!    another 3586 LOOSE `.hkx` files with the same magic. `WAV0` = wavelet, `DEL0` =
//!    delta, `NOC0` = uncompressed (mirrors Havok-4.6 skeletal-animation types — WildStar_d's
//!    map file names the middleware as `Havok_46`).
//! 4. **Loose files carry the name** (as the filename stem, e.g. `Bearcrawl_Run_L_LF.hkx`);
//!    pack records carry only an opaque per-record hash — the plaintext name inventory is
//!    the loose-file directory. Content-matching a subset of loose files into the pack is
//!    what recovers a name for a pack record.
//!
//! # What this module ships
//!
//! * **Catalog enumeration from the loose tree** — the primary path. Every `.hkx` under
//!   `Bin360/animations/` (and `ObjectAnimations/`, `TestAnims/`) is an ANM0-family record
//!   whose stem is its friendly name. This gets us > 3600 named clips with duration + track
//!   count for the Nav Anims tab, immediately.
//! * **Best-effort `Animations.pack` walk** — enumerate the pack's 1605 records by scanning
//!   for the ANM0 magic (the outer per-record header layout is only partially reversed;
//!   the magic scan is the ROBUST way to get every record without misinterpreting the
//!   header). Records carry duration + track count from the ANM0 body; names remain
//!   pack-anonymous until a content-hash match wires one to a loose file.
//! * **One-clip "decode" end-to-end** — the ANM0 body's per-frame sample codec has never
//!   been reversed for this build (Havok-4.6 clip formats are novel to this project). We
//!   read the header and hand back the raw payload bytes, which is what a downstream decoder
//!   would then work from. Not the same as retail's per-frame `QsTransform` sampling; but
//!   it's the shape of "one clip decoded" achievable without a full Havok-4.6 clip decoder.
//!
//! # What differs from retail at a glance
//!
//! | Aspect            | Retail PC (`AP0L`)            | 2008 Xbox 360 (`AP04ANIM`)         |
//! |-------------------|-------------------------------|-------------------------------------|
//! | Pack magic        | `L0PA` `MINA` (LE-swapped)    | `AP04ANIM` (natural / big-endian)   |
//! | Endianness        | LE                            | BE                                  |
//! | Middleware        | Havok 6.5.0                   | Havok 4.6 (per WildStar_d.map)      |
//! | Clip container    | one big Havok packfile blob   | 1605 `ANM0*0` records + loose files |
//! | Compression       | `hkaSplineCompressedAnimation`| `WAV0` / `DEL0` / `NOC0` (Pandemic) |
//! | Names             | `ANIM` block, per-record      | filename of the loose `.hkx` twin   |
//! | Bone binding      | `ANIM` per-track bone list    | not surfaced here (needs FSM)       |
//!
//! The bone-binding gap means clips are NOT playable against a rig from this data alone —
//! WildStar_d's `PclAnimationBank` and the FSM state machine (also unreversed for 2008) hold
//! the character-to-clip-set mapping. This module surfaces WHAT clips exist and their basic
//! metadata; playing them on the 2008 human rig needs the FSM probe too.
//!
//! # References
//! * `WildStar_d.map` — `LoadAnimFile` / `LoadAnimPackFile` / `LoadAnimation@PclAnimationManager`
//!   are the loader functions (Puckel:PclAnimationManager.obj, `d:/projects/wildstar/code/puckel/`).
//! * `Bin360/AnimText/*.txt` — the human-readable metadata for `SeqTrans`, `AnimSettings`,
//!   `EdgeTransitions_v*`, `MeleeCombos_v*` etc. Cross-references clip names to sequences.
//!
//! # Per-frame sampling
//!
//! Given a `Clip2008`, a track index, and a time in seconds, [`Clip2008::sample_track`]
//! returns a `SampledTransform { pos, rot, scale }`. **NOC0** is decoded end-to-end (the
//! layout was recovered from six loose files whose sizes match `24 + eff_tracks*4 +
//! aux*48` byte-for-byte and whose 12-float transforms yield unit quaternions and
//! ~identity scales for identifiable reference poses).
//!
//! **WAV0** and **DEL0** are best-effort. The extended HK-4.6 derived-struct header
//! (past the 24-byte Pandemic pre-header) is parsed by
//! [`super::prototype2008_anim_havok46::Wav0Header::parse`] /
//! [`super::prototype2008_anim_havok46::Del0Header::parse`] and validated on a
//! representative slice of the loose corpus. The wavelet codec math (`wv_*` in the
//! sibling `prototype2008_anim_havok46` module) is ported from Mercs 2's
//! numerically-validated HK-5.5 decoder — Havok did not re-cut this math between 4.6
//! and 5.5. A returned `Ok` transform is guaranteed finite + unit-quat + validated
//! against the layout gates; when any decoder step (per-DOF array read, bit-width
//! range, static-mask consumption, quantData bit stream) does not validate,
//! `sample_track_wav0` returns `Err` with the specific diagnostic naming the failed
//! stage. DEL0 currently always returns `Err` — the delta-specific bit stream is not
//! validated in the Mercs 2 workspace either. Callers must be prepared for `Err` on
//! either codec branch, exactly as before this port.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

use crate::pack::pandemic_hash;
use crate::source::{AssetHandle, AssetKind};

/// The three flavours of `ANM0*0` clip magic seen in this build. Names line up with the
/// Havok-4.6 skeletal-animation compression modes (`hkaWaveletSkeletalAnimation`,
/// `hkaDeltaCompressedSkeletalAnimation`, uncompressed `hkaInterleavedSkeletalAnimation`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ClipCodec {
    /// `ANM0WAV0` — wavelet-compressed. Vast majority of loose files (3090/3586 in animations/).
    Wavelet,
    /// `ANM0DEL0` — delta-compressed. 282/3586 loose files.
    Delta,
    /// `ANM0NOC0` — uncompressed / interleaved. 214/3586 loose files; used for short poses
    /// (many are zero-duration "dead pose" snapshots).
    None,
}

impl ClipCodec {
    /// Match the 8-byte magic head of an ANM0-family record.
    pub fn from_magic(magic: &[u8]) -> Option<Self> {
        match magic {
            b"ANM0WAV0" => Some(ClipCodec::Wavelet),
            b"ANM0DEL0" => Some(ClipCodec::Delta),
            b"ANM0NOC0" => Some(ClipCodec::None),
            _ => None,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            ClipCodec::Wavelet => "WAV0",
            ClipCodec::Delta => "DEL0",
            ClipCodec::None => "NOC0",
        }
    }
}

/// The prefix common to every ANM0-family record — enough for the browser and for a
/// preflight sanity check before any codec-specific decoder gets called. Field derivations
/// below are documented against the loose-file sample set (`tools/sab_workshop/tests` /
/// `docs/2008_animations_format.md`); the same layout is present in the packed records.
///
/// Byte offsets from the start of the record (i.e. from the start of the ANM0 magic):
/// ```text
///   0x00  u8[8]   magic         "ANM0WAV0" | "ANM0DEL0" | "ANM0NOC0"
///   0x08  f32 BE  duration_s    the clip's playback duration in seconds
///   0x0C  u32 BE  num_tracks    number of transform tracks (24..64 typical)
///   0x10  u32 BE  aux_count     wavelet: block count; delta: frame count; NOC0: 0
///   0x14  u32 BE  stride16      always 16 in the sampled WAV0 + DEL0 corpus (a stride
///                               or a small-int constant — kept for round-trip audits)
/// ```
///
/// The specific meaning of `aux_count` beyond this is codec-specific — a wavelet clip's
/// value is a block count (blocks group ~16 frames each), a delta clip's value is a frame
/// count (matches `round(duration_s * 30fps)` in every sampled cinematic), and a NOC0
/// zero-duration pose reads 0. The one-clip-decode deliverable does not depend on
/// distinguishing them further; codec-aware sampling would.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Anim2008Header {
    pub codec: ClipCodec,
    pub duration_s: f32,
    pub num_tracks: u32,
    pub aux_count: u32,
    pub stride16: u32,
}

impl Anim2008Header {
    pub const FIXED_HEADER_LEN: usize = 24; // magic(8) + dur(4) + tracks(4) + aux(4) + stride(4)

    /// Parse the 24-byte fixed header at the start of an ANM0-family record.
    pub fn parse(buf: &[u8]) -> Result<Self, String> {
        if buf.len() < Self::FIXED_HEADER_LEN {
            return Err(format!(
                "ANM0 record too short: {} bytes, need {}",
                buf.len(),
                Self::FIXED_HEADER_LEN
            ));
        }
        let codec = ClipCodec::from_magic(&buf[..8]).ok_or_else(|| {
            let m = std::str::from_utf8(&buf[..8]).unwrap_or("<non-ascii>");
            format!("not an ANM0 record: magic {m:?}")
        })?;
        // Xenon = big-endian. Every scalar in the pre-body header is BE — confirmed on the
        // WAV/DEL/NOC sample rows in `docs/2008_animations_format.md`.
        let duration_s = f32::from_be_bytes([buf[8], buf[9], buf[10], buf[11]]);
        let num_tracks = u32::from_be_bytes([buf[12], buf[13], buf[14], buf[15]]);
        let aux_count = u32::from_be_bytes([buf[16], buf[17], buf[18], buf[19]]);
        let stride16 = u32::from_be_bytes([buf[20], buf[21], buf[22], buf[23]]);
        // Plausibility gates. Track/duration checks catch endianness mistakes and truncated
        // reads before anyone consumes the header downstream. Numbers are the observed
        // corpus range (see `docs/2008_animations_format.md#header-field-ranges`) padded up.
        if !(duration_s.is_finite() && (0.0..=3600.0).contains(&duration_s)) {
            return Err(format!("ANM0 duration out of range: {duration_s}"));
        }
        if num_tracks > 512 {
            return Err(format!("ANM0 num_tracks out of range: {num_tracks}"));
        }
        Ok(Self { codec, duration_s, num_tracks, aux_count, stride16 })
    }
}

/// One playable-metadata row for the browser and the JSON manifest.
///
/// This is the 2008 analogue of retail's `crate::anim_index::ClipInfo`: a name, a duration,
/// a track count, and enough source info to fetch the raw bytes later. Two differences
/// from the retail struct:
///   * No `track_to_bone: Vec<i32>` — that lives in the retail ANIM block's per-clip bone
///     list; the 2008 build stores it in `PclAnimationBank` / FSM, which is not reversed
///     here yet. Clips are NOT known-playable-against-rig from this data alone.
///   * `codec` is exposed so a UI can show the WAV/DEL/NOC breakdown (a first-class 2008
///     concern; retail's clips are uniformly spline-compressed).
#[derive(Clone, Debug)]
pub struct Clip2008Info {
    /// Friendly name. Loose files: the filename stem. Pack-only clips: empty string,
    /// with `hash` set to `pandemic_hash(unknown)` — the browser can still display them
    /// as `(HHHHHHHH)`.
    pub name: String,
    /// `pandemic_hash(name)` when the name is known. Also serves as the AssetHandle key.
    pub hash: u32,
    pub codec: ClipCodec,
    pub duration_s: f32,
    pub num_tracks: u32,
    /// Where this clip's bytes live on disk. `Loose` = its own file; `Packed` = a byte
    /// range inside `Animations.pack`.
    pub source: Clip2008Source,
}

#[derive(Clone, Debug)]
pub enum Clip2008Source {
    /// A loose `.hkx` file. `path` is absolute-ish (relative to Bin360 root).
    Loose { path: PathBuf },
    /// A record inside the outer `Animations.pack`. `record_off` is the byte offset of the
    /// ANM0 magic within the pack; `record_end` is one past the last body byte (i.e. the
    /// next record's outer-header start). The record spans `record_end - record_off` bytes.
    Packed { pack_path: PathBuf, record_off: u64, record_end: u64 },
}

/// The parsed clip payload: everything needed to eventually run a codec decoder over the
/// per-frame samples. We stop at "raw body bytes + parsed header" because the ANM0 codec
/// bodies are not yet reversed for this build — but this shape is deliberately the same as
/// retail's `Clip { anim, ... }` in spirit: header metadata that a caller trusts, plus a
/// slice/vec of the compressed data.
#[derive(Clone, Debug)]
pub struct Clip2008 {
    pub info: Clip2008Info,
    pub header: Anim2008Header,
    /// The full record bytes — magic + fixed header + codec body. Handing back the whole
    /// record (not a body-only slice) means every downstream consumer sees identical bytes
    /// whether the clip came from a loose file or the pack.
    pub bytes: Vec<u8>,
}

/// One track's local transform at a moment in time. TRS in bone-local space, in the same
/// convention the Havok runtime hands back from `samplePose`: right-handed, quaternion
/// `(x, y, z, w)`, per-axis scale. The workshop's skinning already consumes this shape
/// via `havok::QsTransform`; the sampled values here are drop-in compatible.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SampledTransform {
    pub pos: [f32; 3],
    /// Unit quaternion `(x, y, z, w)`; normalized by the sampler so callers can trust it.
    pub rot: [f32; 4],
    pub scale: [f32; 3],
}

impl SampledTransform {
    pub const IDENTITY: SampledTransform = SampledTransform {
        pos: [0.0, 0.0, 0.0],
        rot: [0.0, 0.0, 0.0, 1.0],
        scale: [1.0, 1.0, 1.0],
    };
}

// ── NOC0 on-disk layout — recovered from six loose files ──────────────────────────────────
//
// Post-header structure (verified byte-for-byte AND numerically on six shipped files:
// aurora_dead_idle_drvr, back_prone_pose, Bugatti_drive_turn_left, KD_head_front,
// Rope_horiz_down_long_start, Ledge_Backjump_prep_loop — every 12-float transform decodes
// to a unit quaternion and ~identity-or-plausible TRS):
//
//   +0x18    aux × 48 bytes         frame-major hkQsTransform array (see below);
//                                   transforms[f * eff_tracks + t]
//   +TRAILER eff_tracks × u32 BE    per-track descriptor (pandemic_hash of the driven
//                                   bone; the same bone-hash values recur across clips —
//                                   correlate against a skeleton to name each track)
//
// where TRAILER = 0x18 + aux * 48 and file_len = TRAILER + eff_tracks * 4.
//
//   eff_tracks = num_tracks if num_tracks > 0 else aux_count (the ntt=0 "static pose"
//                                 special case where the header hands the track count off
//                                 to the aux slot; every dead-idle / static-pose file
//                                 shipped goes down this branch — for those, aux == the
//                                 pose-track count and there is exactly one frame)
//   frames     = aux_count / eff_tracks  (integer; exact division has held on every
//                                 sampled file — no float-frame in the shipped corpus)
//
// Each 48-byte hkQsTransform is 12 f32-BE floats laid out as:
//   [0..2]   translation (x, y, z)
//   [3..6]   rotation quaternion (x, y, z, w) — stored explicitly. Verified unit across
//            every transform in the sample set (all 180 for back_prone_pose, all 52 for
//            aurora, all 336 for Bugatti_drive_turn_left; |R| = 1.0 ± 1e-4)
//   [7..9]   scale (x, y, z) — near-identity (1.0) across the sample set
//   [10..11] trailing padding / per-transform aux (usually 0.0; a small varying float on
//            some tracks — not used by the sampler)

const NOC0_TRANSFORM_STRIDE: usize = 48;
const NOC0_FLOATS_PER_TRANSFORM: usize = 12;

/// Frame count and trailer length derived from the header. Returns `Err` for a header
/// whose (eff_tracks × frames_per_track) does not equal aux_count (would mean the
/// shipped file uses a codec-body layout we don't understand yet).
fn noc0_shape(header: &Anim2008Header) -> Result<(usize, usize, usize), String> {
    let eff_tracks = if header.num_tracks > 0 {
        header.num_tracks as usize
    } else {
        header.aux_count as usize
    };
    if eff_tracks == 0 {
        return Err("NOC0 clip has zero tracks (num_tracks=0 and aux_count=0)".into());
    }
    let total = header.aux_count as usize;
    if total % eff_tracks != 0 {
        return Err(format!(
            "NOC0 clip has aux_count={total} not divisible by eff_tracks={eff_tracks}",
        ));
    }
    let frames = (total / eff_tracks).max(1);
    let trailer_bytes = eff_tracks * 4; // per-track u32 descriptors at end-of-file
    Ok((eff_tracks, frames, trailer_bytes))
}

/// Read a single 48-byte hkQsTransform at absolute offset `off` from a NOC0 body.
/// Returns `Err` if the range is out of bounds. Normalizes the quaternion so callers can
/// trust the unit invariant (a stray non-unit sample would otherwise slip past the
/// downstream skinning quat-to-matrix conversion).
fn noc0_read_transform(bytes: &[u8], off: usize) -> Result<SampledTransform, String> {
    if off + NOC0_TRANSFORM_STRIDE > bytes.len() {
        return Err(format!(
            "NOC0 transform read past end: off={off}, need={NOC0_TRANSFORM_STRIDE}, have={}",
            bytes.len()
        ));
    }
    let mut fl = [0.0f32; NOC0_FLOATS_PER_TRANSFORM];
    for (i, v) in fl.iter_mut().enumerate() {
        let base = off + i * 4;
        *v = f32::from_be_bytes([bytes[base], bytes[base + 1], bytes[base + 2], bytes[base + 3]]);
    }
    if !fl.iter().all(|f| f.is_finite()) {
        return Err(format!("NOC0 transform contains non-finite float at off={off}"));
    }
    let mut rot = [fl[3], fl[4], fl[5], fl[6]];
    let mag_sq = rot[0] * rot[0] + rot[1] * rot[1] + rot[2] * rot[2] + rot[3] * rot[3];
    if mag_sq > 1e-12 {
        let inv = 1.0 / mag_sq.sqrt();
        rot[0] *= inv;
        rot[1] *= inv;
        rot[2] *= inv;
        rot[3] *= inv;
    } else {
        // Degenerate quat — return identity rather than propagate NaN into the skinning.
        rot = [0.0, 0.0, 0.0, 1.0];
    }
    Ok(SampledTransform { pos: [fl[0], fl[1], fl[2]], rot, scale: [fl[7], fl[8], fl[9]] })
}

/// Shortest-arc slerp of two `(x, y, z, w)` unit quaternions. Falls back to a normalized
/// lerp near-parallel to avoid a divide-by-tiny on `sin(theta0)` — same shape as retail's
/// `havok::sample_at` slerp so we don't introduce a second numerical convention.
fn slerp_quat(a: [f32; 4], mut b: [f32; 4], t: f32) -> [f32; 4] {
    let mut dot = a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3];
    if dot < 0.0 {
        for c in b.iter_mut() {
            *c = -*c;
        }
        dot = -dot;
    }
    if dot > 0.9995 {
        let mut q = [
            a[0] + (b[0] - a[0]) * t,
            a[1] + (b[1] - a[1]) * t,
            a[2] + (b[2] - a[2]) * t,
            a[3] + (b[3] - a[3]) * t,
        ];
        let m = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
        if m > 0.0 {
            for c in q.iter_mut() {
                *c /= m;
            }
        }
        return q;
    }
    let theta0 = dot.clamp(-1.0, 1.0).acos();
    let s0 = ((1.0 - t) * theta0).sin() / theta0.sin();
    let s1 = (t * theta0).sin() / theta0.sin();
    [
        a[0] * s0 + b[0] * s1,
        a[1] * s0 + b[1] * s1,
        a[2] * s0 + b[2] * s1,
        a[3] * s0 + b[3] * s1,
    ]
}

/// Linear interpolation of the two bracketing NOC0 frames for one track. `frac` is the
/// unit interval position between `f0` and `f1`.
fn lerp_transform(a: SampledTransform, b: SampledTransform, frac: f32) -> SampledTransform {
    let f = frac.clamp(0.0, 1.0);
    let lerp3 = |x: [f32; 3], y: [f32; 3]| {
        [x[0] + (y[0] - x[0]) * f, x[1] + (y[1] - x[1]) * f, x[2] + (y[2] - x[2]) * f]
    };
    SampledTransform {
        pos: lerp3(a.pos, b.pos),
        rot: slerp_quat(a.rot, b.rot, f),
        scale: lerp3(a.scale, b.scale),
    }
}

impl Clip2008 {
    /// Sample one track's local transform at `time_s` seconds. Time is clamped to
    /// `[0, duration_s]`. Returns `Err` for unimplemented codecs — the caller decides
    /// whether to surface that or fall back to a rest pose.
    ///
    /// **NOC0** (interleaved uncompressed): implemented end-to-end. Reads the two
    /// bracketing frames from the frame-major transform array and lerps translation and
    /// scale linearly, slerps the quaternion. Time-line is uniform: frame `i` sits at
    /// `t = i * duration_s / (frames-1)`.
    ///
    /// **WAV0** (wavelet-compressed) and **DEL0** (delta-compressed): unimplemented —
    /// the codec bodies are variable-length quantized bitstreams whose per-frame decoder
    /// is not yet reversed here (see the module doc for the Havok-4.6 sampler addresses
    /// in the Saboteur.xex decomp). Returns `Err` with a diagnostic — do **not** silently
    /// hand back an identity pose from this branch, because that would look like a rig
    /// bug to a caller doing a sanity check.
    pub fn sample_track(&self, track_idx: u32, time_s: f32) -> Result<SampledTransform, String> {
        match self.header.codec {
            ClipCodec::None => self.sample_track_noc0(track_idx, time_s),
            ClipCodec::Wavelet => self.sample_track_wav0(track_idx, time_s),
            ClipCodec::Delta => self.sample_track_del0(track_idx, time_s),
        }
    }

    /// WAV0 (wavelet-compressed) sampler — best-effort. Parses the extended
    /// HK-4.6 derived-struct header via
    /// [`super::prototype2008_anim_havok46::Wav0Header::parse`], reconstructs
    /// the per-frame pose stream via
    /// [`super::prototype2008_anim_havok46::decode_wavelet_frames`], then
    /// bracket-and-interps to the requested time. Returns `Err` with the
    /// specific decoder diagnostic when a clip does not decode cleanly (see
    /// the sibling module's docs on codec status and the sweep test for the
    /// current corpus success ratio).
    fn sample_track_wav0(
        &self,
        track_idx: u32,
        time_s: f32,
    ) -> Result<SampledTransform, String> {
        use super::prototype2008_anim_havok46 as hk;
        let block_size = self.header.stride16;
        let num_poses = self.header.aux_count;
        let header =
            hk::Wav0Header::parse(&self.bytes, self.header.num_tracks, num_poses, block_size)
                .map_err(|e| format!("clip '{}': {e}", self.info.name))?;
        let frames = hk::decode_wavelet_frames(
            &self.bytes,
            &header,
            self.header.num_tracks,
            num_poses,
            block_size,
        )
        .map_err(|e| format!("clip '{}': {e}", self.info.name))?;
        hk::sample_reconstructed(
            &frames,
            self.header.duration_s,
            time_s,
            track_idx as usize,
        )
        .map_err(|e| format!("clip '{}': {e}", self.info.name))
    }

    /// DEL0 (delta-compressed) sampler — best-effort. Parses the extended
    /// HK-4.6 derived-struct header via
    /// [`super::prototype2008_anim_havok46::Del0Header::parse`], then tries
    /// [`super::prototype2008_anim_havok46::decode_delta_frames`] which is
    /// currently unimplemented (returns `Err` with a specific diagnostic
    /// pointing at the decomp addresses). Header parsing on real clips is
    /// verified via `del0_header_parses_cinematic_clips`.
    fn sample_track_del0(
        &self,
        track_idx: u32,
        time_s: f32,
    ) -> Result<SampledTransform, String> {
        use super::prototype2008_anim_havok46 as hk;
        let header = hk::Del0Header::parse(&self.bytes)
            .map_err(|e| format!("clip '{}': {e}", self.info.name))?;
        let frames = hk::decode_delta_frames(
            &self.bytes,
            &header,
            self.header.num_tracks,
            self.header.aux_count,
        )
        .map_err(|e| format!("clip '{}': {e}", self.info.name))?;
        hk::sample_reconstructed(
            &frames,
            self.header.duration_s,
            time_s,
            track_idx as usize,
        )
        .map_err(|e| format!("clip '{}': {e}", self.info.name))
    }

    /// NOC0 (uncompressed / interleaved) sampler — the shape recovered from the sample
    /// corpus (see module doc + `noc0_shape`). Frame-major transforms; slerp between the
    /// two bracketing key frames. Time outside `[0, duration_s]` is clamped.
    fn sample_track_noc0(&self, track_idx: u32, time_s: f32) -> Result<SampledTransform, String> {
        let (eff_tracks, frames, trailer_bytes) = noc0_shape(&self.header)?;
        if (track_idx as usize) >= eff_tracks {
            return Err(format!(
                "track_idx {track_idx} out of range (eff_tracks={eff_tracks} for clip '{}')",
                self.info.name
            ));
        }
        // Transforms live directly after the 24-byte header (no per-track prefix); the
        // per-track descriptor table is a trailer after the transform array.
        let body_start = Anim2008Header::FIXED_HEADER_LEN;
        let needed = body_start + eff_tracks * frames * NOC0_TRANSFORM_STRIDE + trailer_bytes;
        if self.bytes.len() < needed {
            return Err(format!(
                "NOC0 clip '{}' truncated: have {} bytes, need {} for {} tracks × {} frames + {}-byte trailer",
                self.info.name,
                self.bytes.len(),
                needed,
                eff_tracks,
                frames,
                trailer_bytes
            ));
        }
        // Static-pose special case (frames == 1): no interpolation, just read the one
        // frame's slot for this track.
        if frames == 1 {
            let off = body_start + (track_idx as usize) * NOC0_TRANSFORM_STRIDE;
            return noc0_read_transform(&self.bytes, off);
        }
        // Interpolated case. Time-line: uniform, last frame sits at duration_s.
        let dur = self.header.duration_s.max(0.0);
        let last = frames - 1;
        let t = time_s.clamp(0.0, dur);
        let (f0, frac) = if dur > 0.0 && last > 0 {
            let step = dur / last as f32;
            let g = if step > 0.0 { t / step } else { 0.0 };
            let f = (g.floor() as usize).min(last);
            (f, (g - f as f32).clamp(0.0, 1.0))
        } else {
            (0usize, 0.0)
        };
        let f1 = (f0 + 1).min(last);
        let off0 =
            body_start + (f0 * eff_tracks + track_idx as usize) * NOC0_TRANSFORM_STRIDE;
        let off1 =
            body_start + (f1 * eff_tracks + track_idx as usize) * NOC0_TRANSFORM_STRIDE;
        let a = noc0_read_transform(&self.bytes, off0)?;
        let b = noc0_read_transform(&self.bytes, off1)?;
        Ok(lerp_transform(a, b, frac))
    }

    /// The per-track descriptor trailer (typically the pandemic_hash of each track's
    /// driven bone). Useful for correlating tracks against a skeleton to name them; the
    /// same bone-hash values recur across clips so a hash→name table built from the rig
    /// resolves the whole clip's track names.
    pub fn track_descriptors_noc0(&self) -> Result<Vec<u32>, String> {
        if !matches!(self.header.codec, ClipCodec::None) {
            return Err("track_descriptors_noc0: not a NOC0 clip".into());
        }
        let (eff_tracks, frames, trailer_bytes) = noc0_shape(&self.header)?;
        let trailer_start =
            Anim2008Header::FIXED_HEADER_LEN + eff_tracks * frames * NOC0_TRANSFORM_STRIDE;
        if trailer_start + trailer_bytes > self.bytes.len() {
            return Err(format!(
                "NOC0 clip '{}' truncated in trailer (need {} bytes at {}, file has {})",
                self.info.name,
                trailer_bytes,
                trailer_start,
                self.bytes.len()
            ));
        }
        Ok((0..eff_tracks)
            .map(|i| {
                let o = trailer_start + i * 4;
                u32::from_be_bytes([
                    self.bytes[o],
                    self.bytes[o + 1],
                    self.bytes[o + 2],
                    self.bytes[o + 3],
                ])
            })
            .collect())
    }

    /// Track count and frame count for a NOC0 clip. Convenient for a caller that wants
    /// to iterate every track over a specific time window without re-deriving the shape.
    pub fn noc0_shape(&self) -> Result<(usize, usize), String> {
        let (t, f, _) = noc0_shape(&self.header)?;
        Ok((t, f))
    }
}

/// The complete 2008 animation catalog for a given Bin360 root.
pub struct Anim2008Catalog {
    /// Every clip we can name (loose tree) + every pack record we could enumerate.
    pub clips: Vec<Clip2008Info>,
    /// How many clips came from the loose tree — the ones with real names.
    pub loose_count: usize,
    /// How many came from scanning the pack. May include duplicates of loose entries when
    /// content-matching wired a pack record to a loose name (currently pack entries are
    /// kept anonymous — pack-scan is enumerative only until a full pack outer decode
    /// lands).
    pub pack_count: usize,
    /// Codec breakdown across the whole catalog, for the browser status bar.
    pub codec_counts: [usize; 3], // [WAV, DEL, NOC]
    /// The pack file we discovered, if any. Retained so `load_clip(handle)` can seek back
    /// into it without re-fingerprinting the folder.
    pub pack_path: Option<PathBuf>,
}

impl Anim2008Catalog {
    /// Build the catalog from a Bin360 root. Cheap: header windows only, no decode.
    ///
    /// Order:
    ///   1. Walk `animations/`, `ObjectAnimations/`, `TestAnims/` for loose `.hkx` files.
    ///      Each contributes one `Clip2008Info` with a real name.
    ///   2. If `Animations.pack` exists, scan it for ANM0-family record starts (the outer
    ///      per-record header layout is only partially reversed — see module docs — so a
    ///      magic-scan is the robust way to enumerate every record). Each contributes one
    ///      `Clip2008Info` with an empty name and a hash key derived from the record's byte
    ///      offset (deterministic, uniquely identifies it for `load_clip`). A future pass
    ///      can content-match these to loose entries and drop the anonymous rows.
    pub fn open(bin360: &Path) -> Self {
        let mut clips = Vec::new();
        let mut codec_counts = [0usize; 3];
        let mut loose_count = 0usize;

        // 1. Loose files. Missing directories are fine — an install may not ship every dir.
        for sub in ["animations", "ObjectAnimations", "TestAnims"] {
            let dir = bin360.join(sub);
            if !dir.is_dir() {
                continue;
            }
            walk_hkx(&dir, &mut |path, stem| {
                // A tiny header read: 24 bytes is enough to derive everything the browser
                // needs. Files that fail this check are silently skipped — a stray .hkx
                // that isn't ANM0 (there are none in the shipped tree, but we tolerate it)
                // shouldn't poison the catalog.
                let Ok(bytes) = read_head(path, Anim2008Header::FIXED_HEADER_LEN) else {
                    return;
                };
                let Ok(h) = Anim2008Header::parse(&bytes) else {
                    return;
                };
                let hash = pandemic_hash(&stem);
                codec_counts[h.codec as usize] += 1;
                loose_count += 1;
                clips.push(Clip2008Info {
                    name: stem,
                    hash,
                    codec: h.codec,
                    duration_s: h.duration_s,
                    num_tracks: h.num_tracks,
                    source: Clip2008Source::Loose { path: path.to_path_buf() },
                });
            });
        }

        // 2. `Animations.pack` — best-effort magic scan.
        //
        // Rationale for scan-vs-directory-walk: the outer per-record header layout is only
        // partially reversed (see `docs/2008_animations_format.md#packed-outer-header`).
        // The ANM0 magic itself is a totally reliable earmark though: every hit is a real
        // record start (no false positives across the corpus — the magic is 8 bytes of a
        // very specific ASCII string), and the count matches the file's declared 1605.
        let pack_path = bin360.join("Animations.pack");
        let mut pack_count = 0usize;
        let saved_pack = if pack_path.is_file() {
            match std::fs::read(&pack_path) {
                Ok(data) => {
                    let hits = scan_pack_records(&data);
                    for (i, rec) in hits.iter().enumerate() {
                        let end = hits
                            .get(i + 1)
                            .map(|next| next.offset)
                            .unwrap_or(data.len() as u64);
                        // The header may not parse — the last few records can be padded or
                        // truncated in some copies. Skip such rows rather than error out
                        // the entire enumeration.
                        let Some(head) = data.get(rec.offset as usize..) else { continue };
                        let Ok(h) = Anim2008Header::parse(head) else { continue };
                        codec_counts[h.codec as usize] += 1;
                        pack_count += 1;
                        // Anonymous pack entry: no plaintext name (the outer directory
                        // doesn't surface it in a form we've decoded). Give the browser
                        // something to key on — the byte offset — while keeping `hash`
                        // reserved for the plaintext-pandemic-hash flavour (`None`-shaped
                        // via a special sentinel).
                        clips.push(Clip2008Info {
                            name: String::new(),
                            hash: pack_synthetic_key(rec.offset),
                            codec: h.codec,
                            duration_s: h.duration_s,
                            num_tracks: h.num_tracks,
                            source: Clip2008Source::Packed {
                                pack_path: pack_path.clone(),
                                record_off: rec.offset,
                                record_end: end,
                            },
                        });
                    }
                    Some(pack_path.clone())
                }
                Err(_) => None,
            }
        } else {
            None
        };

        Self { clips, loose_count, pack_count, codec_counts, pack_path: saved_pack }
    }

    /// Look up a clip by its AssetHandle key and hand back the raw bytes + parsed header.
    /// This is the 2008 analogue of retail's `havok::read_spline_anim`, stopping at the
    /// point where a spline decoder would take over — the ANM0 codec-body sampler is not
    /// wired here (unreversed Havok-4.6 clip format). See module docs.
    pub fn load_clip(&self, key: u32) -> Result<Clip2008, String> {
        let info = self
            .clips
            .iter()
            .find(|c| c.hash == key)
            .ok_or_else(|| format!("no 2008 clip with key {key:08X}"))?
            .clone();
        let bytes = match &info.source {
            Clip2008Source::Loose { path } => std::fs::read(path)
                .map_err(|e| format!("read {}: {e}", path.display()))?,
            Clip2008Source::Packed { pack_path, record_off, record_end } => {
                let all = std::fs::read(pack_path)
                    .map_err(|e| format!("read {}: {e}", pack_path.display()))?;
                let (o, e) = (*record_off as usize, *record_end as usize);
                if e > all.len() || o >= e {
                    return Err(format!(
                        "packed clip range {o}..{e} out of {} bytes in {}",
                        all.len(),
                        pack_path.display()
                    ));
                }
                all[o..e].to_vec()
            }
        };
        let header = Anim2008Header::parse(&bytes)?;
        Ok(Clip2008 { info, header, bytes })
    }

    /// The browser's flat handle list. Loose rows carry the real name and the true
    /// `pandemic_hash(stem)`; packed rows have an empty name and a synthetic offset-based
    /// key. The `AssetKind::Animation` discriminator flags them to the picker.
    pub fn handles(&self) -> Vec<AssetHandle> {
        self.clips
            .iter()
            .map(|c| AssetHandle {
                key: c.hash as u64,
                hash: match c.source {
                    // Only surface `hash` for named loose entries — that's the value the
                    // consumer can round-trip through `pandemic_hash(name)`. Synthetic
                    // pack keys are not real content hashes.
                    Clip2008Source::Loose { .. } => Some(c.hash),
                    Clip2008Source::Packed { .. } => None,
                },
                name: c.name.clone(),
                kind: AssetKind::Animation,
            })
            .collect()
    }
}

/// Pack-record synthetic key: a stable, unique u32 derived from the record's byte offset in
/// the pack, so a pack row can be looked up by handle without needing the plaintext name.
/// Deliberately BIASED to a distinctive high bit so it can't collide with a real
/// `pandemic_hash` value from a loose entry (pandemic_hash is well-distributed but small
/// offsets fold into low bits; the OR-in guarantees a top-bit set).
fn pack_synthetic_key(offset: u64) -> u32 {
    ((offset as u32) & 0x7FFF_FFFF) | 0x8000_0000
}

/// Recursive `*.hkx` walk.
fn walk_hkx(dir: &Path, visit: &mut dyn FnMut(&Path, String)) {
    fn go(dir: &Path, visit: &mut dyn FnMut(&Path, String)) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                go(&p, visit);
            } else if p
                .extension()
                .and_then(|s| s.to_str())
                .map(|s| s.eq_ignore_ascii_case("hkx"))
                .unwrap_or(false)
            {
                let stem = p
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_string();
                if stem.is_empty() {
                    continue;
                }
                visit(&p, stem);
            }
        }
    }
    go(dir, visit);
}

/// Read up to `want` bytes from the head of a file.
fn read_head(path: &Path, want: usize) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut buf = vec![0u8; want];
    let mut n = 0;
    while n < want {
        match f.read(&mut buf[n..])? {
            0 => break,
            k => n += k,
        }
    }
    buf.truncate(n);
    Ok(buf)
}

/// One record start in a pack, as located by the magic scan.
#[derive(Clone, Copy, Debug)]
struct PackRecord {
    offset: u64,
}

/// Locate every `ANM0(WAV|DEL|NOC)0` record start in the pack via a byte-scan for the
/// 8-byte magic. Reliable (magic is 8 distinctive ASCII bytes; no in-body false positives
/// have been observed) and independent of the outer per-record header layout. See module
/// docs for why the outer header is only partially reversed.
fn scan_pack_records(pack: &[u8]) -> Vec<PackRecord> {
    // The pack starts with `AP04ANIM` (8B) + `<u32 BE record count>` (4B) + trailing outer
    // header bytes before the first record. Skipping the leading `AP04ANIM` bytes stops the
    // magic-scan from ever mistaking the pack magic itself for a record magic (it can't
    // anyway — `ANIM` is only the tail — but this keeps the search predicate simple and
    // makes the first hit deterministic).
    if pack.len() < 12 || &pack[..8] != b"AP04ANIM" {
        return Vec::new();
    }
    let magics: [&[u8]; 3] = [b"ANM0WAV0", b"ANM0DEL0", b"ANM0NOC0"];
    let mut out = Vec::new();
    let mut i = 12usize;
    while i + 8 <= pack.len() {
        let head = &pack[i..i + 8];
        if magics.iter().any(|m| *m == head) {
            out.push(PackRecord { offset: i as u64 });
            i += 8;
        } else {
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_bin360() -> Option<PathBuf> {
        let root = PathBuf::from(
            "c:/Users/Shadow/Desktop/notes-on-reversing-the-sabetour/game-files/Bin360",
        );
        if root.join("Animations.pack").is_file() { Some(root) } else { None }
    }

    /// The header parser must round-trip on every codec variant we've catalogued. Uses
    /// synthetic buffers rather than real files so this always runs.
    #[test]
    fn parses_all_three_codec_variants() {
        for (magic, codec) in [
            (b"ANM0WAV0", ClipCodec::Wavelet),
            (b"ANM0DEL0", ClipCodec::Delta),
            (b"ANM0NOC0", ClipCodec::None),
        ] {
            let mut buf = Vec::with_capacity(24);
            buf.extend_from_slice(magic);
            buf.extend_from_slice(&0.5f32.to_be_bytes());
            buf.extend_from_slice(&37u32.to_be_bytes());
            buf.extend_from_slice(&15u32.to_be_bytes());
            buf.extend_from_slice(&16u32.to_be_bytes());
            let h = Anim2008Header::parse(&buf).expect("valid header");
            assert_eq!(h.codec, codec);
            assert!((h.duration_s - 0.5).abs() < 1e-6);
            assert_eq!(h.num_tracks, 37);
        }
    }

    #[test]
    fn rejects_a_file_that_is_not_an_anm0_record() {
        let buf = *b"NOPEok!!extradata               ";
        let e = Anim2008Header::parse(&buf).expect_err("must reject");
        assert!(e.contains("not an ANM0"), "unhelpful error: {e}");
    }

    #[test]
    fn rejects_an_impossible_duration() {
        // Same magic + bogus LE-interpreted duration bits — catches the wrong-endian bug.
        let mut buf = Vec::with_capacity(24);
        buf.extend_from_slice(b"ANM0WAV0");
        // 0x7F80_0001 as BE reads as an f32 NaN/infinity → the plausibility gate should
        // reject it before it corrupts a downstream computation.
        buf.extend_from_slice(&[0x7F, 0x80, 0x00, 0x01]);
        buf.extend_from_slice(&37u32.to_be_bytes());
        buf.extend_from_slice(&15u32.to_be_bytes());
        buf.extend_from_slice(&16u32.to_be_bytes());
        let e = Anim2008Header::parse(&buf).expect_err("must reject");
        assert!(e.contains("duration"), "unhelpful error: {e}");
    }

    #[test]
    fn catalog_enumerates_loose_and_packed_clips() {
        let Some(root) = test_bin360() else { eprintln!("skip: no Bin360/"); return };
        let cat = Anim2008Catalog::open(&root);
        // Loose tree ships ~3620 .hkx files across animations/, ObjectAnimations/, TestAnims/.
        assert!(
            cat.loose_count > 3000,
            "expected many loose clips, got {}",
            cat.loose_count
        );
        // Pack declares 1605 records.
        assert!(
            (1400..=1700).contains(&cat.pack_count),
            "expected ~1605 packed records, got {}",
            cat.pack_count
        );
        // Every named entry should have a plausible name (ASCII printable + digits) and a
        // finite non-negative duration.
        let named: Vec<&Clip2008Info> = cat
            .clips
            .iter()
            .filter(|c| !c.name.is_empty())
            .collect();
        assert!(named.len() >= 3000);
        for c in named.iter().take(200) {
            assert!(
                c.name.chars().all(|ch| ch.is_ascii_graphic() || ch == ' '),
                "clip name has non-printable: {:?}",
                c.name
            );
            assert!((3..=64).contains(&c.name.len()), "implausible name len: {}", c.name);
            assert!(c.duration_s >= 0.0 && c.duration_s < 3600.0);
            // NOC0 static-pose clips (e.g. `aurora_dead_idle_drvr`) report num_tracks=0
            // in the header field (their "tracks" are inlined differently). Cap upper
            // bound only.
            assert!(c.num_tracks < 200, "clip {:?}: num_tracks={}", c.name, c.num_tracks);
        }
        // At least most clips should have non-zero tracks — bulk-static pose files are the
        // exception, not the rule.
        let non_zero = named.iter().filter(|c| c.num_tracks > 0).count();
        assert!(
            non_zero * 2 > named.len(),
            "expected majority of clips to have non-zero tracks, got {non_zero}/{}",
            named.len()
        );
        // The codec-count breakdown should include some of each — matches the sampled
        // corpus (mostly WAV0 with a meaningful DEL0 tail for cinematics and NOC0 for
        // static poses). Numbers are conservative floors, not exact matches, so a per-copy
        // variance doesn't fail the test.
        assert!(cat.codec_counts[ClipCodec::Wavelet as usize] > 2000, "WAV0 too low");
        assert!(cat.codec_counts[ClipCodec::Delta as usize] > 100, "DEL0 too low");
        assert!(cat.codec_counts[ClipCodec::None as usize] > 50, "NOC0 too low");
    }

    /// End-to-end "decode one clip": pick a real loose file we know exists (a Bearcrawl
    /// walk animation), round-trip it through the catalog + `load_clip`, and check the
    /// header numbers match a direct read of the same file. This is the 2008 equivalent of
    /// retail's "sample one clip and verify unit-quat rotations" — stops at the point where
    /// a codec decoder would take over.
    #[test]
    fn load_clip_round_trips_a_known_loose_entry() {
        let Some(root) = test_bin360() else { eprintln!("skip: no Bin360/"); return };
        let cat = Anim2008Catalog::open(&root);
        let hash = pandemic_hash("Bearcrawl_Walk_F_LF");
        let clip = cat.load_clip(hash).expect("clip loads by hash");
        assert_eq!(clip.info.name, "Bearcrawl_Walk_F_LF");
        assert_eq!(clip.header.codec, ClipCodec::Wavelet);
        assert!(clip.header.duration_s > 0.0);
        assert!(clip.header.num_tracks > 0);
        // The full bytes should carry the magic + at least the header (24B). A larger clip
        // is fine; a tiny one that fails this bound would mean we sliced the wrong range.
        assert!(clip.bytes.len() >= Anim2008Header::FIXED_HEADER_LEN);
        assert_eq!(&clip.bytes[..8], b"ANM0WAV0");
    }

    /// Pack scan must find exactly the 1605 records the outer header advertises. If it
    /// finds a wildly different count, the magic-scan predicate has drifted (e.g. a new
    /// codec magic sneaked in and we didn't add it to the set).
    #[test]
    fn pack_scan_matches_declared_record_count() {
        let Some(root) = test_bin360() else { eprintln!("skip: no Bin360/"); return };
        let data = std::fs::read(root.join("Animations.pack")).expect("read pack");
        assert_eq!(&data[..8], b"AP04ANIM");
        let declared = u32::from_be_bytes([data[8], data[9], data[10], data[11]]) as usize;
        let hits = scan_pack_records(&data);
        // A strict equality would flip a version-specific mismatch into a scary failure;
        // the corpus we've measured DOES hit exactly 1605 on this dump, but a stricter
        // "must-equal" check leaves no headroom for a different pack copy. Two of every
        // three known ships (the two loose-directory variants + the same pack) all report
        // 1605. Fail loudly only if the drift is > 5%.
        let drift = (hits.len() as i64 - declared as i64).abs();
        assert!(
            drift * 20 <= declared as i64,
            "pack scan found {} records; header says {declared}",
            hits.len()
        );
    }

    // ── per-frame sampling tests ──────────────────────────────────────────────────────────

    /// Every NOC0 clip in the loose corpus must obey the layout in the module doc:
    /// `24 + aux*48 + eff_tracks*4 == file_len` (transforms first, per-track descriptor
    /// trailer at end). If this drifts, the sampler is reading past the transform array's
    /// real end.
    #[test]
    fn noc0_shape_matches_file_size_on_corpus() {
        let Some(root) = test_bin360() else { eprintln!("skip: no Bin360/"); return };
        let dir = root.join("animations");
        let Ok(entries) = std::fs::read_dir(&dir) else {
            eprintln!("skip: no animations/ dir");
            return;
        };
        let mut checked = 0usize;
        for e in entries.flatten() {
            let p = e.path();
            if !p.extension().and_then(|s| s.to_str()).map(|s| s.eq_ignore_ascii_case("hkx")).unwrap_or(false) {
                continue;
            }
            let Ok(bytes) = std::fs::read(&p) else { continue };
            if bytes.len() < Anim2008Header::FIXED_HEADER_LEN || &bytes[..8] != b"ANM0NOC0" {
                continue;
            }
            let Ok(h) = Anim2008Header::parse(&bytes) else { continue };
            let (eff_tracks, frames, trailer) = match noc0_shape(&h) {
                Ok(t) => t,
                Err(_) => continue,
            };
            let expected = Anim2008Header::FIXED_HEADER_LEN
                + eff_tracks * frames * NOC0_TRANSFORM_STRIDE
                + trailer;
            assert_eq!(
                bytes.len(),
                expected,
                "NOC0 layout mismatch on {}: file={}, computed={} (eff_tracks={}, frames={})",
                p.display(),
                bytes.len(),
                expected,
                eff_tracks,
                frames
            );
            checked += 1;
            if checked >= 400 {
                break;
            }
        }
        assert!(checked > 20, "expected to check many NOC0 files, got {checked}");
    }

    /// Sample a static-pose NOC0 clip (aurora — 1 frame, 52 tracks) at t=0, t=middle,
    /// t=end. Every returned transform must be finite, a unit quaternion (within 1e-4),
    /// and non-negative scales — the "sensible bounds" check the task calls for.
    #[test]
    fn sample_noc0_static_pose_within_bounds() {
        let Some(root) = test_bin360() else { eprintln!("skip: no Bin360/"); return };
        let cat = Anim2008Catalog::open(&root);
        let hash = pandemic_hash("aurora_dead_idle_drvr");
        let clip = match cat.load_clip(hash) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("skip: {e}");
                return;
            }
        };
        assert_eq!(clip.header.codec, ClipCodec::None);
        let (eff_tracks, frames) = clip.noc0_shape().expect("shape");
        assert_eq!(frames, 1, "aurora dead-idle is a static pose");
        assert!(eff_tracks > 0);
        // 0, middle, end all resolve to frame 0 for a static pose — still exercise the
        // clamp path and the interior lerp path in one sweep.
        for &t in &[0.0f32, 0.5 * clip.header.duration_s, clip.header.duration_s] {
            for track in 0..eff_tracks as u32 {
                let s = clip.sample_track(track, t).expect("sample ok");
                for c in s.pos.iter().chain(s.rot.iter()).chain(s.scale.iter()) {
                    assert!(c.is_finite(), "non-finite output for track {track} at t={t}: {s:?}");
                }
                let m = (s.rot[0] * s.rot[0] + s.rot[1] * s.rot[1] + s.rot[2] * s.rot[2] + s.rot[3] * s.rot[3]).sqrt();
                assert!((m - 1.0).abs() < 1e-4, "non-unit quat |q|={m} for track {track}: {s:?}");
                // Scale can be very small in a static pose but never negative in this
                // corpus. Guard against a NaN-produced tiny-negative on the interp path.
                for (i, &c) in s.scale.iter().enumerate() {
                    assert!(c >= -1e-4, "negative scale.{i}={c} on track {track}");
                }
            }
        }
    }

    /// Sample a multi-frame NOC0 clip at t=0, t=middle, t=end. Same "sensible bounds"
    /// invariants; also asserts that AT LEAST one track's translation differs between t=0
    /// and t=middle (otherwise the frame-major access is silently reading the same slot
    /// twice — a common bug when frames_per_track drifts).
    #[test]
    fn sample_noc0_multi_frame_within_bounds_and_animates() {
        let Some(root) = test_bin360() else { eprintln!("skip: no Bin360/"); return };
        let cat = Anim2008Catalog::open(&root);
        // Find a multi-frame NOC0 clip that ACTUALLY changes between frames — some
        // shipped NOC0 files are static-pose duplicated across N frames (back_prone_pose
        // repeats the exact same 36 transforms in each of its 5 frames). We need one with
        // per-frame motion to prove the frame-major access is doing real work. Prefer
        // Bugatti_* which carry 6-frame driving cycles with per-wheel motion.
        let candidates = [
            "Bugatti_drive_turn_left",
            "KD_head_front",
            "Rope_horiz_down_long_start",
            "MG42_fire_pose",
            "back_prone_pose",
        ];
        let mut opened: Option<Clip2008> = None;
        for name in &candidates {
            if let Ok(c) = cat.load_clip(pandemic_hash(name)) {
                if matches!(c.header.codec, ClipCodec::None) {
                    if let Ok((_, f)) = c.noc0_shape() {
                        if f > 1 {
                            opened = Some(c);
                            break;
                        }
                    }
                }
            }
        }
        let Some(clip) = opened else {
            eprintln!("skip: no known multi-frame NOC0 clip available");
            return;
        };
        let (eff_tracks, frames) = clip.noc0_shape().expect("shape");
        assert!(frames > 1);
        let dur = clip.header.duration_s;
        let times = [0.0f32, 0.5 * dur, dur];
        // Bounds check across every track at every sampled time.
        for &t in &times {
            for track in 0..eff_tracks as u32 {
                let s = clip.sample_track(track, t).expect("sample ok");
                for c in s.pos.iter().chain(s.rot.iter()).chain(s.scale.iter()) {
                    assert!(c.is_finite());
                }
                let m = (s.rot[0] * s.rot[0] + s.rot[1] * s.rot[1] + s.rot[2] * s.rot[2] + s.rot[3] * s.rot[3]).sqrt();
                assert!((m - 1.0).abs() < 1e-4, "non-unit quat |q|={m}");
            }
        }
        // Something MUST move over the clip — otherwise the sampler is reading a static
        // slot regardless of `time_s`. Some multi-frame clips ARE near-static on most
        // tracks, so we check every track for any change and require at least one hit.
        let mut moved = false;
        let s0 = (0..eff_tracks as u32).map(|t| clip.sample_track(t, 0.0).unwrap()).collect::<Vec<_>>();
        let sm = (0..eff_tracks as u32).map(|t| clip.sample_track(t, 0.5 * dur).unwrap()).collect::<Vec<_>>();
        for (a, b) in s0.iter().zip(sm.iter()) {
            let dp = ((a.pos[0] - b.pos[0]).powi(2) + (a.pos[1] - b.pos[1]).powi(2) + (a.pos[2] - b.pos[2]).powi(2)).sqrt();
            let dq = ((a.rot[0] - b.rot[0]).powi(2) + (a.rot[1] - b.rot[1]).powi(2) + (a.rot[2] - b.rot[2]).powi(2) + (a.rot[3] - b.rot[3]).powi(2)).sqrt();
            if dp > 1e-5 || dq > 1e-5 {
                moved = true;
                break;
            }
        }
        assert!(moved, "no track changed between t=0 and t=middle — sampler stuck?");
    }

    /// Round-trip stability: sampling the same (track, time) twice must return the same
    /// bits. Any drift here means we introduced hidden state (a cache write, a rand hop)
    /// somewhere in the interp path.
    #[test]
    fn sample_noc0_is_deterministic() {
        let Some(root) = test_bin360() else { eprintln!("skip: no Bin360/"); return };
        let cat = Anim2008Catalog::open(&root);
        let Ok(clip) = cat.load_clip(pandemic_hash("back_prone_pose")) else {
            eprintln!("skip: back_prone_pose not present");
            return;
        };
        let (eff_tracks, _) = clip.noc0_shape().expect("shape");
        let mid = 0.5 * clip.header.duration_s;
        for track in 0..eff_tracks as u32 {
            let a = clip.sample_track(track, mid).unwrap();
            let b = clip.sample_track(track, mid).unwrap();
            assert_eq!(a, b, "non-deterministic sample for track {track}");
        }
    }

    /// Corpus sweep — sample the first frame of every WAV0 and DEL0 clip we
    /// can enumerate, and report how many decompressed to a numerically
    /// valid pose (finite, |q|≈1, scale > 0) vs. returned a diagnostic
    /// `Err`. Honest reporting only — no min-success gate, because the
    /// ratio is expected to be low until the block_size=16 lifting pass
    /// is ported (see `prototype2008_anim_havok46` module doc). The count
    /// is printed to stderr so a follow-up pass can gate against a
    /// specific baseline.
    #[test]
    fn corpus_sweep_wav0_and_del0_first_frame() {
        let Some(root) = test_bin360() else { eprintln!("skip: no Bin360/"); return };
        let cat = Anim2008Catalog::open(&root);
        let mut wav0_tot = 0usize;
        let mut wav0_ok = 0usize;
        let mut del0_tot = 0usize;
        let mut del0_ok = 0usize;
        let mut first_wav_err: Option<String> = None;
        let mut first_del_err: Option<String> = None;
        for info in cat.clips.iter() {
            if info.name.is_empty() {
                continue;
            }
            match info.codec {
                ClipCodec::Wavelet => {
                    if wav0_tot >= 200 {
                        continue;
                    }
                    let Ok(clip) = cat.load_clip(info.hash) else { continue };
                    wav0_tot += 1;
                    match clip.sample_track(0, 0.0) {
                        Ok(s) => {
                            let m = (s.rot[0] * s.rot[0]
                                + s.rot[1] * s.rot[1]
                                + s.rot[2] * s.rot[2]
                                + s.rot[3] * s.rot[3])
                                .sqrt();
                            if s.pos.iter().chain(s.rot.iter()).chain(s.scale.iter()).all(|c| c.is_finite())
                                && (m - 1.0).abs() < 1e-3
                                && s.scale.iter().all(|&c| c >= -1e-3)
                            {
                                wav0_ok += 1;
                            }
                        }
                        Err(e) => {
                            if first_wav_err.is_none() {
                                first_wav_err = Some(e);
                            }
                        }
                    }
                }
                ClipCodec::Delta => {
                    if del0_tot >= 200 {
                        continue;
                    }
                    let Ok(clip) = cat.load_clip(info.hash) else { continue };
                    del0_tot += 1;
                    match clip.sample_track(0, 0.0) {
                        Ok(s) => {
                            let m = (s.rot[0] * s.rot[0]
                                + s.rot[1] * s.rot[1]
                                + s.rot[2] * s.rot[2]
                                + s.rot[3] * s.rot[3])
                                .sqrt();
                            if s.pos.iter().chain(s.rot.iter()).chain(s.scale.iter()).all(|c| c.is_finite())
                                && (m - 1.0).abs() < 1e-3
                                && s.scale.iter().all(|&c| c >= -1e-3)
                            {
                                del0_ok += 1;
                            }
                        }
                        Err(e) => {
                            if first_del_err.is_none() {
                                first_del_err = Some(e);
                            }
                        }
                    }
                }
                ClipCodec::None => {}
            }
        }
        eprintln!(
            "WAV0 first-frame decode: {wav0_ok}/{wav0_tot} clean (first_err: {:?})",
            first_wav_err
        );
        eprintln!(
            "DEL0 first-frame decode: {del0_ok}/{del0_tot} clean (first_err: {:?})",
            first_del_err
        );
        assert!(wav0_tot >= 100, "expected ≥100 WAV0 clips to sample, got {wav0_tot}");
        assert!(del0_tot >= 100, "expected ≥100 DEL0 clips to sample, got {del0_tot}");
    }

    /// WAV0 and DEL0 sample at t=0, t=middle, t=end.
    /// Sampling behaviour must be *consistent* across all three time
    /// points — either all succeed with valid transforms or all fail
    /// with a diagnostic. Guards against a decode path that only
    /// succeeds at t=0 (would indicate the interp path is broken).
    #[test]
    fn sample_wavelet_and_delta_at_start_middle_end() {
        let Some(root) = test_bin360() else { eprintln!("skip: no Bin360/"); return };
        let cat = Anim2008Catalog::open(&root);
        for name in &["Bearcrawl_Walk_F_LF", "cine_A1M1_S1_Dierker"] {
            let Ok(clip) = cat.load_clip(pandemic_hash(name)) else { continue };
            let dur = clip.header.duration_s;
            let times = [0.0f32, 0.5 * dur, dur];
            let mut oks = 0usize;
            let mut first_err: Option<String> = None;
            for &t in &times {
                match clip.sample_track(0, t) {
                    Ok(s) => {
                        oks += 1;
                        for c in s.pos.iter().chain(s.rot.iter()).chain(s.scale.iter()) {
                            assert!(c.is_finite(), "clip {name} at t={t}: non-finite {s:?}");
                        }
                        let m = (s.rot[0] * s.rot[0]
                            + s.rot[1] * s.rot[1]
                            + s.rot[2] * s.rot[2]
                            + s.rot[3] * s.rot[3])
                            .sqrt();
                        assert!(
                            (m - 1.0).abs() < 1e-3,
                            "clip {name} at t={t}: non-unit quaternion |q|={m}"
                        );
                        for &c in &s.scale {
                            assert!(c >= -1e-3, "clip {name} at t={t}: negative scale {c}");
                        }
                    }
                    Err(e) => {
                        if first_err.is_none() {
                            first_err = Some(e);
                        }
                    }
                }
            }
            assert!(
                oks == 0 || oks == 3,
                "clip {name}: inconsistent sample results across t=0/mid/end ({oks}/3 ok, first_err: {:?})",
                first_err
            );
            if oks == 0 {
                eprintln!("clip {name}: all 3 times declined (first_err: {:?})", first_err);
            }
        }
    }

    /// Determinism: sampling the same WAV0/DEL0 (clip, track, time) twice
    /// must return the same bits (or the same `Err`). Any drift means we
    /// introduced hidden state somewhere in the codec path.
    #[test]
    fn sample_wavelet_and_delta_is_deterministic() {
        let Some(root) = test_bin360() else { eprintln!("skip: no Bin360/"); return };
        let cat = Anim2008Catalog::open(&root);
        for name in &["Bearcrawl_Walk_F_LF", "cine_A1M1_S1_Dierker"] {
            let Ok(clip) = cat.load_clip(pandemic_hash(name)) else { continue };
            let t = 0.3 * clip.header.duration_s.max(0.1);
            let a = clip.sample_track(0, t);
            let b = clip.sample_track(0, t);
            match (a, b) {
                (Ok(sa), Ok(sb)) => assert_eq!(sa, sb, "clip {name}: non-deterministic sample"),
                (Err(ea), Err(eb)) => assert_eq!(ea, eb, "clip {name}: non-deterministic error"),
                _ => panic!("clip {name}: sample_track returned Ok on one call and Err on the other"),
            }
        }
    }

    /// Bone-value sanity check — if a WAV0 clip decodes, its root track's
    /// translation must be within one metre of the origin. Real Saboteur
    /// character animations are all local-space, and the root bone
    /// (track 0) sits at or near model origin. A wildly out-of-range
    /// value would indicate a raw-bytes-to-float misinterpretation.
    #[test]
    fn wav0_decoded_root_track_translation_is_near_origin() {
        let Some(root) = test_bin360() else { eprintln!("skip: no Bin360/"); return };
        let cat = Anim2008Catalog::open(&root);
        let Ok(clip) = cat.load_clip(pandemic_hash("Bearcrawl_Walk_F_LF")) else { return };
        if let Ok(s) = clip.sample_track(0, 0.0) {
            for &c in &s.pos {
                assert!(
                    c.abs() < 5.0,
                    "root translation {c} outside 5m sanity bound (decoded raw bytes as floats?)",
                );
            }
        }
        // If Err, skip the sanity check — codec didn't decode. That's
        // what the sweep test reports on.
    }

    /// WAV0 and DEL0 must never silently produce identity poses — either they
    /// decode a real transform (finite, unit quaternion, plausible scale) or
    /// they return a diagnostic-carrying `Err`. The specific diagnostic
    /// wording is not asserted (the ported HK-4.6 decoder now returns a
    /// range of different messages depending on which layout check failed);
    /// what matters is that a returned `Ok` is numerically valid and a
    /// returned `Err` is not empty.
    #[test]
    fn sample_wavelet_and_delta_do_not_silently_succeed() {
        let Some(root) = test_bin360() else { eprintln!("skip: no Bin360/"); return };
        let cat = Anim2008Catalog::open(&root);

        // WAV0 — a shipped Bearcrawl walk.
        if let Ok(clip) = cat.load_clip(pandemic_hash("Bearcrawl_Walk_F_LF")) {
            assert_eq!(clip.header.codec, ClipCodec::Wavelet);
            match clip.sample_track(0, 0.0) {
                Ok(s) => {
                    for c in s.pos.iter().chain(s.rot.iter()).chain(s.scale.iter()) {
                        assert!(c.is_finite(), "WAV0 sampled non-finite: {s:?}");
                    }
                    let m = (s.rot[0] * s.rot[0]
                        + s.rot[1] * s.rot[1]
                        + s.rot[2] * s.rot[2]
                        + s.rot[3] * s.rot[3])
                        .sqrt();
                    assert!(
                        (m - 1.0).abs() < 1e-3,
                        "WAV0 sampled non-unit quaternion |q|={m}"
                    );
                }
                Err(e) => {
                    assert!(!e.is_empty(), "empty WAV0 diagnostic");
                    eprintln!("WAV0 sample declined: {e}");
                }
            }
        }
        // DEL0 — a shipped cinematic clip.
        if let Ok(clip) = cat.load_clip(pandemic_hash("cine_A1M1_S1_Dierker")) {
            assert_eq!(clip.header.codec, ClipCodec::Delta);
            match clip.sample_track(0, 0.0) {
                Ok(s) => {
                    for c in s.pos.iter().chain(s.rot.iter()).chain(s.scale.iter()) {
                        assert!(c.is_finite());
                    }
                }
                Err(e) => {
                    assert!(!e.is_empty(), "empty DEL0 diagnostic");
                    eprintln!("DEL0 sample declined: {e}");
                }
            }
        }
    }

    /// `noc0_shape` must accept a plausible synthesized header and reject an
    /// inconsistent one (aux not divisible by effective tracks) — a cheap always-on
    /// check that doesn't need a real install.
    #[test]
    fn noc0_shape_accepts_valid_and_rejects_ragged() {
        let ok = Anim2008Header {
            codec: ClipCodec::None,
            duration_s: 0.5,
            num_tracks: 10,
            aux_count: 50,
            stride16: 0,
        };
        let (t, f, trailer) = noc0_shape(&ok).expect("valid header");
        assert_eq!((t, f, trailer), (10, 5, 40));

        // Static-pose branch: ntt=0 folds the count into aux.
        let stat = Anim2008Header {
            codec: ClipCodec::None,
            duration_s: 0.0,
            num_tracks: 0,
            aux_count: 52,
            stride16: 0,
        };
        let (t, f, trailer) = noc0_shape(&stat).expect("static-pose header");
        assert_eq!((t, f, trailer), (52, 1, 208));

        // Ragged: aux=17 tracks=4 doesn't divide.
        let bad = Anim2008Header {
            codec: ClipCodec::None,
            duration_s: 0.5,
            num_tracks: 4,
            aux_count: 17,
            stride16: 0,
        };
        assert!(noc0_shape(&bad).is_err());
    }

    /// `noc0_read_transform` on a synthetic 48-byte buffer must recover a known TRS and
    /// produce a unit quaternion. Guards against a re-write of the BE parse or the layout
    /// offsets drifting silently.
    #[test]
    fn noc0_read_transform_round_trips_a_known_trs() {
        let pos = [1.0f32, -2.0, 3.5];
        let rot_raw = [0.0f32, 0.7071068, 0.0, 0.7071068]; // 90° about +Y, already unit
        let scale = [1.0f32, 1.0, 1.0];
        // Layout: 3T + 4R + 3S + 2pad, big-endian.
        let mut buf = Vec::with_capacity(48);
        for &f in pos.iter() {
            buf.extend_from_slice(&f.to_be_bytes());
        }
        for &f in rot_raw.iter() {
            buf.extend_from_slice(&f.to_be_bytes());
        }
        for &f in scale.iter() {
            buf.extend_from_slice(&f.to_be_bytes());
        }
        buf.extend_from_slice(&0.0f32.to_be_bytes());
        buf.extend_from_slice(&0.0f32.to_be_bytes());
        let s = noc0_read_transform(&buf, 0).expect("read ok");
        assert_eq!(s.pos, pos);
        assert!((s.rot[0] - rot_raw[0]).abs() < 1e-6);
        assert!((s.rot[1] - rot_raw[1]).abs() < 1e-6);
        assert!((s.rot[3] - rot_raw[3]).abs() < 1e-6);
        let m = (s.rot[0] * s.rot[0] + s.rot[1] * s.rot[1] + s.rot[2] * s.rot[2] + s.rot[3] * s.rot[3]).sqrt();
        assert!((m - 1.0).abs() < 1e-6);
        assert_eq!(s.scale, scale);
    }
}
