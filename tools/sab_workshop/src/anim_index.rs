//! The clip catalog: name, duration, per-track bone bindings — one row per animation.
//!
//! Clip `index` N == the N-th `hkaSplineCompressedAnimation` in the pack's main blob
//! (file order). We keep every clip but flag the ones playable on the loaded skeleton
//! (per-track values are bone INDICES into that rig, not foreign name-hashes).
//!
//! Two sources, same catalog:
//!   * [`from_pack`] — the game's own `Animations.pack`. This is the DEFAULT: the `ANIM` block at the
//!     front of the pack is where `sab_animmeta` read all of this from in the first place, so the
//!     generated `anim_bone_map.json` was only ever a cache of bytes the install already has.
//!   * [`load`] — that JSON, for `--index` (a hand-edited or historical catalog).

// A couple of catalog fields are retained for provenance but not read by the viewer.
#![allow(dead_code)]

use serde::Deserialize;

const UNBOUND: i64 = 4294967295; // 0xFFFFFFFF sentinel for an unbound track

#[derive(Deserialize)]
struct RawClip {
    index: usize,
    name: String,
    #[serde(default)]
    duration: f32,
    #[serde(default)]
    num_transform_tracks: usize,
    #[serde(default)]
    num_tracks: usize,
    #[serde(default)]
    bone_ids: Vec<i64>,
    #[serde(default)]
    bone_repr: String,
    #[serde(default)]
    subset_of_skeleton: bool,
}

#[derive(Deserialize)]
struct RawIndex {
    #[serde(default)]
    num_main_clips: usize,
    #[serde(default)]
    skeleton_bones: usize,
    clips: Vec<RawClip>,
}

/// One playable clip descriptor.
#[derive(Clone)]
pub struct ClipInfo {
    pub index: usize,     // == N-th spline anim in the pack
    pub name: String,
    pub duration: f32,
    pub num_tracks: usize,
    /// Per-track skeleton bone index; -1 == unbound (was 0xFFFFFFFF).
    pub track_to_bone: Vec<i32>,
    pub playable: bool,
}

pub struct AnimCatalog {
    pub clips: Vec<ClipInfo>,
    pub skeleton_bones: usize,
    pub num_main_clips: usize,
}

/// Build the catalog from the game's `Animations.pack` — no generated file involved.
///
/// The pack opens with an `ANIM` block: a record per animation carrying its id, name, duration and
/// (for non-streamed clips) the ordered per-track bone list. Format and validation are documented in
/// `tools/sab_animmeta` — walking `recordCount` records lands exactly on the `numAnims`/`hkSize` pair
/// that precedes the Havok blob, which is the structural self-check we assert below.
///
/// `skeleton_bones` is the loaded rig's bone count; it decides which clips are playable. The stored
/// per-track values are polymorphic: biped clips (2155/2214 in retail) hold bone INDICES into the
/// shared biped rig, while 59 exotic-skeleton clips (Cow / Chicken / bird) hold bone name-HASHES.
/// A clip is playable here when every bound track is an index this rig has.
pub fn from_pack(path: &str, skeleton_bones: usize) -> Result<AnimCatalog, String> {
    // The ANIM block is ~0.9 MB in retail, so a prefix is enough — we must not read 187 MB to fill a
    // list that has to appear immediately. Grow only if a bigger pack needs it.
    let mut want = 4 << 20;
    let raw = loop {
        let buf = read_head(path, want)?;
        let complete = buf.len() < want; // short read == we hold the whole file
        match parse_anim(&buf) {
            Ok(clips) => break clips,
            Err(AnimErr::Truncated) if !complete && want < 512 << 20 => {
                want *= 4;
                continue;
            }
            Err(AnimErr::Truncated) => return Err(format!("{path}: ANIM block is truncated")),
            Err(AnimErr::Bad(e)) => return Err(format!("{path}: {e}")),
        }
    };
    let clips: Vec<ClipInfo> = raw
        .into_iter()
        .enumerate()
        .map(|(i, c)| {
            let bound: Vec<u32> = c.bones.iter().copied().filter(|&b| b != UNBOUND_U32).collect();
            let playable =
                !bound.is_empty() && bound.iter().all(|&b| (b as usize) < skeleton_bones);
            let track_to_bone = c
                .bones
                .iter()
                .map(|&b| if (b as usize) < skeleton_bones { b as i32 } else { -1 })
                .collect();
            ClipInfo {
                index: i,
                name: c.name,
                duration: c.duration,
                num_tracks: c.bones.len(),
                track_to_bone,
                playable,
            }
        })
        .collect();
    let n = clips.len();
    Ok(AnimCatalog { clips, skeleton_bones, num_main_clips: n })
}

const UNBOUND_U32: u32 = 0xFFFF_FFFF;

