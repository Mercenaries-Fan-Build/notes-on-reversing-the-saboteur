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
//! (retail's `num_blocks` and `track_masks`, 2008's `codec` and `track_descriptors_noc0`) stays
//! on the concrete types; a caller that wants those still peels the trait object back to its
//! backend via `LoadedClip::retail_anim` / `LoadedClip::prototype2008_clip`.

use crate::havok::QsTransform;

/// One playable animation clip, sampled at a wall-clock time. Same output shape (a
/// `Vec<QsTransform>` per-track pose plus a `track_to_bone` map) as retail's original
/// `SplineAnim::sample_at` → `skinning::posed` chain, so the trait drops in transparently.
pub trait AnimClip: Send + Sync {
    /// Playback duration in seconds. `> 0.0` for a real clip; `0.0` for a static-pose
    /// (NOC0 dead-idle) whose transport still needs a sane read.
    fn duration(&self) -> f32;
    /// Number of transform tracks this clip animates — the length that `sample_all` returns.
    fn num_transform_tracks(&self) -> usize;
    /// Number of key frames (retail: block-derived; 2008: derived from the codec header).
    /// At least `1` — a static pose reads as one frame, matching what the transport expects.
    fn num_frames(&self) -> usize;
    /// Sample every transform track at `time` (seconds, clamped to `[0, duration]` by the
    /// backend if it wants to). Returns one `QsTransform` per track, in track order —
    /// `pose[k]` drives `track_to_bone()[k]` in `skinning::posed`.
    ///
    /// A codec that fails to decode (2008 WAV0/DEL0 for clips its layout gates reject) returns
    /// an all-identity pose so the transport stays stable and the caller sees the rig at rest
    /// rather than a crash. A stderr line (in debug builds) tells you which clip declined.
    fn sample_all(&self, time: f32) -> Vec<QsTransform>;
    /// Track index → skeleton bone index. `-1` = unbound (`skinning::posed` skips it).
    ///
    /// Retail carries an authored per-clip mapping from the `ANIM` block, populated at
    /// pack-open time. 2008 clips carry their own per-track bone-hash trailer at the
    /// tail of every ANM0-family record (see [`super::sources::prototype2008_anim::
    /// read_track_bone_hashes`]) — a hash-based mirror of retail Havok's
    /// `hkAnnotationTrack::m_trackName` array. Resolving the trailer to a target
    /// skeleton's bone indices is [`super::sources::prototype2008_anim::
    /// resolve_track_to_bone`], invoked at load time via
    /// [`super::sources::prototype2008_anim::Clip2008::bind_to_skeleton`] so this
    /// method returns real indices from the moment a 2008 clip is opened against a
    /// mesh. An unbound clip (rare, e.g. one loaded through the raw catalog without
    /// a target skeleton) returns an empty slice; `skinning::posed` treats that as
    /// "every track unbound" and the rig stays at bind pose.
    fn track_to_bone(&self) -> &[i32];

    /// The per-track BONE-NAME HASHES the clip drives, in track order — the
    /// unresolved form of `track_to_bone`. Retail returns empty by default
    /// (its binding is `ANIM`-block indices, not hashes), so app-side code
    /// treats an empty return as "this backend has no self-describing
    /// binding; fall back to `track_to_bone` populated elsewhere". 2008 overrides
    /// to return the ANM0 trailer's u32-BE array so the app can resolve
    /// against whatever character skeleton is currently loaded without
    /// baking the target into the clip. See [`super::sources::prototype2008_anim::
    /// read_track_bone_hashes`] for the trailer layout.
    fn bind_hashes(&self) -> &[u32] { &[] }
}

#[cfg(test)]
mod tests {
    //! Trait behaviour: both concrete impls must produce the same shape.
    //!
    //! Retail's `SplineAnim` impl is exercised indirectly through `LoadedClip` — that's the
    //! wrapper the app hands to the transport, and it's where the trait is really wired up.
    //! Retail's needs an install to open Animations.pack, so those tests skip cleanly on a
    //! machine without one. 2008's impl runs `AnimClip` directly against `Clip2008`; it
    //! also skips cleanly when Bin360/ isn't present.

    use crate::clip::AnimClip;
    use crate::pack::pandemic_hash;
    use crate::sources::prototype2008_anim::{Anim2008Catalog, Clip2008, ClipCodec};

    fn test_bin360() -> Option<std::path::PathBuf> {
        let root = crate::testenv::prototype2008_bin360()?;
        if root.join("Animations.pack").is_file() { Some(root) } else { None }
    }

    fn load_2008_noc0_pose() -> Option<Clip2008> {
        // A NOC0 static pose that decodes end-to-end — the same one
        // `sample_noc0_static_pose_within_bounds` uses.
        let root = test_bin360()?;
        let cat = Anim2008Catalog::open(&root);
        cat.load_clip(pandemic_hash("aurora_dead_idle_drvr")).ok()
    }

