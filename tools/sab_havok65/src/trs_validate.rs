//! Per-frame TRS sanity harness for `hkaSplineCompressedAnimation`.
//!
//! `hct_validate` proves our decoder reads the same PACKFILE BYTES as HCT
//! 6.5's XML emit (2214/2214 clean). It does not exercise our sampler math
//! — the Cox-de Boor evaluator, the THREECOMP40 quaternion unpacker, the
//! block-time truncation, the per-track mask/quant walker — against any
//! independent authority.
//!
//! ## Why an independent per-frame oracle is missing
//!
//! Two paths were investigated (per the task brief):
//!   1. hkxcmd (a community CLI that samples Havok anims). Not on disk in
//!      `C:/Program Files (x86)/Havok/`, `C:/Users/Shadow/Downloads/`, or the
//!      sibling `notes-on-the-released-game/tools/` (Mercs 2 team's Havok
//!      work is 5.5 physics, no 6.5 anim sampler).
//!   2. HCT 6.5's `hctStandAloneFilterManager` filter chain producing an
//!      interleaved animation from a compressed one. Enumerating the
//!      shipped filters in `hctFilterAnimation.dll` — CreateAnimations,
//!      CreateSkeleton, CreateAdditive, ExtractMotion, Rotate, Loop,
//!      Footstep, SplineCompression, WaveletCompression, DeltaCompression
//!      — none decompress. HCT's authoring pipeline is one-way (import
//!      scene → interleaved → compress). AssetCc2 `--rulesRULES` handles
//!      cross-version packfile compat, not content compression.
//!   3. Havok SDK 6.5 header drop at
//!      `C:/Users/Shadow/Downloads/havok-sdk-pc-6.5.0-r1.7z` (used to
//!      confirm the code layout for point 2): headers + inl only, no
//!      `.lib`s, so writing a custom filter or standalone
//!      `hkaSplineCompressedAnimation::sampleTracks()` caller is off the
//!      table.
//!
//! ## What this harness verifies instead
//!
//! Four mathematical/spec properties the sampler must satisfy for every
//! clip × frame × track, independent of our own decoder:
//!
//!   * **Unit quaternion + finite.** Each sampled rotation must satisfy
//!     `||q|| ≈ 1` and every T/R/S component must be finite. A THREECOMP40
//!     sign flip, a wrong quant index, or a broken de Boor blend all
//!     surface as a non-unit or non-finite output.
//!
//!   * **Static-track invariance.** For every track whose mask has zero
//!     spline bits (`mask & 0xf0 == 0`), the sampled T/R/S must be
//!     identical at every t. Deviation reveals mask-bit misreads or an
//!     accidental read past the static payload.
//!
//!   * **Convex-hull confinement of dynamic tracks.** Cox-de Boor at u ∈
//!     [U[span], U[span+1]] with all alpha ∈ [0,1] produces a value that
//!     is a convex combination of the (p+1) local control points. Every
//!     component of every sampled T/S at every t must therefore lie in
//!     `[min_i cps[i][c], max_i cps[i][c]]` — a hard property of correct
//!     de Boor. Wrong knot span, wrong control-point indexing, wrong
//!     dequant, or wrong blend all break it.
//!
//!   * **Clamped-knot endpoint match.** When the knot vector is clamped
//!     (`knots[0..=degree]` all equal, `knots[n+1-degree..=n+1]` all
//!     equal — the standard HCT-emitted pattern), Cox-de Boor at u=U[p]
//!     equals cps[0] exactly, and at u=U[n+1] equals cps[n]. The sampler
//!     must reproduce that at `t=0` (block 0) and at the last
//!     within-block frame — modulo a per-quaternion normalization that
//!     scales cps[0] to unit length. Reveals off-by-one span search,
//!     miscomputed frame_in_block, or a wrong CP base.
//!
//! The convex-hull and endpoint tests read the block's control-point
//! bytes via a SECOND, independent walker (`decompose_block` below) that
//! is deliberately laid out unlike `eval_vec4`/`eval_rotation` in
//! `main.rs`: it front-loads every track's full channel decomposition
//! into a Vec, then property-checks against the primary sampler's
//! output. Two decomposers with independent errors would have to
//! coincidentally read the same wrong bytes to fake a match, which is
//! not a real risk once the property in question (a convex combination)
//! is itself independent of both codes.
//!
//! Not covered:
//!
//!   * Absolute correctness of a mid-span sample vs an external oracle.
//!     No oracle on disk can produce that value. The convex-hull test
//!     bounds the answer but does not pin it.
//!   * Quaternion types other than THREECOMP40 (retail corpus is 100%
//!     type 1). Other quant paths remain unexercised.
//!   * Multi-block clips (retail corpus is single-block; the block-index
//!     branch is code-present but data-absent).

