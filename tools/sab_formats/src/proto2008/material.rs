//! A `materials` [`super::sbl`] slot of the 2008 prototype: a `WSAO` block of `MTLD` records, each a
//! material's texture list and its render passes. Layout and evidence: `docs/2008_materials_format.md`.
//! Engine reader: `ReadMaterials` (`WildStar_d.exe` @0x826554a8, `WildStar_p.exe` @0x8236c440);
//! binding: `XenonOdin::SetMaterial` / `SetStates` (`WildStar_d` @0x831420e8 / @0x83143da8).
//!
//! ```text
//! char  magic[4] = "WSAO"
//! u32   count
//! MTLD  records[count]
//!
//! MTLD
//!   char  magic[4] = "MTLD"
//!   u32   uid                      the key meshes' primitives name
//!   u32   size                     bytes of the record after this word
//!   u32   name_len                 at most 0x100, NUL included
//!   char  name[name_len]
//!   u32   texture_count
//!   u32   textures[texture_count]  texture name hashes
//!   u32   pass_count
//!   Pass  passes[pass_count]
//!
//! Pass
//!   u32   name                     hash of the pass name ("Z-Pass", "Cast-Shadow-Pass", ...)
//!   u32   flags
//!   u32   render_state_count, sampler_state_count
//!   u32   vertex_shader, pixel_shader          shader hashes
//!   u32   texture_ref_count
//!   u32   vs_float_count, vs_int_count, vs_bool_count
//!   u32   ps_float_count, ps_int_count, ps_bool_count
//!   u32   register_vector_count
//!   u32   texture_refs[texture_ref_count]      indices into textures; ref i binds sampler i
//!   { u32 register; f32 value[4] }[register_vector_count]
//!   f32   vs_float[vs_float_count][4]          vertex shader constants from c0
//!   i32   vs_int[vs_int_count][4]              from i0
//!   i32   vs_bool[vs_bool_count]               from b0
//!   f32   ps_float[ps_float_count][4]          pixel shader constants, likewise
//!   i32   ps_int[ps_int_count][4]
//!   i32   ps_bool[ps_bool_count]
//!   { u32 state; u32 value }[render_state_count]    D3DRENDERSTATETYPE
//!   { u32 state; u32 value }[sampler_state_count]   D3DSAMPLERSTATETYPE; STATE_END moves to the
//!                                                   next sampler
//! ```

use super::cursor::Cursor;

/// The slot's magic.
pub const MAGIC: &[u8; 4] = b"WSAO";
/// A record's magic.
pub const RECORD_MAGIC: &[u8; 4] = b"MTLD";
/// In a pass's sampler states, the marker that advances to the next sampler; in its render states,
/// the end of the list (`XenonOdin::SetStates`).
pub const STATE_END: u32 = 0x7FFF_FFFF;
/// `ReadMaterials` asserts the name length is at most this.
pub const MAX_NAME: usize = 0x100;

/// Pass names: `pandemic_hash` of the strings `ReadMaterials` compares a pass's name with
/// ("Z-Pass", "Cast-Shadow-Pass", "Receive-Shadow-Pass", "Foliage-Pass"), and of "Color-Pass", the
/// name of every material's first pass.
pub const PASS_COLOR: u32 = 0x0ABC_CFE8;
pub const PASS_Z: u32 = 0x9E60_A369;
pub const PASS_CAST_SHADOW: u32 = 0xB202_02F9;
pub const PASS_RECEIVE_SHADOW: u32 = 0x236F_5A8D;
pub const PASS_FOLIAGE: u32 = 0xAD3F_8D38;

/// One shader stage's constants, each set loaded from register 0.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Constants {
    pub floats: Vec<[f32; 4]>,
    pub ints: Vec<[i32; 4]>,
    pub bools: Vec<i32>,
}

/// One render pass of a material.
#[derive(Clone, Debug, PartialEq)]
pub struct Pass {
    pub name: u32,
    pub flags: u32,
    pub vertex_shader: u32,
    pub pixel_shader: u32,
    /// Indices into the material's `textures`; entry `i` is bound to sampler `i`.
    pub texture_refs: Vec<u32>,
    /// Vectors the engine sets by register through its `PblVector4` state tracker.
    pub register_vectors: Vec<(u32, [f32; 4])>,
    pub vs: Constants,
    pub ps: Constants,
    pub render_states: Vec<(u32, u32)>,
    pub sampler_states: Vec<(u32, u32)>,
}

impl Pass {
    /// The material texture hashes this pass binds, in sampler order.
    pub fn sampler_textures<'a>(&'a self, m: &'a Material) -> impl Iterator<Item = u32> + 'a {
        self.texture_refs.iter().map(move |&i| m.textures[i as usize])
    }

