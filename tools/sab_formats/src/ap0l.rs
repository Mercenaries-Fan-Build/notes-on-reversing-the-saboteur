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

    /// Absolute offset within the packfile blob of the spline `data` hkArray's
    /// first byte, plus its length. Same slice as [`Self::spline_data`], but
    /// returned as `(offset, len)` so callers that want to WRITE into an owned
    /// copy of the blob (`Vec<u8>`) can do so without going through this
    /// borrow-by-reference wrapper. Returns `None` if the local-fixup for the
    /// `data` pointer field is missing (should not happen in retail data).
    pub fn spline_data_span(&self, obj_src: usize) -> Option<(usize, usize)> {
        let size = read_u32_le(self.blob, self.data_pk + obj_src + 0x74) as usize;
        self.lf.get(&(obj_src + 0x70)).map(|&d| (self.data_pk + d, size))
    }

    /// Read `numBlocks` (`hkaSplineCompressedAnimation + 0x28`). The static-value
    /// mutation helpers require this to be `1` (the entire retail corpus is
    /// single-block; a multi-block walker would need a per-block `blockOffsets`
    /// dispatch that is intentionally not implemented here).
    pub fn num_blocks(&self, obj_src: usize) -> u32 {
        read_u32_le(self.blob, self.data_pk + obj_src + 0x28)
    }

    /// Read `maskAndQuantizationSize` (+0x30). The per-track 4-byte header
    /// table sits at the start of each block; the packed T/R/S stream begins
    /// this many bytes into the block.
    pub fn mask_and_quantization_size(&self, obj_src: usize) -> u32 {
        read_u32_le(self.blob, self.data_pk + obj_src + 0x30)
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

// ---------------------------------------------------------------- spline data walker

/// Failure modes surfaced by the static-value mutation helpers
/// ([`set_static_translation`], [`set_static_rotation`], [`set_static_scale`],
/// [`offset_static_translation`]).
#[derive(Debug, PartialEq)]
pub enum StaticEditError {
    /// `track_idx >= num_transform_tracks` for the target clip.
    TrackOutOfRange { track: usize, count: usize },
    /// The clip has `numBlocks != 1`. Multi-block walks are not implemented —
    /// the entire retail corpus is single-block, so this fires only on unusual
    /// authored data.
    NotSingleBlock { num_blocks: u32 },
    /// The channel's mask says the value is not (fully) static — either the
    /// spline bits are set (dynamic per-frame) or a required component's
    /// static bit is clear. `--set-static-translation` and `--set-static-scale`
    /// require `mask == 0x07` (all three xyz static, no spline); `--set-static-rotation`
    /// requires `mask & 0xf0 == 0` AND `mask & 0x0f != 0` (packed static quaternion).
    NotStatic { channel: &'static str, mask: u8 },
    /// The rotation quantization type is not `1` (THREECOMP40). Only type 1 is
    /// implemented — the entire retail corpus uses it exclusively. See
    /// `tools/sab_havok65/src/main.rs :: unpack_quat` for the other types.
    UnsupportedRotQuant { qtype: u8 },
    /// The walker ran past the end of the compressed data stream. Would only
    /// fire on truncated / malformed input.
    Truncated { at: usize, need: usize, have: usize },
    /// The supplied quaternion had a zero (or near-zero) norm; can't normalize.
    ZeroQuaternion,
}

impl std::fmt::Display for StaticEditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StaticEditError::TrackOutOfRange { track, count } => {
                write!(f, "track {track} out of range (clip has {count} transform tracks)")
            }
            StaticEditError::NotSingleBlock { num_blocks } => {
                write!(f, "clip has numBlocks={num_blocks}, static-value editing requires 1")
            }
            StaticEditError::NotStatic { channel, mask } => write!(
                f,
                "{channel} channel mask=0x{mask:02x} is not the required fully-static shape \
                 (translation/scale need mask==0x07; rotation needs 0x0f mask bits set and 0xf0 clear)"
            ),
            StaticEditError::UnsupportedRotQuant { qtype } => write!(
                f,
                "rotation quantization type {qtype} is not supported (only THREECOMP40=1 is implemented)"
            ),
            StaticEditError::Truncated { at, need, have } => write!(
                f,
                "compressed data truncated at offset {at}: need {need} bytes, have {have}"
            ),
            StaticEditError::ZeroQuaternion => write!(
                f,
                "quaternion has zero or near-zero norm and cannot be normalized"
            ),
        }
    }
}

impl std::error::Error for StaticEditError {}

/// Which channel of a track's compressed block a mutation touches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StaticChannel {
    Translation,
    Rotation,
    Scale,
}

/// One track's per-channel byte offsets and mask bytes, relative to the start
/// of the compressed `data` array (offset 0 == first mask byte of block 0).
/// Present on every track for which the channel exists in the stream, with the
/// mask byte and the byte offset of the FIRST byte of that channel's payload
/// (the byte right after any knot vector + alignment; static values begin
/// here). Callers use [`is_fully_static_vec3`] etc. before writing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackLayout {
    pub ctrl: u8,
    pub trn_mask: u8,
    pub rot_mask: u8,
    pub scl_mask: u8,
    /// Byte offset within `data` where the translation static-values (or
    /// spline range floats) begin — that is, immediately after any knot
    /// vector and the 4-byte alignment.
    pub trn_static_off: usize,
    /// Byte offset within `data` where the rotation payload begins — a single
    /// packed quaternion for the static shape, or the (n, degree, knots)
    /// header for the spline shape.
    pub rot_payload_off: usize,
    /// Byte offset within `data` where the scale static/range floats begin.
    pub scl_static_off: usize,
}

