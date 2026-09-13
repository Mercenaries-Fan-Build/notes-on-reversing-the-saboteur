//! `SBL*` sub-pack — the outer container inside every 2008-05 Xbox 360 `.Pack` file.
//!
//! ## Family, not variants
//!
//! The 2008 build ships **seven** SBL-family magics: `SBL0`, `SBL4`, `SBL5`, `SBL6`, `SBL8`,
//! `SBLL`, `SBLX`. Empirically (a strict-invariant parse across every `.Pack` under `Bin360/`)
//! they use the **identical** container layout — 8-byte header, 24-byte records, contiguous
//! big-endian data — with only the last magic byte differing. All 4389 files of these seven
//! magics pass the same invariants:
//!
//! ```text
//!   SBL4 : 2612/2612 OK      SBL0 : 10/10 OK
//!   SBLL : 1265/1265 OK      SBL5 : 4/4 OK
//!   SBL8 : 234/234 OK        SBLX : 107/107 OK
//!   SBL6 : 157/157 OK
//! ```
//!
//! What the 4th magic byte encodes is unresolved (see the doc), but at the parser level it's
//! **cosmetic** — one parser, one record type, one call-site — so this module exposes a single
//! `SubPack` struct that carries the magic tag along for the ride. Callers that only need the
//! records ignore it; anyone who wants to key on magic can.
//!
//! The other 2008 pack magics (`MP00` for `mega*.pack` streamblocks, `AP04` for
//! `Animations.pack`) are a different family and refused with a clear error.
//!
//! **Why big-endian.** The prototype targets the Xbox 360, so the whole 2008 asset tree is
//! big-endian end-to-end. Ignore this and every offset comes out as gibberish — the 4-record
//! magic word at +0x04 (BE = 4, LE = 0x04000000) is the earmark.
//!
//! ## Byte layout (big-endian, verified over 4389/4389 SBL-family sub-packs under `Bin360/`)
//!
//! ```text
//! HEADER  (8 bytes)
//!   +0x00  char  magic[4] = one of {"SBL0","SBL4","SBL5","SBL6","SBL8","SBLL","SBLX"}
//!   +0x04  u32   nEntries
//!
//! DIRECTORY  (starts at +0x08; nEntries × 24 bytes)
//!   +0x00  u32   kind_hash     pandemic_hash of the slot type name
//!                              ("materials", "meshes", "physics", "textures", …)
//!   +0x04  u32   offset        ABSOLUTE file offset of this slot's bytes
//!   +0x08  u32   size          slot byte length
//!   +0x0C  u32   size_dup      duplicate of `size` (retail's `comp`/`uncomp` pair
//!                              collapses here because SBL* slots are stored
//!                              UNCOMPRESSED — no zlib wrapping at this level)
//!   +0x10  u32   flag          always 1 in every SBL-family sub-pack sampled
//!   +0x14  u32   block_hash    the containing block's hash; usually the same value
//!                              in every record of one file
//!
//! DATA  (starts at dir_end = 8 + nEntries*24)
//!   slot bytes back-to-back, in the SAME order as the directory:
//!     offset[0]   == dir_end
//!     offset[i]   == offset[i-1] + size[i-1]
//!     offset[n-1] + size[n-1] == file_end
//! ```
//!
//! ## How this differs from retail's `ALBS`/`SBLA` (see `tools/sab_sbla`, `docs/formats/sbla_subpack.md`)
//!
//! * **Endianness** — SBL* is big-endian; ALBS is little-endian.
//! * **Header** — 8 bytes vs ALBS's 32 (object) or 68 (streamblock). No name-crc, no aux
//!   fields, no reserved words.
//! * **Compression** — SBL* slots are stored raw. ALBS records carry `compSize` + `uncSize`
//!   because each blob is zlib-wrapped and the loader inflates on the fly. (The 2008 build
//!   does compress inside a slot — the meshes/textures slots contain their own mini-directories
//!   with zlib streams — but that inner structure is a separate format, not part of the outer
//!   SBL* container.)
//! * **Offset semantics** — SBL* offsets are absolute file offsets. ALBS offsets are
//!   `dir_end`-relative (getting this wrong was the `sab_sbla replace` bug fixed 2026-07-24;
//!   irrelevant here because the SBL* family side-steps it entirely).
//! * **Block hash placement** — SBL* stores it PER RECORD (24-byte record includes a
//!   `block_hash` field). ALBS keeps it in the sub-pack header.
//! * **No placeholders** — ALBS uses `hash==0` records as section boundaries that share the
//!   next record's offset. SBL* has no equivalent; the directory is a simple flat list.

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::pack::pandemic_hash;

