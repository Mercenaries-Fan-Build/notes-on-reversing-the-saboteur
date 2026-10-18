// Rust port, 2026-10-18, of FFmpeg 9.0.1 libavcodec/wmaprodec.c (the XMA2 decoder: xma_decode_init, xma_decode_packet
// and the WMA Pro frame decoder they run), with the sample skipping of libavcodec/decode.c.
// Copyright (c) 2007 Baptiste Coudurier, Benjamin Larsson, Ulion; (c) 2008 - 2011 Sascha Sommer,
// Benjamin Larsson; and the FFmpeg developers.
//
// This library is free software; you can redistribute it and/or modify it under the terms of the
// GNU Lesser General Public License as published by the Free Software Foundation; either version 2.1
// of the License, or (at your option) any later version. It is distributed in the hope that it will
// be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or
// FITNESS FOR A PARTICULAR PURPOSE. See the GNU Lesser General Public License for more details; its
// text is `COPYING.LGPLv2.1` in this directory.

//! XMA2 decoding: the Xbox 360 codec of the 2008 prototype's sound
//! (`docs/2008_audio_format.md`, `docs/formats/xma2_decoder.md`).
//!
//! [`decode`] takes one XMA stream's `data` bytes (2048-byte packets) and returns what FFmpeg 9.0.1
//! returns for it: the decoded frames with the first frame and then 64 samples dropped, a final
//! half frame flushed at the end, and `trim_end + trim_start - 192` samples cut from the end.
//! Malformed input is an error; FFmpeg's packet-loss recovery is not reproduced.

mod bits;
mod mdct;
mod tables;

use bits::{BitReader, BitWriter, Vlc};
use mdct::Imdct;
use tables::*;

use crate::pcm::Pcm;

pub const PACKET_SIZE: usize = 2048;

const MAX_SUBFRAMES: usize = 32;
const MAX_BANDS: usize = 29;
const MAX_FRAMESIZE: usize = 32768;
const BLOCK_MIN_BITS: u32 = 6;
const BLOCK_MAX_BITS: u32 = 13;
const BLOCK_MAX_SIZE: usize = 1 << BLOCK_MAX_BITS;
const BLOCK_SIZES: usize = (BLOCK_MAX_BITS - BLOCK_MIN_BITS + 1) as usize;
const VLCBITS: u32 = 9;
const SCALEVLCBITS: u32 = 8;
/// The decode flags XMA2 implies (`decode_init`): 512-sample frames, length prefixes, DRC, up to 4
/// subframes.
const DECODE_FLAGS: u32 = 0x10d6;
const SAMPLES_PER_FRAME: usize = 512;
const BITS_PER_SAMPLE: u32 = 16;
/// `log2(block_align = 2048) + 4`.
const LOG2_FRAME_SIZE: u32 = 15;

struct Vlcs {
    sf: Vlc,
    sf_rl: Vlc,
    coef: [Vlc; 2],
    vec4: Vlc,
    vec2: Vlc,
    vec1: Vlc,
}

fn pair_vlc(bits: u32, max_len: u32, t: &[(u8, u8)], offset: i32) -> Result<Vlc, String> {
    let lens: Vec<u8> = t.iter().map(|p| p.1).collect();
    let syms: Vec<i32> = t.iter().map(|p| p.0 as i32).collect();
    Vlc::from_lengths(bits, max_len, &lens, &syms, offset)
}

impl Vlcs {
    fn new() -> Result<Vlcs, String> {
        let coef0_syms: Vec<i32> = COEF0_SYMS.iter().map(|&s| s as i32).collect();
        let vec4_syms: Vec<i32> = VEC4_SYMS.iter().map(|&s| s as i32).collect();
        Ok(Vlcs {
            sf: pair_vlc(SCALEVLCBITS, 19, &SCALE_TABLE, -60)?,
            sf_rl: pair_vlc(VLCBITS, 21, &SCALE_RL_TABLE, 0)?,
            coef: [Vlc::from_lengths(VLCBITS, 22, &COEF0_LENS, &coef0_syms, 0)?, pair_vlc(VLCBITS, 22, &COEF1_TABLE, 0)?],
            vec4: Vlc::from_lengths(VLCBITS, 14, &VEC4_LENS, &vec4_syms, -1)?,
            vec2: pair_vlc(VLCBITS, 12, &VEC2_TABLE, -1)?,
            vec1: pair_vlc(VLCBITS, 11, &VEC1_TABLE, 0)?,
        })
    }
}

/// `ff_sine_window_init`: `sinf((i + 0.5) * (π / (2n)))`, the argument rounded to float.
fn sine_window(n: usize) -> Vec<f32> {
    (0..n).map(|i| (((i as f64 + 0.5) * (std::f64::consts::PI / (2.0 * n as f64))) as f32).sin()).collect()
}

#[derive(Clone)]
struct Channel {
    prev_block_len: usize,
    transmit_coefs: bool,
    num_subframes: usize,
    subframe_len: [usize; MAX_SUBFRAMES],
    cur_subframe: usize,
    decoded_samples: usize,
    grouped: bool,
    quant_step: i32,
    reuse_sf: bool,
    scale_factor_step: i32,
    max_scale_factor: i32,
    saved_scale_factors: [[i32; MAX_BANDS]; 2],
    scale_factor_idx: usize,
    /// Which of `saved_scale_factors` the current scale factors are.
    scale_factors: usize,
    table_idx: usize,
    /// Offset of the coefficient / output window in `out`.
    coeffs: usize,
    num_vec_coeffs: usize,
    out: Vec<f32>,
}

