//! A 2008 Wwise sound bank (`BKHD` version 29, big-endian): the `HIRC` object list and the object
//! layouts a played line passes through. `docs/2008_audio_format.md`.
//!
//! ```text
//! HIRC body (CAkBankMgr::ProcessHircChunk @0x8289fee8)
//!   u32 count
//!   count × { u32 type, u32 size, payload[size] }     every payload starts with its u32 id
//!
//! Event payload, type 4 (CAkEvent::SetInitialValues @0x828b8ba8; must end exactly)
//!   u32 id, u32 count, u32 action id × count
//!
//! Action payload, type 3 (CAkAction::SetInitialValues @0x828b17b8)
//!   u32 id, u32 action type (CAkAction::Create @0x828b1580 keys on type & 0xFF000; 0x4000 = Play)
//!   u32 target id, 3 × i32 milliseconds (scaled to samples at 48 kHz), u32 params flag
//!   params flag != 0: the action class's own parameters follow (CAkActionPlay::SetActionParams
//!   @0x828cd920 for Play)
//!
//! Sound payload, type 2 (CAkSound::SetInitialValues @0x828b9310 → CAkBankMgr::LoadSource @0x8289e6b8)
//!   u32 id, u32 plugin id, u32 stream type, u32 × 2 audio format, u32 source id, u32 file id
//!   stream type != 1: u32 in-bank offset, u32 in-bank size
//!   u8 flag
//!   plugin id & 0xF == 2 or 5: u32 size, plugin parameters[size]
//!   node base parameters, then u16 × 3; must end exactly
//!
//! Node base parameters (CAkParameterNodeBase::SetNodeBaseParams @0x828c6e40; the four virtual
//! readers' order is the CAkSound / CAkRanSeqCntr vtables' +0x120, +0x11C, +0x124, +0x128 slots)
//!   FX       (CAkParameterNode::SetInitialFxParams @0x828c1a70)
//!            u8 override, u8 count; count != 0: u8 bypass bits,
//!            count × { u8 index, u32 fx id, u8 rendered, u32 size, bytes[size] }
//!   u32 bus id       (non-zero: the bus must already be loaded — "Master bus structure not loaded")
//!   u32 parent id    (non-zero and loaded: the parent adopts this node)
//!   u8 × 4
//!   initial  (CAkParameterNode::SetInitialParams @0x828c3468)       u32 × 13
//!   position (CAkParameterNode::SetPositioningParams @0x828c1d50)
//!            u8 has; has: i32, u32, u32, u8 is3d; is3d == 0: u8;
//!            is3d: u32 kind, u32, u8; kind 3: u8; kind 2: u32, u8, u32, u32 nv, 16 × nv, u32 ni, 8 × ni
//!   advanced (CAkParameterNode::SetAdvSettingsParams @0x828c2068)   u32, u8, u16, u32, u8, u8
//!   u32 state sync, u32 count, count × 9 bytes
//!   RTPC     (CAkParameterNodeBase::SetInitialRTPC @0x828c65f0)
//!            u32 count, count × { u32 × 5, u32 points, 12 × points }
//!
//! Random/sequence container payload, type 5 (CAkRanSeqCntr::SetInitialValues @0x828bb4b0; must end
//! exactly)
//!   u32 id, node base parameters, u16, 22 bytes of playback settings (u8 at +0x10 is the mode)
//!   u32 count, child id × count            (CAkParentNode::SetChildren @0x828bba38)
//!   u32 count, count × { u32 id, u8 weight-or-order byte }   (CAkRanSeqCntr::AddPlaylistItem)
//! ```
//!
//! `HIRC` types 10–13 go to the music engine's handler ("Music engine not initialized : Content can
//! not be loaded from bank" when it is absent); they are not read here.

use crate::bytes;

use super::cursor::Cursor;

pub const HIRC_SOUND: u32 = 2;
pub const HIRC_ACTION: u32 = 3;
pub const HIRC_EVENT: u32 = 4;
pub const HIRC_RAN_SEQ: u32 = 5;

