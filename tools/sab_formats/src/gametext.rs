//! GameText.dlg — The Saboteur's complete localized-text container (one file per language under
//! `Cinematics/Dialog/<Lang>/`). Holds every UI string (objectives, mission names, tooltips,
//! shop/object display names — the text GameTemplates and Lua reference) AND every cinematic VO
//! subtitle. Ground-truthed against all six retail language files and the engine parser
//! `FUN_0095f370 @0x0095f370`. See `docs/formats/gametext.md`.
//!
//! ```text
//! Header (12 bytes): u32 version=5, u32 record_count, u32 total_string_code_units
//! record_count × { "TXTD", u32 asset_id, u16 key_len(incl NUL), key[key_len], u16 str_len(CU),
//!                  u16 str[str_len] (UTF-16LE, NUL-terminated) }
//! "DNEC" section: u32 group_count, group_count × {u32 scene_hash, u32 ABS file_offset},
//!                 group_count × SubTable {u32 count, u32 tsize(==Σ str_len), count × TXTD, "DNEC"}
//! ```
//!
//! * **UI text**: `key_len == 1` (a bare NUL), `asset_id == pandemic_hash("<File>_Text.<Key>")`.
//!   Add one by appending a keyless record whose `asset_id` is that hash — no Lua registration needed.
//! * **VO subtitle**: ascii `vo_…` key; store lookup is `pandemic_hash(key)`; `asset_id` = audio event.
//! * **DNEC sub-tables**: per-cinematic-scene VO subtitles the base section does not carry (1312 extra
//!   records per language). Each is `{count, tsize, records, "DNEC" terminator}`, laid out in directory
//!   order (== ascending offset). Fully parsed and editable here; the writer rebuilds the directory and
//!   every absolute offset, so an unmodified file round-trips byte-identically and an edit anywhere just
//!   re-derives the layout. Verified on all six retail languages.

use crate::pandemic_hash;

const MAGIC_REC: &[u8; 4] = b"TXTD";
const MAGIC_DNEC: &[u8; 4] = b"DNEC";
pub const VERSION: u32 = 5;

#[derive(Clone, Debug)]
pub struct Record {
    pub asset_id: u32,
    /// Raw key bytes exactly as on disk, INCLUDING the trailing NUL. `[0x00]` (empty) for UI text.
    pub key: Vec<u8>,
    /// UTF-16LE code units of the localized string, INCLUDING its trailing NUL terminator.
    pub text: Vec<u16>,
}

impl Record {
    /// A UI-text record: empty ascii key (`key_len==1`, a bare NUL); VO records carry a `vo_…` name.
    pub fn is_ui(&self) -> bool {
        self.key_str().is_empty()
    }
    /// Ascii key without the trailing NUL ("" for UI text).
    pub fn key_str(&self) -> String {
        let end = self.key.iter().position(|&b| b == 0).unwrap_or(self.key.len());
        String::from_utf8_lossy(&self.key[..end]).into_owned()
    }
    /// The localized string without its trailing NUL terminator.
    pub fn text_string(&self) -> String {
        let end = self.text.iter().position(|&u| u == 0).unwrap_or(self.text.len());
        String::from_utf16_lossy(&self.text[..end])
    }
    /// Replace the localized string (kept NUL-terminated on disk, matching every retail record).
    pub fn set_text(&mut self, s: &str) {
        self.text = encode_text(s);
    }
    fn size(&self) -> usize {
        4 + 4 + 2 + self.key.len() + 2 + self.text.len() * 2
    }
}

/// Encode a string to the on-disk UTF-16LE form (code units + NUL terminator; `str_len` counts it).
pub fn encode_text(s: &str) -> Vec<u16> {
    let mut v: Vec<u16> = s.encode_utf16().collect();
    v.push(0);
    v
}

/// One per-scene VO-overlay sub-table from the DNEC section.
#[derive(Clone, Debug)]
pub struct SubTable {
    /// Hash of the cinematic scene; some match a loose `<hash>.pov` at the install root.
    pub scene_hash: u32,
    /// The scene's VO-subtitle records (same TXTD layout as the base records).
    pub records: Vec<Record>,
}

impl SubTable {
    /// On-disk byte size: `{count, tsize}` + records + `"DNEC"` terminator.
    fn size(&self) -> usize {
        8 + self.records.iter().map(|r| r.size()).sum::<usize>() + 4
    }
}

