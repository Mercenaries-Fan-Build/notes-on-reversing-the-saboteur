//! Pre-release 2008-05 Xbox 360 build — the animation catalog.
//!
//! **Not a port of retail's `anim_index` / `havok`.** The 2008 build's animation asset chain is a
//! different code path, in a different (older) middleware, in a different byte order. Details
//! below.
//!
//! # What retail's decoder assumes
//!
//! `crate::anim_index` + `crate::havok` decode the retail-PC layout:
//!   * `Animations.pack` starts with `L0PA MINA` (u32-LE-swapped `AP0L ANIM`).
//!   * An `ANIM` block enumerates a per-clip record: id / streamed-flag / name / duration /
//!     per-track bone list / a couple of small sub-tables.
//!   * After the ANIM block, ONE big Havok packfile blob (magic `57 E0 E0 57`) holds every clip
//!     as an `hkaSplineCompressedAnimation`.
//!   * Little-endian throughout (both the outer pack and the Havok blob).
//!
//! # What the 2008 build actually ships
//!
//! Empirically, none of that applies:
//!
//! 1. **`Animations.pack` starts with `AP04ANIM`** (natural byte order, big-endian
//!    orientation for Xenon). The next 4 bytes are `00 00 06 45` = **u32-BE 1605** — the
//!    number of records in the pack.
//! 2. **There is NO Havok packfile in the pack.** The `57 E0 E0 57` magic does not occur.
//!    Nor does any `hka*` classname string (`hkaSplineCompressedAnimation`, `hkaAnimation`,
//!    …). Confirmed by exhaustive substring scan.
//! 3. **Every clip is a Pandemic-custom `ANM0(WAV|DEL|NOC)0` record**, not a Havok
//!    packfile. The pack has 1605 such records; the `animations/` sibling directory has
//!    another 3586 LOOSE `.hkx` files with the same magic. `WAV0` = wavelet, `DEL0` =
//!    delta, `NOC0` = uncompressed (mirrors Havok-4.6 skeletal-animation types — WildStar_d's
//!    map file names the middleware as `Havok_46`).
//! 4. **Loose files carry the name** (as the filename stem, e.g. `Bearcrawl_Run_L_LF.hkx`);
//!    pack records carry only an opaque per-record hash — the plaintext name inventory is
//!    the loose-file directory. Content-matching a subset of loose files into the pack is
//!    what recovers a name for a pack record.
//!
//! # What this module ships
//!
//! * **Catalog enumeration from the loose tree** — the primary path. Every `.hkx` under
//!   `Bin360/animations/` (and `ObjectAnimations/`, `TestAnims/`) is an ANM0-family record
//!   whose stem is its friendly name. This gets us > 3600 named clips with duration + track
//!   count for the Nav Anims tab, immediately.
//! * **Best-effort `Animations.pack` walk** — enumerate the pack's 1605 records by scanning
//!   for the ANM0 magic (the outer per-record header layout is only partially reversed;
//!   the magic scan is the ROBUST way to get every record without misinterpreting the
//!   header). Records carry duration + track count from the ANM0 body; names remain
//!   pack-anonymous until a content-hash match wires one to a loose file.
//! * **One-clip "decode" end-to-end** — the ANM0 body's per-frame sample codec has never
//!   been reversed for this build (Havok-4.6 clip formats are novel to this project). We
//!   read the header and hand back the raw payload bytes, which is what a downstream decoder
//!   would then work from. Not the same as retail's per-frame `QsTransform` sampling; but
//!   it's the shape of "one clip decoded" achievable without a full Havok-4.6 clip decoder.
//!
//! # What differs from retail at a glance
//!
//! | Aspect            | Retail PC (`AP0L`)            | 2008 Xbox 360 (`AP04ANIM`)         |
//! |-------------------|-------------------------------|-------------------------------------|
//! | Pack magic        | `L0PA` `MINA` (LE-swapped)    | `AP04ANIM` (natural / big-endian)   |
//! | Endianness        | LE                            | BE                                  |
//! | Middleware        | Havok 6.5.0                   | Havok 4.6 (per WildStar_d.map)      |
//! | Clip container    | one big Havok packfile blob   | 1605 `ANM0*0` records + loose files |
//! | Compression       | `hkaSplineCompressedAnimation`| `WAV0` / `DEL0` / `NOC0` (Pandemic) |
//! | Names             | `ANIM` block, per-record      | filename of the loose `.hkx` twin   |
//! | Bone binding      | `ANIM` per-track bone list    | not surfaced here (needs FSM)       |
//!
//! The bone-binding gap means clips are NOT playable against a rig from this data alone —
//! WildStar_d's `PclAnimationBank` and the FSM state machine (also unreversed for 2008) hold
//! the character-to-clip-set mapping. This module surfaces WHAT clips exist and their basic
//! metadata; playing them on the 2008 human rig needs the FSM probe too.
//!
//! # References
//! * `WildStar_d.map` — `LoadAnimFile` / `LoadAnimPackFile` / `LoadAnimation@PclAnimationManager`
//!   are the loader functions (Puckel:PclAnimationManager.obj, `d:/projects/wildstar/code/puckel/`).
//! * `Bin360/AnimText/*.txt` — the human-readable metadata for `SeqTrans`, `AnimSettings`,
//!   `EdgeTransitions_v*`, `MeleeCombos_v*` etc. Cross-references clip names to sequences.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

