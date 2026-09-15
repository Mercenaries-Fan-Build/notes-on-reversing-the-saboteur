//! hkaSplineCompressedAnimation decoder for The Saboteur (Havok 6.5.0-r1).
//!
//! Pipeline: Animations.pack (AP0L) -> concatenated animations.hkx (Havok
//! packfile, magic 0x57e0e057) -> parse sections/classnames/fixups -> find
//! `hkaSplineCompressedAnimation` instances -> decode per-frame hkQsTransforms.
//!
//! Every structural constant is cross-checked against the Ghidra decomp
//! (function VAs in comments) and confirmed against real bytes at runtime.
//! std-only, no external crates.
//!
//! ## Provenance
//! Derived by a double-blind protocol: two investigators independently RE'd the
//! format from `Saboteur.exe` + real `Animations.pack` bytes (both decoded the
//! whole corpus, 0 failures), then a third agent adjudicated the two points
//! their unit-norm validation could not distinguish, resolving them against a
//! direct disassembly of `Saboteur.exe`:
//!   * `unpack_quat` type-1 (THREECOMP40): index=bits36-37, SIGN at bit 38,
//!     dequant `(raw-2047)/(2047*sqrt2)` — per `FUN_00f22470` (0x00f224ae..).
//!   * time model: continuous block-time de Boor with frameDuration-scaled knots,
//!     integer findSpan — per `FUN_00eb8120`/`FUN_00eb65a0`/`FUN_00eb6420`.
//! See docs/formats/animation_havok65.md and memory havok65-spline-decode.
//!
//! ## Verified vs open
//! CONFIRMED against the whole corpus: AP0L carve, packfile layout, struct
//! offsets, channel order (translation/rotation/scale), THREECOMP40, NURBS/de Boor.
//! NOT yet exercised (corpus is uniform ctrl=0x45, single-block): the multi-block
//! path, and rotation quant types 0/2/3/4/5 + 16-bit translation/scale. Those
//! paths are structurally present but need out-of-corpus data or a live capture.

// The struct mirrors the on-disk Havok layout; some fields/helpers document the
// format without being read on the hot path.
#![allow(dead_code)]

use std::env;
use std::fs;

mod gltf;
mod hct_validate;
mod trs_validate;

// Expose primary THREECOMP40 unpacker so `trs_validate` can cross-check
// its independent reference implementation against it (see the
// `threecomp40_ref_vs_primary_random_sample` unit test).
pub fn unpack_quat_pub(data: &[u8], off: usize, qtype: u8) -> [f32; 4] {
    unpack_quat(data, off, qtype)
}

// ---------- little-endian helpers ----------
fn u16le(b: &[u8], o: usize) -> u16 { u16::from_le_bytes([b[o], b[o + 1]]) }
fn u32le(b: &[u8], o: usize) -> u32 { u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]) }
fn i32le(b: &[u8], o: usize) -> i32 { i32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]) }
fn f32le(b: &[u8], o: usize) -> f32 { f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]) }
fn find_sub(hay: &[u8], n: &[u8]) -> Option<usize> {
    hay.windows(n.len()).position(|w| w == n)
}

// ---------- Havok packfile (LE, 32-bit) ----------
struct Packfile<'a> {
    blob: &'a [u8],           // whole hkx blob (magic at 0)
    data_pk: usize,           // abs offset of __data__ body in blob
    names: std::collections::HashMap<usize, String>, // classnames-body-rel off -> name
    lf: std::collections::HashMap<usize, usize>,      // src(data-rel) -> dst(data-rel)
    vfixups: Vec<(usize, String)>,                    // (obj src data-rel, class)
    version: String,
}

impl<'a> Packfile<'a> {
    fn parse(blob: &'a [u8]) -> Result<Self, String> {
        // header @0: magic0,magic1,userTag,fileVer,layout[4],numSec,...
        if u32le(blob, 0) != 0x57e0_e057 { return Err("bad havok magic".into()); }
        let sh = find_sub(blob, b"__classnames__").ok_or("no __classnames__")?;
        // 3 section headers: 20-byte name + 7 u32 [abs, lf, gf, vf, exp, imp, end]
        let mut secs = [[0u32; 7]; 3];
        for s in 0..3 {
            for k in 0..7 { secs[s][k] = u32le(blob, sh + s * 48 + 20 + k * 4); }
        }
        let body0 = sh + 3 * 48;
        let cn_len = secs[0][6] as usize;          // classnames body length (=end)
        let data_pk = body0 + secs[0][6] as usize + secs[1][6] as usize;
        // classnames
        let mut names = std::collections::HashMap::new();
        let cn_end = (body0 + cn_len).min(blob.len());
        let mut p = body0;
        while p + 5 <= cn_end {
            if u32le(blob, p) == 0xFFFF_FFFF { break; }
            let mut q = p + 5;
            while q < cn_end && blob[q] != 0 { q += 1; }
            if let Ok(nm) = std::str::from_utf8(&blob[p + 5..q]) {
                if !nm.is_empty() { names.insert(p + 5 - body0, nm.to_string()); }
            }
            p = q + 1;
        }
        // __data__ fixups (offsets are relative to data section start)
        let (d_lf, d_gf, d_vf, d_end) =
            (secs[2][1] as usize, secs[2][2] as usize, secs[2][3] as usize, secs[2][4] as usize);
        let mut lf = std::collections::HashMap::new();
        let mut k = data_pk + d_lf;
        let lf_end = data_pk + d_gf;
        while k + 8 <= lf_end {
            let src = u32le(blob, k);
            if src == 0xFFFF_FFFF { break; }
            lf.insert(src as usize, u32le(blob, k + 4) as usize);
            k += 8;
        }
        let mut vfixups = Vec::new();
        let mut k = data_pk + d_vf;
        let vf_end = data_pk + d_end;
        while k + 12 <= vf_end {
            let src = u32le(blob, k) as usize;
            let cnoff = u32le(blob, k + 8) as usize;
            if src == 0xFFFF_FFFF { break; }
            k += 12;
            vfixups.push((src, names.get(&cnoff).cloned().unwrap_or_else(|| "?".into())));
        }
        let version = find_sub(blob, b"Havok-").map(|o| {
            let mut q = o;
            while q < blob.len() && blob[q] != 0 && blob[q].is_ascii_graphic() { q += 1; }
            String::from_utf8_lossy(&blob[o..q]).into_owned()
        }).unwrap_or_default();
        Ok(Packfile { blob, data_pk, names, lf, vfixups, version })
    }
    // resolve an hkArray (ptr,size,capflags @ obj_rel field) -> (abs data offset|None, size)
    fn array(&self, obj_src: usize, field_off: usize) -> (Option<usize>, usize) {
        let size = u32le(self.blob, self.data_pk + obj_src + field_off + 4) as usize;
        let dst = self.lf.get(&(obj_src + field_off)).map(|d| self.data_pk + d);
        (dst, size)
    }
}

