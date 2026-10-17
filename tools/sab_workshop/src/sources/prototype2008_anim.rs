//! The 2008 build's animations as workshop assets: every loose `ANM0` clip and every record of every
//! `AP04` pack, read, decoded and sampled by `sab_formats::proto2008::{anim, anim_pack}`
//! (`docs/2008_animations_format.md`).
//!
//! A pack record is keyed by `pandemic_hash` of the animation's name; every key in the build is the
//! hash of a loose clip's file stem, so a record is listed under that stem. A loose clip that
//! `LoadAnimFile` rejects is listed and fails to load with the loader's reason.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use sab_formats::pandemic_hash;
use sab_formats::proto2008::anim::{self, Animation};
use sab_formats::proto2008::anim_pack::{self, Pack};

use crate::clip::AnimClip;
use crate::havok::QsTransform;
use crate::source::{AssetHandle, AssetKind};

enum Location {
    Loose(PathBuf),
    Packed { pack: usize, record: usize },
}

struct Entry {
    name: String,
    key: u32,
    location: Location,
}

pub struct Catalog {
    entries: Vec<Entry>,
    /// Each `AP04` pack, by path relative to `Bin360`.
    packs: Vec<(String, Pack)>,
    loose: usize,
}

fn head(path: &Path) -> Result<Vec<u8>, String> {
    let mut f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut b = Vec::with_capacity(anim_pack::MAGIC.len());
    f.by_ref().take(anim_pack::MAGIC.len() as u64).read_to_end(&mut b).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(b)
}

impl Catalog {
    /// Indexes `files` (every file under `bin360`) by content: `ANM0` files are loose clips, `AP04 ANIM`
    /// files are packs, parsed whole. A pack that does not parse to its last byte fails the open.
    pub fn open(bin360: &Path, files: &[PathBuf]) -> Result<Catalog, String> {
        let mut entries = Vec::new();
        let mut packs = Vec::new();
        let mut stems: HashMap<u32, String> = HashMap::new();
        for p in files {
            let rel = p.strip_prefix(bin360).expect("under Bin360").to_string_lossy().replace('\\', "/");
            let h = head(p)?;
            if h.starts_with(anim::MAGIC) {
                let stem = p.file_stem().and_then(|s| s.to_str()).ok_or_else(|| format!("{rel}: no UTF-8 file stem"))?.to_string();
                let key = pandemic_hash(&stem);
                stems.entry(key).or_insert(stem);
                entries.push(Entry { name: rel, key, location: Location::Loose(p.clone()) });
            } else if h == anim_pack::MAGIC {
                let b = std::fs::read(p).map_err(|e| format!("{rel}: {e}"))?;
                let pack = anim_pack::parse(&b).map_err(|e| format!("{rel}: {e} — the 2008 build's Animations.pack parses, so this copy is damaged"))?;
                if pack.len != b.len() {
                    return Err(format!("{rel}: the pack ends at byte {} of {}", pack.len, b.len()));
                }
                packs.push((rel, pack));
            }
        }
        let loose = entries.len();
        for (pi, (rel, pack)) in packs.iter().enumerate() {
            for (ri, r) in pack.records.iter().enumerate() {
                let name = match stems.get(&r.key) {
                    Some(stem) => format!("{rel}: {stem}"),
                    None => format!("{rel}: {:08X}", r.key),
                };
                entries.push(Entry { name, key: r.key, location: Location::Packed { pack: pi, record: ri } });
            }
        }
        Ok(Catalog { entries, packs, loose })
    }

    /// (loose clips, pack records).
    pub fn counts(&self) -> (usize, usize) {
        (self.loose, self.entries.len() - self.loose)
    }

    pub fn handles(&self) -> Vec<AssetHandle> {
        self.entries
            .iter()
            .enumerate()
            .map(|(i, e)| AssetHandle { key: i as u64, name: e.name.clone(), kind: AssetKind::Animation, hash: Some(e.key) })
            .collect()
    }

    fn entry(&self, handle: &AssetHandle) -> Result<&Entry, String> {
        if handle.kind != AssetKind::Animation {
            return Err(format!("handle {:?} '{}' is not an animation", handle.kind, handle.name));
        }
        self.entries.get(handle.key as usize).ok_or_else(|| format!("animation handle {} out of range", handle.key))
    }