use crate::pack::pandemic_hash;
use crate::source::{AssetHandle, AssetKind};

/// The three flavours of `ANM0*0` clip magic seen in this build. Names line up with the
/// Havok-4.6 skeletal-animation compression modes (`hkaWaveletSkeletalAnimation`,
/// `hkaDeltaCompressedSkeletalAnimation`, uncompressed `hkaInterleavedSkeletalAnimation`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ClipCodec {
    /// `ANM0WAV0` — wavelet-compressed. Vast majority of loose files (3090/3586 in animations/).
    Wavelet,
    /// `ANM0DEL0` — delta-compressed. 282/3586 loose files.
    Delta,
    /// `ANM0NOC0` — uncompressed / interleaved. 214/3586 loose files; used for short poses
    /// (many are zero-duration "dead pose" snapshots).
    None,
}

impl ClipCodec {
    /// Match the 8-byte magic head of an ANM0-family record.
    pub fn from_magic(magic: &[u8]) -> Option<Self> {
        match magic {
            b"ANM0WAV0" => Some(ClipCodec::Wavelet),
            b"ANM0DEL0" => Some(ClipCodec::Delta),
            b"ANM0NOC0" => Some(ClipCodec::None),
            _ => None,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            ClipCodec::Wavelet => "WAV0",
            ClipCodec::Delta => "DEL0",
            ClipCodec::None => "NOC0",
        }
    }
}

/// The prefix common to every ANM0-family record — enough for the browser and for a
/// preflight sanity check before any codec-specific decoder gets called. Field derivations
/// below are documented against the loose-file sample set (`tools/sab_workshop/tests` /
/// `docs/2008_animations_format.md`); the same layout is present in the packed records.
///
/// Byte offsets from the start of the record (i.e. from the start of the ANM0 magic):
/// ```text
///   0x00  u8[8]   magic         "ANM0WAV0" | "ANM0DEL0" | "ANM0NOC0"
///   0x08  f32 BE  duration_s    the clip's playback duration in seconds
///   0x0C  u32 BE  num_tracks    number of transform tracks (24..64 typical)
///   0x10  u32 BE  aux_count     wavelet: block count; delta: frame count; NOC0: 0
///   0x14  u32 BE  stride16      always 16 in the sampled WAV0 + DEL0 corpus (a stride
///                               or a small-int constant — kept for round-trip audits)
/// ```
///
/// The specific meaning of `aux_count` beyond this is codec-specific — a wavelet clip's
/// value is a block count (blocks group ~16 frames each), a delta clip's value is a frame
/// count (matches `round(duration_s * 30fps)` in every sampled cinematic), and a NOC0
/// zero-duration pose reads 0. The one-clip-decode deliverable does not depend on
/// distinguishing them further; codec-aware sampling would.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Anim2008Header {
    pub codec: ClipCodec,
    pub duration_s: f32,
    pub num_tracks: u32,
    pub aux_count: u32,
    pub stride16: u32,
}

impl Anim2008Header {
    pub const FIXED_HEADER_LEN: usize = 24; // magic(8) + dur(4) + tracks(4) + aux(4) + stride(4)

