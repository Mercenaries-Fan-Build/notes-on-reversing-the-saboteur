//! A voice line in the 2008 package (`sound/Saboteur.pck`): the `GameText` key's event
//! `Play_<key>` (Wwise FNV-1, [`crate::wwise::event_name`]) → its one Play action → the sound it
//! targets, or the first playlist entry of a random/sequence container it targets → that sound's
//! media: a package stream, or a `DIDX` entry of the sound's own bank. `docs/2008_audio_format.md`
//! ("A voice line, end to end").
//!
//! Banks that share an object may define it differently (a sound's in-bank media offset, an action's
//! parameters, a container's children), so a chain is read inside one bank: an object the event's
//! bank defines is taken from that bank, and one it does not define must be defined alike by every
//! bank that has it.
//!
//! The banks are held in memory; stream records are read from the file when asked for.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use super::sound_bank::{self, HircObject};
use super::sound_pack::{self, Record};
use crate::wwise::{event_name, wwise_fnv1};

/// Where a sound's media is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaRef {
    /// A package stream record, by id.
    Stream(u32),
    /// A `DIDX` entry of a bank.
    Bank { bank: u32, media: u32 },
}

/// A resolved voice line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Voice {
    /// The bank whose definitions the chain was read from (one that defines the event).
    pub bank: u32,
    pub event: u32,
    pub sound: u32,
    pub media: MediaRef,
    /// When the Play action targets a random/sequence container: the container and its playlist
    /// size (the first entry is the one resolved).
    pub container: Option<(u32, usize)>,
}

struct Bank {
    bytes: Vec<u8>,
    /// `DIDX` media id → its range in `bytes`.
    media: HashMap<u32, (usize, usize)>,
}

pub struct VoicePack {
    path: PathBuf,
    streams: HashMap<u32, Record>,
    banks: HashMap<u32, Bank>,
    /// Object id → every (bank id, object) defining it; payload ranges index the bank's bytes.
    objs: HashMap<u32, Vec<(u32, HircObject)>>,
}

fn read_at(f: &mut File, offset: u64, len: usize, path: &Path) -> Result<Vec<u8>, String> {
    let mut b = vec![0u8; len];
    f.seek(SeekFrom::Start(offset)).and_then(|_| f.read_exact(&mut b)).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(b)
}

impl VoicePack {
    /// Open the package: its tables and every bank.
    pub fn open(path: impl AsRef<Path>) -> Result<VoicePack, String> {
        let path = path.as_ref().to_path_buf();
        let mut f = File::open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let file_len = f.metadata().map_err(|e| format!("{}: {e}", path.display()))?.len();
        let head = read_at(&mut f, 0, sound_pack::HEADER_LEN, &path)?;
        let h = sound_pack::parse_header(&head).map_err(|e| format!("{}: {e}", path.display()))?;
        let end = [(h.bank_table, h.bank_count), (h.stream_table, h.stream_count)]
            .iter()
            .map(|&(o, n)| o as usize + n as usize * sound_pack::RECORD_LEN)
            .max()
            .expect("two tables");
        let prefix = read_at(&mut f, 0, end, &path)?;
        let p = sound_pack::parse(&prefix, file_len).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut banks = HashMap::new();
        let mut objs: HashMap<u32, Vec<(u32, HircObject)>> = HashMap::new();
        for r in &p.banks {
            let bytes = read_at(&mut f, r.offset as u64, r.size as usize, &path)?;
            let at = format!("{}: bank {:08X}", path.display(), r.id);
            let cs = sound_pack::chunks(&bytes).map_err(|e| format!("{at}: {e}"))?;
            let find = |t: &[u8; 4]| cs.iter().find(|c| &c.tag == t).copied();
            if let Some(c) = find(b"HIRC") {
                for mut o in sound_bank::hirc_objects(&bytes[c.start..c.start + c.len]).map_err(|e| format!("{at}: {e}"))? {
                    o.start += c.start;
                    objs.entry(o.id).or_default().push((r.id, o));
                }
            }
            let mut media = HashMap::new();
            if let (Some(idx), Some(data)) = (find(b"DIDX"), find(b"DATA")) {
                if idx.len % 12 != 0 {
                    return Err(format!("{at}: DIDX of {} bytes", idx.len));
                }
                for e in bytes[idx.start..idx.start + idx.len].chunks(12) {
                    let w = |i: usize| u32::from_be_bytes(e[4 * i..4 * i + 4].try_into().expect("4 bytes"));
                    let (off, len) = (w(1) as usize, w(2) as usize);
                    if off + len > data.len {
                        return Err(format!("{at}: media {:08X} past DATA", w(0)));
                    }
                    media.insert(w(0), (data.start + off, len));
                }
            }
            banks.insert(r.id, Bank { bytes, media });
        }
        let streams = p.streams.iter().map(|r| (r.id, *r)).collect();
        Ok(VoicePack { path, streams, banks, objs })
    }

    fn payload(&self, bank: u32, o: &HircObject) -> &[u8] {
        &self.banks[&bank].bytes[o.start..o.start + o.len]
    }