    /// Runs `f` on the handle's clip and, for a pack record, the record.
    fn with_clip<T>(&self, handle: &AssetHandle, f: impl FnOnce(&anim::Clip, Option<&anim_pack::Record>) -> Result<T, String>) -> Result<T, String> {
        let e = self.entry(handle)?;
        match &e.location {
            Location::Loose(path) => {
                let b = std::fs::read(path).map_err(|err| format!("{}: {err}", e.name))?;
                let clip = anim::parse(&b).map_err(|err| format!("{}: {err}", e.name))?;
                if clip.len != b.len() {
                    return Err(format!("{}: the clip ends at byte {} of {}", e.name, clip.len, b.len()));
                }
                f(&clip, None)
            }
            Location::Packed { pack, record } => {
                let r = &self.packs[*pack].1.records[*record];
                f(&r.clip, Some(r))
            }
        }
    }

    /// Decodes the handle's clip. Fails with the loader's reason for a loose clip `LoadAnimFile`
    /// rejects, and for a clip `samplePartialPose` cannot sample over its whole duration.
    pub fn load(&self, handle: &AssetHandle) -> Result<Clip2008, String> {
        let name = &self.entry(handle)?.name;
        self.with_clip(handle, |clip, record| {
            if record.is_none() {
                clip.check_loose().map_err(|e| format!("{name}: LoadAnimFile rejects this clip: {e}"))?;
            }
            let anim = clip.decode().map_err(|e| format!("{name}: {e}"))?;
            if !(anim.duration() > f32::EPSILON) || anim.poses() < 2 {
                return Err(format!(
                    "{name}: {} s over {} poses; samplePartialPose needs a duration above FLT_EPSILON and two poses",
                    anim.duration(),
                    anim.poses()
                ));
            }
            Ok(Clip2008 { name: name.clone(), anim, bones: clip.bones.clone() })
        })
    }

    /// The detail-panel rows for an animation handle.
    pub fn describe(&self, handle: &AssetHandle) -> Result<Vec<(String, String)>, String> {
        let e = self.entry(handle)?;
        let mut kv = vec![("key".to_string(), format!("{:08X}", e.key))];
        self.with_clip(handle, |clip, record| {
            kv.push(("codec".into(), clip.codec().name().into()));
            kv.push(("duration".into(), format!("{} s", clip.duration)));
            kv.push(("tracks".into(), clip.tracks.to_string()));
            if let Some(r) = record {
                kv.push(("object animation".into(), (r.object != 0).to_string()));
                kv.push(("events".into(), r.events.len().to_string()));
                kv.push(("motion length".into(), r.motion_length.to_string()));
            } else if let Err(reason) = clip.check_loose() {
                kv.push(("LoadAnimFile".into(), format!("rejects: {reason}")));
            }
            Ok(())
        })?;
        Ok(kv)
    }
}

/// A decoded clip, sampled through `sab_formats`' engine-order sampler.
pub struct Clip2008 {
    pub name: String,
    anim: Animation,
    /// One bone name CRC per track.
    bones: Vec<u32>,
}

impl AnimClip for Clip2008 {
    fn duration(&self) -> f32 {
        self.anim.duration()
    }

    fn num_transform_tracks(&self) -> usize {
        self.anim.tracks()
    }

    fn num_frames(&self) -> usize {
        self.anim.poses()
    }

    /// The pose at `time`, with every rotation normalized as `PclAnimator::SetupPose` does
    /// (`PclPose::NormalizeQuats`). `Catalog::load` admits only clips the sampler accepts over
    /// `0..=duration` and beyond, so a sampler error here is a caller passing a negative time.
    fn sample_all(&self, time: f32) -> Vec<QsTransform> {
        let pose = self.anim.sample(time).unwrap_or_else(|e| panic!("{}: {e}", self.name));
        pose.iter()
            .map(|x| {
                let len = x.rotation.iter().map(|v| v * v).sum::<f32>().sqrt();
                let [tx, ty, tz] = x.translation;
                let [sx, sy, sz] = x.scale;
                QsTransform { t: [tx, ty, tz, 0.0], r: x.rotation.map(|v| v / len), s: [sx, sy, sz, 1.0] }
            })
            .collect()
    }