impl Channel {
    fn new() -> Channel {
        Channel {
            prev_block_len: SAMPLES_PER_FRAME,
            transmit_coefs: false,
            num_subframes: 0,
            subframe_len: [0; MAX_SUBFRAMES],
            cur_subframe: 0,
            decoded_samples: 0,
            grouped: false,
            quant_step: 0,
            reuse_sf: false,
            scale_factor_step: 0,
            max_scale_factor: 0,
            saved_scale_factors: [[0; MAX_BANDS]; 2],
            scale_factor_idx: 0,
            scale_factors: 0,
            table_idx: 0,
            coeffs: 0,
            num_vec_coeffs: 0,
            out: vec![0.0; BLOCK_MAX_SIZE + BLOCK_MAX_SIZE / 2],
        }
    }

    fn sf(&self, b: usize) -> i32 {
        self.saved_scale_factors[self.scale_factors][b]
    }
}

#[derive(Default)]
struct ChannelGroup {
    num_channels: usize,
    transform: bool,
    transform_band: [bool; MAX_BANDS],
    decorrelation_matrix: [f32; 4],
    channels: Vec<usize>,
}

/// One XMA stream (`WMAProDecodeCtx`).
struct Stream {
    vlc: Vlcs,
    nb_channels: usize,
    max_num_subframes: usize,
    subframe_len_bits: u32,
    max_subframe_len_bit: bool,
    min_samples_per_subframe: usize,
    num_sfb: [usize; BLOCK_SIZES],
    sfb_offsets: [[usize; MAX_BANDS]; BLOCK_SIZES],
    sf_offsets: [[[usize; MAX_BANDS]; BLOCK_SIZES]; BLOCK_SIZES],
    imdct: Vec<Option<Imdct>>,
    windows: Vec<Vec<f32>>,
    tmp: Vec<f32>,

    pb: BitWriter,
    next_packet_start: usize,
    packet_offset: usize,
    num_saved_bits: usize,
    frame_offset: usize,
    packet_loss: bool,
    packet_done: bool,
    eof_done: bool,

    skip_frame: bool,
    parsed_all_subframes: bool,
    trim_start: u32,
    trim_end: u32,

    subframe_len: usize,
    channels_for_cur_subframe: usize,
    channel_indexes_for_cur_subframe: [usize; 2],
    num_bands: usize,
    transmit_num_vec_coeffs: bool,
    table_idx: usize,
    esc_len: u32,
    num_chgroups: usize,
    chgroup: [ChannelGroup; 2],
    channel: Vec<Channel>,
}

fn av_log2(v: usize) -> u32 {
    usize::BITS - 1 - v.max(1).leading_zeros()
}

/// `get_rate`: the band-layout rate for a non-WMA Pro stream.
fn band_rate(sample_rate: u32) -> u32 {
    if sample_rate > 44100 {
        48000
    } else if sample_rate > 32000 {
        44100
    } else if sample_rate > 24000 {
        32000
    } else {
        24000
    }
}

/// `ff_wma_get_large_val`.
fn large_val(r: &mut BitReader) -> Result<u32, String> {
    let mut n = 8;
    if r.bit()? {
        n += 8;
        if r.bit()? {
            n += 8;
            if r.bit()? {
                n += 7;
            }
        }
    }
    r.bits(n)
}

