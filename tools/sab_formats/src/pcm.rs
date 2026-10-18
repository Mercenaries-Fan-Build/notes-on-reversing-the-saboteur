//! Decoded audio: interleaved `f32` samples in [-1, 1], and a 16-bit PCM `WAVE` writer.
//!
//! Both builds' codecs decode into [`Pcm`]: retail Wwise Vorbis ([`crate::wwise`]) and 2008 XMA2
//! ([`crate::xma2`]).

#[derive(Clone, Debug, PartialEq)]
pub struct Pcm {
    pub sample_rate: u32,
    pub channels: u16,
    /// Interleaved: frame `i` is `samples[i * channels .. (i + 1) * channels]`.
    pub samples: Vec<f32>,
}

impl Pcm {
    /// Samples per channel.
    pub fn frames(&self) -> usize {
        self.samples.len() / self.channels as usize
    }

    /// A RIFF `WAVE` file: `fmt ` (PCM, 16-bit) and `data`. Each sample is `x * 32768` rounded to
    /// nearest, ties to even, and clamped to the i16 range.
    pub fn to_wav16(&self) -> Vec<u8> {
        let data_len = self.samples.len() * 2;
        let block_align = self.channels as u32 * 2;
        let mut b = Vec::with_capacity(44 + data_len);
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&(36 + data_len as u32).to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&self.channels.to_le_bytes());
        b.extend_from_slice(&self.sample_rate.to_le_bytes());
        b.extend_from_slice(&(self.sample_rate * block_align).to_le_bytes());
        b.extend_from_slice(&(block_align as u16).to_le_bytes());
        b.extend_from_slice(&16u16.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&(data_len as u32).to_le_bytes());
        for &x in &self.samples {
            let v = (x * 32768.0).round_ties_even().clamp(-32768.0, 32767.0) as i16;
            b.extend_from_slice(&v.to_le_bytes());
        }
        b
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav16_header_and_samples() {
        let p = Pcm { sample_rate: 44100, channels: 2, samples: vec![0.0, 1.0, -1.0, 0.5] };
        let w = p.to_wav16();
        assert_eq!((&w[0..4], &w[8..16], w.len()), (&b"RIFF"[..], &b"WAVEfmt "[..], 44 + 8));
        assert_eq!(u32::from_le_bytes(w[28..32].try_into().unwrap()), 44100 * 4);
        let s: Vec<i16> = w[44..].chunks(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect();
        assert_eq!(s, vec![0, 32767, -32768, 16384]);
        assert_eq!(p.frames(), 2);
    }
}