impl TrackLayout {
    /// True if the translation is a plain 3-float static value (all of x/y/z
    /// static, no w, no spline) — the shape [`set_static_translation`] can
    /// edit directly.
    pub fn is_fully_static_vec3_trn(&self) -> bool {
        self.trn_mask == 0x07
    }

    /// True if the scale channel is a plain 3-float static value.
    pub fn is_fully_static_vec3_scl(&self) -> bool {
        self.scl_mask == 0x07
    }

    /// True if the rotation channel is a single packed static quaternion
    /// (any of bits 0..3 set, no spline bits 4..7). THREECOMP40 fits any of
    /// these.
    pub fn is_static_quat(&self) -> bool {
        self.rot_mask & 0xf0 == 0 && self.rot_mask & 0x0f != 0
    }
}

fn align_up_usize(x: usize, a: usize) -> usize { (x + a - 1) & !(a - 1) }

/// Byte size of a packed rotation control point given its quant type.
/// Type 1 = THREECOMP40 (5 bytes) — the only type used by retail. Others are
/// listed for future callers but the mutator errors out on them.
fn rot_cpsize(qtype: u8) -> usize {
    match qtype {
        0 => 4, // POLAR32
        1 => 5, // THREECOMP40
        2 => 6, // THREECOMP48
        3 => 3, // THREECOMP24
        _ => 5,
    }
}
fn rot_align(_qtype: u8) -> usize { 1 }

/// Replay the spline decoder's byte-pointer arithmetic to compute, for every
/// track of a single-block clip, where its translation / rotation / scale
/// payload begins within the `data` array. This is the walker
/// [`set_static_translation`] and friends use to locate the bytes to overwrite.
///
/// The walker faithfully reproduces the offsets computed by
/// `tools/sab_havok65/src/main.rs :: eval_vec4 / eval_rotation`. It does NOT
/// decode the values themselves — it only advances the pointer.
///
/// * `data`   — the whole packed spline stream for this clip
///   (block 0 lives at offset 0 for single-block clips).
/// * `num_transform_tracks` — from the clip's `numberOfTransformTracks` field.
/// * `mask_and_quantization_size` — from the clip's `maskAndQuantizationSize`.
///
/// Returns per-track layouts, or [`StaticEditError::Truncated`] if the input
/// is shorter than the walker would need.
pub fn walk_spline_track_layouts(
    data: &[u8],
    num_transform_tracks: usize,
    mask_and_quantization_size: usize,
) -> Result<Vec<TrackLayout>, StaticEditError> {
    let bs = 0usize; // single-block: block 0 starts at data[0]
    if data.len() < bs + mask_and_quantization_size {
        return Err(StaticEditError::Truncated {
            at: bs,
            need: mask_and_quantization_size,
            have: data.len(),
        });
    }
    let mut ptr = bs + mask_and_quantization_size;
    let mut out = Vec::with_capacity(num_transform_tracks);
    for t in 0..num_transform_tracks {
        let hdr = bs + t * 4;
        if hdr + 4 > data.len() {
            return Err(StaticEditError::Truncated {
                at: hdr,
                need: 4,
                have: data.len(),
            });
        }
        let ctrl = data[hdr];
        let trn_mask = data[hdr + 1];
        let rot_mask = data[hdr + 2];
        let scl_mask = data[hdr + 3];
        let trn_type = ctrl & 3;
        let rot_type = (ctrl >> 2) & 0xf;
        let scl_type = ctrl >> 6;

        // ---- translation channel ----
        let trn_static_off = walk_vec4_advance(data, &mut ptr, trn_mask, trn_type)?;
        ptr = align_up_usize(ptr, 4);

        // ---- rotation channel ----
        let rot_payload_off = ptr;
        walk_rotation(data, &mut ptr, rot_mask, rot_type)?;
        ptr = align_up_usize(ptr, 4);

        // ---- scale channel ----
        let scl_static_off = walk_vec4_advance(data, &mut ptr, scl_mask, scl_type)?;
        ptr = align_up_usize(ptr, 4);

        out.push(TrackLayout {
            ctrl,
            trn_mask,
            rot_mask,
            scl_mask,
            trn_static_off,
            rot_payload_off,
            scl_static_off,
        });
        let _ = t;
    }
    Ok(out)
}