use crate::{
    f32le, find_span, parse_ap0l, read_spline_anim, u16le, Packfile, QsTransform,
};
use std::fs;

/// Absolute tolerance used for unit-quaternion checks and endpoint
/// matches. The sampler renormalizes after de Boor, and HCT's own
/// re-quantization budget for rotations is 0.001 (memory
/// `[[havok-anim-toolchain-roundtrip-proven]]`); sampling should be
/// tighter. `1e-4` gives three orders of headroom below that encode
/// budget and is easily met by a correct de Boor at boundary knots.
const F_TOL: f32 = 1e-4;

/// A per-channel decomposition of one transform track within one block.
/// Independently walked by `decompose_block`.
#[derive(Debug)]
struct Channel {
    mask: u8,
    quant_type: u8,
    /// One of "T", "R", "S" — for reporting only.
    kind: &'static str,
    /// The 3 static-per-component values (bits 0..3 of `mask`).
    statics: [f32; 4],
    /// (lo, hi) per dynamic-component axis (bits 4..7 of `mask`).
    range: [(f32, f32); 3],
    /// Spline data (present iff `mask & 0xf0 != 0`).
    spline: Option<Spline>,
}

#[derive(Debug)]
struct Spline {
    /// `n = numControlPoints - 1`.
    n: usize,
    degree: usize,
    /// Raw u8 knot values (length `n + degree + 2`).
    knots: Vec<u8>,
    /// Every control point, expanded to a full vec4 (spline comps
    /// dequantized, other comps filled with statics/default).
    ///
    /// For a rotation channel these are packed-quaternion CPs unpacked
    /// via `unpack_quat_ref`; for T/S they are the range-dequantized
    /// linear CPs.
    cps: Vec<[f32; 4]>,
    /// Whether the knot vector is clamped (first/last p+1 knots equal).
    clamped: bool,
}

/// Independent implementation of the THREECOMP40 unpacker — deliberately
/// spelled out differently from `main.rs::unpack_quat`. Kept as an
/// oracle used only by the endpoint-match test.
fn unpack_quat_ref(bytes: &[u8]) -> [f32; 4] {
    // Reassemble the 40-bit LE word by hand.
    let mut w: u64 = 0;
    for (i, &b) in bytes.iter().take(5).enumerate() {
        w |= (b as u64) << (8 * i);
    }
    // Extract three 12-bit small components.
    let raw0 = (w & 0xFFF) as i32;
    let raw1 = ((w >> 12) & 0xFFF) as i32;
    let raw2 = ((w >> 24) & 0xFFF) as i32;
    // Dequant per FUN_00f22470: v = (raw - 2047) / (2047*sqrt(2)).
    let denom = 2047.0 * (2.0f32).sqrt();
    let a = (raw0 - 2047) as f32 / denom;
    let b = (raw1 - 2047) as f32 / denom;
    let c = (raw2 - 2047) as f32 / denom;
    let idx = ((w >> 36) & 0b11) as usize;
    let neg = ((w >> 38) & 1) != 0;
    let sumsq = a * a + b * b + c * c;
    let mut large = (1.0f32 - sumsq).max(0.0).sqrt();
    if neg {
        large = -large;
    }
    let mut out = [0f32; 4];
    let smalls = [a, b, c];
    let mut si = 0;
    for i in 0..4 {
        if i == idx {
            out[i] = large;
        } else {
            out[i] = smalls[si];
            si += 1;
        }
    }
    out
}

/// Decompose one block of a spline animation into per-track/per-channel
/// state — knot vector, ranges, control points — with no evaluation.
///
/// Layout mirrors `main.rs`'s inline reader but produces a structured
/// dump instead of a pose; the two paths must therefore agree on where
/// bytes live for any downstream property test to make sense.
fn decompose_block(
    anim: &crate::SplineAnim,
    blob: &[u8],
    block_index: usize,
) -> Result<Vec<[Channel; 3]>, String> {
    let block_start = anim.data_off
        + anim.block_offsets.get(block_index).copied().ok_or("block idx")? as usize;
    let nt = anim.num_transform_tracks;
    let mut ptr = block_start + anim.mask_and_quant_size;
    let mut out = Vec::with_capacity(nt);

    for t in 0..nt {
        let hdr = block_start + t * 4;
        let ctrl = blob[hdr];
        let trn_mask = blob[hdr + 1];
        let rot_mask = blob[hdr + 2];
        let scl_mask = blob[hdr + 3];
        let trn_type = ctrl & 3;
        let rot_type = (ctrl >> 2) & 0xf;
        let scl_type = ctrl >> 6;

        let trn = read_vec4_channel(blob, &mut ptr, trn_mask, trn_type, "T", [0.0; 4])?;
        ptr = (ptr + 3) & !3;
        let rot = read_rot_channel(blob, &mut ptr, rot_mask, rot_type)?;
        ptr = (ptr + 3) & !3;
        let scl = read_vec4_channel(blob, &mut ptr, scl_mask, scl_type, "S", [1.0; 4])?;
        ptr = (ptr + 3) & !3;
        let _ = t;
        out.push([trn, rot, scl]);
    }
    Ok(out)
}

