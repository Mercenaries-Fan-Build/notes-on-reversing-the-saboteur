//! A `<world>.shaders` file of the 2008 prototype (`Bin360/France.shaders`, ...): the compiled pixel
//! and vertex shaders materials name. Layout and evidence: `docs/2008_shaders_format.md`. Engine
//! reader: `ReadShaders` (`WildStar_d.exe` @0x82656e88).
//!
//! ```text
//! char  magic[4] = "SHDR"
//! u32   pixel_count
//! Group pixel[pixel_count]          tag "PSHD"
//! u32   vertex_count
//! Group vertex[vertex_count]        tag "VSHD"
//!
//! Group (Layout::Variants — what ReadShaders reads)
//!   char  tag[4]
//!   u32   name                      hash
//!   u32   present                   0: nothing follows
//!   u32   variant_count             non-zero
//!   u32   key                       the hash OdinShaderManager registers the group under
//!   Variant variants[variant_count]
//!
//! Group (Layout::Single — `SultanOfSwing.shaders`; no reader in the build)
//!   char  tag[4]
//!   u32   name
//!   u32   present
//!   u32   key
//!   Variant variant
//!
//! Variant
//!   u32   size                      < 0x8000
//!   u8    code[size]                begins with CODE_MAGIC_PIXEL / CODE_MAGIC_VERTEX
//!   u32   param_count
//!   { u32 len; char name[len]; u32 value }[param_count]     len <= 64
//! ```

use super::cursor::Cursor;
use crate::bytes;

/// The first word of a compiled pixel shader.
pub const CODE_MAGIC_PIXEL: u32 = 0x102A_1100;
/// The first word of a compiled vertex shader.
pub const CODE_MAGIC_VERTEX: u32 = 0x102A_1101;

/// How a file lays out its groups.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    /// A variant count and any number of variants per group.
    Variants,
    /// One variant per group, no count.
    Single,
}

/// The file's magic.
pub const MAGIC: &[u8; 4] = b"SHDR";
/// A pixel shader group's tag.
pub const PIXEL_TAG: &[u8; 4] = b"PSHD";
/// A vertex shader group's tag.
pub const VERTEX_TAG: &[u8; 4] = b"VSHD";
/// `ReadShaders` asserts each variant's code is below this.
pub const MAX_CODE: usize = 0x8000;
/// `ReadShaders` reads each parameter name into a 64-byte buffer.
pub const MAX_PARAM_NAME: usize = 64;

/// One compiled shader and its named parameters.
#[derive(Clone, Debug, PartialEq)]
pub struct Variant<'a> {
    pub code: &'a [u8],
    pub params: Vec<(String, u32)>,
}

/// One shader group: the variants a material's shader hash selects among.
#[derive(Clone, Debug, PartialEq)]
pub struct Group<'a> {
    pub name: u32,
    /// `None` when the group's `present` word is 0.
    pub key: Option<u32>,
    pub variants: Vec<Variant<'a>>,
}

/// A parsed file.
#[derive(Clone, Debug, PartialEq)]
pub struct Shaders<'a> {
    pub layout: Layout,
    pub pixel: Vec<Group<'a>>,
    pub vertex: Vec<Group<'a>>,
}

fn group<'a>(c: &mut Cursor<'a>, layout: Layout, tag: &[u8; 4], magic: u32, i: usize) -> Result<Group<'a>, String> {
    let at = format!("{} group {i}", String::from_utf8_lossy(tag));
    if c.take(4, "shader group tag")? != tag {
        return Err(format!("{at} at 0x{:X}: wrong tag", c.at - 4));
    }
    let name = c.u32("shader group name")?;
    if c.u32("shader group present")? == 0 {
        return Ok(Group { name, key: None, variants: Vec::new() });
    }
    let count = match layout {
        Layout::Variants => c.u32("shader variant count")? as usize,
        Layout::Single => 1,
    };
    if count == 0 {
        return Err(format!("{at}: present with no variants"));
    }
    let key = c.u32("shader group key")?;
    let mut variants = Vec::with_capacity(count.min(256));
    for v in 0..count {
        let size = c.u32("shader code size")? as usize;
        if size >= MAX_CODE {
            return Err(format!("{at} variant {v}: code size 0x{size:X} is not below 0x{MAX_CODE:X}"));
        }
        let code = c.take(size, "shader code")?;
        if bytes::u32_be(code, 0, "shader code magic")? != magic {
            return Err(format!("{at} variant {v}: code does not begin with 0x{magic:08X}"));
        }
        let n = c.u32("shader param count")? as usize;
        let mut params = Vec::with_capacity(n.min(256));
        for _ in 0..n {
            let len = c.u32("shader param name length")? as usize;
            if len > MAX_PARAM_NAME {
                return Err(format!("{at} variant {v}: param name length {len} exceeds {MAX_PARAM_NAME}"));
            }
            let raw = c.take(len, "shader param name")?;
            let s = String::from_utf8_lossy(raw.strip_suffix(&[0]).unwrap_or(raw)).into_owned();
            params.push((s, c.u32("shader param value")?));
        }
        variants.push(Variant { code, params });
    }
    Ok(Group { name, key: Some(key), variants })
}

