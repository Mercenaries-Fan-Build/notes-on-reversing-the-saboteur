// Rust port, 2026-10-18, of FFmpeg 9.0.1 libavcodec/get_bits.h, put_bits.h and vlc.c (the parts the
// XMA2 decoder uses).
// Copyright (c) the FFmpeg developers. Licensed under the GNU Lesser General Public License,
// version 2.1 or later; see the LGPL notice in `mod.rs`.

//! MSB-first bit reading and writing, and multi-level VLC tables built from code lengths.
//!
//! The reader fails on any read or skip past its declared size; a peek past it reads zero bits,
//! which only the unused tail of a table lookup sees.

pub(super) struct BitReader<'a> {
    buf: &'a [u8],
    size: usize,
    index: usize,
}

impl<'a> BitReader<'a> {
    /// A reader over the first `size` bits of `buf`.
    pub(super) fn new(buf: &'a [u8], size: usize) -> Result<Self, String> {
        if size > buf.len() * 8 {
            return Err(format!("bit reader of {size} bits over {} bytes", buf.len()));
        }
        Ok(BitReader { buf, size, index: 0 })
    }

    pub(super) fn count(&self) -> usize {
        self.index
    }

    pub(super) fn bytes(&self) -> &'a [u8] {
        self.buf
    }

    fn peek(&self, n: u32) -> u32 {
        let mut v = 0u64;
        for k in 0..n as usize {
            let at = self.index + k;
            let bit = if at < self.size { (self.buf[at >> 3] >> (7 - (at & 7))) & 1 } else { 0 };
            v = (v << 1) | bit as u64;
        }
        v as u32
    }

    pub(super) fn show(&self, n: u32) -> Result<u32, String> {
        if self.index + n as usize > self.size {
            return Err(format!("show of {n} bits at {} of {}", self.index, self.size));
        }
        Ok(self.peek(n))
    }

    pub(super) fn skip(&mut self, n: usize) -> Result<(), String> {
        if self.index + n > self.size {
            return Err(format!("skip of {n} bits at {} of {}", self.index, self.size));
        }
        self.index += n;
        Ok(())
    }

    /// `n` bits, `n <= 32`, unsigned.
    pub(super) fn bits(&mut self, n: u32) -> Result<u32, String> {
        let v = self.show(n)?;
        self.index += n as usize;
        Ok(v)
    }

    pub(super) fn bit(&mut self) -> Result<bool, String> {
        Ok(self.bits(1)? == 1)
    }

    /// `n` bits as a two's-complement signed value.
    pub(super) fn sbits(&mut self, n: u32) -> Result<i32, String> {
        let v = self.bits(n)?;
        Ok(((v << (32 - n)) as i32) >> (32 - n))
    }
}

/// An MSB-first bit buffer (`PutBitContext` over `frame_data`).
#[derive(Default)]
pub(super) struct BitWriter {
    pub(super) buf: Vec<u8>,
    bits: usize,
}

impl BitWriter {
    pub(super) fn clear(&mut self) {
        self.buf.clear();
        self.bits = 0;
    }

    pub(super) fn count(&self) -> usize {
        self.bits
    }

    pub(super) fn put(&mut self, n: u32, v: u32) {
        for k in (0..n).rev() {
            if self.bits % 8 == 0 {
                self.buf.push(0);
            }
            let bit = ((v >> k) & 1) as u8;
            let last = self.buf.len() - 1;
            self.buf[last] |= bit << (7 - (self.bits % 8));
            self.bits += 1;
        }
    }

    /// Append the first `n` bits of `src` (`ff_copy_bits`).
    pub(super) fn copy(&mut self, src: &[u8], n: usize) -> Result<(), String> {
        if n > src.len() * 8 {
            return Err(format!("copy of {n} bits from {} bytes", src.len()));
        }
        for at in 0..n {
            self.put(1, ((src[at >> 3] >> (7 - (at & 7))) & 1) as u32);
        }
        Ok(())
    }
}

/// A VLC table: entries `(symbol, length)`; a negative length marks a subtable whose start index
/// is the symbol (`VLCElem`).
pub(super) struct Vlc {
    table: Vec<(i32, i32)>,
    bits: u32,
    max_depth: u32,
}

#[derive(Clone, Copy)]
struct Code {
    bits: u32,
    code: u32,
    symbol: i32,
}

