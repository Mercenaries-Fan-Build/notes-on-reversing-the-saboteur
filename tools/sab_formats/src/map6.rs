//! `MAP6` world maps: `global.map` and the region map `France.map` (both inside
//! `France/loosefiles_BinPC.pack`), and their DLC counterparts `DLC/01/Global.map` / `FRANCE.map`.
//! Layouts and loaders: `docs/formats/map6.md`.
//!
//! ```text
//! global.map  (FUN_009f2df0 main, FUN_009f3370 DLC)
//!   u32 magic = 0x4D415036 ("6PAM")
//!   u32 numDynamics
//!   u32 n; Record[n]          list 1
//!   u32 n; Record[n]          list 2
//!   Record[numDynamics]       FUN_009f2a00
//!
//! France.map, main  (FUN_009f4320)
//!   u32 magic; char region[] (NUL-terminated); u32 numTiles; u32 unknown; u32 numPaths
//!   if numPaths > 0: u32 pathsHeader; Path[numPaths]
//!   3 x (f32[3], f32[3]); 3 x (u16, u16)
//!   Tile[numTiles]             FUN_009f3900
//!   u32 n; Record[n]           FUN_009f3bf0
//!   u32 n; Record[n]           FUN_009f3fa0
//!
//! France.map, DLC  (FUN_009f75f0)
//!   u32 magic; char region[]; u32 zero (FUN_00427cb0); u32 numTiles; Tile[numTiles];
//!   u32 n; Record[n]; u32 n; Record[n]
//!
//! Record:  u32 hash; u16 nameLen; u8 name[nameLen]; f32 a[3]; f32 b[3]; u16 u0; u16 u1; Tail
//! Tail:    FUN_00658aa0, FUN_00659f20 / FUN_00659f50
//!          u32 n1; {u32,u32}[n1]; u32 n2; {u32,u32}[n2]; u32 x; u32 counts[9];
//!          u32 nA; u32[nA]; u32 nB; u32[nB & 0xFFFF]; u32 nC; {u32 id; u32 k; u32[k]}[nC]
//! Tile:    u32 hash; u16 nameLen; u8 name[nameLen]; f32 a[3]; f32 b[3]; u16 u0; i16 sel;
//!          Tail when sel >= 2
//! Path:    LenStr; LenStr; u8[48]; LenStr      (LenStr = u32 len + len bytes)
//! ```
//!
//! The Path layout is SaboteurToolset's `france_extract` structure, not read from the engine's path
//! loop (whose arguments the decompiler dropped); it consumes all 1,733 retail paths exactly.
//!
//! `counts` are the nine per-type blob counts of the object's ALBS sub-pack ([`crate::sbla`]).

use crate::bytes;

/// `"6PAM"` on disk.
pub const MAGIC: u32 = 0x4D41_5036;

/// The tail every object record (and every extended tile) carries.
#[derive(Clone, Debug, PartialEq)]
pub struct Tail {
    pub list1: Vec<[u32; 2]>,
    pub list2: Vec<[u32; 2]>,
    pub x: u32,
    /// The ALBS sub-pack's nine per-type blob counts.
    pub counts: [u32; 9],
    pub a: Vec<u32>,
    /// The stored `nB`; only its low 16 bits count the entries in `b`.
    pub nb: u32,
    pub b: Vec<u32>,
    pub c: Vec<(u32, Vec<u32>)>,
}

/// An object record.
#[derive(Clone, Debug, PartialEq)]
pub struct Record {
    pub hash: u32,
    /// The stored name bytes (`nameLen` of them, normally ending in NUL).
    pub name: Vec<u8>,
    pub a: [f32; 3],
    pub b: [f32; 3],
    pub u0: u16,
    pub u1: u16,
    pub tail: Tail,
}

impl Record {
    /// The name up to its first NUL.
    pub fn name(&self) -> String {
        let end = self.name.iter().position(|&c| c == 0).unwrap_or(self.name.len());
        String::from_utf8_lossy(&self.name[..end]).into_owned()
    }
}

/// A region tile.
#[derive(Clone, Debug, PartialEq)]
pub struct Tile {
    pub hash: u32,
    pub name: Vec<u8>,
    pub a: [f32; 3],
    pub b: [f32; 3],
    pub u0: u16,
    pub sel: i16,
    /// Present exactly when `sel >= 2`.
    pub tail: Option<Tail>,
}

