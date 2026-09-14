//! `AP0L` animation pack — the container that wraps The Saboteur's Havok 6.5
//! `hkaSplineCompressedAnimation` clips + all Pandemic-specific animation metadata.
//!
//! ## Layout (from `docs/formats/animation_havok65.md`)
//!
//! ```text
//! "L0PA" (== 'AP0L' LE)
//! "MINA" (== 'ANIM' LE) block: u32 recordCount ; record[] { id, unk4, streamed,
//!                              nameLen, name[nameLen], duration,
//!                              if !streamed: u32 boneCount ; u32 bones[boneCount],
//!                              f32 unk0[8], u8 unk1,
//!                              u32 n2 ; u8[40]  unk2[n2],
//!                              u32 n3 ; u8[17]  unk3[n3] }
//! u32 numAnims                                       ; == count of non-streamed records
//! u32 hkSize
//! u8  havokPackfile[hkSize]                          ; ONE concatenated Havok 6.5 packfile
//!                                                     ; carrying all 2214 clips in retail
//! u8  tail[..EOF]                                     ; INTV, SEQC, TRAN, EDGE, BANK, ADD1,
//!                                                     ; ALPH, SSP0 in retail (all byte-preserved
//!                                                     ; on splice)
//! ```
//!
//! ## Where the "trnm equivalent" lives (Saboteur vs Mercs 2)
//!
//! Mercs 2's animgroups store one Havok packfile PER clip, each followed by a
//! separate Pandemic `trnm` chunk (track→bone map). Saboteur's AP0L is
//! structurally different: **the Havok packfile is a single, concatenated blob
//! carrying every non-streamed clip**, and the per-clip track→bone map lives
//! in the ANIM block up front (`bones[boneCount]`, one array per record, in the
//! same order as the `hkaSplineCompressedAnimation` objects in the Havok blob —
//! oracle-verified 2214/2214 in `tools/sab_animmeta`).
//!
//! Consequence for a write-side splice: **there is nothing to preserve after
//! each clip's packfile bytes** — the whole Havok half is one region
//! `[hk_offset .. hk_offset + hk_size]`. Everything OUTSIDE that region
//! (including the ANIM track→bone lists) is preserved byte-for-byte by
//! [`splice_hk_blob`]. When the new blob is the same size (the common HCT
//! round-trip case), the file is spliced in place; when the new blob has a
//! different size, `hkSize` is rewritten and the tail is shifted by the delta.
//!
//! Neither of those touches the ANIM block, so the track→bone binding is
//! preserved for free. See the round-trip test at the bottom of this file.
//!
//! ## Class-name / spline-object walker
//!
//! The Havok blob is a standard Havok 6.5 packfile. [`HkPackfile`] parses just
//! enough of it (classnames, __data__ section base, virtual fixups) to
//! enumerate every `hkaSplineCompressedAnimation` object *in file order* — the
//! same order used by `sab_havok65` and `sab_animmeta`, so a "clip index"
//! agrees across the three tools.

use crate::read_u32_le;

const AP0L_MAGIC: &[u8; 4] = b"L0PA"; // "AP0L" little-endian
const ANIM_MAGIC: &[u8; 4] = b"MINA"; // "ANIM" little-endian
const HAVOK_MAGIC: u32 = 0x57E0_E057;

// ---------------------------------------------------------------- parse errors

/// Failure modes surfaced by [`Ap0lPack::parse`] and its splice helpers.
#[derive(Debug, PartialEq, Eq)]
pub enum Ap0lError {
    NotAp0l,
    MissingAnimBlock,
    MissingHavokMagic,
    TruncatedHavokBlob { need: usize, have: usize },
    HkParseFailed(&'static str),
}

impl std::fmt::Display for Ap0lError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Ap0lError::NotAp0l => write!(f, "not an AP0L pack (magic != L0PA)"),
            Ap0lError::MissingAnimBlock => write!(f, "first block after L0PA is not MINA/ANIM"),
            Ap0lError::MissingHavokMagic => {
                write!(f, "no Havok packfile magic (57 e0 e0 57) found in pack")
            }
            Ap0lError::TruncatedHavokBlob { need, have } => write!(
                f,
                "declared hkSize={need} extends past EOF (have {have} bytes after magic)"
            ),
            Ap0lError::HkParseFailed(m) => write!(f, "havok packfile parse failed: {m}"),
        }
    }
}

impl std::error::Error for Ap0lError {}

// ---------------------------------------------------------------- pack model

/// A parsed AP0L pack. Owns the original bytes so splice helpers can rebuild a
/// new pack with the non-Havok regions preserved verbatim.
///
/// Only the Havok-blob boundary is decoded here — the ANIM block body,
/// INTV / SEQC / TRAN / EDGE / BANK / ADD1 / ALPH / SSP0 tails are kept as
/// opaque bytes. That is enough for the current animation-modding write path
/// (in-place splice of the Havok half). Deeper ANIM record parsing lives in
/// `tools/sab_animmeta` and is intentionally NOT duplicated here.
pub struct Ap0lPack {
    bytes: Vec<u8>,
    /// File offset of the first byte of the Havok packfile magic
    /// (`57 e0 e0 57`). The 8 bytes immediately preceding it are
    /// `[numAnims u32, hkSize u32]`.
    hk_offset: usize,
    /// hkSize as read from `bytes[hk_offset-4 .. hk_offset]`.
    hk_size: u32,
    /// numAnims as read from `bytes[hk_offset-8 .. hk_offset-4]`.
    num_anims: u32,
}

