//! A forward reader over big-endian record bytes, as the 2008 records are stored.

use crate::bytes;

pub(super) struct Cursor<'a> {
    pub(super) b: &'a [u8],
    pub(super) at: usize,
}

impl<'a> Cursor<'a> {
    pub(super) fn u32(&mut self, field: &str) -> Result<u32, String> {
        let v = bytes::u32_be(self.b, self.at, field)?;
        self.at += 4;
        Ok(v)
    }
    pub(super) fn i32(&mut self, field: &str) -> Result<i32, String> {
        let v = bytes::i32_be(self.b, self.at, field)?;
        self.at += 4;
        Ok(v)
    }
    pub(super) fn f32(&mut self, field: &str) -> Result<f32, String> {
        let v = bytes::f32_be(self.b, self.at, field)?;
        self.at += 4;
        Ok(v)
    }
    pub(super) fn f32s<const N: usize>(&mut self, field: &str) -> Result<[f32; N], String> {
        let mut out = [0.0; N];
        for x in &mut out {
            *x = self.f32(field)?;
        }
        Ok(out)
    }
    pub(super) fn i32s<const N: usize>(&mut self, field: &str) -> Result<[i32; N], String> {
        let mut out = [0; N];
        for x in &mut out {
            *x = self.i32(field)?;
        }
        Ok(out)
    }
    pub(super) fn take(&mut self, n: usize, field: &str) -> Result<&'a [u8], String> {
        let s = bytes::slice(self.b, self.at, n, field)?;
        self.at += n;
        Ok(s)
    }
}