/// Walk translation/scale where the mask may be static or spline. Advances
/// `ptr` past the whole channel (knot header + static/range floats + quantized
/// CPs, whichever apply) and returns the byte offset where the static/range
/// float slot begins — that's the write target for the static-value editors.
fn walk_vec4_full(
    data: &[u8],
    ptr: &mut usize,
    mask: u8,
    quant_type: u8,
) -> Result<usize, StaticEditError> {
    let has_spline = mask & 0xf0 != 0;
    let mut n_idx = 0usize;
    if has_spline {
        if *ptr + 3 > data.len() {
            return Err(StaticEditError::Truncated { at: *ptr, need: 3, have: data.len() });
        }
        n_idx = u16::from_le_bytes([data[*ptr], data[*ptr + 1]]) as usize;
        *ptr += 2;
        let degree = data[*ptr] as usize;
        *ptr += 1;
        let knot_ptr = *ptr;
        let end = knot_ptr + n_idx + 2 + degree;
        if end > data.len() {
            return Err(StaticEditError::Truncated { at: knot_ptr, need: n_idx + 2 + degree, have: data.len().saturating_sub(knot_ptr) });
        }
        *ptr = end;
    }
    *ptr = align_up_usize(*ptr, 4);
    let static_off = *ptr;
    for i in 0..3 {
        if mask & (1 << i) != 0 {
            *ptr += 4;
        } else if mask & (0x10 << i) != 0 {
            *ptr += 8;
        }
    }
    if mask & 8 != 0 {
        *ptr += 4;
    }
    if *ptr > data.len() {
        return Err(StaticEditError::Truncated { at: *ptr, need: 0, have: data.len() });
    }
    if has_spline {
        *ptr = (*ptr + 1) & !1;
        let bytes_per = if quant_type == 0 { 1usize } else { 2usize };
        let n_comp = ((mask >> 4) & 7).count_ones() as usize;
        let num_cp = n_idx + 1;
        let need = num_cp * n_comp * bytes_per;
        if *ptr + need > data.len() {
            return Err(StaticEditError::Truncated { at: *ptr, need, have: data.len().saturating_sub(*ptr) });
        }
        *ptr += need;
    }
    Ok(static_off)
}

fn walk_rotation(
    data: &[u8],
    ptr: &mut usize,
    mask: u8,
    qtype: u8,
) -> Result<(), StaticEditError> {
    let cpsize = rot_cpsize(qtype);
    if mask & 0xf0 != 0 {
        if *ptr + 3 > data.len() {
            return Err(StaticEditError::Truncated { at: *ptr, need: 3, have: data.len() });
        }
        let n = u16::from_le_bytes([data[*ptr], data[*ptr + 1]]) as usize;
        *ptr += 2;
        let degree = data[*ptr] as usize;
        *ptr += 1;
        let knot_ptr = *ptr;
        let end = knot_ptr + n + 2 + degree;
        if end > data.len() {
            return Err(StaticEditError::Truncated { at: knot_ptr, need: n + 2 + degree, have: data.len().saturating_sub(knot_ptr) });
        }
        *ptr = end;
        *ptr = align_up_usize(*ptr, rot_align(qtype));
        let need = (n + 1) * cpsize;
        if *ptr + need > data.len() {
            return Err(StaticEditError::Truncated { at: *ptr, need, have: data.len().saturating_sub(*ptr) });
        }
        *ptr += need;
    } else if mask & 0x0f != 0 {
        *ptr = align_up_usize(*ptr, rot_align(qtype));
        if *ptr + cpsize > data.len() {
            return Err(StaticEditError::Truncated { at: *ptr, need: cpsize, have: data.len().saturating_sub(*ptr) });
        }
        *ptr += cpsize;
    }
    Ok(())
}

// Thin alias so the main walker reads naturally.
fn walk_vec4_advance(
    data: &[u8],
    ptr: &mut usize,
    mask: u8,
    quant_type: u8,
) -> Result<usize, StaticEditError> {
    walk_vec4_full(data, ptr, mask, quant_type)
}

// ---------------------------------------------------------------- THREECOMP40 encode

/// Pack a unit quaternion `[qx, qy, qz, qw]` into 5 bytes using Havok's
/// THREECOMP40 format — the encoder inverse of `unpack_quat(_, _, 1)` in
/// `tools/sab_havok65/src/main.rs` and Saboteur.exe `FUN_00f22470`.
///
/// The input MUST have positive largest-abs component after canonicalisation:
/// this function first canonicalises by negating the whole quaternion if the
/// component of largest absolute value is negative (this produces the same
/// rotation), so the sign bit (bit 38) is always 0 in the output.
///
/// Fails only if `q` has zero or near-zero norm.
pub fn encode_threecomp40(q: [f32; 4]) -> Result<[u8; 5], StaticEditError> {
    // Normalize.
    let nrm2 = q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3];
    if nrm2 < 1e-24 {
        return Err(StaticEditError::ZeroQuaternion);
    }
    let s = 1.0 / nrm2.sqrt();
    let mut n = [q[0] * s, q[1] * s, q[2] * s, q[3] * s];
    // Canonicalise: negate so largest-abs is positive.
    let mut idx = 0usize;
    let mut abs_max = n[0].abs();
    for i in 1..4 {
        if n[i].abs() > abs_max {
            abs_max = n[i].abs();
            idx = i;
        }
    }
    if n[idx] < 0.0 {
        for c in n.iter_mut() { *c = -*c; }
    }
    // Pack the three not-omitted components.
    const SCALE: f32 = 2047.0 * std::f32::consts::SQRT_2;
    let mut small = [0f32; 3];
    let mut j = 0;
    for i in 0..4 {
        if i != idx {
            small[j] = n[i];
            j += 1;
        }
    }
    let mut raw = [0u32; 3];
    for i in 0..3 {
        let r = (small[i] * SCALE).round() as i32 + 2047;
        raw[i] = r.clamp(0, 4095) as u32;
    }
    // Assemble 40-bit little-endian word.
    let v: u64 = (raw[0] as u64)
        | ((raw[1] as u64) << 12)
        | ((raw[2] as u64) << 24)
        | ((idx as u64) << 36);
    // bit 38 = 0 (positive largest); bit 39 unused.
    let mut out = [0u8; 5];
    for i in 0..5 {
        out[i] = ((v >> (8 * i)) & 0xff) as u8;
    }
    Ok(out)
}