/// The seven known SBL-family magics. A `.Pack` whose first 4 bytes match any of these is
/// parseable by this module; anything else (empty file, `MP00`, `AP04`, or a magic we
/// haven't catalogued yet) is refused so a mis-detected container never returns garbage.
///
/// Order is documentation-only; membership check is a linear scan of 7 entries.
pub const KNOWN_SUBPACK_MAGICS: &[&[u8; 4]] =
    &[b"SBL0", b"SBL4", b"SBL5", b"SBL6", b"SBL8", b"SBLL", b"SBLX"];

/// One directory record inside an SBL-family sub-pack: a single named slot.
#[derive(Clone, Debug)]
pub struct SubPackRecord {
    /// `pandemic_hash("<slot type>")` — the on-disk key. Matched against the
    /// slot-type table to fill `kind_name`.
    pub kind_hash: u32,
    /// Friendly name of the slot type when its hash resolves against the known
    /// table (`"materials"`, `"meshes"`, `"physics"`, `"textures"`, …); `None`
    /// for the handful of unrecognized hashes (mostly Animation/Streamblock
    /// packs whose non-standard slot names are still being catalogued).
    pub kind_name: Option<&'static str>,
    /// Absolute byte offset of this slot's payload inside the containing file.
    pub offset: usize,
    /// Byte length of the slot's payload.
    pub size: usize,
    /// The block hash the packer wrote alongside this record — usually the
    /// same value for every record in a file (see `has_consistent_block_hash`).
    pub block_hash: u32,
}

/// A parsed SBL-family sub-pack. Owns only the parsed metadata; the caller keeps the
/// underlying bytes and slices `records[i].offset..records[i].offset+size` on demand.
#[derive(Clone, Debug)]
pub struct SubPack {
    /// The 4-byte magic this file was tagged with (`b"SBL4"`, `b"SBLL"`, …). Exposed so
    /// callers that want to log or split by variant can, without re-reading the file.
    pub magic: [u8; 4],
    pub records: Vec<SubPackRecord>,
    /// Block hash from the first record — the value the packer treats as
    /// canonical for this file. See `has_consistent_block_hash` if you need
    /// to know whether every record agrees.
    pub block_hash: u32,
    /// Total bytes consumed = data ends at `file_size` and every record's
    /// `offset+size` is `<=` this. Used for the coverage assertion.
    pub file_size: usize,
}

/// Legacy alias — most callers wrote against `Sbl4SubPack` before the widening. The type is
/// identical; keeping the alias means neither the type name nor the field access changes for
/// the existing test files.
pub type Sbl4SubPack = SubPack;
pub type Sbl4Record = SubPackRecord;