/// The post-records section. Retail files always carry `Dnec`; `Opaque` is a safety fallback for a
/// tail that does not begin with the `DNEC` magic (not seen in any shipped language).
#[derive(Clone, Debug)]
pub enum Tail {
    Dnec(Vec<SubTable>),
    Opaque(Vec<u8>),
}

pub struct GameText {
    pub version: u32,
    pub records: Vec<Record>,
    tail: Tail,
}

fn rd_u16(b: &[u8], o: usize) -> Result<u16, String> {
    b.get(o..o + 2).map(|s| u16::from_le_bytes([s[0], s[1]])).ok_or_else(|| format!("EOF u16 @{o}"))
}
fn rd_u32(b: &[u8], o: usize) -> Result<u32, String> {
    b.get(o..o + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]])).ok_or_else(|| format!("EOF u32 @{o}"))
}

/// Read one TXTD record at `o`, returning it and the offset just past it.
fn parse_record(b: &[u8], mut o: usize, where_: &str) -> Result<(Record, usize), String> {
    if b.get(o..o + 4) != Some(MAGIC_REC.as_slice()) {
        return Err(format!("{where_}: bad magic at {o}, expected TXTD"));
    }
    o += 4;
    let asset_id = rd_u32(b, o)?;
    o += 4;
    let key_len = rd_u16(b, o)? as usize;
    o += 2;
    let key = b.get(o..o + key_len).ok_or("EOF key")?.to_vec();
    o += key_len;
    let str_len = rd_u16(b, o)? as usize;
    o += 2;
    let raw = b.get(o..o + str_len * 2).ok_or("EOF str")?;
    let text: Vec<u16> = raw.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    o += str_len * 2;
    Ok((Record { asset_id, key, text }, o))
}

/// Serialize one TXTD record.
fn write_record(out: &mut Vec<u8>, rec: &Record) {
    out.extend_from_slice(MAGIC_REC);
    out.extend_from_slice(&rec.asset_id.to_le_bytes());
    out.extend_from_slice(&(rec.key.len() as u16).to_le_bytes());
    out.extend_from_slice(&rec.key);
    out.extend_from_slice(&(rec.text.len() as u16).to_le_bytes());
    for &u in &rec.text {
        out.extend_from_slice(&u.to_le_bytes());
    }
}

/// Parse the DNEC section beginning at `o` (the `DNEC` magic): directory of `{scene_hash, abs
/// offset}` pairs, then one `{count, tsize, records, "DNEC"}` sub-table per group. Offsets are
/// absolute; each is seeked to rather than assumed contiguous.
fn parse_dnec(b: &[u8], mut o: usize) -> Result<Tail, String> {
    o += 4; // DNEC magic (checked by caller)
    let group_count = rd_u32(b, o)? as usize;
    o += 4;
    let mut dir = Vec::with_capacity(group_count);
    for _ in 0..group_count {
        let scene_hash = rd_u32(b, o)?;
        o += 4;
        let off = rd_u32(b, o)? as usize;
        o += 4;
        dir.push((scene_hash, off));
    }
    let mut subs = Vec::with_capacity(group_count);
    for (gi, (scene_hash, off)) in dir.into_iter().enumerate() {
        let mut so = off;
        let count = rd_u32(b, so)?;
        so += 4;
        let _tsize = rd_u32(b, so)?; // == Σ str_len; recomputed on write
        so += 4;
        let mut records = Vec::with_capacity(count as usize);
        for ri in 0..count {
            let (rec, no) = parse_record(b, so, &format!("DNEC group {gi} (scene {scene_hash:08x}) record {ri}"))?;
            records.push(rec);
            so = no;
        }
        if b.get(so..so + 4) != Some(MAGIC_DNEC.as_slice()) {
            return Err(format!("DNEC group {gi} (scene {scene_hash:08x}): missing DNEC terminator at {so}"));
        }
        subs.push(SubTable { scene_hash, records });
    }
    Ok(Tail::Dnec(subs))
}

impl GameText {
    pub fn parse(b: &[u8]) -> Result<GameText, String> {
        let version = rd_u32(b, 0)?;
        if version != VERSION {
            return Err(format!("unexpected GameText version {version}, expected {VERSION}"));
        }
        let count = rd_u32(b, 4)?;
        let _total_cu = rd_u32(b, 8)?;
        let mut o = 12;
        let mut records = Vec::with_capacity(count as usize);
        for i in 0..count {
            let (rec, no) = parse_record(b, o, &format!("record {i}"))?;
            records.push(rec);
            o = no;
        }
        let tail = if b.get(o..o + 4) == Some(MAGIC_DNEC.as_slice()) {
            parse_dnec(b, o)?
        } else {
            Tail::Opaque(b[o..].to_vec())
        };
        Ok(GameText { version, records, tail })
    }