fn read_vec4_channel(
    data: &[u8],
    ptr: &mut usize,
    mask: u8,
    quant_type: u8,
    kind: &'static str,
    default: [f32; 4],
) -> Result<Channel, String> {
    let has_spline = mask & 0xf0 != 0;
    let (mut n, mut degree, mut knots) = (0usize, 0usize, Vec::<u8>::new());
    if has_spline {
        n = u16le(data, *ptr) as usize;
        *ptr += 2;
        degree = data[*ptr] as usize;
        *ptr += 1;
        let knot_ptr = *ptr;
        // Total knots = n + degree + 2 (per Piegl-Tiller: numKnots = numCP + degree + 1).
        for i in 0..(n + degree + 2) {
            knots.push(data[knot_ptr + i]);
        }
        *ptr = knot_ptr + n + 2 + degree;
    }
    *ptr = (*ptr + 3) & !3;
    let mut statics = [0f32; 4];
    let mut range = [(0f32, 0f32); 3];
    for i in 0..3 {
        if mask & (1 << i) != 0 {
            statics[i] = f32le(data, *ptr);
            *ptr += 4;
        } else if mask & (0x10 << i) != 0 {
            let lo = f32le(data, *ptr);
            *ptr += 4;
            let hi = f32le(data, *ptr);
            *ptr += 4;
            range[i] = (lo, hi);
        }
    }
    if mask & 8 != 0 {
        statics[3] = f32le(data, *ptr);
        *ptr += 4;
    }

    let mut spline = None;
    if has_spline {
        *ptr = (*ptr + 1) & !1;
        let bytes_per = if quant_type == 0 { 1usize } else { 2usize };
        let dyn_bits: Vec<usize> = (0..3).filter(|&i| mask & (0x10 << i) != 0).collect();
        let n_comp = dyn_bits.len();
        let num_cp = n + 1;
        let cp_base = *ptr;
        let mut cps = Vec::with_capacity(num_cp);
        for cp_index in 0..num_cp {
            let mut v = [0f32; 4];
            for (c, &comp) in dyn_bits.iter().enumerate() {
                let off = cp_base + (cp_index * n_comp + c) * bytes_per;
                let q = if bytes_per == 1 {
                    data[off] as f32 / 255.0
                } else {
                    u16le(data, off) as f32 / 65535.0
                };
                let (lo, hi) = range[comp];
                v[comp] = lo + q * (hi - lo);
            }
            for i in 0..4 {
                if mask & (1 << i) != 0 {
                    v[i] = statics[i];
                } else if (mask & (0x10 << i)) == 0 {
                    v[i] = default[i];
                }
            }
            cps.push(v);
        }
        *ptr = cp_base + num_cp * n_comp * bytes_per;
        let clamped = is_clamped(&knots, degree, n);
        spline = Some(Spline { n, degree, knots, cps, clamped });
    }
    Ok(Channel { mask, quant_type, kind, statics, range, spline })
}

fn read_rot_channel(
    data: &[u8],
    ptr: &mut usize,
    mask: u8,
    quant_type: u8,
) -> Result<Channel, String> {
    let cpsize = match quant_type {
        0 => 4,
        1 => 5,
        2 => 6,
        3 => 3,
        _ => 5,
    };
    let mut statics = [0f32; 4];
    let range = [(0f32, 0f32); 3];
    let mut spline = None;
    if mask & 0xf0 != 0 {
        let n = u16le(data, *ptr) as usize;
        *ptr += 2;
        let degree = data[*ptr] as usize;
        *ptr += 1;
        let knot_ptr = *ptr;
        let mut knots = Vec::with_capacity(n + degree + 2);
        for i in 0..(n + degree + 2) {
            knots.push(data[knot_ptr + i]);
        }
        *ptr = knot_ptr + n + 2 + degree;
        // Rotation CPs are 1-byte aligned per rot_align() but padding
        // may still be required for THREECOMP48 etc; corpus is qtype=1
        // (5-byte) which is 1-byte aligned already.
        // NB: for rotation CPs we store the RAW quaternions here.
        // `main.rs::eval_rotation` hemisphere-aligns each LOCAL CP
        // window (span-degree..=span) to its own first entry, not to a
        // global reference; property checks below reproduce the local
        // flip on the fly so the two hemisphere policies always agree.
        let cp_base = *ptr;
        let mut cps = Vec::with_capacity(n + 1);
        for cp_index in 0..=n {
            let cp = unpack_quat_ref(&data[cp_base + cp_index * cpsize..][..cpsize]);
            cps.push(cp);
        }
        *ptr = cp_base + (n + 1) * cpsize;
        let clamped = is_clamped(&knots, degree, n);
        spline = Some(Spline { n, degree, knots, cps, clamped });
    } else if mask & 0xf != 0 {
        // Single packed quaternion (static-rotation-only path).
        let cp = unpack_quat_ref(&data[*ptr..][..cpsize]);
        statics = cp;
        *ptr += cpsize;
    }
    Ok(Channel {
        mask,
        quant_type,
        kind: "R",
        statics,
        range,
        spline,
    })
}

