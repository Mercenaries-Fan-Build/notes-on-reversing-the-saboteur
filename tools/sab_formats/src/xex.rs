//! `XEX2` — the Xbox 360 executable wrapper around a PowerPC PE image (the 2008 prototype's
//! `Saboteur.xex`, `WildStar_d.xex`, `WildStar_p.xex`, `WildStar_p-org.xex`). All fields big-endian.
//!
//! ```text
//! +0x00  char   magic[4] = "XEX2"
//! +0x04  u32    module flags
//! +0x08  u32    pe_offset           the wrapped image starts here and runs to the end of the file
//! +0x0C  u32    reserved
//! +0x10  u32    security_offset
//! +0x14  u32    header_count
//! +0x18  Header headers[header_count]   {u32 key, u32 value}
//!
//! Security info (at security_offset)
//!   +0x004  u32       image_size      length of the unwrapped image
//!   +0x150  u8[16]    file key        AES-128-ECB-encrypted with the key-encryption key
//!
//! Base file format (the header whose key is 0x000003FF; its value is an offset)
//!   +0x00  u32  info_size
//!   +0x04  u16  encryption          0 none, 1 AES-128-CBC with the file key, IV 0
//!   +0x06  u16  compression         1 basic
//!   +0x08  Block blocks[(info_size - 8) / 8]   basic: {u32 data_size, u32 zero_size}
//! ```
//!
//! Basic compression stores the image as runs: each block's `data_size` bytes come from the decrypted
//! stream, followed by `zero_size` zero bytes. The loader zero-fills the image before laying the runs
//! into it (Xenia `src/xenia/cpu/xex_module.cc`, the basic-compressed reader), so runs that end before
//! `image_size` leave a zero tail — `WildStar_p-org.xex` ends 32,768 bytes short. The prototype's
//! binaries are development-signed: their key-encryption key is sixteen zero bytes.

use mercs2_formats::ps3_crypto::{aes128_cbc_decrypt, aes128_ecb_decrypt_block};

use crate::bytes;

/// The magic, in on-disk byte order.
pub const MAGIC: &[u8; 4] = b"XEX2";
/// Optional-header key of the base file format descriptor.
pub const BASE_FILE_FORMAT: u32 = 0x0000_03FF;
/// Optional-header key of the original PE file name.
pub const ORIGINAL_PE_NAME: u32 = 0x0001_83FF;
/// The development-kit key-encryption key.
pub const DEVKIT_KEK: [u8; 16] = [0; 16];

/// The fields of the wrapper that the unwrap uses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Header {
    pub pe_offset: u32,
    pub security_offset: u32,
    /// `(key, value)` of every optional header, in file order.
    pub headers: Vec<(u32, u32)>,
    pub image_size: u32,
    pub encryption: u16,
    pub compression: u16,
    /// Basic-compression blocks: `(data_size, zero_size)`.
    pub blocks: Vec<(u32, u32)>,
    pub wrapped_key: [u8; 16],
}

impl Header {
    /// The value of the optional header `key`.
    pub fn value(&self, key: u32) -> Option<u32> {
        self.headers.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
    }
}