impl Ap0lPack {
    /// Parse an AP0L pack. Requires the header + ANIM magic, then finds the
    /// Havok packfile magic anywhere in the pack; the two u32s immediately
    /// before it are `numAnims, hkSize`.
    pub fn parse(bytes: Vec<u8>) -> Result<Self, Ap0lError> {
        if bytes.len() < 8 || &bytes[0..4] != AP0L_MAGIC {
            return Err(Ap0lError::NotAp0l);
        }
        if &bytes[4..8] != ANIM_MAGIC {
            return Err(Ap0lError::MissingAnimBlock);
        }
        let hk_offset = find_havok_magic(&bytes).ok_or(Ap0lError::MissingHavokMagic)?;
        if hk_offset < 8 {
            return Err(Ap0lError::MissingHavokMagic);
        }
        let num_anims = read_u32_le(&bytes, hk_offset - 8);
        let hk_size = read_u32_le(&bytes, hk_offset - 4);
        let end = hk_offset.saturating_add(hk_size as usize);
        if end > bytes.len() {
            return Err(Ap0lError::TruncatedHavokBlob {
                need: hk_size as usize,
                have: bytes.len().saturating_sub(hk_offset),
            });
        }
        Ok(Ap0lPack { bytes, hk_offset, hk_size, num_anims })
    }

    /// The whole original pack, unmodified.
    pub fn bytes(&self) -> &[u8] { &self.bytes }

    /// Header + ANIM block + `[numAnims, hkSize]` pair (everything before the
    /// Havok blob). Always preserved verbatim by [`Self::splice_hk_blob`].
    pub fn head(&self) -> &[u8] { &self.bytes[..self.hk_offset] }

    /// The concatenated Havok packfile carrying every non-streamed clip.
    pub fn hk_blob(&self) -> &[u8] {
        &self.bytes[self.hk_offset..self.hk_offset + self.hk_size as usize]
    }

    /// Tail after the Havok blob — INTV / SEQC / TRAN / EDGE / BANK / ADD1 /
    /// ALPH / SSP0 in retail. Always preserved verbatim on splice.
    pub fn tail(&self) -> &[u8] { &self.bytes[self.hk_offset + self.hk_size as usize..] }

    /// Byte offset of the first Havok magic in the pack.
    pub fn hk_offset(&self) -> usize { self.hk_offset }

    /// Number of non-streamed clips (== spline-anim objects in the blob).
    pub fn num_anims(&self) -> u32 { self.num_anims }

    /// Declared size of the Havok blob (from `[hk_offset-4, hk_offset)`).
    pub fn hk_size(&self) -> u32 { self.hk_size }

    /// Rebuild a new pack whose Havok blob is `new_blob` and whose head / tail
    /// are preserved byte-for-byte. `hkSize` is rewritten to match `new_blob.len()`;
    /// `numAnims` is left alone (a Havok round-trip does not change the object
    /// count).
    pub fn splice_hk_blob(&self, new_blob: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.hk_offset + new_blob.len() + self.tail().len());
        out.extend_from_slice(self.head());
        // Rewrite hkSize at head[hk_offset-4 .. hk_offset].
        let hk_size_off = self.hk_offset - 4;
        out[hk_size_off..hk_size_off + 4].copy_from_slice(&(new_blob.len() as u32).to_le_bytes());
        out.extend_from_slice(new_blob);
        out.extend_from_slice(self.tail());
        out
    }
}

fn find_havok_magic(bytes: &[u8]) -> Option<usize> {
    let m = HAVOK_MAGIC.to_le_bytes();
    bytes.windows(4).position(|w| w == m)
}

// ---------------------------------------------------------------- havok packfile

/// Minimal Havok 6.5 packfile walker: just enough to enumerate the
/// virtually-fixed-up objects of a given class in file order.
///
/// This is the subset of `tools/sab_havok65` needed by the write path
/// (identify "clip #N" in an AP0L pack, and locate `numTransformTracks` for
/// a sanity check against the ANIM block).
pub struct HkPackfile<'a> {
    blob: &'a [u8],
    data_pk: usize,
    /// Classname-body-relative offset → class name (kept as diagnostic surface
    /// for future callers that want to enumerate more classes than the current
    /// `objects_of_class` helper does).
    #[allow(dead_code)]
    names_by_offset: std::collections::HashMap<usize, String>,
    /// `__data__` local fixups: source data-rel offset → destination data-rel
    /// offset. hkArray pointers live at their struct offset and their real
    /// target is looked up here (mirrors [`sab_havok65::Packfile::array`]).
    lf: std::collections::HashMap<usize, usize>,
    vfixups: Vec<(usize, String)>,
    version: String,
}