// ---------- AP0L pack ----------
struct Ap0l { blob_off: usize, num_anims: u32, hk_size: usize }
fn parse_ap0l(file: &[u8]) -> Result<Ap0l, String> {
    if &file[0..4] != b"L0PA" { return Err("not AP0L".into()); }
    // The concatenated animations.hkx begins at the first Havok magic; the two
    // u32 immediately before it are (numAnims, hkSize) per anim_extract.cpp.
    let magic = [0x57u8, 0xe0, 0xe0, 0x57];
    let blob_off = find_sub(file, &magic).ok_or("no havok magic in pack")?;
    let num_anims = u32le(file, blob_off - 8);
    let hk_size = u32le(file, blob_off - 4) as usize;
    Ok(Ap0l { blob_off, num_anims, hk_size })
}

// ---------- hkQsTransform ----------
#[derive(Clone, Copy, Debug)]
struct QsTransform { t: [f32; 4], r: [f32; 4], s: [f32; 4] }

// ---------- spline component evaluator (port of FUN_00eb73a0) ----------
// Reads one vec4 track (rotation or scale) at `ptr`, advancing it.
// mask       = pbVar4[1|2|3]  (bits0-3 static per comp x,y,z,w; bits4-6 spline x,y,z)
// quant_type = ctrl&3 / (ctrl>>2)&0xf / ctrl>>6  (0=8-bit CPs, else 16-bit CPs)
// default    = fallback vec4 for unset components
// Returns the 4 evaluated components.
fn eval_vec4(
    data: &[u8], ptr: &mut usize, mask: u8, quant_type: u8,
    frame_in_block: u8, frame_duration: f32, block_time: f32, default: [f32; 4],
) -> [f32; 4] {
    let has_spline = mask & 0xf0 != 0;
    let mut degree = 0usize;
    let mut n_idx = 0usize;             // n = numControlPoints-1 (u16 field)
    let mut span = 0usize;
    let mut local_knots: Vec<f32> = Vec::new();

    // --- knot vector (FUN_00eb65a0) ---
    if has_spline {
        n_idx = u16le(data, *ptr) as usize; *ptr += 2;   // *param_2
        degree = data[*ptr] as usize; *ptr += 1;         // *param_3
        let knot_ptr = *ptr;
        span = find_span(n_idx, degree, frame_in_block, &data[knot_ptr..]);
        // 2*degree local knots: U[span-degree+1 .. span+degree], scaled to seconds
        for i in 0..(2 * degree) {
            let ki = (span as isize - degree as isize + 1 + i as isize) as usize;
            local_knots.push(data[knot_ptr + ki] as f32 * frame_duration);
        }
        *ptr = knot_ptr + n_idx + 2 + degree;            // advance past knots
    }

    // --- per-component static value / spline range (float32) ---
    // component is: static(bit i) -> 1 float; spline(bit 4+i) -> 2 floats (min,max)
    *ptr = (*ptr + 3) & !3;                               // align 4
    let mut statics = [0f32; 4];
    let mut range = [(0f32, 0f32); 3];
    for i in 0..3 {
        if mask & (1 << i) != 0 {
            statics[i] = f32le(data, *ptr); *ptr += 4;
        } else if mask & (0x10 << i) != 0 {
            let lo = f32le(data, *ptr); *ptr += 4;
            let hi = f32le(data, *ptr); *ptr += 4;
            range[i] = (lo, hi);
        }
    }
    if mask & 8 != 0 { statics[3] = f32le(data, *ptr); *ptr += 4; } // comp w static (rare)

    // --- static/default only: no spline ---
    if !has_spline {
        return assemble_static_default(mask, &statics, &default);
    }

    // --- quantized control points (align 2) ---
    *ptr = (*ptr + 1) & !1;
    let bytes_per = if quant_type == 0 { 1usize } else { 2usize };
    let n_comp = ((mask >> 4) & 7).count_ones() as usize; // dynamic comp count
    let num_cp = n_idx + 1;
    let cp_base = *ptr;

    // Read the (degree+1) local control points P[span-degree .. span].
    // Each control point is a full vec4: spline comps dequantized, static/default filled.
    let dyn_bits: Vec<usize> = (0..3).filter(|&i| mask & (0x10 << i) != 0).collect();
    let mut cps: Vec<[f32; 4]> = Vec::with_capacity(degree + 1);
    for r in 0..=degree {
        let cp_index = span - degree + r;
        let mut v = [0f32; 4];
        // dynamic comps
        for (c, &comp) in dyn_bits.iter().enumerate() {
            let off = cp_base + (cp_index * n_comp + c) * bytes_per;
            let q = if bytes_per == 1 { data[off] as f32 / 255.0 }
                    else { u16le(data, off) as f32 / 65535.0 };
            let (lo, hi) = range[comp];
            v[comp] = lo + q * (hi - lo);
        }
        // static & default comps (constant across CPs -> preserved by de Boor)
        for i in 0..4 {
            if mask & (1 << i) != 0 { v[i] = statics[i]; }
            else if (mask & (0x10 << i)) == 0 { v[i] = default[i]; } // not spline, not static
        }
        cps.push(v);
    }
    // advance main pointer past ALL control points (num_cp of them)
    *ptr = cp_base + num_cp * n_comp * bytes_per;

    // --- de Boor evaluation at u = block_time ---
    de_boor(degree, span, &local_knots, &cps, block_time)
}