    /// The object `id` as read from `bank`: that bank's definition, else the one every defining bank
    /// agrees on.
    fn object_in(&self, bank: u32, id: u32) -> Result<Option<(u32, HircObject, &[u8])>, String> {
        let Some(defs) = self.objs.get(&id) else { return Ok(None) };
        if let Some((b, o)) = defs.iter().find(|(b, _)| *b == bank) {
            return Ok(Some((*b, *o, self.payload(*b, o))));
        }
        let first = self.payload(defs[0].0, &defs[0].1);
        if let Some(other) = defs[1..].iter().find(|(b, o)| self.payload(*b, o) != first) {
            return Err(format!(
                "object {id:08X}, read from bank {bank:08X} which lacks it, is defined differently in banks {:08X} and {:08X}",
                defs[0].0, other.0
            ));
        }
        Ok(Some((defs[0].0, defs[0].1, first)))
    }

    fn sound_media(&self, bank: u32, id: u32) -> Result<MediaRef, String> {
        let (bank, o, pl) = self.object_in(bank, id)?.ok_or(format!("sound {id:08X} is in no bank"))?;
        if o.kind != sound_bank::HIRC_SOUND {
            return Err(format!("object {id:08X} is HIRC type {}, not a sound", o.kind));
        }
        let s = sound_bank::parse_sound(pl)?.source;
        if s.plugin_id & 0xF != 1 {
            return Err(format!("sound {id:08X} is plugin {:08X}, which has no media", s.plugin_id));
        }
        match s.stream_type {
            0 => {
                if !self.banks[&bank].media.contains_key(&s.source_id) {
                    return Err(format!("sound {id:08X}: media {:08X} is not in its bank {bank:08X}", s.source_id));
                }
                Ok(MediaRef::Bank { bank, media: s.source_id })
            }
            1 | 2 => {
                if !self.streams.contains_key(&s.file_id) {
                    return Err(format!("sound {id:08X}: stream {:08X} is not in the package", s.file_id));
                }
                Ok(MediaRef::Stream(s.file_id))
            }
            t => Err(format!("sound {id:08X}: stream type {t}")),
        }
    }

    /// Resolve a `GameText` key, read from the first bank (in table order) that defines its event;
    /// `Ok(None)` when no object is named `Play_<key>`.
    pub fn resolve(&self, key: &str) -> Result<Option<Voice>, String> {
        Ok(self.resolve_in_each_bank(key)?.into_iter().next())
    }

    /// Resolve a `GameText` key once per bank that defines its event, in table order.
    pub fn resolve_in_each_bank(&self, key: &str) -> Result<Vec<Voice>, String> {
        let event = wwise_fnv1(&event_name(key));
        let Some(defs) = self.objs.get(&event) else { return Ok(Vec::new()) };
        defs.iter().map(|(bank, _)| self.resolve_from(*bank, key, event)).collect()
    }

    fn resolve_from(&self, bank: u32, key: &str, event: u32) -> Result<Voice, String> {
        let (_, o, pl) = self.object_in(bank, event)?.expect("the event's own bank defines it");
        if o.kind != sound_bank::HIRC_EVENT {
            return Err(format!("Play_{key} ({event:08X}) is HIRC type {}, not an event", o.kind));
        }
        let mut plays = Vec::new();
        for aid in sound_bank::parse_event(pl)?.actions {
            let (_, ao, apl) = self.object_in(bank, aid)?.ok_or(format!("action {aid:08X} is in no bank"))?;
            if ao.kind != sound_bank::HIRC_ACTION {
                return Err(format!("event {event:08X} lists {aid:08X}, HIRC type {}", ao.kind));
            }
            let a = sound_bank::parse_action(apl)?;
            if a.is_play() {
                plays.push(a.target);
            }
        }
        let [target] = plays[..] else {
            return Err(format!("event {event:08X} has {} Play actions", plays.len()));
        };
        let (_, to, tpl) = self.object_in(bank, target)?.ok_or(format!("Play target {target:08X} is in no bank"))?;
        let (sound, container) = match to.kind {
            sound_bank::HIRC_SOUND => (target, None),
            sound_bank::HIRC_RAN_SEQ => {
                let r = sound_bank::parse_ran_seq(tpl)?;
                let first = r.playlist.first().map(|e| e.0).ok_or(format!("container {target:08X} has an empty playlist"))?;
                (first, Some((target, r.playlist.len())))
            }
            k => return Err(format!("Play target {target:08X} is HIRC type {k}")),
        };
        Ok(Voice { bank, event, sound, media: self.sound_media(bank, sound)?, container })
    }

    /// The media's bytes: a whole `RIFX` file.
    pub fn media(&self, m: MediaRef) -> Result<Vec<u8>, String> {
        match m {
            MediaRef::Stream(id) => {
                let r = self.streams.get(&id).ok_or(format!("stream {id:08X} is not in the package"))?;
                let mut f = File::open(&self.path).map_err(|e| format!("{}: {e}", self.path.display()))?;
                read_at(&mut f, r.offset as u64, r.size as usize, &self.path)
            }
            MediaRef::Bank { bank, media } => {
                let b = self.banks.get(&bank).ok_or(format!("bank {bank:08X} is not in the package"))?;
                let &(start, len) = b.media.get(&media).ok_or(format!("media {media:08X} is not in bank {bank:08X}"))?;
                Ok(b.bytes[start..start + len].to_vec())
            }
        }
    }
}
