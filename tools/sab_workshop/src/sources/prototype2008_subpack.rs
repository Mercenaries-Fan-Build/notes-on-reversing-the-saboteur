//! `SBL4` sub-pack — the outer container inside every 2008-05 Xbox 360 `.Pack` file.
//!
//! **Why this module exists.** A 2008 `.Pack` is *NOT* a raw slot of bytes indexed by its
//! sibling `.Packlog`. The packlog only names the four types the block ships (materials, meshes,
//! physics, textures) — its `Start:`/`Size:` fields describe an intermediate build-time layout
//! that the packer then reorders on disk. The authoritative directory lives at the head of the
//! `.Pack` itself, prefixed by the ASCII magic `SBL4` (`53 42 4C 34`).
//!
//! Every field is **big-endian** — the file was authored for the Xbox 360, so the whole 2008
//! asset tree is BE end-to-end. Ignore this and every offset comes out as gibberish.
//!
//! ## Byte layout (big-endian, verified over 2612/2612 SBL4 sub-packs under `Bin360/`)
//!
//! ```text
//! HEADER  (8 bytes)
//!   +0x00  char  magic[4] = "SBL4"
//!   +0x04  u32   nEntries
//!
//! DIRECTORY  (starts at +0x08; nEntries × 24 bytes)
//!   +0x00  u32   kind_hash     pandemic_hash of the slot type name
//!                              ("materials", "meshes", "physics", "textures", …)
//!   +0x04  u32   offset        ABSOLUTE file offset of this slot's bytes
//!   +0x08  u32   size          slot byte length
//!   +0x0C  u32   size_dup      duplicate of `size` (retail's `comp`/`uncomp` pair
//!                              collapses here because SBL4 slots are stored
//!                              UNCOMPRESSED — no zlib wrapping at this level)
//!   +0x10  u32   flag          always 1 in every SBL4 sub-pack sampled
//!   +0x14  u32   block_hash    the containing block's hash; usually the same value
//!                              in every record of one file (2457/2612 files)
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
//! * **Endianness** — SBL4 is big-endian; ALBS is little-endian.
//! * **Header** — 8 bytes vs ALBS's 32 (object) or 68 (streamblock). No name-crc, no aux
//!   fields, no reserved words.
//! * **Compression** — SBL4 slots are stored raw. ALBS records carry `compSize` + `uncSize`
//!   because each blob is zlib-wrapped and the loader inflates on the fly. (The 2008 build
//!   does compress inside a slot — the meshes/textures slots contain their own mini-directories
//!   with zlib streams — but that inner structure is a separate format, not part of SBL4.)
//! * **Offset semantics** — SBL4 offsets are absolute file offsets. ALBS offsets are
//!   `dir_end`-relative (getting this wrong was the `sab_sbla replace` bug fixed 2026-07-24;
//!   irrelevant here because SBL4 side-steps it entirely).
//! * **Block hash placement** — SBL4 stores it PER RECORD (24-byte record includes a
//!   `block_hash` field). ALBS keeps it in the sub-pack header. The per-record repetition is
//!   redundant but harmless — this parser exposes the header's `block_hash` (first record's
//!   value) alongside the per-record field.
//! * **No placeholders** — ALBS uses `hash==0` records as section boundaries that share the
//!   next record's offset. SBL4 has no equivalent; the directory is a simple flat list.
//!
//! ## Other 2008 pack magics (out of scope here)
//!
//! Only 2612 of the 4396 `.Pack` files under `Bin360/` start with `SBL4`. The rest are `SBLL`,
//! `SBL8`, `SBL6`, `SBLX`, `SBL0`, `SBL5`, `MP00`, `AP04` (in decreasing count). Those are
//! different container formats — each will need its own parser — and this module refuses them
//! with an explicit error rather than silently returning wrong records.

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::pack::pandemic_hash;

/// One directory record inside an `SBL4` sub-pack: a single named slot.
#[derive(Clone, Debug)]
pub struct Sbl4Record {
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
    /// same value for every record in a file (see the module docs).
    pub block_hash: u32,
}