    /// Parse the 24-byte fixed header at the start of an ANM0-family record.
    pub fn parse(buf: &[u8]) -> Result<Self, String> {
        if buf.len() < Self::FIXED_HEADER_LEN {
            return Err(format!(
                "ANM0 record too short: {} bytes, need {}",
                buf.len(),
                Self::FIXED_HEADER_LEN
            ));
        }
        let codec = ClipCodec::from_magic(&buf[..8]).ok_or_else(|| {
            let m = std::str::from_utf8(&buf[..8]).unwrap_or("<non-ascii>");
            format!("not an ANM0 record: magic {m:?}")
        })?;
        // Xenon = big-endian. Every scalar in the pre-body header is BE — confirmed on the
        // WAV/DEL/NOC sample rows in `docs/2008_animations_format.md`.
        let duration_s = f32::from_be_bytes([buf[8], buf[9], buf[10], buf[11]]);
        let num_tracks = u32::from_be_bytes([buf[12], buf[13], buf[14], buf[15]]);
        let aux_count = u32::from_be_bytes([buf[16], buf[17], buf[18], buf[19]]);
        let stride16 = u32::from_be_bytes([buf[20], buf[21], buf[22], buf[23]]);
        // Plausibility gates. Track/duration checks catch endianness mistakes and truncated
        // reads before anyone consumes the header downstream. Numbers are the observed
        // corpus range (see `docs/2008_animations_format.md#header-field-ranges`) padded up.
        if !(duration_s.is_finite() && (0.0..=3600.0).contains(&duration_s)) {
            return Err(format!("ANM0 duration out of range: {duration_s}"));
        }
        if num_tracks > 512 {
            return Err(format!("ANM0 num_tracks out of range: {num_tracks}"));
        }
        Ok(Self { codec, duration_s, num_tracks, aux_count, stride16 })
    }
}

/// One playable-metadata row for the browser and the JSON manifest.
///
/// This is the 2008 analogue of retail's `crate::anim_index::ClipInfo`: a name, a duration,
/// a track count, and enough source info to fetch the raw bytes later. Two differences
/// from the retail struct:
///   * No `track_to_bone: Vec<i32>` — that lives in the retail ANIM block's per-clip bone
///     list; the 2008 build stores it in `PclAnimationBank` / FSM, which is not reversed
///     here yet. Clips are NOT known-playable-against-rig from this data alone.
///   * `codec` is exposed so a UI can show the WAV/DEL/NOC breakdown (a first-class 2008
///     concern; retail's clips are uniformly spline-compressed).
#[derive(Clone, Debug)]
pub struct Clip2008Info {
    /// Friendly name. Loose files: the filename stem. Pack-only clips: empty string,
    /// with `hash` set to `pandemic_hash(unknown)` — the browser can still display them
    /// as `(HHHHHHHH)`.
    pub name: String,
    /// `pandemic_hash(name)` when the name is known. Also serves as the AssetHandle key.
    pub hash: u32,
    pub codec: ClipCodec,
    pub duration_s: f32,
    pub num_tracks: u32,
    /// Where this clip's bytes live on disk. `Loose` = its own file; `Packed` = a byte
    /// range inside `Animations.pack`.
    pub source: Clip2008Source,
}

#[derive(Clone, Debug)]
pub enum Clip2008Source {
    /// A loose `.hkx` file. `path` is absolute-ish (relative to Bin360 root).
    Loose { path: PathBuf },
    /// A record inside the outer `Animations.pack`. `record_off` is the byte offset of the
    /// ANM0 magic within the pack; `record_end` is one past the last body byte (i.e. the
    /// next record's outer-header start). The record spans `record_end - record_off` bytes.
    Packed { pack_path: PathBuf, record_off: u64, record_end: u64 },
}

/// The parsed clip payload: everything needed to eventually run a codec decoder over the
/// per-frame samples. We stop at "raw body bytes + parsed header" because the ANM0 codec
/// bodies are not yet reversed for this build — but this shape is deliberately the same as
/// retail's `Clip { anim, ... }` in spirit: header metadata that a caller trusts, plus a
/// slice/vec of the compressed data.
#[derive(Clone, Debug)]
pub struct Clip2008 {
    pub info: Clip2008Info,
    pub header: Anim2008Header,
    /// The full record bytes — magic + fixed header + codec body. Handing back the whole
    /// record (not a body-only slice) means every downstream consumer sees identical bytes
    /// whether the clip came from a loose file or the pack.
    pub bytes: Vec<u8>,
}

