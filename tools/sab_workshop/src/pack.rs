//! In-app megapack reader — open a `.megapack`, enumerate its ALBS sub-pack entries, and hand back
//! each entry's bytes. Enough of `tools/sab_pack` to let the viewer read a character's DTEX bundles
//! straight from the game install (mirrors how `mercs2_workshop` reads the WAD in-process).
//!
//! Megapack layout (COPIED from `tools/sab_pack`, verified vs `Global/Dynamic0.megapack`):
//!   +0x00 char magic[4] = "00PM"
//!   +0x04 u32  count
//!   +0x08 Entry[count] (20 B: u32 crc, u32 index, u32 size, u64 offset)
//!   ...   table2[count] (8 B pairs, ignored here)
//!   ...   sub-pack bytes at each entry's `offset`

#![allow(dead_code)]

const MAGIC: &[u8; 4] = b"00PM";

fn u32le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn u64le(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3], b[o + 4], b[o + 5], b[o + 6], b[o + 7]])
}

/// `pandemic_hash` — the game's name hash (FNV-1a/32 variant, case-folded). Verified kernel:
/// `pandemic_hash("ANY") == 0xED057225`. Not needed for plaintext DTEX enumeration, but kept for
/// resolving hash-keyed lookups (megapack index / future WSAO).
pub fn pandemic_hash(name: &str) -> u32 {
    let mut h: u32 = 0x811C_9DC5;
    for &c in name.as_bytes() {
        h = ((c as u32 | 0x20) ^ h).wrapping_mul(0x0100_0193);
    }
    (h ^ 0x2A).wrapping_mul(0x0100_0193)
}

/// Fill `buf` from byte `off` of `f` without moving a shared cursor — the positioned read each OS
/// provides. Unix has an exact variant; Windows' `seek_read` may return short, so it is looped.
#[cfg(unix)]
fn read_exact_at(f: &std::fs::File, buf: &mut [u8], off: u64) -> std::io::Result<()> {
    use std::os::unix::fs::FileExt;
    f.read_exact_at(buf, off)
}

#[cfg(windows)]
fn read_exact_at(f: &std::fs::File, mut buf: &mut [u8], mut off: u64) -> std::io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !buf.is_empty() {
        match f.seek_read(buf, off) {
            Ok(0) => return Err(std::io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => {
                buf = &mut buf[n..];
                off += n as u64;
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
compile_error!("pack.rs needs a positioned file read for this target (only unix and windows are implemented)");

#[derive(Clone, Copy)]
pub struct Entry {
    pub crc: u32,
    pub index: u32,
    pub size: u32,
    pub offset: u64,
}

/// An opened megapack: the file **memory-mapped** + its parsed index. Opening is instant — only the
/// index (header + `count×20`) is touched up front; sub-pack bytes fault into RAM lazily as they are
/// sliced. The index is hash-only (no plaintext paths), so texture bundles are found by scanning
/// bundle bytes for a character token — but the mmap means we only pay for the bundles we look at.
pub struct Megapack {
    data: memmap2::Mmap,
    /// Kept for buffered, bounded-memory per-sub-pack reads (the big token scan) — avoids
    /// mmap-faulting the whole 714 MB into the process working set. See `read_into`.
    file: std::fs::File,
    entries: Vec<Entry>,
}

impl Megapack {
    pub fn open(path: &str) -> Result<Megapack, String> {
        let file = std::fs::File::open(path).map_err(|e| format!("open {path}: {e}"))?;
        // SAFETY: the game's megapacks are read-only assets we never mutate; a concurrent external
        // truncation is the only UB risk and does not occur for installed game files.
        let data = unsafe { memmap2::Mmap::map(&file).map_err(|e| format!("mmap {path}: {e}"))? };
        if data.len() < 8 || &data[0..4] != MAGIC {
            return Err(format!("not a megapack (magic {:02X?})", &data[0..4.min(data.len())]));
        }
        let count = u32le(&data, 4) as usize;
        let mut entries = Vec::with_capacity(count);
        let mut p = 8usize;
        for _ in 0..count {
            if p + 20 > data.len() {
                return Err("megapack index truncated".into());
            }
            entries.push(Entry {
                crc: u32le(&data, p),
                index: u32le(&data, p + 4),
                size: u32le(&data, p + 8),
                offset: u64le(&data, p + 12),
            });
            p += 20;
        }
        Ok(Megapack { data, file, entries })
    }

    /// Read entry `e`'s sub-pack into `buf` via a positioned file read (no mmap fault), returning the
    /// slice actually read. `buf` is reused across calls, so a full scan holds only ONE sub-pack in
    /// memory (~tens of MB) instead of mapping the whole pack resident.
    pub fn read_into<'b>(&self, e: &Entry, buf: &'b mut Vec<u8>) -> &'b [u8] {
        let n = e.size as usize;
        if buf.len() < n {
            buf.resize(n, 0);
        }
        // The whole entry or nothing: a short read used to come back as a truncated bundle.
        match read_exact_at(&self.file, &mut buf[..n], e.offset) {
            Ok(()) => &buf[..n],
            Err(_) => &[],
        }
    }

    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }

    /// The whole mapped file. Reading a region faults only that region in (lazy).
    pub fn raw(&self) -> &[u8] {
        &self.data
    }

    /// The bytes of entry `i`'s ALBS sub-pack (bounds-checked; empty slice if the entry is corrupt).
    pub fn slice(&self, e: &Entry) -> &[u8] {
        let s = e.offset as usize;
        let end = s.saturating_add(e.size as usize).min(self.data.len());
        if s >= self.data.len() || end <= s {
            return &[];
        }
        &self.data[s..end]
    }

    /// The first `max` bytes of entry `e`'s sub-pack — enough to read its header (e.g. the `AHSM`
    /// MSHA wrapper, which sits near the sub-pack start). Faults in only that window, not the whole
    /// bundle: the earmark that lets `list_meshes` enumerate models without touching 715 MB.
    pub fn window(&self, e: &Entry, max: usize) -> &[u8] {
        let s = e.offset as usize;
        let end = s.saturating_add((e.size as usize).min(max)).min(self.data.len());
        if s >= self.data.len() || end <= s {
            return &[];
        }
        &self.data[s..end]
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// The entry whose sub-pack spans absolute file offset `off` — maps a model found by the raw
    /// `AHSM` sweep back to the bundle it lives in (so we can read that bundle's textures).
    pub fn entry_containing(&self, off: usize) -> Option<Entry> {
        self.entries.iter().find(|e| {
            let s = e.offset as usize;
            off >= s && off < s + e.size as usize
        }).copied()
    }
}

/// Case-insensitive substring search (ASCII) — bundles store DTEX names in plaintext, so a bundle
/// holding a character's textures contains that character token as raw bytes.
pub fn contains_ascii_ci(haystack: &[u8], needle: &str) -> bool {
    let nl = needle.len();
    if nl == 0 || haystack.len() < nl {
        return false;
    }
    let nlow = needle.as_bytes();
    haystack.windows(nl).any(|w| {
        w.iter().zip(nlow).all(|(a, b)| a.eq_ignore_ascii_case(b))
    })
}