/// The layout, from the first pixel group: its code magic is 12 bytes after the `present` word in
/// [`Layout::Variants`] (count, key, size) and 8 in [`Layout::Single`] (key, size).
fn layout(b: &[u8]) -> Result<Layout, String> {
    // magic, pixel count, then the first group: tag, name, present.
    let after_present = 4 + 4 + 12;
    if bytes::u32_be(b, after_present - 4, "first group present")? == 0 {
        return Err(".shaders: the first pixel group is absent; the layout cannot be read from it".into());
    }
    let at = |k: usize| bytes::u32_be(b, after_present + k, "first group word");
    match (at(12)?, at(8)?) {
        (CODE_MAGIC_PIXEL, _) => Ok(Layout::Variants),
        (_, CODE_MAGIC_PIXEL) => Ok(Layout::Single),
        _ => Err(".shaders: no pixel shader code magic 8 or 12 bytes after the first group's present word".into()),
    }
}

/// `D3DXREGISTER_SET`: which register file a constant lives in.
pub const REGISTER_SET_BOOL: u16 = 0;
pub const REGISTER_SET_INT4: u16 = 1;
pub const REGISTER_SET_FLOAT4: u16 = 2;
pub const REGISTER_SET_SAMPLER: u16 = 3;
/// `D3DXSHADER_CONSTANTTABLE::Size`.
const CTAB_HEADER: u32 = 0x1C;
/// `D3DXSHADER_CONSTANTINFO` length.
const CONSTANT_INFO: usize = 20;

/// One entry of a shader's constant table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Constant {
    pub name: String,
    pub set: u16,
    pub index: u16,
    pub count: u16,
}

/// A compiled shader's constant table: the code's word at +0x10 is the offset of a `u32` length
/// followed by a big-endian `D3DXSHADER_CONSTANTTABLE` (`d3dx9shader.h`): header `{Size = 0x1C,
/// Creator, Version, Constants, ConstantInfo, Flags, Target}`, then `Constants` 20-byte
/// `{Name, RegisterSet, RegisterIndex, RegisterCount, Reserved, TypeInfo, DefaultValue}` entries;
/// offsets are from the header. Returns the table's `Version` and its constants.
pub fn constant_table(code: &[u8]) -> Result<(u32, Vec<Constant>), String> {
    let off = bytes::u32_be(code, 0x10, "constant table offset")? as usize;
    let len = bytes::u32_be(code, off, "constant table length")? as usize;
    let t = bytes::slice(code, off + 4, len, "constant table")?;
    let size = bytes::u32_be(t, 0, "CTAB Size")?;
    if size != CTAB_HEADER {
        return Err(format!("constant table header size 0x{size:X}, expected 0x{CTAB_HEADER:X}"));
    }
    let version = bytes::u32_be(t, 8, "CTAB Version")?;
    let count = bytes::u32_be(t, 12, "CTAB Constants")? as usize;
    let info = bytes::u32_be(t, 16, "CTAB ConstantInfo")? as usize;
    let mut out = Vec::with_capacity(count.min(256));
    for i in 0..count {
        let e = info + i * CONSTANT_INFO;
        let name_at = bytes::u32_be(t, e, "constant Name")? as usize;
        let tail = t.get(name_at..).ok_or_else(|| format!("constant {i}: name offset 0x{name_at:X} past the table"))?;
        let end = tail.iter().position(|&c| c == 0).ok_or_else(|| format!("constant {i}: unterminated name"))?;
        out.push(Constant {
            name: String::from_utf8_lossy(&tail[..end]).into_owned(),
            set: bytes::u16_be(t, e + 4, "constant RegisterSet")?,
            index: bytes::u16_be(t, e + 6, "constant RegisterIndex")?,
            count: bytes::u16_be(t, e + 8, "constant RegisterCount")?,
        });
    }
    Ok((version, out))
}