/// A region path.
#[derive(Clone, Debug, PartialEq)]
pub struct Path {
    pub s0: Vec<u8>,
    pub s1: Vec<u8>,
    pub data: [u8; 48],
    pub s2: Vec<u8>,
}

/// `global.map`.
#[derive(Clone, Debug, PartialEq)]
pub struct GlobalMap {
    pub list1: Vec<Record>,
    pub list2: Vec<Record>,
    pub dynamics: Vec<Record>,
}

/// The main `France.map` header fields between the region name and the tiles.
#[derive(Clone, Debug, PartialEq)]
pub struct RegionHeader {
    pub unknown: u32,
    /// Present exactly when there are paths.
    pub paths_header: Option<u32>,
    pub paths: Vec<Path>,
    pub boxes: [[f32; 6]; 3],
    pub pairs: [[u16; 2]; 3],
}

/// A region map (`France.map`).
#[derive(Clone, Debug, PartialEq)]
pub struct RegionMap {
    /// The region name bytes, without the terminating NUL.
    pub region: Vec<u8>,
    /// `Some` for the main file's header; `None` for the DLC file, which stores a zero instead.
    pub header: Option<RegionHeader>,
    pub tiles: Vec<Tile>,
    pub list2: Vec<Record>,
    pub list3: Vec<Record>,
}

struct Cur<'a> {
    b: &'a [u8],
    p: usize,
}

impl<'a> Cur<'a> {
    fn take(&mut self, n: usize, field: &str) -> Result<&'a [u8], String> {
        let s = bytes::slice(self.b, self.p, n, field)?;
        self.p += n;
        Ok(s)
    }
    fn u16(&mut self, field: &str) -> Result<u16, String> {
        let v = bytes::u16_le(self.b, self.p, field)?;
        self.p += 2;
        Ok(v)
    }
    fn u32(&mut self, field: &str) -> Result<u32, String> {
        let v = bytes::u32_le(self.b, self.p, field)?;
        self.p += 4;
        Ok(v)
    }
    fn f32x3(&mut self, field: &str) -> Result<[f32; 3], String> {
        let mut v = [0f32; 3];
        for x in &mut v {
            *x = bytes::f32_le(self.b, self.p, field)?;
            self.p += 4;
        }
        Ok(v)
    }
    fn u32s(&mut self, n: usize, field: &str) -> Result<Vec<u32>, String> {
        (0..n).map(|_| self.u32(field)).collect()
    }
    fn pairs(&mut self, field: &str) -> Result<Vec<[u32; 2]>, String> {
        let n = self.u32(field)? as usize;
        (0..n).map(|_| Ok([self.u32(field)?, self.u32(field)?])).collect()
    }
    fn len_str(&mut self, field: &str) -> Result<Vec<u8>, String> {
        let n = self.u32(field)? as usize;
        Ok(self.take(n, field)?.to_vec())
    }
    fn cstr(&mut self, field: &str) -> Result<Vec<u8>, String> {
        let rest = &self.b[self.p.min(self.b.len())..];
        let n = rest.iter().position(|&c| c == 0).ok_or_else(|| format!("{field}: no NUL at 0x{:X}", self.p))?;
        let s = rest[..n].to_vec();
        self.p += n + 1;
        Ok(s)
    }
    fn magic(&mut self) -> Result<(), String> {
        let m = self.u32("MAP6 magic")?;
        if m != MAGIC {
            return Err(format!("bad MAP6 magic 0x{m:08X}"));
        }
        Ok(())
    }
    fn end(&self, what: &str) -> Result<(), String> {
        if self.p != self.b.len() {
            return Err(format!("{what}: parse ends at 0x{:X}, file is 0x{:X} bytes", self.p, self.b.len()));
        }
        Ok(())
    }

    fn tail(&mut self) -> Result<Tail, String> {
        let list1 = self.pairs("tail list1")?;
        let list2 = self.pairs("tail list2")?;
        let x = self.u32("tail x")?;
        let mut counts = [0u32; 9];
        for c in &mut counts {
            *c = self.u32("tail counts")?;
        }
        let na = self.u32("tail nA")? as usize;
        let a = self.u32s(na, "tail A")?;
        let nb = self.u32("tail nB")?;
        let b = self.u32s((nb & 0xFFFF) as usize, "tail B")?;
        let nc = self.u32("tail nC")? as usize;
        let mut c = Vec::with_capacity(nc);
        for _ in 0..nc {
            let id = self.u32("tail C id")?;
            let k = self.u32("tail C k")? as usize;
            c.push((id, self.u32s(k, "tail C values")?));
        }
        Ok(Tail { list1, list2, x, counts, a, nb, b, c })
    }

    fn record(&mut self) -> Result<Record, String> {
        let hash = self.u32("record hash")?;
        let n = self.u16("record nameLen")? as usize;
        let name = self.take(n, "record name")?.to_vec();
        let a = self.f32x3("record a")?;
        let b = self.f32x3("record b")?;
        let u0 = self.u16("record u0")?;
        let u1 = self.u16("record u1")?;
        let tail = self.tail()?;
        Ok(Record { hash, name, a, b, u0, u1, tail })
    }

    fn records(&mut self, n: usize) -> Result<Vec<Record>, String> {
        (0..n).map(|_| self.record()).collect()
    }

    fn counted_records(&mut self, field: &str) -> Result<Vec<Record>, String> {
        let n = self.u32(field)? as usize;
        self.records(n)
    }

    fn tile(&mut self) -> Result<Tile, String> {
        let hash = self.u32("tile hash")?;
        let n = self.u16("tile nameLen")? as usize;
        let name = self.take(n, "tile name")?.to_vec();
        let a = self.f32x3("tile a")?;
        let b = self.f32x3("tile b")?;
        let u0 = self.u16("tile u0")?;
        let sel = self.u16("tile sel")? as i16;
        let tail = if sel >= 2 { Some(self.tail()?) } else { None };
        Ok(Tile { hash, name, a, b, u0, sel, tail })
    }
}

