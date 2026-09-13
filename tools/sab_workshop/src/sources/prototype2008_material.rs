//! 2008 Xbox 360 `<hash>.materials` slot parser — the shard-per-block equivalent of retail's
//! single loose `France.materials`.
//!
//! The 2008 build has NO consolidated material file: every packlog block ships its own
//! `<blockhash>.materials` slot inside its `.Pack` container. Each slot is a self-contained
//! **big-endian** `WSAO` file with an inline stream of `MTLD` records (one per material). This
//! contrasts with retail, which uses **little-endian** `OASW` with separate `WSMA` / `WSTX`
//! blocks and indirection through a shared texture-hash array.
//!
//! **What we consume** (the minimum the workshop needs to render/preview): the material UID and
//! its `[diffuse, spec, normal, wm]` texture-slot hashes. The full record has aliases + render
//! passes + shader / constant / property sub-blocks that we skip — those matter for exact
//! shader reconstruction, not for the picker + Blinn-Phong preview + bundle export.
//!
//! ── On-disk layout (verified against `Global/Common.materials` and `Global/Dynamite.materials`;
//! byte-level ground truth in `docs/2008_materials_format.md`) ─────────────────────────────────
//!
//! ```text
//! WSAO file:
//!   char magic[4]   = "WSAO"          (BE — retail is "OASW" LE)
//!   u32  num_records                  (BE)
//!   MTLD records[num_records]
//!
//! MTLD record:
//!   char magic[4]   = "MTLD"
//!   u32  uid                          (BE; drawcall material hash)
//!   u32  body_size                    (BE; bytes that follow, NOT counting these 12)
//!   body[body_size]:
//!     u32  name_len                   (BE; INCLUDES the trailing NUL)
//!     char name[name_len]             (NOT 4-byte padded; subsequent u32s are unaligned)
//!     u32  num_textures               (BE)
//!     u32  texture_hashes[num_textures]  (slot order [diffuse, spec, normal, wm])
//!     ... sub-blocks (render passes, constants, shader refs) — skipped for now ...
//! ```
//!
//! ── SBL4-container wrapping (the ~98% we can't reach in this probe) ───────────────────────────
//!
//! The `.Pack` files themselves are `SBL4` sub-pack containers (probe #1's target). For **12**
//! of the ~717 non-truncated material slots on the current extract, the packlog `Start:` offset
//! happens to land on raw `WSAO` bytes we can read straight through — those give us the ~78
//! materials this index holds today. For the remaining ~705 slots, the container has re-laid-out
//! the payload and the packlog offset lands on high-entropy container bytes; a handful of those
//! start with the literal `SBL4` magic, most just look random. Both cases are counted separately
//! (`slots_sbl4` + `slots_bad_magic`) — probe #1 unwraps both, at which point every material in
//! the build becomes indexable and this parser stays exactly as-is: it just gets fed un-wrapped
//! bytes.

use std::collections::HashMap;

/// One material entry, in the shape the workshop wants — a lookup key + the 4-slot vector.
#[derive(Clone, Debug)]
pub struct MaterialRecord {
    /// The drawcall material hash. Drawcalls in `.meshes` slots reference this.
    pub uid: u32,
    /// Slot order `[diffuse, specular, normal, wm]`. `None` in a slot the material did not
    /// declare (i.e., `num_textures < 4`). Filled left-to-right from the on-disk texture array.
    pub textures: [Option<u32>; 4],
    /// The material's plaintext name (e.g. `"DefaultLib.GR_C2_metal:0"`). Kept for the
    /// picker / name inventory — 2008 packlogs don't name materials on their own (each block
    /// has ONE `.materials` slot, so the packlog only knows the block name).
    pub name: String,
}

fn be_u32(b: &[u8], o: usize) -> Option<u32> {
    (o + 4 <= b.len()).then(|| {
        u32::from_be_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
    })
}

