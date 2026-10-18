//! `sound/WWiseIDTable.bin` — the 2008 build's map from game-side `PblCRC` keys to Wwise ids
//! (`WWiseIDTable::Load` @0x825fc488, called from `WSSoundManager::InitSoundData`). The game posts an
//! event by key: `WSSoundManager::PlayEvent` @0x825f6bf8 looks the key up with
//! `WWiseIDTable::GetEventPlayID` @0x825fbf70 and posts the play id. `docs/2008_audio_format.md`.
//!
//! ```text
//! u32 version (1; anything else asserts and fails the load)
//! u32 events, u32 state groups, u32 switch groups, u32 triggers, u32 RTPCs
//! events        × { u32 key, u32 play id, u32 stop id }        (ReadEvents @0x825fc210; -1 = none)
//! state groups  × group                                        (ReadGroups @0x825fc2c0; GetStateID)
//! switch groups × group                                        (GetSwitchID)
//! triggers      × { u32 key, u32 id }                          (GetTriggerID)
//! RTPCs         × { u32 key, u32 id }                          (GetRPTCID)
//! group = { u32 key, u32 group id, u32 n, n × { u32 key, u32 id } }
//! ```

use super::cursor::Cursor;

pub const VERSION: u32 = 1;
/// The id stored for an event without a play or stop event.
pub const NONE: u32 = u32::MAX;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EventIds {
    pub key: u32,
    pub play: u32,
    pub stop: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    pub key: u32,
    pub id: u32,
    pub members: Vec<(u32, u32)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdTable {
    pub events: Vec<EventIds>,
    pub state_groups: Vec<Group>,
    pub switch_groups: Vec<Group>,
    pub triggers: Vec<(u32, u32)>,
    pub rtpcs: Vec<(u32, u32)>,
}

fn pairs(c: &mut Cursor, n: u32, what: &str) -> Result<Vec<(u32, u32)>, String> {
    (0..n).map(|_| Ok((c.u32(what)?, c.u32(what)?))).collect()
}

fn groups(c: &mut Cursor, n: u32, what: &str) -> Result<Vec<Group>, String> {
    let mut out = Vec::new();
    for _ in 0..n {
        let key = c.u32(what)?;
        let id = c.u32(what)?;
        let m = c.u32(what)?;
        out.push(Group { key, id, members: pairs(c, m, what)? });
    }
    Ok(out)
}

/// Parse the whole file; the tables must end exactly at its end.
pub fn parse(b: &[u8]) -> Result<IdTable, String> {
    let mut c = Cursor { b, at: 0 };
    let version = c.u32("version")?;
    if version != VERSION {
        return Err(format!("WWiseIDTable version {version}, the loader accepts {VERSION}"));
    }
    let [ne, ns, nw, nt, nr] = [c.u32("event count")?, c.u32("state group count")?, c.u32("switch group count")?, c.u32("trigger count")?, c.u32("RTPC count")?];
    let mut events = Vec::new();
    for _ in 0..ne {
        events.push(EventIds { key: c.u32("event key")?, play: c.u32("event play id")?, stop: c.u32("event stop id")? });
    }
    let t = IdTable {
        events,
        state_groups: groups(&mut c, ns, "state group")?,
        switch_groups: groups(&mut c, nw, "switch group")?,
        triggers: pairs(&mut c, nt, "trigger")?,
        rtpcs: pairs(&mut c, nr, "RTPC")?,
    };
    if c.at != b.len() {
        return Err(format!("WWiseIDTable: tables end at 0x{:X} of 0x{:X} bytes", c.at, b.len()));
    }
    Ok(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_in_order() {
        let words = [1u32, 1, 1, 0, 1, 0, 0xA, 0xB, NONE, 0x5, 0x6, 1, 0x7, 0x8, 0xC, 0xD];
        let b: Vec<u8> = words.iter().flat_map(|w| w.to_be_bytes()).collect();
        let t = parse(&b).unwrap();
        assert_eq!(t.events, vec![EventIds { key: 0xA, play: 0xB, stop: NONE }]);
        assert_eq!(t.state_groups, vec![Group { key: 5, id: 6, members: vec![(7, 8)] }]);
        assert_eq!((t.switch_groups.len(), t.triggers.clone(), t.rtpcs.len()), (0, vec![(0xC, 0xD)], 0));
        assert!(parse(&b[..b.len() - 4]).is_err());
        let mut v2 = b.clone();
        v2[3] = 2;
        assert!(parse(&v2).unwrap_err().contains("version 2"));
    }
}