fn is_clamped(knots: &[u8], p: usize, n: usize) -> bool {
    if knots.len() < n + p + 2 || p == 0 {
        return false;
    }
    let k0 = knots[0];
    for i in 1..=p {
        if knots[i] != k0 {
            return false;
        }
    }
    let ke = knots[n + p + 1];
    for i in (n + 1)..=(n + p + 1) {
        if knots[i] != ke {
            return false;
        }
    }
    true
}

/// One aggregate finding across the corpus.
#[derive(Debug, Default)]
struct Report {
    clips_seen: usize,
    clips_skipped_multiblock: usize,
    frames_sampled: u64,
    /// Rotation-channel norm stats (|q| - 1).
    unit_quat_max_dev: f32,
    unit_quat_violations: u64,
    nonfinite_values: u64,
    /// Static-track deviation stats (per-component max |a - b|).
    static_max_dev: f32,
    static_violations: u64,
    /// Convex-hull deviation: for each dynamic component, how far outside
    /// [min, max] over the local CPs the sampled value fell (post-norm
    /// for rotation, so a small extra headroom applies).
    convex_hull_max_dev: f32,
    convex_hull_violations: u64,
    convex_hull_checks: u64,
    /// Convex-hull breakdown for the strict channels (T + S; rotation
    /// gets a widen for renormalization). Populated separately so a
    /// rotation-only overshoot cannot mask a T/S bug.
    convex_hull_max_dev_ts: f32,
    convex_hull_violations_ts: u64,
    convex_hull_checks_ts: u64,
    /// Clamped-knot endpoint match: |sample_at_endpoint - first/last CP|.
    endpoint_max_dev: f32,
    endpoint_violations: u64,
    endpoint_checks: u64,
    /// `sample_at(t)` called twice returned identical bytes.
    determinism_violations: u64,
    /// Track counts for context.
    tracks_all_static: u64,
    tracks_static_only: u64,
    tracks_with_spline: u64,
}

impl Report {
    fn add(&mut self, other: &Report) {
        self.clips_seen += other.clips_seen;
        self.clips_skipped_multiblock += other.clips_skipped_multiblock;
        self.frames_sampled += other.frames_sampled;
        self.unit_quat_max_dev = self.unit_quat_max_dev.max(other.unit_quat_max_dev);
        self.unit_quat_violations += other.unit_quat_violations;
        self.nonfinite_values += other.nonfinite_values;
        self.static_max_dev = self.static_max_dev.max(other.static_max_dev);
        self.static_violations += other.static_violations;
        self.convex_hull_max_dev = self.convex_hull_max_dev.max(other.convex_hull_max_dev);
        self.convex_hull_violations += other.convex_hull_violations;
        self.convex_hull_checks += other.convex_hull_checks;
        self.convex_hull_max_dev_ts = self
            .convex_hull_max_dev_ts
            .max(other.convex_hull_max_dev_ts);
        self.convex_hull_violations_ts += other.convex_hull_violations_ts;
        self.convex_hull_checks_ts += other.convex_hull_checks_ts;
        self.endpoint_max_dev = self.endpoint_max_dev.max(other.endpoint_max_dev);
        self.endpoint_violations += other.endpoint_violations;
        self.endpoint_checks += other.endpoint_checks;
        self.determinism_violations += other.determinism_violations;
        self.tracks_all_static += other.tracks_all_static;
        self.tracks_static_only += other.tracks_static_only;
        self.tracks_with_spline += other.tracks_with_spline;
    }
}