/// `CAkAction::Create` selects the action class from these bits of the action type.
pub const ACTION_CLASS_MASK: u32 = 0xFF000;
pub const ACTION_CLASS_PLAY: u32 = 0x4000;

/// One `HIRC` object: its type, id, and payload range in the `HIRC` body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HircObject {
    pub kind: u32,
    pub id: u32,
    pub start: usize,
    pub len: usize,
}

/// Walk a `HIRC` body; the objects must end exactly at its end.
pub fn hirc_objects(body: &[u8]) -> Result<Vec<HircObject>, String> {
    let count = bytes::u32_be(body, 0, "HIRC count")? as usize;
    let mut out = Vec::with_capacity(count);
    let mut o = 4usize;
    for i in 0..count {
        let kind = bytes::u32_be(body, o, &format!("HIRC object {i} type"))?;
        let len = bytes::u32_be(body, o + 4, &format!("HIRC object {i} size"))? as usize;
        let start = o + 8;
        bytes::slice(body, start, len, &format!("HIRC object {i} (type {kind})"))?;
        let id = bytes::u32_be(body, start, &format!("HIRC object {i} id"))?;
        out.push(HircObject { kind, id, start, len });
        o = start + len;
    }
    if o != body.len() {
        return Err(format!("HIRC: {count} objects end at 0x{o:X}, the body is 0x{:X} bytes", body.len()));
    }
    Ok(out)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    pub id: u32,
    pub actions: Vec<u32>,
}

pub fn parse_event(p: &[u8]) -> Result<Event, String> {
    let id = bytes::u32_be(p, 0, "event id")?;
    let n = bytes::u32_be(p, 4, "event action count")? as usize;
    if p.len() != 8 + 4 * n {
        return Err(format!("event {id:08X}: {n} actions in a {}-byte payload", p.len()));
    }
    let actions = (0..n).map(|i| bytes::u32_be(p, 8 + 4 * i, "event action id")).collect::<Result<_, _>>()?;
    Ok(Event { id, actions })
}

/// The fields every action shares; `params` is the rest of the payload, the action class's own
/// parameters, present when the flag is non-zero.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Action<'a> {
    pub id: u32,
    pub action_type: u32,
    pub target: u32,
    pub times_ms: [i32; 3],
    pub params_flag: u32,
    pub params: &'a [u8],
}

impl Action<'_> {
    pub fn is_play(&self) -> bool {
        self.action_type & ACTION_CLASS_MASK == ACTION_CLASS_PLAY
    }
}

pub fn parse_action(p: &[u8]) -> Result<Action<'_>, String> {
    let w = |o: usize, f: &str| bytes::u32_be(p, o, f);
    let id = w(0, "action id")?;
    let params_flag = w(0x18, "action params flag")?;
    let params = &p[0x1C..];
    if params_flag == 0 && !params.is_empty() {
        return Err(format!("action {id:08X}: params flag 0 with {} bytes after it", params.len()));
    }
    Ok(Action {
        id,
        action_type: w(4, "action type")?,
        target: w(8, "action target")?,
        times_ms: [
            bytes::i32_be(p, 0x0C, "action time 0")?,
            bytes::i32_be(p, 0x10, "action time 1")?,
            bytes::i32_be(p, 0x14, "action time 2")?,
        ],
        params_flag,
        params,
    })
}

/// A sound's source, as `LoadSource` reads it; `end` is the payload offset the node base parameters
/// start at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SoundSource {
    pub id: u32,
    pub plugin_id: u32,
    pub stream_type: u32,
    pub format: [u32; 2],
    pub source_id: u32,
    pub file_id: u32,
    /// In-bank media offset and size; absent for stream type 1.
    pub in_bank: Option<(u32, u32)>,
    pub flag: u8,
    /// Plugin parameter bytes' range in the payload, for a plugin id with `& 0xF` of 2 or 5.
    pub plugin_params: Option<(usize, usize)>,
    pub end: usize,
}

