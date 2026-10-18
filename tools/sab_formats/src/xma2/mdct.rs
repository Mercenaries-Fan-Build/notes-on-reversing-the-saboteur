// Rust port, 2026-10-18, of FFmpeg 9.0.1 libavutil/tx.c and tx_template.c (the float inverse MDCT and the split-radix
// FFT codelets it runs on). Copyright (c) Lynne, (c) 2008 Loren Merritt, (c) 2002 Fabrice Bellard.
// Licensed under the GNU Lesser General Public License, version 2.1 or later; see the LGPL notice in
// `mod.rs`.

//! The half inverse MDCT (`ff_tx_mdct_inv`, `len` coefficients in, `len` samples out) over the
//! in-place split-radix FFT (`ff_tx_fft{N}_ns`) with its pre-shuffled input map.
//!
//! Products are fused where clang's default `-ffp-contract=on` fuses them in the C source: within
//! one expression `a*b ± c*d` is `fma(a, b, ±(c*d))`.

use std::f64::consts::{FRAC_PI_2, PI};

#[derive(Clone, Copy, Default)]
struct C32 {
    re: f32,
    im: f32,
}

/// `CMUL(dre, dim, are, aim, bre, bim)`: `(dre, dim)`.
#[inline]
fn cmul(are: f32, aim: f32, bre: f32, bim: f32) -> (f32, f32) {
    (are.mul_add(bre, -(aim * bim)), are.mul_add(bim, aim * bre))
}

/// `ff_tx_tab_N`: `cos(2πi/N)` for `i < N/4`, then 0.
fn cos_table(n: usize) -> Vec<f32> {
    let freq = 2.0 * PI / n as f64;
    let mut t: Vec<f32> = (0..n / 4).map(|i| (i as f64 * freq).cos() as f32).collect();
    t.push(0.0);
    t
}

fn split_radix_permutation(i: usize, len: usize, inv: bool) -> isize {
    let len = len >> 1;
    if len <= 1 {
        return (i & 1) as isize;
    }
    if i & len == 0 {
        return split_radix_permutation(i, len, inv) * 2;
    }
    let len = len >> 1;
    split_radix_permutation(i, len, inv) * 4 + 1 - 2 * ((i & len == 0) ^ inv) as isize
}

struct Fft {
    n: usize,
    /// `cos_tables[k]` is the table for size `8 << k`.
    cos_tables: Vec<Vec<f32>>,
}

impl Fft {
    fn new(n: usize) -> Fft {
        let mut cos_tables = Vec::new();
        let mut s = 8;
        while s <= n.max(8) {
            cos_tables.push(cos_table(s));
            s <<= 1;
        }
        Fft { n, cos_tables }
    }

    fn tab(&self, n: usize) -> &[f32] {
        &self.cos_tables[n.trailing_zeros() as usize - 3]
    }

    fn run(&self, z: &mut [C32]) {
        self.fft(self.n, z);
    }

    fn fft(&self, n: usize, z: &mut [C32]) {
        match n {
            2 => fft2(z),
            4 => fft4(z),
            8 => fft8(z, self.tab(8)[1]),
            16 => self.fft16(z),
            _ => {
                let n2 = n / 2;
                let n4 = n / 4;
                self.fft(n2, &mut z[..n2]);
                self.fft(n4, &mut z[n4 * 2..n4 * 3]);
                self.fft(n4, &mut z[n4 * 3..n]);
                sr_combine(z, self.tab(n), n4 >> 1);
            }
        }
    }

    fn fft16(&self, z: &mut [C32]) {
        let cos = self.tab(16);
        let (c1, c2, c3) = (cos[1], cos[2], cos[3]);
        fft8(&mut z[0..8], self.tab(8)[1]);
        fft4(&mut z[8..12]);
        fft4(&mut z[12..16]);
        let mut t = Temps { t1: z[8].re, t2: z[8].im, t5: z[12].re, t6: z[12].im, ..Default::default() };
        butterflies(z, &mut t, 0, 4, 8, 12);
        transform(z, 2, 6, 10, 14, c2, c2);
        transform(z, 1, 5, 9, 13, c1, c3);
        transform(z, 3, 7, 11, 15, c3, c1);
    }
}

