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
//!
//! The 2008 Xbox 360 build's `GameText.dlg` is version 3, read by `WSTextManager::LoadTextFiles`
//! (WildStar_d @0x82a3bfc8, WildStar_p @0x826350c0): every word big-endian, records tagged `DTXT`
//! (`TXTD` read as a big-endian word), each text `str_len` **bytes** including its NUL, and the record
//! list closed by `CEND` instead of a DNEC section.
//!
//! ```text
//! Header (12 bytes, big-endian): u32 version=3, u32 record_count, u32 total_text_bytes
//! record_count × { "DTXT", u32 asset_id, u16 key_len(incl NUL), key[key_len], u16 str_len(bytes),
//!                  u8 str[str_len] (NUL-terminated) }
//! "CEND"
//! ```

use crate::pandemic_hash;

const MAGIC_REC: &[u8; 4] = b"TXTD";
const MAGIC_DNEC: &[u8; 4] = b"DNEC";
pub const VERSION: u32 = 5;
/// The 2008 Xbox 360 build's revision.
pub const VERSION_2008: u32 = 3;
const MAGIC_REC_2008: &[u8; 4] = b"DTXT";
const MAGIC_END_2008: &[u8; 4] = b"CEND";

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
    /// The speaking character/voice for this record's VO key, canonicalized for grouping; `None`
    /// for UI text. See [`speaker`].
    pub fn speaker(&self) -> Option<String> {
        speaker(&self.key_str())
    }
    fn size(&self) -> usize {
        4 + 4 + 2 + self.key.len() + 2 + self.text.len() * 2
    }
}

// ------------------------------------------------------------------ speaker (from the VO key)
//
// VO/subtitle keys encode who is speaking, under two naming grammars observed across all six retail
// files (8459 keyed records, 100% coverage):
//   * scripted dialogue  `vo_<cat>_<scene…>_<Speaker>_<idx>`  — speaker is the token BEFORE the
//     trailing line index (`vo_mis_A1M0_..._Javier_01`); `vo_gen_Death_<Speaker>` has no index so the
//     speaker is the last token.
//   * combat/ambient barks `<Speaker>_<CAT>_<action>`         — speaker is the FIRST token
//     (`Adler_Spo_CarCrash`, `MNS1_mis_…`). A leading `Play_` wrapper is stripped first.
// This is a naming CONVENTION, not a byte-format fact — a mislabel is cosmetic, never a parse error.

/// Does `s` look like a trailing line index — `^[A-Za-z]{0,6}\d+[A-Za-z]?$` (e.g. `01`, `Hury01`,
/// `11c`)? Used to find the speaker token in the scripted-dialogue grammar.
fn is_line_index(s: &str) -> bool {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() && b[i].is_ascii_alphabetic() && i < 6 {
        i += 1;
    }
    let digit_start = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    if i == digit_start {
        return false; // no digits
    }
    if i < b.len() && b[i].is_ascii_alphabetic() {
        i += 1; // one optional trailing letter
    }
    i == b.len()
}

/// Canonicalize a raw speaker token for grouping: collapse a numbered archetype voice to its base
/// (`MNS1..6` → `MNS`, `drsgrl1` → `drsgrl`) then merge known spelling/typo variants.
fn canon_speaker(raw: &str) -> String {
    let base = raw.trim_end_matches(|c: char| c.is_ascii_digit());
    let base = if base.is_empty() { raw } else { base };
    match base.to_ascii_lowercase().as_str() {
        "annoucer" | "announcer" => "Announcer".into(),
        "juless" | "jules" => "Jules".into(),
        "minto" | "mingo" => "Mingo".into(),
        "lecrochet" | "lecrochete" | "crochet" => "LeCrochet".into(),
        "dorisgirl" | "dorrisgirl" | "dorisgirls" | "drsgrl" => "Doris's Girls".into(),
        "prost" => "Prostitute".into(),
        _ => base.to_string(),
    }
}

