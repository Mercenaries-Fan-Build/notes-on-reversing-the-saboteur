//! MSH4 — the 2008 Xbox 360 build's mesh format.
//!
//! ## Container framing (proven)
//!
//! An inflated MSH4 blob lives inside a `<hash>.meshes` slot's inner mini-directory (see
//! `Prototype2008Source::read_mesh_bytes`). Layout, all big-endian, byte-verified against
//! `BP_AL_Sean_HD.Pack` (164159 B), `BP_AL_Sean_LB.Pack` (110005 B):
//!
//! ```text
//! +0x00  char   magic[4]          = "MSH4"
//! +0x04  u32be  name_len          (includes trailing \0, padded up to 16-byte align)
//! +0x08  char[] name[name_len]    ASCII mesh name (e.g. "CH_AL_SeanDevlin_01_HD\0")
//!        char[] pad_to_align_16   zero pad so the payload starts 16-byte-aligned
//!
//! +0x20  … PAYLOAD (streams, primitives, drawcalls, skeleton, VB+IB, IBMs) …
//!
//! FILE_END-8  char   magic[4]     = "MSHC"
//! FILE_END-4  u32be  content_hash (some hash of the payload; not yet reproduced)
//! ```
//!
//! `MSHC + u32` is a proven 8-byte footer: on `BP_AL_Sean_HD` it lands at file_len - 8;
//! same on `BP_AL_Sean_LB`. So the payload region is bounded exactly.
//!
//! ## Payload — what's known (as of 2026-09-13, probe #4)
//!
//! The payload is a **multi-stream** VB/IB block, distinct from retail's `MSHA` in shape:
//! retail keeps VB/IB in a companion `.dat` zlib blob; MSH4 inlines them after the header.
//!
//! * **Vertex stride is per-stream**, sampled from the same file at 24, 28, and 32 bytes.
//!   The universal earmark for a skinned vertex is `ff XX XX XX XX` at bytes 10..15 —
//!   weight = 255 (single dominant bone) followed by four repeated bone indices. On Sean HD
//!   we count 314 stride-24 hits and 141 stride-28 hits in the same body.
//! * **Positions are half3 LE** (little-endian half-float) at bytes 0..5 of each vertex,
//!   with two pad bytes to align. Read as LE inside a BE container — this is the Xbox 360
//!   XGI convention where GPU-fed buffers keep D3D9-native byte order.
//! * **Indices are u16 LE triangle lists** in contiguous regions. HD's biggest IB region is
//!   at `@0x1bdde` for 33954 bytes (16977 indices = 5659 tris).
//! * **`MSHC` footer** is not the retail `MSHA` pattern — it's a new terminator we haven't
//!   seen in the released game.
//!
//! ## What this parser does today
//!
//! `parse_msh4` decodes the framing (magic, name, footer + hash), locates the aligned
//! payload region between them, and returns an `Msh4Body` whose geometry fields (positions,
//! normals, uvs, joints, weights, indices, prims) are **all empty** — a valid but empty
//! `Smsh`. It also fills `diagnostics` with the discovered payload byte-range and a scan for
//! candidate index-buffer regions so the UI has something to show.
//!
//! **Why empty geometry**: I need the stream-table layout that says "here are the N streams,
//! each with (format:u32, vcount:u32, vboff:u32, vbsize:u32, stride:u32, iboff:u32, ibsize:u32,
//! nidx:u32)". Retail's answer is a 152-byte Stream record; MSH4's may be a similar shape,
//! but its byte offset inside the payload isn't yet fixed. I've eyeballed the first ~0x50
//! bytes of the payload and see multi-word records (0x0000010f is a constant across every
//! MSH4 I've probed; the counts at 0x30/0x34/0x38 change per file) — that's where the
//! stream table lives. Extracting real geometry means resolving those offsets, and I'm
//! calling that out as the immediate follow-up rather than shipping heuristic geometry
//! that renders wrong.
//!
//! ## References
//!
//! Retail's parser (`meshload::parse_mesh` + `decode_geometry`) is the ground truth for the
//! *shape* of the tail (streams / primitives / drawcalls) and vertex bitfield decode. The
//! symbols in `game-files/Bin360/WildStar_d.map` name the 2008 loader:
//!   * `?ReadMesh@@YAXAAVPblFile@@_NAAV?$PblHandle@VOdinMesh@@@@PAVWSStreamBlock@@@Z`
//!     @ `0x82658168` in `WSLoaderXenon.obj` — the actual reader body.
//!   * `XenonMesh` / `XenonPrimitive` / `XenonSharedBuffers` classes — the runtime shape.
//! These are the next things to disassemble to nail down the stream-table byte layout.

