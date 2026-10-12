//! Names for `pandemic_hash` values the retail game uses: the table `data/hash_names.tsv`, built by
//! `sab_probe hash-names` (sources, grades and the chance estimate: `docs/hash_names.md`).
//!
//! ```text
//! hash        0x%08x, the target
//! name        pandemic_hash(name) == hash
//! grade       structural | data | text
//! chance      text rows: estimated probability the name is a chance collision; "-" otherwise
//! kinds       where the game uses the hash (megapack.crc, albs.type0, editnodes.object, ...)
//! sources     where the name was found
//! ```
//!
//! Rows are sorted by hash; a hash has more than one row when more than one name was found for it.

use std::sync::OnceLock;

static TSV: &str = include_str!("../../../data/hash_names.tsv");

/// How a name is known to be the hash's name, best first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Grade {
    /// Checked against a second relation in the data: an EditNodes object name whose parent path
    /// hashes, as a container, to the entry that holds the object.
    Structural,
    /// The name is a string stored in the game data (a mesh, texture, map, loose-file, EditNodes or
    /// GameTemplates string).
    Data,
    /// The name comes from text outside the game data (scripts, the exe, symbols, community lists).
    Text,
}

/// One row of the table.
#[derive(Clone, Copy, Debug)]
pub struct Name {
    pub hash: u32,
    pub name: &'static str,
    pub grade: Grade,
    /// For [`Grade::Text`], the estimated probability that the name is a chance collision.
    pub chance: Option<f32>,
    /// Comma-separated: where the game uses the hash.
    pub kinds: &'static str,
    /// Comma-separated: where the name was found.
    pub sources: &'static str,
}

fn parse_row(line: &'static str, no: usize) -> Name {
    let bad = |what: &str| -> ! { panic!("data/hash_names.tsv line {no}: {what}: {line:?}") };
    let cols: Vec<&'static str> = line.split('\t').collect();
    let [hash, name, grade, chance, kinds, sources] = cols[..] else { bad("expected 6 columns") };
    let hash = hash.strip_prefix("0x").and_then(|h| u32::from_str_radix(h, 16).ok()).unwrap_or_else(|| bad("hash"));
    let grade = match grade {
        "structural" => Grade::Structural,
        "data" => Grade::Data,
        "text" => Grade::Text,
        _ => bad("grade"),
    };
    let chance = match (grade, chance) {
        (Grade::Text, c) => Some(c.parse::<f32>().unwrap_or_else(|_| bad("chance"))),
        (_, "-") => None,
        _ => bad("chance on a non-text row"),
    };
    Name { hash, name, grade, chance, kinds, sources }
}

/// Every row, sorted by hash.
pub fn table() -> &'static [Name] {
    static TABLE: OnceLock<Vec<Name>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut lines = TSV.lines().enumerate();
        match lines.next() {
            Some((_, "hash\tname\tgrade\tchance\tkinds\tsources")) => {}
            other => panic!("data/hash_names.tsv: unexpected header {other:?}"),
        }
        lines.map(|(i, l)| parse_row(l, i + 1)).collect()
    })
}

/// Every name found for `hash` (empty when none is known).
pub fn names(hash: u32) -> &'static [Name] {
    let t = table();
    let start = t.partition_point(|r| r.hash < hash);
    let end = t.partition_point(|r| r.hash <= hash);
    &t[start..end]
}

/// The best name for `hash`: the best [`Grade`], then the lowest chance.
pub fn name(hash: u32) -> Option<&'static str> {
    names(hash)
        .iter()
        .min_by(|a, b| a.grade.cmp(&b.grade).then(a.chance.unwrap_or(0.0).total_cmp(&b.chance.unwrap_or(0.0))))
        .map(|r| r.name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pandemic_hash;

    #[test]
    fn every_row_is_a_preimage_and_the_table_is_sorted() {
        let t = table();
        assert!(t.len() > 80_000, "{} rows", t.len());
        for r in t {
            assert_eq!(pandemic_hash(r.name), r.hash, "{:?} does not hash to 0x{:08x}", r.name, r.hash);
        }
        assert!(t.windows(2).all(|w| w[0].hash <= w[1].hash), "rows must be sorted by hash");
    }

    #[test]
    fn lookups() {
        assert_eq!(name(pandemic_hash("ClassName")), Some("ClassName"));
        assert_eq!(name(pandemic_hash("LuaParam")), Some("LuaParam"), "the EditNodes tag of a Lua parameter");
        assert_eq!(name(pandemic_hash("LuaCrcList")), None, "a type marker inside a value, not a key");
        // An object named only by a script string, proven by its container: the streamblock name
        // that also hashes to it ranks below.
        assert_eq!(name(0xd260_aa99), Some(r"Missions\act_3\mission_5\starter\Gondola_Kessler"));
        assert_eq!(names(0xd260_aa99).len(), 2);
        assert_eq!(names(0xdead_beef).len(), 0);
    }
}