/// Read at most `want` bytes from the head of a file. A short result means the file ended.
fn read_head(path: &str, want: usize) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let mut f = std::fs::File::open(path).map_err(|e| format!("open {path}: {e}"))?;
    let mut buf = vec![0u8; want];
    let mut n = 0;
    while n < want {
        match f.read(&mut buf[n..]).map_err(|e| format!("read {path}: {e}"))? {
            0 => break,
            k => n += k,
        }
    }
    buf.truncate(n);
    Ok(buf)
}

/// A raw ANIM record for a non-streamed (main-blob) clip.
struct RawAnim {
    name: String,
    duration: f32,
    bones: Vec<u32>,
}

enum AnimErr {
    /// Ran off the end of the buffer — the caller may hold only a prefix of the file.
    Truncated,
    /// The bytes are not an ANIM block we understand; a bigger read will not help.
    Bad(String),
}

/// Cursor over the pack head that fails with `Truncated` rather than panicking at the buffer end.
struct Cur<'a> {
    b: &'a [u8],
    o: usize,
}

impl<'a> Cur<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], AnimErr> {
        let end = self.o.checked_add(n).ok_or(AnimErr::Truncated)?;
        let s = self.b.get(self.o..end).ok_or(AnimErr::Truncated)?;
        self.o = end;
        Ok(s)
    }
    fn u32(&mut self) -> Result<u32, AnimErr> {
        let s = self.take(4)?;
        Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }
    fn f32(&mut self) -> Result<f32, AnimErr> {
        Ok(f32::from_bits(self.u32()?))
    }
    fn u8(&mut self) -> Result<u8, AnimErr> {
        Ok(self.take(1)?[0])
    }
}

/// Parse the leading `ANIM` block of an AP0L pack into its non-streamed clips, in file order.
fn parse_anim(file: &[u8]) -> Result<Vec<RawAnim>, AnimErr> {
    let mut c = Cur { b: file, o: 0 };
    // "AP0L" / "ANIM" stored byte-reversed, as everywhere else in this format.
    if c.take(4)? != b"L0PA" {
        return Err(AnimErr::Bad("not an AP0L pack (magic != L0PA)".into()));
    }
    if c.take(4)? != b"MINA" {
        return Err(AnimErr::Bad("first pack block is not ANIM".into()));
    }
    let count = c.u32()? as usize;
    // A plausibility bound: retail has 3463 records. Anything wildly past that means we are not
    // reading a record count at all, and would otherwise loop for a very long time.
    if count > 1_000_000 {
        return Err(AnimErr::Bad(format!("implausible ANIM record count {count}")));
    }
    let mut out = Vec::new();
    for _ in 0..count {
        let _id = c.u32()?;
        let _unk4 = c.u8()?;
        let streamed = c.u8()? != 0;
        let nlen = c.u32()? as usize;
        if nlen > 4096 {
            return Err(AnimErr::Bad(format!("implausible clip name length {nlen}")));
        }
        let name = String::from_utf8_lossy(c.take(nlen)?).into_owned();
        let duration = c.f32()?;
        let mut bones = Vec::new();
        if !streamed {
            // Streamed records carry no bone list; their sub-animations live in the SSP0 block.
            let bc = c.u32()? as usize;
            if bc > 100_000 {
                return Err(AnimErr::Bad(format!("implausible track count {bc}")));
            }
            let raw = c.take(bc * 4)?;
            bones.reserve(bc);
            for i in 0..bc {
                bones.push(u32::from_le_bytes([
                    raw[i * 4],
                    raw[i * 4 + 1],
                    raw[i * 4 + 2],
                    raw[i * 4 + 3],
                ]));
            }
        }
        c.take(8 * 4)?; // f32 unk0[8]
        let _unk1 = c.u8()?;
        let n2 = c.u32()? as usize;
        c.take(n2.checked_mul(40).ok_or(AnimErr::Truncated)?)?; // ANIMStruct0 = u32[10]
        let n3 = c.u32()? as usize;
        c.take(n3.checked_mul(17).ok_or(AnimErr::Truncated)?)?; // ANIMStruct1 = u32,u8,f32[2],u32
        if !streamed {
            out.push(RawAnim { name, duration, bones });
        }
    }
    // The structural self-check: the cursor must now sit on `numAnims` / `hkSize`, and `hkSize` must
    // name the Havok blob that starts 8 bytes later. If the walk drifted, this is where we find out
    // — rather than by listing 3000 clips of garbage.
    let num_anims = c.u32()? as usize;
    let _hk_size = c.u32()?;
    let magic = c.take(4)?;
    if magic != [0x57, 0xe0, 0xe0, 0x57] {
        return Err(AnimErr::Bad(
            "ANIM walk did not land on the Havok blob — record layout mismatch".into(),
        ));
    }
    // Retail: 2214 == 2214. A mismatch is worth saying out loud, but the magic above already proved
    // the walk landed correctly, so it must not cost the user their clip list.
    if num_anims != out.len() {
        eprintln!(
            "[sab_workshop] ANIM: numAnims {num_anims} != {} non-streamed records",
            out.len()
        );
    }
    Ok(out)
}