use std::io::Read;

use crate::formats::Smsh;

/// The 4-byte magic that starts every MSH4 record after zlib inflate.
pub const MSH4_MAGIC: &[u8; 4] = b"MSH4";
/// The 4-byte magic of the 8-byte footer at the end of every MSH4 record.
pub const MSHC_FOOTER_MAGIC: &[u8; 4] = b"MSHC";

/// The alignment the mesh-name field is padded up to. `name_len` includes the trailing NUL,
/// but the payload starts on the next 16-byte boundary after it — confirmed on both HD (name
/// occupies 8..30, payload starts at 32) and LB (same layout).
///
/// 16 is chosen because 8 does not fit (name_len 23 → 8+23=31 → next 8-aligned = 32 works
/// too, but 16-aligned reads as universal for the samples). Widening to 16 will pass both.
const NAME_PAYLOAD_ALIGN: usize = 16;

/// One MSH4 record, parsed out of the raw inflated bytes.
///
/// Geometry fields are `Vec` so the parser can grow into decoding them without breaking any
/// caller that already writes them into a `Smsh` (see [`to_smsh`]). Everything except `name`,
/// `content_hash`, and `payload_range` is currently empty — see the module comment.
pub struct Msh4Body {
    /// The mesh's plaintext name as written in the file header (e.g. `"CH_AL_SeanDevlin_01_HD"`).
    /// NUL bytes are trimmed.
    pub name: String,
    /// The `MSHC` footer's trailing `u32be` — a content hash whose derivation we don't yet
    /// reproduce. Preserved so a round-trip re-write can put it back verbatim.
    pub content_hash: u32,
    /// `[start .. end)` byte-range of the payload region **inside the original body bytes**
    /// (i.e. between the aligned name-pad and the `MSHC` footer). Callers that want to
    /// inspect the still-unparsed bulk of the mesh use this to slice.
    pub payload_range: std::ops::Range<usize>,

    // ─── the retail-Smsh-shaped fields — currently empty best-effort ──────────────────────
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub joints: Vec<[u16; 4]>,
    pub weights: Vec<[f32; 4]>,
    pub indices: Vec<u32>,
    /// One entry per drawcall: `(index_start, index_count, material_hash)`. Empty until the
    /// primitive+drawcall table is decoded.
    pub prims: Vec<(u32, u32, u32)>,

    /// What the parser found while walking the payload — index-buffer candidates, apparent
    /// vertex-stride histogram, anything that helps a downstream inspector understand what's
    /// present without re-running the scan.
    pub diagnostics: Msh4Diagnostics,
}

/// What the parser was able to observe about the payload, even when it couldn't fully decode
/// the streams. Kept structured (not a debug string) so the UI can render fields
/// individually.
#[derive(Debug, Default, Clone)]
pub struct Msh4Diagnostics {
    /// Byte length of the payload region between the aligned name-pad and the MSHC footer.
    pub payload_len: usize,
    /// Byte-offsets of contiguous runs of `u16 LE < 4096` at least 128 words long — a
    /// heuristic proxy for "this looks like an index buffer". Reported as
    /// `(payload_relative_offset, num_u16_words)`.
    pub index_regions: Vec<(usize, usize)>,
    /// Histogram of stride deltas between consecutive `ff XX XX XX XX` skin markers, keyed by
    /// stride. Sean HD reports `{24: 314, 28: 141, 32: 7}` — that's three vertex formats.
    pub vertex_stride_histogram: std::collections::BTreeMap<u32, u32>,
}