    /// The sampler states grouped per sampler: [`STATE_END`] closes one sampler's group and opens the
    /// next.
    pub fn samplers(&self) -> Vec<Vec<(u32, u32)>> {
        let mut out = vec![Vec::new()];
        for &(s, v) in &self.sampler_states {
            if s == STATE_END {
                out.push(Vec::new());
            } else {
                out.last_mut().expect("never empty").push((s, v));
            }
        }
        if self.sampler_states.is_empty() {
            out.clear();
        }
        out
    }
}

/// One `MTLD` record.
#[derive(Clone, Debug, PartialEq)]
pub struct Material {
    pub uid: u32,
    pub name: String,
    pub textures: Vec<u32>,
    pub passes: Vec<Pass>,
}

fn constants(c: &mut Cursor, floats: u32, ints: u32, bools: u32, stage: &str) -> Result<Constants, String> {
    let mut k = Constants::default();
    for _ in 0..floats {
        k.floats.push(c.f32s::<4>(&format!("{stage} float constant"))?);
    }
    for _ in 0..ints {
        k.ints.push(c.i32s::<4>(&format!("{stage} int constant"))?);
    }
    for _ in 0..bools {
        k.bools.push(c.i32(&format!("{stage} bool constant"))?);
    }
    Ok(k)
}

fn pairs(c: &mut Cursor, n: u32, field: &str) -> Result<Vec<(u32, u32)>, String> {
    (0..n).map(|_| Ok((c.u32(field)?, c.u32(field)?))).collect()
}

fn pass(c: &mut Cursor, texture_count: usize, at: &str) -> Result<Pass, String> {
    let name = c.u32("pass name")?;
    let flags = c.u32("pass flags")?;
    let render_state_count = c.u32("render state count")?;
    let sampler_state_count = c.u32("sampler state count")?;
    let vertex_shader = c.u32("vertex shader")?;
    let pixel_shader = c.u32("pixel shader")?;
    let texture_ref_count = c.u32("texture ref count")?;
    let vs_f = c.u32("vs float count")?;
    let vs_i = c.u32("vs int count")?;
    let vs_b = c.u32("vs bool count")?;
    let ps_f = c.u32("ps float count")?;
    let ps_i = c.u32("ps int count")?;
    let ps_b = c.u32("ps bool count")?;
    let vectors = c.u32("register vector count")?;
    let mut texture_refs = Vec::new();
    for _ in 0..texture_ref_count {
        let r = c.u32("texture ref")?;
        if r as usize >= texture_count {
            return Err(format!("{at}: texture ref {r} with {texture_count} textures"));
        }
        texture_refs.push(r);
    }
    let mut register_vectors = Vec::new();
    for _ in 0..vectors {
        register_vectors.push((c.u32("register vector register")?, c.f32s::<4>("register vector")?));
    }
    let vs = constants(c, vs_f, vs_i, vs_b, "vertex shader")?;
    let ps = constants(c, ps_f, ps_i, ps_b, "pixel shader")?;
    let render_states = pairs(c, render_state_count, "render state")?;
    let sampler_states = pairs(c, sampler_state_count, "sampler state")?;
    Ok(Pass { name, flags, vertex_shader, pixel_shader, texture_refs, register_vectors, vs, ps, render_states, sampler_states })
}