/// Decode a THREECOMP40 5-byte packed quaternion back into
/// `[qx, qy, qz, qw]`. Inverse of [`encode_threecomp40`] modulo re-quantization.
/// Used by [`offset_static_translation`]'s rotation cousin (currently unused
/// — offset variants are only wired for translation — but exposed so tests
/// can round-trip.)
pub fn decode_threecomp40(bytes: [u8; 5]) -> [f32; 4] {
    const S: f32 = 1.0 / (std::f32::consts::SQRT_2 * 2047.0);
    let v: u64 = (0..5).map(|i| (bytes[i] as u64) << (8 * i)).sum();
    let small = [
        (((v & 0xFFF) as i32) - 2047) as f32 * S,
        ((((v >> 12) & 0xFFF) as i32) - 2047) as f32 * S,
        ((((v >> 24) & 0xFFF) as i32) - 2047) as f32 * S,
    ];
    let idx = ((v >> 36) & 3) as usize;
    let neg = (v >> 38) & 1 != 0;
    let ss = small[0] * small[0] + small[1] * small[1] + small[2] * small[2];
    let mut largest = (1.0 - ss).max(0.0).sqrt();
    if neg { largest = -largest; }
    let mut q = [0f32; 4];
    let mut j = 0;
    for i in 0..4 {
        if i == idx { q[i] = largest; } else { q[i] = small[j]; j += 1; }
    }
    q
}

// ---------------------------------------------------------------- mutation helpers

/// Read the current static translation of `track` (a `[f32; 3]`), or error if
/// the track's translation is not the fully-static 3-float shape.
pub fn read_static_translation(
    data: &[u8],
    layouts: &[TrackLayout],
    track: usize,
) -> Result<[f32; 3], StaticEditError> {
    let lay = layouts.get(track).ok_or(StaticEditError::TrackOutOfRange {
        track,
        count: layouts.len(),
    })?;
    if !lay.is_fully_static_vec3_trn() {
        return Err(StaticEditError::NotStatic { channel: "translation", mask: lay.trn_mask });
    }
    let o = lay.trn_static_off;
    if o + 12 > data.len() {
        return Err(StaticEditError::Truncated { at: o, need: 12, have: data.len() });
    }
    Ok([
        f32::from_le_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]]),
        f32::from_le_bytes([data[o + 4], data[o + 5], data[o + 6], data[o + 7]]),
        f32::from_le_bytes([data[o + 8], data[o + 9], data[o + 10], data[o + 11]]),
    ])
}

/// Read the current static scale of `track` (a `[f32; 3]`).
pub fn read_static_scale(
    data: &[u8],
    layouts: &[TrackLayout],
    track: usize,
) -> Result<[f32; 3], StaticEditError> {
    let lay = layouts.get(track).ok_or(StaticEditError::TrackOutOfRange {
        track,
        count: layouts.len(),
    })?;
    if !lay.is_fully_static_vec3_scl() {
        return Err(StaticEditError::NotStatic { channel: "scale", mask: lay.scl_mask });
    }
    let o = lay.scl_static_off;
    if o + 12 > data.len() {
        return Err(StaticEditError::Truncated { at: o, need: 12, have: data.len() });
    }
    Ok([
        f32::from_le_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]]),
        f32::from_le_bytes([data[o + 4], data[o + 5], data[o + 6], data[o + 7]]),
        f32::from_le_bytes([data[o + 8], data[o + 9], data[o + 10], data[o + 11]]),
    ])
}

/// Read the current static rotation of `track` (unit quaternion `[x, y, z, w]`).
pub fn read_static_rotation(
    data: &[u8],
    layouts: &[TrackLayout],
    track: usize,
) -> Result<[f32; 4], StaticEditError> {
    let lay = layouts.get(track).ok_or(StaticEditError::TrackOutOfRange {
        track,
        count: layouts.len(),
    })?;
    if !lay.is_static_quat() {
        return Err(StaticEditError::NotStatic { channel: "rotation", mask: lay.rot_mask });
    }
    let qtype = (lay.ctrl >> 2) & 0xf;
    if qtype != 1 {
        return Err(StaticEditError::UnsupportedRotQuant { qtype });
    }
    let o = lay.rot_payload_off;
    if o + 5 > data.len() {
        return Err(StaticEditError::Truncated { at: o, need: 5, have: data.len() });
    }
    let mut b = [0u8; 5];
    b.copy_from_slice(&data[o..o + 5]);
    Ok(decode_threecomp40(b))
}

/// Overwrite `track`'s static translation with `xyz`. Returns the old value.
///
/// Fails if:
/// * `track >= layouts.len()` — [`StaticEditError::TrackOutOfRange`].
/// * The track's translation is not the fully-static xyz shape
///   (`mask == 0x07`) — [`StaticEditError::NotStatic`].
/// * The offset is past end of buffer — [`StaticEditError::Truncated`].
pub fn set_static_translation(
    data: &mut [u8],
    layouts: &[TrackLayout],
    track: usize,
    xyz: [f32; 3],
) -> Result<[f32; 3], StaticEditError> {
    let old = read_static_translation(data, layouts, track)?;
    let o = layouts[track].trn_static_off;
    data[o..o + 4].copy_from_slice(&xyz[0].to_le_bytes());
    data[o + 4..o + 8].copy_from_slice(&xyz[1].to_le_bytes());
    data[o + 8..o + 12].copy_from_slice(&xyz[2].to_le_bytes());
    Ok(old)
}