fn build_table(table: &mut Vec<(i32, i32)>, nb_bits: u32, codes: &mut [Code]) -> Result<usize, String> {
    let start = table.len();
    table.resize(start + (1usize << nb_bits), (0, 0));
    let mut i = 0;
    while i < codes.len() {
        let n = codes[i].bits;
        let code = codes[i].code;
        let symbol = codes[i].symbol;
        if n <= nb_bits {
            let j = (code >> (32 - nb_bits)) as usize;
            for k in 0..1usize << (nb_bits - n) {
                let e = &mut table[start + j + k];
                if (e.1 != 0 || e.0 != 0) && (e.1 != n as i32 || e.0 != symbol) {
                    return Err("incorrect VLC codes".into());
                }
                *e = (symbol, n as i32);
            }
            i += 1;
        } else {
            let code_prefix = code >> (32 - nb_bits);
            let mut subtable_bits = n - nb_bits;
            codes[i].bits = n - nb_bits;
            codes[i].code = code << nb_bits;
            let mut k = i + 1;
            while k < codes.len() {
                let nk = codes[k].bits as i32 - nb_bits as i32;
                if nk <= 0 || codes[k].code >> (32 - nb_bits) != code_prefix {
                    break;
                }
                codes[k].bits = nk as u32;
                codes[k].code <<= nb_bits;
                subtable_bits = subtable_bits.max(nk as u32);
                k += 1;
            }
            let subtable_bits = subtable_bits.min(nb_bits);
            let j = start + code_prefix as usize;
            table[j].1 = -(subtable_bits as i32);
            let index = build_table(table, subtable_bits, &mut codes[i..k])?;
            table[j].0 = index as i32;
            i = k;
        }
    }
    for e in &mut table[start..start + (1usize << nb_bits)] {
        if e.1 == 0 {
            e.0 = -1;
        }
    }
    Ok(start)
}

impl Vlc {
    /// `ff_vlc_init_from_lengths`: codes assigned in table order, each the next left-aligned
    /// 32-bit code; the symbol is `symbols[i] + offset`.
    pub(super) fn from_lengths(bits: u32, max_len: u32, lens: &[u8], symbols: &[i32], offset: i32) -> Result<Vlc, String> {
        let len_max = (3 * bits).min(32);
        let mut code: u64 = 0;
        let mut codes = Vec::with_capacity(lens.len());
        for (i, &len) in lens.iter().enumerate() {
            let len = len as u32;
            if len == 0 {
                continue;
            }
            if len > len_max || code & ((1u64 << (32 - len)) - 1) != 0 {
                return Err(format!("invalid VLC length {len}"));
            }
            codes.push(Code { bits: len, code: code as u32, symbol: symbols[i] + offset });
            code += 1u64 << (32 - len);
            if code > u32::MAX as u64 + 1 {
                return Err("overdetermined VLC tree".into());
            }
        }
        let mut table = Vec::new();
        build_table(&mut table, bits, &mut codes)?;
        Ok(Vlc { table, bits, max_depth: max_len.div_ceil(bits) })
    }

    /// `get_vlc2`: the symbol, or -1 when no code matches (no bits consumed at the first level).
    pub(super) fn read(&self, r: &mut BitReader) -> Result<i32, String> {
        let idx = r.peek(self.bits) as usize;
        let (mut code, mut n) = self.table[idx];
        if self.max_depth > 1 && n < 0 {
            r.skip(self.bits as usize)?;
            let nb = (-n) as u32;
            let idx = r.peek(nb) as usize + code as usize;
            (code, n) = self.table[idx];
            if self.max_depth > 2 && n < 0 {
                r.skip(nb as usize)?;
                let nb = (-n) as u32;
                let idx = r.peek(nb) as usize + code as usize;
                (code, n) = self.table[idx];
            }
        }
        if n < 0 {
            return Err("VLC code longer than the table depth".into());
        }
        r.skip(n as usize)?;
        Ok(code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reader_signed_and_bounds() {
        let b = [0b1011_0011u8, 0xF0];
        let mut r = BitReader::new(&b, 12).unwrap();
        assert_eq!(r.bits(3).unwrap(), 0b101);
        assert_eq!(r.sbits(3).unwrap(), -4);
        assert_eq!(r.bits(6).unwrap(), 0b11_1111);
        assert_eq!(r.count(), 12);
        assert!(r.bit().is_err());
    }

    #[test]
    fn writer_copies_unaligned() {
        let mut w = BitWriter::default();
        w.put(3, 0b101);
        w.copy(&[0xFF, 0x80], 9).unwrap();
        assert_eq!((w.count(), w.buf.clone()), (12, vec![0b1011_1111, 0b1111_0000]));
    }

    #[test]
    fn vlc_multi_level() {
        // Lengths 1..=12 then a second 12: a complete prefix code. A 5-bit first level sends the
        // longer codes through subtables.
        let lens = [1u8, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 12];
        let syms: Vec<i32> = (0..lens.len() as i32).collect();
        let v = Vlc::from_lengths(5, 12, &lens, &syms, 0).unwrap();
        // 0 -> sym 0 (1 bit); 10 -> sym 1; 1111 1111 1111 -> last sym.
        let b = [0b0101_1111, 0xFF, 0xF0];
        let mut r = BitReader::new(&b, 24).unwrap();
        assert_eq!(v.read(&mut r).unwrap(), 0);
        assert_eq!(v.read(&mut r).unwrap(), 1);
        assert_eq!(v.read(&mut r).unwrap(), 12);
        assert_eq!(r.count(), 15);
    }
}
