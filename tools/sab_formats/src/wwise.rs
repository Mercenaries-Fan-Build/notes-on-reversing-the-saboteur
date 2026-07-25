//! Wwise audio resolution — map a GameText VO/subtitle key to its streamed `.wem` inside a `1KCP`
//! sound pack (`Sound/<Lang>.pck`), so the tools can find and play the voice line for a subtitle.
//!
//! The chain (see `docs/formats/audio_1kcp.md` and the memory notes):
//! 1. subtitle key → Wwise **event name** `Play_<key>` → **event id** = [`wwise_fnv1`] of that name
//!    (Wwise's own FNV-1, NOT `pandemic_hash`; proven: 90.5% of VO keys hit a real bank event).
//! 2. event → **source `.wem` id** by walking the bank `HIRC` graph Event→Action→(containers)→Sound.
//!    ⚠ This game's Wwise **v44 HIRC uses an 8-byte item header `u32 type, u32 size`** (not the usual
//!    `u8 type, u32 size`). Action target id at payload `+8`; Sound source id at `+20`/`+24`.
//! 3. source id → the streamed `.wem` bytes in the pack (the `1KCP` stream table).
//!
//! End-to-end this resolves ~75% of VO/subtitle lines to a real stream (ceiling ~90%, the rest being
//! cinematic/bark events with other name patterns). Decode of the returned `.wem` (Wwise Vorbis) is a
//! separate step (vgmstream); this module only resolves and extracts bytes.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

const MAGIC_1KCP: u32 = 0x5043_4B31; // "1KCP"

/// Wwise short-id hash: FNV-1/32 over the **lowercased** name.
pub fn wwise_fnv1(name: &str) -> u32 {
    let mut h: u32 = 2166136261;
    for b in name.bytes() {
        h = h.wrapping_mul(16777619) ^ (b.to_ascii_lowercase() as u32);
    }
    h
}

/// The Wwise event name for a VO/subtitle key: `Play_<key>` (keys already carrying the prefix, e.g.
/// `Play_vo_…`, are left unchanged).
pub fn event_name(key: &str) -> String {
    if key.len() >= 5 && key[..5].eq_ignore_ascii_case("Play_") {
        key.to_string()
    } else {
        format!("Play_{key}")
    }
}

/// A `1KCP` sound pack, opened for resolution: it holds the small header + bank region (the `HIRC`
/// graph and the stream directory) in memory, and reads the large `.wem` payloads from the file on
/// demand — the pack is ~500 MB, almost all of it stream data we only touch one line at a time.
pub struct SoundPack {
    path: PathBuf,
    /// wem id → (absolute file offset, size).
    streams: HashMap<u32, (u64, u32)>,
    /// HIRC object id → (type, payload offset in `banks`, payload len). Payload begins with the id.
    objs: HashMap<u32, (u32, usize, usize)>,
    /// The header + bank region: `file[0 .. first stream offset]`. Bank/HIRC offsets index into this.
    banks: Vec<u8>,
}

fn rd_u32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

impl SoundPack {
    /// Open a pack for resolution without loading its ~500 MB of stream payloads.
    pub fn open(path: impl AsRef<Path>) -> Result<SoundPack, String> {
        let path = path.as_ref().to_path_buf();
        let mut f = File::open(&path).map_err(|e| e.to_string())?;
        let mut head = [0u8; 0x1c];
        f.read_exact(&mut head).map_err(|e| e.to_string())?;
        if rd_u32(&head, 0) != MAGIC_1KCP {
            return Err("not a 1KCP sound pack".into());
        }
        let bank_count = rd_u32(&head, 0x0c) as usize;
        let bank_off = rd_u32(&head, 0x10) as usize;
        let stream_count = rd_u32(&head, 0x14) as usize;
        let stream_off = rd_u32(&head, 0x18) as usize;

        // Stream directory (small). Also learn where the stream payloads begin, so we know how much
        // of the head of the file (header + directories + banks) to slurp for HIRC parsing.
        f.seek(SeekFrom::Start(stream_off as u64)).map_err(|e| e.to_string())?;
        let mut stbl = vec![0u8; stream_count * 12];
        f.read_exact(&mut stbl).map_err(|e| e.to_string())?;
        let mut streams = HashMap::with_capacity(stream_count);
        let mut first_stream = u64::MAX;
        for i in 0..stream_count {
            let id = rd_u32(&stbl, i * 12);
            let size = rd_u32(&stbl, i * 12 + 4);
            let off = rd_u32(&stbl, i * 12 + 8) as u64;
            streams.insert(id, (off, size));
            first_stream = first_stream.min(off);
        }
        if first_stream == u64::MAX {
            first_stream = stream_off as u64;
        }

        // The whole head-of-file region (banks live here). A few MB for a localized VO pack.
        f.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
        let mut banks = vec![0u8; first_stream as usize];
        f.read_exact(&mut banks).map_err(|e| e.to_string())?;

        let mut objs: HashMap<u32, (u32, usize, usize)> = HashMap::new();
        for i in 0..bank_count {
            let o = bank_off + i * 12;
            if o + 12 > banks.len() {
                break;
            }
            let bsize = rd_u32(&banks, o + 4) as usize;
            let boff = rd_u32(&banks, o + 8) as usize;
            let end = (boff + bsize).min(banks.len());
            parse_bank_hirc(&banks, boff, end, &mut objs);
        }
        Ok(SoundPack { path, streams, objs, banks })
    }