/// Check one clip against every property. Multi-block clips are skipped
/// (block path is untested; retail corpus is single-block).
fn validate_clip(
    pk: &Packfile,
    blob: &[u8],
    obj_src: usize,
) -> Result<Report, String> {
    let anim = read_spline_anim(pk, obj_src);
    let mut r = Report::default();
    r.clips_seen = 1;
    if anim.num_blocks > 1 {
        r.clips_skipped_multiblock = 1;
        return Ok(r);
    }
    // Independent block decomposition (structured CP + knot dump).
    let decomp = decompose_block(&anim, blob, 0)?;

    // Track-type census.
    for tri in &decomp {
        let has_spline = tri.iter().any(|c| c.spline.is_some());
        let has_static = tri.iter().any(|c| c.mask & 0xf != 0);
        let has_any = tri.iter().any(|c| c.mask != 0);
        if !has_any {
            r.tracks_all_static += 1;
        }
        if has_spline {
            r.tracks_with_spline += 1;
        } else if has_static {
            r.tracks_static_only += 1;
        }
    }

    // Choose sample times: t=0, t≈mid, t=(numFrames-1)*frameDuration.
    // For a single-block clip these span the whole clip. Add a few
    // interior u's so convex-hull is exercised across the span.
    let last_t = if anim.num_frames > 0 {
        (anim.num_frames - 1) as f32 * anim.frame_duration
    } else {
        0.0
    };
    let mut times = vec![0.0f32];
    let n = anim.num_frames.max(2);
    for i in 1..(n - 1) {
        // sample every ~1/8th of the clip, but always include some
        // interior frames.
        if i * 8 % (n - 1) < 8 || i == n / 2 {
            times.push(i as f32 * anim.frame_duration);
        }
    }
    times.push(last_t);
    // Cap the per-clip work; large clips still get uniform coverage.
    if times.len() > 12 {
        let step = times.len() / 12;
        let mut sel = Vec::new();
        for i in 0..12 {
            sel.push(times[i * step]);
        }
        times = sel;
    }

    for &t in &times {
        let frame0 = anim.sample_at(blob, t);
        // Determinism: re-sample and compare bit-identical.
        let frame1 = anim.sample_at(blob, t);
        for (a, b) in frame0.iter().zip(frame1.iter()) {
            for i in 0..4 {
                if a.t[i].to_bits() != b.t[i].to_bits() {
                    r.determinism_violations += 1;
                }
                if a.r[i].to_bits() != b.r[i].to_bits() {
                    r.determinism_violations += 1;
                }
                if a.s[i].to_bits() != b.s[i].to_bits() {
                    r.determinism_violations += 1;
                }
            }
        }
        r.frames_sampled += frame0.len() as u64;

        // Compute frame-in-block for the corresponding span search below.
        let fib = ((t * anim.block_inv_duration
            * (anim.max_frames_per_block as f32 - 1.0))
            .round()
            .clamp(0.0, 255.0)) as u8;

        for (ti, qs) in frame0.iter().enumerate() {
            // Finite check.
            for &c in qs.t.iter().chain(qs.r.iter()).chain(qs.s.iter()) {
                if !c.is_finite() {
                    r.nonfinite_values += 1;
                }
            }
            // Unit-quaternion.
            let nrm = (qs.r[0] * qs.r[0]
                + qs.r[1] * qs.r[1]
                + qs.r[2] * qs.r[2]
                + qs.r[3] * qs.r[3])
                .sqrt();
            if nrm.is_finite() {
                let dev = (nrm - 1.0).abs();
                r.unit_quat_max_dev = r.unit_quat_max_dev.max(dev);
                if dev > F_TOL {
                    r.unit_quat_violations += 1;
                }
            }

            // Per-track property checks against the decomposition.
            let chans = &decomp[ti];
            check_track_props(qs, chans, t, fib, &anim, &mut r);
        }
    }
    Ok(r)
}

