//! `sound/Saboteur.pck` — the 2008 build's Wwise file package, and the big-endian `RIFX` / bank chunk
//! streams inside it. `docs/2008_audio_format.md`.
//!
//! ```text
//! Header (0x1C bytes, read whole by WSSoundPack::LoadPack @0x825f7eb0)
//!   +0x00  u32  magic = "PCK0"
//!   +0x04  u32  block size        (0x800) — WSStreamingIO::Open divides a record's offset by it
//!   +0x08  u32  (not read)
//!   +0x0C  u32  bank count
//!   +0x10  u32  bank table offset
//!   +0x14  u32  stream count
//!   +0x18  u32  stream table offset
//!
//! Record (16 bytes, SoundPackTOCEntry; each table sorted by id — SoundEntryList::FindID @0x825f7be8
//! binary-searches it)
//!   +0x00  u32  id
//!   +0x04  u32  size
//!   +0x08  u32  offset            absolute, in bytes
//!   +0x0C  u32  f0c
//! ```
//!
//! `WSStreamingIO::Open` @0x825fa010 looks a bank (`AkFileSystemFlags` codec 0) up in the bank table
//! and any other file in the stream table.

use crate::bytes;

pub const MAGIC: &[u8; 4] = b"PCK0";
pub const HEADER_LEN: usize = 0x1C;
pub const RECORD_LEN: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub block_size: u32,
    pub unread: u32,
    pub bank_count: u32,
    pub bank_table: u32,
    pub stream_count: u32,
    pub stream_table: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Record {
    pub id: u32,
    pub size: u32,
    pub offset: u32,
    pub f0c: u32,
}

impl Record {
    /// The record's bytes in the whole pack.
    pub fn data<'a>(&self, pack: &'a [u8]) -> Result<&'a [u8], String> {
        bytes::slice(pack, self.offset as usize, self.size as usize, &format!("record {:08X}", self.id))
    }
}

/// The package: its header and both tables.
#[derive(Clone, Debug)]
pub struct Pack {
    pub header: Header,
    pub banks: Vec<Record>,
    pub streams: Vec<Record>,
}

pub fn parse_header(head: &[u8]) -> Result<Header, String> {
    let magic = bytes::slice(head, 0, 4, "PCK0 magic")?;
    if magic != MAGIC {
        return Err(format!("magic {magic:02X?}, expected PCK0"));
    }
    let w = |o: usize, f: &str| bytes::u32_be(head, o, f);
    let h = Header {
        block_size: w(0x04, "block size")?,
        unread: w(0x08, "header +0x08")?,
        bank_count: w(0x0C, "bank count")?,
        bank_table: w(0x10, "bank table offset")?,
        stream_count: w(0x14, "stream count")?,
        stream_table: w(0x18, "stream table offset")?,
    };
    if h.block_size == 0 {
        return Err("block size 0".into());
    }
    Ok(h)
}

fn table(b: &[u8], offset: u32, count: u32, what: &str) -> Result<Vec<Record>, String> {
    let n = count as usize;
    let t = bytes::slice(b, offset as usize, n.checked_mul(RECORD_LEN).ok_or(format!("{what} count {n} overflows"))?, what)?;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let w = |k: usize| bytes::u32_be(t, i * RECORD_LEN + 4 * k, what);
        out.push(Record { id: w(0)?, size: w(1)?, offset: w(2)?, f0c: w(3)? });
    }
    if let Some(i) = (1..n).find(|&i| out[i - 1].id >= out[i].id) {
        return Err(format!("{what}: ids not ascending at record {i} ({:08X} after {:08X})", out[i].id, out[i - 1].id));
    }
    Ok(out)
}

/// Read the header and both tables from the head of the file (`b` must hold them; it may be the whole
/// pack). A record's data must lie inside `file_len`.
pub fn parse(b: &[u8], file_len: u64) -> Result<Pack, String> {
    let header = parse_header(bytes::slice(b, 0, HEADER_LEN, "PCK0 header")?)?;
    let banks = table(b, header.bank_table, header.bank_count, "bank table")?;
    let streams = table(b, header.stream_table, header.stream_count, "stream table")?;
    for r in banks.iter().chain(&streams) {
        if r.offset as u64 + r.size as u64 > file_len {
            return Err(format!("record {:08X}: 0x{:X}+0x{:X} past the 0x{file_len:X}-byte file", r.id, r.offset, r.size));
        }
    }
    Ok(Pack { header, banks, streams })
}

/// One chunk of a big-endian chunk stream: its tag and its body's range in the stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Chunk {
    pub tag: [u8; 4],
    pub start: usize,
    pub len: usize,
}

/// Walk `{tag[4], u32 size, body[size]}` chunks (sizes big-endian, no padding) over all of `b`. The
/// last chunk must end exactly at the end.
pub fn chunks(b: &[u8]) -> Result<Vec<Chunk>, String> {
    let mut out = Vec::new();
    let mut o = 0usize;
    while o < b.len() {
        let head = bytes::slice(b, o, 8, &format!("chunk header at 0x{o:X}"))?;
        let tag: [u8; 4] = head[..4].try_into().expect("4 bytes");
        let len = bytes::u32_be(head, 4, "chunk size")? as usize;
        bytes::slice(b, o + 8, len, &format!("chunk {} at 0x{o:X}", String::from_utf8_lossy(&tag)))?;
        out.push(Chunk { tag, start: o + 8, len });
        o += 8 + len;
    }
    Ok(out)
}

