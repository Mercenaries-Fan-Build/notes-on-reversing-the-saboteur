//! `ANM0` animation clips: a loose `.hkx` file (`animations/`, `ObjectAnimations/`, `TestAnims/`) or
//! the clip inside an [`super::anim_pack`] record. Read as `PclAnimationManager::LoadAnimFile`
//! @0x83197e50 and `LoadAnimFromPackfile` @0x8319e6f8 read them (WildStar_d): the `ANM0` word, a codec
//! word, the codec header and data that the codec loader reads into a Havok 4.6 animation object, then
//! a bone count and that many bone name CRCs, one per track.
//!
//! | codec | loader | Havok class |
//! |---|---|---|
//! | `WAV0` | `LoadWaveletAnim` @0x83197080 | `hkWaveletSkeletalAnimation` |
//! | `DEL0` | `LoadDeltaAnim` @0x83197620 | `hkDeltaCompressedSkeletalAnimation` |
//! | `NOC0` | `LoadUncompressedAnim` @0x83197be0 | `hkInterleavedSkeletalAnimation` |
//!
//! Header words and `NOC0` transforms are big-endian. The tables inside a `WAV0` / `DEL0` data buffer
//! are little-endian: `handleEndian` (@0x83330948 wavelet, @0x833328b8 delta) swaps them on load.
//! [`Clip::decode`] expands a clip to its stored poses and [`Animation::sample`] samples them as the
//! engine's `samplePartialPose` does.

use super::cursor::Cursor;
use crate::bytes;

pub const MAGIC: &[u8; 4] = b"ANM0";
/// The codec loaders reject a clip whose duration is not below this (`__real_447a0000`).
pub const MAX_DURATION: f32 = 1000.0;
/// The `WAV0` / `DEL0` loaders reject a data buffer size outside `1..=MAX_DATA_BUFFER` (`!badSize`).
pub const MAX_DATA_BUFFER: i32 = 0x2F_FFFF;
/// `LoadUncompressedAnim` sizes the transform array `numTransforms * 0x30` only below this count.
pub const MAX_TRANSFORMS: u32 = 0x555_5556;
/// `LoadAnimFile` rejects a loose clip whose track count is outside `1..=MAX_LOOSE_TRACKS`;
/// `LoadAnimFromPackfile` does not check it.
pub const MAX_LOOSE_TRACKS: i32 = 4999;
/// The track-mask type of a component group the engine writes as identity: zero translation, unit
/// quaternion, unit scale (`hkTrackAnalysis::recompose` @0x83338d18).
pub const IDENTITY: u16 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Codec {
    Wavelet,
    Delta,
    Uncompressed,
}

impl Codec {
    pub const ALL: [Codec; 3] = [Codec::Wavelet, Codec::Delta, Codec::Uncompressed];

    pub const fn tag(self) -> &'static [u8; 4] {
        match self {
            Codec::Wavelet => b"WAV0",
            Codec::Delta => b"DEL0",
            Codec::Uncompressed => b"NOC0",
        }
    }

    pub fn from_tag(tag: &[u8]) -> Option<Codec> {
        Codec::ALL.into_iter().find(|c| c.tag() == tag)
    }

    pub fn name(self) -> &'static str {
        std::str::from_utf8(self.tag()).expect("codec tags are ASCII")
    }
}

/// `hkQsTransform`: translation, rotation quaternion (x, y, z, w), scale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct QsTransform {
    pub translation: [f32; 3],
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
}

impl QsTransform {
    pub const IDENTITY: QsTransform = QsTransform { translation: [0.0; 3], rotation: [0.0, 0.0, 0.0, 1.0], scale: [1.0; 3] };

    /// `hkQsTransform::setInterpolate4` @0x83333430: translation and scale interpolated linearly,
    /// rotation as `(1 - t) * a + t' * b` with `t' = -t` when `dot(a, b) < 0`, then normalized.
    pub fn interpolate(a: &QsTransform, b: &QsTransform, t: f32) -> QsTransform {
        let lerp3 = |x: [f32; 3], y: [f32; 3]| [0, 1, 2].map(|i| x[i] + t * (y[i] - x[i]));
        let dot: f32 = (0..4).map(|i| a.rotation[i] * b.rotation[i]).sum();
        let tb = if dot < 0.0 { -t } else { t };
        let ta = 1.0 - t;
        let q = [0, 1, 2, 3].map(|i| tb * b.rotation[i] + ta * a.rotation[i]);
        let len = q.iter().map(|v| v * v).sum::<f32>().sqrt();
        QsTransform { translation: lerp3(a.translation, b.translation), rotation: q.map(|v| v / len), scale: lerp3(a.scale, b.scale) }
    }
}

/// The quantization members shared by both compressed classes (`hkQuantizationFormat`).
#[derive(Clone, Debug, PartialEq)]
pub struct Quantization {
    /// `maxBitWidth`, object +0x28.
    pub max_bit_width: u8,
    /// `preserved`: leading samples of each channel block stored as raw floats, object +0x29.
    pub preserved: u8,
    /// `numD`: dynamic channel count, object +0x2C.
    pub num_d: u32,
    pub offset_idx: u32,
    pub scale_idx: u32,
    pub bit_width_idx: u32,
}

