//! The 2008 build's animations: every loose `ANM0` clip and every record of every `AP04` pack, read,
//! decoded and sampled by `sab_formats::proto2008::{anim, anim_pack}` (`docs/2008_animations_format.md`).
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
use crate::source::ClipRow;

enum Location {
    Loose(PathBuf),
    Packed { pack: usize, record: usize },
}

struct Entry {
    name: String,
    location: Location,
}

pub struct Catalog {
    entries: Vec<Entry>,
    rows: Vec<ClipRow>,
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

fn read_clip(path: &Path, name: &str) -> Result<anim::Clip, String> {
    let b = std::fs::read(path).map_err(|e| format!("{name}: {e}"))?;
    let clip = anim::parse(&b).map_err(|e| format!("{name}: {e}"))?;
    if clip.len != b.len() {
        return Err(format!("{name}: the clip ends at byte {} of {}", clip.len, b.len()));
    }
    Ok(clip)
}

impl Catalog {
    /// Indexes `files` (every file under `bin360`) by content: `ANM0` files are loose clips, `AP04 ANIM`
    /// files are packs, parsed whole. A clip or pack that does not parse to its last byte fails the
    /// open.
    pub fn open(bin360: &Path, files: &[PathBuf]) -> Result<Catalog, String> {
        let mut entries = Vec::new();
        let mut rows = Vec::new();
        let mut packs = Vec::new();
        let mut stems: HashMap<u32, String> = HashMap::new();
        for p in files {
            let rel = p.strip_prefix(bin360).expect("under Bin360").to_string_lossy().replace('\\', "/");
            let h = head(p)?;
            if h.starts_with(anim::MAGIC) {
                let stem = p.file_stem().and_then(|s| s.to_str()).ok_or_else(|| format!("{rel}: no UTF-8 file stem"))?.to_string();
                let clip = read_clip(p, &rel)?;
                stems.entry(pandemic_hash(&stem)).or_insert(stem);
                rows.push(ClipRow { name: rel.clone(), duration: clip.duration, bones: clip.bones });
                entries.push(Entry { name: rel, location: Location::Loose(p.clone()) });
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
                rows.push(ClipRow { name: name.clone(), duration: r.clip.duration, bones: r.clip.bones.clone() });
                entries.push(Entry { name, location: Location::Packed { pack: pi, record: ri } });
            }
        }
        Ok(Catalog { entries, rows, packs, loose })
    }

    /// (loose clips, pack records).
    pub fn counts(&self) -> (usize, usize) {
        (self.loose, self.entries.len() - self.loose)
    }

    pub fn rows(&self) -> &[ClipRow] {
        &self.rows
    }

    /// Decodes clip `index`. Fails with the loader's reason for a loose clip `LoadAnimFile` rejects,
    /// and for a clip `samplePartialPose` cannot sample over its whole duration.
    pub fn load(&self, index: usize) -> Result<Clip2008, String> {
        let e = self.entries.get(index).ok_or_else(|| format!("clip {index} out of range"))?;
        let name = &e.name;
        let owned;
        let clip = match &e.location {
            Location::Loose(path) => {
                owned = read_clip(path, name)?;
                owned.check_loose().map_err(|err| format!("{name}: LoadAnimFile rejects this clip: {err}"))?;
                &owned
            }
            Location::Packed { pack, record } => &self.packs[*pack].1.records[*record].clip,
        };
        let anim = clip.decode().map_err(|err| format!("{name}: {err}"))?;
        if !(anim.duration() > f32::EPSILON) || anim.poses() < 2 {
            return Err(format!(
                "{name}: {} s over {} poses; samplePartialPose needs a duration above FLT_EPSILON and two poses",
                anim.duration(),
                anim.poses()
            ));
        }
        Ok(Clip2008 { name: name.clone(), anim, bones: clip.bones.clone() })
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

    fn bind_hashes(&self) -> &[u32] {
        &self.bones
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