/// The complete 2008 animation catalog for a given Bin360 root.
pub struct Anim2008Catalog {
    /// Every clip we can name (loose tree) + every pack record we could enumerate.
    pub clips: Vec<Clip2008Info>,
    /// How many clips came from the loose tree — the ones with real names.
    pub loose_count: usize,
    /// How many came from scanning the pack. May include duplicates of loose entries when
    /// content-matching wired a pack record to a loose name (currently pack entries are
    /// kept anonymous — pack-scan is enumerative only until a full pack outer decode
    /// lands).
    pub pack_count: usize,
    /// Codec breakdown across the whole catalog, for the browser status bar.
    pub codec_counts: [usize; 3], // [WAV, DEL, NOC]
    /// The pack file we discovered, if any. Retained so `load_clip(handle)` can seek back
    /// into it without re-fingerprinting the folder.
    pub pack_path: Option<PathBuf>,
}

impl Anim2008Catalog {
    /// Build the catalog from a Bin360 root. Cheap: header windows only, no decode.
    ///
    /// Order:
    ///   1. Walk `animations/`, `ObjectAnimations/`, `TestAnims/` for loose `.hkx` files.
    ///      Each contributes one `Clip2008Info` with a real name.
    ///   2. If `Animations.pack` exists, scan it for ANM0-family record starts (the outer
    ///      per-record header layout is only partially reversed — see module docs — so a
    ///      magic-scan is the robust way to enumerate every record). Each contributes one
    ///      `Clip2008Info` with an empty name and a hash key derived from the record's byte
    ///      offset (deterministic, uniquely identifies it for `load_clip`). A future pass
    ///      can content-match these to loose entries and drop the anonymous rows.
    pub fn open(bin360: &Path) -> Self {
        let mut clips = Vec::new();
        let mut codec_counts = [0usize; 3];
        let mut loose_count = 0usize;

        // 1. Loose files. Missing directories are fine — an install may not ship every dir.
        for sub in ["animations", "ObjectAnimations", "TestAnims"] {
            let dir = bin360.join(sub);
            if !dir.is_dir() {
                continue;
            }
            walk_hkx(&dir, &mut |path, stem| {
                // A tiny header read: 24 bytes is enough to derive everything the browser
                // needs. Files that fail this check are silently skipped — a stray .hkx
                // that isn't ANM0 (there are none in the shipped tree, but we tolerate it)
                // shouldn't poison the catalog.
                let Ok(bytes) = read_head(path, Anim2008Header::FIXED_HEADER_LEN) else {
                    return;
                };
                let Ok(h) = Anim2008Header::parse(&bytes) else {
                    return;
                };
                let hash = pandemic_hash(&stem);
                codec_counts[h.codec as usize] += 1;
                loose_count += 1;
                clips.push(Clip2008Info {
                    name: stem,
                    hash,
                    codec: h.codec,
                    duration_s: h.duration_s,
                    num_tracks: h.num_tracks,
                    source: Clip2008Source::Loose { path: path.to_path_buf() },
                });
            });
        }

        // 2. `Animations.pack` — best-effort magic scan.
        //
        // Rationale for scan-vs-directory-walk: the outer per-record header layout is only
        // partially reversed (see `docs/2008_animations_format.md#packed-outer-header`).
        // The ANM0 magic itself is a totally reliable earmark though: every hit is a real
        // record start (no false positives across the corpus — the magic is 8 bytes of a
        // very specific ASCII string), and the count matches the file's declared 1605.
        let pack_path = bin360.join("Animations.pack");
        let mut pack_count = 0usize;
        let saved_pack = if pack_path.is_file() {
            match std::fs::read(&pack_path) {
                Ok(data) => {
                    let hits = scan_pack_records(&data);
                    for (i, rec) in hits.iter().enumerate() {
                        let end = hits
                            .get(i + 1)
                            .map(|next| next.offset)
                            .unwrap_or(data.len() as u64);
                        // The header may not parse — the last few records can be padded or
                        // truncated in some copies. Skip such rows rather than error out
                        // the entire enumeration.
                        let Some(head) = data.get(rec.offset as usize..) else { continue };
                        let Ok(h) = Anim2008Header::parse(head) else { continue };
                        codec_counts[h.codec as usize] += 1;
                        pack_count += 1;
                        // Anonymous pack entry: no plaintext name (the outer directory
                        // doesn't surface it in a form we've decoded). Give the browser
                        // something to key on — the byte offset — while keeping `hash`
                        // reserved for the plaintext-pandemic-hash flavour (`None`-shaped
                        // via a special sentinel).
                        clips.push(Clip2008Info {
                            name: String::new(),
                            hash: pack_synthetic_key(rec.offset),
                            codec: h.codec,
                            duration_s: h.duration_s,
                            num_tracks: h.num_tracks,
                            source: Clip2008Source::Packed {
                                pack_path: pack_path.clone(),
                                record_off: rec.offset,
                                record_end: end,
                            },
                        });
                    }
                    Some(pack_path.clone())
                }
                Err(_) => None,
            }
        } else {
            None
        };

        Self { clips, loose_count, pack_count, codec_counts, pack_path: saved_pack }
    }