/// The directory of `global.map`, which prefixes the pack path of every global object.
pub const GLOBAL_DIR: &str = "Global";

fn insert_counts(
    m: &mut std::collections::BTreeMap<u32, [u32; 9]>,
    key: u32,
    counts: [u32; 9],
    what: &str,
) -> Result<(), String> {
    match m.insert(key, counts) {
        Some(old) if old != counts => Err(format!("{what} 0x{key:08X}: counts {old:?} and {counts:?}")),
        _ => Ok(()),
    }
}

/// The pack path of a world streamblock, without extension (`FUN_009f5a10`): `<region>\<id>` for an id
/// below 10, else `<region>\<first two digits of id>\<id>`. Its megapack entry's `crc` is
/// `pandemic_hash` of the path plus `.pack`, and its `index` is `id`.
pub fn streamblock_path(region: &str, id: u32) -> String {
    let d = id.to_string();
    if id < 10 {
        format!("{region}\\{d}")
    } else {
        format!("{region}\\{}\\{d}", &d[..2])
    }
}

/// The ALBS type counts of the map objects a megapack names by path, keyed by the megapack entry's
/// `crc`: `pandemic_hash` of the object's path (`"%s\\%s"` via `FUN_006586c0`, set by the map
/// loaders) plus the extension its pack loader appends.
///
/// ```text
/// global.map dynamics    Global\<name>.dynpack         FUN_009f2a00; FUN_009f2370
/// global.map list 2      Global\<name>1.palettepack    FUN_009f2df0 ("%s%d"); FUN_009f1520
/// region list 2, list 3  <region>\<name>.pack          FUN_009f3bf0, FUN_009f3fa0; FUN_00658870
/// ```
///
/// Two objects with one path and different counts are an error.
pub fn pack_counts(
    globals: &[&GlobalMap],
    regions: &[&RegionMap],
) -> Result<std::collections::BTreeMap<u32, [u32; 9]>, String> {
    let mut m = std::collections::BTreeMap::new();
    for (path, counts) in pack_paths(globals, regions)? {
        insert_counts(&mut m, crate::pandemic_hash(&path), counts, &path)?;
    }
    Ok(m)
}

/// Each map object's pack path (the table on [`pack_counts`]) with its ALBS type counts, in map order.
pub fn pack_paths(globals: &[&GlobalMap], regions: &[&RegionMap]) -> Result<Vec<(String, [u32; 9])>, String> {
    let mut v = Vec::new();
    for g in globals {
        for r in &g.dynamics {
            v.push((format!("{GLOBAL_DIR}\\{}.dynpack", r.name()), r.tail.counts));
        }
        for r in &g.list2 {
            v.push((format!("{GLOBAL_DIR}\\{}1.palettepack", r.name()), r.tail.counts));
        }
    }
    for rm in regions {
        let region = String::from_utf8(rm.region.clone()).map_err(|_| "region name is not UTF-8".to_string())?;
        for r in rm.list2.iter().chain(&rm.list3) {
            v.push((format!("{region}\\{}.pack", r.name()), r.tail.counts));
        }
    }
    Ok(v)
}