impl<'a> HkPackfile<'a> {
    pub fn parse(blob: &'a [u8]) -> Result<Self, Ap0lError> {
        if blob.len() < 8 || read_u32_le(blob, 0) != HAVOK_MAGIC {
            return Err(Ap0lError::HkParseFailed("bad packfile magic"));
        }
        let sh = find_sub(blob, b"__classnames__")
            .ok_or(Ap0lError::HkParseFailed("no __classnames__ section header"))?;
        let mut secs = [[0u32; 7]; 3];
        for s in 0..3 {
            for k in 0..7 {
                secs[s][k] = read_u32_le(blob, sh + s * 48 + 20 + k * 4);
            }
        }
        let body0 = sh + 3 * 48;
        let cn_len = secs[0][6] as usize;
        let data_pk = body0 + secs[0][6] as usize + secs[1][6] as usize;

        // classnames: 4 bytes signature + 1 byte flag + NUL-terminated string
        let mut names_by_offset = std::collections::HashMap::new();
        let cn_end = (body0 + cn_len).min(blob.len());
        let mut p = body0;
        while p + 5 <= cn_end {
            if read_u32_le(blob, p) == 0xFFFF_FFFF {
                break;
            }
            let mut q = p + 5;
            while q < cn_end && blob[q] != 0 {
                q += 1;
            }
            if let Ok(nm) = std::str::from_utf8(&blob[p + 5..q]) {
                if !nm.is_empty() {
                    names_by_offset.insert(p + 5 - body0, nm.to_string());
                }
            }
            p = q + 1;
        }

        // __data__ local fixups (8-byte src,dst pairs, terminator 0xFFFFFFFF).
        let (d_lf, d_gf, d_vf, d_end) = (
            secs[2][1] as usize,
            secs[2][2] as usize,
            secs[2][3] as usize,
            secs[2][4] as usize,
        );
        let mut lf = std::collections::HashMap::new();
        let mut k = data_pk + d_lf;
        let lf_end = data_pk + d_gf;
        while k + 8 <= lf_end {
            let src = read_u32_le(blob, k);
            if src == 0xFFFF_FFFF { break; }
            lf.insert(src as usize, read_u32_le(blob, k + 4) as usize);
            k += 8;
        }

        // __data__ virtual fixups bind objects → classnames.
        let mut vfixups = Vec::new();
        let mut k = data_pk + d_vf;
        let vf_end = data_pk + d_end;
        while k + 12 <= vf_end {
            let src = read_u32_le(blob, k) as usize;
            let cnoff = read_u32_le(blob, k + 8) as usize;
            if src as u32 == 0xFFFF_FFFF {
                break;
            }
            k += 12;
            let class = names_by_offset
                .get(&cnoff)
                .cloned()
                .unwrap_or_else(|| "?".into());
            vfixups.push((src, class));
        }

        let version = find_sub(blob, b"Havok-")
            .map(|o| {
                let mut q = o;
                while q < blob.len() && blob[q] != 0 && blob[q].is_ascii_graphic() {
                    q += 1;
                }
                String::from_utf8_lossy(&blob[o..q]).into_owned()
            })
            .unwrap_or_default();

        Ok(HkPackfile { blob, data_pk, names_by_offset, lf, vfixups, version })
    }

    pub fn version(&self) -> &str { &self.version }

    /// `data`-relative source offsets of every object of `class_name`, in file
    /// order. Empty if the packfile carries none.
    pub fn objects_of_class(&self, class_name: &str) -> Vec<usize> {
        self.vfixups
            .iter()
            .filter(|(_, c)| c == class_name)
            .map(|(s, _)| *s)
            .collect()
    }

    /// Read `numTransformTracks` (`hkaSplineCompressedAnimation + 0x10`) for a
    /// given object source offset. Useful as an oracle against the ANIM block.
    pub fn num_transform_tracks(&self, obj_src: usize) -> u32 {
        read_u32_le(self.blob, self.data_pk + obj_src + 0x10)
    }

    /// The `data` hkArray of a spline anim as `&[u8]` — the packed spline
    /// stream itself. Returns `&[]` if the local fixup for the pointer field
    /// is missing (should not happen in retail data).
    pub fn spline_data(&self, obj_src: usize) -> &'a [u8] {
        // data hkArray = {ptr@+0x70, size@+0x74, capFlags@+0x78}
        let size = read_u32_le(self.blob, self.data_pk + obj_src + 0x74) as usize;
        match self.lf.get(&(obj_src + 0x70)) {
            Some(&d) => {
                let start = self.data_pk + d;
                let end = start.saturating_add(size).min(self.blob.len());
                &self.blob[start..end]
            }
            None => &[],
        }
    }

    /// Structural fingerprint of a spline-anim object — stable across the
    /// AssetCc2 XML round-trip and (on the 2214-clip retail corpus)
    /// disambiguating: no two clips share the same tuple. Used by
    /// [`map_hk_to_xml_indices`] to reconcile the packfile's virtual-fixup
    /// order with the XML's declaration order.
    ///
    /// The integer half comes straight off the object's u32 fields; the
    /// duration is stored as a string with six decimal places, matching how
    /// AssetCc2 serializes it — so the same clip's fingerprint reads
    /// identically from a packfile and from its XML decompilation.
    pub fn spline_fingerprint(&self, obj_src: usize) -> SplineFp {
        let o = self.data_pk + obj_src;
        let duration_f32 = f32::from_le_bytes([
            self.blob[o + 0x0c],
            self.blob[o + 0x0d],
            self.blob[o + 0x0e],
            self.blob[o + 0x0f],
        ]);
        let block_duration_f32 = f32::from_le_bytes([
            self.blob[o + 0x34],
            self.blob[o + 0x35],
            self.blob[o + 0x36],
            self.blob[o + 0x37],
        ]);
        SplineFp {
            num_transform_tracks: read_u32_le(self.blob, o + 0x10),
            num_float_tracks: read_u32_le(self.blob, o + 0x14),
            num_frames: read_u32_le(self.blob, o + 0x24),
            num_blocks: read_u32_le(self.blob, o + 0x28),
            max_frames_per_block: read_u32_le(self.blob, o + 0x2c),
            mask_and_quantization_size: read_u32_le(self.blob, o + 0x30),
            duration: format!("{duration_f32:.6}"),
            block_duration: format!("{block_duration_f32:.6}"),
            // hkArray sizes (all `<hkparam numelements="N">` in XML). Together
            // these fully disambiguate the retail corpus (see
            // `map_uniquely_identifies_every_clip_in_retail_corpus` — the
            // whole-pack empirical check gated below the unit tests).
            data_len: read_u32_le(self.blob, o + 0x74),
            block_offsets_size: read_u32_le(self.blob, o + 0x44),
            float_block_offsets_size: read_u32_le(self.blob, o + 0x50),
            float_offsets_size: read_u32_le(self.blob, o + 0x68),
            transform_offsets_size: read_u32_le(self.blob, o + 0x5c),
        }
    }
}