/// The speaking character/voice for a VO or subtitle key, canonicalized for grouping (named cast
/// kept individual; numbered archetype voices collapsed; typo variants merged). `None` for UI text
/// (empty key) or a key with no recoverable speaker.
pub fn speaker(key: &str) -> Option<String> {
    if key.is_empty() {
        return None;
    }
    let k = key.strip_prefix("Play_").unwrap_or(key);
    let toks: Vec<&str> = k.split('_').collect();
    let raw = if toks.first() == Some(&"vo") {
        let t = &toks[1..];
        if t.len() < 2 {
            return None;
        }
        if is_line_index(t[t.len() - 1]) {
            t[t.len() - 2]
        } else {
            t[t.len() - 1]
        }
    } else if toks.len() >= 2 {
        toks[0]
    } else {
        return None;
    };
    Some(canon_speaker(raw))
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
    /// Version 3: the `CEND` that closes the record list.
    End,
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

/// Unicode for bytes 0x80..=0x9F of Windows-1252, the code page of version 3 text: the English file's
/// `0x85` closes "I see you, Irish…", `0x92` is the apostrophe of "Chippin’", `0x93`/`0x94` quote
/// "uncle" and `0x96` is a dash. Bytes 0xA0..=0xFF are the same code points (`0xE9` in "Résistants").
/// The five bytes 1252 leaves undefined map to their C1 code points.
const CP1252_80: [u16; 32] = [
    0x20AC, 0x0081, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160, 0x2039, 0x0152, 0x008D, 0x017D, 0x008F,
    0x0090, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014, 0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0x009D, 0x017E, 0x0178,
];

fn from_cp1252(c: u8) -> u16 {
    match c {
        0x80..=0x9F => CP1252_80[(c - 0x80) as usize],
        _ => c as u16,
    }
}

fn to_cp1252(u: u16) -> Option<u8> {
    match u {
        0..=0x7F | 0xA0..=0xFF => Some(u as u8),
        _ => CP1252_80.iter().position(|&x| x == u).map(|i| 0x80 + i as u8),
    }
}

/// Version 3: the big-endian header, `DTXT` records with Windows-1252 text, then `CEND`.
fn parse_2008(b: &[u8]) -> Result<GameText, String> {
    let be32 = |o: usize| b.get(o..o + 4).map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]])).ok_or_else(|| format!("EOF u32 @{o}"));
    let be16 = |o: usize| b.get(o..o + 2).map(|s| u16::from_be_bytes([s[0], s[1]])).ok_or_else(|| format!("EOF u16 @{o}"));
    let count = be32(4)?;
    let total = be32(8)?;
    let mut o = 12;
    let mut records = Vec::with_capacity(count as usize);
    for i in 0..count {
        if b.get(o..o + 4) != Some(MAGIC_REC_2008.as_slice()) {
            return Err(format!("record {i}: expected DTXT at {o}"));
        }
        let asset_id = be32(o + 4)?;
        let key_len = be16(o + 8)? as usize;
        let key = b.get(o + 10..o + 10 + key_len).ok_or_else(|| format!("record {i}: EOF key"))?.to_vec();
        o += 10 + key_len;
        let str_len = be16(o)? as usize;
        let text = b.get(o + 2..o + 2 + str_len).ok_or_else(|| format!("record {i}: EOF text"))?.iter().map(|&c| from_cp1252(c)).collect();
        o += 2 + str_len;
        records.push(Record { asset_id, key, text });
    }
    if b.get(o..o + 4) != Some(MAGIC_END_2008.as_slice()) {
        return Err(format!("expected CEND after {count} records at {o}"));
    }
    if o + 4 != b.len() {
        return Err(format!("{} bytes after CEND", b.len() - o - 4));
    }
    let gt = GameText { version: VERSION_2008, records, tail: Tail::End };
    if gt.total_code_units() != total {
        return Err(format!("header says {total} text bytes, the records hold {}", gt.total_code_units()));
    }
    Ok(gt)
}