/// The ALBS type counts of every object in the given maps, by record hash: every record of a
/// `global.map`, and every list record and extended tile of a region map. A `.kiloPack` entry's
/// `index` and a DLC loose pack's `pandemic_hash(file stem)` are the hash of the object whose counts
/// its pack uses. Two entries with one hash and different counts are an error.
pub fn object_counts(
    globals: &[&GlobalMap],
    regions: &[&RegionMap],
) -> Result<std::collections::BTreeMap<u32, [u32; 9]>, String> {
    let mut m = std::collections::BTreeMap::new();
    let mut add = |hash: u32, counts: [u32; 9]| insert_counts(&mut m, hash, counts, "object");
    for g in globals {
        for r in g.list1.iter().chain(&g.list2).chain(&g.dynamics) {
            add(r.hash, r.tail.counts)?;
        }
    }
    for rm in regions {
        for r in rm.list2.iter().chain(&rm.list3) {
            add(r.hash, r.tail.counts)?;
        }
        for t in &rm.tiles {
            if let Some(tail) = &t.tail {
                add(t.hash, tail.counts)?;
            }
        }
    }
    Ok(m)
}

/// A [`Tail`] at `p`, and the offset just past it.
pub(crate) fn parse_tail_at(buf: &[u8], p: usize) -> Result<(Tail, usize), String> {
    let mut c = Cur { b: buf, p };
    let t = c.tail()?;
    Ok((t, c.p))
}

/// Append `t` in its on-disk form.
pub(crate) fn write_tail(o: &mut Vec<u8>, t: &Tail) -> Result<(), String> {
    check_tails(std::iter::once(t))?;
    put_tail(o, t);
    Ok(())
}

/// Parse `global.map` (main or DLC; both loaders read the same layout). The parse must consume `buf`
/// exactly.
pub fn parse_global(buf: &[u8]) -> Result<GlobalMap, String> {
    let mut c = Cur { b: buf, p: 0 };
    c.magic()?;
    let nd = c.u32("numDynamics")? as usize;
    let list1 = c.counted_records("global list1 count")?;
    let list2 = c.counted_records("global list2 count")?;
    let dynamics = c.records(nd)?;
    c.end("global.map")?;
    Ok(GlobalMap { list1, list2, dynamics })
}

/// Parse the main `France.map` (`FUN_009f4320`). The parse must consume `buf` exactly.
pub fn parse_region_main(buf: &[u8]) -> Result<RegionMap, String> {
    let mut c = Cur { b: buf, p: 0 };
    c.magic()?;
    let region = c.cstr("region name")?;
    let num_tiles = c.u32("numTiles")? as usize;
    let unknown = c.u32("region unknown")?;
    let num_paths = c.u32("numPaths")? as usize;
    let paths_header = if num_paths > 0 { Some(c.u32("paths header")?) } else { None };
    let mut paths = Vec::with_capacity(num_paths);
    for _ in 0..num_paths {
        let s0 = c.len_str("path s0")?;
        let s1 = c.len_str("path s1")?;
        let data: [u8; 48] = c.take(48, "path data")?.try_into().expect("48 bytes");
        let s2 = c.len_str("path s2")?;
        paths.push(Path { s0, s1, data, s2 });
    }
    let mut boxes = [[0f32; 6]; 3];
    for bx in &mut boxes {
        let (a, b) = (c.f32x3("region box min")?, c.f32x3("region box max")?);
        *bx = [a[0], a[1], a[2], b[0], b[1], b[2]];
    }
    let mut pairs = [[0u16; 2]; 3];
    for pr in &mut pairs {
        *pr = [c.u16("region pair")?, c.u16("region pair")?];
    }
    let tiles = (0..num_tiles).map(|_| c.tile()).collect::<Result<Vec<_>, _>>()?;
    let list2 = c.counted_records("region list2 count")?;
    let list3 = c.counted_records("region list3 count")?;
    c.end("France.map")?;
    Ok(RegionMap {
        region,
        header: Some(RegionHeader { unknown, paths_header, paths, boxes, pairs }),
        tiles,
        list2,
        list3,
    })
}