/// Parse a `materials` slot. Every record must consume exactly its `size`, and the records must end
/// at the slot's end.
pub fn parse(slot: &[u8]) -> Result<Vec<Material>, String> {
    let mut c = Cursor { b: slot, at: 0 };
    if c.take(4, "WSAO magic")? != MAGIC {
        return Err(format!("materials slot: magic {:02X?}, expected WSAO", &slot[..4]));
    }
    let count = c.u32("material count")? as usize;
    let mut out = Vec::with_capacity(count.min(1 << 12));
    for i in 0..count {
        if c.take(4, "MTLD magic")? != RECORD_MAGIC {
            return Err(format!("material {i} at 0x{:X}: no MTLD magic", c.at - 4));
        }
        let uid = c.u32("material uid")?;
        let size = c.u32("material size")? as usize;
        let start = c.at;
        let name_len = c.u32("material name_len")? as usize;
        if name_len > MAX_NAME {
            return Err(format!("material {i}: name_len {name_len} exceeds 0x{MAX_NAME:X}"));
        }
        let raw = c.take(name_len, "material name")?;
        let name = String::from_utf8_lossy(raw.strip_suffix(&[0]).unwrap_or(raw)).into_owned();
        let at = format!("material {i} '{name}'");
        let texture_count = c.u32("texture count")? as usize;
        let textures = (0..texture_count).map(|_| c.u32("texture hash")).collect::<Result<Vec<_>, _>>()?;
        let pass_count = c.u32("pass count")?;
        let passes = (0..pass_count).map(|_| pass(&mut c, texture_count, &at)).collect::<Result<Vec<_>, _>>()?;
        if c.at - start != size {
            return Err(format!("{at}: record reads {} bytes, size says {size}", c.at - start));
        }
        out.push(Material { uid, name, textures, passes });
    }
    if c.at != slot.len() {
        return Err(format!("materials slot: {count} records end at 0x{:X} of 0x{:X}", c.at, slot.len()));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn be(v: &mut Vec<u8>, words: &[u32]) {
        for w in words {
            v.extend_from_slice(&w.to_be_bytes());
        }
    }

    fn sample() -> Vec<u8> {
        let mut body = Vec::new();
        be(&mut body, &[4]);
        body.extend_from_slice(b"Mat\0");
        be(&mut body, &[2, 0xAAAA, 0xBBBB, 1]);
        // render 1, sampler 2, vs, ps, 2 texture refs, vs f/i/b 1/0/1, ps f/i/b 0/1/0, 1 vector
        be(&mut body, &[0x1234, 1, 1, 2, 0xC0, 0xD0, 2, 1, 0, 1, 0, 1, 0, 1]);
        be(&mut body, &[1, 0]);
        be(&mut body, &[3, 1.0f32.to_bits(), 2.0f32.to_bits(), 3.0f32.to_bits(), 4.0f32.to_bits()]);
        be(&mut body, &[0.5f32.to_bits(); 4]);
        be(&mut body, &[1]);
        be(&mut body, &[7, 8, 9, 10]);
        be(&mut body, &[0x1B, 1, 0x05, 2, STATE_END, 0]);
        let mut b = b"WSAO".to_vec();
        be(&mut b, &[1]);
        b.extend_from_slice(b"MTLD");
        be(&mut b, &[0x5150, body.len() as u32]);
        b.extend_from_slice(&body);
        b
    }

    #[test]
    fn parses_a_record_in_engine_order() {
        let ms = parse(&sample()).unwrap();
        let m = &ms[0];
        assert_eq!((m.uid, m.name.as_str(), m.textures.as_slice()), (0x5150, "Mat", &[0xAAAA, 0xBBBB][..]));
        let p = &m.passes[0];
        assert_eq!((p.name, p.flags, p.vertex_shader, p.pixel_shader), (0x1234, 1, 0xC0, 0xD0));
        assert_eq!(p.sampler_textures(m).collect::<Vec<_>>(), vec![0xBBBB, 0xAAAA]);
        assert_eq!(p.register_vectors, vec![(3, [1.0, 2.0, 3.0, 4.0])]);
        assert_eq!((p.vs.floats.as_slice(), p.vs.bools.as_slice()), (&[[0.5; 4]][..], &[1][..]));
        assert_eq!(p.ps.ints, vec![[7, 8, 9, 10]]);
        assert_eq!(p.render_states, vec![(0x1B, 1)]);
        assert_eq!(p.sampler_states, vec![(0x05, 2), (STATE_END, 0)]);
    }

    #[test]
    fn pass_names_are_their_hashes() {
        for (name, h) in [
            ("Color-Pass", PASS_COLOR),
            ("Z-Pass", PASS_Z),
            ("Cast-Shadow-Pass", PASS_CAST_SHADOW),
            ("Receive-Shadow-Pass", PASS_RECEIVE_SHADOW),
            ("Foliage-Pass", PASS_FOLIAGE),
        ] {
            assert_eq!(crate::pandemic_hash(name), h, "{name}");
        }
        let p = &parse(&sample()).unwrap()[0].passes[0];
        assert_eq!(p.samplers(), vec![vec![(0x05, 2)], vec![]]);
    }

    #[test]
    fn rejects_size_mismatch_bad_refs_and_trailing_bytes() {
        let good = sample();
        let mut long = good.clone();
        long.push(0);
        assert!(parse(&long).is_err());
        let mut bad_size = good.clone();
        let n = u32::from_be_bytes(bad_size[16..20].try_into().unwrap());
        bad_size[16..20].copy_from_slice(&(n + 4).to_be_bytes());
        assert!(parse(&bad_size).is_err());
        let mut bad_ref = good.clone();
        let refs_at = 20 + 4 + 4 + 4 + 8 + 4 + 14 * 4;
        bad_ref[refs_at..refs_at + 4].copy_from_slice(&2u32.to_be_bytes());
        assert!(parse(&bad_ref).is_err());
    }
}