impl Stream {
    /// `decode_init` for an XMA2 stream of `nb_channels` (1 or 2).
    fn new(nb_channels: usize, sample_rate: u32) -> Result<Stream, String> {
        let log2_max_num_subframes = (DECODE_FLAGS & 0x38) >> 3;
        let max_num_subframes = 1usize << log2_max_num_subframes;
        let num_possible_block_sizes = log2_max_num_subframes as usize + 1;
        let rate = band_rate(sample_rate) as usize;
        let mut num_sfb = [0usize; BLOCK_SIZES];
        let mut sfb_offsets = [[0usize; MAX_BANDS]; BLOCK_SIZES];
        for i in 0..num_possible_block_sizes {
            let subframe_len = SAMPLES_PER_FRAME >> i;
            let mut band = 1;
            sfb_offsets[i][0] = 0;
            for &freq in CRITICAL_FREQ.iter().take(MAX_BANDS - 1) {
                if sfb_offsets[i][band - 1] >= subframe_len {
                    break;
                }
                let offset = ((subframe_len * 2 * freq as usize) / rate + 2) & !3;
                if offset > sfb_offsets[i][band - 1] {
                    sfb_offsets[i][band] = offset;
                    band += 1;
                }
                if offset >= subframe_len {
                    break;
                }
            }
            sfb_offsets[i][band - 1] = subframe_len;
            num_sfb[i] = band - 1;
            if num_sfb[i] == 0 {
                return Err("num_sfb invalid".into());
            }
        }
        let mut sf_offsets = [[[0usize; MAX_BANDS]; BLOCK_SIZES]; BLOCK_SIZES];
        for i in 0..num_possible_block_sizes {
            for b in 0..num_sfb[i] {
                let offset = ((sfb_offsets[i][b] + sfb_offsets[i][b + 1] - 1) << i) >> 1;
                for x in 0..num_possible_block_sizes {
                    let mut v = 0;
                    while sfb_offsets[x][v + 1] << x < offset {
                        v += 1;
                        if v + 1 >= MAX_BANDS {
                            return Err("scale factor resample offset out of range".into());
                        }
                    }
                    sf_offsets[i][x][b] = v;
                }
            }
        }
        let imdct = (0..BLOCK_SIZES)
            .map(|i| {
                let len = 1usize << (BLOCK_MIN_BITS as usize + i);
                if len < SAMPLES_PER_FRAME >> log2_max_num_subframes || len > SAMPLES_PER_FRAME {
                    return None;
                }
                let scale = (1.0 / (1u64 << (BLOCK_MIN_BITS as usize + i - 1)) as f64 / (1u64 << (BITS_PER_SAMPLE - 1)) as f64) as f32;
                Some(Imdct::new(len, scale))
            })
            .collect();
        let windows = (0..BLOCK_SIZES).map(|i| sine_window(1 << (BLOCK_MIN_BITS as usize + i))).collect();
        Ok(Stream {
            vlc: Vlcs::new()?,
            nb_channels,
            max_num_subframes,
            subframe_len_bits: av_log2(log2_max_num_subframes as usize) + 1,
            max_subframe_len_bit: max_num_subframes == 16 || max_num_subframes == 4,
            min_samples_per_subframe: SAMPLES_PER_FRAME / max_num_subframes,
            num_sfb,
            sfb_offsets,
            sf_offsets,
            imdct,
            windows,
            tmp: vec![0.0; BLOCK_MAX_SIZE],
            pb: BitWriter::default(),
            next_packet_start: 0,
            packet_offset: 0,
            num_saved_bits: 0,
            frame_offset: 0,
            packet_loss: true,
            packet_done: false,
            eof_done: false,
            skip_frame: true,
            parsed_all_subframes: false,
            trim_start: 0,
            trim_end: 0,
            subframe_len: 0,
            channels_for_cur_subframe: 0,
            channel_indexes_for_cur_subframe: [0; 2],
            num_bands: 0,
            transmit_num_vec_coeffs: false,
            table_idx: 0,
            esc_len: 0,
            num_chgroups: 0,
            chgroup: Default::default(),
            channel: vec![Channel::new(); nb_channels],
        })
    }

    fn decode_subframe_length(&self, gb: &mut BitReader, offset: usize) -> Result<usize, String> {
        if offset == SAMPLES_PER_FRAME - self.min_samples_per_subframe {
            return Ok(self.min_samples_per_subframe);
        }
        let mut frame_len_shift = 0;
        if self.max_subframe_len_bit {
            if gb.bit()? {
                frame_len_shift = 1 + gb.bits(self.subframe_len_bits - 1)?;
            }
        } else {
            frame_len_shift = gb.bits(self.subframe_len_bits)?;
        }
        let subframe_len = SAMPLES_PER_FRAME >> frame_len_shift;
        if subframe_len < self.min_samples_per_subframe || subframe_len > SAMPLES_PER_FRAME {
            return Err(format!("broken frame: subframe_len {subframe_len}"));
        }
        Ok(subframe_len)
    }

    fn decode_tilehdr(&mut self, gb: &mut BitReader) -> Result<(), String> {
        let mut num_samples = [0usize; 2];
        let mut contains_subframe = [false; 2];
        let mut channels_for_cur_subframe = self.nb_channels;
        let mut min_channel_len = 0;
        for c in &mut self.channel {
            c.num_subframes = 0;
        }
        let fixed_channel_layout = self.max_num_subframes == 1 || gb.bit()?;
        loop {
            for c in 0..self.nb_channels {
                contains_subframe[c] = if num_samples[c] == min_channel_len {
                    if fixed_channel_layout
                        || channels_for_cur_subframe == 1
                        || min_channel_len == SAMPLES_PER_FRAME - self.min_samples_per_subframe
                    {
                        true
                    } else {
                        gb.bit()?
                    }
                } else {
                    false
                };
            }
            let subframe_len = self.decode_subframe_length(gb, min_channel_len)?;
            min_channel_len += subframe_len;
            for c in 0..self.nb_channels {
                let chan = &mut self.channel[c];
                if contains_subframe[c] {
                    if chan.num_subframes >= MAX_SUBFRAMES {
                        return Err("broken frame: num subframes > 31".into());
                    }
                    chan.subframe_len[chan.num_subframes] = subframe_len;
                    num_samples[c] += subframe_len;
                    chan.num_subframes += 1;
                    if num_samples[c] > SAMPLES_PER_FRAME {
                        return Err("broken frame: channel len > samples_per_frame".into());
                    }
                } else if num_samples[c] <= min_channel_len {
                    if num_samples[c] < min_channel_len {
                        channels_for_cur_subframe = 0;
                        min_channel_len = num_samples[c];
                    }
                    channels_for_cur_subframe += 1;
                }
            }
            if min_channel_len >= SAMPLES_PER_FRAME {
                break;
            }
        }
        Ok(())
    }