// Port of FUN_00eb72c0: ROTATION channel. Quaternion CPs are packed
// (THREECOMP40 = 5 bytes for quant type 1) and read via FUN_00eb6640 + the
// PTR_FUN_011de950 unpacker table, then blended by de Boor and re-normalized.
fn eval_rotation(
    data: &[u8], ptr: &mut usize, mask: u8, qtype: u8,
    fib: u8, frame_duration: f32, block_time: f32, default: [f32; 4],
) -> [f32; 4] {
    let cpsize = rot_cpsize(qtype);
    let q = if mask & 0xf0 != 0 {
        // spline of quaternions
        let n = u16le(data, *ptr) as usize; *ptr += 2;
        let degree = data[*ptr] as usize; *ptr += 1;
        let knot_ptr = *ptr;
        let span = find_span(n, degree, fib, &data[knot_ptr..]);
        let mut local_knots = Vec::new();
        for i in 0..(2 * degree) {
            let ki = (span as isize - degree as isize + 1 + i as isize) as usize;
            local_knots.push(data[knot_ptr + ki] as f32 * frame_duration);
        }
        if std::env::var("TRACE").is_ok() && (n > 500 || degree > 8) {
            eprintln!("   ROT spline knot_ptr=0x{:x} n={} degree={} span={} mask=0x{:02x}",
                knot_ptr, n, degree, span, mask);
        }
        *ptr = knot_ptr + n + 2 + degree;
        *ptr = align_up(*ptr, rot_align(qtype));
        let cp_base = *ptr;
        let mut cps: Vec<[f32; 4]> = Vec::with_capacity(degree + 1);
        for r in 0..=degree {
            let idx = span - degree + r;
            let mut cp = unpack_quat(data, cp_base + idx * cpsize, qtype);
            // hemisphere-align to first CP so component-wise blend is valid
            if let Some(first) = cps.first() {
                let dot: f32 = (0..4).map(|k| first[k] * cp[k]).sum();
                if dot < 0.0 { for k in 0..4 { cp[k] = -cp[k]; } }
            }
            cps.push(cp);
        }
        *ptr = cp_base + (n + 1) * cpsize;
        de_boor(degree, span, &local_knots, &cps, block_time)
    } else if mask & 0xf != 0 {
        // single static packed quaternion
        *ptr = align_up(*ptr, rot_align(qtype));
        let v = unpack_quat(data, *ptr, qtype);
        *ptr += cpsize;
        v
    } else {
        default
    };
    // normalize (de Boor of unit quats is only approximately unit)
    let nrm = (q[0]*q[0] + q[1]*q[1] + q[2]*q[2] + q[3]*q[3]).sqrt();
    if nrm > 1e-12 { [q[0]/nrm, q[1]/nrm, q[2]/nrm, q[3]/nrm] } else { [0.0,0.0,0.0,1.0] }
}

fn align_up(x: usize, a: usize) -> usize { (x + a - 1) & !(a - 1) }
// DAT_0109dad0[type] (CP byte size). Confirmed type 1 = 5 (THREECOMP40).
// type map (public Havok RotationQuantization): 0=POLAR32(4) 1=THREECOMP40(5)
// 2=THREECOMP48(6) 3=THREECOMP24(3). Only type 1 confirmed against Saboteur data.
fn rot_cpsize(t: u8) -> usize { match t { 0 => 4, 1 => 5, 2 => 6, 3 => 3, _ => 5 } }
fn rot_align(_t: u8) -> usize { 1 }

// PTR_FUN_011de950[type]: unpack one packed quaternion -> (x,y,z,w).
// THREECOMP40 (type 1) — faithful to FUN_00f22470, disassembled from Saboteur.exe
// (Ghidra skipped this FPU-heavy function). 40-bit little-endian word `v`:
//   bits [0:12),[12:24),[24:36) : 3 "small" components, raw in [0,4095]
//       value = (raw - 2047) / (2047 * sqrt2)   (offset 2047 @0x00f22505; scale
//       1/(2047*sqrt2) from consts 2.0/2047.0 @0x00f2247a..)
//   bits [36:38)                : index of the omitted (largest) component
//   bit  38                     : sign of the largest (fchs @0x00f225a3); bit 39 unused
//   largest = ±sqrt(1 - sum(small^2))   (@0x00f2256e..)
// NOTE: the sign bit and the (raw-2047)/(2047*sqrt2) dequant are the adjudicated
// corrections; unit-norm alone cannot detect a wrong sign (mirrored rotation).
fn unpack_quat(data: &[u8], off: usize, qtype: u8) -> [f32; 4] {
    match qtype {
        1 => {
            const S: f32 = 1.0 / (std::f32::consts::SQRT_2 * 2047.0);
            let v: u64 = (0..5).map(|i| (data[off + i] as u64) << (8 * i)).sum();
            let small = [
                (((v & 0xFFF) as i32) - 2047) as f32 * S,
                ((((v >> 12) & 0xFFF) as i32) - 2047) as f32 * S,
                ((((v >> 24) & 0xFFF) as i32) - 2047) as f32 * S,
            ];
            let idx = ((v >> 36) & 3) as usize;
            let neg = (v >> 38) & 1 != 0;
            let s = small[0] * small[0] + small[1] * small[1] + small[2] * small[2];
            let mut largest = (1.0 - s).max(0.0).sqrt();
            if neg { largest = -largest; }
            let mut q = [0f32; 4];
            let mut j = 0;
            for i in 0..4 {
                if i == idx { q[i] = largest; } else { q[i] = small[j]; j += 1; }
            }
            q
        }
        _ => [0.0, 0.0, 0.0, 1.0], // POLAR32/THREECOMP48/24/STRAIGHT16/UNCOMPRESSED: not in corpus
    }
}