    /// Resolved by the app against the loaded rig's bone hashes, from [`AnimClip::bind_hashes`].
    fn track_to_bone(&self) -> &[i32] {
        &[]
    }

    fn bind_hashes(&self) -> &[u32] {
        &self.bones
    }
}

#[cfg(all(test, feature = "retail"))]
mod tests {
    use super::*;
    use crate::source::GameSource;
    use crate::sources::prototype2008::Prototype2008Source;

    fn source() -> Prototype2008Source {
        let bin360 = sab_formats::game_paths::local_config_proto2008_bin360(Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap_or_else(|e| panic!("{e}"));
        Prototype2008Source::open(&bin360).unwrap_or_else(|e| panic!("{e}"))
    }

    /// Every listed animation loads and samples at its start, middle and end, except the 30 loose
    /// clips `LoadAnimFile` rejects.
    #[test]
    fn every_animation_of_the_build_loads_and_samples() {
        let src = source();
        let handles = src.list_animations();
        let mut rejected = 0;
        let mut failures = Vec::new();
        for h in &handles {
            match src.load_animation_clip(h) {
                Ok(c) => {
                    for t in [0.0, c.duration() * 0.5, c.duration()] {
                        let pose = c.sample_all(t);
                        if pose.len() != c.num_transform_tracks() || c.bind_hashes().len() != pose.len() {
                            failures.push(format!("{}: {} transforms, {} tracks, {} bone CRCs", h.name, pose.len(), c.num_transform_tracks(), c.bind_hashes().len()));
                        }
                        if let Some(q) = pose.iter().find(|q| (q.r.iter().map(|v| v * v).sum::<f32>() - 1.0).abs() > 1e-5) {
                            failures.push(format!("{}: rotation {:?} is not unit", h.name, q.r));
                        }
                    }
                }
                Err(e) if e.contains("LoadAnimFile rejects") => rejected += 1,
                Err(e) => failures.push(e),
            }
        }
        assert!(failures.is_empty(), "{} failures:\n{}", failures.len(), failures.join("\n"));
        assert_eq!(handles.len(), 3620 + 1605);
        assert_eq!(rejected, 30);
    }

    /// `Sean_wave` (60 tracks) drives Sean HD's rig: its track CRCs are the rig's bone hashes, and
    /// sampled through `skinning::posed` it moves bound bones off the bind pose and over time.
    #[test]
    fn sean_wave_drives_sean_hd() {
        let src = source();
        let mesh = src.list_meshes().into_iter().find(|h| h.name == "CH_AL_SeanDevlin_01_HD").expect("Sean HD mesh");
        let m = src.read_mesh_record(&mesh).expect("Sean HD record").mesh().clone();
        let (_, rig, _) = crate::sources::prototype2008::convert(&m).expect("convert Sean HD");
        let clip = src.list_animations().into_iter().find(|h| h.name == "animations/Sean_wave.hkx").expect("Sean_wave");
        let c = src.load_animation_clip(&clip).expect("Sean_wave loads");
        let index: HashMap<u32, i32> = m.bones.iter().enumerate().map(|(i, b)| (b.hash, i as i32)).collect();
        let t2b: Vec<i32> = c.bind_hashes().iter().map(|h| index.get(h).copied().unwrap_or(-1)).collect();
        let bound = t2b.iter().filter(|&&i| i >= 0).count();
        assert_eq!((t2b.len(), bound), (60, 60));

        let bind = crate::skinning::bind_pose(&rig);
        let at0 = crate::skinning::posed(&rig, &c.sample_all(0.0), &t2b, true);
        let mid = crate::skinning::posed(&rig, &c.sample_all(c.duration() * 0.5), &t2b, true);
        let differs = |a: &glam::Mat4, b: &glam::Mat4| a.to_cols_array().iter().zip(b.to_cols_array()).map(|(x, y)| (x - y).abs()).sum::<f32>() > 1e-3;
        let off_bind = t2b.iter().filter(|&&b| differs(&at0[b as usize], &bind[b as usize])).count();
        let moving = t2b.iter().filter(|&&b| differs(&at0[b as usize], &mid[b as usize])).count();
        assert!(off_bind > 0 && moving > 0, "{off_bind} bones off the bind pose, {moving} moving");
    }
}