    fn decode_channel_transform(&mut self, gb: &mut BitReader) -> Result<(), String> {
        self.num_chgroups = 0;
        if self.nb_channels <= 1 {
            return Ok(());
        }
        let mut remaining_channels = self.channels_for_cur_subframe;
        if gb.bit()? {
            return Err("unsupported channel transform bit".into());
        }
        while remaining_channels > 0 && self.num_chgroups < self.channels_for_cur_subframe {
            let g = self.num_chgroups;
            self.chgroup[g].num_channels = 0;
            self.chgroup[g].transform = false;
            self.chgroup[g].channels.clear();
            if remaining_channels > 2 {
                return Err(format!("{remaining_channels} channels in one XMA stream"));
            }
            self.chgroup[g].num_channels = remaining_channels;
            for i in 0..self.channels_for_cur_subframe {
                let idx = self.channel_indexes_for_cur_subframe[i];
                if !self.channel[idx].grouped {
                    self.chgroup[g].channels.push(idx);
                }
                self.channel[idx].grouped = true;
            }
            if self.chgroup[g].num_channels == 2 {
                if gb.bit()? {
                    if gb.bit()? {
                        return Err("unknown channel transform type".into());
                    }
                } else {
                    self.chgroup[g].transform = true;
                    self.chgroup[g].decorrelation_matrix = if self.nb_channels == 2 {
                        [1.0, -1.0, 1.0, 1.0]
                    } else {
                        [0.70703125, -0.70703125, 0.70703125, 0.70703125]
                    };
                }
            }
            if self.chgroup[g].transform {
                if !gb.bit()? {
                    for i in 0..self.num_bands {
                        self.chgroup[g].transform_band[i] = gb.bit()?;
                    }
                } else {
                    for i in 0..self.num_bands {
                        self.chgroup[g].transform_band[i] = true;
                    }
                }
            }
            remaining_channels -= self.chgroup[g].num_channels;
            self.num_chgroups += 1;
        }
        Ok(())
    }

    /// `ff_wma_run_level_decode` with `version` 1 (WMA Pro).
    fn run_level_decode(
        &mut self,
        gb: &mut BitReader,
        c: usize,
        table: usize,
        mut offset: usize,
        num_coefs: usize,
    ) -> Result<(), String> {
        let (level_table, run_table): (&[f32], &[u16]) = if table == 1 { (&COEF1_LEVEL, &COEF1_RUN) } else { (&COEF0_LEVEL, &COEF0_RUN) };
        let block_len = self.subframe_len;
        let coef_mask = block_len - 1;
        let base = self.channel[c].coeffs;
        while offset < num_coefs {
            let code = self.vlc.coef[table].read(gb)?;
            if code > 1 {
                let code = code as usize;
                offset += run_table[code] as usize;
                let negative = !gb.bit()?;
                let v = level_table[code];
                self.channel[c].out[base + (offset & coef_mask)] = if negative { -v } else { v };
            } else if code == 1 {
                break;
            } else if code == 0 {
                let level = large_val(gb)?;
                if gb.bit()? {
                    if gb.bit()? {
                        if gb.bit()? {
                            return Err("broken escape sequence".into());
                        }
                        offset += gb.bits(self.esc_len)? as usize + 4;
                    } else {
                        offset += gb.bits(2)? as usize + 1;
                    }
                }
                let sign = gb.bit()? as i32 - 1;
                self.channel[c].out[base + (offset & coef_mask)] = ((level as i32 ^ sign) - sign) as f32;
            } else {
                return Err("invalid run-level code".into());
            }
            offset += 1;
        }
        if offset > num_coefs {
            return Err(format!("overflow ({offset} > {num_coefs}) in spectral RLE"));
        }
        Ok(())
    }

    fn decode_coeffs(&mut self, gb: &mut BitReader, c: usize) -> Result<(), String> {
        const FVAL: [f32; 16] = [0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0, 13.0, 14.0, 15.0];
        let vlctable = gb.bit()? as usize;
        let mut rl_mode = false;
        let mut cur_coeff = 0usize;
        let mut num_zeros = 0usize;
        let base = self.channel[c].coeffs;
        let num_vec_coeffs = self.channel[c].num_vec_coeffs;
        while (self.transmit_num_vec_coeffs || !rl_mode) && cur_coeff + 3 < num_vec_coeffs {
            let mut vals = [0f32; 4];
            let idx = self.vlc.vec4.read(gb)?;
            if idx < 0 {
                for i in (0..4).step_by(2) {
                    let idx = self.vlc.vec2.read(gb)?;
                    if idx < 0 {
                        let mut v0 = self.vlc.vec1.read(gb)? as u32;
                        if v0 as usize == VEC1_TABLE.len() - 1 {
                            v0 += large_val(gb)?;
                        }
                        let mut v1 = self.vlc.vec1.read(gb)? as u32;
                        if v1 as usize == VEC1_TABLE.len() - 1 {
                            v1 += large_val(gb)?;
                        }
                        vals[i] = v0 as f32;
                        vals[i + 1] = v1 as f32;
                    } else {
                        vals[i] = FVAL[(idx >> 4) as usize];
                        vals[i + 1] = FVAL[(idx & 0xF) as usize];
                    }
                }
            } else {
                vals[0] = FVAL[(idx >> 12) as usize];
                vals[1] = FVAL[((idx >> 8) & 0xF) as usize];
                vals[2] = FVAL[((idx >> 4) & 0xF) as usize];
                vals[3] = FVAL[(idx & 0xF) as usize];
            }
            for v in vals {
                if v.to_bits() != 0 {
                    let sign = (gb.bit()? as u32).wrapping_sub(1);
                    self.channel[c].out[base + cur_coeff] = f32::from_bits(v.to_bits() ^ (sign << 31));
                    num_zeros = 0;
                } else {
                    self.channel[c].out[base + cur_coeff] = 0.0;
                    num_zeros += 1;
                    rl_mode |= num_zeros > self.subframe_len >> 8;
                }
                cur_coeff += 1;
            }
        }
        if cur_coeff < self.subframe_len {
            for x in &mut self.channel[c].out[base + cur_coeff..base + self.subframe_len] {
                *x = 0.0;
            }
            self.run_level_decode(gb, c, vlctable, cur_coeff, self.subframe_len)?;
        }
        Ok(())
    }