/// Structural fingerprint of a `hkaSplineCompressedAnimation` object.
/// See [`HkPackfile::spline_fingerprint`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SplineFp {
    pub num_transform_tracks: u32,
    pub num_float_tracks: u32,
    pub num_frames: u32,
    pub num_blocks: u32,
    pub max_frames_per_block: u32,
    pub mask_and_quantization_size: u32,
    /// AssetCc2-style six-decimal render of the f32 duration; used verbatim
    /// so packfile→XML fingerprint matching does not go through float
    /// arithmetic (round-trip parsing "0.933333" → f32 → format!("{:.6}") is
    /// identity for the retail corpus, and is what AssetCc2 emits either
    /// way).
    pub duration: String,
    /// AssetCc2-style six-decimal render of `blockDuration` (f32 @+0x34).
    pub block_duration: String,
    /// `data` hkArray size (bytes in the packed spline stream).
    pub data_len: u32,
    /// `blockOffsets` hkArray size (one entry per block).
    pub block_offsets_size: u32,
    /// `floatBlockOffsets` hkArray size (usually 1 with a single zero even
    /// for zero-float-track clips — but a defensive discriminator).
    pub float_block_offsets_size: u32,
    /// `floatOffsets` hkArray size.
    pub float_offsets_size: u32,
    /// `transformOffsets` hkArray size (empty across the retail corpus, but
    /// kept for stability if a mod adds one).
    pub transform_offsets_size: u32,
}

fn find_sub(hay: &[u8], n: &[u8]) -> Option<usize> {
    hay.windows(n.len()).position(|w| w == n)
}

// ---------------------------------------------------------------- xml surgeon

/// Find the `hkobject name="#NNNN" class="<class_name>" ...>` block for the
/// `idx`-th occurrence (0-based) in an HCT-emitted XML packfile. Returns
/// `(start, end)` byte offsets that bracket the whole `<hkobject>...</hkobject>`
/// element inclusive.
///
/// AssetCc2's XML is stable enough to make byte-range surgery safe: every
/// spline anim starts on its own line with `<hkobject name="#NNNN"
/// class="hkaSplineCompressedAnimation"` and ends with a `</hkobject>` at the
/// same indentation. The perturb tools ([`set_hkparam_in_object`] and
/// friends) work inside this range and never touch anything outside it, so a
/// single-clip edit cannot ripple to another clip's data.
pub fn find_nth_object(
    xml: &str,
    class_name: &str,
    idx: usize,
) -> Option<(usize, usize)> {
    let bytes = xml.as_bytes();
    let needle_class = format!("class=\"{class_name}\"");
    let mut found = 0usize;
    let mut scan = 0usize;
    while let Some(rel) = xml[scan..].find(&needle_class) {
        let class_pos = scan + rel;
        // Walk backward to the opening `<hkobject`.
        let mut start = class_pos;
        while start > 0 && !bytes[start..].starts_with(b"<hkobject") {
            start -= 1;
        }
        // Walk forward to the matching `</hkobject>` at the same indentation.
        // Since HCT never nests `<hkobject class=...>` inside another
        // `<hkobject class=...>`, the first `</hkobject>` after our start IS
        // the match.
        let end_needle = b"</hkobject>";
        let after_open = class_pos + needle_class.len();
        let end_rel = xml[after_open..].find("</hkobject>")?;
        let end = after_open + end_rel + end_needle.len();
        if found == idx {
            return Some((start, end));
        }
        found += 1;
        scan = end;
    }
    None
}

/// Extract the [`SplineFp`] fingerprint of the `xml_idx`-th
/// `hkaSplineCompressedAnimation` object in AssetCc2 XML file order.
///
/// Returns `None` if any of the six integer hkparams is missing/malformed.
pub fn xml_spline_fingerprint(xml: &str, xml_idx: usize) -> Option<SplineFp> {
    let (s, e) = find_nth_object(xml, "hkaSplineCompressedAnimation", xml_idx)?;
    let scope = &xml[s..e];
    fn param_str<'a>(scope: &'a str, name: &str) -> Option<&'a str> {
        let open = format!("<hkparam name=\"{name}\">");
        let start = scope.find(&open)? + open.len();
        let end = scope[start..].find("</hkparam>")?;
        Some(scope[start..start + end].trim())
    }
    fn u32_param(scope: &str, name: &str) -> Option<u32> {
        param_str(scope, name)?.parse::<u32>().ok()
    }
    // Re-format the parsed duration so the string matches the same f32→
    // format!("{:.6}") the packfile-side fingerprint uses. AssetCc2 already
    // emits six decimals, but a defensive re-format keeps the two sides
    // aligned even if a future toolchain change spells the same value
    // differently.
    let duration_f32: f32 = param_str(scope, "duration")?.parse().ok()?;
    let block_duration_f32: f32 = param_str(scope, "blockDuration")?.parse().ok()?;
    // hkArray fields render as `<hkparam name="X" numelements="N">…</hkparam>`;
    // pull `N` from the opening tag.
    fn numelements_of(scope: &str, name: &str) -> Option<u32> {
        let anchor = format!("<hkparam name=\"{name}\"");
        let a = scope.find(&anchor)?;
        let tail = &scope[a..];
        let key = "numelements=\"";
        let k = tail.find(key)? + key.len();
        let end = tail[k..].find('"')?;
        tail[k..k + end].parse::<u32>().ok()
    }
    Some(SplineFp {
        num_transform_tracks: u32_param(scope, "numberOfTransformTracks")?,
        num_float_tracks: u32_param(scope, "numberOfFloatTracks")?,
        num_frames: u32_param(scope, "numFrames")?,
        num_blocks: u32_param(scope, "numBlocks")?,
        max_frames_per_block: u32_param(scope, "maxFramesPerBlock")?,
        mask_and_quantization_size: u32_param(scope, "maskAndQuantizationSize")?,
        duration: format!("{duration_f32:.6}"),
        block_duration: format!("{block_duration_f32:.6}"),
        data_len: numelements_of(scope, "data")?,
        block_offsets_size: numelements_of(scope, "blockOffsets")?,
        float_block_offsets_size: numelements_of(scope, "floatBlockOffsets")?,
        float_offsets_size: numelements_of(scope, "floatOffsets")?,
        transform_offsets_size: numelements_of(scope, "transformOffsets")?,
    })
}