/// `hkWaveletSkeletalAnimation` as `LoadWaveletAnim` stores it.
#[derive(Clone, Debug, PartialEq)]
pub struct Wavelet {
    /// `numberOfPoses`, object +0x20.
    pub poses: i32,
    /// `blockSize`, object +0x24.
    pub block_size: i32,
    pub quantization: Quantization,
    pub static_mask_idx: u32,
    pub static_dofs_idx: u32,
    pub block_index_idx: u32,
    pub block_index_size: u32,
    pub quantized_data_idx: u32,
    pub quantized_data_size: u32,
    pub data: Vec<u8>,
}

/// `hkDeltaCompressedSkeletalAnimation` as `LoadDeltaAnim` stores it.
#[derive(Clone, Debug, PartialEq)]
pub struct Delta {
    /// `numberOfPoses`, object +0x20.
    pub poses: i32,
    /// `blockSize`, object +0x24.
    pub block_size: i32,
    pub quantization: Quantization,
    pub quantized_data_idx: u32,
    pub quantized_data_size: u32,
    pub static_mask_idx: u32,
    /// Stored; the decoder sizes the mask by the track count.
    pub static_mask_size: u32,
    pub static_dofs_idx: u32,
    /// Stored; the decoder sizes the static values from the masks.
    pub static_dofs_size: u32,
    /// Byte stride of the blocks in the quantized data.
    pub total_block_size: u32,
    /// Stored; the decoder sizes the last block from `numberOfPoses`.
    pub last_block_size: u32,
    pub data: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Body {
    Wavelet(Wavelet),
    Delta(Delta),
    /// `m_transforms`: `numTransforms` transforms, frame-major (`[frame * tracks + track]`).
    Uncompressed(Vec<QsTransform>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Clip {
    /// `m_duration` in seconds, object +0x0C.
    pub duration: f32,
    /// `m_numberOfTracks`, object +0x10.
    pub tracks: i32,
    pub body: Body,
    /// One bone name CRC per track. A clip loaded as a character animation binds track `i` to
    /// `PclSkeleton::FindBoneIndex(bones[i])`; an object animation keeps the CRCs.
    pub bones: Vec<u32>,
    /// Bytes read, from the `ANM0` word through the last bone CRC.
    pub len: usize,
}

impl Clip {
    pub fn codec(&self) -> Codec {
        match self.body {
            Body::Wavelet(_) => Codec::Wavelet,
            Body::Delta(_) => Codec::Delta,
            Body::Uncompressed(_) => Codec::Uncompressed,
        }
    }

    /// `LoadAnimFile`'s check on a loose clip: a track count outside `1..=4999` fails the load.
    pub fn check_loose(&self) -> Result<(), String> {
        if (1..=MAX_LOOSE_TRACKS).contains(&self.tracks) {
            Ok(())
        } else {
            Err(format!("{} tracks: LoadAnimFile accepts 1..={MAX_LOOSE_TRACKS}", self.tracks))
        }
    }

    /// Every stored pose, ready to sample.
    pub fn decode(&self) -> Result<Animation, String> {
        let tracks = usize::try_from(self.tracks).map_err(|_| format!("negative track count {}", self.tracks))?;
        let kind = match &self.body {
            Body::Wavelet(w) => Kind::Channels(decode_wavelet(w, tracks)?),
            Body::Delta(d) => Kind::Channels(decode_delta(d, tracks)?),
            Body::Uncompressed(t) => {
                if tracks == 0 {
                    return Err("NOC0 with 0 tracks: frames = numTransforms / numberOfTracks divides by zero".into());
                }
                Kind::Transforms(t.clone())
            }
        };
        Ok(Animation { duration: self.duration, tracks, kind })
    }
}

/// Reads one clip from `b`, which starts at the `ANM0` word and may run past the clip.
pub fn parse(b: &[u8]) -> Result<Clip, String> {
    let mut c = Cursor { b, at: 0 };
    let magic = c.take(4, "ANM0 magic")?;
    if magic != MAGIC {
        return Err(format!("not an ANM0 clip: magic {magic:02x?}"));
    }
    let tag = c.take(4, "codec")?;
    let codec = Codec::from_tag(tag).ok_or_else(|| format!("unknown ANM0 codec {tag:02x?}"))?;
    let duration = c.f32("m_duration")?;
    let (tracks, body) = match codec {
        Codec::Wavelet | Codec::Delta => {
            if !(duration < MAX_DURATION) {
                return Err(format!("{}: duration {duration} is not below {MAX_DURATION}; the loader rejects the clip", codec.name()));
            }
            let tracks = c.i32("m_numberOfTracks")?;
            let poses = c.i32("m_numberOfPoses")?;
            let block_size = c.i32("m_blockSize")?;
            let max_bit_width = c.u8("maxBitWidth")?;
            let preserved = c.u8("preserved")?;
            let num_d = c.u32("numD")?;
            let offset_idx = c.u32("offsetIdx")?;
            let scale_idx = c.u32("scaleIdx")?;
            let bit_width_idx = c.u32("bitWidthIdx")?;
            let quantization = Quantization { max_bit_width, preserved, num_d, offset_idx, scale_idx, bit_width_idx };
            let body = if codec == Codec::Wavelet {
                let [static_mask_idx, static_dofs_idx, block_index_idx, block_index_size, quantized_data_idx, quantized_data_size] =
                    [0; 6].map(|_| c.u32("wavelet index"));
                let data = data_buffer(&mut c, codec)?;
                Body::Wavelet(Wavelet {
                    poses,
                    block_size,
                    quantization,
                    static_mask_idx: static_mask_idx?,
                    static_dofs_idx: static_dofs_idx?,
                    block_index_idx: block_index_idx?,
                    block_index_size: block_index_size?,
                    quantized_data_idx: quantized_data_idx?,
                    quantized_data_size: quantized_data_size?,
                    data,
                })
            } else {
                let [quantized_data_idx, quantized_data_size, static_mask_idx, static_mask_size, static_dofs_idx, static_dofs_size, total_block_size, last_block_size] =
                    [0; 8].map(|_| c.u32("delta index"));
                let data = data_buffer(&mut c, codec)?;
                Body::Delta(Delta {
                    poses,
                    block_size,
                    quantization,
                    quantized_data_idx: quantized_data_idx?,
                    quantized_data_size: quantized_data_size?,
                    static_mask_idx: static_mask_idx?,
                    static_mask_size: static_mask_size?,
                    static_dofs_idx: static_dofs_idx?,
                    static_dofs_size: static_dofs_size?,
                    total_block_size: total_block_size?,
                    last_block_size: last_block_size?,
                    data,
                })
            };
            (tracks, body)
        }
        Codec::Uncompressed => {
            let tracks = c.i32("m_numberOfTracks")?;
            let n = c.u32("m_numTransforms")?;
            if n >= MAX_TRANSFORMS {
                return Err(format!("NOC0: {n} transforms overflow the 0x30-byte allocation"));
            }
            let mut transforms = Vec::with_capacity(n as usize);
            for _ in 0..n {
                let [t0, t1, t2, _, r0, r1, r2, r3, s0, s1, s2, _] = c.f32s::<12>("hkQsTransform")?;
                transforms.push(QsTransform { translation: [t0, t1, t2], rotation: [r0, r1, r2, r3], scale: [s0, s1, s2] });
            }
            (tracks, Body::Uncompressed(transforms))
        }
    };
    let n = c.count("bone count")?;
    let mut bones = Vec::with_capacity(n);
    for _ in 0..n {
        bones.push(c.u32("bone CRC")?);
    }
    Ok(Clip { duration, tracks, body, bones, len: c.at })
}

fn data_buffer(c: &mut Cursor, codec: Codec) -> Result<Vec<u8>, String> {
    let size = c.i32("m_numDataBuffer")?;
    if !(1..=MAX_DATA_BUFFER).contains(&size) {
        return Err(format!("{}: data buffer size {size} outside 1..={MAX_DATA_BUFFER}; the loader rejects the clip", codec.name()));
    }
    Ok(c.take(size as usize, "data buffer")?.to_vec())
}

/// A clip expanded to its stored poses.
#[derive(Clone, Debug)]
pub struct Animation {
    duration: f32,
    tracks: usize,
    kind: Kind,
}

#[derive(Clone, Debug)]
enum Kind {
    Channels(Channels),
    Transforms(Vec<QsTransform>),
}

/// A compressed clip's poses as its dynamic channel values, which the engine interpolates before
/// recomposing them with the static values.
#[derive(Clone, Debug)]
struct Channels {
    masks: Vec<u16>,
    statics: Vec<f32>,
    num_d: usize,
    poses: usize,
    /// `[pose * num_d + channel]`.
    values: Vec<f32>,
}

impl Animation {
    pub fn duration(&self) -> f32 {
        self.duration
    }

    pub fn tracks(&self) -> usize {
        self.tracks
    }

    /// Stored poses: `numberOfPoses`, or `numTransforms / numberOfTracks` for `NOC0`.
    pub fn poses(&self) -> usize {
        match &self.kind {
            Kind::Channels(c) => c.poses,
            Kind::Transforms(t) => t.len() / self.tracks,
        }
    }

    /// Stored pose `frame`, one transform per track.
    pub fn pose(&self, frame: usize) -> Result<Vec<QsTransform>, String> {
        if frame >= self.poses() {
            return Err(format!("pose {frame} of {}", self.poses()));
        }
        Ok(match &self.kind {
            Kind::Channels(c) => c.recompose(&c.values[frame * c.num_d..(frame + 1) * c.num_d]),
            Kind::Transforms(t) => t[frame * self.tracks..(frame + 1) * self.tracks].to_vec(),
        })
    }

    /// The pose at `time` seconds, as `samplePartialPose` computes it: wavelet @0x8332fdc0, delta
    /// @0x83331e08, interleaved @0x83332c10. A compressed clip's rotations are interpolated
    /// component-wise and are not normalized here; the engine normalizes the whole pose afterwards
    /// (`PclPose::NormalizeQuats`). The engine asserts `time >= 0` and `duration > FLT_EPSILON`; the
    /// compressed samplers also need two poses to interpolate between.
    pub fn sample(&self, time: f32) -> Result<Vec<QsTransform>, String> {
        if !(time >= 0.0) {
            return Err(format!("sample time {time}: samplePartialPose asserts time >= 0"));
        }
        if !(self.duration > f32::EPSILON) {
            return Err(format!("duration {}: samplePartialPose asserts duration > FLT_EPSILON", self.duration));
        }
        match &self.kind {
            Kind::Channels(c) => {
                if c.poses < 2 {
                    return Err(format!("{} poses: samplePartialPose asserts numberOfPoses >= 2", c.poses));
                }
                let (frame, delta) = frame_and_delta(time, self.duration, c.poses);
                let a = &c.values[frame * c.num_d..(frame + 1) * c.num_d];
                let b = &c.values[(frame + 1) * c.num_d..(frame + 2) * c.num_d];
                let one_minus = 1.0 - delta;
                let dynamic: Vec<f32> = a.iter().zip(b).map(|(x, y)| one_minus * x + delta * y).collect();
                Ok(c.recompose(&dynamic))
            }
            Kind::Transforms(t) => {
                if time < self.duration {
                    let poses = t.len() / self.tracks;
                    let x = time / (self.duration / (poses as f32 - 1.0));
                    let frame = x as usize;
                    let delta = x - frame as f32;
                    if frame + 1 >= poses {
                        return Err(format!("time {time} interpolates poses {frame} and {} of {poses}", frame + 1));
                    }
                    let a = &t[frame * self.tracks..(frame + 1) * self.tracks];
                    let b = &t[(frame + 1) * self.tracks..(frame + 2) * self.tracks];
                    Ok(a.iter().zip(b).map(|(a, b)| QsTransform::interpolate(a, b, delta)).collect())
                } else {
                    let last = t.len().checked_sub(self.tracks).ok_or_else(|| format!("{} transforms for {} tracks", t.len(), self.tracks))?;
                    Ok(t[last..].to_vec())
                }
            }
        }
    }
}

/// `getFrameAndDelta` of the compressed classes (@0x8332ed98, @0x83330df8): the frame index is capped
/// at `poses - 2` and the delta at 1.
fn frame_and_delta(time: f32, duration: f32, poses: usize) -> (usize, f32) {
    let frame_duration = duration / (poses as f32 - 1.0);
    let x = time / frame_duration;
    let frame = (x as usize).min(poses - 2);
    (frame, (x - frame as f32).min(1.0))
}

impl Channels {
    /// `hkTrackAnalysis::recompose` @0x83338d18: per track, translation x y z, rotation x y z w, scale
    /// x y z, each component the next dynamic value when its mask bit is set and the next static value
    /// otherwise; a group of type [`IDENTITY`] reads nothing.
    fn recompose(&self, dynamic: &[f32]) -> Vec<QsTransform> {
        let (mut d, mut s) = (dynamic.iter(), self.statics.iter());
        let mut next = |bit: bool| *(if bit { d.next() } else { s.next() }).expect("value counts are checked when decoding");
        self.masks
            .iter()
            .map(|&m| {
                let bits = m >> 6;
                let mut x = QsTransform::IDENTITY;
                if m & 3 != IDENTITY {
                    x.translation = [4, 2, 1].map(|b| next(bits & b != 0));
                }
                if (m >> 2) & 3 != IDENTITY {
                    x.rotation = [0x40, 0x20, 0x10, 0x8].map(|b| next(bits & b != 0));
                }
                if (m >> 4) & 3 != IDENTITY {
                    x.scale = [0x200, 0x100, 0x80].map(|b| next(bits & b != 0));
                }
                x
            })
            .collect()
    }
}

/// The track masks (little-endian `u16`, one per track) and the static values they call for, with
/// the dynamic channel count checked against `numD`.
fn masks_and_statics(data: &[u8], q: &Quantization, tracks: usize, mask_idx: u32, statics_idx: u32) -> Result<(Vec<u16>, Vec<f32>), String> {
    let mut masks = Vec::with_capacity(tracks);
    let (mut dynamic, mut statics) = (0usize, 0usize);
    for t in 0..tracks {
        let m = bytes::u16_le(data, mask_idx as usize + 2 * t, "track mask")?;
        let bits = m >> 6;
        for (group, width, dyn_bits) in [(m & 3, 3, bits & 7), ((m >> 2) & 3, 4, (bits >> 3) & 0xF), ((m >> 4) & 3, 3, bits >> 7)] {
            let n = dyn_bits.count_ones() as usize;
            if group == IDENTITY {
                if n != 0 {
                    return Err(format!("track {t}: mask 0x{m:04x} has an identity group with dynamic bits"));
                }
            } else {
                statics += width - n;
            }
            dynamic += n;
        }
        masks.push(m);
    }
    if dynamic != q.num_d as usize {
        return Err(format!("track masks give {dynamic} dynamic channels, numD is {}", q.num_d));
    }
    let values = (0..statics).map(|i| bytes::f32_le(data, statics_idx as usize + 4 * i, "static value")).collect::<Result<_, _>>()?;
    Ok((masks, values))
}

/// Per dynamic channel: offset, scale and bit width (1..=16, asserted by `hkBlockDecode` and
/// `hkExpandReal`).
fn channel_formats(data: &[u8], q: &Quantization) -> Result<Vec<(f32, f32, u32)>, String> {
    (0..q.num_d as usize)
        .map(|i| {
            let offset = bytes::f32_le(data, q.offset_idx as usize + 4 * i, "channel offset")?;
            let scale = bytes::f32_le(data, q.scale_idx as usize + 4 * i, "channel scale")?;
            let bits = bytes::u8_at(data, q.bit_width_idx as usize + i, "channel bit width")? as u32;
            if !(1..=16).contains(&bits) {
                return Err(format!("channel {i}: bit width {bits} outside 1..=16"));
            }
            Ok((offset, scale, bits))
        })
        .collect()
}

fn poses_and_block(poses: i32, block_size: i32) -> Result<(usize, usize), String> {
    let p = usize::try_from(poses).map_err(|_| format!("negative pose count {poses}"))?;
    let b = usize::try_from(block_size).ok().filter(|&b| b > 0).ok_or_else(|| format!("block size {block_size}"))?;
    Ok((p, b))
}

/// `hkCalcQuantizedSize` @0x8333b698: the bytes one channel of an `n`-sample block takes.
pub fn quantized_size(n: usize, bits: u32, preserved: usize) -> usize {
    preserved * 4 + (bits as usize * (n - preserved)).div_ceil(8)
}

/// `hkExpandReal` @0x8333a458: `preserved` raw little-endian floats, then for each remaining sample
/// `(q + 0.5) * (scale / 2^bits) + offset`, `q` read LSB-first from a little-endian bit stream.
fn expand_real(src: &[u8], at: usize, bits: u32, preserved: usize, scale: f32, offset: f32, out: &mut [f32]) -> Result<(), String> {
    for (i, v) in out[..preserved].iter_mut().enumerate() {
        *v = bytes::f32_le(src, at + 4 * i, "preserved sample")?;
    }
    let count = out.len() - preserved;
    let stream = bytes::slice(src, at + 4 * preserved, (bits as usize * count).div_ceil(8), "quantized samples")?;
    let step = (1.0 / (1u32 << bits) as f32) * scale;
    let mask = (1u32 << bits) - 1;
    for (k, v) in out[preserved..].iter_mut().enumerate() {
        let bit = k * bits as usize;
        let word = (0..3).filter_map(|j| stream.get(bit / 8 + j)).enumerate().fold(0u32, |w, (j, &b)| w | (b as u32) << (8 * j));
        *v = (((word >> (bit % 8)) & mask) as f32 + 0.5) * step + offset;
    }
    Ok(())
}

/// `(int)(longlong)v` on the Xenon: `fctidz` saturating to 64 bits, then the low 32 bits.
fn fctidz_low32(v: f32) -> i32 {
    let l: i64 = if v.is_nan() { i64::MIN } else { v as i64 };
    l as i32
}

fn decode_wavelet(w: &Wavelet, tracks: usize) -> Result<Channels, String> {
    let q = &w.quantization;
    let (poses, n) = poses_and_block(w.poses, w.block_size)?;
    if n < 8 || !n.is_power_of_two() {
        return Err(format!("block size {n}: hkInverseWaveletTransform needs a power of two >= 8"));
    }
    let preserved = q.preserved as usize;
    if preserved > n {
        return Err(format!("{preserved} preserved samples in a {n}-sample block"));
    }
    let (masks, statics) = masks_and_statics(&w.data, q, tracks, w.static_mask_idx, w.static_dofs_idx)?;
    let formats = channel_formats(&w.data, q)?;
    let blocks = poses.div_ceil(n);
    if (w.block_index_size as usize) < blocks {
        return Err(format!("{} block index entries for {blocks} blocks", w.block_index_size));
    }
    let quantized_end = w.quantized_data_idx as usize + w.quantized_data_size as usize;
    if quantized_end > w.data.len() {
        return Err(format!("quantized data ends at 0x{quantized_end:x}, past the 0x{:x}-byte buffer", w.data.len()));
    }
    let index = |b: usize| bytes::u32_le(&w.data, w.block_index_idx as usize + 4 * b, "block index").map(|v| v as usize);
    let num_d = formats.len();
    let mut values = vec![0f32; blocks * n * num_d];
    let mut samples = vec![0f32; n];
    for b in 0..blocks {
        let start = w.quantized_data_idx as usize + index(b)?;
        let end = if b + 1 == w.block_index_size as usize { quantized_end } else { w.quantized_data_idx as usize + index(b + 1)? };
        let mut at = start;
        for (ch, &(offset, scale, bits)) in formats.iter().enumerate() {
            let size = quantized_size(n, bits, preserved);
            let full = (1u32 << bits) as f32;
            let mut zero = fctidz_low32((full * -offset) / scale);
            if zero == 1 << bits {
                zero -= 1;
            }
            let (dense, used, consumed_end) = block_decode(&w.data, at, bits, zero, preserved, size)?;
            if consumed_end > end {
                return Err(format!("block {b} channel {ch}: bits run to 0x{consumed_end:x}, past the block end 0x{end:x}"));
            }
            at += used;
            expand_real(&dense, 0, bits, preserved, scale, offset, &mut samples)?;
            inverse_wavelet(&mut samples);
            for (k, &v) in samples.iter().enumerate() {
                values[(b * n + k) * num_d + ch] = v;
            }
        }
        if at > end {
            return Err(format!("block {b}: channels run to 0x{at:x}, past the block end 0x{end:x}"));
        }
    }
    values.truncate(poses * num_d);
    Ok(Channels { masks, statics, num_d, poses, values })
}

/// `hkBlockDecode` @0x8333aca0 for one channel of a wavelet block at `src[at..]`: `preserved` raw
/// words, a skip bitmap of `count` bits (set = the coefficient is not stored and decodes as `zero`),
/// then the stored codes LSB-first from 16-bit little-endian words. Returns the codes re-packed densely
/// for [`expand_real`], the bytes the engine advances by, and the end of the bytes whose bits it used.
/// The engine refills 16 bits at a time, so it may load up to two bytes past those it uses; such bytes
/// read as zero here.
fn block_decode(src: &[u8], at: usize, bits: u32, zero: i32, preserved: usize, size: usize) -> Result<(Vec<u8>, usize, usize), String> {
    let count = ((size - preserved * 4) * 8) as u32 / bits;
    let rd = |o: usize| src.get(o).copied().unwrap_or(0) as u32;
    let mut dst = vec![0u8; size + 4];
    for k in 0..preserved * 4 {
        dst[k] = rd(at + k) as u8;
    }
    let mut out = preserved * 4;
    let mut skip_at = at + preserved * 4;
    let mut skip_bit = 1u32;
    let mut stream = skip_at + count.div_ceil(8) as usize;
    let mut acc = rd(stream) | rd(stream + 1) << 8;
    stream += 2;
    let mut avail = 16u32;
    let mut read = 0u32;
    let mask = (1u32 << bits) - 1;
    let (mut packed, mut packed_bits) = (0u32, 0u32);
    for _ in 0..count {
        let skipped = rd(skip_at) & skip_bit != 0;
        let code = if skipped {
            zero as u32
        } else {
            if avail < bits {
                acc |= (rd(stream) | rd(stream + 1) << 8) << avail;
                stream += 2;
                avail += 16;
            }
            let v = acc & mask;
            acc >>= bits;
            avail -= bits;
            read += 1;
            v
        };
        match bits {
            8 => {
                dst[out] = code as u8;
                out += 1;
            }
            16 => {
                dst[out..out + 2].copy_from_slice(&(code as u16).to_le_bytes());
                out += 2;
            }
            _ => {
                packed |= code.wrapping_shl(packed_bits);
                packed_bits += bits;
                if packed_bits > 15 {
                    dst[out..out + 2].copy_from_slice(&(packed as u16).to_le_bytes());
                    out += 2;
                    packed >>= 16;
                    packed_bits -= 16;
                }
            }
        }
        skip_bit <<= 1;
        if skip_bit == 0x100 {
            skip_bit = 1;
            skip_at += 1;
        }
    }
    if packed_bits != 0 {
        dst[out..out + 2].copy_from_slice(&(packed as u16).to_le_bytes());
    }
    let used = ((read * bits) as usize + 32 * preserved + count as usize).div_ceil(8);
    let consumed_end = at + preserved * 4 + count.div_ceil(8) as usize + ((read * bits) as usize).div_ceil(8);
    if consumed_end > src.len() {
        return Err(format!("wavelet codes run to 0x{consumed_end:x}, past the 0x{:x}-byte buffer", src.len()));
    }
    Ok((dst, used, consumed_end))
}

/// Rows 1..7 of the 8-sample inverse basis `hkInverseWaveletTransform` @0x833393f0 multiplies by
/// (copied from `.rdata` 0x8229f030..0x8229f12f): row `k` is coefficient `k`'s contribution to samples
/// 0..7; coefficient 0 adds to every sample. Equal to inverse CDF 5/3 lifting down to one sample.
const BASIS: [[f32; 8]; 7] = [
    [-0.5, -0.25, 0.0, 0.25, 0.5, 0.5, 0.5, 0.5],
    [-0.5, 0.0625, 0.625, 0.1875, -0.25, -0.25, -0.25, -0.25],
    [0.0, -0.0625, -0.125, -0.1875, -0.25, 0.25, 0.75, 0.75],
    [-0.5, 0.625, -0.25, -0.125, 0.0, 0.0, 0.0, 0.0],
    [0.0, -0.125, -0.25, 0.75, -0.25, -0.125, 0.0, 0.0],
    [0.0, 0.0, 0.0, -0.125, -0.25, 0.75, -0.25, -0.25],
    [0.0, 0.0, 0.0, 0.0, 0.0, -0.125, -0.25, 0.75],
];

/// `hkInverseWaveletTransform` @0x833393f0: the 8-sample basis, then per doubling `hkUpdateReflect`
/// (-1/4) @0x8333a0d0, `hkPredictReflect` (+1/2) @0x83339fe0 and `hkMergeOddEven` @0x833399c0.
fn inverse_wavelet(x: &mut [f32]) {
    let mut first = [x[0]; 8];
    for (k, row) in BASIS.iter().enumerate() {
        for (j, v) in first.iter_mut().enumerate() {
            *v = x[k + 1].mul_add(row[j], *v);
        }
    }
    x[..8].copy_from_slice(&first);
    let mut len = 8;
    while len < x.len() {
        let (s, d) = x.split_at_mut(len);
        for i in (1..len).rev() {
            s[i] += -0.25 * (d[i - 1] + d[i]);
        }
        s[0] += 2.0 * -0.25 * d[0];
        for i in 0..len - 1 {
            d[i] += 0.5 * (s[i] + s[i + 1]);
        }
        d[len - 1] += 2.0 * 0.5 * s[len - 1];
        len *= 2;
        let merged: Vec<f32> = (0..len).map(|i| if i % 2 == 0 { x[i / 2] } else { x[len / 2 + i / 2] }).collect();
        x[..len].copy_from_slice(&merged);
    }
}

fn decode_delta(d: &Delta, tracks: usize) -> Result<Channels, String> {
    let q = &d.quantization;
    let (poses, n) = poses_and_block(d.poses, d.block_size)?;
    let preserved = q.preserved as usize;
    let (masks, statics) = masks_and_statics(&d.data, q, tracks, d.static_mask_idx, d.static_dofs_idx)?;
    let formats = channel_formats(&d.data, q)?;
    let block_bytes = |frames: usize| -> Result<usize, String> {
        if preserved > frames {
            return Err(format!("{preserved} preserved samples in a {frames}-frame block"));
        }
        Ok(formats.iter().map(|&(_, _, bits)| quantized_size(frames, bits, preserved)).sum())
    };
    let blocks = poses.div_ceil(n);
    if blocks > 1 && block_bytes(n)? != d.total_block_size as usize {
        return Err(format!("a full block takes {} bytes, totalBlockSize is {}", block_bytes(n)?, d.total_block_size));
    }
    let num_d = formats.len();
    let mut values = vec![0f32; poses * num_d];
    for b in 0..blocks {
        let frames = n.min(poses - b * n);
        if preserved > frames {
            return Err(format!("block {b}: {preserved} preserved samples in {frames} frames"));
        }
        let mut at = d.quantized_data_idx as usize + b * d.total_block_size as usize;
        let mut samples = vec![0f32; frames];
        for (ch, &(offset, scale, bits)) in formats.iter().enumerate() {
            expand_real(&d.data, at, bits, preserved, scale, offset, &mut samples)?;
            for i in 1..frames {
                samples[i] += samples[i - 1];
            }
            for (k, &v) in samples.iter().enumerate() {
                values[(b * n + k) * num_d + ch] = v;
            }
            at += quantized_size(frames, bits, preserved);
        }
    }
    Ok(Channels { masks, statics, num_d, poses, values })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(codec: &[u8; 4], duration: f32) -> Vec<u8> {
        let mut b = MAGIC.to_vec();
        b.extend_from_slice(codec);
        b.extend_from_slice(&duration.to_be_bytes());
        b
    }

    fn be(words: &[u32]) -> Vec<u8> {
        words.iter().flat_map(|w| w.to_be_bytes()).collect()
    }

    /// One track, translation x dynamic (mask bits 6+2 = 0x100), rotation and scale identity
    /// (types 2): 1 dynamic channel, 2 static values.
    fn one_channel_data(bits: u8, codes: &[u8], preserved: &[f32], offset: f32, scale: f32) -> (Vec<u8>, [u32; 5]) {
        let mask: u16 = (4 << 6) | (IDENTITY << 2) | (IDENTITY << 4);
        let mut data = mask.to_le_bytes().to_vec();
        data.extend_from_slice(&[0, 0]);
        let statics_idx = data.len() as u32;
        data.extend(7.0f32.to_le_bytes());
        data.extend(9.0f32.to_le_bytes());
        let offset_idx = data.len() as u32;
        data.extend(offset.to_le_bytes());
        let scale_idx = data.len() as u32;
        data.extend(scale.to_le_bytes());
        let bit_width_idx = data.len() as u32;
        data.push(bits);
        while data.len() % 4 != 0 {
            data.push(0);
        }
        let quantized_idx = data.len() as u32;
        for p in preserved {
            data.extend(p.to_le_bytes());
        }
        data.extend_from_slice(codes);
        (data, [statics_idx, offset_idx, scale_idx, bit_width_idx, quantized_idx])
    }

    #[test]
    fn delta_clip_decodes_and_samples() {
        // 3 poses, block size 8, preserved 1: first sample 1.0, then two 8-bit deltas with scale 256,
        // offset 0: (q + 0.5) * 1.0 -> 2.5 and 3.5, running sum 1.0, 3.5, 7.0.
        let (data, [statics_idx, offset_idx, scale_idx, bit_width_idx, quantized_idx]) = one_channel_data(8, &[2, 3], &[1.0], 0.0, 256.0);
        let mut b = header(b"DEL0", 2.0);
        b.extend(be(&[1, 3, 8]));
        b.extend_from_slice(&[8, 1]);
        b.extend(be(&[1, offset_idx, scale_idx, bit_width_idx, quantized_idx, 6, 0, 0, statics_idx, 0, 6, 6]));
        b.extend(be(&[data.len() as u32]));
        b.extend_from_slice(&data);
        b.extend(be(&[1, 0xABCD_1234]));
        let clip = parse(&b).unwrap();
        assert_eq!(clip.len, b.len());
        assert_eq!(clip.bones, [0xABCD_1234]);
        let a = clip.decode().unwrap();
        assert_eq!(a.poses(), 3);
        let x = |p: &[QsTransform]| p[0].translation;
        assert_eq!(x(&a.pose(0).unwrap()), [1.0, 7.0, 9.0]);
        assert_eq!(x(&a.pose(2).unwrap()), [7.0, 7.0, 9.0]);
        assert_eq!(a.pose(1).unwrap()[0].rotation, [0.0, 0.0, 0.0, 1.0]);
        // Duration 2 over 3 poses: t = 0.5 is frame 0, delta 0.5.
        assert_eq!(x(&a.sample(0.5).unwrap()), [2.25, 7.0, 9.0]);
        assert_eq!(x(&a.sample(5.0).unwrap()), [7.0, 7.0, 9.0]);
        assert!(a.sample(-1.0).is_err());
    }

    #[test]
    fn wavelet_skipped_coefficients_decode_as_the_zero_code() {
        // 16 coefficients of 8 bits, all skipped (bitmap 0xFFFF): each decodes as the zero code
        // (int)(256 * -offset / scale) = 128 and expands to (128 + 0.5) * (scale / 256) + offset.
        let scale = 2.0f32;
        let offset = -1.0f32;
        let mut codes = vec![0xFF, 0xFF];
        codes.extend_from_slice(&[0, 0]);
        let (data, [statics_idx, offset_idx, scale_idx, bit_width_idx, quantized_idx]) = one_channel_data(8, &codes, &[], offset, scale);
        let mut data = data;
        let block_index_idx = data.len() as u32;
        data.extend(0u32.to_le_bytes());
        let mut b = header(b"WAV0", 1.0);
        b.extend(be(&[1, 16, 16]));
        b.extend_from_slice(&[8, 0]);
        b.extend(be(&[1, offset_idx, scale_idx, bit_width_idx, 0, statics_idx, block_index_idx, 1, quantized_idx, 4]));
        b.extend(be(&[data.len() as u32]));
        b.extend_from_slice(&data);
        b.extend(be(&[1, 7]));
        let clip = parse(&b).unwrap();
        let a = clip.decode().unwrap();
        assert_eq!(a.poses(), 16);
        let c = (128.0 + 0.5) * ((1.0 / 256.0) * scale) + offset;
        let mut expect = vec![c; 16];
        inverse_wavelet(&mut expect);
        for (f, e) in expect.iter().enumerate() {
            assert_eq!(a.pose(f).unwrap()[0].translation[0], *e);
        }
    }

    #[test]
    fn inverse_wavelet_of_a_single_dc_coefficient_is_flat() {
        let mut x = vec![0f32; 16];
        x[0] = 3.0;
        inverse_wavelet(&mut x);
        assert!(x.iter().all(|&v| v == 3.0), "{x:?}");
    }

    /// The basis rows equal the lifting steps run from one sample up to eight (the inverse of
    /// `hkWaveletTransform` @0x83339b60, which lifts down to one sample).
    #[test]
    fn basis_equals_lifting_from_one_sample() {
        for k in 1..8 {
            let mut x = [0f32; 8];
            x[k] = 1.0;
            let mut len = 1;
            while len < 8 {
                let (s, d) = x.split_at_mut(len);
                for i in (1..len).rev() {
                    s[i] += -0.25 * (d[i - 1] + d[i]);
                }
                s[0] += 2.0 * -0.25 * d[0];
                for i in 0..len - 1 {
                    d[i] += 0.5 * (s[i] + s[i + 1]);
                }
                d[len - 1] += 2.0 * 0.5 * s[len - 1];
                len *= 2;
                let merged: Vec<f32> = (0..len).map(|i| if i % 2 == 0 { x[i / 2] } else { x[len / 2 + i / 2] }).collect();
                x[..len].copy_from_slice(&merged);
            }
            assert_eq!(x, BASIS[k - 1], "row {k}");
        }
    }

    #[test]
    fn uncompressed_clip_samples_like_the_engine() {
        let mut b = header(b"NOC0", 1.0);
        b.extend(be(&[1, 2]));
        for (tx, rz) in [(0.0f32, 0.0f32), (2.0, 1.0)] {
            let rw = (1.0f32 - rz * rz).sqrt();
            for v in [tx, 0.0, 0.0, 0.0, 0.0, 0.0, rz, rw, 1.0, 1.0, 1.0, 0.0] {
                b.extend(v.to_be_bytes());
            }
        }
        b.extend(be(&[1, 5]));
        let clip = parse(&b).unwrap();
        assert_eq!(clip.codec(), Codec::Uncompressed);
        let a = clip.decode().unwrap();
        assert_eq!(a.poses(), 2);
        let mid = a.sample(0.5).unwrap();
        assert_eq!(mid[0].translation, [1.0, 0.0, 0.0]);
        let r = mid[0].rotation;
        assert!((r[2] - 0.5f32.sqrt()).abs() < 1e-6 && (r[3] - 0.5f32.sqrt()).abs() < 1e-6, "{r:?}");
        assert_eq!(a.sample(1.0).unwrap()[0].translation, [2.0, 0.0, 0.0]);
    }

    #[test]
    fn loaders_reject_what_the_engine_rejects() {
        let mut b = header(b"WAV0", 1000.0);
        b.extend(be(&[1, 2, 16]));
        assert!(parse(&b).unwrap_err().contains("not below"));
        let mut b = header(b"DEL0", 1.0);
        b.extend(be(&[1, 2, 8]));
        b.extend_from_slice(&[8, 1]);
        b.extend(be(&[0; 12]));
        b.extend(be(&[0]));
        assert!(parse(&b).unwrap_err().contains("data buffer size 0"));
        let mut b = header(b"NOC0", 0.0);
        b.extend(be(&[0, 0, 0]));
        let clip = parse(&b).unwrap();
        assert!(clip.check_loose().is_err());
        assert!(clip.decode().is_err());
        assert!(parse(b"ANM0XYZ0").is_err());
    }
}