    /// Σ str_len over the base records (the header's `total_string_code_units`).
    pub fn total_code_units(&self) -> u32 {
        self.records.iter().map(|r| r.text.len() as u32).sum()
    }

    fn base_len(&self) -> usize {
        12 + self.records.iter().map(|r| r.size()).sum::<usize>()
    }

    /// The DNEC per-scene VO-overlay sub-tables (empty slice if the tail is not a DNEC section).
    pub fn dnec_groups(&self) -> &[SubTable] {
        match &self.tail {
            Tail::Dnec(subs) => subs,
            Tail::Opaque(_) => &[],
        }
    }

    /// Total DNEC sub-records across all scene groups.
    pub fn dnec_record_count(&self) -> usize {
        self.dnec_groups().iter().map(|s| s.records.len()).sum()
    }

    /// Find a base record by raw asset_id.
    pub fn find(&self, asset_id: u32) -> Option<&Record> {
        self.records.iter().find(|r| r.asset_id == asset_id)
    }
    pub fn find_mut(&mut self, asset_id: u32) -> Option<&mut Record> {
        self.records.iter_mut().find(|r| r.asset_id == asset_id)
    }
    /// Find a base record by dotted UI id (`pandemic_hash(id)`).
    pub fn find_id(&self, dotted_id: &str) -> Option<&Record> {
        self.find(pandemic_hash(dotted_id))
    }

    /// Find a record by asset_id across the base records and every DNEC sub-table. A `scene` filter
    /// restricts the search to that DNEC group (and skips the base records) — use it when the same
    /// asset_id appears in more than one place.
    pub fn find_any(&self, asset_id: u32, scene: Option<u32>) -> Option<&Record> {
        if scene.is_none() {
            if let Some(r) = self.find(asset_id) {
                return Some(r);
            }
        }
        for s in self.dnec_groups() {
            if scene.is_some_and(|sc| sc != s.scene_hash) {
                continue;
            }
            if let Some(r) = s.records.iter().find(|r| r.asset_id == asset_id) {
                return Some(r);
            }
        }
        None
    }
    pub fn find_any_mut(&mut self, asset_id: u32, scene: Option<u32>) -> Option<&mut Record> {
        if scene.is_none() && self.records.iter().any(|r| r.asset_id == asset_id) {
            return self.records.iter_mut().find(|r| r.asset_id == asset_id);
        }
        if let Tail::Dnec(subs) = &mut self.tail {
            for s in subs.iter_mut() {
                if scene.is_some_and(|sc| sc != s.scene_hash) {
                    continue;
                }
                if let Some(r) = s.records.iter_mut().find(|r| r.asset_id == asset_id) {
                    return Some(r);
                }
            }
        }
        None
    }

    /// Append a NEW VO-subtitle record to an existing DNEC scene group. `asset_id` is the store
    /// lookup key (for VO records it is an opaque id, NOT `pandemic_hash(key)`); `key` is the ascii
    /// `vo_…` name (pass "" for a keyless record). Errors if the scene group does not exist or the
    /// asset_id is already present in it. The writer recomputes the group's `count`/`tsize` and every
    /// absolute offset, so the resulting file stays consistent.
    pub fn add_dnec(&mut self, scene_hash: u32, asset_id: u32, key: &str, text: &str) -> Result<(), String> {
        let subs = match &mut self.tail {
            Tail::Dnec(subs) => subs,
            Tail::Opaque(_) => return Err("file has no DNEC section".into()),
        };
        let s = subs
            .iter_mut()
            .find(|s| s.scene_hash == scene_hash)
            .ok_or_else(|| format!("no DNEC scene group 0x{scene_hash:08x}"))?;
        if s.records.iter().any(|r| r.asset_id == asset_id) {
            return Err(format!("scene 0x{scene_hash:08x} already has asset_id 0x{asset_id:08x}"));
        }
        let key_bytes = if key.is_empty() {
            vec![0u8]
        } else {
            let mut k = key.as_bytes().to_vec();
            k.push(0);
            k
        };
        s.records.push(Record { asset_id, key: key_bytes, text: encode_text(text) });
        Ok(())
    }