/// FNV-1a 64-bit — used only as a discriminator inside a same-fingerprint
/// group, so cryptographic strength is irrelevant.
pub fn hash_bytes(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x1000_0000_01b3);
    }
    h
}

/// Hash the space-separated decimal bytes inside the `data` hkparam of
/// `xml[start..end]` (an `<hkobject>` slice). Order matches
/// [`HkPackfile::spline_data`]'s byte stream, so two clips whose fingerprints
/// tie unambiguously distinguish here.
pub fn xml_spline_data_hash(xml: &str, start: usize, end: usize) -> Option<u64> {
    let scope = &xml[start..end];
    let open = "<hkparam name=\"data\"";
    let a = scope.find(open)?;
    // step past the opening tag `<hkparam name="data" numelements="N">`
    let after_tag = scope[a..].find('>')? + a + 1;
    let close_rel = scope[after_tag..].find("</hkparam>")?;
    let payload = &scope[after_tag..after_tag + close_rel];
    // parse decimal tokens; each is one byte.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for tok in payload.split_ascii_whitespace() {
        let b: u8 = tok.parse().ok()?;
        h ^= b as u64;
        h = h.wrapping_mul(0x1000_0000_01b3);
    }
    Some(h)
}

/// Given the fixup-order fingerprints (from [`HkPackfile::spline_fingerprint`]
/// walked over [`HkPackfile::objects_of_class`]) and, for each fixup entry,
/// the FNV-1a hash of its `data` hkArray bytes (from
/// [`HkPackfile::spline_data`]), return a vector `map` where
/// `map[fixup_idx] = xml_idx` — the XML file-order index of the same clip.
///
/// Matching is two-phase:
///  1. Structural fingerprint (unique in most retail cases).
///  2. If a fingerprint ties across the packfile OR the XML, resolve inside
///     the group by matching data-payload hashes.
///
/// Fails only if a clip is missing from the XML altogether, or if a group of
/// same-fingerprint clips STILL cannot be paired up 1:1 by data hash (which
/// would be an authored duplicate, not seen in the retail corpus).
pub fn map_hk_to_xml_indices(
    xml: &str,
    hk_fps: &[SplineFp],
    hk_data_hashes: &[u64],
) -> Result<Vec<usize>, String> {
    assert_eq!(hk_fps.len(), hk_data_hashes.len());
    // Enumerate ALL XML spline objects once, capturing (start, end, fingerprint, data-hash).
    let mut xml_objs: Vec<(usize, usize, SplineFp, u64)> = Vec::new();
    let mut xi = 0usize;
    loop {
        let (s, e) = match find_nth_object(xml, "hkaSplineCompressedAnimation", xi) {
            Some(v) => v,
            None => break,
        };
        let fp = xml_spline_fingerprint(xml, xi)
            .ok_or_else(|| format!("XML object #{xi} missing fingerprint fields"))?;
        let dh = xml_spline_data_hash(xml, s, e)
            .ok_or_else(|| format!("XML object #{xi} missing data hkparam"))?;
        xml_objs.push((s, e, fp, dh));
        xi += 1;
    }
    if xml_objs.len() != hk_fps.len() {
        return Err(format!(
            "XML has {} spline anims, packfile has {} — cannot reconcile",
            xml_objs.len(),
            hk_fps.len()
        ));
    }
    let mut out = vec![usize::MAX; hk_fps.len()];
    let mut used = vec![false; xml_objs.len()];
    for (fi, fp) in hk_fps.iter().enumerate() {
        let dh = hk_data_hashes[fi];
        let fp_hits: Vec<usize> = xml_objs
            .iter()
            .enumerate()
            .filter(|(i, (_, _, xfp, _))| !used[*i] && xfp == fp)
            .map(|(i, _)| i)
            .collect();
        if fp_hits.is_empty() {
            return Err(format!("fixup clip #{fi} fingerprint {fp:?} not present in XML"));
        }
        // Inside the fingerprint group, prefer the match whose data hash agrees.
        let chosen = fp_hits.iter().find(|&&xi| xml_objs[xi].3 == dh).copied();
        let picked = match chosen {
            Some(x) => x,
            None if fp_hits.len() == 1 => fp_hits[0],
            None => {
                return Err(format!(
                    "fixup clip #{fi} data-hash 0x{dh:016x} does not match any of the {} XML \
                     candidates with the same fingerprint (fingerprint: {fp:?})",
                    fp_hits.len()
                ))
            }
        };
        out[fi] = picked;
        used[picked] = true;
    }
    Ok(out)
}