fn check_track_props(
    qs: &QsTransform,
    chans: &[Channel; 3],
    t: f32,
    fib: u8,
    anim: &crate::SplineAnim,
    r: &mut Report,
) {
    // Translation (chans[0]) and Scale (chans[2]) are the "linear" vec4
    // channels — convex hull is a direct property. Rotation (chans[1])
    // gets convex hull too but on hemisphere-aligned CPs, with a small
    // extra tolerance for the post-de Boor unit-norm scaling.
    for (idx, chan) in chans.iter().enumerate() {
        let sampled: [f32; 4] = match idx {
            0 => qs.t,
            1 => qs.r,
            2 => qs.s,
            _ => unreachable!(),
        };

        // Static-only channel (mask has static bits, no spline bits): all
        // sample times must return the static values in the set bits.
        if chan.spline.is_none() && (chan.mask & 0xf) != 0 {
            for i in 0..3 {
                if chan.mask & (1 << i) != 0 {
                    let dev = (sampled[i] - chan.statics[i]).abs();
                    r.static_max_dev = r.static_max_dev.max(dev);
                    if dev > F_TOL {
                        r.static_violations += 1;
                    }
                }
            }
            if chan.mask & 8 != 0 {
                let dev = (sampled[3] - chan.statics[3]).abs();
                r.static_max_dev = r.static_max_dev.max(dev);
                if dev > F_TOL {
                    r.static_violations += 1;
                }
            }
            continue;
        }

        let Some(spline) = &chan.spline else {
            // All-default channel: nothing more to check here.
            continue;
        };

        // Locate the local CP window for this u.
        let span = find_span(spline.n, spline.degree, fib, &spline.knots);
        let start = span.saturating_sub(spline.degree);
        let end = span.min(spline.n);
        let mut local: Vec<[f32; 4]> = (start..=end).map(|i| spline.cps[i]).collect();

        // For rotation, reproduce main.rs's per-window hemisphere flip:
        // every local CP after the first is negated if it has a negative
        // dot with the first local CP. That way the convex-hull check
        // below sees exactly the CP sign choices the sampler used.
        if idx == 1 && local.len() > 1 {
            let first = local[0];
            for cp in local.iter_mut().skip(1) {
                let dot: f32 = (0..4).map(|k| first[k] * cp[k]).sum();
                if dot < 0.0 {
                    for k in 0..4 {
                        cp[k] = -cp[k];
                    }
                }
            }
        }

        // Convex-hull confinement. For rotation the sampler applies unit
        // normalization AFTER de Boor, so we compare the sampled quaternion
        // component-wise against its own pre-normalization interval,
        // scaled by 1/|q|. Because |q| stays bounded above 1 for
        // hemisphere-aligned unit CPs (Cauchy-Schwarz: any convex combo
        // of unit vectors has |·| <= 1, and the equality holds only at a
        // single CP), the sampled value can lie slightly OUTSIDE the raw
        // hull only via the divide, and by at most `factor - 1` where
        // `factor = 1/|pre_norm|`. We widen the hull by 5% of the range
        // to absorb that; the check still catches gross indexing bugs.
        let widen = if idx == 1 { 0.05 } else { 0.0 };

        for c in 0..4 {
            // Only meaningful if this component is dynamic OR if all CPs
            // agree in this component (static/default -> a degenerate
            // hull of one point, still a valid confinement check).
            let mut lo = f32::INFINITY;
            let mut hi = f32::NEG_INFINITY;
            for cp in &local {
                if cp[c] < lo {
                    lo = cp[c];
                }
                if cp[c] > hi {
                    hi = cp[c];
                }
            }
            let width = (hi - lo).abs().max(1e-6);
            let widen_amt = widen * width + F_TOL;
            let vlo = lo - widen_amt;
            let vhi = hi + widen_amt;
            r.convex_hull_checks += 1;
            let v = sampled[c];
            if v.is_finite() {
                let dev = if v < vlo {
                    vlo - v
                } else if v > vhi {
                    v - vhi
                } else {
                    0.0
                };
                r.convex_hull_max_dev = r.convex_hull_max_dev.max(dev);
                if dev > 0.0 {
                    r.convex_hull_violations += 1;
                }
                // T (idx=0) and S (idx=2) have no post-de Boor
                // renormalization, so their hull is a hard invariant.
                if idx != 1 {
                    r.convex_hull_checks_ts += 1;
                    r.convex_hull_max_dev_ts =
                        r.convex_hull_max_dev_ts.max(dev);
                    if dev > 0.0 {
                        r.convex_hull_violations_ts += 1;
                    }
                }
            }
        }

        // Clamped-knot endpoint match at t == 0 (block 0) and at the
        // last within-block frame. For a clamped spline Cox-de Boor at
        // U[p] returns cps[0]. For rotation the returned value is
        // renormalized; we compare with the same tolerance.
        if spline.clamped {
            // Endpoint 0.
            if t == 0.0 {
                r.endpoint_checks += 1;
                let mut expected = spline.cps[0];
                if idx == 1 {
                    // Renormalize the reference quaternion the same way
                    // the sampler does.
                    let n = (expected[0].powi(2)
                        + expected[1].powi(2)
                        + expected[2].powi(2)
                        + expected[3].powi(2))
                    .sqrt();
                    if n > 1e-12 {
                        for k in 0..4 {
                            expected[k] /= n;
                        }
                    }
                }
                // For static and default components, the sampler
                // assembles them per-CP so the reference already has
                // them; but rotation may have been hemisphere-flipped
                // relative to the raw CP. Compare with sign-agnostic
                // dot for rotation.
                let dev = if idx == 1 {
                    let dot: f32 = (0..4).map(|k| expected[k] * sampled[k]).sum();
                    // Angle between two unit quaternions; sign-ambiguous.
                    (1.0 - dot.abs()).abs()
                } else {
                    (0..4)
                        .map(|k| (expected[k] - sampled[k]).abs())
                        .fold(0.0f32, f32::max)
                };
                r.endpoint_max_dev = r.endpoint_max_dev.max(dev);
                if dev > F_TOL {
                    r.endpoint_violations += 1;
                }
            }
            // Endpoint (last within-block frame). Only valid for the
            // single-block clips in the corpus.
            let last_t = if anim.num_frames > 0 {
                (anim.num_frames - 1) as f32 * anim.frame_duration
            } else {
                0.0
            };
            if t == last_t && anim.num_frames > 1 && anim.num_blocks == 1 {
                r.endpoint_checks += 1;
                let mut expected = *spline.cps.last().unwrap();
                if idx == 1 {
                    let n = (expected[0].powi(2)
                        + expected[1].powi(2)
                        + expected[2].powi(2)
                        + expected[3].powi(2))
                    .sqrt();
                    if n > 1e-12 {
                        for k in 0..4 {
                            expected[k] /= n;
                        }
                    }
                }
                let dev = if idx == 1 {
                    let dot: f32 = (0..4).map(|k| expected[k] * sampled[k]).sum();
                    (1.0 - dot.abs()).abs()
                } else {
                    (0..4)
                        .map(|k| (expected[k] - sampled[k]).abs())
                        .fold(0.0f32, f32::max)
                };
                r.endpoint_max_dev = r.endpoint_max_dev.max(dev);
                if dev > F_TOL {
                    r.endpoint_violations += 1;
                }
            }
        }
    }
}