pub fn parse_sound_source(p: &[u8]) -> Result<SoundSource, String> {
    let w = |o: usize, f: &str| bytes::u32_be(p, o, f);
    let id = w(0, "sound id")?;
    let plugin_id = w(4, "sound plugin id")?;
    let stream_type = w(8, "sound stream type")?;
    let mut o = 0x1C;
    let in_bank = if stream_type == 1 {
        None
    } else {
        o += 8;
        Some((w(0x1C, "sound in-bank offset")?, w(0x20, "sound in-bank size")?))
    };
    let flag = bytes::u8_at(p, o, "sound source flag")?;
    o += 1;
    let plugin_params = match plugin_id & 0xF {
        0 => None,
        1 if stream_type <= 2 => None,
        1 => return Err(format!("sound {id:08X}: codec plugin {plugin_id:08X} with stream type {stream_type}")),
        2 | 5 => {
            let n = w(o, "sound plugin parameter size")? as usize;
            bytes::slice(p, o + 4, n, "sound plugin parameters")?;
            let r = (o + 4, n);
            o += 4 + n;
            Some(r)
        }
        t => return Err(format!("sound {id:08X}: plugin {plugin_id:08X} of class {t}")),
    };
    Ok(SoundSource {
        id,
        plugin_id,
        stream_type,
        format: [w(0x0C, "sound format 0")?, w(0x10, "sound format 1")?],
        source_id: w(0x14, "sound source id")?,
        file_id: w(0x18, "sound file id")?,
        in_bank,
        flag,
        plugin_params,
        end: o,
    })
}

/// The node base parameters' two links; `end` is the payload offset after them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NodeBase {
    pub bus_id: u32,
    pub parent_id: u32,
    pub end: usize,
}

fn skip_n(c: &mut Cursor, n: u32, each: usize, field: &str) -> Result<(), String> {
    let len = (n as usize).checked_mul(each).ok_or(format!("{field}: count {n} overflows"))?;
    c.take(len, field).map(|_| ())
}

/// Read the node base parameters starting at `at` in an object payload.
pub fn parse_node_base(p: &[u8], at: usize) -> Result<NodeBase, String> {
    let mut c = Cursor { b: p, at };
    c.u8("FX override")?;
    let fx = c.u8("FX count")?;
    if fx != 0 {
        c.u8("FX bypass bits")?;
        for _ in 0..fx {
            c.take(6, "FX slot")?;
            let n = c.u32("FX parameter size")?;
            c.take(n as usize, "FX parameters")?;
        }
    }
    let bus_id = c.u32("bus id")?;
    let parent_id = c.u32("parent id")?;
    c.take(4, "node flags")?;
    c.take(13 * 4, "initial parameters")?;
    if c.u8("has positioning")? != 0 {
        c.take(12, "positioning")?;
        if c.u8("3D positioning")? == 0 {
            c.u8("2D positioning")?;
        } else {
            let kind = c.u32("3D kind")?;
            c.take(5, "3D parameters")?;
            match kind {
                3 => {
                    c.u8("3D kind 3 flag")?;
                }
                2 => {
                    c.take(9, "3D path settings")?;
                    let nv = c.u32("path vertex count")?;
                    skip_n(&mut c, nv, 16, "path vertices")?;
                    let ni = c.u32("path list count")?;
                    skip_n(&mut c, ni, 8, "path list")?;
                }
                _ => {}
            }
        }
    }
    c.take(13, "advanced settings")?;
    c.u32("state sync")?;
    let states = c.u32("state count")?;
    skip_n(&mut c, states, 9, "states")?;
    let rtpcs = c.u32("RTPC count")?;
    for _ in 0..rtpcs {
        c.take(20, "RTPC")?;
        let pts = c.u32("RTPC point count")?;
        skip_n(&mut c, pts, 12, "RTPC points")?;
    }
    Ok(NodeBase { bus_id, parent_id, end: c.at })
}

