//! Bounds-checked scalar reads over `&[u8]` — the one reader set every format module uses.
//!
//! Every read names the field it is reading, so a malformed file fails with *what* was being read and
//! *where*, never with an index panic or a silently-defaulted value. Little- and big-endian (the 2008
//! Xbox 360 prototype is big-endian). Zero-copy: these borrow the caller's bytes.

/// The `len` bytes at `off`, or an error naming `field`, the offset and the buffer length.
pub fn slice<'a>(b: &'a [u8], off: usize, len: usize, field: &str) -> Result<&'a [u8], String> {
    off.checked_add(len)
        .and_then(|end| b.get(off..end))
        .ok_or_else(|| format!("{field}: needs {len} bytes at 0x{off:X}, buffer is 0x{:X} long", b.len()))
}

/// `N` bytes at `off` as an array.
fn array<const N: usize>(b: &[u8], off: usize, field: &str) -> Result<[u8; N], String> {
    Ok(slice(b, off, N, field)?.try_into().expect("slice() returned exactly N bytes"))
}

pub fn u8_at(b: &[u8], off: usize, field: &str) -> Result<u8, String> {
    Ok(array::<1>(b, off, field)?[0])
}
pub fn u16_le(b: &[u8], off: usize, field: &str) -> Result<u16, String> {
    Ok(u16::from_le_bytes(array(b, off, field)?))
}
pub fn u16_be(b: &[u8], off: usize, field: &str) -> Result<u16, String> {
    Ok(u16::from_be_bytes(array(b, off, field)?))
}
pub fn i16_le(b: &[u8], off: usize, field: &str) -> Result<i16, String> {
    Ok(i16::from_le_bytes(array(b, off, field)?))
}
pub fn i16_be(b: &[u8], off: usize, field: &str) -> Result<i16, String> {
    Ok(i16::from_be_bytes(array(b, off, field)?))
}
pub fn u32_le(b: &[u8], off: usize, field: &str) -> Result<u32, String> {
    Ok(u32::from_le_bytes(array(b, off, field)?))
}
pub fn u32_be(b: &[u8], off: usize, field: &str) -> Result<u32, String> {
    Ok(u32::from_be_bytes(array(b, off, field)?))
}
pub fn i32_le(b: &[u8], off: usize, field: &str) -> Result<i32, String> {
    Ok(i32::from_le_bytes(array(b, off, field)?))
}
pub fn i32_be(b: &[u8], off: usize, field: &str) -> Result<i32, String> {
    Ok(i32::from_be_bytes(array(b, off, field)?))
}
pub fn u64_le(b: &[u8], off: usize, field: &str) -> Result<u64, String> {
    Ok(u64::from_le_bytes(array(b, off, field)?))
}
pub fn f32_le(b: &[u8], off: usize, field: &str) -> Result<f32, String> {
    Ok(f32::from_le_bytes(array(b, off, field)?))
}
pub fn f32_be(b: &[u8], off: usize, field: &str) -> Result<f32, String> {
    Ok(f32::from_be_bytes(array(b, off, field)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_both_byte_orders() {
        let b = [0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08];
        assert_eq!(u16_le(&b, 0, "x"), Ok(0x0201));
        assert_eq!(u16_be(&b, 0, "x"), Ok(0x0102));
        assert_eq!(u32_le(&b, 4, "x"), Ok(0x0807_0605));
        assert_eq!(u32_be(&b, 4, "x"), Ok(0x0506_0708));
        assert_eq!(u64_le(&b, 0, "x"), Ok(0x0807_0605_0403_0201));
        assert_eq!(i16_le(&[0xFF, 0xFF], 0, "x"), Ok(-1));
        assert_eq!(f32_le(&1.5f32.to_le_bytes(), 0, "x"), Ok(1.5));
    }

    /// Out of bounds — including an offset whose end overflows usize — is an error naming the field.
    #[test]
    fn out_of_bounds_names_the_field() {
        let b = [0u8; 6];
        let e = u32_le(&b, 4, "numBones").unwrap_err();
        assert!(e.contains("numBones") && e.contains("0x4") && e.contains("0x6"), "{e}");
        assert!(slice(&b, usize::MAX, 2, "f").is_err());
        assert_eq!(slice(&b, 6, 0, "f"), Ok(&[][..]));
    }
}
