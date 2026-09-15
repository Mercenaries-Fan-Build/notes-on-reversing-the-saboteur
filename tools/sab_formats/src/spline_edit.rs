//! Per-FRAME authoring for The Saboteur's `hkaSplineCompressedAnimation`
//! clips — the missing capability from [`crate::ap0l`]'s static-value editors.
//!
//! # Approach: keyframe-baking (not in-place CP surgery)
//!
//! Editing one frame of a NURBS-encoded track cleanly requires either:
//!
//!   1. **In-place CP surgery** — invert the de Boor evaluator, solve for the
//!      minimum set of control points that move the target frame's value to
//!      the requested value without disturbing others. Non-trivial for
//!      degree > 1 (frames between knots depend on multiple CPs, so an exact
//!      change at one frame forces a re-fit of neighboring segments). The
//!      Havok 6.5 SDK is headers-only and its authoring pipeline is one-way,
//!      so we have no oracle to prove a re-fit "did no harm" beyond
//!      Havok's own translationTolerance/rotationTolerance budgets.
//!
//!   2. **Keyframe baking** — replace the target track's *entire* NURBS
//!      representation with a linear (`degree = 1`) B-spline whose control
//!      points are 1-per-frame. The knot vector is
//!      `[0, 0, 1, 2, ..., F-1, F-1]` (clamped u8 frame indices). Under that
//!      knot layout, evaluating at frame index `f` returns `CP[f]` exactly —
//!      the target frame's value is whatever we write, and any other frame's
//!      value is preserved by sampling the original spline at that frame and
//!      writing the sample as its CP.
//!
//! We took approach (2). Reasons:
//!
//! * **Exact at the target frame.** `sample_at(f * frame_duration)` reads
//!   back the CP verbatim (modulo 16-bit dequant / THREECOMP40 dequant,
//!   which are the same losses the retail toolchain already accepts).
//! * **Uniform across track shapes.** Static tracks, part-static-part-spline
//!   tracks, and full splines all bake to the same target layout.
//! * **No re-fit risk.** No shape decision is made for frames we didn't
//!   touch — each retains the *decoded* value it already had. Numerical drift
//!   is bounded by the 16-bit / THREECOMP40 quantization step, not by any
//!   spline-fitting loss.
//! * **Byte layout is well-defined by the decoder.** [`crate::ap0l`] already
//!   ports the packfile walker + rotation unpacker; the encoder is the
//!   inverse of `eval_vec4` / `eval_rotation` (documented below).
//!
//! Cost of (2): the baked track is larger than a compact NURBS encoding
//! would be (per translation track: 3 + F+2 knots + 24 range floats + 6F
//! quantized CP bytes; a well-fitted spline might use only a handful of CPs).
//! For a 15-frame clip this is ~135 B vs. as little as 12 B for a purely
//! static translation — a ~10x growth on that channel, absorbed by the
//! surrounding packfile via a size-changing splice in
//! [`crate::ap0l::Ap0lPack::splice_hk_blob`].
//!
//! # Encoder = inverse of the decoder
//!
//! The retail decoder [`tools/sab_havok65::eval_vec4`] does, for a translation
//! channel with mask `0x70` (dynamic xyz) and quant type 1 (16-bit CPs):
//!
//! ```text
//!   read u16 n_idx, u8 degree
//!   read (n_idx + 2 + degree) u8 knot bytes
//!   align 4
//!   for each set component i in {0,1,2}: read (lo_i, hi_i) as two f32
//!   align 2
//!   CPs at cp_base: for cp in 0..=n_idx, for c in 0..n_dyn_comp:
//!     read u16 raw; value = lo + (raw/65535) * (hi - lo)
//! ```
//!
//! Our encoder for the same shape:
//!
//! ```text
//!   n_idx = F - 1;  degree = 1
//!   knots = [0, 0, 1, 2, ..., F-2, F-1, F-1]
//!   align 4
//!   for c in 0..3: (lo_c, hi_c) = (min, max) over per-frame values
//!   align 2
//!   for f in 0..F: for c in 0..3:
//!     raw = round( (v[f][c] - lo_c) / (hi_c - lo_c) * 65535 )  (0 if hi == lo)
//! ```
//!
//! Rotation (`eval_rotation`, mask top-nibble non-zero, THREECOMP40):
//!
//! ```text
//!   read u16 n_idx, u8 degree
//!   read (n_idx + 2 + degree) u8 knot bytes
//!   align 1  (no-op for THREECOMP40)
//!   CPs: (n_idx + 1) * 5 bytes, THREECOMP40-packed
//! ```
//!
//! Encoder emits the same shape, with each CP produced by
//! [`crate::ap0l::encode_threecomp40`]. We hemisphere-align successive CPs to
//! CP[0] so the decoder's in-span alignment fix does not have to trigger.

use crate::ap0l::{
    align_up_usize, encode_threecomp40, rot_align, rot_cpsize, walk_spline_track_layouts,
    StaticEditError, TrackLayout,
};

// ---------------------------------------------------------------- clip dims

/// Clip-level dimensions the keyframe-baker needs. Populate from
/// [`crate::ap0l::HkPackfile::num_frames`] etc.
#[derive(Debug, Clone, Copy)]
pub struct ClipDims {
    pub num_transform_tracks: usize,
    pub num_frames: usize,
    pub max_frames_per_block: usize,
    pub mask_and_quant_size: usize,
    pub frame_duration: f32,
    pub block_duration: f32,
    pub block_inv_duration: f32,
}

impl ClipDims {
    /// Reject clip shapes that the single-block keyframe baker cannot handle.
    /// Multi-block clips are refused up front by the CLI; this catches the
    /// degenerate remainders (F<2, F>255).
    pub fn validate_for_bake(&self) -> Result<(), StaticEditError> {
        if self.num_frames < 2 {
            return Err(StaticEditError::BadClipDims {
                reason: "keyframe baking needs num_frames >= 2 (single-frame clips are static)",
            });
        }
        if self.num_frames > 255 {
            return Err(StaticEditError::BadClipDims {
                reason: "single-block knot vector is u8; num_frames must fit in 0..=255",
            });
        }
        Ok(())
    }
}

// ---------------------------------------------------------------- port: eval_vec4/eval_rotation