/// Overwrite `track`'s static scale with `sxyz`. Same constraints as
/// [`set_static_translation`].
pub fn set_static_scale(
    data: &mut [u8],
    layouts: &[TrackLayout],
    track: usize,
    sxyz: [f32; 3],
) -> Result<[f32; 3], StaticEditError> {
    let old = read_static_scale(data, layouts, track)?;
    let o = layouts[track].scl_static_off;
    data[o..o + 4].copy_from_slice(&sxyz[0].to_le_bytes());
    data[o + 4..o + 8].copy_from_slice(&sxyz[1].to_le_bytes());
    data[o + 8..o + 12].copy_from_slice(&sxyz[2].to_le_bytes());
    Ok(old)
}

/// Overwrite `track`'s static rotation with `quat` (in `[x, y, z, w]` order).
/// The input is normalized before packing, but a zero-norm quaternion errors
/// out.
///
/// Returns the old (decoded) quaternion. Round-tripping through
/// THREECOMP40 quantization is lossy at the ~1/2047 level; callers doing
/// exact byte comparisons should compare within a tolerance.
pub fn set_static_rotation(
    data: &mut [u8],
    layouts: &[TrackLayout],
    track: usize,
    quat: [f32; 4],
) -> Result<[f32; 4], StaticEditError> {
    let old = read_static_rotation(data, layouts, track)?;
    let bytes = encode_threecomp40(quat)?;
    let o = layouts[track].rot_payload_off;
    data[o..o + 5].copy_from_slice(&bytes);
    Ok(old)
}