/// Parse one MSH4 body — the inflated bytes from a `.meshes` slot's zlib stream.
///
/// Errors on:
///   * `bytes.len() < 12` (can't even hold magic+len+footer)
///   * missing `MSH4` header magic
///   * missing `MSHC` footer magic (checked at `bytes.len() - 8`)
///   * a `name_len` that would overrun the buffer
///   * a name that isn't valid ASCII printable + NUL
pub fn parse_msh4(bytes: &[u8]) -> Result<Msh4Body, String> {
    // ── header ────────────────────────────────────────────────────────────────────────────
    // A minimum viable MSH4 is 4B magic + 4B name_len + 1B name + 8B MSHC footer = 17 bytes.
    // Anything smaller can't hold every field the format demands, so refuse before any
    // arithmetic that could underflow.
    if bytes.len() < 17 {
        return Err(format!(
            "MSH4: too short ({} bytes) — need at least magic+name_len+1char+MSHC footer",
            bytes.len()
        ));
    }
    if &bytes[0..4] != MSH4_MAGIC {
        return Err(format!(
            "MSH4: missing header magic (got {:02X?}, expected {:02X?})",
            &bytes[0..4],
            MSH4_MAGIC,
        ));
    }
    // name_len is BIG-endian u32 (the entire container is BE, per the SBL4 parent — see
    // `sources/prototype2008_subpack.rs`). Includes the trailing NUL in every sample.
    let name_len = u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;
    // A name longer than 256 bytes almost certainly means we're not reading the right file.
    // The XenonMesh loader in the game uses a `char[0x100]` buffer for the name; sampling
    // showed names in the 15–40-byte range. Refuse anything absurd early rather than let a
    // downstream slice panic.
    if name_len == 0 || name_len > 256 {
        return Err(format!("MSH4: implausible name_len {name_len}"));
    }
    let name_end = 8 + name_len;
    if name_end > bytes.len() {
        return Err(format!(
            "MSH4: name_len {name_len} would overrun buffer ({} bytes)",
            bytes.len()
        ));
    }
    let name_bytes = &bytes[8..name_end];
    // Names are printable ASCII with a trailing NUL — accept a single NUL at the end and
    // require every other byte to be printable, so a bit-flip on the first byte can't sneak
    // through as a valid string.
    for (i, &b) in name_bytes.iter().enumerate() {
        let is_terminal_nul = b == 0 && i == name_len - 1;
        let is_printable_ascii = (0x20..0x7f).contains(&b);
        if !(is_terminal_nul || is_printable_ascii) {
            return Err(format!(
                "MSH4: non-printable byte 0x{b:02X} at name position {i} (name_len={name_len})"
            ));
        }
    }
    let name = std::str::from_utf8(name_bytes)
        .map_err(|_| "MSH4: name is not valid UTF-8".to_string())?
        .trim_end_matches('\0')
        .to_string();

    // ── footer ────────────────────────────────────────────────────────────────────────────
    // MSHC + u32be = 8 bytes; must be at the very end. Verified on every real sample.
    let footer_off = bytes.len() - 8;
    if &bytes[footer_off..footer_off + 4] != MSHC_FOOTER_MAGIC {
        return Err(format!(
            "MSH4: missing MSHC footer magic at file offset {footer_off:#x} (got \
             {:02X?}, expected {:02X?})",
            &bytes[footer_off..footer_off + 4],
            MSHC_FOOTER_MAGIC,
        ));
    }
    let content_hash =
        u32::from_be_bytes([bytes[footer_off + 4], bytes[footer_off + 5], bytes[footer_off + 6], bytes[footer_off + 7]]);

    // ── payload window ────────────────────────────────────────────────────────────────────
    // Payload sits between the aligned name-pad and the footer. On the sampled files the
    // pad rounds `name_end` up to the next 16-byte boundary; if a future sample doesn't
    // follow that rule the payload_range simply narrows, but the framing check above still
    // guarantees we don't collide with the footer.
    let payload_start = name_end.div_ceil(NAME_PAYLOAD_ALIGN) * NAME_PAYLOAD_ALIGN;
    if payload_start > footer_off {
        return Err(format!(
            "MSH4: aligned name-pad end 0x{payload_start:X} overruns MSHC footer at \
             0x{footer_off:X} — malformed header"
        ));
    }
    let payload_range = payload_start..footer_off;

    // ── diagnostics scan (best-effort; never fatal) ───────────────────────────────────────
    // This is where we would build the real streams array. Until that lands, at least log
    // *what's there* so an inspector can see structure — index-buffer-shaped regions and
    // the vertex-stride histogram from skin markers. Neither is authoritative geometry;
    // both are proof-of-life for the not-yet-decoded region.
    let diagnostics = scan_payload(&bytes[payload_range.clone()]);

    Ok(Msh4Body {
        name,
        content_hash,
        payload_range,
        positions: Vec::new(),
        normals: Vec::new(),
        uvs: Vec::new(),
        joints: Vec::new(),
        weights: Vec::new(),
        indices: Vec::new(),
        prims: Vec::new(),
        diagnostics,
    })
}