// FUN_00eb80a0 semantics: fill vec4 from static (bit i) else default.
fn assemble_static_default(mask: u8, statics: &[f32; 4], default: &[f32; 4]) -> [f32; 4] {
    let mut v = [0f32; 4];
    for i in 0..4 {
        v[i] = if mask & (1 << i) != 0 { statics[i] } else { default[i] };
    }
    v
}

// FUN_00eb6420: NURBS knot span search on u8 knots. n = numCP-1, p = degree.
// Robust Piegl-Tiller findSpan (bounded).
fn find_span(n: usize, p: usize, u: u8, knots: &[u8]) -> usize {
    if u >= knots[n + 1] { return n; }
    if u <= knots[p] { return p; }
    let mut low = p;
    let mut high = n + 1;
    let mut mid = (low + high) / 2;
    let mut guard = 0;
    while u < knots[mid] || u >= knots[mid + 1] {
        if u < knots[mid] { high = mid; } else { low = mid; }
        mid = (low + high) / 2;
        guard += 1;
        if guard > 64 || low >= high { break; }
    }
    mid
}

// Cox-de Boor evaluation. local_knots = U[span-p+1 .. span+p] (len 2p),
// cps = P[span-p .. span] (len p+1), evaluate spline at u.
fn de_boor(p: usize, span: usize, uk: &[f32], cps: &[[f32; 4]], u: f32) -> [f32; 4] {
    if p == 0 { return cps[0]; }
    let mut d = cps.to_vec(); // d[j] = P[span-p+j]
    for r in 1..=p {
        for j in (r..=p).rev() {
            // i = j + span - p ; alpha = (u - U[i]) / (U[i+p-r+1] - U[i])
            // U[i]        = uk[j-1]
            // U[i+p-r+1]  = uk[j + p - r]
            let ui = uk[j - 1];
            let uipr = uk[j + p - r];
            let denom = uipr - ui;
            let a = if denom.abs() < 1e-20 { 0.0 } else { (u - ui) / denom };
            let mut nv = [0f32; 4];
            for c in 0..4 { nv[c] = (1.0 - a) * d[j - 1][c] + a * d[j][c]; }
            d[j] = nv;
        }
    }
    let _ = span;
    d[p]
}

// ---------- clip decode ----------
struct SplineAnim {
    duration: f32,
    num_transform_tracks: usize,
    num_float_tracks: usize,
    num_frames: usize,
    num_blocks: usize,
    max_frames_per_block: usize,
    mask_and_quant_size: usize,
    block_duration: f32,
    block_inv_duration: f32,
    frame_duration: f32,
    // resolved arrays
    block_offsets: Vec<u32>,
    transform_offsets: Vec<u32>,
    data_off: usize,   // abs offset in blob of the data blob
    data_len: usize,
}

fn read_spline_anim(pk: &Packfile, obj_src: usize) -> SplineAnim {
    let b = pk.blob;
    let o = pk.data_pk + obj_src;
    let block_offsets = {
        let (dst, size) = pk.array(obj_src, 0x40);
        let mut v = Vec::new();
        if let Some(d) = dst { for i in 0..size { v.push(u32le(b, d + i * 4)); } }
        v
    };
    let transform_offsets = {
        let (dst, size) = pk.array(obj_src, 0x58);
        let mut v = Vec::new();
        if let Some(d) = dst { for i in 0..size { v.push(u32le(b, d + i * 4)); } }
        v
    };
    let (data_dst, data_len) = pk.array(obj_src, 0x70);
    SplineAnim {
        duration: f32le(b, o + 0x0c),
        num_transform_tracks: u32le(b, o + 0x10) as usize,
        num_float_tracks: u32le(b, o + 0x14) as usize,
        num_frames: u32le(b, o + 0x24) as usize,
        num_blocks: u32le(b, o + 0x28) as usize,
        max_frames_per_block: u32le(b, o + 0x2c) as usize,
        mask_and_quant_size: u32le(b, o + 0x30) as usize,
        block_duration: f32le(b, o + 0x34),
        block_inv_duration: f32le(b, o + 0x38),
        frame_duration: f32le(b, o + 0x3c),
        block_offsets,
        transform_offsets,
        data_off: data_dst.unwrap_or(0),
        data_len,
    }
}

impl SplineAnim {
    // Resolve (blockIndex, blockLocalTime, frameInBlock) — port of FUN_00eb8120.
    fn block_and_frame(&self, time: f32) -> (usize, f32, u8) {
        let bi = if self.num_blocks <= 1 { 0 }
                 else { ((time / self.block_duration).floor() as i64)
                        .clamp(0, self.num_blocks as i64 - 1) as usize };
        let block_time = time - bi as f32 * self.block_duration;
        let fib = (block_time * self.block_inv_duration * (self.max_frames_per_block as f32 - 1.0))
            .round().clamp(0.0, 255.0) as u8;
        (bi, block_time, fib)
    }