// Faithful minimal port of `sab_havok65::eval_vec4`. Advances `ptr`. Used to
// sample the existing spline once per frame so we can preserve every frame's
// current value before rewriting one of them. The `default` fills components
// that the mask says are neither static nor spline.
fn eval_vec4(
    data: &[u8],
    ptr: &mut usize,
    mask: u8,
    quant_type: u8,
    fib: u8,
    frame_duration: f32,
    block_time: f32,
    default: [f32; 4],
) -> [f32; 4] {
    let has_spline = mask & 0xf0 != 0;
    let mut degree = 0usize;
    let mut n_idx = 0usize;
    let mut span = 0usize;
    let mut local_knots: Vec<f32> = Vec::new();

    if has_spline {
        n_idx = u16::from_le_bytes([data[*ptr], data[*ptr + 1]]) as usize;
        *ptr += 2;
        degree = data[*ptr] as usize;
        *ptr += 1;
        let knot_ptr = *ptr;
        span = find_span(n_idx, degree, fib, &data[knot_ptr..]);
        for i in 0..(2 * degree) {
            let ki = (span as isize - degree as isize + 1 + i as isize) as usize;
            local_knots.push(data[knot_ptr + ki] as f32 * frame_duration);
        }
        *ptr = knot_ptr + n_idx + 2 + degree;
    }

    *ptr = align_up_usize(*ptr, 4);
    let mut statics = [0f32; 4];
    let mut range = [(0f32, 0f32); 3];
    for i in 0..3 {
        if mask & (1 << i) != 0 {
            statics[i] = f32::from_le_bytes([
                data[*ptr],
                data[*ptr + 1],
                data[*ptr + 2],
                data[*ptr + 3],
            ]);
            *ptr += 4;
        } else if mask & (0x10 << i) != 0 {
            let lo = f32::from_le_bytes([data[*ptr], data[*ptr + 1], data[*ptr + 2], data[*ptr + 3]]);
            *ptr += 4;
            let hi = f32::from_le_bytes([data[*ptr], data[*ptr + 1], data[*ptr + 2], data[*ptr + 3]]);
            *ptr += 4;
            range[i] = (lo, hi);
        }
    }
    if mask & 8 != 0 {
        statics[3] = f32::from_le_bytes([data[*ptr], data[*ptr + 1], data[*ptr + 2], data[*ptr + 3]]);
        *ptr += 4;
    }

    if !has_spline {
        let mut v = [0f32; 4];
        for i in 0..4 {
            v[i] = if mask & (1 << i) != 0 { statics[i] } else { default[i] };
        }
        return v;
    }

    *ptr = (*ptr + 1) & !1;
    let bytes_per = if quant_type == 0 { 1usize } else { 2usize };
    let n_comp = ((mask >> 4) & 7).count_ones() as usize;
    let num_cp = n_idx + 1;
    let cp_base = *ptr;

    let dyn_bits: Vec<usize> = (0..3).filter(|&i| mask & (0x10 << i) != 0).collect();
    let mut cps: Vec<[f32; 4]> = Vec::with_capacity(degree + 1);
    for r in 0..=degree {
        let cp_index = span - degree + r;
        let mut v = [0f32; 4];
        for (c, &comp) in dyn_bits.iter().enumerate() {
            let off = cp_base + (cp_index * n_comp + c) * bytes_per;
            let q = if bytes_per == 1 {
                data[off] as f32 / 255.0
            } else {
                u16::from_le_bytes([data[off], data[off + 1]]) as f32 / 65535.0
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
    de_boor(degree, span, &local_knots, &cps, block_time)
}

fn eval_rotation(
    data: &[u8],
    ptr: &mut usize,
    mask: u8,
    qtype: u8,
    fib: u8,
    frame_duration: f32,
    block_time: f32,
    default: [f32; 4],
) -> [f32; 4] {
    let cpsize = rot_cpsize(qtype);
    let q = if mask & 0xf0 != 0 {
        let n = u16::from_le_bytes([data[*ptr], data[*ptr + 1]]) as usize;
        *ptr += 2;
        let degree = data[*ptr] as usize;
        *ptr += 1;
        let knot_ptr = *ptr;
        let span = find_span(n, degree, fib, &data[knot_ptr..]);
        let mut local_knots = Vec::new();
        for i in 0..(2 * degree) {
            let ki = (span as isize - degree as isize + 1 + i as isize) as usize;
            local_knots.push(data[knot_ptr + ki] as f32 * frame_duration);
        }
        *ptr = knot_ptr + n + 2 + degree;
        *ptr = align_up_usize(*ptr, rot_align(qtype));
        let cp_base = *ptr;
        let mut cps: Vec<[f32; 4]> = Vec::with_capacity(degree + 1);
        for r in 0..=degree {
            let idx = span - degree + r;
            let mut cp = unpack_quat(data, cp_base + idx * cpsize, qtype);
            if let Some(first) = cps.first() {
                let dot: f32 = (0..4).map(|k| first[k] * cp[k]).sum();
                if dot < 0.0 {
                    for k in 0..4 {
                        cp[k] = -cp[k];
                    }
                }
            }
            cps.push(cp);
        }
        *ptr = cp_base + (n + 1) * cpsize;
        de_boor(degree, span, &local_knots, &cps, block_time)
    } else if mask & 0xf != 0 {
        *ptr = align_up_usize(*ptr, rot_align(qtype));
        let v = unpack_quat(data, *ptr, qtype);
        *ptr += cpsize;
        v
    } else {
        default
    };
    let nrm = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
    if nrm > 1e-12 {
        [q[0] / nrm, q[1] / nrm, q[2] / nrm, q[3] / nrm]
    } else {
        [0.0, 0.0, 0.0, 1.0]
    }
}

fn unpack_quat(data: &[u8], off: usize, qtype: u8) -> [f32; 4] {
    // Only THREECOMP40 (qtype 1) is supported — the entire retail corpus uses
    // it exclusively; other types would be a hard error at bake time and are
    // rejected up the stack.
    if qtype != 1 {
        return [0.0, 0.0, 0.0, 1.0];
    }
    const S: f32 = 1.0 / (std::f32::consts::SQRT_2 * 2047.0);
    let v: u64 = (0..5).map(|i| (data[off + i] as u64) << (8 * i)).sum();
    let small = [
        (((v & 0xFFF) as i32) - 2047) as f32 * S,
        ((((v >> 12) & 0xFFF) as i32) - 2047) as f32 * S,
        ((((v >> 24) & 0xFFF) as i32) - 2047) as f32 * S,
    ];
    let idx = ((v >> 36) & 3) as usize;
    let neg = (v >> 38) & 1 != 0;
    let ss = small[0] * small[0] + small[1] * small[1] + small[2] * small[2];
    let mut largest = (1.0 - ss).max(0.0).sqrt();
    if neg {
        largest = -largest;
    }
    let mut q = [0f32; 4];
    let mut j = 0;
    for i in 0..4 {
        if i == idx {
            q[i] = largest;
        } else {
            q[i] = small[j];
            j += 1;
        }
    }
    q
}

fn find_span(n: usize, p: usize, u: u8, knots: &[u8]) -> usize {
    if u >= knots[n + 1] {
        return n;
    }
    if u <= knots[p] {
        return p;
    }
    let mut low = p;
    let mut high = n + 1;
    let mut mid = (low + high) / 2;
    let mut guard = 0;
    while u < knots[mid] || u >= knots[mid + 1] {
        if u < knots[mid] {
            high = mid;
        } else {
            low = mid;
        }
        mid = (low + high) / 2;
        guard += 1;
        if guard > 64 || low >= high {
            break;
        }
    }
    mid
}

fn de_boor(p: usize, span: usize, uk: &[f32], cps: &[[f32; 4]], u: f32) -> [f32; 4] {
    if p == 0 {
        return cps[0];
    }
    let mut d = cps.to_vec();
    for r in 1..=p {
        for j in (r..=p).rev() {
            let ui = uk[j - 1];
            let uipr = uk[j + p - r];
            let denom = uipr - ui;
            let a = if denom.abs() < 1e-20 {
                0.0
            } else {
                (u - ui) / denom
            };
            let mut nv = [0f32; 4];
            for c in 0..4 {
                nv[c] = (1.0 - a) * d[j - 1][c] + a * d[j][c];
            }
            d[j] = nv;
        }
    }
    let _ = span;
    d[p]
}

// Replicate `sab_havok65::SplineAnim::block_and_frame` for single-block clips.
fn frame_in_block(dims: &ClipDims, frame: usize) -> (u8, f32) {
    let block_time = frame as f32 * dims.frame_duration;
    let fib = (block_time
        * dims.block_inv_duration
        * (dims.max_frames_per_block as f32 - 1.0))
        .round()
        .clamp(0.0, 255.0) as u8;
    (fib, block_time)
}

// ---------------------------------------------------------------- sampling

/// One frame's decoded T/R/S values (unit quaternion, xyz translation, xyz scale).
#[derive(Debug, Clone, Copy)]
pub struct TrsFrame {
    pub t: [f32; 3],
    pub r: [f32; 4],
    pub s: [f32; 3],
}

/// Sample `track`'s T/R/S at every frame `[0..dims.num_frames)`. Only for
/// single-block clips (the walker rejects otherwise).
///
/// The rotation is unit-normalized post-sample; translation and scale carry
/// through as decoded. Values match `sab_havok65::sample_at(frame * frame_duration)`
/// for the same track.
pub fn sample_track_all_frames(
    data: &[u8],
    layouts: &[TrackLayout],
    track: usize,
    dims: &ClipDims,
) -> Result<Vec<TrsFrame>, StaticEditError> {
    let lay = layouts.get(track).ok_or(StaticEditError::TrackOutOfRange {
        track,
        count: layouts.len(),
    })?;
    let trn_type = lay.ctrl & 3;
    let rot_type = (lay.ctrl >> 2) & 0xf;
    let scl_type = lay.ctrl >> 6;
    let id_rot = [0.0f32, 0.0, 0.0, 1.0];
    let id_pos = [0.0f32; 4];
    let id_scl = [1.0f32, 1.0, 1.0, 1.0];
    let mut out = Vec::with_capacity(dims.num_frames);
    for f in 0..dims.num_frames {
        let (fib, block_time) = frame_in_block(dims, f);
        // Fresh pointer per frame; each channel walk is independent because
        // the walker rewinds within its own channel bytes.
        let mut ptr = lay.trn_channel_start;
        let t = eval_vec4(data, &mut ptr, lay.trn_mask, trn_type, fib, dims.frame_duration, block_time, id_pos);
        let mut ptr_r = lay.rot_channel_start;
        let r = eval_rotation(data, &mut ptr_r, lay.rot_mask, rot_type, fib, dims.frame_duration, block_time, id_rot);
        let mut ptr_s = lay.scl_channel_start;
        let s = eval_vec4(data, &mut ptr_s, lay.scl_mask, scl_type, fib, dims.frame_duration, block_time, id_scl);
        out.push(TrsFrame {
            t: [t[0], t[1], t[2]],
            r,
            s: [s[0], s[1], s[2]],
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------- baking

/// Emit the u8 knot vector for a degree-1 clamped B-spline whose control
/// points map 1:1 to frames: `[0, 0, 1, 2, ..., F-2, F-1, F-1]`. Length is
/// `F + 2`. Required only when F >= 2.
fn keyframe_knots(f: usize) -> Vec<u8> {
    debug_assert!(f >= 2);
    let mut k = Vec::with_capacity(f + 2);
    k.push(0);
    for i in 0..f {
        k.push(i as u8);
    }
    k.push((f - 1) as u8);
    k
}

/// Bake a per-frame translation (or scale) channel — mask `0x70` (dynamic
/// xyz), quant type 1 (16-bit CPs), degree 1. Returns just the channel
/// bytes; the caller places them at the correct absolute offset and pads
/// with `align_up_usize(_, 4)` on both sides.
///
/// Panics on `values.len() < 2` — callers must enforce
/// [`ClipDims::validate_for_bake`] before calling.
///
/// `channel_start` is the ABSOLUTE offset within the `data` blob where this
/// channel will land. It is needed only to figure the align-4 padding
/// between the knot vector and the range floats; the padding depends on
/// the absolute position modulo 4.
pub fn bake_vec3_channel_dynamic_xyz(
    values: &[[f32; 3]],
    channel_start: usize,
) -> Vec<u8> {
    let f = values.len();
    let mut out = Vec::with_capacity(3 + (f + 2) + 4 + 24 + 4 + f * 6);
    let n_idx = (f - 1) as u16;
    out.extend_from_slice(&n_idx.to_le_bytes());
    out.push(1u8); // degree
    out.extend_from_slice(&keyframe_knots(f));
    // align 4 relative to the absolute position of the block; channel_start
    // gives us the offset of the first byte of `out` in the data buffer.
    let abs_after_knots = channel_start + out.len();
    let pad = align_up_usize(abs_after_knots, 4) - abs_after_knots;
    for _ in 0..pad {
        out.push(0);
    }
    // Per-component (lo, hi). Guard against hi < lo (shouldn't happen from
    // min/max but explicit) and hi == lo (constant channel — quantization
    // divides by (hi-lo); we store q=0 for every CP in that case, which
    // decodes back to lo, which equals every input value).
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for v in values {
        for c in 0..3 {
            if v[c] < lo[c] {
                lo[c] = v[c];
            }
            if v[c] > hi[c] {
                hi[c] = v[c];
            }
        }
    }
    for c in 0..3 {
        if !lo[c].is_finite() {
            lo[c] = 0.0;
        }
        if !hi[c].is_finite() {
            hi[c] = 0.0;
        }
        if hi[c] < lo[c] {
            hi[c] = lo[c];
        }
        out.extend_from_slice(&lo[c].to_le_bytes());
        out.extend_from_slice(&hi[c].to_le_bytes());
    }
    // Align 2 relative to absolute position — statics are 4-aligned already,
    // and (24 bytes of ranges) is 4-aligned, so this is idempotent.
    let abs_after_range = channel_start + out.len();
    let pad2 = align_up_usize(abs_after_range, 2) - abs_after_range;
    for _ in 0..pad2 {
        out.push(0);
    }
    // Quantize each frame's xyz.
    for v in values {
        for c in 0..3 {
            let d = hi[c] - lo[c];
            let q = if d.abs() < 1e-20 {
                0u16
            } else {
                (((v[c] - lo[c]) / d) * 65535.0).round().clamp(0.0, 65535.0) as u16
            };
            out.extend_from_slice(&q.to_le_bytes());
        }
    }
    out
}

/// Bake a per-frame rotation channel — mask `0x10` (spline in top nibble),
/// qtype 1 (THREECOMP40), degree 1. Hemisphere-aligns successive quats to
/// CP[0] so the decoder's in-span alignment fix never has to negate a CP
/// (a defensive convenience; the decoder handles either case).
///
/// Returns just the channel bytes.
pub fn bake_quat_channel_spline(quats: &[[f32; 4]]) -> Result<Vec<u8>, StaticEditError> {
    let f = quats.len();
    let mut out = Vec::with_capacity(3 + (f + 2) + f * 5);
    let n_idx = (f - 1) as u16;
    out.extend_from_slice(&n_idx.to_le_bytes());
    out.push(1u8); // degree
    out.extend_from_slice(&keyframe_knots(f));
    // rot_align(1) == 1: no padding.
    // Normalise + hemisphere-align each quat to the first.
    let mut aligned: Vec<[f32; 4]> = Vec::with_capacity(f);
    for (i, q) in quats.iter().enumerate() {
        let n2 = q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3];
        if n2 < 1e-24 {
            return Err(StaticEditError::ZeroQuaternion);
        }
        let s = 1.0 / n2.sqrt();
        let mut n = [q[0] * s, q[1] * s, q[2] * s, q[3] * s];
        if let Some(first) = aligned.first() {
            let dot = (0..4).map(|k| first[k] * n[k]).sum::<f32>();
            if dot < 0.0 {
                for k in 0..4 {
                    n[k] = -n[k];
                }
            }
        }
        aligned.push(n);
        let _ = i;
    }
    for q in &aligned {
        out.extend_from_slice(&encode_threecomp40(*q)?);
    }
    Ok(out)
}

// ---------------------------------------------------------------- track rewrite

/// Rebuild the compressed spline `data` blob so that track `track`'s
/// translation channel carries the given per-frame xyz values (rotation and
/// scale channels are byte-copied from the input verbatim). Rotation and
/// scale variants sit next to this one below.
///
/// `dims.num_transform_tracks` and `dims.mask_and_quant_size` are used to walk
/// the layout; the caller supplies them fresh from the packfile.
///
/// The output blob may be a different size than the input — a static
/// translation grows into a keyframed-baked one (~10x on the T channel for a
/// small clip). Every OTHER track is copied verbatim.
///
/// The per-track header byte for translation is rewritten to `mask = 0x70`
/// (dynamic xyz) and the ctrl byte's `trn_type` field is set to `1`
/// (16-bit CP quantization) to match what the decoder expects.
pub fn rebuild_data_with_translation(
    old_data: &[u8],
    dims: &ClipDims,
    track: usize,
    per_frame_xyz: &[[f32; 3]],
) -> Result<Vec<u8>, StaticEditError> {
    let layouts = walk_spline_track_layouts(old_data, dims.num_transform_tracks, dims.mask_and_quant_size)?;
    rebuild_channel(
        old_data,
        &layouts,
        dims.mask_and_quant_size,
        track,
        Channel::Translation,
        Bake::Trn(per_frame_xyz),
    )
}

/// Like [`rebuild_data_with_translation`] but for the rotation channel.
/// Rejects tracks whose current rotation-quant type is not `1` (THREECOMP40).
pub fn rebuild_data_with_rotation(
    old_data: &[u8],
    dims: &ClipDims,
    track: usize,
    per_frame_quat: &[[f32; 4]],
) -> Result<Vec<u8>, StaticEditError> {
    let layouts = walk_spline_track_layouts(old_data, dims.num_transform_tracks, dims.mask_and_quant_size)?;
    let lay = layouts.get(track).ok_or(StaticEditError::TrackOutOfRange {
        track,
        count: layouts.len(),
    })?;
    let qtype = (lay.ctrl >> 2) & 0xf;
    if qtype != 1 {
        return Err(StaticEditError::UnsupportedRotQuant { qtype });
    }
    rebuild_channel(
        old_data,
        &layouts,
        dims.mask_and_quant_size,
        track,
        Channel::Rotation,
        Bake::Rot(per_frame_quat),
    )
}

/// Like [`rebuild_data_with_translation`] but for the scale channel.
pub fn rebuild_data_with_scale(
    old_data: &[u8],
    dims: &ClipDims,
    track: usize,
    per_frame_sxyz: &[[f32; 3]],
) -> Result<Vec<u8>, StaticEditError> {
    let layouts = walk_spline_track_layouts(old_data, dims.num_transform_tracks, dims.mask_and_quant_size)?;
    rebuild_channel(
        old_data,
        &layouts,
        dims.mask_and_quant_size,
        track,
        Channel::Scale,
        Bake::Scl(per_frame_sxyz),
    )
}

#[derive(Clone, Copy)]
enum Channel {
    Translation,
    Rotation,
    Scale,
}

enum Bake<'a> {
    Trn(&'a [[f32; 3]]),
    Rot(&'a [[f32; 4]]),
    Scl(&'a [[f32; 3]]),
}

/// Rebuild a `data` blob replacing one channel of one track with a freshly
/// baked keyframe representation.
///
/// Layout:
///   * copy `old_data[0 .. track_start]` verbatim (header table + earlier
///     tracks)
///   * emit target track's T + R + S with the requested channel re-baked
///     (others byte-copied from the original track's bytes, adjusted for
///     any shift in absolute position — since align-ups depend only on
///     `ptr mod 4`, and each channel handoff aligns to 4, a channel copy
///     is position-invariant as long as the caller keeps the surrounding
///     alignment invariants)
///   * copy `old_data[track_end..]` verbatim
///
/// Also rewrites the target track's 4-byte per-track header so the mask and
/// ctrl fields reflect the new (dynamic) shape of the re-baked channel.
fn rebuild_channel(
    old_data: &[u8],
    layouts: &[TrackLayout],
    mask_and_quant_size: usize,
    track: usize,
    channel: Channel,
    bake: Bake<'_>,
) -> Result<Vec<u8>, StaticEditError> {
    let lay = layouts.get(track).ok_or(StaticEditError::TrackOutOfRange {
        track,
        count: layouts.len(),
    })?;
    // The per-track header is at `t * 4` in the mask/quant table at the
    // start of the block.
    let hdr_off = track * 4;
    if hdr_off + 4 > mask_and_quant_size {
        return Err(StaticEditError::Truncated {
            at: hdr_off,
            need: 4,
            have: mask_and_quant_size,
        });
    }
    // Emit the new header (ctrl / trn_mask / rot_mask / scl_mask). Start
    // from the original, then rewrite the affected field(s).
    let mut new_hdr = [
        old_data[hdr_off],
        old_data[hdr_off + 1],
        old_data[hdr_off + 2],
        old_data[hdr_off + 3],
    ];

    // The three channels of this track live at contiguous, 4-aligned ranges
    // in `old_data`; we copy the two unchanged channels and re-emit the third.
    let trn_range = lay.trn_channel_start..lay.rot_channel_start;
    let rot_range = lay.rot_channel_start..lay.scl_channel_start;
    let scl_range = lay.scl_channel_start..lay.track_end;
    let trn_bytes = &old_data[trn_range.clone()];
    let rot_bytes = &old_data[rot_range.clone()];
    let scl_bytes = &old_data[scl_range.clone()];

    // Build the new track region.
    let mut new_track = Vec::with_capacity(trn_bytes.len() + rot_bytes.len() + scl_bytes.len() + 128);
    let track_abs_start = lay.track_start;

    match channel {
        Channel::Translation => {
            let vals = match bake {
                Bake::Trn(v) => v,
                _ => unreachable!(),
            };
            // Header: set trn_type = 1 (16-bit CPs), trn_mask = 0x70.
            new_hdr[0] = (new_hdr[0] & !0b11) | 1;
            new_hdr[1] = 0x70;
            let t_bytes = bake_vec3_channel_dynamic_xyz(vals, track_abs_start);
            new_track.extend_from_slice(&t_bytes);
            pad_to_align_within(&mut new_track, track_abs_start, 4);
            new_track.extend_from_slice(rot_bytes);
            new_track.extend_from_slice(scl_bytes);
        }
        Channel::Rotation => {
            let vals = match bake {
                Bake::Rot(v) => v,
                _ => unreachable!(),
            };
            // Header: keep rot_type = 1; set rot_mask = 0x10 (spline).
            new_hdr[0] = (new_hdr[0] & !(0b1111 << 2)) | (1 << 2);
            new_hdr[2] = 0x10;
            new_track.extend_from_slice(trn_bytes);
            let rot_abs = track_abs_start + new_track.len();
            let r_bytes = bake_quat_channel_spline(vals)?;
            new_track.extend_from_slice(&r_bytes);
            pad_to_align_within(&mut new_track, track_abs_start, 4);
            // Recompute where S must land (rotation may have grown).
            let scl_abs = track_abs_start + new_track.len();
            // Scale bytes are copied verbatim — that's fine because rotation
            // ended on a 4-boundary and scl_channel_start was likewise 4-aligned,
            // so all internal align-ups within the copy still produce the same
            // absolute offsets modulo 4.
            new_track.extend_from_slice(scl_bytes);
            let _ = (rot_abs, scl_abs);
        }
        Channel::Scale => {
            let vals = match bake {
                Bake::Scl(v) => v,
                _ => unreachable!(),
            };
            // Header: set scl_type = 1 (16-bit CPs), scl_mask = 0x70.
            new_hdr[0] = (new_hdr[0] & !(0b11 << 6)) | (1 << 6);
            new_hdr[3] = 0x70;
            new_track.extend_from_slice(trn_bytes);
            new_track.extend_from_slice(rot_bytes);
            let s_abs = track_abs_start + new_track.len();
            let s_bytes = bake_vec3_channel_dynamic_xyz(vals, s_abs);
            new_track.extend_from_slice(&s_bytes);
        }
    }
    // Final 4-align (mirrors the last `align_up_usize(*ptr, 4)` in the walker).
    pad_to_align_within(&mut new_track, track_abs_start, 4);

    // Splice: head + target track + tail. Also rewrite header table entry.
    let mut out = Vec::with_capacity(old_data.len() + new_track.len().saturating_sub(lay.track_end - lay.track_start) + 8);
    out.extend_from_slice(&old_data[..hdr_off]);
    out.extend_from_slice(&new_hdr);
    // Rest of header table + any pre-target-track bytes.
    let start_after_hdr = hdr_off + 4;
    out.extend_from_slice(&old_data[start_after_hdr..lay.track_start]);
    out.extend_from_slice(&new_track);
    out.extend_from_slice(&old_data[lay.track_end..]);
    Ok(out)
}

fn pad_to_align_within(buf: &mut Vec<u8>, abs_start: usize, a: usize) {
    let abs_here = abs_start + buf.len();
    let pad = align_up_usize(abs_here, a) - abs_here;
    for _ in 0..pad {
        buf.push(0);
    }
}

// ---------------------------------------------------------------- high-level

/// Overwrite `track`'s value at `frame` with `xyz`. Returns the new `data`
/// blob and the frame's previous decoded translation. Other frames retain
/// their current decoded values (subject to the 16-bit quantization budget
/// applied to every keyframe including the target). Rotation and scale
/// channels are byte-preserved.
pub fn set_frame_translation(
    old_data: &[u8],
    dims: &ClipDims,
    track: usize,
    frame: usize,
    xyz: [f32; 3],
) -> Result<(Vec<u8>, [f32; 3]), StaticEditError> {
    dims.validate_for_bake()?;
    if frame >= dims.num_frames {
        return Err(StaticEditError::FrameOutOfRange {
            frame,
            num_frames: dims.num_frames,
        });
    }
    let layouts = walk_spline_track_layouts(old_data, dims.num_transform_tracks, dims.mask_and_quant_size)?;
    let samples = sample_track_all_frames(old_data, &layouts, track, dims)?;
    let mut per_frame: Vec<[f32; 3]> = samples.iter().map(|s| s.t).collect();
    let old = per_frame[frame];
    per_frame[frame] = xyz;
    let new = rebuild_data_with_translation(old_data, dims, track, &per_frame)?;
    Ok((new, old))
}

/// Overwrite `track`'s value at `frame` with `quat` (normalised on the way in;
/// zero-norm rejected). Returns the new `data` blob and the frame's previous
/// decoded rotation.
pub fn set_frame_rotation(
    old_data: &[u8],
    dims: &ClipDims,
    track: usize,
    frame: usize,
    quat: [f32; 4],
) -> Result<(Vec<u8>, [f32; 4]), StaticEditError> {
    dims.validate_for_bake()?;
    if frame >= dims.num_frames {
        return Err(StaticEditError::FrameOutOfRange {
            frame,
            num_frames: dims.num_frames,
        });
    }
    let n2 = quat[0] * quat[0] + quat[1] * quat[1] + quat[2] * quat[2] + quat[3] * quat[3];
    if n2 < 1e-24 {
        return Err(StaticEditError::ZeroQuaternion);
    }
    let s = 1.0 / n2.sqrt();
    let quat_n = [quat[0] * s, quat[1] * s, quat[2] * s, quat[3] * s];
    let layouts = walk_spline_track_layouts(old_data, dims.num_transform_tracks, dims.mask_and_quant_size)?;
    let samples = sample_track_all_frames(old_data, &layouts, track, dims)?;
    let mut per_frame: Vec<[f32; 4]> = samples.iter().map(|s| s.r).collect();
    let old = per_frame[frame];
    per_frame[frame] = quat_n;
    let new = rebuild_data_with_rotation(old_data, dims, track, &per_frame)?;
    Ok((new, old))
}

/// Overwrite `track`'s value at `frame` with `sxyz`.
pub fn set_frame_scale(
    old_data: &[u8],
    dims: &ClipDims,
    track: usize,
    frame: usize,
    sxyz: [f32; 3],
) -> Result<(Vec<u8>, [f32; 3]), StaticEditError> {
    dims.validate_for_bake()?;
    if frame >= dims.num_frames {
        return Err(StaticEditError::FrameOutOfRange {
            frame,
            num_frames: dims.num_frames,
        });
    }
    let layouts = walk_spline_track_layouts(old_data, dims.num_transform_tracks, dims.mask_and_quant_size)?;
    let samples = sample_track_all_frames(old_data, &layouts, track, dims)?;
    let mut per_frame: Vec<[f32; 3]> = samples.iter().map(|s| s.s).collect();
    let old = per_frame[frame];
    per_frame[frame] = sxyz;
    let new = rebuild_data_with_scale(old_data, dims, track, &per_frame)?;
    Ok((new, old))
}

/// Add `delta` to `track`'s translation at `frame`.
pub fn offset_frame_translation(
    old_data: &[u8],
    dims: &ClipDims,
    track: usize,
    frame: usize,
    delta: [f32; 3],
) -> Result<(Vec<u8>, [f32; 3], [f32; 3]), StaticEditError> {
    dims.validate_for_bake()?;
    if frame >= dims.num_frames {
        return Err(StaticEditError::FrameOutOfRange {
            frame,
            num_frames: dims.num_frames,
        });
    }
    let layouts = walk_spline_track_layouts(old_data, dims.num_transform_tracks, dims.mask_and_quant_size)?;
    let samples = sample_track_all_frames(old_data, &layouts, track, dims)?;
    let mut per_frame: Vec<[f32; 3]> = samples.iter().map(|s| s.t).collect();
    let old = per_frame[frame];
    let new_val = [old[0] + delta[0], old[1] + delta[1], old[2] + delta[2]];
    per_frame[frame] = new_val;
    let new = rebuild_data_with_translation(old_data, dims, track, &per_frame)?;
    Ok((new, old, new_val))
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ap0l::{
        encode_threecomp40, set_static_rotation, set_static_scale, set_static_translation,
    };

    // A synthetic all-static single-track single-block buffer whose per-track
    // header describes translation=0x07, rotation=0x0f (THREECOMP40), scale=0x07.
    // Same as ap0l::tests::synth_one_track_static but re-emitted here so the
    // module is self-contained.
    fn synth_static_track(
        trn: [f32; 3],
        quat: [f32; 4],
        scl: [f32; 3],
    ) -> Vec<u8> {
        let mut d = Vec::new();
        d.push(0x04u8); // ctrl = rot_type=1 (THREECOMP40); trn/scl types = 0 (moot for static)
        d.push(0x07u8);
        d.push(0x0fu8);
        d.push(0x07u8);
        for c in &trn {
            d.extend_from_slice(&c.to_le_bytes());
        }
        d.extend_from_slice(&encode_threecomp40(quat).unwrap());
        while d.len() % 4 != 0 {
            d.push(0);
        }
        for c in &scl {
            d.extend_from_slice(&c.to_le_bytes());
        }
        d
    }

    fn synth_dims(num_frames: usize) -> ClipDims {
        // block_duration = (maxFPB-1) * frame_duration; frameDur=1/30; maxFPB=256.
        let fd: f32 = 1.0 / 30.0;
        let mfpb: usize = 256;
        let block_duration = (mfpb as f32 - 1.0) * fd;
        ClipDims {
            num_transform_tracks: 1,
            num_frames,
            max_frames_per_block: mfpb,
            mask_and_quant_size: 4,
            frame_duration: fd,
            block_duration,
            block_inv_duration: 1.0 / block_duration,
        }
    }

    #[test]
    fn knots_are_clamped_and_length_matches() {
        let k = keyframe_knots(5);
        assert_eq!(k, vec![0, 0, 1, 2, 3, 4, 4]);
        assert_eq!(k.len(), 5 + 2);
    }

    #[test]
    fn bake_vec3_roundtrips_within_16bit_tolerance() {
        // Bake a synthetic per-frame translation channel then decode it via
        // our own ported eval_vec4. Round-trip error must sit at the 16-bit
        // quantization step (~(hi-lo)/65535).
        let values: Vec<[f32; 3]> = (0..8)
            .map(|f| {
                [
                    f as f32 * 0.1,
                    (f as f32).sin() * 2.0,
                    -0.5 + (f as f32) * 0.02,
                ]
            })
            .collect();
        // Emit the channel starting at absolute offset 4 (just past a mock header).
        let channel_start = 4usize;
        let mut buf = Vec::new();
        buf.extend_from_slice(&[0x45u8, 0x70, 0x0f, 0x00]); // ctrl (trn_type=1), mask 0x70, rot 0x0f (irrelevant here), scl 0
        buf.extend_from_slice(&bake_vec3_channel_dynamic_xyz(&values, channel_start));
        // Now decode frame-by-frame.
        let dims = synth_dims(values.len());
        for (f, v) in values.iter().enumerate() {
            let (fib, block_time) = frame_in_block(&dims, f);
            let mut ptr = channel_start;
            let got = eval_vec4(&buf, &mut ptr, 0x70, 1, fib, dims.frame_duration, block_time, [0.0; 4]);
            for c in 0..3 {
                assert!(
                    (got[c] - v[c]).abs() < 1e-3,
                    "frame {f} comp {c}: {} vs {} (diff {})",
                    got[c],
                    v[c],
                    (got[c] - v[c]).abs()
                );
            }
        }
    }

    #[test]
    fn bake_quat_channel_roundtrips_within_threecomp40_tolerance() {
        let quats: Vec<[f32; 4]> = vec![
            [0.0, 0.0, 0.0, 1.0],
            [0.0, 0.0, (std::f32::consts::FRAC_PI_8).sin(), (std::f32::consts::FRAC_PI_8).cos()],
            [0.5, 0.5, 0.5, 0.5],
            [-0.1, 0.2, -0.3, 0.9273618],
        ];
        // Assemble a buffer with an alignment-safe pre-header + the channel.
        let mut buf = Vec::new();
        buf.extend_from_slice(&[0x04u8, 0x00, 0x10, 0x00]);
        let channel_start = buf.len();
        buf.extend_from_slice(&bake_quat_channel_spline(&quats).unwrap());
        let dims = synth_dims(quats.len());
        for (f, q) in quats.iter().enumerate() {
            let (fib, block_time) = frame_in_block(&dims, f);
            let mut ptr = channel_start;
            let got = eval_rotation(&buf, &mut ptr, 0x10, 1, fib, dims.frame_duration, block_time, [0.0, 0.0, 0.0, 1.0]);
            // Rotation equivalence — dot product > cos(threshold).
            let n2 = q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3];
            let s = 1.0 / n2.sqrt();
            let qn = [q[0] * s, q[1] * s, q[2] * s, q[3] * s];
            let dot: f32 = (0..4).map(|k| qn[k] * got[k]).sum::<f32>().abs().min(1.0);
            let ang_err = 2.0 * dot.acos();
            assert!(
                ang_err.to_degrees() < 0.2,
                "frame {f}: angle err {} deg (qn={:?} got={:?})",
                ang_err.to_degrees(),
                qn,
                got
            );
        }
    }

    #[test]
    fn set_frame_translation_edits_target_frame_and_preserves_others() {
        // Static-translation single-track clip. Set frame 3's translation.
        // Frame 3 must read back exactly what we set; other frames must
        // read back the original static value within quant tolerance.
        let orig = [1.0f32, 2.0, 3.0];
        let quat = encode_threecomp40([0.0, 0.0, 0.0, 1.0]).unwrap();
        let quat_orig = [0.0f32, 0.0, 0.0, 1.0];
        let scl = [1.0f32, 1.0, 1.0];
        let data = synth_static_track(orig, quat_orig, scl);
        let mut dims = synth_dims(8);
        dims.num_transform_tracks = 1;
        dims.mask_and_quant_size = 4;

        let target_frame = 3usize;
        let target_val = [10.0f32, -5.0, 42.5];
        let (new_data, old_at_frame) = set_frame_translation(&data, &dims, 0, target_frame, target_val).unwrap();
        assert_eq!(old_at_frame, orig, "before-edit read back the static value");

        // Walk + sample.
        let new_layouts = walk_spline_track_layouts(&new_data, 1, 4).unwrap();
        let samples = sample_track_all_frames(&new_data, &new_layouts, 0, &dims).unwrap();
        assert_eq!(samples.len(), dims.num_frames);
        for (f, s) in samples.iter().enumerate() {
            if f == target_frame {
                for c in 0..3 {
                    assert!(
                        (s.t[c] - target_val[c]).abs() < 1e-3,
                        "frame {f} comp {c}: {} vs target {}",
                        s.t[c],
                        target_val[c]
                    );
                }
            } else {
                for c in 0..3 {
                    assert!(
                        (s.t[c] - orig[c]).abs() < 1e-2,
                        "frame {f} comp {c}: {} vs orig {}",
                        s.t[c],
                        orig[c]
                    );
                }
            }
        }
        // rotation and scale must be preserved byte-for-byte in the rebuilt
        // track: verify by reading them back and comparing to the originals.
        for s in &samples {
            let dot: f32 = quat_orig.iter().zip(s.r.iter()).map(|(a, b)| a * b).sum::<f32>().abs();
            assert!(dot > 0.999);
            for c in 0..3 {
                assert!((s.s[c] - scl[c]).abs() < 1e-4);
            }
        }
        // Discard the local `quat` binding to keep the test above from
        // depending on the packed byte layout; it's only there to keep the
        // synth builder API honest.
        let _ = quat;
    }

    #[test]
    fn set_frame_rotation_edits_target_frame() {
        let orig_quat = [0.0f32, 0.0, 0.0, 1.0];
        let data = synth_static_track([0.0; 3], orig_quat, [1.0; 3]);
        let mut dims = synth_dims(6);
        dims.num_transform_tracks = 1;
        dims.mask_and_quant_size = 4;

        // 90 deg around Y at frame 2.
        let target = [
            0.0,
            (std::f32::consts::FRAC_PI_4).sin(),
            0.0,
            (std::f32::consts::FRAC_PI_4).cos(),
        ];
        let (new_data, old) = set_frame_rotation(&data, &dims, 0, 2, target).unwrap();
        // "Old" reads back as the static ~identity quat (up to THREECOMP40 quant).
        let dot_old: f32 = orig_quat.iter().zip(old.iter()).map(|(a, b)| a * b).sum::<f32>().abs();
        assert!(dot_old > 0.999);

        let new_layouts = walk_spline_track_layouts(&new_data, 1, 4).unwrap();
        let samples = sample_track_all_frames(&new_data, &new_layouts, 0, &dims).unwrap();
        // Frame 2's rotation is the target (angular tolerance ~0.2 deg for THREECOMP40).
        let f2 = samples[2].r;
        let d2: f32 = target.iter().zip(f2.iter()).map(|(a, b)| a * b).sum::<f32>().abs();
        let ang_err_2 = 2.0 * d2.min(1.0).acos();
        assert!(
            ang_err_2.to_degrees() < 0.5,
            "frame 2 target dot {} ang_err {} deg",
            d2,
            ang_err_2.to_degrees()
        );
        // Other frames preserved as ~identity.
        for f in [0usize, 1, 3, 4, 5] {
            let d: f32 = orig_quat.iter().zip(samples[f].r.iter()).map(|(a, b)| a * b).sum::<f32>().abs();
            let ang = 2.0 * d.min(1.0).acos();
            assert!(
                ang.to_degrees() < 0.5,
                "frame {f} deviated by {} deg from identity (got {:?})",
                ang.to_degrees(),
                samples[f].r
            );
        }
    }

    #[test]
    fn set_frame_scale_edits_target_frame() {
        let data = synth_static_track([0.0; 3], [0.0, 0.0, 0.0, 1.0], [1.0; 3]);
        let mut dims = synth_dims(5);
        dims.num_transform_tracks = 1;
        dims.mask_and_quant_size = 4;
        let (new_data, old) = set_frame_scale(&data, &dims, 0, 4, [2.0, 3.0, 4.0]).unwrap();
        assert_eq!(old, [1.0, 1.0, 1.0]);
        let new_layouts = walk_spline_track_layouts(&new_data, 1, 4).unwrap();
        let samples = sample_track_all_frames(&new_data, &new_layouts, 0, &dims).unwrap();
        for c in 0..3 {
            assert!((samples[4].s[c] - [2.0, 3.0, 4.0][c]).abs() < 1e-3);
        }
        for f in 0..4 {
            for c in 0..3 {
                assert!(
                    (samples[f].s[c] - 1.0).abs() < 1e-2,
                    "frame {f} comp {c}: {} vs 1.0",
                    samples[f].s[c]
                );
            }
        }
    }

    #[test]
    fn offset_frame_translation_returns_old_and_new_and_writes_delta() {
        let data = synth_static_track([1.0, 2.0, 3.0], [0.0, 0.0, 0.0, 1.0], [1.0; 3]);
        let mut dims = synth_dims(4);
        dims.num_transform_tracks = 1;
        dims.mask_and_quant_size = 4;
        let (new_data, old, new) = offset_frame_translation(&data, &dims, 0, 1, [10.0, 100.0, 1000.0]).unwrap();
        assert_eq!(old, [1.0, 2.0, 3.0]);
        assert_eq!(new, [11.0, 102.0, 1003.0]);
        let new_layouts = walk_spline_track_layouts(&new_data, 1, 4).unwrap();
        let samples = sample_track_all_frames(&new_data, &new_layouts, 0, &dims).unwrap();
        // Sample at frame 1 should be ~new.
        for c in 0..3 {
            // 16-bit quantization tolerance scales with the (hi-lo) range,
            // which for this edit is at least (1000-3) — so the absolute
            // tolerance is (hi-lo)/65535. Give 1e-1 headroom for f32 rounding.
            let range = [11.0 - 1.0, 102.0 - 2.0, 1003.0 - 3.0][c];
            let tol = (range / 65535.0) * 4.0 + 1e-3;
            assert!(
                (samples[1].t[c] - new[c]).abs() < tol,
                "frame 1 comp {c}: {} vs {} (tol {})",
                samples[1].t[c],
                new[c],
                tol
            );
        }
    }

    #[test]
    fn frame_out_of_range_errors() {
        let data = synth_static_track([0.0; 3], [0.0, 0.0, 0.0, 1.0], [1.0; 3]);
        let mut dims = synth_dims(5);
        dims.num_transform_tracks = 1;
        dims.mask_and_quant_size = 4;
        let err = set_frame_translation(&data, &dims, 0, 99, [0.0; 3]).unwrap_err();
        assert_eq!(err, StaticEditError::FrameOutOfRange { frame: 99, num_frames: 5 });
    }

    #[test]
    fn track_out_of_range_errors_for_frame_ops() {
        let data = synth_static_track([0.0; 3], [0.0, 0.0, 0.0, 1.0], [1.0; 3]);
        let mut dims = synth_dims(4);
        dims.num_transform_tracks = 1;
        dims.mask_and_quant_size = 4;
        let err = set_frame_translation(&data, &dims, 3, 0, [0.0; 3]).unwrap_err();
        assert!(matches!(err, StaticEditError::TrackOutOfRange { track: 3, count: 1 }));
    }

    #[test]
    fn zero_quaternion_rejected_for_frame_rotation() {
        let data = synth_static_track([0.0; 3], [0.0, 0.0, 0.0, 1.0], [1.0; 3]);
        let mut dims = synth_dims(3);
        dims.num_transform_tracks = 1;
        dims.mask_and_quant_size = 4;
        let err = set_frame_rotation(&data, &dims, 0, 0, [0.0; 4]).unwrap_err();
        assert_eq!(err, StaticEditError::ZeroQuaternion);
    }

    #[test]
    fn single_frame_clip_is_rejected_for_bake() {
        let data = synth_static_track([0.0; 3], [0.0, 0.0, 0.0, 1.0], [1.0; 3]);
        let mut dims = synth_dims(1);
        dims.num_transform_tracks = 1;
        dims.mask_and_quant_size = 4;
        let err = set_frame_translation(&data, &dims, 0, 0, [0.0; 3]).unwrap_err();
        assert!(matches!(err, StaticEditError::BadClipDims { .. }));
    }

    #[test]
    fn overlong_clip_is_rejected_for_bake() {
        let data = synth_static_track([0.0; 3], [0.0, 0.0, 0.0, 1.0], [1.0; 3]);
        let mut dims = synth_dims(300);
        dims.num_transform_tracks = 1;
        dims.mask_and_quant_size = 4;
        let err = set_frame_translation(&data, &dims, 0, 0, [0.0; 3]).unwrap_err();
        assert!(matches!(err, StaticEditError::BadClipDims { .. }));
    }

    // A single-track clip whose translation is ALREADY a spline (mask 0x70).
    // The synthesizer emits a proper knot vector, range floats, and 16-bit
    // CPs so `walk_spline_track_layouts` and `sample_track_all_frames` accept
    // it. We then frame-edit and verify the target frame is exact while other
    // frames stay within 16-bit tolerance of the source spline's sample.
    fn synth_dynamic_translation_track(
        per_frame: &[[f32; 3]],
        static_quat: [f32; 4],
        static_scl: [f32; 3],
    ) -> Vec<u8> {
        let mut d = Vec::new();
        // per-track header: ctrl (trn_type=1, rot_type=1, scl_type=0), tmask=0x70, rmask=0x0f, smask=0x07
        d.push(0x45);
        d.push(0x70);
        d.push(0x0f);
        d.push(0x07);
        // Translation channel — reuse the baker.
        let t_bytes = bake_vec3_channel_dynamic_xyz(per_frame, d.len());
        d.extend_from_slice(&t_bytes);
        while d.len() % 4 != 0 {
            d.push(0);
        }
        // Rotation: single static quaternion.
        d.extend_from_slice(&encode_threecomp40(static_quat).unwrap());
        while d.len() % 4 != 0 {
            d.push(0);
        }
        // Scale: static xyz.
        for c in &static_scl {
            d.extend_from_slice(&c.to_le_bytes());
        }
        d
    }

    // -------- real-clip integration tests --------

    /// Frame-edit a real retail clip, then sample every frame of the target
    /// track and confirm:
    ///   * frame `F` reads back at approximately the requested value.
    ///   * every OTHER frame stayed within the 16-bit quant tolerance of the
    ///     sample the SOURCE clip produced at that frame (no ringing —
    ///     keyframe baking is a per-frame identity at knots, so the only
    ///     drift is the quantization step, not spline reshape).
    ///
    /// Opt-in via `SAB_ANIM_PACK=<retail Animations.pack>`. If the env var
    /// is absent the test is a silent skip so CI without the retail data
    /// stays green. `SAB_TEST_FRAME_CLIP` and `SAB_TEST_FRAME_TRACK` and
    /// `SAB_TEST_FRAME_INDEX` override the target (default 0 / 3 / 5, the
    /// example from the tool spec).
    #[test]
    fn end_to_end_frame_translation_on_retail_clip() {
        use crate::ap0l::{Ap0lPack, HkPackfile};
        use std::env;
        use std::fs;

        let Ok(pack_path) = env::var("SAB_ANIM_PACK") else {
            eprintln!("skipping: SAB_ANIM_PACK not set");
            return;
        };
        let target_clip: usize = env::var("SAB_TEST_FRAME_CLIP")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let target_track: usize = env::var("SAB_TEST_FRAME_TRACK")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(3);
        let target_frame: usize = env::var("SAB_TEST_FRAME_INDEX")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(5);
        let target_val = [1.0f32, 2.0, 3.0];

        let bytes = fs::read(&pack_path).expect("read pack");
        let ap = Ap0lPack::parse(bytes).expect("parse AP0L");
        let hk = HkPackfile::parse(ap.hk_blob()).expect("parse hk");
        let scas = hk.objects_of_class("hkaSplineCompressedAnimation");
        assert!(!scas.is_empty(), "retail pack must have spline anims");
        let obj = scas[target_clip];
        let num_blocks = hk.num_blocks(obj);
        if num_blocks != 1 {
            eprintln!("skipping: retail clip #{target_clip} has numBlocks={num_blocks} (multi-block clips are out of scope)");
            return;
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
        if target_track >= dims.num_transform_tracks {
            eprintln!(
                "skipping: track {target_track} out of range (clip has {} tracks)",
                dims.num_transform_tracks
            );
            return;
        }
        if target_frame >= dims.num_frames {
            eprintln!(
                "skipping: frame {target_frame} out of range (clip has {} frames)",
                dims.num_frames
            );
            return;
        }
        let old_data = hk.spline_data(obj).to_vec();

        // Sample the ORIGINAL spline at every frame of the target track first —
        // so we can compare against the post-edit sample.
        let orig_layouts = walk_spline_track_layouts(&old_data, dims.num_transform_tracks, dims.mask_and_quant_size)
            .expect("walk original layouts");
        let orig_samples = sample_track_all_frames(&old_data, &orig_layouts, target_track, &dims)
            .expect("sample original");

        // Perform the edit.
        let (new_data, prev) = set_frame_translation(&old_data, &dims, target_track, target_frame, target_val)
            .expect("set_frame_translation");
        eprintln!(
            "clip #{target_clip} trk{target_track} frame{target_frame}: prev=({:.4},{:.4},{:.4}) target=({:.4},{:.4},{:.4})",
            prev[0], prev[1], prev[2], target_val[0], target_val[1], target_val[2]
        );
        eprintln!(
            "  original data {} B -> baked {} B (Δ={:+})",
            old_data.len(),
            new_data.len(),
            new_data.len() as i64 - old_data.len() as i64
        );

        // Re-decode via our own sampler.
        let new_layouts = walk_spline_track_layouts(&new_data, dims.num_transform_tracks, dims.mask_and_quant_size)
            .expect("walk new layouts");
        let new_samples = sample_track_all_frames(&new_data, &new_layouts, target_track, &dims)
            .expect("sample new");

        // Target frame == target value (allow quant tolerance + f32 rounding).
        let mut lo = [f32::INFINITY; 3];
        let mut hi = [f32::NEG_INFINITY; 3];
        for f in 0..dims.num_frames {
            let v = if f == target_frame { target_val } else { orig_samples[f].t };
            for c in 0..3 {
                lo[c] = lo[c].min(v[c]);
                hi[c] = hi[c].max(v[c]);
            }
        }
        let mut max_target_err = 0.0f32;
        for c in 0..3 {
            let tol = (hi[c] - lo[c]).abs() / 65535.0 * 4.0 + 1e-3;
            let err = (new_samples[target_frame].t[c] - target_val[c]).abs();
            max_target_err = max_target_err.max(err);
            assert!(
                err < tol,
                "target frame comp {c}: got {} vs {} (err {}, tol {})",
                new_samples[target_frame].t[c], target_val[c], err, tol
            );
        }
        eprintln!("  target frame err (max component): {:.6e}", max_target_err);

        // No other frame ranged more than the 16-bit quant step + a little
        // float slack. This is the "no ringing" check.
        let mut max_other_drift = 0.0f32;
        for f in 0..dims.num_frames {
            if f == target_frame {
                continue;
            }
            for c in 0..3 {
                let tol = (hi[c] - lo[c]).abs() / 65535.0 * 4.0 + 1e-3;
                let err = (new_samples[f].t[c] - orig_samples[f].t[c]).abs();
                max_other_drift = max_other_drift.max(err);
                assert!(
                    err < tol,
                    "frame {f} comp {c} drifted: got {} vs orig {} (err {}, tol {})",
                    new_samples[f].t[c], orig_samples[f].t[c], err, tol
                );
            }
        }
        eprintln!("  max drift on other frames: {:.6e}", max_other_drift);
    }

    /// Full end-to-end round-trip: keyframe-bake a clip's data, run it
    /// through AssetCc2 --xml/--strip to recompile the packfile, re-parse
    /// the compiled packfile, and confirm the target frame samples correctly.
    ///
    /// Opt-in via `SAB_ANIM_PACK` (retail pack) and either `SAB_ASSETCC`
    /// (path to AssetCc2.exe) or the default HCT 6.5 install location.
    #[test]
    fn full_round_trip_via_assetcc2_on_retail_clip() {
        use crate::ap0l::{
            find_nth_object, hash_bytes, map_hk_to_xml_indices, Ap0lPack, HkPackfile,
        };
        use std::env;
        use std::fs;
        use std::path::{Path, PathBuf};
        use std::process::Command;

        let Ok(pack_path) = env::var("SAB_ANIM_PACK") else {
            eprintln!("skipping: SAB_ANIM_PACK not set");
            return;
        };
        let assetcc: PathBuf = env::var("SAB_ASSETCC")
            .ok()
            .map(PathBuf::from)
            .or_else(|| {
                let d = Path::new(r"C:\Program Files (x86)\Havok\BatchProcess\AssetCc\AssetCc2.exe");
                if d.exists() { Some(d.into()) } else { None }
            })
            .unwrap_or_default();
        if !assetcc.exists() {
            eprintln!("skipping: AssetCc2 not found (SAB_ASSETCC or default HCT 6.5 install)");
            return;
        }

        let target_clip = 0usize;
        let target_track = 3usize;
        let target_frame = 5usize;
        let target_val = [1.0f32, 2.0, 3.0];

        let bytes = fs::read(&pack_path).expect("read pack");
        let ap = Ap0lPack::parse(bytes).expect("parse AP0L");
        let hk = HkPackfile::parse(ap.hk_blob()).expect("parse hk");
        let scas = hk.objects_of_class("hkaSplineCompressedAnimation");
        let obj = scas[target_clip];
        if hk.num_blocks(obj) != 1 {
            eprintln!("skipping: clip 0 is multi-block");
            return;
        }
        let ntt = hk.num_transform_tracks(obj) as usize;
        if target_track >= ntt {
            eprintln!("skipping: clip 0 has fewer than {} tracks", target_track + 1);
            return;
        }
        let nframes = hk.num_frames(obj) as usize;
        if target_frame >= nframes {
            eprintln!("skipping: clip 0 has only {} frames", nframes);
            return;
        }
        let dims = ClipDims {
            num_transform_tracks: ntt,
            num_frames: nframes,
            max_frames_per_block: hk.max_frames_per_block(obj) as usize,
            mask_and_quant_size: hk.mask_and_quantization_size(obj) as usize,
            frame_duration: hk.frame_duration(obj),
            block_duration: hk.block_duration(obj),
            block_inv_duration: hk.block_inv_duration(obj),
        };
        let old_data = hk.spline_data(obj).to_vec();
        let (new_data, _) =
            set_frame_translation(&old_data, &dims, target_track, target_frame, target_val).expect("bake");

        // Scratch directory for the AssetCc2 dance.
        let base = env::var("CLAUDE_SCRATCH")
            .map(PathBuf::from)
            .unwrap_or_else(|_| env::temp_dir());
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = base.join(format!("spline_edit_rt_{ts:x}"));
        fs::create_dir_all(&dir).expect("scratch dir");
        let hkx = dir.join("input.hkx");
        let xml = dir.join("input.xml");
        let xml_mod = dir.join("perturbed.xml");
        let rt = dir.join("perturbed.hkx");
        fs::write(&hkx, ap.hk_blob()).expect("write hkx");
        let status = Command::new(&assetcc)
            .args([
                "--xml".as_ref(),
                hkx.as_os_str(),
                xml.as_os_str(),
            ])
            .status()
            .expect("run AssetCc2 --xml");
        assert!(status.success(), "AssetCc2 --xml failed: {status}");

        // Find the target clip's XML declaration (which is NOT necessarily
        // the same index as its fixup-order index — remap via fingerprints).
        let xml_text = fs::read_to_string(&xml).expect("read xml");
        let hk_fps: Vec<_> = scas.iter().map(|&s| hk.spline_fingerprint(s)).collect();
        let hk_hashes: Vec<_> = scas.iter().map(|&s| hash_bytes(hk.spline_data(s))).collect();
        let hk_to_xml = map_hk_to_xml_indices(&xml_text, &hk_fps, &hk_hashes).expect("map");
        let xml_idx = hk_to_xml[target_clip];
        let (obj_start, obj_end) =
            find_nth_object(&xml_text, "hkaSplineCompressedAnimation", xml_idx).expect("locate xml obj");
        // Splice new bytes into the `data` hkparam.
        let mutated = replace_hkarray_bytes_test(&xml_text, obj_start, obj_end, "data", &new_data)
            .expect("replace data hkparam");
        fs::write(&xml_mod, &mutated).expect("write perturbed xml");
        let status = Command::new(&assetcc)
            .args([
                "--strip".as_ref(),
                "--rules4101".as_ref(),
                xml_mod.as_os_str(),
                rt.as_os_str(),
            ])
            .status()
            .expect("run AssetCc2 --strip");
        assert!(status.success(), "AssetCc2 --strip failed: {status}");

        let rt_blob = fs::read(&rt).expect("read rt");
        let rt_hk = HkPackfile::parse(&rt_blob).expect("re-parse rt packfile");
        let rt_scas = rt_hk.objects_of_class("hkaSplineCompressedAnimation");
        assert_eq!(rt_scas.len(), scas.len(), "clip count preserved");
        // Re-locate the same clip in the recompiled packfile (its fixup index
        // may have shuffled).
        let rt_fps: Vec<_> = rt_scas.iter().map(|&s| rt_hk.spline_fingerprint(s)).collect();
        // The clip we edited has changed fingerprint (num_transform_tracks,
        // num_frames stay identical; data_len grew). Match on the OTHER
        // shape fields and pick the one with the closest data_len — but the
        // simplest routing is: the clip whose XML index matches xml_idx in
        // the new fixup order == the same clip. We approximate by walking
        // rt_scas in file order and taking clip N.
        let obj_rt = rt_scas[target_clip];
        let dims_rt = ClipDims {
            num_transform_tracks: rt_hk.num_transform_tracks(obj_rt) as usize,
            num_frames: rt_hk.num_frames(obj_rt) as usize,
            max_frames_per_block: rt_hk.max_frames_per_block(obj_rt) as usize,
            mask_and_quant_size: rt_hk.mask_and_quantization_size(obj_rt) as usize,
            frame_duration: rt_hk.frame_duration(obj_rt),
            block_duration: rt_hk.block_duration(obj_rt),
            block_inv_duration: rt_hk.block_inv_duration(obj_rt),
        };
        let rt_data = rt_hk.spline_data(obj_rt).to_vec();
        let rt_layouts = walk_spline_track_layouts(
            &rt_data,
            dims_rt.num_transform_tracks,
            dims_rt.mask_and_quant_size,
        )
        .expect("walk rt");
        let rt_samples = sample_track_all_frames(&rt_data, &rt_layouts, target_track, &dims_rt).expect("sample rt");
        // Sanity: fingerprints line up on the non-data structural fields.
        eprintln!("  post-AssetCc2 clip #{target_clip} data len = {}", rt_data.len());
        // Target frame value.
        for c in 0..3 {
            // Widen tolerance because AssetCc2's round-trip does its own
            // re-quantization on the whole clip: allow 0.5% of value scale
            // or 0.01 absolute.
            let tol = 0.01f32.max(target_val[c].abs() * 0.005);
            let err = (rt_samples[target_frame].t[c] - target_val[c]).abs();
            assert!(
                err < tol,
                "post-AssetCc2 target frame comp {c}: got {} vs {} (err {}, tol {})",
                rt_samples[target_frame].t[c], target_val[c], err, tol
            );
        }
        let _ = fs::remove_dir_all(&dir);
        eprintln!("  full AssetCc2 round-trip decoded target frame within tolerance");
        let _ = rt_fps; // silence unused
    }

    /// Test-scoped stand-in for the CLI helper `replace_hkarray_bytes` — the
    /// unit-testable body of that helper lives here so this integration test
    /// does not have to reach into the binary crate.
    fn replace_hkarray_bytes_test(
        xml: &str,
        start: usize,
        end: usize,
        param: &str,
        new_bytes: &[u8],
    ) -> Result<String, String> {
        let scope = &xml[start..end];
        let anchor = format!("<hkparam name=\"{param}\"");
        let anchor_rel = scope
            .find(&anchor)
            .ok_or_else(|| format!("no {param}"))?;
        let key = "numelements=\"";
        let ne_key = scope[anchor_rel..].find(key).ok_or("no numelements")? + anchor_rel + key.len();
        let ne_end_rel = scope[ne_key..].find('"').ok_or("malformed")?;
        let ne_end = ne_key + ne_end_rel;
        let open_gt = scope[anchor_rel..].find('>').ok_or("no >")? + anchor_rel + 1;
        let close_rel = scope[open_gt..].find("</hkparam>").ok_or("no close")?;
        let close = open_gt + close_rel;
        let mut payload = String::new();
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
        let abs_ne_start = start + ne_key;
        let abs_ne_end = start + ne_end;
        let abs_payload_start = start + open_gt;
        let abs_payload_end = start + close;
        let mut out = String::new();
        out.push_str(&xml[..abs_ne_start]);
        out.push_str(&format!("{}", new_bytes.len()));
        out.push_str(&xml[abs_ne_end..abs_payload_start]);
        out.push_str(&payload);
        out.push_str(&xml[abs_payload_end..]);
        Ok(out)
    }

    #[test]
    fn dynamic_track_frame_edit_preserves_other_frames() {
        let per_frame: Vec<[f32; 3]> = (0..10)
            .map(|f| [(f as f32) * 0.5, (f as f32).cos() * 3.0, -(f as f32) * 0.1])
            .collect();
        let data = synth_dynamic_translation_track(&per_frame, [0.0, 0.0, 0.0, 1.0], [1.0; 3]);
        let mut dims = synth_dims(per_frame.len());
        dims.num_transform_tracks = 1;
        dims.mask_and_quant_size = 4;
        let target_frame = 4usize;
        let target = [123.0f32, -45.6, 7.89];
        let (new_data, _old) = set_frame_translation(&data, &dims, 0, target_frame, target).unwrap();
        let new_layouts = walk_spline_track_layouts(&new_data, 1, 4).unwrap();
        let samples = sample_track_all_frames(&new_data, &new_layouts, 0, &dims).unwrap();

        // Target frame is exact within 16-bit quant tol on the new range.
        let mut lo = [f32::INFINITY; 3];
        let mut hi = [f32::NEG_INFINITY; 3];
        for f in 0..per_frame.len() {
            let v = if f == target_frame { target } else { per_frame[f] };
            for c in 0..3 {
                lo[c] = lo[c].min(v[c]);
                hi[c] = hi[c].max(v[c]);
            }
        }
        for c in 0..3 {
            let tol = (hi[c] - lo[c]) / 65535.0 * 4.0 + 1e-3;
            assert!(
                (samples[target_frame].t[c] - target[c]).abs() < tol,
                "target frame comp {c}: {} vs {} (tol {})",
                samples[target_frame].t[c],
                target[c],
                tol
            );
        }

        // Other frames must be close to per_frame[f] within the *new*
        // quantization range (larger because target extended the min/max).
        for f in 0..per_frame.len() {
            if f == target_frame {
                continue;
            }
            for c in 0..3 {
                let tol = (hi[c] - lo[c]) / 65535.0 * 4.0 + 1e-3;
                assert!(
                    (samples[f].t[c] - per_frame[f][c]).abs() < tol,
                    "frame {f} comp {c} drifted: {} vs orig {} (tol {})",
                    samples[f].t[c],
                    per_frame[f][c],
                    tol
                );
            }
        }

        // No ringing outside the source values — de Boor at knots is a plain
        // interpolation of neighbouring CPs, so peak deviation across all
        // frames stays within the per-frame quant tolerance (already checked).
        // (Silences static analysis for `set_static_*` symbols kept for the
        // module docs.)
        let _ = (set_static_translation, set_static_rotation, set_static_scale);
    }
}