/// Fold a parsed `Msh4Body` into the retail `Smsh` shape the renderer already consumes.
///
/// Until the payload decode lands this hands back an empty `Smsh` — the renderer's
/// `set_mesh` accepts an empty mesh (no positions, no indices) and simply draws nothing,
/// which is the correct behaviour when the parser can't yet extract geometry.
pub fn to_smsh(body: &Msh4Body) -> Smsh {
    Smsh {
        positions: body.positions.clone(),
        normals: body.normals.clone(),
        uvs: body.uvs.clone(),
        joints: body.joints.clone(),
        weights: body.weights.clone(),
        indices: body.indices.clone(),
        prims: body
            .prims
            .iter()
            .map(|&(start, count, material)| crate::formats::Prim {
                index_start: start,
                index_count: count,
                material_hash: material,
                flags: 0,
                parent_bone: 0,
            })
            .collect(),
    }
}

/// Inflate the first zlib stream of a `.meshes` slot into its raw MSH4 body bytes.
///
/// A `.meshes` slot is itself a mini directory (`BE u32 count | count × 24-byte records |
/// zlib streams`); each record's zlib blob inflates to a distinct MSH4 record. This helper
/// reads just the first record — that matches every 2008-build character part sampled,
/// which ships one mesh per pack.
pub fn inflate_first_msh4_stream(slot_bytes: &[u8]) -> Result<Vec<u8>, String> {
    if slot_bytes.len() < 4 {
        return Err(format!(
            "meshes slot: too short ({} bytes) for a directory header",
            slot_bytes.len()
        ));
    }
    let count =
        u32::from_be_bytes([slot_bytes[0], slot_bytes[1], slot_bytes[2], slot_bytes[3]]) as usize;
    if count == 0 || count > 4096 {
        return Err(format!("meshes slot: implausible record count {count}"));
    }
    // First record starts at 4; each is 24 bytes {hash, off, csz, unc, flag, block}, BE u32s.
    let rec = &slot_bytes.get(4..4 + 24).ok_or("meshes slot: truncated record 0")?;
    let off = u32::from_be_bytes([rec[4], rec[5], rec[6], rec[7]]) as usize;
    let csz = u32::from_be_bytes([rec[8], rec[9], rec[10], rec[11]]) as usize;
    let unc = u32::from_be_bytes([rec[12], rec[13], rec[14], rec[15]]) as usize;
    if off + csz > slot_bytes.len() {
        return Err(format!(
            "meshes slot: record 0 [{off}..{end}) overruns slot ({} bytes)",
            slot_bytes.len(),
            end = off + csz,
        ));
    }
    let mut dec = flate2::read::ZlibDecoder::new(&slot_bytes[off..off + csz]);
    let mut out = Vec::with_capacity(unc);
    dec.read_to_end(&mut out).map_err(|e| format!("meshes slot: zlib decode: {e}"))?;
    if !out.starts_with(MSH4_MAGIC) {
        return Err(format!(
            "meshes slot: inflated stream doesn't start with MSH4 (first 4 bytes: {:02X?})",
            &out[..4.min(out.len())]
        ));
    }
    Ok(out)
}