impl SubPack {
    /// Parse an SBL-family sub-pack from raw bytes. Fails cleanly on wrong magic,
    /// bad entry counts, non-contiguous data blocks, or truncated files.
    pub fn parse(buf: &[u8]) -> Result<Self, String> {
        if buf.len() < 8 {
            return Err(format!("SubPack: file too short ({} bytes)", buf.len()));
        }
        // Magic check: must be exactly one of the seven catalogued SBL-family tags.
        // A raw `SBL` prefix isn't enough — a hypothetical `SBLZ` file would fail invariants
        // in ways we haven't sampled, and we'd rather refuse than guess.
        let magic = [buf[0], buf[1], buf[2], buf[3]];
        if !KNOWN_SUBPACK_MAGICS.iter().any(|m| **m == magic) {
            return Err(format!(
                "SubPack: unknown magic {magic:02X?} (expected one of {:?})",
                KNOWN_SUBPACK_MAGICS
                    .iter()
                    .map(|m| std::str::from_utf8(m.as_slice()).unwrap_or("?"))
                    .collect::<Vec<_>>()
            ));
        }
        let n = u32be(buf, 4)? as usize;
        // Real 2008 SBL-family sub-packs have between 1 and 17 records (histogram is
        // dominated by 4-record SBL4 blocks and 2-record SBLL blocks). A wildly large
        // count means we're reading a different container — refuse rather than allocate GBs.
        if n == 0 || n > 256 {
            return Err(format!("SubPack: implausible nEntries = {n}"));
        }
        let dir_end = 8 + n * 24;
        if dir_end > buf.len() {
            return Err(format!(
                "SubPack: directory of {n} × 24 bytes overruns file ({} bytes)",
                buf.len()
            ));
        }

        let mut recs = Vec::with_capacity(n);
        // The directory is laid out in offset order and every real SBL-family sub-pack
        // packs data contiguously, so we check the running cursor as we go. A
        // mismatch usually means we're being handed a different container with a matching
        // magic patched in — reject instead of silently returning nonsense records.
        let mut expected = dir_end as u64;
        for i in 0..n {
            let o = 8 + i * 24;
            let kind_hash = u32be(buf, o)?;
            let offset = u32be(buf, o + 4)? as u64;
            let size = u32be(buf, o + 8)? as u64;
            let size_dup = u32be(buf, o + 12)?;
            let flag = u32be(buf, o + 16)?;
            let block_hash = u32be(buf, o + 20)?;

            if offset != expected {
                return Err(format!(
                    "SubPack: record {i} offset 0x{offset:X} != expected 0x{expected:X} (data \
                     region not contiguous)"
                ));
            }
            if offset + size > buf.len() as u64 {
                return Err(format!(
                    "SubPack: record {i} slot [0x{offset:X}..0x{end:X}) overruns file ({} bytes)",
                    buf.len(),
                    end = offset + size
                ));
            }
            if size as u32 != size_dup {
                // Every sub-pack sampled had `size == size_dup`. If the game
                // ever ships a variant where they diverge, we want to know
                // rather than assume — reject explicitly.
                return Err(format!(
                    "SubPack: record {i} size 0x{size:X} != size_dup 0x{size_dup:X} (expected \
                     equal in every sampled sub-pack)"
                ));
            }
            let _ = flag;

            recs.push(SubPackRecord {
                kind_hash,
                kind_name: slot_name_for_hash(kind_hash),
                offset: offset as usize,
                size: size as usize,
                block_hash,
            });
            expected = offset + size;
        }
        // The data region MUST cover to end-of-file. The whole point of this
        // check is that the current `Prototype2008Source::read_mesh_bytes`
        // silently returned garbage when it read past the wrong end — refusing
        // here is what surfaces the mismatch.
        if expected as usize != buf.len() {
            return Err(format!(
                "SubPack: data ends at 0x{expected:X} but file is {} bytes (0x{:X}) — \
                 gap/overhang of {} bytes",
                buf.len(),
                buf.len(),
                buf.len() as i64 - expected as i64
            ));
        }

        let block_hash = recs[0].block_hash;
        Ok(SubPack { magic, records: recs, block_hash, file_size: buf.len() })
    }

    /// The 4-byte magic as an ASCII string. Convenience for logging; the raw bytes are
    /// always ASCII 0x30..0x5A on the known set.
    pub fn magic_str(&self) -> &str {
        std::str::from_utf8(&self.magic).unwrap_or("????")
    }

    /// Find the record for a given slot type (`"meshes"`, `"textures"`, …). Matched
    /// by `pandemic_hash` so misspelling `MESHES` still resolves the same slot.
    pub fn find(&self, kind: &str) -> Option<&SubPackRecord> {
        let h = pandemic_hash(kind);
        self.records.iter().find(|r| r.kind_hash == h)
    }

    /// Slice this record's payload out of the underlying sub-pack bytes. Returns `None` if
    /// `buf` is shorter than what the sub-pack was parsed from (a defense against passing the
    /// wrong buffer back in).
    pub fn slot_bytes<'a>(&self, rec: &SubPackRecord, buf: &'a [u8]) -> Option<&'a [u8]> {
        let end = rec.offset.checked_add(rec.size)?;
        if end > buf.len() {
            return None;
        }
        Some(&buf[rec.offset..end])
    }

    /// True iff every record's `block_hash` word agreed. Some sub-packs — animation banks and
    /// giant streamblocks — put different values in each record; exposing the flag lets callers
    /// decide whether to trust `self.block_hash` as an identity.
    pub fn has_consistent_block_hash(&self) -> bool {
        self.records.iter().all(|r| r.block_hash == self.block_hash)
    }
}

// ── slot-name table ──────────────────────────────────────────────────────────

/// Every 2008-build slot-name string we've decoded so far. Extended as new blocks
/// surface — a `.pack` whose records don't resolve to a name here is still parseable,
/// it just shows raw hashes in the UI until we identify the string.
///
/// The four canonical types (materials/meshes/physics/textures) are ubiquitous and
/// verified by hashing them against every record hash across all SBL-family sub-packs in
/// `Bin360/`. Others are still open — see `docs/2008_subpack_variants.md`.
const KNOWN_SLOT_NAMES: &[&str] = &["materials", "meshes", "physics", "textures"];

