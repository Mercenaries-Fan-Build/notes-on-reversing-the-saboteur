//! A `<world>.ShaderMapping` file of the 2008 prototype: per material and pass, the shaders that
//! replace the ones the material record names. Layout and evidence: `docs/2008_shaders_format.md`.
//! Engine: `ReadShaderMappingTable` (`WildStar_p.exe` @0x8236b2f0) registers each record with
//! `OdinShaderManager::AddMapping`; `ReadMaterials` (@0x8236c440) looks each pass up with
//! `GetMapping` (@0x8236db48 vertex, @0x8236dbdc pixel) and keeps the record's own hash when there is
//! no mapping.
//!
//! ```text
//! char  magic[4] = "WSTO"
//! u32   count
//! { u32 material; u32 pass; u32 pixel_shader; u32 vertex_shader }[count]
//! ```
//!
//! The lookup key is `pandemic_hash(format!("{material:x}_{pass:x}_P"))` (pixel) or `..._V` (vertex):
//! the `"%x_%x_P"` / `"%x_%x_V"` strings at `WildStar_p` 0x8205141C / 0x82051424.

use std::collections::HashMap;

use crate::bytes;
use crate::pandemic_hash;

/// The file's magic.
pub const MAGIC: &[u8; 4] = b"WSTO";
/// Record length.
pub const RECORD_LEN: usize = 16;

/// One record, as stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Record {
    pub material: u32,
    pub pass: u32,
    pub pixel_shader: u32,
    pub vertex_shader: u32,
}

/// Which shader of a pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Pixel,
    Vertex,
}

/// The key `AddMapping` / `GetMapping` use for a material uid, pass name hash and stage.
pub fn key(material: u32, pass: u32, stage: Stage) -> u32 {
    let tag = match stage {
        Stage::Pixel => 'P',
        Stage::Vertex => 'V',
    };
    pandemic_hash(&format!("{material:x}_{pass:x}_{tag}"))
}

/// The mapping a file registers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Mapping {
    pub records: Vec<Record>,
    by_key: HashMap<u32, u32>,
}

impl Mapping {
    /// The shader a pass of material `material` uses for `stage`: the mapped one, or `stored` (the
    /// hash the material record holds) when the file maps none.
    pub fn resolve(&self, material: u32, pass: u32, stage: Stage, stored: u32) -> u32 {
        self.by_key.get(&key(material, pass, stage)).copied().unwrap_or(stored)
    }

    /// The mapped shader, if the file maps one.
    pub fn get(&self, material: u32, pass: u32, stage: Stage) -> Option<u32> {
        self.by_key.get(&key(material, pass, stage)).copied()
    }
}

/// Parse a whole file; its length must be exactly the header and `count` records.
pub fn parse(b: &[u8]) -> Result<Mapping, String> {
    if bytes::slice(b, 0, 4, "WSTO magic")? != MAGIC {
        return Err("not a .ShaderMapping file (no WSTO magic)".into());
    }
    let count = bytes::u32_be(b, 4, "shader mapping count")? as usize;
    if b.len() != 8 + count * RECORD_LEN {
        return Err(format!(".ShaderMapping: {count} records need {} bytes, file has {}", 8 + count * RECORD_LEN, b.len()));
    }
    let mut m = Mapping::default();
    for i in 0..count {
        let r = 8 + i * RECORD_LEN;
        let w = |k: usize| bytes::u32_be(b, r + 4 * k, "shader mapping record");
        let rec = Record { material: w(0)?, pass: w(1)?, pixel_shader: w(2)?, vertex_shader: w(3)? };
        m.by_key.insert(key(rec.material, rec.pass, Stage::Pixel), rec.pixel_shader);
        m.by_key.insert(key(rec.material, rec.pass, Stage::Vertex), rec.vertex_shader);
        m.records.push(rec);
    }
    Ok(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_by_material_and_pass_and_keeps_the_stored_hash_otherwise() {
        let mut b = b"WSTO".to_vec();
        for w in [1u32, 0x1C7C, 0x0ABC_CFE8, 0xAAAA, 0xBBBB] {
            b.extend_from_slice(&w.to_be_bytes());
        }
        let m = parse(&b).unwrap();
        assert_eq!(key(0x1C7C, 0x0ABC_CFE8, Stage::Vertex), pandemic_hash("1c7c_abccfe8_V"));
        assert_eq!(m.resolve(0x1C7C, 0x0ABC_CFE8, Stage::Pixel, 1), 0xAAAA);
        assert_eq!(m.resolve(0x1C7C, 0x0ABC_CFE8, Stage::Vertex, 1), 0xBBBB);
        assert_eq!(m.resolve(0x1C7D, 0x0ABC_CFE8, Stage::Vertex, 7), 7);
        b.push(0);
        assert!(parse(&b).is_err());
    }
}