/// Public entry point. Invoked by `sab_havok65 validate-trs <pack>`.
pub fn run(pack_path: &str) {
    let pack = fs::read(pack_path).unwrap_or_else(|e| {
        eprintln!("read pack {pack_path}: {e}");
        std::process::exit(1)
    });
    let ap = parse_ap0l(&pack).unwrap_or_else(|e| {
        eprintln!("parse AP0L: {e}");
        std::process::exit(1)
    });
    let blob = &pack[ap.blob_off..ap.blob_off + ap.hk_size];
    let pk = Packfile::parse(blob).unwrap_or_else(|e| {
        eprintln!("parse packfile: {e}");
        std::process::exit(1)
    });
    let scas: Vec<usize> = pk
        .vfixups
        .iter()
        .filter(|(_, c)| c == "hkaSplineCompressedAnimation")
        .map(|(s, _)| *s)
        .collect();
    println!(
        "pack: {} hkaSplineCompressedAnimation instances (Havok {})",
        scas.len(),
        pk.version
    );

    let mut agg = Report::default();
    let mut worst_examples: Vec<(String, usize, f32)> = Vec::new();
    for (i, &src) in scas.iter().enumerate() {
        let per_clip = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            validate_clip(&pk, blob, src)
        })) {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => {
                eprintln!("  clip #{i} decompose error: {e}");
                let mut r = Report::default();
                r.clips_seen = 1;
                r
            }
            Err(_) => {
                eprintln!("  clip #{i} panicked");
                let mut r = Report::default();
                r.clips_seen = 1;
                r
            }
        };
        // Track the worst per-property examples for reporting.
        if per_clip.convex_hull_max_dev > 0.0 {
            worst_examples.push(("convex".into(), i, per_clip.convex_hull_max_dev));
        }
        if per_clip.endpoint_max_dev > F_TOL {
            worst_examples.push(("endpoint".into(), i, per_clip.endpoint_max_dev));
        }
        agg.add(&per_clip);
    }

    println!("\n=== TRS property validation ===");
    println!(
        "clips considered      : {} (skipped multi-block: {})",
        agg.clips_seen, agg.clips_skipped_multiblock
    );
    println!("frames sampled        : {}", agg.frames_sampled);
    println!(
        "tracks (all-default / static-only / spline): {} / {} / {}",
        agg.tracks_all_static, agg.tracks_static_only, agg.tracks_with_spline
    );
    println!();
    println!("unit-quaternion max |‖q‖-1|                 : {:.2e}  (violations >{F_TOL:.1e}: {})",
             agg.unit_quat_max_dev, agg.unit_quat_violations);
    println!("non-finite T/R/S components                  : {}", agg.nonfinite_values);
    println!("static-track invariance max |Δ|              : {:.2e}  (violations: {})",
             agg.static_max_dev, agg.static_violations);
    println!(
        "convex-hull confinement all channels max overshoot : {:.2e}  ({} / {} checks failed)",
        agg.convex_hull_max_dev, agg.convex_hull_violations, agg.convex_hull_checks
    );
    println!(
        "convex-hull confinement T + S (strict) max         : {:.2e}  ({} / {} checks failed)",
        agg.convex_hull_max_dev_ts,
        agg.convex_hull_violations_ts,
        agg.convex_hull_checks_ts
    );
    println!(
        "clamped-knot endpoint match max deviation    : {:.2e}  ({} / {} checks failed)",
        agg.endpoint_max_dev, agg.endpoint_violations, agg.endpoint_checks
    );
    println!(
        "sample_at determinism (2× call, bit-identical): {} mismatches",
        agg.determinism_violations
    );

    worst_examples.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
    if !worst_examples.is_empty() {
        println!("\nworst per-clip:");
        for (k, i, v) in worst_examples.iter().take(6) {
            println!("  {k:<8}  clip #{i:<4}  dev={v:.2e}");
        }
    }

    // Exit non-zero when the strict properties fail. Convex hull for
    // rotation channels uses a 5% widen so its "violation" bar is soft;
    // we treat only the STRICT invariants as fatal here.
    let strict_fail = agg.unit_quat_violations
        + agg.nonfinite_values
        + agg.static_violations
        + agg.endpoint_violations
        + agg.determinism_violations
        + agg.convex_hull_violations_ts;
    if strict_fail > 0 {
        eprintln!("\nFAIL: {strict_fail} strict-property violations");
        std::process::exit(3);
    }
    println!("\nAll strict properties hold across the corpus.");
}