impl GameText {
    /// Version 5 (retail PC) or version 3 (the 2008 Xbox 360 build), told apart by the first word.
    pub fn parse(b: &[u8]) -> Result<GameText, String> {
        let version = rd_u32(b, 0)?;
        if b.get(0..4) == Some(VERSION_2008.to_be_bytes().as_slice()) {
            return parse_2008(b);
        }
        if version != VERSION {
            return Err(format!("unexpected GameText version {version}, expected {VERSION} (or {VERSION_2008} big-endian)"));
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
            Tail::Opaque(_) | Tail::End => &[],
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
            Tail::Opaque(_) | Tail::End => return Err("file has no DNEC section".into()),
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
    ///
    /// A version 3 file is written in its own layout. Its text is Windows-1252, so a text holding a
    /// character outside that code page cannot be written and panics naming the record.
    pub fn write(&self) -> Vec<u8> {
        if matches!(self.tail, Tail::End) {
            let mut out = Vec::new();
            out.extend_from_slice(&VERSION_2008.to_be_bytes());
            out.extend_from_slice(&(self.records.len() as u32).to_be_bytes());
            out.extend_from_slice(&self.total_code_units().to_be_bytes());
            for r in &self.records {
                out.extend_from_slice(MAGIC_REC_2008);
                out.extend_from_slice(&r.asset_id.to_be_bytes());
                out.extend_from_slice(&(r.key.len() as u16).to_be_bytes());
                out.extend_from_slice(&r.key);
                out.extend_from_slice(&(r.text.len() as u16).to_be_bytes());
                for &u in &r.text {
                    let c = to_cp1252(u)
                        .unwrap_or_else(|| panic!("record 0x{:08x}: U+{u:04X} is not in Windows-1252, the version 3 text code page", r.asset_id));
                    out.push(c);
                }
            }
            out.extend_from_slice(MAGIC_END_2008);
            return out;
        }
        let mut out = Vec::with_capacity(self.base_len());
        out.extend_from_slice(&self.version.to_le_bytes());
        out.extend_from_slice(&(self.records.len() as u32).to_le_bytes());
        out.extend_from_slice(&self.total_code_units().to_le_bytes());
        for rec in &self.records {
            write_record(&mut out, rec);
        }
        match &self.tail {
            Tail::End => unreachable!("version 3 is written above"),
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
    fn speaker_from_key() {
        // scripted dialogue: speaker before the trailing index
        assert_eq!(speaker("vo_mis_A1M0_Race_To_Saar_End_Lose_Javier_01").as_deref(), Some("Javier"));
        assert_eq!(speaker("vo_cin_404_CinB_CataOut_Sean_Hury01").as_deref(), Some("Sean"));
        // no-index death barks: speaker is the last token
        assert_eq!(speaker("vo_gen_Death_Bishop").as_deref(), Some("Bishop"));
        // combat/ambient barks: speaker is the first token
        assert_eq!(speaker("Adler_Spo_CarCrash").as_deref(), Some("Adler"));
        assert_eq!(speaker("Play_vo_mis_A1M4_OverhearTorture_Victim_01").as_deref(), Some("Victim"));
        // archetype collapse + typo merge
        assert_eq!(speaker("MNS4_mis_grntgen_high_02").as_deref(), Some("MNS"));
        assert_eq!(speaker("vo_x_scene_Annoucer_03").as_deref(), Some("Announcer"));
        assert_eq!(speaker("drsgrl1_mis_grntgen_high_01").as_deref(), Some("Doris's Girls"));
        // UI text has no speaker
        assert_eq!(speaker(""), None);
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

    /// A version 3 file: big-endian header, `DTXT` records with byte text, `CEND`.
    #[test]
    fn version_3_parses_and_round_trips() {
        let mut b = Vec::new();
        b.extend_from_slice(&3u32.to_be_bytes());
        b.extend_from_slice(&2u32.to_be_bytes());
        b.extend_from_slice(&(6u32 + 3).to_be_bytes());
        for (id, key, text) in [(0x897A_3333u32, &b"vo_a\0"[..], &b"Hell\x92\0"[..]), (0x0000_0042, &b"\0"[..], &b"\xE9k\0"[..])] {
            b.extend_from_slice(b"DTXT");
            b.extend_from_slice(&id.to_be_bytes());
            b.extend_from_slice(&(key.len() as u16).to_be_bytes());
            b.extend_from_slice(key);
            b.extend_from_slice(&(text.len() as u16).to_be_bytes());
            b.extend_from_slice(text);
        }
        b.extend_from_slice(b"CEND");
        let gt = GameText::parse(&b).unwrap();
        assert_eq!(gt.version, VERSION_2008);
        assert_eq!(gt.records.len(), 2);
        assert_eq!((gt.records[0].key_str().as_str(), gt.records[0].text_string().as_str()), ("vo_a", "Hell\u{2019}"));
        assert!(gt.records[1].is_ui());
        assert_eq!(gt.records[1].text_string(), "\u{e9}k");
        assert!(gt.dnec_groups().is_empty());
        assert_eq!(gt.write(), b);
        let mut short = b.clone();
        short.truncate(b.len() - 1);
        assert!(GameText::parse(&short).is_err());
    }

    #[test]
    fn add_dnec_rejects_unknown_scene_and_dupes() {
        let mut gt = minimal();
        assert!(gt.add_dnec(0xDEAD_BEEF, 0x4444, "vo_x", "x").is_err()); // no such scene
        assert!(gt.add_dnec(0xABCD_0001, 0x2222_2222, "vo_dup", "x").is_err()); // asset_id taken
    }
}
