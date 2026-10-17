# 2008 Xbox 360 prototype — materials (`WSAO` / `MTLD`)

A `materials` slot of an `SBL*` sub-pack (`docs/2008_subpack_format.md`) is one `WSAO` block of
`MTLD` records. Each record is a material: its texture list and one to three render passes, each
pass naming a vertex and a pixel shader, the textures it binds, shader constants, and D3D render and
sampler states. Reader: `sab_formats::proto2008::material`.

Engine evidence (WildStar_d unless marked; WildStar_p's copy of `ReadMaterials` reads the same
fields in the same order but is constant-folded by the decompiler):

* `ReadMaterials` @0x826554a8 (WildStar_p @0x8236c440) — the reader.
* `XenonOdin::SetMaterial` @0x831420e8 — binds a pass's textures, shaders and constants.
* `XenonOdin::SetStates` @0x83143da8 — applies its render and sampler states.
* `D3DRENDERSTATETYPE`, `D3DSAMPLERSTATETYPE` and value enums — `WildStar_p.pdb`.

## Layout (big-endian)

```text
char  magic[4] = "WSAO"
u32   count
MTLD  records[count]

MTLD
  char  magic[4] = "MTLD"
  u32   uid                      the key mesh primitives name (see below)
  u32   size                     bytes of the record after this word
  u32   name_len                 <= 0x100, NUL included
  char  name[name_len]
  u32   texture_count
  u32   textures[texture_count]  texture name hashes
  u32   pass_count
  Pass  passes[pass_count]

Pass
  u32   name                     pandemic_hash of the pass name
  u32   flags
  u32   render_state_count
  u32   sampler_state_count
  u32   vertex_shader            shader hash
  u32   pixel_shader             shader hash, 0 for depth-only passes
  u32   texture_ref_count
  u32   vs_float_count, vs_int_count, vs_bool_count
  u32   ps_float_count, ps_int_count, ps_bool_count
  u32   register_vector_count
  u32   texture_refs[texture_ref_count]           indices into textures
  { u32 register; f32 v[4] }[register_vector_count]
  f32   vs_float[vs_float_count][4]
  i32   vs_int[vs_int_count][4]
  i32   vs_bool[vs_bool_count]
  f32   ps_float[ps_float_count][4]
  i32   ps_int[ps_int_count][4]
  i32   ps_bool[ps_bool_count]
  { u32 state; u32 value }[render_state_count]    D3DRENDERSTATETYPE
  { u32 state; u32 value }[sampler_state_count]   D3DSAMPLERSTATETYPE; 0x7FFFFFFF ends a sampler
```

PROVEN: every one of the **3,552 `materials` slots** in the build — all seven sub-pack revisions
(`SBL0` 4, `SBL4` 1,891, `SBL5` 4, `SBL6` 57, `SBL8` 234, `SBLL` 1,257, `SBLX` 105) — parses with every
record consuming exactly its `size`, every texture ref inside its material's list, and the records
ending at the slot's end: 97,597 materials, 257,093 passes.

## Fields

* **`uid`** — `ReadMaterials` registers the material under it (`OdinMaterialManager::SetMaterial`)
  and skips `size` bytes when a material with that uid is already loaded. Every mesh primitive's
  `material` (`docs/2008_msh4_format.md`), and every variant binding's material, is the uid of a
  material in **the same pack**: 298,524 / 298,524 references. PROVEN. The uid is not
  `pandemic_hash(name)` for any record (names repeat: `Procedural Mtl:0`, `DefaultLib.Material:0`);
  its preimage is unknown.
* **`size`** — the bytes after the `size` word to the end of the record. PROVEN (all records).
* **`textures`** — texture name hashes, the keys of `textures` slot records
  (`docs/2008_texture_format.md`): of 341,248 references, 196,096 name a texture in the same pack,
  143,387 one only in another pack, 1,765 none in the build. PROVEN.
