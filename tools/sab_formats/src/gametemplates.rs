//! GameTemplates.wsd (magic `AULB`) — The Saboteur's object-definition layer: a flat table of named
//! templates (disguise use-points, cars, weapons, props, lights, particles, humans…), each a bag of
//! `{property-hash → data}` pairs. Ground-truthed against `DLC/01/GameTemplates.wsd` and the full main
//! DB (embedded in `France/loosefiles_BinPC.pack`), and the loader `FUN_0162bfa0 @0x0162bfa0`. See
//! `docs/formats/gametemplates.md`.
//!
//! ```text
//! Header: "AULB", u32 entry_count
//! entry_count × Entry, each a Marker (12 bytes: 08 00 00 00 + 8 zero) or a Template:
//!   u32 total_size (bytes after this field), u32 unk1=0, u32 unk2=1,
//!   u32 name_len, name+NUL, u32 type_len, type+NUL, u32 pair_count,
//!   pair_count × { u32 hash=pandemic_hash(prop) LE, u32 data_size, data[data_size] }
//! ```
//!
//! A 4-byte `data` may itself be a `pandemic_hash` — a sibling-template ref, `"none"`, or a **texture**
//! (`data == pandemic_hash(textureName)`, the ALBS/WSTX key; see the format doc §Texture references).

const MAGIC: &[u8; 4] = b"AULB";
const MARKER: [u8; 12] = [8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];

#[derive(Clone, Debug)]
pub struct Pair {
    pub hash: u32,
    pub data: Vec<u8>,
}

impl Pair {
    /// If `data` is exactly 4 bytes, its little-endian u32 (an int / f32 bits / a pandemic_hash ref).
    pub fn as_u32(&self) -> Option<u32> {
        (self.data.len() == 4).then(|| u32::from_le_bytes(self.data.clone().try_into().unwrap()))
    }
}

#[derive(Clone, Debug)]
pub struct Template {
    pub unk1: u32,
    pub unk2: u32,
    pub name: String,
    pub ttype: String,
    pub pairs: Vec<Pair>,
}

impl Template {
    pub fn pair(&self, hash: u32) -> Option<&Pair> {
        self.pairs.iter().find(|p| p.hash == hash)
    }
    pub fn pair_mut(&mut self, hash: u32) -> Option<&mut Pair> {
        self.pairs.iter_mut().find(|p| p.hash == hash)
    }
    /// Set (or insert) a pair's data by property-name hash.
    pub fn set_pair(&mut self, hash: u32, data: Vec<u8>) {
        match self.pair_mut(hash) {
            Some(p) => p.data = data,
            None => self.pairs.push(Pair { hash, data }),
        }
    }
    fn body_size(&self) -> usize {
        let mut n = 4 + 4; // unk1 + unk2
        n += 4 + self.name.len() + 1;
        n += 4 + self.ttype.len() + 1;
        n += 4; // pair_count
        for p in &self.pairs {
            n += 4 + 4 + p.data.len();
        }
        n
    }
}

#[derive(Clone, Debug)]
pub enum Entry {
    Marker,
    Template(Template),
}

pub struct GameTemplates {
    pub entries: Vec<Entry>,
}

fn rd_u32(b: &[u8], o: usize) -> Result<u32, String> {
    b.get(o..o + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]])).ok_or_else(|| format!("EOF u32 @{o}"))
}

impl GameTemplates {
    /// Iterate templates (skipping markers) with their entry index.
    pub fn templates(&self) -> impl Iterator<Item = (usize, &Template)> {
        self.entries.iter().enumerate().filter_map(|(i, e)| match e {
            Entry::Template(t) => Some((i, t)),
            Entry::Marker => None,
        })
    }
    pub fn find(&self, name: &str) -> Option<(usize, &Template)> {
        self.templates().find(|(_, t)| t.name == name)
    }
    pub fn template_mut(&mut self, entry_index: usize) -> Option<&mut Template> {
        match self.entries.get_mut(entry_index) {
            Some(Entry::Template(t)) => Some(t),
            _ => None,
        }
    }

    pub fn parse(b: &[u8]) -> Result<(GameTemplates, usize), String> {
        if b.get(0..4) != Some(MAGIC.as_slice()) {
            return Err(format!("bad magic {:?}, expected AULB", &b.get(0..4)));
        }
        let count = rd_u32(b, 4)?;
        let mut o = 8;
        let mut entries = Vec::with_capacity(count as usize);
        for ti in 0..count {
            if b.get(o..o + 12) == Some(MARKER.as_slice()) {
                o += 12;
                entries.push(Entry::Marker);
                continue;
            }
            let total_size = rd_u32(b, o)?;
            let body_start = o + 4;
            let unk1 = rd_u32(b, body_start)?;
            let unk2 = rd_u32(b, body_start + 4)?;
            let mut p = body_start + 8;
            let nlen = rd_u32(b, p)? as usize;
            p += 4;
            let name = String::from_utf8_lossy(b.get(p..p + nlen - 1).ok_or("EOF name")?).into_owned();
            p += nlen;
            let tlen = rd_u32(b, p)? as usize;
            p += 4;
            let ttype = String::from_utf8_lossy(b.get(p..p + tlen - 1).ok_or("EOF type")?).into_owned();
            p += tlen;
            let pcount = rd_u32(b, p)?;
            p += 4;
            let mut pairs = Vec::with_capacity(pcount as usize);
            for _ in 0..pcount {
                let hash = rd_u32(b, p)?;
                let dsize = rd_u32(b, p + 4)? as usize;
                let data = b.get(p + 8..p + 8 + dsize).ok_or("EOF pair data")?.to_vec();
                p += 8 + dsize;
                pairs.push(Pair { hash, data });
            }
            let measured = (p - body_start) as u32;
            if measured != total_size {
                return Err(format!(
                    "template {ti} ({name:?}): total_size={total_size} but measured body={measured}"
                ));
            }
            o = p;
            entries.push(Entry::Template(Template { unk1, unk2, name, ttype, pairs }));
        }
        Ok((GameTemplates { entries }, o))
    }

