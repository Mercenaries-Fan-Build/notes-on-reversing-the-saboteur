# 2008 Xbox 360 prototype — shaders (`*.shaders`)

`Bin360/` holds three shader files: `France.shaders`, `AnimationTest.shaders` and
`SultanOfSwing.shaders`. They hold the compiled pixel and vertex shaders that material passes
(`docs/2008_materials_format.md`) name by hash. Reader: `sab_formats::proto2008::shaders`. Engine
reader: `ReadShaders` (`WildStar_d` @0x82656e88, `WildStar_p` @0x8236b5f0, same field order).

## Layout (big-endian)

```text
char  magic[4] = "SHDR"
u32   pixel_count             ReadShaders asserts 1..9999
Group pixel[pixel_count]      tag "PSHD"
u32   vertex_count            asserts 1..999
Group vertex[vertex_count]    tag "VSHD"

Group (France, AnimationTest — the layout ReadShaders reads)
  char  tag[4]
  u32   name                  0 in every group of the build
  u32   present               0: nothing follows
  u32   variant_count         asserted non-zero
  u32   key                   OdinShaderManager::AddShader registers the group under this
  Variant variants[variant_count]

Group (SultanOfSwing)
  char  tag[4]
  u32   name, present, key
  Variant variant             one, no count

Variant
  u32   size                  asserted < 0x8000
  u8    code[size]            handed to Odin's CreatePixelShader / CreateVertexShader
  u32   param_count
  { u32 len; char name[len]; u32 value }[param_count]     len <= 64, NUL included
```

* All three files parse to their end: `France` 737 pixel + 59 vertex groups, `AnimationTest` 255 +
  43, `SultanOfSwing` 458 + 33. Every group is present. PROVEN.
* **`SultanOfSwing.shaders`** has no `variant_count`: after `present` come the key and one variant.
  Neither `WildStar_d` nor `WildStar_p` reads that layout (both read a count before the key), and
  there is no `SultanOfSwing.ShaderMapping` beside it. It belongs with the `SultanOfSwing/` packs:
  their materials' 18 passes name its groups. The reader tells the layouts apart by where the first group's code
  magic sits after its `present` word: 12 bytes (count, key, size) or 8 (key, size). PROVEN.
* **`key`** is what material passes name: every material shader hash that names a group names it
  by `key`; none names a `name` word (all 0). PROVEN.
* **Variants** are light-count permutations. A pixel group has 1 or 3 variants; each variant's
  params are `PointLightCount` and `SpotLightCount`, or none. Vertex groups have one variant and no
  params. `SetMaterial` picks a variant by index (`OdinShaders::GetShader`). PROVEN.
* Of the material shader hashes in the build, 746 pixel and 73 vertex name a group in some file;
  484 pixel and 28 vertex name none (50,200 passes). Those materials' shaders are not shipped.
  PROVEN.

## Compiled shader (`code`)

```text
+0x00  u32  magic                 0x102A1100 pixel, 0x102A1101 vertex
+0x10  u32  offset of the constant table: u32 length, then a D3DXSHADER_CONSTANTTABLE
```

The constant table is the public big-endian `D3DXSHADER_CONSTANTTABLE` of `d3dx9shader.h`: a
0x1C-byte header `{Size, Creator, Version, Constants, ConstantInfo, Flags, Target}`, then 20-byte
`D3DXSHADER_CONSTANTINFO {Name, RegisterSet, RegisterIndex, RegisterCount, Reserved, TypeInfo,
DefaultValue}`, offsets from the header. Every one of the 3,340 pixel and 135 vertex variants has
one: `Version` `0xFFFF0300` (`ps_3_0`) and `0xFFFE0300` (`vs_3_0`). PROVEN. The other header words
and the microcode are not read here. (In France's first pixel variant, `+0x0C` is the offset of a
block holding the path `D:\projects\WildStar\Art\Cache\Updb\b0\b033bc47-ab880145.updb`; in
SultanOfSwing's first it is 0.)

### Samplers

* **Material samplers** are named `g_Sampler<N>`, and every one sits at sampler register `s<N>`
  (8,835 / 8,835). So a pass's texture ref `i` (`SetMaterial` binds it to sampler `i`) is the shader's
  `g_Sampler<i>`. PROVEN.
* In 184,681 passes every variant's shaders use exactly as many `g_Sampler`s as the pass binds
  texture refs; in 22,212 some variant uses fewer (indices with gaps, such as `[0, 1, 3]`, where the
  compiler dropped an unused sampler); none uses more. PROVEN.
* **Engine samplers** sit at fixed registers above the material ones: `RainRippleMap` s9,
  `WTFIntensityTexture` s10, `LightProbeEnvMapTexture` s11, `ZFadeTexture` s12, `ShadowTexture` /
  `ZShadowTexture` s13, `SpotLightProjTexture` s14, `DiffuseEnvMapTexture` s15; the vertex shader's
  `QuantizedNormalsTexture` s2. PROVEN.
* No shipped data names what a `g_Sampler<N>` is for (diffuse, normal, specular, mask): the constant
  names are generic, the variant params are light counts, and `.ShaderMapping` records hold hashes
  only. What the textures bound to them are is in `docs/2008_materials_format.md`
  ("Texture kinds per sampler").

## `.ShaderMapping` — per-material shader overrides

`France.ShaderMapping` (6,827 records) and `AnimationTest.ShaderMapping` (2,807). Reader:
`sab_formats::proto2008::shader_mapping`.

```text
char  magic[4] = "WSTO"
u32   count                   the file is exactly 8 + 16 * count bytes
{ u32 material; u32 pass; u32 pixel_shader; u32 vertex_shader }[count]
```

* `ReadShaderMappingTable` (`WildStar_p` @0x8236b2f0; not in `WildStar_d`) reads `<world>.ShaderMapping`
  and, per record, calls `OdinShaderManager::AddMapping` twice with the `PblCRC` of
  `sprintf("%x_%x_P", …)` and `sprintf("%x_%x_V", …)` (format strings at 0x8205141C / 0x82051424).
* `ReadMaterials` (`WildStar_p` @0x8236c440) builds the same strings from the record's `MTLD` uid
  (read into `0x90(r31)` at 0x8236c678) and the pass's name (`0x8(XenonMaterial)`), and for each
  pass calls `GetMapping` (vertex @0x8236db48, pixel @0x8236dbdc). A non-zero result replaces the
  shader hash the material record holds; zero keeps it. PROVEN (disassembly).
* So `material` is a material uid and `pass` a pass-name hash (`Color-Pass` 3,107 + 1,028 records,
  `Receive-Shadow-Pass` 2,581 + 931, `Cast-Shadow-Pass` 1,028 + 843, `Z-Pass` 111 + 5, France +
  AnimationTest). Every `vertex_shader` is a group key; `pixel_shader` is a group key or, on depth
  passes, 0. PROVEN.
* Effect on the build's materials, with `France.ShaderMapping` (the world `France/` packs belong
  to): of `France/` passes, 17,589 are mapped to the shaders they already hold, 207 have a shader
  replaced, 218,288 are not mapped. 48,440 `France/` passes still name a shader absent from the
  three `.shaders` files, mapped or not. PROVEN.

## Method

```
sab_probe proto2008-shaders <Bin360>
```

## Unknowns

* The `name` word (0 everywhere) and the code header words other than the magic and `+0x10`.
* What each shader computes from its samplers — that needs the Xenos microcode disassembled.
