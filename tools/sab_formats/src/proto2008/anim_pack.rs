//! `Animations.pack` (`AP04` `ANIM`), read in the order of `PclAnimationManager::LoadAnimPackFile`
//! @0x8319ebf8 (WildStar_d): a count of animation records, then the blend bone exception and the
//! tagged sections `INTV`, `SEQC`, `TRAN`, `EDGE`, `DIST`, `BANK`, `ADD0`, `ALPH`, each of which the
//! loader requires in that order. There is no directory: each record carries the byte size of its
//! clip and the loader reads straight through.

use super::anim::{self, Clip};
use super::cursor::Cursor;

pub const MAGIC: &[u8; 8] = b"AP04ANIM";
/// The `INTV` interval type the loader accepts (`AddUnInterruptibleInterval`); any other asserts.
pub const UNINTERRUPTIBLE: u32 = 0x501B_960D;
/// `PclTransition` type names by value (`GenerateDebugString` @0x831c25f8).
pub const TRANSITION_TYPES: [&str; 5] = ["SET", "FADE", "QUEUE", "SYNC", "QUEUE_FADE"];
/// Bytes of the two `EDGE` blocks the loader copies into `PclEdgeTransData`, and of the `DIST` block.
pub const EDGE_LEN: [usize; 2] = [0x1F80, 0x360];
pub const DIST_LEN: usize = 0x60;

/// An animation event (`PclAnimEvent`).
#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    pub frame: i32,
    /// Event name CRC.
    pub name: u32,
    /// Four (parameter name CRC, value) pairs.
    pub params: [(u32, f32); 4],
}

/// One animation: its clip and the `PclAnimation` members read after it.
#[derive(Clone, Debug, PartialEq)]
pub struct Record {
    /// `pandemic_hash` of the animation's name (the `PblCRC` `LoadAnimation` registers it under).
    pub key: u32,
    /// Nonzero: an object animation, which keeps its bone CRCs; zero: a character animation, whose
    /// tracks bind through `PclSkeleton::FindBoneIndex`.
    pub object: u8,
    /// Bytes of the clip; the loader asserts the clip ends there.
    pub size: i32,
    /// Offset of the clip's `ANM0` word in the pack.
    pub clip_at: usize,
    pub clip: Clip,
    /// `GetAnimationMotionLength`, `PclAnimation` +0x20.
    pub motion_length: f32,
    /// `GetAnimationMovement`, +0x24.
    pub movement: [f32; 3],
    /// `GetAnimationRotation`, +0x30.
    pub rotation: f32,
    /// `GetStartAimAngle`, +0x34.
    pub start_aim: f32,
    /// `GetEndAimAngle`, +0x38.
    pub end_aim: f32,
    /// `GetAimPitch`, +0x3C.
    pub aim_pitch: f32,
    /// Nonzero sets `GetAnimationTurnPositive` (bit 7 of +0x7C).
    pub turn_positive: u8,
    pub events: Vec<Event>,
    /// `PclAnimOffset`s: a vector and a value.
    pub offsets: Vec<([f32; 3], f32)>,
}

/// A `PclAnimInterval` attached to each listed animation.
#[derive(Clone, Debug, PartialEq)]
pub struct Interval {
    /// Interval type CRC; the loader handles [`UNINTERRUPTIBLE`] only.
    pub kind: u32,
    /// The two 16-bit values stored at `PclAnimInterval` +0x0C and +0x0E.
    pub values: [i16; 2],
    /// Record keys of the animations it applies to.
    pub animations: Vec<u32>,
}

/// A `PclAnimationSequence::Instruction`.
#[derive(Clone, Debug, PartialEq)]
pub struct Instruction {
    /// Stored at `Instruction` +0x00 (a transition's inline sequence sets it to 1).
    pub word0: u32,
    /// Stored at `Instruction` +0x04.
    pub word1: i32,
    /// `AddAnimation`: record keys.
    pub animations: Vec<u32>,
    /// `AddTag`: tag CRCs.
    pub tags: Vec<u32>,
}