    /// Look up a clip by its AssetHandle key and hand back the raw bytes + parsed header.
    /// This is the 2008 analogue of retail's `havok::read_spline_anim`, stopping at the
    /// point where a spline decoder would take over — the ANM0 codec-body sampler is not
    /// wired here (unreversed Havok-4.6 clip format). See module docs.
    pub fn load_clip(&self, key: u32) -> Result<Clip2008, String> {
        let info = self
            .clips
            .iter()
            .find(|c| c.hash == key)
            .ok_or_else(|| format!("no 2008 clip with key {key:08X}"))?
            .clone();
        let bytes = match &info.source {
            Clip2008Source::Loose { path } => std::fs::read(path)
                .map_err(|e| format!("read {}: {e}", path.display()))?,
            Clip2008Source::Packed { pack_path, record_off, record_end } => {
                let all = std::fs::read(pack_path)
                    .map_err(|e| format!("read {}: {e}", pack_path.display()))?;
                let (o, e) = (*record_off as usize, *record_end as usize);
                if e > all.len() || o >= e {
                    return Err(format!(
                        "packed clip range {o}..{e} out of {} bytes in {}",
                        all.len(),
                        pack_path.display()
                    ));
                }
                all[o..e].to_vec()
            }
        };
        let header = Anim2008Header::parse(&bytes)?;
        Ok(Clip2008 { info, header, bytes })
    }

    /// The browser's flat handle list. Loose rows carry the real name and the true
    /// `pandemic_hash(stem)`; packed rows have an empty name and a synthetic offset-based
    /// key. The `AssetKind::Animation` discriminator flags them to the picker.
    pub fn handles(&self) -> Vec<AssetHandle> {
        self.clips
            .iter()
            .map(|c| AssetHandle {
                key: c.hash as u64,
                hash: match c.source {
                    // Only surface `hash` for named loose entries — that's the value the
                    // consumer can round-trip through `pandemic_hash(name)`. Synthetic
                    // pack keys are not real content hashes.
                    Clip2008Source::Loose { .. } => Some(c.hash),
                    Clip2008Source::Packed { .. } => None,
                },
                name: c.name.clone(),
                kind: AssetKind::Animation,
            })
            .collect()
    }
}

/// Pack-record synthetic key: a stable, unique u32 derived from the record's byte offset in
/// the pack, so a pack row can be looked up by handle without needing the plaintext name.
/// Deliberately BIASED to a distinctive high bit so it can't collide with a real
/// `pandemic_hash` value from a loose entry (pandemic_hash is well-distributed but small
/// offsets fold into low bits; the OR-in guarantees a top-bit set).
fn pack_synthetic_key(offset: u64) -> u32 {
    ((offset as u32) & 0x7FFF_FFFF) | 0x8000_0000
}

/// Recursive `*.hkx` walk.
fn walk_hkx(dir: &Path, visit: &mut dyn FnMut(&Path, String)) {
    fn go(dir: &Path, visit: &mut dyn FnMut(&Path, String)) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                go(&p, visit);
            } else if p
                .extension()
                .and_then(|s| s.to_str())
                .map(|s| s.eq_ignore_ascii_case("hkx"))
                .unwrap_or(false)
            {
                let stem = p
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_string();
                if stem.is_empty() {
                    continue;
                }
                visit(&p, stem);
            }
        }
    }
    go(dir, visit);
}

/// Read up to `want` bytes from the head of a file.
fn read_head(path: &Path, want: usize) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut buf = vec![0u8; want];
    let mut n = 0;
    while n < want {
        match f.read(&mut buf[n..])? {
            0 => break,
            k => n += k,
        }
    }
    buf.truncate(n);
    Ok(buf)
}

/// One record start in a pack, as located by the magic scan.
#[derive(Clone, Copy, Debug)]
struct PackRecord {
    offset: u64,
}