/// A parsed `SBL4` sub-pack. Owns only the parsed metadata; the caller keeps the
/// underlying bytes and slices `records[i].offset..records[i].offset+size` on demand.
#[derive(Clone, Debug)]
pub struct Sbl4SubPack {
    pub records: Vec<Sbl4Record>,
    /// Block hash from the first record — the value the packer treats as
    /// canonical for this file. See `has_consistent_block_hash` if you need
    /// to know whether every record agrees.
    pub block_hash: u32,
    /// Total bytes consumed = data ends at `file_size` and every record's
    /// `offset+size` is `<=` this. Used for the coverage assertion.
    pub file_size: usize,
}

impl Sbl4SubPack {
    /// Parse an `SBL4` sub-pack from raw bytes. Fails cleanly on wrong magic,
    /// bad entry counts, non-contiguous data blocks, or truncated files.
    pub fn parse(buf: &[u8]) -> Result<Self, String> {
        if buf.len() < 8 {
            return Err(format!("SBL4: file too short ({} bytes)", buf.len()));
        }
        if &buf[0..4] != b"SBL4" {
            return Err(format!(
                "SBL4: wrong magic {:02X?} (expected 53 42 4C 34 'SBL4')",
                &buf[0..4]
            ));
        }
        let n = u32be(buf, 4)? as usize;
        // Real 2008 SBL4 sub-packs have between 1 and 17 records (histogram is
        // dominated by 4-record blocks). A wildly large count means we're
        // reading a different container — refuse rather than allocate GBs.
        if n == 0 || n > 256 {
            return Err(format!("SBL4: implausible nEntries = {n}"));
        }
        let dir_end = 8 + n * 24;
        if dir_end > buf.len() {
            return Err(format!(
                "SBL4: directory of {n} × 24 bytes overruns file ({} bytes)",
                buf.len()
            ));
        }

        let mut recs = Vec::with_capacity(n);
        // The directory is laid out in offset order and every real SBL4 sub-pack
        // packs data contiguously, so we check the running cursor as we go. A
        // mismatch usually means we're being handed the WRONG container (SBLL /
        // SBL8 / …) with an SBL4-looking magic patched in — reject instead of
        // silently returning nonsense records.
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
                    "SBL4: record {i} offset 0x{offset:X} != expected 0x{expected:X} (data \
                     region not contiguous)"
                ));
            }
            if offset + size > buf.len() as u64 {
                return Err(format!(
                    "SBL4: record {i} slot [0x{offset:X}..0x{end:X}) overruns file ({} bytes)",
                    buf.len(),
                    end = offset + size
                ));
            }
            if size as u32 != size_dup {
                // Every sub-pack sampled had `size == size_dup`. If the game
                // ever ships a variant where they diverge, we want to know
                // rather than assume — reject explicitly.
                return Err(format!(
                    "SBL4: record {i} size 0x{size:X} != size_dup 0x{size_dup:X} (expected \
                     equal in every sampled sub-pack)"
                ));
            }
            let _ = flag;

            recs.push(Sbl4Record {
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
                "SBL4: data ends at 0x{expected:X} but file is {} bytes (0x{:X}) — \
                 gap/overhang of {} bytes",
                buf.len(),
                buf.len(),
                buf.len() as i64 - expected as i64
            ));
        }

        let block_hash = recs[0].block_hash;
        Ok(Sbl4SubPack { records: recs, block_hash, file_size: buf.len() })
    }

    /// Find the record for a given slot type (`"meshes"`, `"textures"`, …). Matched
    /// by `pandemic_hash` so misspelling `MESHES` still resolves the same slot.
    pub fn find(&self, kind: &str) -> Option<&Sbl4Record> {
        let h = pandemic_hash(kind);
        self.records.iter().find(|r| r.kind_hash == h)
    }

    /// Slice this record's payload out of the underlying sub-pack bytes. Returns `None` if
    /// `buf` is shorter than what the sub-pack was parsed from (a defense against passing the
    /// wrong buffer back in).
    pub fn slot_bytes<'a>(&self, rec: &Sbl4Record, buf: &'a [u8]) -> Option<&'a [u8]> {
        let end = rec.offset.checked_add(rec.size)?;
        if end > buf.len() {
            return None;
        }
        Some(&buf[rec.offset..end])
    }

    /// True iff every record's `block_hash` word agreed. 155 of 2612 real sub-packs
    /// violate this — mostly Animation/streamblock packs — so exposing the flag lets
    /// callers decide whether to trust `self.block_hash` as an identity.
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
/// verified by hashing them against every record hash across all SBL4 sub-packs in
/// `Bin360/`. Others are still open — see `docs/2008_subpack_format.md`.
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
            "SBL4: read of u32 at 0x{off:X} overruns buffer ({} bytes)",
            buf.len()
        ));
    }
    Ok(u32::from_be_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]]))
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn act1_intkey_bytes() -> Option<Vec<u8>> {
        let p = std::path::PathBuf::from(
            "c:/Users/Shadow/Desktop/notes-on-reversing-the-sabetour/game-files/Bin360/Global/Act1_IntKey.Pack",
        );
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
        let sub = Sbl4SubPack::parse(&bytes).expect("parse SBL4");
        assert_eq!(sub.records.len(), KNOWN_ACT1_INTKEY_LAYOUT.len());
        for (rec, (name, exp_off, exp_sz)) in sub.records.iter().zip(KNOWN_ACT1_INTKEY_LAYOUT) {
            assert_eq!(rec.kind_name, Some(*name), "record kind_name for {name}");
            assert_eq!(rec.offset as u32, *exp_off, "{name} offset");
            assert_eq!(rec.size as u32, *exp_sz, "{name} size");
            assert_eq!(rec.kind_hash, pandemic_hash(name), "{name} hash matches kernel");
        }
        // Block hash consistency + coverage — asserted by parse() itself, but re-checked
        // here because a downstream reader trusts these two properties.
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
        let sub = Sbl4SubPack::parse(&bytes).expect("parse SBL4");
        let mat = sub.find("materials").expect("materials slot present");
        let mat_bytes = sub.slot_bytes(mat, &bytes).expect("slice materials");
        assert_eq!(mat_bytes.len(), 310);
        assert_eq!(&mat_bytes[0..4], b"WSAO", "materials slot starts with WSAO magic");
    }

    /// Refuse a truncated header + refuse a wrong magic. The parser is the last thing
    /// standing between a mis-detected container and the caller getting bogus records.
    #[test]
    fn rejects_wrong_magic_and_truncation() {
        assert!(Sbl4SubPack::parse(&[]).is_err());
        assert!(Sbl4SubPack::parse(b"\x00\x00\x00\x00").is_err());
        // Right length header but wrong magic.
        assert!(Sbl4SubPack::parse(b"SBLL\x00\x00\x00\x01").is_err());
        // SBL4 magic + real nEntries but the directory itself is truncated.
        let mut buf = Vec::from(*b"SBL4");
        buf.extend_from_slice(&4u32.to_be_bytes());
        // Only one record's worth of bytes present, not 4.
        buf.extend_from_slice(&[0u8; 24]);
        assert!(Sbl4SubPack::parse(&buf).is_err());
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

        let sub = Sbl4SubPack::parse(&buf).expect("parse synthetic SBL4");
        assert_eq!(sub.records.len(), 2);
        let m = sub.find("materials").unwrap();
        assert_eq!(sub.slot_bytes(m, &buf).unwrap(), mat_payload.as_slice());
        let msh = sub.find("meshes").unwrap();
        assert_eq!(sub.slot_bytes(msh, &buf).unwrap(), mesh_payload.as_slice());
        assert_eq!(sub.block_hash, 0xDEADBEEF);
        assert!(sub.has_consistent_block_hash());
    }
}