/// The `PclAnimationSequence` members read after its instructions.
#[derive(Clone, Debug, PartialEq)]
pub struct SequenceBody {
    /// `SetAnimationLength`.
    pub length: f32,
    /// `SetAnimationMovement`.
    pub movement: [f32; 3],
    /// `SetAnimationRotation`.
    pub rotation: f32,
    /// `SetType` (`InstructionType`).
    pub kind: u32,
    /// `GetAimMinMax`, 16 bytes.
    pub aim_min_max: [u8; 16],
    /// `GetAimFrames`: three lists of 8-byte `PblVector2`s, counted by three signed bytes.
    pub aim_frames: [Vec<[u8; 8]>; 3],
    /// Nonzero: `SetTurnPositive(true)`.
    pub turn_positive: u8,
    /// Nonzero: `SetSynchronized(true)`.
    pub synchronized: u8,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Sequence {
    pub name: u32,
    pub instructions: Vec<Instruction>,
    pub body: SequenceBody,
}

/// The one-instruction sequence a transition can carry.
#[derive(Clone, Debug, PartialEq)]
pub struct InlineSequence {
    pub name: u32,
    /// The animation of its instruction.
    pub animation: u32,
    /// The instruction's tag CRC, 0 for none.
    pub tag: u32,
    pub body: SequenceBody,
}

/// A `PclTransition`. `Match` @0x831c1d18 applies it from a sequence whose name is `from_sequence`
/// to one whose name is `to_sequence` (either may be `pandemic_hash("ANY")`), when the current
/// instruction carries `from_tag` and the target sequence `to_tag` (0 = no condition), and the time
/// is at least `min_time`.
#[derive(Clone, Debug, PartialEq)]
pub struct Transition {
    pub from_sequence: u32,
    pub from_tag: u32,
    pub to_sequence: u32,
    pub to_tag: u32,
    /// +0x10.
    pub min_time: f32,
    /// +0x14: the type of the transition into the inline sequence ([`TRANSITION_TYPES`]).
    pub inline_type: u32,
    /// +0x18: its time.
    pub inline_time: f32,
    /// +0x1C.
    pub inline: Option<InlineSequence>,
    /// +0x20: the transition type ([`TRANSITION_TYPES`]).
    pub kind: u32,
    /// +0x24: its time.
    pub time: f32,
    /// +0x28: the index of the target sequence's instruction that carries the tag, or -1.
    pub instruction: i32,
    /// +0x2C: the transition's text, e.g. `[TRANS]'LADDER_IDLE'(left_foot)->'ANY'(left_foot)` with its
    /// terminating NUL; an inline sequence is created under this name.
    pub label: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TransitionContainer {
    pub name: u32,
    pub transitions: Vec<Transition>,
}

/// A `PclAnimationBank`.
#[derive(Clone, Debug, PartialEq)]
pub struct Bank {
    /// Name CRC.
    pub name: u32,
    /// The stored name bytes.
    pub text: Vec<u8>,
    /// Parent bank CRC, 0 for none.
    pub parent: u32,
    /// `AddOverride(first, second)` pairs.
    pub overrides: Vec<(u32, u32)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Pack {
    pub records: Vec<Record>,
    /// `PclAnimation::SetBlendBoneException`.
    pub blend_bone_exception: i32,
    pub intervals: Vec<Interval>,
    pub sequences: Vec<Sequence>,
    pub transitions: Vec<TransitionContainer>,
    /// The `EDGE` blocks, copied verbatim into `PclEdgeTransData`.
    pub edge: [Vec<u8>; 2],
    /// The `DIST` block, copied verbatim.
    pub dist: Vec<u8>,
    pub banks: Vec<Bank>,
    /// `AdditiveAnimNamePair(first, second)` pairs.
    pub additives: Vec<(u32, u32)>,
    /// `ALPH` first list, appended to `PclAnimationManager` +0x88.
    pub list_88: Vec<u32>,
    /// `ALPH` second list, appended to `PclAnimationManager` +0x7C.
    pub list_7c: Vec<u32>,
    /// Bytes read.
    pub len: usize,
}

pub fn parse(b: &[u8]) -> Result<Pack, String> {
    let mut c = Cursor { b, at: 0 };
    if c.take(8, "pack magic")? != MAGIC {
        return Err(format!("not an AP04 ANIM pack: {:02x?}", &b[..8]));
    }
    let n = c.count("record count")?;
    let records = (0..n).map(|i| record(&mut c).map_err(|e| format!("record {i}: {e}"))).collect::<Result<_, _>>()?;
    let blend_bone_exception = c.i32("blend bone exception")?;

    tag(&mut c, b"INTV")?;
    let intervals = list(&mut c, "interval", |c| {
        Ok(Interval { kind: c.u32("interval type")?, values: [c.i16("interval value")?, c.i16("interval value")?], animations: crcs(c, "interval animation")? })
    })?;

    tag(&mut c, b"SEQC")?;
    let sequences = list(&mut c, "sequence", |c| {
        let name = c.u32("sequence name")?;
        let instructions = list(c, "instruction", |c| {
            Ok(Instruction { word0: c.u32("instruction word 0")?, word1: c.i32("instruction word 1")?, animations: crcs(c, "instruction animation")?, tags: crcs(c, "instruction tag")? })
        })?;
        Ok(Sequence { name, instructions, body: sequence_body(c)? })
    })?;

    tag(&mut c, b"TRAN")?;
    let transitions = list(&mut c, "transition container", |c| {
        let name = c.u32("container name")?;
        let transitions = list(c, "transition", |c| {
            let [from_sequence, from_tag, to_sequence, to_tag] = [0; 4].map(|_| c.u32("transition CRC"));
            let min_time = c.f32("transition min time")?;
            let inline_type = c.u32("inline type")?;
            let inline_time = c.f32("inline time")?;
            let seq = c.u32("inline sequence name")?;
            let inline = if seq == 0 {
                None
            } else {
                let animation = c.u32("inline animation")?;
                let tag = c.u32("inline tag")?;
                Some(InlineSequence { name: seq, animation, tag, body: sequence_body(c)? })
            };
            let kind = c.u32("transition type")?;
            let time = c.f32("transition time")?;
            let instruction = c.i32("transition instruction")?;
            let len = c.i16("transition label length")?;
            let len = usize::try_from(len).map_err(|_| format!("negative transition label length {len}"))?;
            Ok(Transition {
                from_sequence: from_sequence?,
                from_tag: from_tag?,
                to_sequence: to_sequence?,
                to_tag: to_tag?,
                min_time,
                inline_type,
                inline_time,
                inline,
                kind,
                time,
                instruction,
                label: c.take(len, "transition label")?.to_vec(),
            })
        })?;
        Ok(TransitionContainer { name, transitions })
    })?;

    tag(&mut c, b"EDGE")?;
    let edge = [c.take(EDGE_LEN[0], "EDGE block")?.to_vec(), c.take(EDGE_LEN[1], "EDGE block")?.to_vec()];
    tag(&mut c, b"DIST")?;
    let dist = c.take(DIST_LEN, "DIST block")?.to_vec();

    tag(&mut c, b"BANK")?;
    let banks = list(&mut c, "bank", |c| {
        let name = c.u32("bank name")?;
        let len = c.i16("bank name length")?;
        let len = usize::try_from(len).map_err(|_| format!("negative bank name length {len}"))?;
        let text = c.take(len, "bank name text")?.to_vec();
        let parent = c.u32("bank parent")?;
        let overrides = list(c, "bank override", |c| Ok((c.u32("override")?, c.u32("override")?)))?;
        Ok(Bank { name, text, parent, overrides })
    })?;

    tag(&mut c, b"ADD0")?;
    let additives = list(&mut c, "additive pair", |c| Ok((c.u32("additive")?, c.u32("additive")?)))?;
    tag(&mut c, b"ALPH")?;
    let list_88 = crcs(&mut c, "ALPH list")?;
    let list_7c = crcs(&mut c, "ALPH list")?;
    Ok(Pack { records, blend_bone_exception, intervals, sequences, transitions, edge, dist, banks, additives, list_88, list_7c, len: c.at })
}

fn record(c: &mut Cursor) -> Result<Record, String> {
    let key = c.u32("record key")?;
    let object = c.u8("object flag")?;
    let size = c.i32("clip size")?;
    let clip_at = c.at;
    let clip = anim::parse(&c.b[clip_at..]).map_err(|e| format!("key 0x{key:08x}: {e}"))?;
    if usize::try_from(size) != Ok(clip.len) {
        return Err(format!("key 0x{key:08x}: the clip takes {} bytes, the record says {size}", clip.len));
    }
    c.at += clip.len;
    let motion_length = c.f32("motion length")?;
    let movement = c.f32s::<3>("movement")?;
    let rotation = c.f32("rotation")?;
    let start_aim = c.f32("start aim angle")?;
    let end_aim = c.f32("end aim angle")?;
    let aim_pitch = c.f32("aim pitch")?;
    let turn_positive = c.u8("turn positive")?;
    let events = list(c, "event", |c| {
        let frame = c.i32("event frame")?;
        let name = c.u32("event name")?;
        let mut params = [(0, 0.0); 4];
        for p in &mut params {
            *p = (c.u32("event parameter")?, c.f32("event value")?);
        }
        Ok(Event { frame, name, params })
    })?;
    let offsets = list(c, "offset", |c| Ok((c.f32s::<3>("offset vector")?, c.f32("offset value")?)))?;
    Ok(Record { key, object, size, clip_at, clip, motion_length, movement, rotation, start_aim, end_aim, aim_pitch, turn_positive, events, offsets })
}

fn sequence_body(c: &mut Cursor) -> Result<SequenceBody, String> {
    let length = c.f32("sequence length")?;
    let movement = c.f32s::<3>("sequence movement")?;
    let rotation = c.f32("sequence rotation")?;
    let kind = c.u32("sequence type")?;
    let aim_min_max = c.take(16, "AimMinMax")?.try_into().expect("16 bytes");
    let counts = c.take(3, "aim frame counts")?;
    let mut aim_frames: [Vec<[u8; 8]>; 3] = Default::default();
    for (frames, &n) in aim_frames.iter_mut().zip(counts) {
        let n = usize::try_from(n as i8).map_err(|_| format!("negative aim frame count {}", n as i8))?;
        *frames = (0..n).map(|_| c.take(8, "aim frame").map(|s| s.try_into().expect("8 bytes"))).collect::<Result<_, _>>()?;
    }
    let turn_positive = c.u8("sequence turn positive")?;
    let synchronized = c.u8("sequence synchronized")?;
    Ok(SequenceBody { length, movement, rotation, kind, aim_min_max, aim_frames, turn_positive, synchronized })
}

fn tag(c: &mut Cursor, want: &[u8; 4]) -> Result<(), String> {
    let at = c.at;
    let got = c.take(4, "section tag")?;
    if got != want {
        return Err(format!("expected section {} at 0x{at:x}, found {got:02x?}", String::from_utf8_lossy(want)));
    }
    Ok(())
}

fn list<T>(c: &mut Cursor, what: &str, mut item: impl FnMut(&mut Cursor) -> Result<T, String>) -> Result<Vec<T>, String> {
    let n = c.count(&format!("{what} count"))?;
    (0..n).map(|i| item(c).map_err(|e| format!("{what} {i}: {e}"))).collect()
}

fn crcs(c: &mut Cursor, what: &str) -> Result<Vec<u32>, String> {
    list(c, what, |c| c.u32(what))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn be(words: &[u32]) -> Vec<u8> {
        words.iter().flat_map(|w| w.to_be_bytes()).collect()
    }

    fn sequence_tail() -> Vec<u8> {
        let mut b = be(&[1.5f32.to_bits(), 0, 0, 0, 0, 4]);
        b.extend([0u8; 16]);
        b.extend([1, 0, 0]);
        b.extend([9u8; 8]);
        b.extend([1, 0]);
        b
    }

    #[test]
    fn a_minimal_pack_reads_to_its_end() {
        let mut clip = b"ANM0NOC0".to_vec();
        clip.extend(be(&[1.0f32.to_bits(), 1, 2]));
        clip.extend(vec![0u8; 2 * 0x30]);
        clip.extend(be(&[1, 0x1111_2222]));

        let mut b = MAGIC.to_vec();
        b.extend(be(&[1, 0xAAAA_0001]));
        b.push(0);
        b.extend(be(&[clip.len() as u32]));
        b.extend(&clip);
        b.extend(be(&[0; 8]));
        b.push(1);
        b.extend(be(&[1, 3, 0xEEEE_0001]));
        for i in 0..4 {
            b.extend(be(&[i, 0]));
        }
        b.extend(be(&[0, 101]));
        b.extend(b"INTV");
        b.extend(be(&[1, UNINTERRUPTIBLE]));
        b.extend([0, 0, 0, 10]);
        b.extend(be(&[1, 0xAAAA_0001]));
        b.extend(b"SEQC");
        b.extend(be(&[1, 0x5E00_0001, 1, 0, 0, 1, 0xAAAA_0001, 0]));
        b.extend(sequence_tail());
        b.extend(b"TRAN");
        b.extend(be(&[1, 0xC000_0001, 1, 0x5E00_0001, 1, 0x5E00_0001, 0, 0, 2, 0, 0x5E00_0002, 0xAAAA_0001, 0]));
        b.extend(sequence_tail());
        b.extend(be(&[1, 0, 0xFFFF_FFFF]));
        b.extend([0, 4]);
        b.extend(b"abc\0");
        b.extend(b"EDGE");
        b.extend(vec![0u8; EDGE_LEN[0] + EDGE_LEN[1]]);
        b.extend(b"DIST");
        b.extend(vec![0u8; DIST_LEN]);
        b.extend(b"BANK");
        b.extend(be(&[1, 0xBA00_0001]));
        b.extend([0, 2]);
        b.extend(b"x\0");
        b.extend(be(&[0, 1, 0xAAAA_0001, 0xAAAA_0002]));
        b.extend(b"ADD0");
        b.extend(be(&[0]));
        b.extend(b"ALPH");
        b.extend(be(&[1, 0x5E00_0001, 1, 0xAAAA_0001]));

        let p = parse(&b).unwrap();
        assert_eq!(p.len, b.len());
        let r = &p.records[0];
        assert_eq!((r.key, r.clip_at, r.turn_positive), (0xAAAA_0001, 21, 1));
        assert_eq!(r.clip.bones, [0x1111_2222]);
        assert_eq!(r.events[0].params[3], (3, 0.0));
        assert_eq!(p.blend_bone_exception, 101);
        assert_eq!(p.intervals[0].values, [0, 10]);
        assert_eq!(p.sequences[0].instructions[0].animations, [0xAAAA_0001]);
        assert_eq!(p.sequences[0].body.aim_frames[0], [[9u8; 8]]);
        let t = &p.transitions[0].transitions[0];
        assert_eq!((t.from_tag, t.inline_type, t.instruction), (1, 2, -1));
        assert_eq!(t.inline.as_ref().map(|s| s.animation), Some(0xAAAA_0001));
        assert_eq!(t.label, b"abc\0");
        assert_eq!(p.banks[0].overrides, [(0xAAAA_0001, 0xAAAA_0002)]);
        assert_eq!((p.list_88.as_slice(), p.list_7c.as_slice()), ([0x5E00_0001].as_slice(), [0xAAAA_0001].as_slice()));

        let mut short = b.clone();
        short.truncate(b.len() - 1);
        assert!(parse(&short).is_err());
    }
}