    fn decode_scale_factors(&mut self, gb: &mut BitReader) -> Result<(), String> {
        for i in 0..self.channels_for_cur_subframe {
            let c = self.channel_indexes_for_cur_subframe[i];
            let nb = self.num_bands;
            let table_idx = self.table_idx;
            let sf_offsets = self.sf_offsets[table_idx][self.channel[c].table_idx];
            let ch = &mut self.channel[c];
            ch.scale_factors = (ch.scale_factor_idx == 0) as usize;
            if ch.reuse_sf {
                for b in 0..nb {
                    ch.saved_scale_factors[ch.scale_factors][b] = ch.saved_scale_factors[ch.scale_factor_idx][sf_offsets[b]];
                }
            }
            if ch.cur_subframe == 0 || gb.bit()? {
                if !ch.reuse_sf {
                    ch.scale_factor_step = gb.bits(2)? as i32 + 1;
                    let mut val = 45 / ch.scale_factor_step;
                    for b in 0..nb {
                        val += self.vlc.sf.read(gb)?;
                        ch.saved_scale_factors[ch.scale_factors][b] = val;
                    }
                } else {
                    let mut b = 0usize;
                    while b < nb {
                        let idx = self.vlc.sf_rl.read(gb)?;
                        let (skip, val, sign);
                        if idx == 0 {
                            let code = gb.bits(14)?;
                            val = (code >> 6) as i32;
                            sign = (code & 1) as i32 - 1;
                            skip = ((code & 0x3f) >> 1) as usize;
                        } else if idx == 1 {
                            break;
                        } else if idx > 1 {
                            skip = SCALE_RL_RUN[idx as usize] as usize;
                            val = SCALE_RL_LEVEL[idx as usize] as i32;
                            sign = gb.bit()? as i32 - 1;
                        } else {
                            return Err("invalid scale factor run-level code".into());
                        }
                        b += skip;
                        if b >= nb {
                            return Err("invalid scale factor coding".into());
                        }
                        ch.saved_scale_factors[ch.scale_factors][b] += (val ^ sign) - sign;
                        b += 1;
                    }
                }
                ch.scale_factor_idx = (ch.scale_factor_idx == 0) as usize;
                ch.table_idx = table_idx;
                ch.reuse_sf = true;
            }
            ch.max_scale_factor = (0..nb).map(|b| ch.sf(b)).max().expect("at least one band");
        }
        Ok(())
    }

    fn inverse_channel_transform(&mut self) {
        let sfb = self.sfb_offsets[self.table_idx];
        for g in 0..self.num_chgroups {
            if !self.chgroup[g].transform {
                continue;
            }
            let chans = self.chgroup[g].channels.clone();
            let mat = self.chgroup[g].decorrelation_matrix;
            let n = chans.len();
            for b in 0..self.num_bands {
                let (lo, hi) = (sfb[b], sfb[b + 1].min(self.subframe_len));
                if self.chgroup[g].transform_band[b] {
                    for y in lo..hi {
                        let data: Vec<f32> = chans.iter().map(|&c| self.channel[c].out[self.channel[c].coeffs + y]).collect();
                        for (k, &c) in chans.iter().enumerate() {
                            let mut sum = 0f32;
                            for (j, d) in data.iter().enumerate() {
                                sum = d.mul_add(mat[k * n + j], sum);
                            }
                            let base = self.channel[c].coeffs;
                            self.channel[c].out[base + y] = sum;
                        }
                    }
                } else if self.nb_channels == 2 {
                    for &c in &chans {
                        let base = self.channel[c].coeffs;
                        for x in &mut self.channel[c].out[base + lo..base + hi] {
                            *x *= (181.0 / 128.0) as f32;
                        }
                    }
                }
            }
        }
    }

    fn window(&mut self) {
        for i in 0..self.channels_for_cur_subframe {
            let c = self.channel_indexes_for_cur_subframe[i];
            let ch = &mut self.channel[c];
            let mut winlen = ch.prev_block_len;
            let mut start = ch.coeffs - (winlen >> 1);
            if self.subframe_len < winlen {
                start += (winlen - self.subframe_len) >> 1;
                winlen = self.subframe_len;
            }
            let window = &self.windows[(av_log2(winlen) - BLOCK_MIN_BITS) as usize];
            let len = winlen >> 1;
            let buf = &mut ch.out[start..start + 2 * len];
            for k in 0..len {
                let i = k;
                let j = 2 * len - 1 - k;
                let (s0, s1) = (buf[i], buf[j]);
                let (wi, wj) = (window[i], window[j]);
                buf[i] = s0.mul_add(wj, -(s1 * wi));
                buf[j] = s0.mul_add(wi, s1 * wj);
            }
            ch.prev_block_len = self.subframe_len;
        }
    }