    // Decode all transform tracks at `time` (seconds).
    fn sample(&self, blob: &[u8]) -> Vec<Vec<QsTransform>> {
        let mut frames = Vec::new();
        let n = self.num_frames.max(1);
        for f in 0..n {
            let time = f as f32 * self.frame_duration;
            frames.push(self.sample_at(blob, time));
        }
        frames
    }

    fn sample_at(&self, blob: &[u8], time: f32) -> Vec<QsTransform> {
        let (bi, block_time, fib) = self.block_and_frame(time);
        let block_start = self.data_off + self.block_offsets.get(bi).copied().unwrap_or(0) as usize;
        let nt = self.num_transform_tracks;
        // Per-track 4-byte masks occupy the first nt*4 bytes; the packed spline
        // data begins at block_start + maskAndQuantizationSize (usually == nt*4,
        // but padded for large skeletons). CONFIRMED against real clips.
        let mut ptr = block_start + self.mask_and_quant_size;
        let mut out = Vec::with_capacity(nt);
        let id_rot = [0f32, 0f32, 0f32, 1f32];
        let id_pos = [0f32, 0f32, 0f32, 0f32];
        let id_scl = [1f32, 1f32, 1f32, 1f32];
        let trace = std::env::var("TRACE").is_ok();
        for t in 0..nt {
            let hdr = block_start + t * 4;
            let ctrl = blob[hdr];
            // per-track 4-byte header: [ctrl, transMask, rotMask, scaleMask]
            let trn_mask = blob[hdr + 1];
            let rot_mask = blob[hdr + 2];
            let scl_mask = blob[hdr + 3];
            let trn_type = ctrl & 3;
            let rot_type = (ctrl >> 2) & 0xf;
            let scl_type = ctrl >> 6;
            if trace {
                eprintln!("trk{:2} +0x{:x} ctrl=0x{:02x} tmask=0x{:02x} rmask=0x{:02x} smask=0x{:02x}",
                    t, ptr - block_start, ctrl, trn_mask, rot_mask, scl_mask);
            }
            // Channel order in the block: TRANSLATION, ROTATION, SCALE
            // (FUN_00eb7e00 -> 7880/7830/7930, writing hkQsTransform T@0/R@0x10/S@0x20).
            // Each channel decode is followed by 4-byte alignment.
            let t_ = eval_vec4(blob, &mut ptr, trn_mask, trn_type, fib, self.frame_duration, block_time, id_pos);
            ptr = (ptr + 3) & !3;
            let r = eval_rotation(blob, &mut ptr, rot_mask, rot_type, fib, self.frame_duration, block_time, id_rot);
            ptr = (ptr + 3) & !3;
            let s = eval_vec4(blob, &mut ptr, scl_mask, scl_type, fib, self.frame_duration, block_time, id_scl);
            ptr = (ptr + 3) & !3;
            out.push(QsTransform { t: t_, r, s });
        }
        out
    }
}

fn bulk_validate(pk: &Packfile, blob: &[u8], scas: &[usize]) {
    use std::panic;
    let mut ok = 0u32; let mut panicked = 0u32;
    let mut bad_quat = 0u32; let mut big_trans = 0u32; let mut multiblock = 0u32;
    let mut multiblock_examples = Vec::new();
    let mut frame_mismatch = 0u32;
    let mut worst_trans = 0f32;
    let mut ctrl_hist: std::collections::BTreeMap<u8, u32> = std::collections::BTreeMap::new();
    for (i, &src) in scas.iter().enumerate() {
        let r = panic::catch_unwind(panic::AssertUnwindSafe(|| {
            let a = read_spline_anim(pk, src);
            if a.num_blocks > 1 { return (true, a.num_blocks, 0u32, 0u32, 0f32, true); }
            // control-byte histogram from block 0
            let bs = a.data_off + a.block_offsets.get(0).copied().unwrap_or(0) as usize;
            let frames = a.sample(blob);
            let expect = if a.num_frames == 0 { 1 } else { a.num_frames };
            let fm = frames.len() != expect;
            let mut bq = 0u32; let mut wt = 0f32;
            for fr in &frames { for q in fr {
                let n = (q.r[0]*q.r[0]+q.r[1]*q.r[1]+q.r[2]*q.r[2]+q.r[3]*q.r[3]).sqrt();
                if !(n.is_finite() && (n-1.0).abs() < 1e-3) { bq += 1; }
                for c in 0..3 { let v = q.t[c].abs(); if v.is_finite() { wt = wt.max(v); } else { bq += 1; } }
            }}
            let _ = bs;
            (false, a.num_blocks, bq, if fm {1} else {0}, wt, true)
        }));
        match r {
            Ok((mb, nb, bq, fm, wt, _)) => {
                if mb { multiblock += 1; if multiblock_examples.len() < 5 { multiblock_examples.push((i, nb)); } continue; }
                let a = read_spline_anim(pk, src);
                let bs = a.data_off + a.block_offsets.get(0).copied().unwrap_or(0) as usize;
                for t in 0..a.num_transform_tracks { *ctrl_hist.entry(blob[bs + t*4]).or_insert(0) += 1; }
                if bq > 0 { bad_quat += 1; }
                if wt > 100.0 { big_trans += 1; }
                if fm > 0 { frame_mismatch += 1; }
                worst_trans = worst_trans.max(if wt.is_finite() {wt} else {0.0});
                if bq == 0 && fm == 0 { ok += 1; }
            }
            Err(_) => { panicked += 1;
                let a = read_spline_anim(pk, src);
                eprintln!("  PANIC clip #{} tracks={} frames={} blocks={} dataLen={} blockOff0={:?}",
                    i, a.num_transform_tracks, a.num_frames, a.num_blocks, a.data_len,
                    a.block_offsets.get(0));
            }
        }
    }
    println!("BULK VALIDATION over {} clips:", scas.len());
    println!("  fully-clean (unit quats, exact frame count): {}", ok);
    println!("  single-block decoded with quat/frame issue: bad_quat={} frame_mismatch={} big_trans(>100m)={}",
             bad_quat, frame_mismatch, big_trans);
    println!("  multi-block clips (block path): {}  examples(idx,nblocks)={:?}", multiblock, multiblock_examples);
    println!("  panicked: {}", panicked);
    println!("  worst finite |translation| among single-block: {:.2}m", worst_trans);
    println!("  distinct control bytes across all tracks: {}", ctrl_hist.len());
    let mut top: Vec<_> = ctrl_hist.iter().collect();
    top.sort_by(|a,b| b.1.cmp(a.1));
    print!("  top control bytes:");
    for (c,n) in top.iter().take(8) { print!(" 0x{:02x}={}", c, n); }
    println!();
}