#[derive(Default)]
struct Temps {
    t1: f32,
    t2: f32,
    t3: f32,
    t4: f32,
    t5: f32,
    t6: f32,
}

fn fft2(z: &mut [C32]) {
    let (s0, s1) = (z[0], z[1]);
    z[1] = C32 { re: s0.re - s1.re, im: s0.im - s1.im };
    z[0] = C32 { re: s0.re + s1.re, im: s0.im + s1.im };
}

fn fft4(z: &mut [C32]) {
    let (s0, s1, s2, s3) = (z[0], z[1], z[2], z[3]);
    let (t3, t1) = (s0.re - s1.re, s0.re + s1.re);
    let (t8, t6) = (s3.re - s2.re, s3.re + s2.re);
    z[2].re = t1 - t6;
    z[0].re = t1 + t6;
    let (t4, t2) = (s0.im - s1.im, s0.im + s1.im);
    let (t7, t5) = (s2.im - s3.im, s2.im + s3.im);
    z[3].im = t4 - t8;
    z[1].im = t4 + t8;
    z[3].re = t3 - t7;
    z[1].re = t3 + t7;
    z[2].im = t2 - t5;
    z[0].im = t2 + t5;
}

fn fft8(z: &mut [C32], cos: f32) {
    let (s4, s5, s6, s7) = (z[4], z[5], z[6], z[7]);
    fft4(z);
    let mut t = Temps::default();
    t.t1 = s4.re - -s5.re;
    z[5].re = s4.re + -s5.re;
    t.t2 = s4.im - -s5.im;
    z[5].im = s4.im + -s5.im;
    t.t5 = s6.re - -s7.re;
    z[7].re = s6.re + -s7.re;
    t.t6 = s6.im - -s7.im;
    z[7].im = s6.im + -s7.im;
    butterflies(z, &mut t, 0, 2, 4, 6);
    transform(z, 1, 3, 5, 7, cos, cos);
}

/// `BUTTERFLIES(a0, a1, a2, a3)` over `z` indices, with `t1, t2, t5, t6` set by the caller.
fn butterflies(z: &mut [C32], t: &mut Temps, a0: usize, a1: usize, a2: usize, a3: usize) {
    let (r0, i0, r1, i1) = (z[a0].re, z[a0].im, z[a1].re, z[a1].im);
    t.t3 = t.t5 - t.t1;
    t.t5 += t.t1;
    z[a2].re = r0 - t.t5;
    z[a0].re = r0 + t.t5;
    z[a3].im = i1 - t.t3;
    z[a1].im = i1 + t.t3;
    t.t4 = t.t2 - t.t6;
    t.t6 += t.t2;
    z[a3].re = r1 - t.t4;
    z[a1].re = r1 + t.t4;
    z[a2].im = i0 - t.t6;
    z[a0].im = i0 + t.t6;
}

/// `TRANSFORM(a0, a1, a2, a3, wre, wim)`.
fn transform(z: &mut [C32], a0: usize, a1: usize, a2: usize, a3: usize, wre: f32, wim: f32) {
    let mut t = Temps::default();
    (t.t1, t.t2) = cmul(z[a2].re, z[a2].im, wre, -wim);
    (t.t5, t.t6) = cmul(z[a3].re, z[a3].im, wre, wim);
    butterflies(z, &mut t, a0, a1, a2, a3);
}

/// `ff_tx_fft_sr_combine(z, cos, len)`: z[0..8len], cos[0..2len].
fn sr_combine(z: &mut [C32], cos: &[f32], len: usize) {
    let o1 = 2 * len;
    let o2 = 4 * len;
    let o3 = 6 * len;
    let mut zb = 0usize;
    let mut cb = 0usize;
    let mut wb = o1 as isize - 7;
    let mut i = 0;
    while i < len {
        let w = |k: isize| cos[(wb + k) as usize];
        for (k, wk) in [(0usize, 7isize), (2, 5), (4, 3), (6, 1), (1, 6), (3, 4), (5, 2), (7, 0)] {
            transform(z, zb + k, zb + o1 + k, zb + o2 + k, zb + o3 + k, cos[cb + k], w(wk));
        }
        zb += 8;
        cb += 8;
        wb -= 8;
        i += 4;
    }
}