/// Parse a DLC region map (`FUN_009f75f0`). The parse must consume `buf` exactly.
pub fn parse_region_dlc(buf: &[u8]) -> Result<RegionMap, String> {
    let mut c = Cur { b: buf, p: 0 };
    c.magic()?;
    let region = c.cstr("region name")?;
    let zero = c.u32("DLC region zero")?;
    if zero != 0 {
        return Err(format!("DLC region map: word after the name is 0x{zero:X}, expected 0"));
    }
    let num_tiles = c.u32("numTiles")? as usize;
    let tiles = (0..num_tiles).map(|_| c.tile()).collect::<Result<Vec<_>, _>>()?;
    let list2 = c.counted_records("region list2 count")?;
    let list3 = c.counted_records("region list3 count")?;
    c.end("DLC region map")?;
    Ok(RegionMap { region, header: None, tiles, list2, list3 })
}

fn put_u16(o: &mut Vec<u8>, v: u16) {
    o.extend_from_slice(&v.to_le_bytes());
}
fn put_u32(o: &mut Vec<u8>, v: u32) {
    o.extend_from_slice(&v.to_le_bytes());
}
fn put_f32s(o: &mut Vec<u8>, v: &[f32]) {
    for x in v {
        o.extend_from_slice(&x.to_le_bytes());
    }
}

fn put_tail(o: &mut Vec<u8>, t: &Tail) {
    for list in [&t.list1, &t.list2] {
        put_u32(o, list.len() as u32);
        for [p, q] in list {
            put_u32(o, *p);
            put_u32(o, *q);
        }
    }
    put_u32(o, t.x);
    for c in t.counts {
        put_u32(o, c);
    }
    put_u32(o, t.a.len() as u32);
    t.a.iter().for_each(|&v| put_u32(o, v));
    put_u32(o, t.nb);
    t.b.iter().for_each(|&v| put_u32(o, v));
    put_u32(o, t.c.len() as u32);
    for (id, vs) in &t.c {
        put_u32(o, *id);
        put_u32(o, vs.len() as u32);
        vs.iter().for_each(|&v| put_u32(o, v));
    }
}

fn put_record(o: &mut Vec<u8>, r: &Record) {
    put_u32(o, r.hash);
    put_u16(o, r.name.len() as u16);
    o.extend_from_slice(&r.name);
    put_f32s(o, &r.a);
    put_f32s(o, &r.b);
    put_u16(o, r.u0);
    put_u16(o, r.u1);
    put_tail(o, &r.tail);
}

fn put_records(o: &mut Vec<u8>, rs: &[Record]) {
    put_u32(o, rs.len() as u32);
    rs.iter().for_each(|r| put_record(o, r));
}

fn put_tile(o: &mut Vec<u8>, t: &Tile) {
    put_u32(o, t.hash);
    put_u16(o, t.name.len() as u16);
    o.extend_from_slice(&t.name);
    put_f32s(o, &t.a);
    put_f32s(o, &t.b);
    put_u16(o, t.u0);
    put_u16(o, t.sel as u16);
    if let Some(tail) = &t.tail {
        put_tail(o, tail);
    }
}

impl GlobalMap {
    /// Serialise. A tail's `nB` is written as stored; its low 16 bits must equal `b.len()`.
    pub fn write(&self) -> Result<Vec<u8>, String> {
        check_tails(self.list1.iter().chain(&self.list2).chain(&self.dynamics).map(|r| &r.tail))?;
        let mut o = Vec::new();
        put_u32(&mut o, MAGIC);
        put_u32(&mut o, self.dynamics.len() as u32);
        put_records(&mut o, &self.list1);
        put_records(&mut o, &self.list2);
        self.dynamics.iter().for_each(|r| put_record(&mut o, r));
        Ok(o)
    }
}