    fn decode_subframe(&mut self, gb: &mut BitReader) -> Result<(), String> {
        let mut offset = SAMPLES_PER_FRAME;
        let mut subframe_len = SAMPLES_PER_FRAME;
        let mut total_samples = SAMPLES_PER_FRAME * self.nb_channels;
        let mut transmit_coeffs = false;
        for ch in &mut self.channel {
            ch.grouped = false;
            if offset > ch.decoded_samples {
                offset = ch.decoded_samples;
                subframe_len = ch.subframe_len[ch.cur_subframe];
            }
        }
        self.channels_for_cur_subframe = 0;
        for i in 0..self.nb_channels {
            let ch = &mut self.channel[i];
            total_samples -= ch.decoded_samples;
            if offset == ch.decoded_samples && subframe_len == ch.subframe_len[ch.cur_subframe] {
                total_samples -= ch.subframe_len[ch.cur_subframe];
                ch.decoded_samples += ch.subframe_len[ch.cur_subframe];
                self.channel_indexes_for_cur_subframe[self.channels_for_cur_subframe] = i;
                self.channels_for_cur_subframe += 1;
            }
        }
        if total_samples == 0 {
            self.parsed_all_subframes = true;
        }
        self.table_idx = av_log2(SAMPLES_PER_FRAME / subframe_len) as usize;
        self.num_bands = self.num_sfb[self.table_idx];
        offset += SAMPLES_PER_FRAME >> 1;
        for i in 0..self.channels_for_cur_subframe {
            let c = self.channel_indexes_for_cur_subframe[i];
            self.channel[c].coeffs = offset;
        }
        self.subframe_len = subframe_len;
        self.esc_len = av_log2(subframe_len - 1) + 1;

        if gb.bit()? {
            let mut num_fill_bits = gb.bits(2)? as usize;
            if num_fill_bits == 0 {
                let len = gb.bits(4)?;
                num_fill_bits = if len == 0 { 0 } else { gb.bits(len)? as usize } + 1;
            }
            if gb.count() + num_fill_bits > self.num_saved_bits {
                return Err("invalid number of fill bits".into());
            }
            gb.skip(num_fill_bits)?;
        }
        if gb.bit()? {
            return Err("reserved bit set".into());
        }
        self.decode_channel_transform(gb)?;
        for i in 0..self.channels_for_cur_subframe {
            let c = self.channel_indexes_for_cur_subframe[i];
            self.channel[c].transmit_coefs = gb.bit()?;
            if self.channel[c].transmit_coefs {
                transmit_coeffs = true;
            }
        }
        if transmit_coeffs {
            let mut quant_step = (90 * BITS_PER_SAMPLE >> 4) as i32;
            self.transmit_num_vec_coeffs = gb.bit()?;
            if self.transmit_num_vec_coeffs {
                let num_bits = av_log2(self.subframe_len.div_ceil(4)) + 1;
                for i in 0..self.channels_for_cur_subframe {
                    let c = self.channel_indexes_for_cur_subframe[i];
                    let num_vec_coeffs = (gb.bits(num_bits)? as usize) << 2;
                    if num_vec_coeffs > self.subframe_len {
                        return Err(format!("num_vec_coeffs {num_vec_coeffs} is too large"));
                    }
                    self.channel[c].num_vec_coeffs = num_vec_coeffs;
                }
            } else {
                for i in 0..self.channels_for_cur_subframe {
                    let c = self.channel_indexes_for_cur_subframe[i];
                    self.channel[c].num_vec_coeffs = self.subframe_len;
                }
            }
            let mut step = gb.sbits(6)?;
            quant_step += step;
            if step == -32 || step == 31 {
                let sign = (step == 31) as i32 - 1;
                let mut quant = 0;
                loop {
                    if gb.count() + 5 >= self.num_saved_bits {
                        break;
                    }
                    step = gb.bits(5)? as i32;
                    if step != 31 {
                        break;
                    }
                    quant += 31;
                }
                quant_step += ((quant + step) ^ sign) - sign;
            }
            if self.channels_for_cur_subframe == 1 {
                let c = self.channel_indexes_for_cur_subframe[0];
                self.channel[c].quant_step = quant_step;
            } else {
                let modifier_len = gb.bits(3)?;
                for i in 0..self.channels_for_cur_subframe {
                    let c = self.channel_indexes_for_cur_subframe[i];
                    self.channel[c].quant_step = quant_step;
                    if gb.bit()? {
                        if modifier_len != 0 {
                            self.channel[c].quant_step += gb.bits(modifier_len)? as i32 + 1;
                        } else {
                            self.channel[c].quant_step += 1;
                        }
                    }
                }
            }
            self.decode_scale_factors(gb)?;
        }
        for i in 0..self.channels_for_cur_subframe {
            let c = self.channel_indexes_for_cur_subframe[i];
            if self.channel[c].transmit_coefs && gb.count() < self.num_saved_bits {
                self.decode_coeffs(gb, c)?;
            } else {
                let base = self.channel[c].coeffs;
                for x in &mut self.channel[c].out[base..base + subframe_len] {
                    *x = 0.0;
                }
            }
        }
        if transmit_coeffs {
            self.inverse_channel_transform();
            let sfb = self.sfb_offsets[self.table_idx];
            let tx = (av_log2(subframe_len) - BLOCK_MIN_BITS) as usize;
            for i in 0..self.channels_for_cur_subframe {
                let c = self.channel_indexes_for_cur_subframe[i];
                let ch = &self.channel[c];
                for b in 0..self.num_bands {
                    let end = sfb[b + 1].min(self.subframe_len);
                    let exp = ch.quant_step - (ch.max_scale_factor - ch.sf(b)) * ch.scale_factor_step;
                    let quant = (std::f64::consts::LOG2_10 * (exp as f64 / 20.0)).exp2() as f32;
                    for k in sfb[b]..end {
                        self.tmp[k] = ch.out[ch.coeffs + k] * quant;
                    }
                }
                let base = ch.coeffs;
                let imdct = self.imdct[tx].as_mut().ok_or(format!("no IMDCT for a {subframe_len}-sample subframe"))?;
                imdct.run(&mut self.channel[c].out[base..base + subframe_len], &self.tmp[..subframe_len]);
            }
        }
        self.window();
        for i in 0..self.channels_for_cur_subframe {
            let c = self.channel_indexes_for_cur_subframe[i];
            if self.channel[c].cur_subframe >= self.channel[c].num_subframes {
                return Err("broken subframe".into());
            }
            self.channel[c].cur_subframe += 1;
        }
        Ok(())
    }

