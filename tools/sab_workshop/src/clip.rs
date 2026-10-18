//! Source-agnostic animation clip trait.
//!
//! Retail ships one flavour of clip (`hkaSplineCompressedAnimation`, decoded in `havok.rs`); the
//! 2008 Xbox 360 prototype ships three (`ANM0NOC0` uncompressed, `ANM0WAV0` wavelet, `ANM0DEL0`
//! delta — see `sources/prototype2008_anim.rs`). Both feed the same `skinning::posed` pipeline
//! and the same transport UI, so the app talks to `dyn AnimClip` and never has to know which
//! codec produced a pose.
//!
//! The trait is deliberately small — exactly what the app's playback loop and its diagnostic
//! harnesses (`selftest`, `anim_sweep`) need to reach through it. Everything version-specific
//! (retail's `num_blocks` and `track_masks`) stays on the concrete types, reached with `as_any`.

use crate::havok::QsTransform;

/// One playable animation clip, sampled at a wall-clock time.
pub trait AnimClip: Send + Sync {
    /// Playback duration in seconds.
    fn duration(&self) -> f32;
    /// Number of transform tracks this clip animates — the length that `sample_all` returns.
    fn num_transform_tracks(&self) -> usize;
    /// Number of key frames (retail: block-derived; 2008: the stored poses).
    fn num_frames(&self) -> usize;
    /// Sample every transform track at `time` (seconds, `>= 0`). Returns one `QsTransform` per
    /// track, in track order.
    fn sample_all(&self, time: f32) -> Vec<QsTransform>;
    /// The bone name hash each track drives, in track order (`0` for an unbound track). The app binds
    /// tracks to the loaded rig by these hashes, as the engine's `PclSkeleton::FindBoneIndex` does.
    fn bind_hashes(&self) -> &[u32];
    /// The concrete clip, for diagnostics that read one codec's own state.
    fn as_any(&self) -> &dyn std::any::Any;
}

/// Each track's rig bone: the bone whose name hash it drives, `-1` when the rig has none or the
/// track is unbound.
pub fn bind_to_rig(clip: &dyn AnimClip, rig_hashes: &[u32]) -> Vec<i32> {
    let index: std::collections::HashMap<u32, i32> = rig_hashes.iter().enumerate().rev().map(|(i, &h)| (h, i as i32)).collect();
    clip.bind_hashes().iter().map(|h| if *h == 0 { -1 } else { index.get(h).copied().unwrap_or(-1) }).collect()
}