/// Read a generated `anim_bone_map.json` (the `--index` override).
pub fn load(path: &str) -> Result<AnimCatalog, String> {
    let text: String = std::fs::read_to_string(path).map_err(|e| format!("read {path}: {e}"))?;
    let raw: RawIndex = serde_json::from_str(&text).map_err(|e| format!("parse {path}: {e}"))?;
    let clips = raw
        .clips
        .into_iter()
        .map(|c| {
            let track_to_bone = c
                .bone_ids
                .iter()
                .map(|&b| if b == UNBOUND || b < 0 || b > i32::MAX as i64 { -1 } else { b as i32 })
                .collect();
            let playable = c.bone_repr == "index" && c.subset_of_skeleton;
            let num_tracks = if c.num_transform_tracks > 0 { c.num_transform_tracks } else { c.num_tracks };
            ClipInfo { index: c.index, name: c.name, duration: c.duration, num_tracks, track_to_bone, playable }
        })
        .collect();
    Ok(AnimCatalog { clips, skeleton_bones: raw.skeleton_bones, num_main_clips: raw.num_main_clips })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ANIM block's own cross-check, inside the same file: `len(bones) == numTransformTracks`
    /// of the N-th `hkaSplineCompressedAnimation` in the Havok blob. The two structures are written
    /// independently, so agreeing on all 2214 clips proves the record walk stayed in step — the thing
    /// a stored reference catalog could only ever assert by having been generated from the same walk.
    #[test]
    fn every_clip_matches_the_havok_blobs_track_count() {
        let Some(s) = crate::settings::detected_retail() else {
            eprintln!("skip: no Saboteur install detected");
            return;
        };
        let path = s.anim_pack();
        if !std::path::Path::new(&path).exists() {
            eprintln!("skip: {path} not present");
            return;
        }
        let got = from_pack(&path, 191).expect("read catalog from the pack");
        assert!(!got.clips.is_empty(), "no clips");

        // The independent witness: walk the Havok packfile and read numTransformTracks per spline
        // animation, in file order.
        let file = std::fs::read(&path).expect("read pack");
        let ap = crate::havok::parse_ap0l(&file).expect("AP0L");
        let blob = &file[ap.blob_off..ap.blob_off + ap.hk_size];
        let pk = crate::havok::Packfile::parse(blob).expect("havok packfile");
        let counts: Vec<usize> = pk
            .vfixups
            .iter()
            .filter(|(_, c)| c == "hkaSplineCompressedAnimation")
            .map(|(src, _)| crate::havok::read_spline_anim(&pk, *src).num_transform_tracks)
            .collect();

        assert_eq!(counts.len(), got.clips.len(), "spline anims vs ANIM records");
        let mut bad = 0;
        for (i, (c, n)) in got.clips.iter().zip(&counts).enumerate() {
            if c.num_tracks != *n {
                if bad < 10 {
                    eprintln!("  clip {i} '{}': {} tracks, blob says {n}", c.name, c.num_tracks);
                }
                bad += 1;
            }
        }
        assert_eq!(bad, 0, "{bad} clips disagree with the Havok blob");
        let playable = got.clips.iter().filter(|c| c.playable).count();
        eprintln!("{} clips, all track counts agree, {playable} playable on a 191-bone rig", got.clips.len());
    }

    /// Clip names have to be real: the list is searched by name, and a drifted record walk yields
    /// plausible-looking garbage rather than an obvious failure.
    #[test]
    fn clip_names_are_sane() {
        let Some(s) = crate::settings::detected_retail() else {
            eprintln!("skip: no Saboteur install detected");
            return;
        };
        let Ok(got) = from_pack(&s.anim_pack(), 191) else {
            eprintln!("skip: no readable Animations.pack");
            return;
        };
        for c in got.clips.iter().take(200) {
            assert!(!c.name.is_empty(), "clip {} has no name", c.index);
            assert!(
                c.name.chars().all(|ch| ch.is_ascii_graphic() || ch == ' '),
                "clip {} name is not printable: {:?}",
                c.index,
                c.name
            );
            assert!(c.duration >= 0.0 && c.duration < 3600.0, "clip '{}' duration {}", c.name, c.duration);
        }
    }

    /// A file that is not an animation pack must fail with a diagnosis, not by growing the read to
    /// half a gigabyte looking for a record layout that was never there.
    #[test]
    fn rejects_a_file_that_is_not_an_anim_pack() {
        let p = std::env::temp_dir().join("sab_workshop_not_a_pack.bin");
        std::fs::write(&p, b"NOPE and then some filler bytes to read").expect("write");
        let e = match from_pack(&p.to_string_lossy(), 191) {
            Err(e) => e,
            Ok(c) => panic!("must reject, but read {} clips", c.clips.len()),
        };
        assert!(e.contains("AP0L"), "unhelpful error: {e}");
        let _ = std::fs::remove_file(&p);
    }
}