impl RegionMap {
    /// Serialise, in the main layout when `header` is `Some` and the DLC layout when it is `None`. A
    /// tile's `tail` must be present exactly when `sel >= 2`.
    pub fn write(&self) -> Result<Vec<u8>, String> {
        for (i, t) in self.tiles.iter().enumerate() {
            if t.tail.is_some() != (t.sel >= 2) {
                return Err(format!("tile {i}: sel {} with tail present = {}", t.sel, t.tail.is_some()));
            }
        }
        check_tails(
            self.tiles.iter().filter_map(|t| t.tail.as_ref()).chain(self.list2.iter().chain(&self.list3).map(|r| &r.tail)),
        )?;
        let mut o = Vec::new();
        put_u32(&mut o, MAGIC);
        o.extend_from_slice(&self.region);
        o.push(0);
        match &self.header {
            Some(h) => {
                if h.paths_header.is_some() != !h.paths.is_empty() {
                    return Err("region header: paths_header must be present exactly when there are paths".into());
                }
                put_u32(&mut o, self.tiles.len() as u32);
                put_u32(&mut o, h.unknown);
                put_u32(&mut o, h.paths.len() as u32);
                if let Some(ph) = h.paths_header {
                    put_u32(&mut o, ph);
                }
                for p in &h.paths {
                    for s in [&p.s0, &p.s1] {
                        put_u32(&mut o, s.len() as u32);
                        o.extend_from_slice(s);
                    }
                    o.extend_from_slice(&p.data);
                    put_u32(&mut o, p.s2.len() as u32);
                    o.extend_from_slice(&p.s2);
                }
                for bx in &h.boxes {
                    put_f32s(&mut o, bx);
                }
                for [p, q] in h.pairs {
                    put_u16(&mut o, p);
                    put_u16(&mut o, q);
                }
            }
            None => {
                put_u32(&mut o, 0);
                put_u32(&mut o, self.tiles.len() as u32);
            }
        }
        self.tiles.iter().for_each(|t| put_tile(&mut o, t));
        put_records(&mut o, &self.list2);
        put_records(&mut o, &self.list3);
        Ok(o)
    }
}

fn check_tails<'a>(tails: impl Iterator<Item = &'a Tail>) -> Result<(), String> {
    for t in tails {
        if (t.nb & 0xFFFF) as usize != t.b.len() {
            return Err(format!("tail: nB 0x{:X} does not match {} B entries", t.nb, t.b.len()));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tail(counts: [u32; 9]) -> Tail {
        Tail { list1: vec![[1, 2]], list2: vec![], x: 7, counts, a: vec![3], nb: 1, b: vec![4], c: vec![(5, vec![6, 7])] }
    }

    fn record(name: &str) -> Record {
        let mut n = name.as_bytes().to_vec();
        n.push(0);
        Record {
            hash: crate::pandemic_hash(name),
            name: n,
            a: [0.0; 3],
            b: [1.0, 2.0, 3.0],
            u0: 1,
            u1: 2,
            tail: tail([0, 1, 0, 0, 0, 0, 0, 0, 0]),
        }
    }

    #[test]
    fn global_round_trips() {
        let g = GlobalMap { list1: vec![record("A")], list2: vec![], dynamics: vec![record("KnifeThrow"), record("B")] };
        let bytes = g.write().unwrap();
        let back = parse_global(&bytes).unwrap();
        assert_eq!(back, g);
        assert_eq!(back.dynamics[0].name(), "KnifeThrow");
    }

    #[test]
    fn region_round_trips_in_both_layouts() {
        let tiles = vec![
            Tile { hash: 1, name: vec![], a: [0.0; 3], b: [0.0; 3], u0: 0, sel: 1, tail: None },
            Tile { hash: 2, name: b"T\0".to_vec(), a: [0.0; 3], b: [0.0; 3], u0: 0, sel: 2, tail: Some(tail([0; 9])) },
        ];
        let header = RegionHeader {
            unknown: 9,
            paths_header: Some(3),
            paths: vec![Path { s0: b"a".to_vec(), s1: vec![], data: [5; 48], s2: b"zz".to_vec() }],
            boxes: [[1.0; 6]; 3],
            pairs: [[1, 2]; 3],
        };
        let main = RegionMap { region: b"France".to_vec(), header: Some(header), tiles: tiles.clone(), list2: vec![record("X")], list3: vec![] };
        assert_eq!(parse_region_main(&main.write().unwrap()).unwrap(), main);
        let dlc = RegionMap { region: b"FRANCE".to_vec(), header: None, tiles, list2: vec![], list3: vec![record("Y")] };
        assert_eq!(parse_region_dlc(&dlc.write().unwrap()).unwrap(), dlc);
    }

    #[test]
    fn trailing_bytes_and_bad_magic_are_errors() {
        let mut bytes = GlobalMap { list1: vec![], list2: vec![], dynamics: vec![record("A")] }.write().unwrap();
        bytes.push(0);
        assert!(parse_global(&bytes).unwrap_err().contains("parse ends"));
        assert!(parse_global(&[0, 0, 0, 0]).unwrap_err().contains("magic"));
    }
}