/// A sound object: its source, node base, and the three closing u16 values.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sound {
    pub source: SoundSource,
    pub node: NodeBase,
    pub tail: [u16; 3],
}

pub fn parse_sound(p: &[u8]) -> Result<Sound, String> {
    let source = parse_sound_source(p)?;
    let node = parse_node_base(p, source.end)?;
    let mut c = Cursor { b: p, at: node.end };
    let tail = [c.u16("sound tail 0")?, c.u16("sound tail 1")?, c.u16("sound tail 2")?];
    if c.at != p.len() {
        return Err(format!("sound {:08X}: ends at 0x{:X} of a 0x{:X}-byte payload", source.id, c.at, p.len()));
    }
    Ok(Sound { source, node, tail })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RanSeq {
    pub id: u32,
    pub node: NodeBase,
    pub mode: u8,
    pub children: Vec<u32>,
    /// Playlist entries: id and the byte stored with it.
    pub playlist: Vec<(u32, u8)>,
}

pub fn parse_ran_seq(p: &[u8]) -> Result<RanSeq, String> {
    let id = bytes::u32_be(p, 0, "container id")?;
    let node = parse_node_base(p, 4)?;
    let mut c = Cursor { b: p, at: node.end };
    c.u16("container u16")?;
    let settings = c.take(22, "container playback settings")?;
    let n = c.u32("child count")?;
    let children = (0..n).map(|_| c.u32("child id")).collect::<Result<Vec<_>, _>>()?;
    let n = c.u32("playlist count")?;
    let mut playlist = Vec::new();
    for _ in 0..n {
        playlist.push((c.u32("playlist id")?, c.u8("playlist byte")?));
    }
    if c.at != p.len() {
        return Err(format!("container {id:08X}: ends at 0x{:X} of a 0x{:X}-byte payload", c.at, p.len()));
    }
    Ok(RanSeq { id, node, mode: settings[0x10], children, playlist })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn be(v: &[u32]) -> Vec<u8> {
        v.iter().flat_map(|x| x.to_be_bytes()).collect()
    }

    #[test]
    fn hirc_walk_and_event() {
        let ev = be(&[0x11, 2, 0x21, 0x22]);
        let mut body = be(&[1, HIRC_EVENT, ev.len() as u32]);
        body.extend(&ev);
        let objs = hirc_objects(&body).unwrap();
        assert_eq!((objs[0].kind, objs[0].id, objs[0].start, objs[0].len), (HIRC_EVENT, 0x11, 12, 16));
        assert_eq!(parse_event(&ev).unwrap().actions, vec![0x21, 0x22]);
        assert!(hirc_objects(&body[..body.len() - 1]).is_err());
        assert!(parse_event(&ev[..12]).is_err());
    }

    #[test]
    fn action_and_streamed_sound() {
        let mut a = be(&[0x21, 0x4011, 0x31, 0, 0, 0, 1]);
        a.extend([9u8; 5]);
        let act = parse_action(&a).unwrap();
        assert!(act.is_play());
        assert_eq!((act.target, act.params.len()), (0x31, 5));
        assert!(!parse_action(&be(&[0x21, 0x1011, 0x31, 0, 0, 0, 0])).unwrap().is_play());
        assert!(parse_action(&be(&[0x21, 0x4011, 0x31, 0, 0, 0, 0, 7])).is_err());

        let mut s = be(&[0x31, 0x0003_0001, 1, 0, 0, 0x41, 0x51]);
        s.push(0);
        let src = parse_sound_source(&s).unwrap();
        assert_eq!((src.file_id, src.in_bank, src.end), (0x51, None, 0x1D));
        let mut s0 = be(&[0x31, 0x0003_0001, 0, 0, 0, 0x41, 0x51, 0x100, 0x80]);
        s0.push(1);
        assert_eq!(parse_sound_source(&s0).unwrap().in_bank, Some((0x100, 0x80)));
    }
}