/// Parse the wrapper's header, security info and base file format.
pub fn parse(buf: &[u8]) -> Result<Header, String> {
    let magic = bytes::slice(buf, 0, 4, "XEX magic")?;
    if magic != MAGIC {
        return Err(format!("XEX: magic {magic:02X?}, expected {MAGIC:02X?}"));
    }
    let pe_offset = bytes::u32_be(buf, 8, "XEX pe_offset")?;
    let security_offset = bytes::u32_be(buf, 0x10, "XEX security_offset")?;
    let count = bytes::u32_be(buf, 0x14, "XEX header_count")? as usize;
    let mut headers = Vec::with_capacity(count);
    for i in 0..count {
        let p = 0x18 + 8 * i;
        headers.push((bytes::u32_be(buf, p, "XEX header key")?, bytes::u32_be(buf, p + 4, "XEX header value")?));
    }
    let sec = security_offset as usize;
    let image_size = bytes::u32_be(buf, sec + 4, "XEX image_size")?;
    let wrapped_key: [u8; 16] = bytes::slice(buf, sec + 0x150, 16, "XEX file key")?.try_into().expect("16 bytes");
    let bff = headers
        .iter()
        .find(|(k, _)| *k == BASE_FILE_FORMAT)
        .ok_or("XEX: no base file format header (key 0x000003FF)")?
        .1 as usize;
    let info_size = bytes::u32_be(buf, bff, "XEX base file format info_size")? as usize;
    let encryption = bytes::u16_be(buf, bff + 4, "XEX encryption")?;
    let compression = bytes::u16_be(buf, bff + 6, "XEX compression")?;
    if compression != 1 {
        return Err(format!("XEX: compression {compression}; only basic (1) is read"));
    }
    if encryption > 1 {
        return Err(format!("XEX: encryption {encryption} is neither none (0) nor AES (1)"));
    }
    if info_size < 8 || (info_size - 8) % 8 != 0 {
        return Err(format!("XEX: basic-compression info_size {info_size} is not 8 + 8*blocks"));
    }
    let mut blocks = Vec::new();
    for i in 0..(info_size - 8) / 8 {
        let p = bff + 8 + 8 * i;
        blocks.push((bytes::u32_be(buf, p, "XEX block data_size")?, bytes::u32_be(buf, p + 4, "XEX block zero_size")?));
    }
    Ok(Header { pe_offset, security_offset, headers, image_size, encryption, compression, blocks, wrapped_key })
}

/// The original PE file name the wrapper records, when it has one.
pub fn original_pe_name(buf: &[u8], h: &Header) -> Result<Option<String>, String> {
    let Some(off) = h.value(ORIGINAL_PE_NAME) else { return Ok(None) };
    let len = bytes::u32_be(buf, off as usize, "XEX original name length")? as usize;
    let raw = bytes::slice(buf, off as usize + 4, len.saturating_sub(4), "XEX original name")?;
    let end = raw.iter().position(|&c| c == 0).unwrap_or(raw.len());
    Ok(Some(String::from_utf8_lossy(&raw[..end]).into_owned()))
}