/// The inverse MDCT for one length, with its permuted twiddles and input map.
pub(super) struct Imdct {
    len: usize,
    /// Input map, already doubled (`map[i] <<= 1`).
    map: Vec<usize>,
    /// `exp[0..len/2]` permuted by the map; `exp[len/2..len]` in order.
    exp: Vec<C32>,
    fft: Fft,
    z: Vec<C32>,
}

impl Imdct {
    /// `av_tx_init(AV_TX_FLOAT_MDCT, inv = 1, len, scale)`.
    pub(super) fn new(len: usize, scale: f32) -> Imdct {
        let n = len >> 1;
        let sub_map: Vec<usize> = (0..n).map(|i| (-split_radix_permutation(i, n, true) & (n as isize - 1)) as usize).collect();
        let scale_d = scale as f64;
        let theta = 1.0 / 8.0;
        let s = scale_d.abs().sqrt();
        let mut exp = vec![C32::default(); 2 * n];
        for i in 0..n {
            let alpha = FRAC_PI_2 * (i as f64 + theta) / n as f64;
            exp[n + i] = C32 { re: (alpha.cos() * s) as f32, im: (alpha.sin() * s) as f32 };
        }
        for i in 0..n {
            exp[i] = exp[n + sub_map[i]];
        }
        let map = sub_map.iter().map(|&m| m << 1).collect();
        Imdct { len, map, exp, fft: Fft::new(n), z: vec![C32::default(); n] }
    }

    /// `len` coefficients in `src` to `len` samples in `dst`.
    pub(super) fn run(&mut self, dst: &mut [f32], src: &[f32]) {
        let len2 = self.len >> 1;
        let len4 = self.len >> 2;
        for i in 0..len2 {
            let k = self.map[i];
            let (tre, tim) = (src[self.len - 1 - k], src[k]);
            let e = self.exp[i];
            let (re, im) = cmul(tre, tim, e.re, e.im);
            self.z[i] = C32 { re, im };
        }
        self.fft.run(&mut self.z);
        let exp = &self.exp[len2..];
        let z = &mut self.z;
        for i in 0..len4 {
            let i0 = len4 + i;
            let i1 = len4 - i - 1;
            let src1 = (z[i1].im, z[i1].re);
            let src0 = (z[i0].im, z[i0].re);
            let (a, b) = cmul(src1.0, src1.1, exp[i1].im, exp[i1].re);
            z[i1].re = a;
            z[i0].im = b;
            let (c, d) = cmul(src0.0, src0.1, exp[i0].im, exp[i0].re);
            z[i0].re = c;
            z[i1].im = d;
        }
        for (i, c) in z.iter().enumerate() {
            dst[2 * i] = c.re;
            dst[2 * i + 1] = c.im;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Against FFmpeg's reference `ff_tx_mdct_naive_inv`, in double precision.
    #[test]
    fn half_imdct_matches_the_naive_transform() {
        for len in [16usize, 64, 128, 256, 512] {
            let scale = 1.0 / len as f32;
            let mut m = Imdct::new(len, scale);
            let src: Vec<f32> = (0..len).map(|k| ((k * 7919 % 97) as f32 - 48.0) / 64.0).collect();
            let mut dst = vec![0f32; len];
            m.run(&mut dst, &src);
            let half = len / 2;
            let phase = PI / (4.0 * len as f64);
            for i in 0..half {
                let i_d = phase * (2 * len - 2 * i - 1) as f64;
                let i_u = phase * (3 * len + 2 * i + 1) as f64;
                let (mut sum_d, mut sum_u) = (0.0f64, 0.0f64);
                for (j, &v) in src.iter().enumerate() {
                    let a = (2 * j + 1) as f64;
                    sum_d += (a * i_d).cos() * v as f64;
                    sum_u += (a * i_u).cos() * v as f64;
                }
                let want = [sum_d * scale as f64, -sum_u * scale as f64];
                for (got, want) in [dst[i], dst[i + half]].into_iter().zip(want) {
                    assert!((got as f64 - want).abs() < 1e-5, "len {len} sample {i}: {got} vs {want}");
                }
            }
        }
    }
}