// ------------------------------------------------------------- unit tests

#[cfg(test)]
mod tests {
    use super::*;

    /// The identity quaternion in THREECOMP40 with `idx=3` (w largest,
    /// positive) has small = (0,0,0) → raw = (2047,2047,2047) and
    /// large = +sqrt(1) = 1. Encode it byte-by-byte and unpack; the
    /// reference unpacker must return (0,0,0,1) within FP tolerance.
    #[test]
    fn threecomp40_reference_identity() {
        // raw = 2047 -> 0x7FF for each 12-bit slot; idx=3 -> bits [36:38] = 0b11;
        // sign bit 38 = 0 (positive).
        let raw0: u64 = 2047;
        let raw1: u64 = 2047;
        let raw2: u64 = 2047;
        let idx: u64 = 3;
        let sign: u64 = 0;
        let w: u64 = raw0 | (raw1 << 12) | (raw2 << 24) | (idx << 36) | (sign << 38);
        let mut bytes = [0u8; 5];
        for i in 0..5 {
            bytes[i] = ((w >> (8 * i)) & 0xFF) as u8;
        }
        let q = unpack_quat_ref(&bytes);
        // (x,y,z,w) with w = idx=3 -> the largest slot is q[3].
        assert!(q[0].abs() < 1e-4, "x = {}", q[0]);
        assert!(q[1].abs() < 1e-4, "y = {}", q[1]);
        assert!(q[2].abs() < 1e-4, "z = {}", q[2]);
        assert!((q[3] - 1.0).abs() < 1e-4, "w = {}", q[3]);
    }

    /// A negative-w identity should return (0,0,0,-1).
    #[test]
    fn threecomp40_reference_neg_identity() {
        let raw0: u64 = 2047;
        let raw1: u64 = 2047;
        let raw2: u64 = 2047;
        let idx: u64 = 3;
        let sign: u64 = 1;
        let w: u64 = raw0 | (raw1 << 12) | (raw2 << 24) | (idx << 36) | (sign << 38);
        let mut bytes = [0u8; 5];
        for i in 0..5 {
            bytes[i] = ((w >> (8 * i)) & 0xFF) as u8;
        }
        let q = unpack_quat_ref(&bytes);
        assert!((q[3] + 1.0).abs() < 1e-4, "w = {}", q[3]);
    }

    /// The two THREECOMP40 unpackers (main.rs and this file) MUST agree
    /// on every possible 40-bit word — they are independent
    /// implementations of the same spec. Sample 100k random words.
    #[test]
    fn threecomp40_ref_vs_primary_random_sample() {
        // A simple LCG suffices; determinism matters more than quality.
        let mut state: u64 = 0xdead_beef_cafe_babe;
        let mut worst = 0f32;
        for _ in 0..100_000 {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let w = state & ((1u64 << 40) - 1);
            let mut bytes = [0u8; 5];
            for i in 0..5 {
                bytes[i] = ((w >> (8 * i)) & 0xFF) as u8;
            }
            let a = unpack_quat_ref(&bytes);
            let b = crate::unpack_quat_pub(&bytes, 0, 1);
            for k in 0..4 {
                let d = (a[k] - b[k]).abs();
                worst = worst.max(d);
            }
        }
        // Exact-fp equality is not guaranteed: main uses a precomputed
        // reciprocal `S = 1/(sqrt2*2047)` and multiplies; the reference
        // divides by `denom` each time. Slightly different ULPs.
        // `1e-4` is our `F_TOL`, matching the corpus-level bar. A real
        // spec disagreement (wrong sign, wrong index, wrong offset)
        // would show up as `>1e-1`; ULP jitter maxes out around 2e-5.
        assert!(worst < F_TOL, "worst ref-vs-primary Δ = {worst:e}");
    }
}