* **Passes.** `ReadMaterials` builds one `XenonMaterial` per pass, chained, and sorts each into a
  render bucket by its name: `Z-Pass` → 0x12, `Cast-Shadow-Pass` → 2, `Receive-Shadow-Pass` → 0x1C,
  else `flags & 1` → 0x40, else `Foliage-Pass` → 0x4E, else 0x27. The four names are string literals
  in the reader; the fifth name in the data, `0x0ABCCFE8`, is `pandemic_hash("Color-Pass")`.

  | pass order | materials |
  |---|---|
  | Color, Receive-Shadow, Cast-Shadow | 59,636 |
  | Color, Receive-Shadow | 34,820 |
  | Color, Z, Cast-Shadow | 2,371 |
  | Color, Z | 716 |
  | (none) | 54 |

  Every material with passes starts with `Color-Pass`. `Foliage-Pass` does not occur. PROVEN.
* **`flags`** — `ReadMaterials` tests bit 0 (bucket 0x40) and counts bit 2. 73 distinct values
  occur (0x0 on 109,076 passes); the reader does nothing else with them. What bits other than 0 and
  2 select: UNKNOWN.
* **Shaders** — `ReadMaterials` resolves both hashes through `OdinShaderManager::GetShaders`. The
  first word goes to material `+0x24`, which `SetMaterial` binds with `SetVertexShader`; the second
  to `+0x20`, bound with `SetPixelShader`. Every pass has a vertex shader. Every `Color-Pass` and
  `Receive-Shadow-Pass` has a pixel shader; a `Z-Pass` or `Cast-Shadow-Pass` has one exactly when it
  sets `ALPHATESTENABLE` (829 + 18,552 passes) and none otherwise (2,258 + 43,455). 1,331 distinct
  hashes. PROVEN.
* **`texture_refs`** — `SetMaterial` binds ref `i` to sampler `i` (or to vertex sampler `0x10+n`
  when bit `i` of material `+0x38` is set, which only `OdinMaterial::SetVertexShaderTextureFlag`
  does — no file field). PROVEN.
* **Constants** — `SetMaterial` loads the float, int and bool sets with
  `Set{Vertex,Pixel}ShaderConstant{F,I,B}` from register 0 (`c0`, `i0`, `b0`). The data uses floats
  only (vs 0–2, ps 0–11 vectors). PROVEN.
* **`register_vectors`** — `{register, vec4}` pairs `SetMaterial` sets through its `PblVector4`
  state tracker at the given register; registers 0, 1, 4, 5, 6 occur. Which shader stage the tracker
  feeds: UNKNOWN.
* **Render states** — the ids that occur, by `WildStar_p.pdb`: `ZWRITEENABLE`, `CULLMODE`,
  `ALPHABLENDENABLE`, `ALPHATESTENABLE`, `ALPHAREF`, `STENCILENABLE` / `FUNC` / `PASS` / `REF` /
  `WRITEMASK`, `DEPTHBIAS`, `COLORWRITEENABLE` (set to 0 on every `Z-Pass` and `Cast-Shadow-Pass`).
  `ALPHATESTENABLE` is on in 18,552 `Color-Pass`, 17,723 `Receive-Shadow-Pass`, 18,552
  `Cast-Shadow-Pass` and 829 `Z-Pass` passes. PROVEN.
* **Sampler states** — split into one group per sampler by `0x7FFFFFFF` (`D3DSAMP_FORCE_DWORD`);
  `SetStates` advances the sampler at each. Where a pass has groups (35,534 passes) there is one
  per texture ref, except 321 passes with 2 groups for 3 refs. Every group is
  `ADDRESSU, ADDRESSV, MAGFILTER, MINFILTER, MIPFILTER` = all `CLAMP` (105,565) or all `WRAP`
  (61,195), with `LINEAR` filters. PROVEN.

## What this replaces

The earlier version of this page read 12 of 863 packlog-addressed slots, stopped after the texture
list and assumed a `[diffuse, specular, normal, wm]` slot order taken from retail. The record has no
fixed slot order: a pass's `texture_refs` order is its shader's sampler order. Which sampler a
shader samples as diffuse, normal or specular is the shaders' business (`*.shaders`, next).

## Method

```
sab_probe proto2008-materials <Bin360>     # every number above
```

`D3D*` enum values: CodeView `LF_ENUMERATE` records of `WildStar_p.pdb`.

## Unknowns

* The preimage of `uid`.
* `flags` bits other than 0 and 2.
* The shader stage behind `register_vectors`.
* Sampler roles per shader — from the `.shaders` files and their constant tables.
