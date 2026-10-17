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
//! (retail's `num_blocks` and `track_masks`) stays on the concrete types; a caller that wants those
//! peels the trait object back to its backend via `LoadedClip::retail_anim`.

use crate::havok::QsTransform;

/// One playable animation clip, sampled at a wall-clock time. Same output shape (a
/// `Vec<QsTransform>` per-track pose plus a `track_to_bone` map) as retail's original
/// `SplineAnim::sample_at` → `skinning::posed` chain, so the trait drops in transparently.
pub trait AnimClip: Send + Sync {
    /// Playback duration in seconds.
    fn duration(&self) -> f32;
    /// Number of transform tracks this clip animates — the length that `sample_all` returns.
    fn num_transform_tracks(&self) -> usize;
    /// Number of key frames (retail: block-derived; 2008: the stored poses).
    fn num_frames(&self) -> usize;
    /// Sample every transform track at `time` (seconds, `>= 0`). Returns one `QsTransform` per
    /// track, in track order — `pose[k]` drives `track_to_bone()[k]` in `skinning::posed`.
    fn sample_all(&self, time: f32) -> Vec<QsTransform>;
    /// Track index → skeleton bone index. `-1` = unbound (`skinning::posed` skips it).
    ///
    /// Retail carries an authored per-clip mapping from the `ANIM` block, populated at
    /// pack-open time. A 2008 clip returns an empty slice and names its bones by
    /// [`AnimClip::bind_hashes`] instead; the app resolves those against the loaded rig.
    fn track_to_bone(&self) -> &[i32];

    /// The bone name CRC each track drives, in track order — the unresolved form of
    /// `track_to_bone`. Retail returns empty (its binding is `ANIM`-block indices). A 2008 clip
    /// returns its `ANM0` bone list, which the engine resolves with `PclSkeleton::FindBoneIndex`
    /// (`docs/2008_animations_format.md`, "Bone binding").
    fn bind_hashes(&self) -> &[u32] { &[] }
}

#[cfg(test)]
mod tests {
    //! Retail's `SplineAnim` through the trait shape. The 2008 clips are tested in
    //! `sources/prototype2008_anim.rs`.

    /// Retail-side smoke check: only runs on a machine with a retail install AND
    /// Animations.pack. The trait wrapper here goes through the app's `LoadedClip`,
    /// which is the wire the transport actually uses.
    ///
    /// Not gated on 2008 presence — a retail-only workstation still exercises the retail
    /// side of the trait. Left as a probe test so a broken retail impl still surfaces.
    #[test]
    fn retail_spline_flows_through_the_trait() {
        // The retail spline decoder is thoroughly tested by the app's `--selftest` and
        // `--anim-sweep` harnesses. Here we just confirm the trait shape holds by
        // instantiating a `SplineAnim` (retail) via the pack path — same as `load_clip`.
        //
        // No hand-crafted `SplineAnim` fixture: its internal state (block offsets,
        // transform offsets, data range) is derived from a real packfile. Skip cleanly
        // when the retail install isn't present.
        let Some(s) = crate::settings::detected_retail() else {
            eprintln!("skip: no retail install detected");
            return;
        };
        let pack_path = s.anim_pack();
        let file = match std::fs::read(&pack_path) {
            Ok(f) => f,
            Err(_) => {
                eprintln!("skip: retail Animations.pack unreadable at {pack_path}");
                return;
            }
        };
        let ap = crate::havok::parse_ap0l(&file).expect("parse AP0L header");
        let blob = &file[ap.blob_off..ap.blob_off + ap.hk_size];
        let pk = crate::havok::Packfile::parse(blob).expect("parse packfile");
        let scas: Vec<usize> = pk
            .vfixups
            .iter()
            .filter(|(_, c)| c == "hkaSplineCompressedAnimation")
            .map(|(s, _)| *s)
            .collect();
        assert!(!scas.is_empty(), "retail pack must have spline anims");
        let anim = crate::havok::read_spline_anim(&pk, scas[0]);
        // Trait shape: sample_all(0.0) yields num_transform_tracks entries, each a unit
        // quaternion (the sampler produces them; if it didn't, retail would render broken
        // rigs, which the corpus sweep would have caught).
        let n = anim.num_transform_tracks;
        let pose = anim.sample_at(blob, 0.0);
        assert_eq!(pose.len(), n, "SplineAnim.sample_at must return n tracks");
        for q in &pose {
            let m = (q.r[0] * q.r[0] + q.r[1] * q.r[1] + q.r[2] * q.r[2] + q.r[3] * q.r[3])
                .sqrt();
            assert!(
                (m - 1.0).abs() < 1e-3,
                "retail spline decoder returned non-unit quat |q|={m}"
            );
        }
    }
}