// ── payload scanner ─────────────────────────────────────────────────────────────────────

/// Heuristic scan over the payload region: catalogue index-buffer-shaped runs and the
/// stride histogram implied by skin-vertex markers. NOT authoritative — the goal is to
/// give an inspector a shape hint until the real stream-table decode lands.
fn scan_payload(pay: &[u8]) -> Msh4Diagnostics {
    let mut diag = Msh4Diagnostics { payload_len: pay.len(), ..Default::default() };
    // Index candidates: runs of `u16 LE < 4096` of length ≥ 128 words. 4096 is generous
    // enough to cover a large character-part mesh (Sean HD's biggest IB tops out ~ 3400)
    // and small enough to reject arbitrary bytes that happen to LE-read as small.
    let mut i = 0usize;
    let mut run_start: Option<usize> = None;
    let mut run_count = 0usize;
    while i + 2 <= pay.len() {
        let v = u16::from_le_bytes([pay[i], pay[i + 1]]);
        if v < 4096 {
            if run_start.is_none() {
                run_start = Some(i);
            }
            run_count += 1;
        } else {
            if let Some(s) = run_start {
                if run_count >= 128 {
                    diag.index_regions.push((s, run_count));
                }
            }
            run_start = None;
            run_count = 0;
        }
        i += 2;
    }
    if let Some(s) = run_start {
        if run_count >= 128 {
            diag.index_regions.push((s, run_count));
        }
    }
    // Skin markers: `0xFF XX XX XX XX` where all four `XX` are equal — a single-bone
    // skinned vertex (weight = 255 UNORM on one bone, indices repeat the same bone). The
    // spacing between hits gives us the vertex stride for that stream.
    let mut hits: Vec<usize> = Vec::new();
    let n = pay.len();
    if n >= 5 {
        for j in 0..=n - 5 {
            if pay[j] == 0xFF && pay[j + 1] == pay[j + 2] && pay[j + 1] == pay[j + 3] && pay[j + 1] == pay[j + 4] {
                hits.push(j);
            }
        }
    }
    for w in hits.windows(2) {
        let d = (w[1] - w[0]) as u32;
        // Cap at 128 to avoid recording the "gap between streams" as if it were a stride;
        // a real stride sits at 16..64.
        if d > 0 && d <= 128 {
            *diag.vertex_stride_histogram.entry(d).or_insert(0) += 1;
        }
    }
    diag
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Round-trip: build the smallest possible synthetic MSH4 body — MSH4 magic + name +
    /// footer, no payload — and check the parser handles it. This runs on machines with no
    /// prototype install and pins the framing invariants.
    #[test]
    fn parses_minimal_synthetic_msh4() {
        let mut b = Vec::new();
        b.extend_from_slice(MSH4_MAGIC);
        // name "AB\0", padded up to 16-byte align (payload starts at 16 or 32 depending on
        // where the pad lands). name_len includes the NUL, so 3 bytes.
        b.extend_from_slice(&3u32.to_be_bytes());
        b.extend_from_slice(b"AB\0");
        // Zero-pad the head so payload_start (16) is honoured; keep the payload region
        // itself empty. We land at file offset 8+3 = 11, so we need 5 bytes of pad to hit
        // 16 — the parser's alignment step advances name_end up to 16 automatically, so
        // the raw bytes here can just be zeros of the right length.
        b.extend_from_slice(&[0u8; 5]);
        // MSHC + u32 footer.
        b.extend_from_slice(MSHC_FOOTER_MAGIC);
        b.extend_from_slice(&0xDEADBEEFu32.to_be_bytes());

        let m = parse_msh4(&b).expect("parse synthetic MSH4");
        assert_eq!(m.name, "AB");
        assert_eq!(m.content_hash, 0xDEADBEEF);
        // Payload window: from aligned end-of-name (16) to start-of-footer (b.len() - 8).
        assert_eq!(m.payload_range.start, 16);
        assert_eq!(m.payload_range.end, b.len() - 8);
        // No geometry is decoded yet — every field should be empty.
        assert!(m.positions.is_empty());
        assert!(m.indices.is_empty());
        assert!(m.prims.is_empty());
    }

    /// Guard the framing: every "obviously wrong" input must error (not panic, not
    /// silently accept). Each subcase names what it's protecting against.
    #[test]
    fn rejects_malformed_headers_and_truncation() {
        // Empty and near-empty inputs.
        assert!(parse_msh4(&[]).is_err(), "empty must error");
        assert!(parse_msh4(&[0u8; 4]).is_err(), "just magic-length is not enough");
        // Wrong magic.
        let mut bad = Vec::from(*b"NOPE");
        bad.extend_from_slice(&0u32.to_be_bytes());
        bad.extend_from_slice(&[0u8; 8]);
        assert!(parse_msh4(&bad).is_err(), "wrong magic must error");
        // Huge name_len that would overrun.
        let mut huge = Vec::from(*b"MSH4");
        huge.extend_from_slice(&0xFFFF_0000u32.to_be_bytes());
        huge.extend_from_slice(&[0u8; 20]);
        assert!(parse_msh4(&huge).is_err(), "implausible name_len must error");
        // Missing MSHC footer.
        let mut no_footer = Vec::from(*b"MSH4");
        no_footer.extend_from_slice(&3u32.to_be_bytes());
        no_footer.extend_from_slice(b"AB\0");
        no_footer.extend_from_slice(&[0u8; 24]); // 24 zero bytes — no MSHC at the end
        assert!(parse_msh4(&no_footer).is_err(), "missing MSHC footer must error");
        // Non-printable byte inside the name.
        let mut bad_name = Vec::from(*b"MSH4");
        bad_name.extend_from_slice(&3u32.to_be_bytes());
        bad_name.extend_from_slice(&[0x01, b'A', 0]); // control char at name[0]
        bad_name.extend_from_slice(&[0u8; 5]);
        bad_name.extend_from_slice(MSHC_FOOTER_MAGIC);
        bad_name.extend_from_slice(&0u32.to_be_bytes());
        assert!(parse_msh4(&bad_name).is_err(), "non-printable name byte must error");
    }

    // ── real-corpus checks — SKIPPED cleanly when the prototype isn't extracted ────────

    fn bin360_root() -> Option<std::path::PathBuf> {
        let p = std::path::PathBuf::from(
            "c:/Users/Shadow/Desktop/notes-on-reversing-the-sabetour/game-files",
        );
        if p.join("Bin360").is_dir() { Some(p) } else { None }
    }

    /// Real MSH4 blob out of the prototype must parse cleanly, land on the known name, and
    /// pin a plausible payload region. This is the acceptance test — if it passes on
    /// `BP_AL_Sean_HD.Pack` the framing decode is correct against the actual game data.
    #[test]
    fn parses_real_sean_hd_mesh() {
        let Some(root) = bin360_root() else {
            eprintln!("skip: no Bin360/ prototype");
            return;
        };
        use crate::source::GameSource;
        use crate::sources::prototype2008::Prototype2008Source;
        let src = Prototype2008Source::open(&root).expect("open prototype");
        let handle = src
            .list_meshes()
            .into_iter()
            .find(|h| h.name == "BP_AL_Sean_HD")
            .expect("BP_AL_Sean_HD mesh present");
        let slot = src.read_mesh_bytes(&handle).expect("read Sean HD .meshes slot");
        let raw = inflate_first_msh4_stream(&slot).expect("inflate first MSH4 stream");
        let body = parse_msh4(&raw).expect("parse MSH4 body");
        assert_eq!(body.name, "CH_AL_SeanDevlin_01_HD", "the MSH4 name matches the asset");
        // The payload window must be non-empty and end well before EOF (MSHC footer takes 8).
        assert!(body.payload_range.start >= 16);
        assert!(body.payload_range.end <= raw.len() - 8);
        assert!(body.payload_range.end > body.payload_range.start);
        // The MSHC footer's content_hash isn't reproducible from our end yet, but it must
        // be a real value the parser preserved verbatim (rather than 0 from a mis-read).
        // On BP_AL_Sean_HD the observed value is 0x8A70949E — pin it so a regression is loud.
        assert_eq!(body.content_hash, 0x8A70949E, "MSHC footer hash for Sean HD");
        // Diagnostics: the scanner should flag at least one big index-buffer-shaped region
        // (Sean HD's biggest IB is ~33 KB of u16 LE indices).
        assert!(
            body.diagnostics.index_regions.iter().any(|&(_, words)| words > 1000),
            "expected at least one big u16-LE index region — payload scanner regressed?"
        );
        // And a stride histogram with entries at 24, 28 — the vertex formats Sean HD uses.
        assert!(
            body.diagnostics.vertex_stride_histogram.get(&24).copied().unwrap_or(0) > 100,
            "expected many stride-24 skin markers on Sean HD (got histogram: {:?})",
            body.diagnostics.vertex_stride_histogram
        );
    }

    /// Negative test on real data: passing a TRUNCATED prototype MSH4 must return `Err`,
    /// never panic. A missing MSHC at the end is the canonical failure mode when a slot
    /// carries a shorter stream than expected.
    #[test]
    fn truncated_real_mesh_returns_err() {
        let Some(root) = bin360_root() else {
            eprintln!("skip: no Bin360/ prototype");
            return;
        };
        use crate::source::GameSource;
        use crate::sources::prototype2008::Prototype2008Source;
        let src = Prototype2008Source::open(&root).expect("open prototype");
        let handle = src
            .list_meshes()
            .into_iter()
            .find(|h| h.name == "BP_AL_Sean_HD")
            .expect("BP_AL_Sean_HD present");
        let slot = src.read_mesh_bytes(&handle).expect("read slot");
        let raw = inflate_first_msh4_stream(&slot).expect("inflate MSH4");
        // Drop the last 32 bytes — that includes the MSHC footer, so the parser must reject
        // rather than run off the end of the payload window.
        let truncated = &raw[..raw.len() - 32];
        assert!(parse_msh4(truncated).is_err(), "truncated body must Err, not panic");
    }

    /// The Prototype2008Source's `read_mesh_smsh` helper wraps the parser + `to_smsh`
    /// conversion so the renderer's `set_mesh` can accept a 2008 asset the same way it
    /// accepts a retail one. Verify the wrapper resolves against Sean HD and produces a
    /// valid (empty) Smsh — the renderer draws nothing, but nothing panics.
    #[test]
    fn read_mesh_smsh_returns_valid_empty_smsh_for_real_mesh() {
        let Some(root) = bin360_root() else {
            eprintln!("skip: no Bin360/ prototype");
            return;
        };
        use crate::source::GameSource;
        use crate::sources::prototype2008::Prototype2008Source;
        let src = Prototype2008Source::open(&root).expect("open prototype");
        let handle = src
            .list_meshes()
            .into_iter()
            .find(|h| h.name == "BP_AL_Sean_HD")
            .expect("BP_AL_Sean_HD present");
        let m = src.read_mesh_smsh(&handle).expect("read Sean HD as Smsh");
        // Geometry is empty for now — that's the honest state. But an empty Smsh must
        // still be internally consistent (indices divisible by 3 vacuously holds).
        assert_eq!(m.positions.len(), 0);
        assert_eq!(m.indices.len() % 3, 0);
    }
}