/// Parse one 2008 `.materials` slot into its `MaterialRecord`s.
///
/// Returns `Err` when the slot isn't a WSAO (SBL4-wrapped is the common non-error reason; the
/// caller uses that to bump the "unreadable_slots" counter rather than fail loudly). A slot
/// whose header parses but whose records go wrong is truncated at the last good record — one
/// bad tail record doesn't lose the earlier good ones.
pub fn parse_wsao(bytes: &[u8]) -> Result<Vec<MaterialRecord>, String> {
    if bytes.len() < 8 {
        return Err(format!("too small ({} bytes)", bytes.len()));
    }
    if &bytes[0..4] == b"SBL4" {
        return Err("SBL4-wrapped (needs sub-pack unwrap — probe #1)".into());
    }
    if &bytes[0..4] != b"WSAO" {
        return Err(format!("bad magic {:02X?}", &bytes[0..4]));
    }
    let num_records = be_u32(bytes, 4).ok_or("truncated header")? as usize;

    let mut out = Vec::with_capacity(num_records);
    let mut o = 8usize;
    for _ in 0..num_records {
        if o + 12 > bytes.len() { break; }
        if &bytes[o..o + 4] != b"MTLD" { break; }
        let uid = be_u32(bytes, o + 4).unwrap();
        let body_size = be_u32(bytes, o + 8).unwrap() as usize;
        let body_start = o + 12;
        let body_end = body_start + body_size;
        if body_end > bytes.len() { break; }

        let body = &bytes[body_start..body_end];
        if let Some(rec) = parse_record(uid, body) {
            out.push(rec);
        }
        // Advance regardless of whether we could interpret the body — the record framing itself
        // is separately valid (magic + size), so a body we can't decode still lets us find the
        // NEXT record. Stopping on a mid-file parse failure would silently lose the tail.
        o = body_end;
    }
    Ok(out)
}

fn parse_record(uid: u32, body: &[u8]) -> Option<MaterialRecord> {
    let name_len = be_u32(body, 0)? as usize;
    // A sane cap: material names in the samples are 6–37 bytes. A wildly-large name_len means
    // we're not looking at a real record body.
    if name_len == 0 || name_len > 512 || 4 + name_len > body.len() {
        return None;
    }
    let name_bytes = &body[4..4 + name_len];
    // Strip the trailing NUL — name_len INCLUDES it in this format.
    let name = String::from_utf8_lossy(
        name_bytes.strip_suffix(&[0u8]).unwrap_or(name_bytes),
    )
    .into_owned();

    // Immediately after the name (NO alignment padding — subsequent u32s are unaligned reads):
    //   u32  num_textures
    //   u32  texture_hashes[num_textures]
    let p = 4 + name_len;
    let num_textures = be_u32(body, p)? as usize;
    // Real records have 1..=4 textures on this build; a much larger value means the layout
    // guess is wrong for this record (e.g. a rare variant we haven't seen) — skip cleanly.
    if num_textures > 10 {
        return None;
    }
    let mut textures = [None; 4];
    for i in 0..num_textures {
        let h = be_u32(body, p + 4 + i * 4)?;
        if i < 4 && h != 0 {
            textures[i] = Some(h);
        }
    }
    Some(MaterialRecord { uid, textures, name })
}

/// Build a global `material_uid → [d,s,n,wm]` lookup by walking every `.materials` slot.
///
/// Slots that fail to parse (SBL4-wrapped, truncated, bad magic) are silently counted — the
/// caller reports the counts in the fingerprint. The map's value is the 4-slot vector directly,
/// so `textures_for_material` is a `HashMap::get` and nothing more.
pub struct MaterialIndex {
    pub by_uid: HashMap<u32, [Option<u32>; 4]>,
    /// Plaintext material names by uid — feeds the workshop's `name_of_hash` fallback for
    /// material handles (packlogs only name BLOCKS, not the materials inside them).
    pub name_by_uid: HashMap<u32, String>,
    pub slots_read: usize,
    pub slots_ok: usize,
    pub slots_sbl4: usize,
    pub slots_bad_magic: usize,
    pub materials_indexed: usize,
}

impl MaterialIndex {
    pub fn new() -> Self {
        Self {
            by_uid: HashMap::new(),
            name_by_uid: HashMap::new(),
            slots_read: 0,
            slots_ok: 0,
            slots_sbl4: 0,
            slots_bad_magic: 0,
            materials_indexed: 0,
        }
    }