/// Parse a whole `.shaders` file; it must end after the vertex groups.
pub fn parse(b: &[u8]) -> Result<Shaders<'_>, String> {
    let mut c = Cursor { b, at: 0 };
    if c.take(4, "SHDR magic")? != MAGIC {
        return Err("not a .shaders file (no SHDR magic)".into());
    }
    let layout = layout(b)?;
    let n = c.u32("pixel shader count")? as usize;
    let pixel = (0..n).map(|i| group(&mut c, layout, PIXEL_TAG, CODE_MAGIC_PIXEL, i)).collect::<Result<Vec<_>, _>>()?;
    let n = c.u32("vertex shader count")? as usize;
    let vertex = (0..n).map(|i| group(&mut c, layout, VERTEX_TAG, CODE_MAGIC_VERTEX, i)).collect::<Result<Vec<_>, _>>()?;
    if c.at != b.len() {
        return Err(format!(".shaders: groups end at 0x{:X} of 0x{:X}", c.at, b.len()));
    }
    Ok(Shaders { layout, pixel, vertex })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn be(v: &mut Vec<u8>, words: &[u32]) {
        for w in words {
            v.extend_from_slice(&w.to_be_bytes());
        }
    }

    /// A compiled-shader blob whose constant table holds one sampler, `g_Sampler0` at s0.
    fn code(magic: u32, version: u32) -> Vec<u8> {
        let mut t = Vec::new();
        be(&mut t, &[CTAB_HEADER, 0, version, 1, 0x1C, 0, 0]);
        be(&mut t, &[0x30, (REGISTER_SET_SAMPLER as u32) << 16, 1 << 16, 0, 0]);
        t.extend_from_slice(b"g_Sampler0\0");
        let mut c = Vec::new();
        be(&mut c, &[magic, 0, 0, 0, 0x14, t.len() as u32]);
        c.extend_from_slice(&t);
        c
    }

    fn file(layout: Layout) -> Vec<u8> {
        let mut b = b"SHDR".to_vec();
        for (tag, magic, version) in [(PIXEL_TAG, CODE_MAGIC_PIXEL, 0xFFFF_0300), (VERTEX_TAG, CODE_MAGIC_VERTEX, 0xFFFE_0300)] {
            be(&mut b, &[1]);
            b.extend_from_slice(tag);
            be(&mut b, &[0, 1]);
            if layout == Layout::Variants {
                be(&mut b, &[1]);
            }
            be(&mut b, &[0xABCD]);
            let c = code(magic, version);
            be(&mut b, &[c.len() as u32]);
            b.extend_from_slice(&c);
            be(&mut b, &[1, 3]);
            b.extend_from_slice(b"Ab\0");
            be(&mut b, &[7]);
        }
        b
    }

    #[test]
    fn reads_both_layouts_and_the_constant_table() {
        for layout in [Layout::Variants, Layout::Single] {
            let b = file(layout);
            let s = parse(&b).unwrap();
            assert_eq!(s.layout, layout);
            let g = &s.pixel[0];
            assert_eq!((g.key, g.variants.len()), (Some(0xABCD), 1));
            assert_eq!(g.variants[0].params, vec![("Ab".to_string(), 7)]);
            let (version, cs) = constant_table(s.vertex[0].variants[0].code).unwrap();
            assert_eq!(version, 0xFFFE_0300);
            assert_eq!(cs, vec![Constant { name: "g_Sampler0".into(), set: REGISTER_SET_SAMPLER, index: 0, count: 1 }]);
        }
    }

    #[test]
    fn rejects_trailing_bytes_and_a_wrong_code_magic() {
        let mut long = file(Layout::Variants);
        long.push(0);
        assert!(parse(&long).is_err());
        let mut b = file(Layout::Variants);
        let vertex_code = b.windows(4).rposition(|w| w == CODE_MAGIC_VERTEX.to_be_bytes()).unwrap();
        b[vertex_code + 3] = 0;
        assert!(parse(&b).is_err());
    }
}