    /// One frame from the saved bits; `Ok(Some(more_frames))`, and `frame` holds its samples when
    /// it is output (not the skipped first frame).
    fn decode_frame(&mut self, frame: &mut Option<Vec<Vec<f32>>>) -> Result<bool, String> {
        let saved = std::mem::take(&mut self.pb.buf);
        let result = self.decode_frame_from(&saved, frame);
        self.pb.buf = saved;
        result
    }

    fn decode_frame_from(&mut self, saved: &[u8], frame: &mut Option<Vec<Vec<f32>>>) -> Result<bool, String> {
        let mut gb = BitReader::new(saved, self.num_saved_bits)?;
        gb.skip(self.frame_offset)?;
        let len = gb.bits(LOG2_FRAME_SIZE)? as usize;
        self.decode_tilehdr(&mut gb)?;
        if self.nb_channels > 1 && gb.bit()? && gb.bit()? {
            gb.skip(4 * self.nb_channels * self.nb_channels)?;
        }
        if DECODE_FLAGS & 0x80 != 0 {
            gb.bits(8)?;
        }
        if gb.bit()? {
            if gb.bit()? {
                self.trim_start = gb.bits(av_log2(SAMPLES_PER_FRAME * 2))?;
            }
            if gb.bit()? {
                self.trim_end = gb.bits(av_log2(SAMPLES_PER_FRAME * 2))?;
            }
        } else {
            self.trim_start = 0;
            self.trim_end = 0;
        }
        self.parsed_all_subframes = false;
        for ch in &mut self.channel {
            ch.decoded_samples = 0;
            ch.cur_subframe = 0;
            ch.reuse_sf = false;
        }
        while !self.parsed_all_subframes {
            self.decode_subframe(&mut gb)?;
        }
        let out: Vec<Vec<f32>> = self.channel.iter().map(|ch| ch.out[..SAMPLES_PER_FRAME].to_vec()).collect();
        for ch in &mut self.channel {
            ch.out.copy_within(SAMPLES_PER_FRAME..SAMPLES_PER_FRAME + SAMPLES_PER_FRAME / 2, 0);
        }
        if self.skip_frame {
            self.skip_frame = false;
        } else {
            *frame = Some(out);
        }
        let used = gb.count() - self.frame_offset;
        if len != used + 2 {
            return Err(format!("frame would have to skip {} bits", len as isize - used as isize - 1));
        }
        gb.skip(len - used - 1)?;
        Ok(gb.bit()?)
    }

    /// `save_bits`: put `len` bits of `gb` into the frame buffer, appending or starting afresh.
    fn save_bits(&mut self, gb: &mut BitReader, len: usize, append: bool) -> Result<(), String> {
        let buflen;
        if !append {
            self.frame_offset = gb.count() & 7;
            self.num_saved_bits = self.frame_offset;
            self.pb.clear();
            buflen = (self.num_saved_bits + len + 7) >> 3;
        } else {
            buflen = (self.pb.count() + len + 7) >> 3;
        }
        if len == 0 || buflen > MAX_FRAMESIZE {
            return Err(format!("frame buffer of {buflen} bytes for {len} bits"));
        }
        self.num_saved_bits += len;
        let bytes = gb.bytes();
        if !append {
            self.pb.copy(&bytes[gb.count() >> 3..], self.num_saved_bits)?;
            gb.skip(len)?;
        } else {
            let align = (8 - (gb.count() & 7)).min(len);
            let v = gb.bits(align as u32)?;
            self.pb.put(align as u32, v);
            let rest = len - align;
            self.pb.copy(&bytes[gb.count() >> 3..], rest)?;
            gb.skip(rest)?;
        }
        Ok(())
    }