/// A stream record: `RIFX`, a size word, `WAVE`, then big-endian chunks to the end of the record. The
/// size word alone is little-endian and holds the whole record's length, header included.
pub fn riff_chunks(b: &[u8]) -> Result<Vec<Chunk>, String> {
    let head = bytes::slice(b, 0, 12, "RIFX header")?;
    if &head[0..4] != b"RIFX" || &head[8..12] != b"WAVE" {
        return Err(format!("not a RIFX WAVE: {:02X?}", &head[..12]));
    }
    let size = bytes::u32_le(head, 4, "RIFX size")? as usize;
    if size != b.len() {
        return Err(format!("RIFX size word {size} for a {}-byte record", b.len()));
    }
    let mut cs = chunks(&b[12..])?;
    for c in &mut cs {
        c.start += 12;
    }
    Ok(cs)
}

/// An `XMA2` format chunk — the layout of `XMA2WAVEFORMAT` (Microsoft `xma2defs.h`), big-endian.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Xma2Format {
    pub version: u8,
    pub num_streams: u8,
    pub loop_count: u8,
    pub loop_begin: u32,
    pub loop_end: u32,
    pub sample_rate: u32,
    pub encode_options: u32,
    pub pseudo_bytes_per_sec: u32,
    pub block_size: u32,
    pub samples_encoded: u32,
    pub samples_in_source: u32,
    pub block_count: u32,
    /// Per stream: channels and channel mask.
    pub streams: Vec<(u8, u16)>,
}

pub fn parse_xma2(c: &[u8]) -> Result<Xma2Format, String> {
    let w = |o: usize, f: &str| bytes::u32_be(c, o, f);
    let num_streams = bytes::u8_at(c, 1, "XMA2 streams")?;
    let mut streams = Vec::new();
    for s in 0..num_streams as usize {
        streams.push((bytes::u8_at(c, 40 + 4 * s, "XMA2 stream channels")?, bytes::u16_be(c, 42 + 4 * s, "XMA2 channel mask")?));
    }
    if c.len() != 40 + 4 * num_streams as usize {
        return Err(format!("XMA2 chunk is {} bytes for {num_streams} streams", c.len()));
    }
    Ok(Xma2Format {
        version: bytes::u8_at(c, 0, "XMA2 version")?,
        num_streams,
        loop_count: bytes::u8_at(c, 3, "XMA2 loop count")?,
        loop_begin: w(4, "XMA2 loop begin")?,
        loop_end: w(8, "XMA2 loop end")?,
        sample_rate: w(12, "XMA2 sample rate")?,
        encode_options: w(16, "XMA2 encode options")?,
        pseudo_bytes_per_sec: w(20, "XMA2 bytes per second")?,
        block_size: w(24, "XMA2 block size")?,
        samples_encoded: w(28, "XMA2 samples encoded")?,
        samples_in_source: w(32, "XMA2 samples in source")?,
        block_count: w(36, "XMA2 block count")?,
        streams,
    })
}

/// Decode a whole `RIFX` media file (a stream record or a bank's `DIDX` media) to PCM. Every 2008
/// media is one XMA stream; more streams are an error.
pub fn decode_media(m: &[u8]) -> Result<crate::pcm::Pcm, String> {
    let cs = riff_chunks(m)?;
    let body = |t: &[u8; 4]| {
        cs.iter()
            .find(|c| &c.tag == t)
            .map(|c| &m[c.start..c.start + c.len])
            .ok_or(format!("no {} chunk", String::from_utf8_lossy(t)))
    };
    let x = parse_xma2(body(b"XMA2")?)?;
    if x.num_streams != 1 {
        return Err(format!("{} XMA streams in one media", x.num_streams));
    }
    crate::xma2::decode(body(b"data")?, x.streams[0].0 as u16, x.sample_rate)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(id: u32, size: u32, offset: u32) -> Vec<u8> {
        [id, size, offset, 0].iter().flat_map(|v| v.to_be_bytes()).collect()
    }

    #[test]
    fn header_tables_and_order() {
        let mut b = b"PCK0".to_vec();
        for v in [0x800u32, 0, 1, 0x1C, 2, 0x2C] {
            b.extend_from_slice(&v.to_be_bytes());
        }
        b.extend(rec(7, 4, 0x4C));
        b.extend(rec(1, 4, 0x4C));
        b.extend(rec(9, 4, 0x4C));
        b.extend([0u8; 4]);
        let p = parse(&b, b.len() as u64).unwrap();
        assert_eq!((p.banks.len(), p.streams.len(), p.streams[1].id), (1, 2, 9));
        let mut unsorted = b.clone();
        unsorted[0x2C..0x30].copy_from_slice(&10u32.to_be_bytes());
        assert!(parse(&unsorted, b.len() as u64).unwrap_err().contains("not ascending"));
        assert!(parse(&b, 0x40).unwrap_err().contains("past"));
    }

    #[test]
    fn chunk_streams_end_exactly() {
        let mut b = b"RIFX".to_vec();
        b.extend_from_slice(&24u32.to_le_bytes());
        b.extend_from_slice(b"WAVEdata");
        b.extend_from_slice(&4u32.to_be_bytes());
        b.extend_from_slice(&[1, 2, 3, 4]);
        let cs = riff_chunks(&b).unwrap();
        assert_eq!((cs.len(), &cs[0].tag, cs[0].start, cs[0].len), (1, b"data", 20, 4));
        assert!(chunks(&b[12..b.len() - 1]).is_err());
    }
}