    pub fn write(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&(self.entries.len() as u32).to_le_bytes());
        for e in &self.entries {
            match e {
                Entry::Marker => out.extend_from_slice(&MARKER),
                Entry::Template(t) => {
                    out.extend_from_slice(&(t.body_size() as u32).to_le_bytes());
                    out.extend_from_slice(&t.unk1.to_le_bytes());
                    out.extend_from_slice(&t.unk2.to_le_bytes());
                    out.extend_from_slice(&((t.name.len() + 1) as u32).to_le_bytes());
                    out.extend_from_slice(t.name.as_bytes());
                    out.push(0);
                    out.extend_from_slice(&((t.ttype.len() + 1) as u32).to_le_bytes());
                    out.extend_from_slice(t.ttype.as_bytes());
                    out.push(0);
                    out.extend_from_slice(&(t.pairs.len() as u32).to_le_bytes());
                    for p in &t.pairs {
                        out.extend_from_slice(&p.hash.to_le_bytes());
                        out.extend_from_slice(&(p.data.len() as u32).to_le_bytes());
                        out.extend_from_slice(&p.data);
                    }
                }
            }
        }
        out
    }
}

#[path = "gametemplates_props.rs"]
mod props;

/// What a property's bytes mean. Derived, not declared: the engine's per-class `SetProperty`
/// dispatchers reveal which reader helper consumes each key, and the type is only recorded when
/// that reader agrees with the size histogram observed across all 10,761 retail templates.
///
/// This is for DISPLAY and authoring assistance only, never for framing. `size` in the pair header
/// is always authoritative — see `docs/formats/editnodes.md`'s account of a value-based classifier
/// that became a live parse bug.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PropType {
    /// one byte, usually a flag (often packed into a bitfield by the engine)
    Bool,
    /// four bytes read plainly — an int or a float, and the corpus cannot always say which
    Scalar4,
    /// four bytes read byte-reversing: a `pandemic_hash` naming another blueprint, a texture or a
    /// string. `"none"` (`0x4FF9F863`) is the null sentinel.
    Ref,
    /// twelve bytes, three f32
    Vec3,
    /// NUL-terminated text
    Str,
    /// anything else, including keys whose width varies between templates
    Bytes,
}

/// The property name for a key, if one is known ([`crate::names`]).
pub fn known_property_name(hash: u32) -> Option<&'static str> {
    crate::names::name(hash)
}

/// The derived value type for a key. `None` for a key that does not occur in the retail table.
pub fn property_type(hash: u32) -> Option<PropType> {
    props::TYPES
        .binary_search_by_key(&hash, |&(k, _)| k)
        .ok()
        .map(|i| props::TYPES[i].1)
}

#[cfg(test)]
mod prop_table_tests {
    use super::*;
    use crate::pandemic_hash;

    /// The type table is binary-searched, so sortedness is load-bearing, and a duplicate key would
    /// make the lookup pick an arbitrary one of them.
    #[test]
    fn the_type_table_is_sorted_and_unique() {
        for w in props::TYPES.windows(2) {
            assert!(w[0].0 < w[1].0, "TYPES not sorted/unique at 0x{:08X}", w[0].0);
        }
    }

    /// Spot-checks that survive regeneration, one per evidence class:
    /// - `Looping` / `Sound Events` — the audio attachment block, where the recovered NAMES and the
    ///   decompiled record layout independently agree
    /// - `Game Event Name` — a name containing SPACES, which identifier-shaped harvesting cannot
    ///   produce and which the community dictionary filed under a byteswapped key
    /// - `Model` / `Texture` — carried over from the 15 hand-listed names, so nothing regressed
    #[test]
    fn known_names_and_types_survive_regeneration() {
        for (key, name) in [
            (0x1E59E605u32, "Looping"),
            (0x42C6DA9D, "Sound Events"),
            (0x246DB06C, "Game Event Name"),
            (0xD76D7747, "Sound Event Name"),
            (0x5B724250, "Model"),
            (0xF011157A, "Texture"),
        ] {
            assert_eq!(pandemic_hash(name), key, "{name:?} is not a preimage of 0x{key:08X}");
            assert_eq!(
                known_property_name(key).map(str::to_ascii_lowercase),
                Some(name.to_ascii_lowercase()),
                "0x{key:08X} should be named {name:?} (pandemic_hash case-folds, so compare folded)"
            );
        }
        // Width-backed types are trustworthy: `Looping` is one byte in all 6,525 uses.
        assert_eq!(property_type(0x1E59E605), Some(PropType::Bool));
        // `AudibleEvent` is a Ref on the engine's own reader (FUN_004511d0) agreeing with width 4.
        assert_eq!(property_type(0x02D3EB9F), Some(PropType::Ref));
        // KNOWN LIMIT, not an oversight. `Game Event Name` really is a Ref — the decomp reads it
        // with the byte-reversing helper — but its only appearance is behind an inverted
        // `if (param_2 != 0xKEY)` guard, so equality-only attribution cannot claim it and the
        // size histogram cannot tell Scalar4 from Ref (both are 4 bytes). It therefore falls back
        // to Scalar4. Separating the two needs brace-aware extraction; until then this asserts the
        // conservative answer rather than a flattering one.
        assert_eq!(property_type(0x246DB06C), Some(PropType::Scalar4));
        // A key that is not in the retail table has no type.
        assert_eq!(property_type(0xDEADBEEF), None);
    }
}