/// The unwrapped PE image (`image_size` bytes), decrypted with the key-encryption key `kek`. The basic
/// blocks must consume the whole wrapped stream (up to its 16-byte CBC alignment) and expand to at most
/// `image_size`; the rest of the image is zero. The image must begin with `MZ`.
pub fn unwrap(buf: &[u8], h: &Header, kek: &[u8; 16]) -> Result<Vec<u8>, String> {
    let wrapped = bytes::slice(buf, h.pe_offset as usize, buf.len().saturating_sub(h.pe_offset as usize), "XEX image")?;
    let stream = if h.encryption == 1 {
        let key = aes128_ecb_decrypt_block(kek, &h.wrapped_key);
        aes128_cbc_decrypt(&key, &[0; 16], wrapped)
    } else {
        wrapped.to_vec()
    };
    let data_total: usize = h.blocks.iter().map(|(d, _)| *d as usize).sum();
    if data_total > stream.len() || stream.len() - data_total >= 16 {
        return Err(format!(
            "XEX: basic blocks hold {data_total} data bytes, the wrapped stream is {} bytes",
            stream.len()
        ));
    }
    let mut image = Vec::with_capacity(h.image_size as usize);
    let mut at = 0usize;
    for (d, z) in &h.blocks {
        image.extend_from_slice(&stream[at..at + *d as usize]);
        at += *d as usize;
        image.resize(image.len() + *z as usize, 0);
    }
    if image.len() > h.image_size as usize {
        return Err(format!("XEX: blocks expand to {} bytes, past image_size {}", image.len(), h.image_size));
    }
    image.resize(h.image_size as usize, 0);
    if !image.starts_with(b"MZ") {
        return Err(format!(
            "XEX: the unwrapped image starts {:02X?}, not MZ (wrong key-encryption key?)",
            &image[..image.len().min(2)]
        ));
    }
    Ok(image)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mercs2_formats::ps3_crypto::aes128_ecb_encrypt_block;

    fn cbc_encrypt(key: &[u8; 16], data: &[u8]) -> Vec<u8> {
        let mut prev = [0u8; 16];
        let mut out = Vec::new();
        for chunk in data.chunks_exact(16) {
            let mut x = [0u8; 16];
            for i in 0..16 {
                x[i] = chunk[i] ^ prev[i];
            }
            prev = aes128_ecb_encrypt_block(key, &x);
            out.extend_from_slice(&prev);
        }
        out
    }

    /// A wrapper around `image` (two basic blocks with zero runs), encrypted with `file_key` under `kek`.
    fn build(image_runs: &[(&[u8], u32)], file_key: [u8; 16], kek: &[u8; 16]) -> Vec<u8> {
        let mut data: Vec<u8> = image_runs.iter().flat_map(|(d, _)| d.iter().copied()).collect();
        let pad = (16 - data.len() % 16) % 16;
        data.resize(data.len() + pad, 0xEE);
        let image_size: u32 = image_runs.iter().map(|(d, z)| d.len() as u32 + z).sum();
        let (bff, sec, pe) = (0x20usize, 0x40usize, 0x200usize);
        let mut buf = vec![0u8; pe];
        buf[..4].copy_from_slice(MAGIC);
        buf[8..12].copy_from_slice(&(pe as u32).to_be_bytes());
        buf[0x10..0x14].copy_from_slice(&(sec as u32).to_be_bytes());
        buf[0x14..0x18].copy_from_slice(&1u32.to_be_bytes());
        buf[0x18..0x1C].copy_from_slice(&BASE_FILE_FORMAT.to_be_bytes());
        buf[0x1C..0x20].copy_from_slice(&(bff as u32).to_be_bytes());
        buf[bff..bff + 4].copy_from_slice(&(8 + 8 * image_runs.len() as u32).to_be_bytes());
        buf[bff + 4..bff + 6].copy_from_slice(&1u16.to_be_bytes());
        buf[bff + 6..bff + 8].copy_from_slice(&1u16.to_be_bytes());
        for (i, (d, z)) in image_runs.iter().enumerate() {
            let p = bff + 8 + 8 * i;
            buf[p..p + 4].copy_from_slice(&(d.len() as u32).to_be_bytes());
            buf[p + 4..p + 8].copy_from_slice(&z.to_be_bytes());
        }
        buf[sec + 4..sec + 8].copy_from_slice(&image_size.to_be_bytes());
        buf[sec + 0x150..sec + 0x160].copy_from_slice(&aes128_ecb_encrypt_block(kek, &file_key));
        buf.extend_from_slice(&cbc_encrypt(&file_key, &data));
        buf
    }

    #[test]
    fn unwraps_encrypted_basic_blocks() {
        let a = b"MZ\x90\x00 first run of the image";
        let b = b"second run";
        let buf = build(&[(&a[..], 7), (&b[..], 3)], [0x5A; 16], &DEVKIT_KEK);
        let h = parse(&buf).unwrap();
        assert_eq!(h.blocks, [(a.len() as u32, 7), (b.len() as u32, 3)]);
        let img = unwrap(&buf, &h, &DEVKIT_KEK).unwrap();
        let mut want = a.to_vec();
        want.extend_from_slice(&[0; 7]);
        want.extend_from_slice(b);
        want.extend_from_slice(&[0; 3]);
        assert_eq!(img, want);

        let e = unwrap(&buf, &h, &[1; 16]).unwrap_err();
        assert!(e.contains("not MZ") || e.contains("blocks"), "{e}");

        let mut short = h.clone();
        short.image_size += 5;
        let img = unwrap(&buf, &short, &DEVKIT_KEK).unwrap();
        assert_eq!(&img[want.len()..], &[0; 5]);
        short.image_size -= 6;
        assert!(unwrap(&buf, &short, &DEVKIT_KEK).unwrap_err().contains("past image_size"));
    }

    #[test]
    fn rejects_other_magic_and_compression() {
        assert!(parse(b"XEX1\0\0\0\0").unwrap_err().contains("magic"));
        let mut buf = build(&[(&b"MZ"[..], 0)], [1; 16], &DEVKIT_KEK);
        buf[0x20 + 6..0x20 + 8].copy_from_slice(&2u16.to_be_bytes());
        assert!(parse(&buf).unwrap_err().contains("compression 2"));
    }
}