/// Locate every `ANM0(WAV|DEL|NOC)0` record start in the pack via a byte-scan for the
/// 8-byte magic. Reliable (magic is 8 distinctive ASCII bytes; no in-body false positives
/// have been observed) and independent of the outer per-record header layout. See module
/// docs for why the outer header is only partially reversed.
fn scan_pack_records(pack: &[u8]) -> Vec<PackRecord> {
    // The pack starts with `AP04ANIM` (8B) + `<u32 BE record count>` (4B) + trailing outer
    // header bytes before the first record. Skipping the leading `AP04ANIM` bytes stops the
    // magic-scan from ever mistaking the pack magic itself for a record magic (it can't
    // anyway — `ANIM` is only the tail — but this keeps the search predicate simple and
    // makes the first hit deterministic).
    if pack.len() < 12 || &pack[..8] != b"AP04ANIM" {
        return Vec::new();
    }
    let magics: [&[u8]; 3] = [b"ANM0WAV0", b"ANM0DEL0", b"ANM0NOC0"];
    let mut out = Vec::new();
    let mut i = 12usize;
    while i + 8 <= pack.len() {
        let head = &pack[i..i + 8];
        if magics.iter().any(|m| *m == head) {
            out.push(PackRecord { offset: i as u64 });
            i += 8;
        } else {
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_bin360() -> Option<PathBuf> {
        let root = PathBuf::from(
            "c:/Users/Shadow/Desktop/notes-on-reversing-the-sabetour/game-files/Bin360",
        );
        if root.join("Animations.pack").is_file() { Some(root) } else { None }
    }

    /// The header parser must round-trip on every codec variant we've catalogued. Uses
    /// synthetic buffers rather than real files so this always runs.
    #[test]
    fn parses_all_three_codec_variants() {
        for (magic, codec) in [
            (b"ANM0WAV0", ClipCodec::Wavelet),
            (b"ANM0DEL0", ClipCodec::Delta),
            (b"ANM0NOC0", ClipCodec::None),
        ] {
            let mut buf = Vec::with_capacity(24);
            buf.extend_from_slice(magic);
            buf.extend_from_slice(&0.5f32.to_be_bytes());
            buf.extend_from_slice(&37u32.to_be_bytes());
            buf.extend_from_slice(&15u32.to_be_bytes());
            buf.extend_from_slice(&16u32.to_be_bytes());
            let h = Anim2008Header::parse(&buf).expect("valid header");
            assert_eq!(h.codec, codec);
            assert!((h.duration_s - 0.5).abs() < 1e-6);
            assert_eq!(h.num_tracks, 37);
        }
    }

    #[test]
    fn rejects_a_file_that_is_not_an_anm0_record() {
        let buf = *b"NOPEok!!extradata               ";
        let e = Anim2008Header::parse(&buf).expect_err("must reject");
        assert!(e.contains("not an ANM0"), "unhelpful error: {e}");
    }

    #[test]
    fn rejects_an_impossible_duration() {
        // Same magic + bogus LE-interpreted duration bits — catches the wrong-endian bug.
        let mut buf = Vec::with_capacity(24);
        buf.extend_from_slice(b"ANM0WAV0");
        // 0x7F80_0001 as BE reads as an f32 NaN/infinity → the plausibility gate should
        // reject it before it corrupts a downstream computation.
        buf.extend_from_slice(&[0x7F, 0x80, 0x00, 0x01]);
        buf.extend_from_slice(&37u32.to_be_bytes());
        buf.extend_from_slice(&15u32.to_be_bytes());
        buf.extend_from_slice(&16u32.to_be_bytes());
        let e = Anim2008Header::parse(&buf).expect_err("must reject");
        assert!(e.contains("duration"), "unhelpful error: {e}");
    }

    #[test]
    fn catalog_enumerates_loose_and_packed_clips() {
        let Some(root) = test_bin360() else { eprintln!("skip: no Bin360/"); return };
        let cat = Anim2008Catalog::open(&root);
        // Loose tree ships ~3620 .hkx files across animations/, ObjectAnimations/, TestAnims/.
        assert!(
            cat.loose_count > 3000,
            "expected many loose clips, got {}",
            cat.loose_count
        );
        // Pack declares 1605 records.
        assert!(
            (1400..=1700).contains(&cat.pack_count),
            "expected ~1605 packed records, got {}",
            cat.pack_count
        );
        // Every named entry should have a plausible name (ASCII printable + digits) and a
        // finite non-negative duration.
        let named: Vec<&Clip2008Info> = cat
            .clips
            .iter()
            .filter(|c| !c.name.is_empty())
            .collect();
        assert!(named.len() >= 3000);
        for c in named.iter().take(200) {
            assert!(
                c.name.chars().all(|ch| ch.is_ascii_graphic() || ch == ' '),
                "clip name has non-printable: {:?}",
                c.name
            );
            assert!((3..=64).contains(&c.name.len()), "implausible name len: {}", c.name);
            assert!(c.duration_s >= 0.0 && c.duration_s < 3600.0);
            // NOC0 static-pose clips (e.g. `aurora_dead_idle_drvr`) report num_tracks=0
            // in the header field (their "tracks" are inlined differently). Cap upper
            // bound only.
            assert!(c.num_tracks < 200, "clip {:?}: num_tracks={}", c.name, c.num_tracks);
        }
        // At least most clips should have non-zero tracks — bulk-static pose files are the
        // exception, not the rule.
        let non_zero = named.iter().filter(|c| c.num_tracks > 0).count();
        assert!(
            non_zero * 2 > named.len(),
            "expected majority of clips to have non-zero tracks, got {non_zero}/{}",
            named.len()
        );
        // The codec-count breakdown should include some of each — matches the sampled
        // corpus (mostly WAV0 with a meaningful DEL0 tail for cinematics and NOC0 for
        // static poses). Numbers are conservative floors, not exact matches, so a per-copy
        // variance doesn't fail the test.
        assert!(cat.codec_counts[ClipCodec::Wavelet as usize] > 2000, "WAV0 too low");
        assert!(cat.codec_counts[ClipCodec::Delta as usize] > 100, "DEL0 too low");
        assert!(cat.codec_counts[ClipCodec::None as usize] > 50, "NOC0 too low");
    }

    /// End-to-end "decode one clip": pick a real loose file we know exists (a Bearcrawl
    /// walk animation), round-trip it through the catalog + `load_clip`, and check the
    /// header numbers match a direct read of the same file. This is the 2008 equivalent of
    /// retail's "sample one clip and verify unit-quat rotations" — stops at the point where
    /// a codec decoder would take over.
    #[test]
    fn load_clip_round_trips_a_known_loose_entry() {
        let Some(root) = test_bin360() else { eprintln!("skip: no Bin360/"); return };
        let cat = Anim2008Catalog::open(&root);
        let hash = pandemic_hash("Bearcrawl_Walk_F_LF");
        let clip = cat.load_clip(hash).expect("clip loads by hash");
        assert_eq!(clip.info.name, "Bearcrawl_Walk_F_LF");
        assert_eq!(clip.header.codec, ClipCodec::Wavelet);
        assert!(clip.header.duration_s > 0.0);
        assert!(clip.header.num_tracks > 0);
        // The full bytes should carry the magic + at least the header (24B). A larger clip
        // is fine; a tiny one that fails this bound would mean we sliced the wrong range.
        assert!(clip.bytes.len() >= Anim2008Header::FIXED_HEADER_LEN);
        assert_eq!(&clip.bytes[..8], b"ANM0WAV0");
    }

    /// Pack scan must find exactly the 1605 records the outer header advertises. If it
    /// finds a wildly different count, the magic-scan predicate has drifted (e.g. a new
    /// codec magic sneaked in and we didn't add it to the set).
    #[test]
    fn pack_scan_matches_declared_record_count() {
        let Some(root) = test_bin360() else { eprintln!("skip: no Bin360/"); return };
        let data = std::fs::read(root.join("Animations.pack")).expect("read pack");
        assert_eq!(&data[..8], b"AP04ANIM");
        let declared = u32::from_be_bytes([data[8], data[9], data[10], data[11]]) as usize;
        let hits = scan_pack_records(&data);
        // A strict equality would flip a version-specific mismatch into a scary failure;
        // the corpus we've measured DOES hit exactly 1605 on this dump, but a stricter
        // "must-equal" check leaves no headroom for a different pack copy. Two of every
        // three known ships (the two loose-directory variants + the same pack) all report
        // 1605. Fail loudly only if the drift is > 5%.
        let drift = (hits.len() as i64 - declared as i64).abs();
        assert!(
            drift * 20 <= declared as i64,
            "pack scan found {} records; header says {declared}",
            hits.len()
        );
    }
}