    /// Feed one raw material-slot blob. Returns the number of materials extracted from it (0 if
    /// the slot wasn't parseable), so the caller can chain per-slot logging without a second
    /// pass.
    pub fn ingest_slot(&mut self, bytes: &[u8]) -> usize {
        self.slots_read += 1;
        match parse_wsao(bytes) {
            Ok(records) => {
                self.slots_ok += 1;
                let n = records.len();
                for rec in records {
                    // First writer wins — same-name materials CAN appear across many blocks (e.g.
                    // `PGA:0` occurs twice in Common alone with different UIDs). We key on UID
                    // so both distinct entries survive; the name map only records one.
                    self.by_uid.entry(rec.uid).or_insert(rec.textures);
                    self.name_by_uid.entry(rec.uid).or_insert(rec.name);
                }
                self.materials_indexed += n;
                n
            }
            Err(e) if e.starts_with("SBL4-wrapped") => {
                self.slots_sbl4 += 1;
                0
            }
            Err(_) => {
                self.slots_bad_magic += 1;
                0
            }
        }
    }

    pub fn textures_for(&self, uid: u32) -> Option<[Option<u32>; 4]> {
        self.by_uid.get(&uid).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Manufactured minimum-WSAO with a single MTLD carrying two textures — proves the parser
    /// wiring end-to-end without depending on real game files.
    fn make_wsao_one(name: &str, uid: u32, tex: &[u32]) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(b"WSAO");
        b.extend_from_slice(&1u32.to_be_bytes()); // one record

        // Build body
        let mut body = Vec::new();
        let name_bytes = {
            let mut v = name.as_bytes().to_vec();
            v.push(0);
            v
        };
        body.extend_from_slice(&(name_bytes.len() as u32).to_be_bytes());
        body.extend_from_slice(&name_bytes);
        body.extend_from_slice(&(tex.len() as u32).to_be_bytes());
        for h in tex {
            body.extend_from_slice(&h.to_be_bytes());
        }

        b.extend_from_slice(b"MTLD");
        b.extend_from_slice(&uid.to_be_bytes());
        b.extend_from_slice(&(body.len() as u32).to_be_bytes());
        b.extend_from_slice(&body);
        b
    }

    #[test]
    fn parses_synthetic_wsao_with_three_textures() {
        let bytes = make_wsao_one("DefaultLib.Test:0", 0xdeadbeef, &[0x1111_1111, 0x2222_2222, 0x3333_3333]);
        let recs = parse_wsao(&bytes).unwrap();
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].uid, 0xdeadbeef);
        assert_eq!(recs[0].name, "DefaultLib.Test:0");
        assert_eq!(recs[0].textures[0], Some(0x1111_1111));
        assert_eq!(recs[0].textures[1], Some(0x2222_2222));
        assert_eq!(recs[0].textures[2], Some(0x3333_3333));
        assert!(recs[0].textures[3].is_none());
    }

    #[test]
    fn sbl4_wrapper_reports_the_wrapper_reason() {
        let mut b = b"SBL4".to_vec();
        b.extend_from_slice(&[0u8; 100]);
        let err = parse_wsao(&b).unwrap_err();
        assert!(err.contains("SBL4"), "should surface the wrapper reason, got: {err}");
    }

    #[test]
    fn bad_magic_errors_cleanly() {
        let b = b"XXXX\x00\x00\x00\x00".to_vec();
        assert!(parse_wsao(&b).is_err());
    }

    /// A material with 1 texture leaves slots 1..3 as `None` — the picker expects to see the
    /// missing slots (rather than a hash of zero or a phantom repeat).
    #[test]
    fn one_texture_material_has_three_none_slots() {
        let bytes = make_wsao_one("skin", 0x1234, &[0xC0DE]);
        let recs = parse_wsao(&bytes).unwrap();
        assert_eq!(recs[0].textures, [Some(0xC0DE), None, None, None]);
    }

    /// A single MaterialIndex.ingest_slot pass populates by_uid + name_by_uid and increments
    /// the OK counter — the caller uses these to fill the fingerprint.
    #[test]
    fn material_index_ingest_wires_counters_and_lookup() {
        let bytes = make_wsao_one("DefaultLib.A:0", 0xAAA, &[0x1, 0x2, 0x3]);
        let mut idx = MaterialIndex::new();
        idx.ingest_slot(&bytes);
        assert_eq!(idx.slots_ok, 1);
        assert_eq!(idx.slots_read, 1);
        assert_eq!(idx.materials_indexed, 1);
        assert_eq!(idx.textures_for(0xAAA), Some([Some(0x1), Some(0x2), Some(0x3), None]));
        assert_eq!(idx.name_by_uid.get(&0xAAA).map(|s| s.as_str()), Some("DefaultLib.A:0"));
    }
}