/// Add `delta` to `track`'s current static translation, in-place.
pub fn offset_static_translation(
    data: &mut [u8],
    layouts: &[TrackLayout],
    track: usize,
    delta: [f32; 3],
) -> Result<([f32; 3], [f32; 3]), StaticEditError> {
    let old = read_static_translation(data, layouts, track)?;
    let new = [old[0] + delta[0], old[1] + delta[1], old[2] + delta[2]];
    set_static_translation(data, layouts, track, new)?;
    Ok((old, new))
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

    // -------- static-value walker / mutator tests --------

    /// Build a minimal single-block spline `data` array for one track whose
    /// translation is fully-static (mask 0x07 => 3 f32 static), rotation is a
    /// single packed THREECOMP40 quaternion (mask 0x0f), and scale is
    /// fully-static. This mirrors the retail "identity idle" pattern for
    /// non-moving bones.
    fn synth_one_track_static(
        trn: [f32; 3],
        quat_bytes: [u8; 5],
        scl: [f32; 3],
    ) -> Vec<u8> {
        // Layout inside block 0:
        //   [0..4]  per-track header {ctrl, trnMask, rotMask, sclMask}
        //   [4..X]  packed T/R/S (X aligned per channel)
        // mask_and_quantization_size == 4 (one track, one 4-byte header, no padding).
        let mut d = Vec::new();
        // Header: ctrl = rot_type=1 << 2 = 0x04 (THREECOMP40; T=8-bit, S=8-bit)
        d.push(0x04u8); // ctrl (rot_type=1)
        d.push(0x07u8); // trn_mask: static x/y/z
        d.push(0x0fu8); // rot_mask: static quaternion (any of 0x01..0x08)
        d.push(0x07u8); // scl_mask: static x/y/z
        // Translation: 3 f32 (already 4-aligned)
        for c in &trn { d.extend_from_slice(&c.to_le_bytes()); }
        // 4-byte align (already aligned since 12 bytes added).
        // Rotation: 5 bytes THREECOMP40; rot_align=1.
        d.extend_from_slice(&quat_bytes);
        // 4-byte align: need to pad from len==4+12+5==21 up to 24.
        while d.len() % 4 != 0 { d.push(0); }
        // Scale: 3 f32
        for c in &scl { d.extend_from_slice(&c.to_le_bytes()); }
        // 4-align (already aligned).
        d
    }

    #[test]
    fn walker_finds_static_translation_offset_for_a_synth_track() {
        let quat = encode_threecomp40([0.0, 0.0, 0.0, 1.0]).unwrap();
        let data = synth_one_track_static([1.0, 2.0, 3.0], quat, [0.5, 0.5, 0.5]);
        let ls = walk_spline_track_layouts(&data, 1, 4).unwrap();
        assert_eq!(ls.len(), 1);
        assert_eq!(ls[0].trn_mask, 0x07);
        assert_eq!(ls[0].rot_mask, 0x0f);
        assert_eq!(ls[0].scl_mask, 0x07);
        assert_eq!(ls[0].trn_static_off, 4); // right after the 4-byte header
        // rot at offset 4 + 12 = 16
        assert_eq!(ls[0].rot_payload_off, 16);
        // scale at 16 + 5 = 21, aligned up to 24
        assert_eq!(ls[0].scl_static_off, 24);
    }

    #[test]
    fn set_static_translation_writes_bytes_and_returns_old() {
        let quat = encode_threecomp40([0.0, 0.0, 0.0, 1.0]).unwrap();
        let mut data = synth_one_track_static([1.0, 2.0, 3.0], quat, [0.5, 0.5, 0.5]);
        let ls = walk_spline_track_layouts(&data, 1, 4).unwrap();
        let old = set_static_translation(&mut data, &ls, 0, [10.0, 20.0, 30.0]).unwrap();
        assert_eq!(old, [1.0, 2.0, 3.0]);
        let read = read_static_translation(&data, &ls, 0).unwrap();
        assert_eq!(read, [10.0, 20.0, 30.0]);
    }

    #[test]
    fn set_static_scale_writes_bytes_and_returns_old() {
        let quat = encode_threecomp40([0.0, 0.0, 0.0, 1.0]).unwrap();
        let mut data = synth_one_track_static([0.0; 3], quat, [1.0, 2.0, 4.0]);
        let ls = walk_spline_track_layouts(&data, 1, 4).unwrap();
        let old = set_static_scale(&mut data, &ls, 0, [8.0, 8.0, 8.0]).unwrap();
        assert_eq!(old, [1.0, 2.0, 4.0]);
        let read = read_static_scale(&data, &ls, 0).unwrap();
        assert_eq!(read, [8.0, 8.0, 8.0]);
    }

    #[test]
    fn set_static_rotation_writes_five_bytes_and_round_trips_approximately() {
        let quat = encode_threecomp40([0.0, 0.0, 0.0, 1.0]).unwrap();
        let mut data = synth_one_track_static([0.0; 3], quat, [1.0; 3]);
        let ls = walk_spline_track_layouts(&data, 1, 4).unwrap();
        // 45-degree rotation around Z: (0, 0, sin(pi/8), cos(pi/8))
        let target = [0.0f32, 0.0, (std::f32::consts::FRAC_PI_8).sin(),
                      (std::f32::consts::FRAC_PI_8).cos()];
        set_static_rotation(&mut data, &ls, 0, target).unwrap();
        let read = read_static_rotation(&data, &ls, 0).unwrap();
        // THREECOMP40 quantization tolerance: ~ 1/(2047*sqrt2) ~ 3.5e-4 per component.
        for i in 0..4 {
            assert!(
                (read[i].abs() - target[i].abs()).abs() < 1e-3,
                "component {i}: read={read:?} target={target:?}"
            );
        }
        // Norm ~ 1.
        let n2: f32 = read.iter().map(|c| c * c).sum();
        assert!((n2 - 1.0).abs() < 1e-3);
    }

    #[test]
    fn offset_static_translation_shifts_the_current_value() {
        let quat = encode_threecomp40([0.0, 0.0, 0.0, 1.0]).unwrap();
        let mut data = synth_one_track_static([1.0, 2.0, 3.0], quat, [1.0; 3]);
        let ls = walk_spline_track_layouts(&data, 1, 4).unwrap();
        let (old, new) = offset_static_translation(&mut data, &ls, 0, [0.5, -1.0, 100.0]).unwrap();
        assert_eq!(old, [1.0, 2.0, 3.0]);
        assert_eq!(new, [1.5, 1.0, 103.0]);
        assert_eq!(read_static_translation(&data, &ls, 0).unwrap(), new);
    }

    #[test]
    fn track_out_of_range_errors() {
        let quat = encode_threecomp40([0.0, 0.0, 0.0, 1.0]).unwrap();
        let mut data = synth_one_track_static([0.0; 3], quat, [1.0; 3]);
        let ls = walk_spline_track_layouts(&data, 1, 4).unwrap();
        let err = set_static_translation(&mut data, &ls, 5, [0.0, 0.0, 0.0]).unwrap_err();
        assert_eq!(err, StaticEditError::TrackOutOfRange { track: 5, count: 1 });
    }

    #[test]
    fn dynamic_translation_rejects_static_edit() {
        // A track whose translation is a SPLINE (mask 0x70 == bits 4|5|6, all
        // dynamic; no static). The walker still advances through it, but
        // set_static_translation must refuse.
        let mut d = Vec::new();
        // ctrl: rot_type=1 (THREECOMP40), trn_type=0 (8-bit CP), scl_type=0
        d.push(0x04u8);
        d.push(0x70u8); // trn_mask: spline x/y/z, no static
        d.push(0x0fu8); // rot: static quat
        d.push(0x07u8); // scl: static xyz
        // knot vector: n_idx=0, degree=1 -> knots = 3 bytes  (n_idx+2+degree = 3)
        d.push(0x00);
        d.push(0x00); // u16 n_idx=0
        d.push(0x01); // u8 degree=1
        // 3 knot bytes (n_idx + 2 + degree = 3):
        d.push(0);
        d.push(0);
        d.push(1);
        // align 4: current len = 4 + 2 + 1 + 3 = 10; pad to 12
        while d.len() % 4 != 0 { d.push(0); }
        // Static/range floats: 3 components each with spline -> 3 * 8 = 24 bytes of (lo,hi)
        for _ in 0..6 { d.extend_from_slice(&0f32.to_le_bytes()); }
        // Align 2 (already 4-aligned).
        // Quantized CPs: (n_idx+1) * n_comp * bytes_per = 1 * 3 * 1 = 3 bytes.
        d.push(0);
        d.push(0);
        d.push(0);
        // Align 4: current len 12+24+3=39, pad to 40.
        while d.len() % 4 != 0 { d.push(0); }
        // Rotation: 5 bytes.
        d.extend_from_slice(&encode_threecomp40([0.0, 0.0, 0.0, 1.0]).unwrap());
        while d.len() % 4 != 0 { d.push(0); }
        // Scale: 12 bytes.
        for _ in 0..3 { d.extend_from_slice(&1f32.to_le_bytes()); }
        let ls = walk_spline_track_layouts(&d, 1, 4).unwrap();
        assert_eq!(ls[0].trn_mask, 0x70);
        let mut d2 = d.clone();
        let err = set_static_translation(&mut d2, &ls, 0, [0.0; 3]).unwrap_err();
        assert!(matches!(err, StaticEditError::NotStatic { channel: "translation", .. }));
    }

    #[test]
    fn threecomp40_round_trip_preserves_within_quant_tolerance() {
        // A handful of "authored-looking" quaternions.
        for q in &[
            [0.0f32, 0.0, 0.0, 1.0],
            [1.0, 0.0, 0.0, 0.0],
            [0.5, 0.5, 0.5, 0.5],
            [0.1, -0.2, 0.3, 0.9273618],
            [(std::f32::consts::FRAC_PI_8).sin(), 0.0, 0.0,
             (std::f32::consts::FRAC_PI_8).cos()],
        ] {
            let n2: f32 = q.iter().map(|c| c * c).sum();
            let s = 1.0 / n2.sqrt();
            let qn = [q[0] * s, q[1] * s, q[2] * s, q[3] * s];
            let bytes = encode_threecomp40(qn).unwrap();
            let dec = decode_threecomp40(bytes);
            // A rotation and its negation are equivalent — compare absolute values,
            // making sure both sides picked the same canonical hemisphere.
            let dot: f32 = qn.iter().zip(dec.iter()).map(|(a, b)| a * b).sum();
            let cos_a = dot.abs().min(1.0);
            let angle_err = 2.0 * cos_a.acos();
            // THREECOMP40 quantum ~1/(2047·sqrt2) per component; angular error
            // scales up to ~0.1 deg in the worst case (largest-abs = 0.5, all
            // components equal — a rounding step falls onto every component).
            assert!(
                angle_err.to_degrees() < 0.1,
                "angle err {} deg too large for {qn:?} -> {dec:?}",
                angle_err.to_degrees()
            );
        }
    }

    #[test]
    fn threecomp40_rejects_zero_quaternion() {
        assert_eq!(encode_threecomp40([0.0, 0.0, 0.0, 0.0]).unwrap_err(),
                   StaticEditError::ZeroQuaternion);
    }

    #[test]
    fn threecomp40_normalizes_unnormalized_input() {
        // Not a unit quaternion — encoder must normalize before packing.
        let bytes = encode_threecomp40([2.0, 0.0, 0.0, 0.0]).unwrap();
        let dec = decode_threecomp40(bytes);
        let n: f32 = dec.iter().map(|c| c * c).sum::<f32>().sqrt();
        assert!((n - 1.0).abs() < 1e-3);
        // The rotation is 180deg around X; either sign of X is legal.
        assert!(dec[0].abs() > 0.99);
        assert!(dec[1].abs() < 1e-2);
        assert!(dec[2].abs() < 1e-2);
        assert!(dec[3].abs() < 1e-2);
    }

    /// Round-trip a static-translation edit through the whole write path on
    /// real retail data: parse `SAB_ANIM_PACK`, locate a fully-static
    /// translation track, apply [`set_static_translation`], splice, then
    /// re-parse the spliced pack and confirm:
    ///   * head bytes byte-identical
    ///   * tail bytes byte-identical
    ///   * Havok blob same size
    ///   * exactly 12 bytes differ (one 3xf32 static translation)
    ///   * post-decode, the edited track's translation reads back as the new
    ///     value (within f32 rounding; static values are stored as f32 so
    ///     drift is 0).
    #[test]
    fn end_to_end_static_translation_edit_on_retail_pack() {
        use std::env;
        use std::fs;
        let Ok(pack_path) = env::var("SAB_ANIM_PACK") else {
            eprintln!("skipping: SAB_ANIM_PACK not set");
            return;
        };
        let bytes = fs::read(&pack_path).expect("read pack");
        let baseline = bytes.clone();
        let ap = Ap0lPack::parse(bytes).expect("parse ap0l");
        // First, find a fully-static translation on a fixup clip (any of them).
        let mut new_blob = ap.hk_blob().to_vec();
        let (target_clip, target_track, target_off) = {
            let hk = HkPackfile::parse(&new_blob).expect("parse hk");
            let scas = hk.objects_of_class("hkaSplineCompressedAnimation");
            let mut found: Option<(usize, usize, usize)> = None;
            'outer: for (ci, &s) in scas.iter().enumerate() {
                if hk.num_blocks(s) != 1 { continue; }
                let ntt = hk.num_transform_tracks(s) as usize;
                let mq = hk.mask_and_quantization_size(s) as usize;
                let (data_off, data_len) = match hk.spline_data_span(s) {
                    Some(v) => v, None => continue,
                };
                let data_slice = &new_blob[data_off..data_off + data_len];
                let ls = match walk_spline_track_layouts(data_slice, ntt, mq) {
                    Ok(v) => v, Err(_) => continue,
                };
                for (ti, l) in ls.iter().enumerate() {
                    if l.is_fully_static_vec3_trn() {
                        found = Some((ci, ti, data_off + l.trn_static_off));
                        break 'outer;
                    }
                }
            }
            found.expect("no fully-static translation found in retail pack")
        };
        // Now apply the edit: set to (1.0, 2.0, 3.0).
        let target = [1.0f32, 2.0, 3.0];
        {
            let hk = HkPackfile::parse(&new_blob).expect("parse hk");
            let scas = hk.objects_of_class("hkaSplineCompressedAnimation");
            let obj = scas[target_clip];
            let ntt = hk.num_transform_tracks(obj) as usize;
            let mq = hk.mask_and_quantization_size(obj) as usize;
            let (data_off, data_len) = hk.spline_data_span(obj).unwrap();
            let ls = walk_spline_track_layouts(&new_blob[data_off..data_off + data_len], ntt, mq).unwrap();
            let data_mut = &mut new_blob[data_off..data_off + data_len];
            let _old = set_static_translation(data_mut, &ls, target_track, target)
                .expect("static translation write");
        }
        // Splice + verify.
        let spliced = ap.splice_hk_blob(&new_blob);
        assert_eq!(&spliced[..ap.hk_offset()], &baseline[..ap.hk_offset()],
                   "head bytes must be preserved");
        assert_eq!(&spliced[ap.hk_offset() + ap.hk_size() as usize..],
                   &baseline[ap.hk_offset() + ap.hk_size() as usize..],
                   "tail bytes must be preserved");
        assert_eq!(spliced.len(), baseline.len(),
                   "no size drift (static bytes are same width)");
        // The change should be exactly 12 bytes.
        let diffs = spliced.iter().zip(baseline.iter()).filter(|(a, b)| a != b).count();
        assert_eq!(diffs, 12, "static translation is 3xf32 = 12 bytes; got {diffs} diffs");
        let drift_pct = 100.0 * diffs as f64 / baseline.len() as f64;
        assert!(drift_pct < 0.1, "drift {drift_pct}% exceeded 0.1% budget");
        // Confirm the target offset changed as expected. `target_off` is a
        // Havok-blob-relative offset; convert to a file offset by adding the
        // AP0L blob start.
        let file_off = ap.hk_offset() + target_off;
        assert_eq!(
            f32::from_le_bytes([spliced[file_off], spliced[file_off + 1], spliced[file_off + 2], spliced[file_off + 3]]),
            target[0]
        );
        // Re-parse the spliced pack and confirm the static translation reads back.
        let ap2 = Ap0lPack::parse(spliced).expect("re-parse spliced");
        let hk2 = HkPackfile::parse(ap2.hk_blob()).expect("re-parse hk");
        let scas2 = hk2.objects_of_class("hkaSplineCompressedAnimation");
        let obj2 = scas2[target_clip];
        let ntt2 = hk2.num_transform_tracks(obj2) as usize;
        let mq2 = hk2.mask_and_quantization_size(obj2) as usize;
        let (d2_off, d2_len) = hk2.spline_data_span(obj2).unwrap();
        let ls2 = walk_spline_track_layouts(&ap2.hk_blob()[d2_off..d2_off + d2_len], ntt2, mq2)
            .expect("re-walk layouts");
        let read_back =
            read_static_translation(&ap2.hk_blob()[d2_off..d2_off + d2_len], &ls2, target_track)
                .expect("read back");
        assert_eq!(read_back, target,
                   "static translation must be exactly what was written (no re-quant)");
        eprintln!(
            "OK: clip #{target_clip} track {target_track} translation set to ({:.3},{:.3},{:.3}), \
             {diffs} bytes changed in the pack, drift {drift_pct:.6}%",
            target[0], target[1], target[2]
        );
    }

    #[test]
    fn end_to_end_ap0l_static_translation_edit_via_hkpackfile() {
        // Opt-in against a real Havok blob dump: SAB_ANIM_HKX=path/to/retail_anim.hkx.
        // Confirms that on real data, walk_spline_track_layouts produces plausible
        // offsets (monotone increasing, bounded by data_len) for every clip.
        use std::env;
        use std::fs;
        let Ok(hkx) = env::var("SAB_ANIM_HKX") else {
            eprintln!("skipping: SAB_ANIM_HKX not set");
            return;
        };
        let blob = fs::read(hkx).expect("read hkx");
        let hk = HkPackfile::parse(&blob).expect("parse hk");
        let scas = hk.objects_of_class("hkaSplineCompressedAnimation");
        assert!(!scas.is_empty());
        let mut n_walked = 0usize;
        let mut n_static_translations = 0usize;
        for &s in scas.iter().take(50) {
            let ntt = hk.num_transform_tracks(s) as usize;
            let nb = hk.num_blocks(s);
            let mq = hk.mask_and_quantization_size(s) as usize;
            if nb != 1 { continue; }
            let data = hk.spline_data(s);
            let ls = match walk_spline_track_layouts(data, ntt, mq) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("clip obj_src=0x{s:x} walk err: {e}");
                    continue;
                }
            };
            n_walked += 1;
            for lay in &ls {
                if lay.trn_mask == 0x07 { n_static_translations += 1; }
            }
        }
        assert!(n_walked > 0);
        eprintln!("walked {n_walked} clips, found {n_static_translations} fully-static translations");
    }
}