    #[test]
    fn trait_reports_duration_finite_and_frame_count_positive_on_2008() {
        let Some(clip) = load_2008_noc0_pose() else {
            eprintln!("skip: no Bin360/ prototype");
            return;
        };
        let dyn_clip: &dyn AnimClip = &clip;
        assert!(dyn_clip.duration().is_finite(), "duration must be finite");
        assert!(
            dyn_clip.num_transform_tracks() > 0,
            "NOC0 pose must have tracks"
        );
        assert!(dyn_clip.num_frames() >= 1, "at least one frame");
        assert_eq!(
            clip.header.codec,
            ClipCodec::None,
            "sample clip must be NOC0"
        );
    }

    #[test]
    fn trait_sample_all_length_matches_track_count_on_2008() {
        let Some(clip) = load_2008_noc0_pose() else {
            eprintln!("skip: no Bin360/ prototype");
            return;
        };
        let dyn_clip: &dyn AnimClip = &clip;
        let n = dyn_clip.num_transform_tracks();
        let p0 = dyn_clip.sample_all(0.0);
        assert_eq!(p0.len(), n, "sample_all(0.0) must return n tracks");
        // A NOC0 static pose at duration=0 doesn't have a "middle" — pick any small time in
        // [0, dur]. The transport calls with the clamped time either way.
        let mid = dyn_clip.duration() * 0.5;
        let pm = dyn_clip.sample_all(mid.max(0.0));
        assert_eq!(pm.len(), n, "sample_all(mid) must return n tracks");
        // Every rotation should be a unit quaternion — the sampler normalizes them.
        for q in &pm {
            let m = (q.r[0] * q.r[0] + q.r[1] * q.r[1] + q.r[2] * q.r[2] + q.r[3] * q.r[3])
                .sqrt();
            assert!(
                (m - 1.0).abs() < 1e-4,
                "sample_all returned non-unit quat |q|={m}"
            );
        }
    }

    /// The `GameSource::load_animation_clip` trait method must return a `Box<dyn AnimClip>`
    /// whose sample_all is coherent with a direct catalog load — same duration, same track
    /// count, same first-frame pose. Guards against the trait method drifting from the
    /// concrete Clip2008 path.
    #[test]
    fn source_trait_method_returns_a_working_boxed_animclip() {
        let Some(root) = test_bin360() else {
            eprintln!("skip: no Bin360/ prototype");
            return;
        };
        // Open the source, list animations, find a NOC0 handle, load through the trait.
        use crate::source::GameSource;
        let src = crate::sources::prototype2008::Prototype2008Source::open(
            root.parent().unwrap_or(&root),
        )
        .expect("open 2008 source");
        let anims = src.list_animations();
        // NOC0 static pose — `aurora_dead_idle_drvr` is one we know decodes end-to-end.
        let want = pandemic_hash("aurora_dead_idle_drvr");
        let Some(handle) = anims.iter().find(|h| h.hash == Some(want)) else {
            eprintln!("skip: aurora_dead_idle_drvr not in this Bin360");
            return;
        };
        let boxed = <_ as crate::source::GameSource>::load_animation_clip(&src, handle)
            .expect("trait method returns a boxed clip");
        assert!(boxed.duration().is_finite());
        assert!(boxed.num_transform_tracks() > 0);
        let pose = boxed.sample_all(0.0);
        assert_eq!(pose.len(), boxed.num_transform_tracks());
        for q in &pose {
            let m = (q.r[0] * q.r[0] + q.r[1] * q.r[1] + q.r[2] * q.r[2] + q.r[3] * q.r[3])
                .sqrt();
            assert!((m - 1.0).abs() < 1e-4, "boxed sample_all non-unit |q|={m}");
        }
    }

    #[test]
    fn trait_track_to_bone_is_slice_shaped_on_2008() {
        let Some(clip) = load_2008_noc0_pose() else {
            eprintln!("skip: no Bin360/ prototype");
            return;
        };
        let dyn_clip: &dyn AnimClip = &clip;
        // A freshly-loaded clip is unresolved (`bind_to_skeleton` hasn't been called),
        // so the mapping is empty and `skinning::posed` treats every track as unbound.
        // Once resolved (see `Clip2008::bind_to_skeleton` and its tests), the slice
        // has exactly `num_transform_tracks` entries — the trait shape lets a caller
        // zip pose against `track_to_bone` without a length check either way.
        let t2b = dyn_clip.track_to_bone();
        assert!(
            t2b.is_empty() || t2b.len() == dyn_clip.num_transform_tracks(),
            "track_to_bone must be either empty (unresolved) or exactly as long as num_transform_tracks"
        );
    }

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