    /// Append a NEW UI-text record for a dotted id (`asset_id = pandemic_hash(id)`, empty key).
    /// Errors if the id already exists (use `find_mut(...).set_text` to edit).
    pub fn add_ui(&mut self, dotted_id: &str, text: &str) -> Result<u32, String> {
        let asset_id = pandemic_hash(dotted_id);
        if self.records.iter().any(|r| r.asset_id == asset_id) {
            return Err(format!("id {dotted_id:?} (0x{asset_id:08x}) already exists"));
        }
        self.records.push(Record { asset_id, key: vec![0u8], text: encode_text(text) });
        Ok(asset_id)
    }

    /// Serialize back to bytes: header + base records, then the DNEC directory + sub-tables with
    /// every absolute offset and sub-table header recomputed. Unmodified input round-trips
    /// byte-identical; an edit anywhere re-derives the layout.
    pub fn write(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.base_len());
        out.extend_from_slice(&self.version.to_le_bytes());
        out.extend_from_slice(&(self.records.len() as u32).to_le_bytes());
        out.extend_from_slice(&self.total_code_units().to_le_bytes());
        for rec in &self.records {
            write_record(&mut out, rec);
        }
        match &self.tail {
            // Non-DNEC tail: append verbatim (no shipped file hits this path).
            Tail::Opaque(bytes) => out.extend_from_slice(bytes),
            Tail::Dnec(subs) => {
                // Sub-tables follow the directory in directory order; recompute absolute offsets.
                let dir_bytes = 4 + 4 + subs.len() * 8; // "DNEC" + group_count + pairs
                let mut off = out.len() + dir_bytes;
                let mut offsets = Vec::with_capacity(subs.len());
                for s in subs {
                    offsets.push(off as u32);
                    off += s.size();
                }
                out.extend_from_slice(MAGIC_DNEC);
                out.extend_from_slice(&(subs.len() as u32).to_le_bytes());
                for (s, &o) in subs.iter().zip(&offsets) {
                    out.extend_from_slice(&s.scene_hash.to_le_bytes());
                    out.extend_from_slice(&o.to_le_bytes());
                }
                for s in subs {
                    let tsize: u32 = s.records.iter().map(|r| r.text.len() as u32).sum();
                    out.extend_from_slice(&(s.records.len() as u32).to_le_bytes());
                    out.extend_from_slice(&tsize.to_le_bytes());
                    for rec in &s.records {
                        write_record(&mut out, rec);
                    }
                    out.extend_from_slice(MAGIC_DNEC); // 4-byte terminator
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal but structurally faithful file: 1 base UI record + a DNEC section with one
    /// scene group holding one VO record. The bytes come from `write()` of a hand-assembled model, so
    /// the test needs no game install.
    fn minimal() -> GameText {
        let base = Record { asset_id: 0x1111_1111, key: vec![0u8], text: encode_text("base") };
        let vo = Record { asset_id: 0x2222_2222, key: b"vo_a\0".to_vec(), text: encode_text("hi") };
        GameText {
            version: VERSION,
            records: vec![base],
            tail: Tail::Dnec(vec![SubTable { scene_hash: 0xABCD_0001, records: vec![vo] }]),
        }
    }

    #[test]
    fn dnec_roundtrips_and_parses() {
        let gt = minimal();
        let bytes = gt.write();
        let re = GameText::parse(&bytes).unwrap();
        assert_eq!(re.records.len(), 1);
        assert_eq!(re.dnec_groups().len(), 1);
        assert_eq!(re.dnec_record_count(), 1);
        assert_eq!(re.write(), bytes); // byte-identical round-trip through the structured tail
    }

    #[test]
    fn add_dnec_appends_into_scene() {
        let mut gt = minimal();
        gt.add_dnec(0xABCD_0001, 0x3333_3333, "vo_b", "second line").unwrap();
        // present in the model, and via the scene-scoped lookup
        assert_eq!(gt.dnec_record_count(), 2);
        assert_eq!(gt.find_any(0x3333_3333, Some(0xABCD_0001)).unwrap().text_string(), "second line");
        // survives a write→parse and the new record's tsize/offsets are consistent
        let re = GameText::parse(&gt.write()).unwrap();
        assert_eq!(re.dnec_groups()[0].records.len(), 2);
        assert_eq!(re.find_any(0x3333_3333, None).unwrap().key_str(), "vo_b");
    }

    #[test]
    fn add_dnec_rejects_unknown_scene_and_dupes() {
        let mut gt = minimal();
        assert!(gt.add_dnec(0xDEAD_BEEF, 0x4444, "vo_x", "x").is_err()); // no such scene
        assert!(gt.add_dnec(0xABCD_0001, 0x2222_2222, "vo_dup", "x").is_err()); // asset_id taken
    }
}