fn slot_name_for_hash(h: u32) -> Option<&'static str> {
    // Lazy-populated so `pandemic_hash` runs exactly once per slot name per process.
    static TABLE: OnceLock<HashMap<u32, &'static str>> = OnceLock::new();
    let map = TABLE.get_or_init(|| KNOWN_SLOT_NAMES.iter().map(|s| (pandemic_hash(s), *s)).collect());
    map.get(&h).copied()
}

// ── little utilities ─────────────────────────────────────────────────────────

fn u32be(buf: &[u8], off: usize) -> Result<u32, String> {
    if off + 4 > buf.len() {
        return Err(format!(
            "SubPack: read of u32 at 0x{off:X} overruns buffer ({} bytes)",
            buf.len()
        ));
    }
    Ok(u32::from_be_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Verified by hand-parsing the header of Global/Act1_IntKey.Pack:
    ///   magic=SBL4  nEntries=4
    ///   rec0 materials  off=104   size=310    (starts with WSAO)
    ///   rec1 meshes     off=414   size=11266  (starts with count/rec/zlib(MSH4))
    ///   rec2 physics    off=11680 size=370
    ///   rec3 textures   off=12050 size=987151
    /// file total 999201; last slot ends at 12050+987151 == 999201 ✓
    /// The hand-parse checked all these numbers agree with the raw hex dump.
    const KNOWN_ACT1_INTKEY_LAYOUT: &[(&str, u32, u32)] = &[
        ("materials", 104, 310),
        ("meshes", 414, 11266),
        ("physics", 11680, 370),
        ("textures", 12050, 987151),
    ];

    fn bin360() -> Option<PathBuf> {
        let p = PathBuf::from("c:/Users/Shadow/Desktop/notes-on-reversing-the-sabetour/game-files/Bin360");
        if p.is_dir() { Some(p) } else { None }
    }

    fn act1_intkey_bytes() -> Option<Vec<u8>> {
        let p = bin360()?.join("Global").join("Act1_IntKey.Pack");
        if !p.is_file() {
            return None;
        }
        std::fs::read(&p).ok()
    }

    /// Parses a real SBL4 file and matches the hand-verified layout above.
    #[test]
    fn parses_act1_intkey_pack() {
        let Some(bytes) = act1_intkey_bytes() else {
            eprintln!("skip: Act1_IntKey.Pack not present");
            return;
        };
        let sub = SubPack::parse(&bytes).expect("parse SBL4");
        assert_eq!(&sub.magic, b"SBL4");
        assert_eq!(sub.records.len(), KNOWN_ACT1_INTKEY_LAYOUT.len());
        for (rec, (name, exp_off, exp_sz)) in sub.records.iter().zip(KNOWN_ACT1_INTKEY_LAYOUT) {
            assert_eq!(rec.kind_name, Some(*name), "record kind_name for {name}");
            assert_eq!(rec.offset as u32, *exp_off, "{name} offset");
            assert_eq!(rec.size as u32, *exp_sz, "{name} size");
            assert_eq!(rec.kind_hash, pandemic_hash(name), "{name} hash matches kernel");
        }
        assert!(sub.has_consistent_block_hash());
        assert_eq!(sub.file_size, bytes.len());
    }

    /// The `find()` helper resolves a slot name via `pandemic_hash` and slices bytes cleanly.
    /// Materials in Act1_IntKey start with the ASCII magic `WSAO` — the same magic retail's
    /// materials use — which is the earmark that we sliced the right region.
    #[test]
    fn find_and_slice_materials_slot() {
        let Some(bytes) = act1_intkey_bytes() else {
            eprintln!("skip: Act1_IntKey.Pack not present");
            return;
        };
        let sub = SubPack::parse(&bytes).expect("parse SBL4");
        let mat = sub.find("materials").expect("materials slot present");
        let mat_bytes = sub.slot_bytes(mat, &bytes).expect("slice materials");
        assert_eq!(mat_bytes.len(), 310);
        assert_eq!(&mat_bytes[0..4], b"WSAO", "materials slot starts with WSAO magic");
    }

    /// Refuse a truncated header + refuse an out-of-family magic (`AP04` = Animations.pack).
    /// The parser is the last thing standing between a mis-detected container and the caller
    /// getting bogus records.
    #[test]
    fn rejects_wrong_magic_and_truncation() {
        assert!(SubPack::parse(&[]).is_err());
        assert!(SubPack::parse(&[0u8; 4]).is_err());
        // Right length header but non-SBL magic (Animations.pack).
        assert!(SubPack::parse(b"AP04\x00\x00\x00\x01").is_err());
        // Right length header, mega streamblock magic.
        assert!(SubPack::parse(b"MP00\x00\x00\x00\x01").is_err());
        // SBL4 magic + real nEntries but the directory itself is truncated.
        let mut buf = Vec::from(*b"SBL4");
        buf.extend_from_slice(&4u32.to_be_bytes());
        buf.extend_from_slice(&[0u8; 24]);
        assert!(SubPack::parse(&buf).is_err());
    }

    /// A synthetic 2-record SBL4 (materials + meshes) round-trips through the parser and
    /// hands back exactly the slot bytes we packed in. This is the deterministic control
    /// for [`parses_act1_intkey_pack`] — it runs on machines with no prototype install.
    #[test]
    fn parses_a_hand_crafted_two_slot_sub_pack() {
        let mat_payload = b"WSAO\x00\x00\x00\x00hello-materials".to_vec();
        let mesh_payload = b"\x00\x00\x00\x00hello-meshes".to_vec();
        let dir_end = 8 + 2 * 24;
        let mat_off = dir_end;
        let mesh_off = mat_off + mat_payload.len();

        let mut buf = Vec::new();
        buf.extend_from_slice(b"SBL4");
        buf.extend_from_slice(&2u32.to_be_bytes());
        // record 0: materials
        buf.extend_from_slice(&pandemic_hash("materials").to_be_bytes());
        buf.extend_from_slice(&(mat_off as u32).to_be_bytes());
        buf.extend_from_slice(&(mat_payload.len() as u32).to_be_bytes());
        buf.extend_from_slice(&(mat_payload.len() as u32).to_be_bytes());
        buf.extend_from_slice(&1u32.to_be_bytes());
        buf.extend_from_slice(&0xDEADBEEFu32.to_be_bytes());
        // record 1: meshes
        buf.extend_from_slice(&pandemic_hash("meshes").to_be_bytes());
        buf.extend_from_slice(&(mesh_off as u32).to_be_bytes());
        buf.extend_from_slice(&(mesh_payload.len() as u32).to_be_bytes());
        buf.extend_from_slice(&(mesh_payload.len() as u32).to_be_bytes());
        buf.extend_from_slice(&1u32.to_be_bytes());
        buf.extend_from_slice(&0xDEADBEEFu32.to_be_bytes());
        // data
        buf.extend_from_slice(&mat_payload);
        buf.extend_from_slice(&mesh_payload);

        let sub = SubPack::parse(&buf).expect("parse synthetic SBL4");
        assert_eq!(sub.records.len(), 2);
        let m = sub.find("materials").unwrap();
        assert_eq!(sub.slot_bytes(m, &buf).unwrap(), mat_payload.as_slice());
        let msh = sub.find("meshes").unwrap();
        assert_eq!(sub.slot_bytes(msh, &buf).unwrap(), mesh_payload.as_slice());
        assert_eq!(sub.block_hash, 0xDEADBEEF);
        assert!(sub.has_consistent_block_hash());
    }

    // ── variant tests — one per non-SBL4 magic, against a real file in the corpus ─────
    //
    // Each of these opens a specific `.Pack` of that variant, parses it with the SAME
    // `SubPack::parse`, and checks the round-trip: magic tag as expected, records
    // enumerable, a known slot's payload sliceable and starts with the right earmark.
    //
    // The specific files were picked because they're small (fast to read) and their layouts
    // were hand-verified from the initial hex dump — the offset / size numbers below match
    // those dumps byte-for-byte.

    /// SBLL is the most common non-SBL4 variant (1265 files). All 1257 3-slot instances hold
    /// (materials, meshes, …); a tiny 40-byte 1-slot variant carries only the `0x9AB5A351`
    /// unknown-type slot. Both are the same shape.
    ///
    /// Sample: `France/327680.Pack` — 40 bytes total; single record, unknown slot type.
    /// Header dump:
    ///   0000  53 42 4C 4C 00 00 00 01 9A B5 A3 51 00 00 00 20   SBLL.......Q...
    ///   0010  00 00 00 08 00 00 00 08 00 00 00 01 00 00 00 00   ................
    ///   0020  00 00 00 00 00 00 00 00                           ........
    /// Payload is 8 zero bytes at offset 0x20 = dir_end.
    #[test]
    fn sbll_variant_parses_single_slot_pack() {
        let Some(b) = bin360() else {
            eprintln!("skip: no Bin360/");
            return;
        };
        let bytes = std::fs::read(b.join("France").join("327680.Pack")).expect("read SBLL sample");
        let sub = SubPack::parse(&bytes).expect("parse SBLL");
        assert_eq!(&sub.magic, b"SBLL");
        assert_eq!(sub.magic_str(), "SBLL");
        assert_eq!(sub.records.len(), 1);
        assert_eq!(sub.records[0].kind_hash, 0x9AB5A351);
        assert_eq!(sub.records[0].offset, 0x20);
        assert_eq!(sub.records[0].size, 8);
        assert_eq!(sub.slot_bytes(&sub.records[0], &bytes).unwrap(), &[0u8; 8]);
    }

    /// SBLL's dominant shape (1257/1265 files) is `materials + meshes`. Sample below
    /// starts with the WSAO magic on the materials slot — exact same 4-record shape as
    /// retail materials, confirming the SBL* container is just a shell.
    #[test]
    fn sbll_variant_materials_slot_starts_with_wsao() {
        let Some(b) = bin360() else {
            eprintln!("skip: no Bin360/");
            return;
        };
        // Any 2-record SBLL from Global/ works. Sean_material.Pack — pick one that exists
        // and is dominated by its materials slot; picking a shallow one keeps the read cheap.
        // We iterate to find the first SBLL under Global/ rather than hard-coding a name
        // because packlog filenames aren't stable across corpus snapshots.
        let mut used = None;
        for entry in walkdir(&b.join("Global")).into_iter().take(20000) {
            if entry.extension().and_then(|s| s.to_str()).map(|s| s.eq_ignore_ascii_case("pack")).unwrap_or(false) {
                if let Ok(head) = std::fs::read(&entry) {
                    if head.len() >= 8 && &head[..4] == b"SBLL" {
                        if let Ok(sub) = SubPack::parse(&head) {
                            if sub.find("materials").is_some() {
                                used = Some((entry.clone(), head, sub));
                                break;
                            }
                        }
                    }
                }
            }
        }
        let Some((path, bytes, sub)) = used else {
            eprintln!("skip: no 2-record SBLL with a materials slot under Global/");
            return;
        };
        let mat = sub.find("materials").unwrap();
        let mat_bytes = sub.slot_bytes(mat, &bytes).unwrap();
        assert_eq!(&mat_bytes[0..4], b"WSAO", "materials slot at {path:?} should start with WSAO");
    }

    /// SBL8 is uniformly 4-record (all 234 files) and always carries the same slot set as SBL4
    /// (materials + meshes + physics + textures). Same shape as SBL4, different magic byte.
    ///
    /// Sample: `Global/A1Esc_Human_SS_Grenadier_RPG.Pack` (6715 bytes).
    /// Header dump:
    ///   magic=SBL8  nEntries=4
    ///   rec0: kind=0xA40D777D(textures) off=0x68 size=0x16DA
    ///   rec1: kind=0xD72829F9(materials) off=0x1742 size=0x127
    ///   rec2: kind=0xFE5E3A56(meshes)    off=0x1869 size=0x1A1
    ///   rec3: kind=0x4445EA18(physics)   off=0x1A0A size=0x31
    ///   block_hash = 0x36EC2E15 on every record
    #[test]
    fn sbl8_variant_parses_known_pack() {
        let Some(b) = bin360() else {
            eprintln!("skip: no Bin360/");
            return;
        };
        let path = b.join("Global").join("A1Esc_Human_SS_Grenadier_RPG.Pack");
        let Ok(bytes) = std::fs::read(&path) else {
            eprintln!("skip: {path:?} not present");
            return;
        };
        let sub = SubPack::parse(&bytes).expect("parse SBL8");
        assert_eq!(&sub.magic, b"SBL8");
        assert_eq!(sub.records.len(), 4);
        assert_eq!(sub.records[0].kind_name, Some("textures"));
        assert_eq!(sub.records[0].offset, 0x68);
        assert_eq!(sub.records[0].size, 0x16DA);
        assert_eq!(sub.records[1].kind_name, Some("materials"));
        assert_eq!(sub.block_hash, 0x36EC2E15);
        assert!(sub.has_consistent_block_hash());
        // Materials slot earmark
        let mat = sub.find("materials").unwrap();
        let mb = sub.slot_bytes(mat, &bytes).unwrap();
        assert_eq!(&mb[0..4], b"WSAO");
    }

    /// SBL6 comes in three shapes — 1-rec (92 files), 4-rec (55), a handful of 2/3-rec.
    /// Same shape rules on all.
    ///
    /// Sample: `France/1239297.Pack` (164 bytes, 3-rec) — the smallest multi-record SBL6.
    #[test]
    fn sbl6_variant_parses_multi_record_pack() {
        let Some(b) = bin360() else {
            eprintln!("skip: no Bin360/");
            return;
        };
        let path = b.join("France").join("1239297.Pack");
        let Ok(bytes) = std::fs::read(&path) else {
            eprintln!("skip: {path:?} not present");
            return;
        };
        let sub = SubPack::parse(&bytes).expect("parse SBL6");
        assert_eq!(&sub.magic, b"SBL6");
        assert_eq!(sub.records.len(), 3);
        // From the hex dump: rec offsets 0x50, 0x58, 0x7C; sizes 8, 0x24, 0x28.
        assert_eq!(sub.records[0].offset, 0x50);
        assert_eq!(sub.records[0].size, 8);
        assert_eq!(sub.records[1].offset, 0x58);
        assert_eq!(sub.records[1].size, 0x24);
        assert_eq!(sub.records[2].offset, 0x7C);
        assert_eq!(sub.records[2].size, 0x28);
        // Last record ends at 0x7C + 0x28 = 0xA4 = 164 = file size
        assert_eq!(sub.records[2].offset + sub.records[2].size, sub.file_size);
    }

    /// SBLX is 4-rec (93/107) and centers on effect / squib packs. Sample below is
    /// `Global/barrel_of_boom_massivePush.Pack` (21197 bytes) — a small but complete example.
    #[test]
    fn sblx_variant_parses_known_pack() {
        let Some(b) = bin360() else {
            eprintln!("skip: no Bin360/");
            return;
        };
        let path = b.join("Global").join("barrel_of_boom_massivePush.Pack");
        let Ok(bytes) = std::fs::read(&path) else {
            eprintln!("skip: {path:?} not present");
            return;
        };
        let sub = SubPack::parse(&bytes).expect("parse SBLX");
        assert_eq!(&sub.magic, b"SBLX");
        assert_eq!(sub.records.len(), 4);
        // From the hex dump: rec 0 is textures at 0x68, rec 1 materials at 0x27F5.
        assert_eq!(sub.records[0].kind_name, Some("textures"));
        assert_eq!(sub.records[0].offset, 0x68);
        assert_eq!(sub.records[0].size, 0x278D);
        assert_eq!(sub.records[1].kind_name, Some("materials"));
        assert_eq!(sub.records[1].offset, 0x27F5);
        // Sanity: materials slot starts with WSAO magic.
        let mat = sub.find("materials").unwrap();
        assert_eq!(&sub.slot_bytes(mat, &bytes).unwrap()[0..4], b"WSAO");
    }

    /// SBL0 is small (10 files); most are single-record `textures`-only packs.
    ///
    /// Sample: `Global/Dock_Post0.Pack` (60 bytes, 1-rec `textures`).
    #[test]
    fn sbl0_variant_parses_single_slot_textures() {
        let Some(b) = bin360() else {
            eprintln!("skip: no Bin360/");
            return;
        };
        let path = b.join("Global").join("Dock_Post0.Pack");
        let Ok(bytes) = std::fs::read(&path) else {
            eprintln!("skip: {path:?} not present");
            return;
        };
        let sub = SubPack::parse(&bytes).expect("parse SBL0");
        assert_eq!(&sub.magic, b"SBL0");
        assert_eq!(sub.records.len(), 1);
        assert_eq!(sub.records[0].kind_name, Some("textures"));
        // rec 0: off=0x20 size=0x1C — 28 bytes of payload starting at dir_end
        assert_eq!(sub.records[0].offset, 0x20);
        assert_eq!(sub.records[0].size, 0x1C);
        assert_eq!(sub.records[0].block_hash, 0x092D3CFF);
    }

    /// SBL5 is tiny (4 files) but includes the SultanOfSwing / France DLC blocks. Sample below
    /// is `SultanOfSwing/16776960.Pack` (4385 bytes, 3-rec: materials + meshes + `0x9AB5A351`).
    #[test]
    fn sbl5_variant_parses_known_pack() {
        let Some(b) = bin360() else {
            eprintln!("skip: no Bin360/");
            return;
        };
        let path = b.join("SultanOfSwing").join("16776960.Pack");
        let Ok(bytes) = std::fs::read(&path) else {
            eprintln!("skip: {path:?} not present");
            return;
        };
        let sub = SubPack::parse(&bytes).expect("parse SBL5");
        assert_eq!(&sub.magic, b"SBL5");
        assert_eq!(sub.records.len(), 3);
        // rec 0: materials off=0x50 size=0x13E (starts with WSAO/MTLD)
        assert_eq!(sub.records[0].kind_name, Some("materials"));
        assert_eq!(sub.records[0].offset, 0x50);
        assert_eq!(sub.records[0].size, 0x13E);
        let mat_bytes = sub.slot_bytes(&sub.records[0], &bytes).unwrap();
        assert_eq!(&mat_bytes[0..4], b"WSAO");
        assert_eq!(&mat_bytes[8..12], b"MTLD");
        assert_eq!(sub.records[1].kind_name, Some("meshes"));
    }

    /// Corpus-wide sanity check: every SBL-family .Pack under Bin360/ must parse cleanly with
    /// the widened parser. If this ever regresses to skipping a whole variant, the count is
    /// the alarm bell.
    ///
    /// The invariant we care about: for each of the seven magics, parse-success rate is
    /// **exactly** 100% — because the invariant validator (scratchpad/validate_shape.py) has
    /// already established that every file passes the shape rules.
    #[test]
    fn corpus_wide_sbl_family_parses_100_percent() {
        let Some(b) = bin360() else {
            eprintln!("skip: no Bin360/");
            return;
        };
        let mut per_magic_total: HashMap<[u8; 4], u32> = HashMap::new();
        let mut per_magic_ok: HashMap<[u8; 4], u32> = HashMap::new();
        for path in walkdir(&b) {
            if !path
                .extension()
                .and_then(|s| s.to_str())
                .map(|s| s.eq_ignore_ascii_case("pack"))
                .unwrap_or(false)
            {
                continue;
            }
            let Ok(bytes) = std::fs::read(&path) else { continue };
            if bytes.len() < 4 {
                continue;
            }
            let magic: [u8; 4] = [bytes[0], bytes[1], bytes[2], bytes[3]];
            if !KNOWN_SUBPACK_MAGICS.iter().any(|m| **m == magic) {
                continue;
            }
            *per_magic_total.entry(magic).or_default() += 1;
            if SubPack::parse(&bytes).is_ok() {
                *per_magic_ok.entry(magic).or_default() += 1;
            }
        }
        let mut fails = Vec::new();
        for (magic, total) in &per_magic_total {
            let ok = *per_magic_ok.get(magic).unwrap_or(&0);
            let tag = std::str::from_utf8(magic).unwrap_or("?");
            eprintln!("[corpus] {tag}: {ok}/{total} parse OK");
            if ok != *total {
                fails.push(format!("{tag}: {ok}/{total}"));
            }
        }
        assert!(fails.is_empty(), "sub-pack corpus regression: {fails:?}");
        // Also insist the SBL4 count is what the docs recorded, so any silent corpus swap gets
        // caught here rather than in a downstream test.
        assert_eq!(per_magic_total.get(b"SBL4").copied().unwrap_or(0), 2612, "SBL4 total");
        assert_eq!(per_magic_total.get(b"SBLL").copied().unwrap_or(0), 1265, "SBLL total");
        assert_eq!(per_magic_total.get(b"SBL8").copied().unwrap_or(0), 234, "SBL8 total");
        assert_eq!(per_magic_total.get(b"SBL6").copied().unwrap_or(0), 157, "SBL6 total");
        assert_eq!(per_magic_total.get(b"SBLX").copied().unwrap_or(0), 107, "SBLX total");
        assert_eq!(per_magic_total.get(b"SBL0").copied().unwrap_or(0), 10, "SBL0 total");
        assert_eq!(per_magic_total.get(b"SBL5").copied().unwrap_or(0), 4, "SBL5 total");
    }

    /// Small recursive walker used by the corpus tests — avoids depending on `walkdir`.
    fn walkdir(root: &std::path::Path) -> Vec<std::path::PathBuf> {
        let mut out = Vec::new();
        fn go(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            let Ok(rd) = std::fs::read_dir(dir) else { return };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    go(&p, out);
                } else {
                    out.push(p);
                }
            }
        }
        go(root, &mut out);
        out
    }
}