fn main() {
    let args: Vec<String> = env::args().collect();

    // Plain static mesh (diagnostic; no pack): `sab_havok65 mesh <smsh> <out.glb>`
    if args.get(1).map(|s| s == "mesh").unwrap_or(false) {
        let smsh_path = args.get(2).map(|s| s.as_str()).unwrap_or("mesh.smsh");
        let out = args.get(3).map(|s| s.as_str()).unwrap_or("mesh.glb");
        let mesh = gltf::read_smsh(&fs::read(smsh_path).expect("read .smsh")).expect("parse SMSH");
        let glb = gltf::export_mesh_only(&mesh);
        fs::write(out, &glb).expect("write glb");
        println!("wrote {out}: plain mesh, {} verts, {} tris", mesh.positions.len(), mesh.indices.len() / 3);
        return;
    }

    // Cross-validate our decode against HCT 6.5's AssetCc2 XML emit:
    //   `sab_havok65 validate-hct <pack> <retail_anim.xml>`
    // See hct_validate.rs for the field-by-field diff.
    if args.get(1).map(|s| s == "validate-hct").unwrap_or(false) {
        let pack = args.get(2).map(|s| s.as_str())
            .unwrap_or("C:/GOG Games/The Saboteur/Animations.pack");
        let xml = args.get(3).map(|s| s.as_str())
            .expect("usage: sab_havok65 validate-hct <pack> <retail_anim.xml>");
        hct_validate::run(pack, xml);
        return;
    }

    // Per-frame TRS property harness: `sab_havok65 validate-trs <pack>`
    // Exercises the sampler math (Cox-de Boor, THREECOMP40, block-time
    // truncation) via mathematical property tests (convex-hull
    // confinement of de Boor, clamped-knot endpoint match, unit-quat,
    // static-track invariance, determinism). See trs_validate.rs for
    // why an external per-frame oracle was not achievable.
    if args.get(1).map(|s| s == "validate-trs").unwrap_or(false) {
        let pack = args.get(2).map(|s| s.as_str())
            .unwrap_or("C:/GOG Games/The Saboteur/Animations.pack");
        trs_validate::run(pack);
        return;
    }

    // Drill-down of the rotation-channel convex-hull soft overshoots
    // that `validate-trs` reports as informational-only.  For each
    // overshoot, recomputes the pre-normalization Cox-de Boor blend
    // through an independent implementation and prints the raw hull,
    // the pre-norm value, |pre|, and both the ACTUAL overshoot and the
    // one PREDICTED by pure post-blend renormalization.  If those two
    // agree and pre[c] itself stays inside the raw hull, the deviation
    // is a mathematical consequence of renormalizing a de Boor blend of
    // unit quaternions with |·|<1 — NOT a decoder bug.
    //   `sab_havok65 validate-trs-drill <pack> [clip_idx] [max_reports]`
    if args.get(1).map(|s| s == "validate-trs-drill").unwrap_or(false) {
        let pack = args.get(2).map(|s| s.as_str())
            .unwrap_or("C:/GOG Games/The Saboteur/Animations.pack");
        let target = args.get(3).and_then(|s| s.parse::<usize>().ok());
        let max_reports = args.get(4).and_then(|s| s.parse::<usize>().ok()).unwrap_or(200);
        trs_validate::drill(pack, target, max_reports);
        return;
    }

    // Skeleton-only export needs no animation pack: `sab_havok65 skeleton <skel> <out.glb>`
    if args.get(1).map(|s| s == "skeleton").unwrap_or(false) {
        let skel_path = args.get(2).map(|s| s.as_str()).unwrap_or("skeleton.skel");
        let out = args.get(3).map(|s| s.as_str()).unwrap_or("skeleton.glb");
        let skel_text = fs::read_to_string(skel_path).expect("read .skel");
        let skel = gltf::read_skel(&skel_text);
        let glb = gltf::export_skeleton_glb(&skel);
        fs::write(out, &glb).expect("write glb");
        println!("wrote {out}: {} bones (bind pose, no animation)", skel.len());
        return;
    }

    let pack_path = args.get(1).map(|s| s.as_str())
        .unwrap_or("C:/GOG Games/The Saboteur/Animations.pack");
    let want = args.get(2).and_then(|s| s.parse::<usize>().ok());

    let file = fs::read(pack_path).expect("read pack");
    let ap = parse_ap0l(&file).expect("parse AP0L");
    println!("AP0L: numAnims={} hkSize={} (0x{:x}) blobOff=0x{:x}",
             ap.num_anims, ap.hk_size, ap.hk_size, ap.blob_off);
    let blob = &file[ap.blob_off..ap.blob_off + ap.hk_size];
    let pk = Packfile::parse(blob).expect("parse packfile");
    println!("Havok version: {}  data_pk=0x{:x}", pk.version, pk.data_pk);

    let scas: Vec<usize> = pk.vfixups.iter()
        .filter(|(_, c)| c == "hkaSplineCompressedAnimation")
        .map(|(s, _)| *s).collect();
    println!("hkaSplineCompressedAnimation instances in main blob: {}\n", scas.len());

    if args.get(2).map(|s| s == "all").unwrap_or(false) {
        bulk_validate(&pk, blob, &scas);
        return;
    }

    // Export EVERY clip in the main blob: `sab_havok65 <pack> gltf-all <outdir>`
    if args.get(2).map(|s| s == "gltf-all").unwrap_or(false) {
        let outdir = args.get(3).map(|s| s.as_str()).unwrap_or("gltf_out");
        fs::create_dir_all(outdir).expect("create outdir");
        let mut ok = 0usize;
        let mut bytes = 0u64;
        for (i, &src) in scas.iter().enumerate() {
            let anim = read_spline_anim(&pk, src);
            let glb = gltf::export_glb(&anim, blob, None);
            let path = format!("{outdir}/clip_{i:04}.glb");
            fs::write(&path, &glb).expect("write glb");
            ok += 1;
            bytes += glb.len() as u64;
            if i % 250 == 0 { println!("  {i}/{} ...", scas.len()); }
        }
        println!("exported {ok} clips -> {outdir}  ({:.1} MB total)", bytes as f64 / 1e6);
        return;
    }

    // Plain mesh diagnostic: `sab_havok65 mesh <smsh> <out.glb>` (no pack needed — handled early).

    // FULL PREVIEW: skinned mesh + skeleton + animation, one glTF.
    // `sab_havok65 <pack> preview <index> <skel> <smsh> <out.glb> [trackmap]`
    if args.get(2).map(|s| s == "preview").unwrap_or(false) {
        let idx: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(0);
        let skel_path = args.get(4).map(|s| s.as_str()).unwrap_or("skeleton.skel");
        let smsh_path = args.get(5).map(|s| s.as_str()).unwrap_or("mesh.smsh");
        let out = args.get(6).map(|s| s.as_str()).unwrap_or("preview.glb");
        if idx >= scas.len() {
            eprintln!("clip index {idx} out of range (have {})", scas.len());
            return;
        }
        let skel = gltf::read_skel(&fs::read_to_string(skel_path).expect("read .skel"));
        let mesh = gltf::read_smsh(&fs::read(smsh_path).expect("read .smsh")).expect("parse SMSH");
        let track_to_bone: Vec<i32> = match args.get(7) {
            Some(p) => fs::read_to_string(p).expect("read trackmap")
                .split_whitespace().map(|t| t.parse::<i32>().unwrap_or(-1)).collect(),
            None => Vec::new(),
        };
        let anim = read_spline_anim(&pk, scas[idx]);
        let glb = gltf::export_preview(&anim, blob, &skel, &track_to_bone, &mesh);
        fs::write(out, &glb).expect("write glb");
        println!(
            "wrote {out}: clip #{idx} ({} tracks) on {} bones + skinned mesh ({} verts, {} tris)",
            anim.num_transform_tracks, skel.len(), mesh.positions.len(), mesh.indices.len() / 3
        );
        return;
    }

    // Rigged export: `sab_havok65 <pack> gltf-rigged <index> <skel> <out.glb> [trackmap]`
    // Nests the clip onto a skeleton (parent tree + bind pose). With a `trackmap`
    // (whitespace list of skeleton bone indices per track, -1 = unbound — the AP0L
    // ANIM bone list) each track drives its real bone; without it, positional.
    if args.get(2).map(|s| s == "gltf-rigged").unwrap_or(false) {
        let idx: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(0);
        let skel_path = args.get(4).map(|s| s.as_str()).unwrap_or("skeleton.skel");
        let out = args.get(5).map(|s| s.as_str()).unwrap_or("rigged.glb");
        if idx >= scas.len() {
            eprintln!("clip index {idx} out of range (have {})", scas.len());
            return;
        }
        let skel_text = fs::read_to_string(skel_path).expect("read .skel");
        let skel = gltf::read_skel(&skel_text);
        let track_to_bone: Vec<i32> = match args.get(6) {
            Some(p) => fs::read_to_string(p)
                .expect("read trackmap")
                .split_whitespace()
                .map(|t| t.parse::<i32>().unwrap_or(-1))
                .collect(),
            None => Vec::new(),
        };
        let anim = read_spline_anim(&pk, scas[idx]);
        let bound = if track_to_bone.is_empty() {
            anim.num_transform_tracks.min(skel.len())
        } else {
            track_to_bone.iter().filter(|&&b| b >= 0 && (b as usize) < skel.len()).count()
        };
        let glb = gltf::export_glb_rigged(&anim, blob, &skel, &track_to_bone);
        fs::write(out, &glb).expect("write glb");
        println!(
            "wrote {out}: clip #{idx} ({} tracks) -> {} bones, {bound} bound{}",
            anim.num_transform_tracks, skel.len(),
            if track_to_bone.is_empty() { " (positional)" } else { " (ANIM trackmap)" }
        );
        return;
    }

    // Export one clip to binary glTF: `sab_havok65 <pack> gltf <index> <out.glb>`
    if args.get(2).map(|s| s == "gltf" || s == "glb").unwrap_or(false) {
        let idx: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(0);
        let out = args.get(4).map(|s| s.as_str()).unwrap_or("clip.glb");
        if idx >= scas.len() {
            eprintln!("clip index {idx} out of range (have {})", scas.len());
            return;
        }
        let anim = read_spline_anim(&pk, scas[idx]);
        let glb = gltf::export_glb(&anim, blob, None);
        fs::write(out, &glb).expect("write glb");
        println!(
            "wrote {out} ({} bytes): clip #{idx}, {} tracks, {} frames, {:.3}s",
            glb.len(), anim.num_transform_tracks, anim.num_frames.max(1), anim.duration
        );
        return;
    }

    let indices: Vec<usize> = match want {
        Some(i) => vec![i],
        None => vec![0, 1, 2, 5, 10, 100],
    };

    for &idx in &indices {
        if idx >= scas.len() { continue; }
        let anim = read_spline_anim(&pk, scas[idx]);
        println!("=== clip #{} (obj @data+0x{:x}) ===", idx, scas[idx]);
        println!("  duration={:.4}s numFrames={} frameDuration={:.5} numTracks={} floatTracks={}",
                 anim.duration, anim.num_frames, anim.frame_duration,
                 anim.num_transform_tracks, anim.num_float_tracks);
        println!("  numBlocks={} maxFramesPerBlock={} blockDuration={:.4} blockInv={:.5} dataLen={}",
                 anim.num_blocks, anim.max_frames_per_block, anim.block_duration,
                 anim.block_inv_duration, anim.data_len);
        // consistency
        let derived_fd = anim.duration / (anim.num_frames.max(2) as f32 - 1.0);
        println!("  check: duration/(numFrames-1)={:.5} vs frameDuration={:.5}  (blockDur=(maxFPB-1)*fd={:.4})",
                 derived_fd, anim.frame_duration,
                 (anim.max_frames_per_block as f32 - 1.0) * anim.frame_duration);

        let frames = anim.sample(blob);
        // invariants
        let mut min_qn = f32::INFINITY; let mut max_qn = 0f32;
        let mut max_t = 0f32; let mut nonfinite = 0u32;
        for fr in &frames {
            for q in fr {
                let qn = (q.r[0]*q.r[0]+q.r[1]*q.r[1]+q.r[2]*q.r[2]+q.r[3]*q.r[3]).sqrt();
                if qn.is_finite() { min_qn = min_qn.min(qn); max_qn = max_qn.max(qn); } else { nonfinite += 1; }
                for c in 0..3 { let v = q.t[c].abs(); if v.is_finite() { max_t = max_t.max(v); } else { nonfinite += 1; } }
            }
        }
        println!("  decoded frames={}  quat|norm| in [{:.5},{:.5}]  max|translation|={:.4}m  nonfinite={}",
                 frames.len(), min_qn, max_qn, max_t, nonfinite);
        // show first-frame track 0..3
        if let Some(f0) = frames.first() {
            for t in 0..3.min(f0.len()) {
                let q = &f0[t];
                println!("    f0 trk{}: R=({:+.4},{:+.4},{:+.4},{:+.4}) T=({:+.4},{:+.4},{:+.4}) S=({:+.3},{:+.3},{:+.3})",
                         t, q.r[0], q.r[1], q.r[2], q.r[3], q.t[0], q.t[1], q.t[2], q.s[0], q.s[1], q.s[2]);
            }
        }
        // motion check: max rotation deviation of each track from its frame-0 pose,
        // taken over the WHOLE sequence (robust to looping idles where f0==fN).
        if frames.len() > 1 {
            let f0 = &frames[0];
            let mut best = 0usize; let mut bestd = 0f32;
            for t in 0..f0.len() {
                let mut mx = 0f32;
                for fr in frames.iter() {
                    let dot: f32 = (0..4).map(|k| f0[t].r[k]*fr[t].r[k]).sum::<f32>().abs().min(1.0);
                    mx = mx.max(2.0 * dot.acos());
                }
                if mx > bestd { bestd = mx; best = t; }
            }
            println!("    most-moving track = trk{} (peak Δangle vs f0 = {:.1} deg over sequence)",
                     best, bestd.to_degrees());
            // max-translation track + its trajectory (spot-checks translation splines)
            let mut bt = 0usize; let mut btv = 0f32;
            for t in 0..f0.len() {
                for fr in frames.iter() {
                    let m = (fr[t].t[0].powi(2)+fr[t].t[1].powi(2)+fr[t].t[2].powi(2)).sqrt();
                    if m.is_finite() && m > btv { btv = m; bt = t; }
                }
            }
            println!("    max-translation track = trk{} (peak |T|={:.2}m)", bt, btv);
            print!("    trk{} T over frames:", bt);
            for fr in frames.iter() { let p=&fr[bt].t; print!(" ({:+.2},{:+.2},{:+.2})", p[0],p[1],p[2]); }
            println!();
            print!("    trk{} R over frames:", best);
            for fr in frames.iter() {
                let q=&fr[best].r; print!(" ({:+.2},{:+.2},{:+.2},{:+.2})", q[0],q[1],q[2],q[3]);
            }
            println!();
        }
        println!();
    }
    // also report a census of mask usage in clip 0
    if let Some(&s0) = scas.first() {
        let a = read_spline_anim(&pk, s0);
        let bs = a.data_off + a.block_offsets.get(0).copied().unwrap_or(0) as usize;
        let mut rt = [0u32; 4]; let mut tt = [0u32; 16];
        for t in 0..a.num_transform_tracks {
            let c = blob[bs + t * 4];
            rt[(c & 3) as usize] += 1;
            tt[((c >> 2) & 0xf) as usize] += 1;
        }
        println!("clip0 rotType census {:?}  transType census {:?}", rt, &tt[..4]);
    }
}