    /// `decode_packet` for one call with `buf` (the rest of a packet); returns the bytes consumed
    /// and the frame decoded, if any.
    fn decode_packet(&mut self, buf: &[u8]) -> Result<(usize, Option<Vec<Vec<f32>>>), String> {
        let mut frame = None;
        if buf.is_empty() {
            self.packet_done = false;
            if self.eof_done {
                return Ok((0, None));
            }
            let flush = self
                .channel
                .iter()
                .map(|ch| {
                    let mut v = vec![0f32; SAMPLES_PER_FRAME];
                    v[..SAMPLES_PER_FRAME / 2].copy_from_slice(&ch.out[..SAMPLES_PER_FRAME / 2]);
                    v
                })
                .collect();
            self.eof_done = true;
            self.packet_done = true;
            return Ok((0, Some(flush)));
        }
        let buf_bit_size;
        let gb_len;
        if self.packet_done || self.packet_loss {
            self.packet_done = false;
            let size = buf.len().min(PACKET_SIZE);
            self.next_packet_start = buf.len() - size;
            buf_bit_size = size << 3;
            gb_len = size;
            let mut gb = BitReader::new(&buf[..gb_len], buf_bit_size)?;
            gb.bits(6)?;
            let mut num_bits_prev_frame = gb.bits(LOG2_FRAME_SIZE)? as usize;
            gb.skip(3)?;
            gb.bits(8)?;
            if num_bits_prev_frame > 0 {
                let remaining_packet_bits = buf_bit_size - gb.count();
                if num_bits_prev_frame >= remaining_packet_bits {
                    num_bits_prev_frame = remaining_packet_bits;
                    self.packet_done = true;
                }
                self.save_bits(&mut gb, num_bits_prev_frame, true)?;
                if !self.packet_loss {
                    self.decode_frame(&mut frame)?;
                }
            }
            if self.packet_loss {
                self.num_saved_bits = 0;
                self.packet_loss = false;
            }
            return self.finish_packet(gb, buf_bit_size, frame);
        }
        if buf.len() < self.next_packet_start {
            return Err("packet shorter than the next packet start".into());
        }
        buf_bit_size = (buf.len() - self.next_packet_start) << 3;
        gb_len = buf.len() - self.next_packet_start;
        let mut gb = BitReader::new(&buf[..gb_len], buf_bit_size)?;
        gb.skip(self.packet_offset)?;
        let remaining = buf_bit_size - gb.count();
        let frame_size = if remaining > LOG2_FRAME_SIZE as usize { gb.show(LOG2_FRAME_SIZE)? as usize } else { 0 };
        if remaining > LOG2_FRAME_SIZE as usize && frame_size != 0 && frame_size <= remaining {
            self.save_bits(&mut gb, frame_size, false)?;
            self.packet_done = !self.decode_frame(&mut frame)?;
        } else {
            self.packet_done = true;
        }
        self.finish_packet(gb, buf_bit_size, frame)
    }

    fn finish_packet(
        &mut self,
        mut gb: BitReader,
        buf_bit_size: usize,
        frame: Option<Vec<Vec<f32>>>,
    ) -> Result<(usize, Option<Vec<Vec<f32>>>), String> {
        let remaining = buf_bit_size - gb.count();
        if self.packet_done && remaining > 0 {
            self.save_bits(&mut gb, remaining, false)?;
        }
        self.packet_offset = gb.count() & 7;
        Ok((gb.count() >> 3, frame))
    }
}

/// Decode one XMA2 stream: `data` is its packets (a multiple of [`PACKET_SIZE`] bytes), `channels`
/// 1 or 2, `sample_rate` the stream's rate.
pub fn decode(data: &[u8], channels: u16, sample_rate: u32) -> Result<Pcm, String> {
    if data.len() % PACKET_SIZE != 0 {
        return Err(format!("XMA data of {} bytes is not whole {PACKET_SIZE}-byte packets", data.len()));
    }
    if !(1..=2).contains(&channels) {
        return Err(format!("{channels} channels in one XMA stream"));
    }
    let nch = channels as usize;
    let mut s = Stream::new(nch, sample_rate)?;
    let mut fifo: Vec<Vec<f32>> = vec![Vec::new(); nch];
    let mut emitted = 0usize;
    let (mut trim_start, mut trim_end) = (0u32, 0u32);
    let mut packets = data.chunks(PACKET_SIZE);
    let mut eof = false;
    while !eof {
        let packet: &[u8] = packets.next().unwrap_or(&[]);
        let mut at = 0usize;
        loop {
            let rest = &packet[at..];
            let (consumed, frame) = if s.eof_done { (0, None) } else { s.decode_packet(rest)? };
            if s.trim_start != 0 {
                trim_start = s.trim_start;
            }
            if s.trim_end != 0 {
                trim_end = s.trim_end;
            }
            if let Some(f) = frame {
                for (q, ch) in fifo.iter_mut().zip(f) {
                    q.extend(ch);
                }
            }
            if s.packet_done || s.packet_loss {
                let mut nb = fifo[0].len() - emitted;
                if rest.is_empty() {
                    nb -= ((trim_end + trim_start) as i64 - 128 - 64).clamp(0, nb as i64) as usize;
                    emitted += nb;
                    eof = true;
                    break;
                }
                nb -= nb.min(4096);
                emitted += nb;
            }
            at += consumed;
            if at >= packet.len() {
                break;
            }
            if consumed == 0 {
                return Err(format!("decoder made no progress at byte {at} of a packet"));
            }
        }
    }
    let skip = 64.min(emitted);
    let mut samples = Vec::with_capacity((emitted - skip) * nch);
    for i in skip..emitted {
        for q in &fifo {
            samples.push(q[i]);
        }
    }
    Ok(Pcm { sample_rate, channels, samples })
}