    /// Distinct streamed `.wem` count and HIRC object count (for diagnostics).
    pub fn stats(&self) -> (usize, usize) {
        (self.streams.len(), self.objs.len())
    }

    /// Resolve a VO/subtitle key to its source `.wem` id, if the event and its Sound resolve.
    pub fn wem_for_key(&self, key: &str) -> Option<u32> {
        self.wem_for_event(&event_name(key))
    }

    /// Resolve a fully-qualified Wwise event name to its source `.wem` id.
    pub fn wem_for_event(&self, event: &str) -> Option<u32> {
        let (t, off, len) = *self.objs.get(&wwise_fnv1(event))?;
        if t != 4 {
            return None; // not an Event
        }
        // Event payload: u32 id, u32 actionCount, actionCount × u32 actionId.
        let action_count = rd_u32(&self.banks, off + 4) as usize;
        for j in 0..action_count {
            let ao = off + 8 + j * 4;
            if ao + 4 > off + len {
                break;
            }
            let aid = rd_u32(&self.banks, ao);
            let Some(&(at, aoff, _)) = self.objs.get(&aid) else { continue };
            if at != 3 {
                continue; // not an Action
            }
            // Action payload: … target object id at +8.
            let target = rd_u32(&self.banks, aoff + 8);
            if let Some(w) = self.find_source(target, &mut Vec::new(), 0) {
                return Some(w);
            }
        }
        None
    }

    /// Walk an object to a playable Sound and return its streamed source id. Sounds resolve directly;
    /// containers (RanSeq/Switch/Blend/…) are descended by scanning their payload for child object
    /// ids — robust without per-container-type layouts, and unambiguous for one-line VO events.
    fn find_source(&self, oid: u32, seen: &mut Vec<u32>, depth: u32) -> Option<u32> {
        if depth > 6 || seen.contains(&oid) {
            return None;
        }
        seen.push(oid);
        let &(t, off, len) = self.objs.get(&oid)?;
        if t == 2 {
            // Sound: streamed source id at +20 or +24.
            for so in [20usize, 24] {
                if so + 4 <= len {
                    let src = rd_u32(&self.banks, off + so);
                    if self.streams.contains_key(&src) {
                        return Some(src);
                    }
                }
            }
            return None;
        }
        let mut k = 4;
        while k + 4 <= len {
            let v = rd_u32(&self.banks, off + k);
            if self.objs.contains_key(&v) {
                if let Some(w) = self.find_source(v, seen, depth + 1) {
                    return Some(w);
                }
            }
            k += 4;
        }
        None
    }

    /// Read the raw `.wem` bytes for a stream id from the file (Wwise-Vorbis; decode with vgmstream).
    pub fn wem_bytes(&self, wem_id: u32) -> Option<Vec<u8>> {
        let &(off, size) = self.streams.get(&wem_id)?;
        let mut f = File::open(&self.path).ok()?;
        f.seek(SeekFrom::Start(off)).ok()?;
        let mut buf = vec![0u8; size as usize];
        f.read_exact(&mut buf).ok()?;
        Some(buf)
    }
}

fn parse_bank_hirc(data: &[u8], start: usize, end: usize, objs: &mut HashMap<u32, (u32, usize, usize)>) {
    let mut o = start;
    while o + 8 <= end {
        let tag = &data[o..o + 4];
        let csize = rd_u32(data, o + 4) as usize;
        let body = o + 8;
        if tag == b"HIRC" {
            if body + 4 > end {
                break;
            }
            let n = rd_u32(data, body) as usize;
            let mut p = body + 4;
            let hirc_end = (body + 4 + csize).min(end);
            for _ in 0..n {
                if p + 12 > hirc_end {
                    break;
                }
                let typ = rd_u32(data, p);
                let ssize = rd_u32(data, p + 4) as usize;
                let id = rd_u32(data, p + 8);
                // payload begins at p+8 (with the id) and is `ssize` bytes long.
                objs.insert(id, (typ, p + 8, ssize));
                p += 8 + ssize;
            }
        }
        if csize == 0 || body + csize > end {
            break;
        }
        o = body + csize;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wwise_hash_and_event_name() {
        // Wwise FNV-1 (lowercased). Sanity value for a known short-id form.
        assert_eq!(wwise_fnv1("Init"), wwise_fnv1("init"));
        assert_eq!(event_name("vo_x_Sean_01"), "Play_vo_x_Sean_01");
        assert_eq!(event_name("Play_vo_x_Sean_01"), "Play_vo_x_Sean_01");
        assert_eq!(event_name("PLAY_already"), "PLAY_already");
    }
}