/// In-place replacement of a `<hkparam name="NAME">VALUE</hkparam>` inside
/// the object at `xml[start..end]`. Returns a new XML string with just that
/// value substituted. Fails if the param is absent (unchanged XML would
/// silently pass) or occurs more than once (ambiguous target).
pub fn set_hkparam_in_object(
    xml: &str,
    start: usize,
    end: usize,
    param: &str,
    new_value: &str,
) -> Result<String, String> {
    let scope = &xml[start..end];
    let open = format!("<hkparam name=\"{param}\">");
    let close = "</hkparam>";
    let mut hits = 0usize;
    let mut first_open = 0usize;
    let mut first_close = 0usize;
    let mut cursor = 0usize;
    while let Some(rel) = scope[cursor..].find(&open) {
        let o = cursor + rel;
        let after = o + open.len();
        let c_rel = scope[after..].find(close).ok_or_else(|| {
            format!("hkparam name={param:?} opens at {o} without a closing tag")
        })?;
        if hits == 0 {
            first_open = o + open.len();
            first_close = after + c_rel;
        }
        hits += 1;
        cursor = after + c_rel + close.len();
    }
    if hits == 0 {
        return Err(format!("hkparam name={param:?} not found in object"));
    }
    if hits > 1 {
        return Err(format!("hkparam name={param:?} occurs {hits} times in object (ambiguous)"));
    }
    let abs_open = start + first_open;
    let abs_close = start + first_close;
    let mut out = String::with_capacity(xml.len() + new_value.len());
    out.push_str(&xml[..abs_open]);
    out.push_str(new_value);
    out.push_str(&xml[abs_close..]);
    Ok(out)
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal synthetic AP0L file whose Havok "blob" is a mock 32-byte
    /// buffer starting with the real magic. We're testing the CONTAINER
    /// splice, not real Havok parsing.
    fn synth_pack(hk_blob: &[u8]) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(b"L0PA");           // magic
        b.extend_from_slice(b"MINA");           // ANIM block start
        b.extend_from_slice(&0u32.to_le_bytes()); // recordCount = 0 (empty ANIM)
        b.extend_from_slice(&0u32.to_le_bytes()); // numAnims = 0
        b.extend_from_slice(&(hk_blob.len() as u32).to_le_bytes()); // hkSize
        b.extend_from_slice(hk_blob);
        // fake tail (represents INTV..SSP0 in real packs)
        b.extend_from_slice(b"TAIL-BYTES-END-OF-PACK");
        b
    }

    fn fake_hk_blob(size: usize) -> Vec<u8> {
        let mut v = Vec::with_capacity(size);
        v.extend_from_slice(&HAVOK_MAGIC.to_le_bytes()); // 4 bytes
        while v.len() < size {
            v.push((v.len() % 251) as u8);
        }
        v.truncate(size);
        v
    }

    #[test]
    fn parse_and_boundaries() {
        let blob = fake_hk_blob(64);
        let pack = synth_pack(&blob);
        let ap = Ap0lPack::parse(pack.clone()).unwrap();
        assert_eq!(ap.hk_size(), blob.len() as u32);
        assert_eq!(ap.hk_blob(), blob.as_slice());
        assert_eq!(ap.head().len() + ap.hk_blob().len() + ap.tail().len(), pack.len());
        // splice with identical blob reproduces original bytes exactly
        let round = ap.splice_hk_blob(&blob);
        assert_eq!(round, pack);
    }

    #[test]
    fn splice_same_size_preserves_head_and_tail_bytes() {
        let blob = fake_hk_blob(128);
        let pack = synth_pack(&blob);
        let ap = Ap0lPack::parse(pack.clone()).unwrap();
        // A same-size blob with different contents: head+tail must be byte-identical.
        let mut mutated = blob.clone();
        for (i, b) in mutated.iter_mut().enumerate().skip(4) {
            *b = *b ^ ((i & 0xff) as u8);
        }
        let out = ap.splice_hk_blob(&mutated);
        assert_eq!(&out[..ap.hk_offset()], &pack[..ap.hk_offset()],
                   "head bytes (AP0L + ANIM + [numAnims,hkSize]) must be preserved");
        assert_eq!(
            &out[ap.hk_offset() + ap.hk_size() as usize..],
            &pack[ap.hk_offset() + ap.hk_size() as usize..],
            "tail bytes (INTV..SSP0) must be preserved"
        );
        assert_eq!(read_u32_le(&out, ap.hk_offset() - 4), mutated.len() as u32);
    }

    #[test]
    fn splice_larger_blob_updates_hksize_and_shifts_tail_untouched() {
        let orig = fake_hk_blob(48);
        let pack = synth_pack(&orig);
        let ap = Ap0lPack::parse(pack.clone()).unwrap();
        let bigger = fake_hk_blob(64);
        let out = ap.splice_hk_blob(&bigger);
        assert_eq!(out.len(), pack.len() + (bigger.len() - orig.len()));
        // hkSize field rewritten
        assert_eq!(read_u32_le(&out, ap.hk_offset() - 4), bigger.len() as u32);
        // tail content preserved verbatim (just shifted)
        let new_tail_start = ap.hk_offset() + bigger.len();
        assert_eq!(&out[new_tail_start..], ap.tail());
    }

    #[test]
    fn splice_smaller_blob_updates_hksize_and_shifts_tail_untouched() {
        let orig = fake_hk_blob(64);
        let pack = synth_pack(&orig);
        let ap = Ap0lPack::parse(pack.clone()).unwrap();
        let smaller = fake_hk_blob(48);
        let out = ap.splice_hk_blob(&smaller);
        assert_eq!(out.len(), pack.len() - (orig.len() - smaller.len()));
        assert_eq!(read_u32_le(&out, ap.hk_offset() - 4), smaller.len() as u32);
        let new_tail_start = ap.hk_offset() + smaller.len();
        assert_eq!(&out[new_tail_start..], ap.tail());
    }

    #[test]
    fn parse_rejects_non_ap0l() {
        let mut bad = vec![0u8; 32];
        bad[0..4].copy_from_slice(b"XXXX");
        assert!(matches!(Ap0lPack::parse(bad), Err(Ap0lError::NotAp0l)));
    }

    #[test]
    fn parse_rejects_missing_havok_magic() {
        let mut b = vec![0u8; 64];
        b[0..4].copy_from_slice(b"L0PA");
        b[4..8].copy_from_slice(b"MINA");
        assert!(matches!(Ap0lPack::parse(b), Err(Ap0lError::MissingHavokMagic)));
    }

    #[test]
    fn xml_find_nth_object_isolates_a_single_clip() {
        // Two clips with an identical param inside; find_nth_object + set_hkparam_in_object
        // must edit exactly the requested one.
        let xml = "\
<hkobject name=\"#0004\" class=\"hkaSplineCompressedAnimation\">
\t<hkparam name=\"duration\">1.000000</hkparam>
</hkobject>
<hkobject name=\"#0005\" class=\"hkaSplineCompressedAnimation\">
\t<hkparam name=\"duration\">2.000000</hkparam>
</hkobject>";
        let (s0, e0) = find_nth_object(xml, "hkaSplineCompressedAnimation", 0).unwrap();
        assert!(xml[s0..e0].contains("#0004"));
        let (s1, e1) = find_nth_object(xml, "hkaSplineCompressedAnimation", 1).unwrap();
        assert!(xml[s1..e1].contains("#0005"));

        let edited = set_hkparam_in_object(xml, s0, e0, "duration", "9.999999").unwrap();
        assert!(edited.contains("\t<hkparam name=\"duration\">9.999999</hkparam>\n</hkobject>\n<hkobject name=\"#0005\""));
        assert!(edited.contains("<hkparam name=\"duration\">2.000000</hkparam>"),
                "second clip must be untouched");
    }

    #[test]
    fn xml_fingerprint_and_reorder_reconciles_order() {
        // Two clips whose XML declaration order (#0004 first) is REVERSED
        // relative to their fixup order — matches what AssetCc2 does on
        // retail Animations.pack, where XML #0004 = fixup clip #~1900,
        // not #0.
        let xml = "\
<hkobject name=\"#0004\" class=\"hkaSplineCompressedAnimation\">
  <hkparam name=\"duration\">0.933333</hkparam>
  <hkparam name=\"numberOfTransformTracks\">62</hkparam>
  <hkparam name=\"numberOfFloatTracks\">0</hkparam>
  <hkparam name=\"numFrames\">15</hkparam>
  <hkparam name=\"numBlocks\">1</hkparam>
  <hkparam name=\"maxFramesPerBlock\">256</hkparam>
  <hkparam name=\"maskAndQuantizationSize\">248</hkparam>
  <hkparam name=\"blockDuration\">17.000000</hkparam>
  <hkparam name=\"blockOffsets\" numelements=\"1\">0</hkparam>
  <hkparam name=\"floatBlockOffsets\" numelements=\"1\">0</hkparam>
  <hkparam name=\"transformOffsets\" numelements=\"0\"></hkparam>
  <hkparam name=\"floatOffsets\" numelements=\"0\"></hkparam>
  <hkparam name=\"data\" numelements=\"3152\">1 2 3 4</hkparam>
</hkobject>
<hkobject name=\"#0005\" class=\"hkaSplineCompressedAnimation\">
  <hkparam name=\"duration\">0.833333</hkparam>
  <hkparam name=\"numberOfTransformTracks\">64</hkparam>
  <hkparam name=\"numberOfFloatTracks\">0</hkparam>
  <hkparam name=\"numFrames\">13</hkparam>
  <hkparam name=\"numBlocks\">1</hkparam>
  <hkparam name=\"maxFramesPerBlock\">256</hkparam>
  <hkparam name=\"maskAndQuantizationSize\">256</hkparam>
  <hkparam name=\"blockDuration\">17.708334</hkparam>
  <hkparam name=\"blockOffsets\" numelements=\"1\">0</hkparam>
  <hkparam name=\"floatBlockOffsets\" numelements=\"1\">0</hkparam>
  <hkparam name=\"transformOffsets\" numelements=\"0\"></hkparam>
  <hkparam name=\"floatOffsets\" numelements=\"0\"></hkparam>
  <hkparam name=\"data\" numelements=\"4832\">1 2 3 4</hkparam>
</hkobject>";
        let fp_xml0 = xml_spline_fingerprint(xml, 0).unwrap();
        assert_eq!(fp_xml0.num_transform_tracks, 62);
        let fp_xml1 = xml_spline_fingerprint(xml, 1).unwrap();
        assert_eq!(fp_xml1.num_transform_tracks, 64);
        // Fixup order says "clip #0 has 64 tracks, clip #1 has 62 tracks".
        let hk_fps = vec![fp_xml1.clone(), fp_xml0.clone()];
        // In this fixture each fingerprint is unique so the data-hash tiebreaker
        // doesn't fire; any value survives.
        let hk_data_hashes = vec![0u64, 0u64];
        let map = map_hk_to_xml_indices(xml, &hk_fps, &hk_data_hashes).unwrap();
        assert_eq!(map, vec![1, 0]);
    }

    #[test]
    fn map_uses_data_hash_to_disambiguate_identical_fingerprints() {
        // Two clips whose STRUCTURAL fingerprint is identical (both are 1-track,
        // 1-frame, 1-block, same maskAndQuantizationSize, same block/data sizes,
        // same duration) but whose DATA payloads differ. This mirrors the
        // retail-corpus case where 2 clips shared the whole coarse fingerprint.
        let xml = "\
<hkobject name=\"#0004\" class=\"hkaSplineCompressedAnimation\">
  <hkparam name=\"duration\">1.000000</hkparam>
  <hkparam name=\"numberOfTransformTracks\">1</hkparam>
  <hkparam name=\"numberOfFloatTracks\">0</hkparam>
  <hkparam name=\"numFrames\">1</hkparam>
  <hkparam name=\"numBlocks\">1</hkparam>
  <hkparam name=\"maxFramesPerBlock\">256</hkparam>
  <hkparam name=\"maskAndQuantizationSize\">4</hkparam>
  <hkparam name=\"blockDuration\">1.000000</hkparam>
  <hkparam name=\"blockOffsets\" numelements=\"1\">0</hkparam>
  <hkparam name=\"floatBlockOffsets\" numelements=\"1\">0</hkparam>
  <hkparam name=\"transformOffsets\" numelements=\"0\"></hkparam>
  <hkparam name=\"floatOffsets\" numelements=\"0\"></hkparam>
  <hkparam name=\"data\" numelements=\"4\">1 2 3 4</hkparam>
</hkobject>
<hkobject name=\"#0005\" class=\"hkaSplineCompressedAnimation\">
  <hkparam name=\"duration\">1.000000</hkparam>
  <hkparam name=\"numberOfTransformTracks\">1</hkparam>
  <hkparam name=\"numberOfFloatTracks\">0</hkparam>
  <hkparam name=\"numFrames\">1</hkparam>
  <hkparam name=\"numBlocks\">1</hkparam>
  <hkparam name=\"maxFramesPerBlock\">256</hkparam>
  <hkparam name=\"maskAndQuantizationSize\">4</hkparam>
  <hkparam name=\"blockDuration\">1.000000</hkparam>
  <hkparam name=\"blockOffsets\" numelements=\"1\">0</hkparam>
  <hkparam name=\"floatBlockOffsets\" numelements=\"1\">0</hkparam>
  <hkparam name=\"transformOffsets\" numelements=\"0\"></hkparam>
  <hkparam name=\"floatOffsets\" numelements=\"0\"></hkparam>
  <hkparam name=\"data\" numelements=\"4\">99 88 77 66</hkparam>
</hkobject>";
        let fp0 = xml_spline_fingerprint(xml, 0).unwrap();
        let fp1 = xml_spline_fingerprint(xml, 1).unwrap();
        assert_eq!(fp0, fp1, "the two clips must have identical structural fingerprints for this test");
        let h_a = hash_bytes(&[1u8, 2, 3, 4]);
        let h_b = hash_bytes(&[99u8, 88, 77, 66]);
        // Fixup order: XML #0's data, then XML #1's data — mapping should keep pairing.
        let map = map_hk_to_xml_indices(xml, &[fp0.clone(), fp0.clone()], &[h_a, h_b]).unwrap();
        assert_eq!(map, vec![0, 1]);
        // Fixup order: reversed — mapping should follow the data.
        let map_rev = map_hk_to_xml_indices(xml, &[fp0.clone(), fp0.clone()], &[h_b, h_a]).unwrap();
        assert_eq!(map_rev, vec![1, 0]);
    }

    /// Whole-pack empirical check: for every one of the 2214 clips in the
    /// retail `Animations.pack`, the fingerprint + data-hash discriminator is
    /// unique enough that [`map_hk_to_xml_indices`] produces a 1:1 mapping
    /// against the AssetCc2 XML.
    ///
    /// Opt-in — set `SAB_ANIM_PACK` (path to `Animations.pack`) and
    /// `SAB_ANIM_XML` (path to `AssetCc2 --xml` output of the extracted Havok
    /// blob) to enable. Kept as a smoke test rather than a fixture because
    /// the XML is 30 MB and would inflate the repo. On a machine without the
    /// env vars the test is skipped silently, so CI without the retail
    /// install still stays green.
    #[test]
    fn map_is_bijective_on_retail_corpus_when_env_present() {
        use std::env;
        use std::fs;
        let (Ok(pack), Ok(xml)) = (env::var("SAB_ANIM_PACK"), env::var("SAB_ANIM_XML")) else {
            eprintln!("skipping: SAB_ANIM_PACK and SAB_ANIM_XML must both be set");
            return;
        };
        let bytes = fs::read(pack).expect("read pack");
        let ap = Ap0lPack::parse(bytes).expect("parse ap0l");
        let hk = HkPackfile::parse(ap.hk_blob()).expect("parse hk");
        let scas = hk.objects_of_class("hkaSplineCompressedAnimation");
        let fps: Vec<_> = scas.iter().map(|&s| hk.spline_fingerprint(s)).collect();
        let hashes: Vec<_> = scas.iter().map(|&s| hash_bytes(hk.spline_data(s))).collect();
        let xml_text = fs::read_to_string(xml).expect("read xml");
        let map = map_hk_to_xml_indices(&xml_text, &fps, &hashes).expect("map");
        assert_eq!(map.len(), scas.len());
        let mut seen = std::collections::HashSet::new();
        for &x in &map {
            assert!(seen.insert(x), "map is not 1:1: duplicate xml_idx {x}");
        }
    }

    #[test]
    fn xml_set_hkparam_rejects_absent_or_ambiguous() {
        let xml = "<hkobject class=\"X\">\
                   <hkparam name=\"a\">1</hkparam>\
                   <hkparam name=\"a\">2</hkparam>\
                   </hkobject>";
        let (s, e) = (0, xml.len());
        assert!(set_hkparam_in_object(xml, s, e, "missing", "0").is_err());
        assert!(set_hkparam_in_object(xml, s, e, "a", "0").is_err());
    }
}
