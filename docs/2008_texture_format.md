# 2008 Xbox 360 prototype — textures

Two kinds of texture data ship in the 2008 prototype (`Bin360/`):

* **`textures` slot records** inside the `SBL*` sub-packs (`docs/2008_subpack_format.md`) — 47,313
  records, the textures the engine streams. Each is a big-endian header plus zlib streams that inflate
  to the **Xbox 360 GPU memory image** of the texture (tiled, big-endian elements, full mip chain).
  Reader: `sab_formats::proto2008::texture`.
* **Loose `.dds` files** — 615 ordinary little-endian PC DDS files. Reader: `sab_formats::dds`.

Pixel decoding for both (BC1/2/3/5, X8R8G8B8, A8R8G8B8, R5G6B5, A8, L8) is `sab_formats::pixels`.

Engine readers cited below are in `WildStar_p.exe` (the 2008 format oracle,
`docs/2008_binaries_decomp.md`) unless marked `WildStar_d`.

## Slot record (`textures` slot directory entry)

The slot directory is the `WSStreamTOC` list described in `docs/2008_subpack_format.md`; for a
`textures` slot each record's words are:

| word | meaning | evidence | grade |
|---|---|---|---|
| `+0x00` | `pandemic_hash(name)` of the texture the record holds — 47,313 / 47,313 | probe | PROVEN |
| `+0x04` | offset of the record in the slot | `ReloadNormalMap` @0x821a1338 seeks slot offset + record `+0x04` | PROVEN |
| `+0x08` | stored length | probe (records tile the slot) | PROVEN |
| `+0x0C` | size estimate — see below | `ReadTexture` @0x8237bcf8, `WSStreamBlock::GetTextureSize` @0x821bed20 | PROVEN |
| `+0x10` | 1 | probe | PROVEN |
| `+0x14` | 1 = the streaming manager may evict and reload this texture | `UnloadNormalMaps` @0x821c2420 | PROVEN |

* **`+0x0C`.** `ReadTexture` skips a record whose `+0x0C` is below 9; `GetTextureSize` sums `+0x0C`
  over a block's not-yet-loaded textures and multiplies by 3.5 as the block's texture budget. The
  exporter that wrote the pack decides what the number is:

  | sub-pack revision | `+0x0C` | records |
  |---|---|---|
  | `SBL0`, `SBL4`, `SBL6` | the header's `unpacked` (memory image size) | 24 + 44,874 + 1,234 |
  | `SBL8`, `SBLX` | the record's stored length (`+0x08`) | 683 + 498 |

  PROVEN (every record falls in one of the two columns, split exactly by revision).
* **`+0x14`.** `UnloadNormalMaps` walks a loaded block's texture records and, for each one whose
  `+0x14` is non-zero, calls `UnloadNormalMap` and queues a `ReloadNormalMap` (which re-reads the
  record through `ReadTextureDeprecated`). The 5,679 records with `+0x14 = 1` are all normal-map
  formats: 4,388 DXN, 944 DXT5, 347 X8R8G8B8 (every X8R8G8B8 record). PROVEN.

## Record payload

```text
u32   name_len            at most 0x400 (ReadTextureDeprecated rejects more)
char  name[name_len]      NUL-terminated
u32   format              Xbox 360 D3DFORMAT
u32   flags
u16   width, height, mips
u32   unpacked            bytes of the inflated memory image
u32   stream_count        0 means 1
{ u32 packed; u8 zlib[packed] }[stream_count]
u8    trailer[8]          "MSHC" + 4 bytes; not read by the engine
```

Big-endian throughout. Readers: `ReadTextureDeprecated` (@0x823706d8; `WildStar_d` @0x8265ad48) and
the streamed `ReadTexture` / `DecompressTexture` (@0x8237bcf8 / @0x8237c810), which inflate the
streams straight into the texture's locked memory.

* 47,303 records have one stream, 9 have two, 1 has three; every record's streams inflate to
  exactly `unpacked` bytes and every record ends with the 8-byte `MSHC` trailer. PROVEN (probe).
* **Formats** — the values are the `D3DFORMAT` enum of `WildStar_p.pdb`:

  | `format` | name | element | records |
  |---|---|---|---|
  | `0x1A200152` | `D3DFMT_DXT1` | 4×4 block, 8 B | 14,037 |
  | `0x1A200154` | `D3DFMT_DXT5` (= `DXT4`) | 4×4 block, 16 B | 4,388 |
  | `0x1A200171` | `D3DFMT_DXN` (BC5: two BC4 channels) | 4×4 block, 16 B | 6,563 |
  | `0x28280144` | `D3DFMT_R5G6B5` | 2 B texel | 21,978 |
  | `0x28280186` | `D3DFMT_X8R8G8B8` | 4 B texel | 347 |

  `D3DFMT_A8R8G8B8` is `0x18280186`; no record uses it. PROVEN.
* **sRGB.** `ReadTextureDeprecated` @0x82370b50 tests `flags & 0x79`; when it is zero it ORs
  `0x7E00` into the format (`ori r24,r29,0x7e00` @0x82370b6c) — the `SIGN_X/Y/Z` fields set to
  `GPUSIGN_GAMMA`, i.e. the texture is sampled as sRGB. Per format: DXT1 flags `0x2`/`0x6` (10,920)
  sRGB, `0xE`/`0x26` (3,117) linear; DXT5 `0x2`/`0x6` (2,580) sRGB, the rest linear; every DXN,
  R5G6B5 and X8R8G8B8 record linear. PROVEN.

## Memory image

The inflated bytes are what the Xbox 360 GPU reads: the base level, then the mip chain, each level
**tiled** and each element **big-endian**. Implemented in `texture::levels`, `texture::tiled_offset`
and `texture::level_bytes`.

### Tiling

Element `(x, y)` of a surface whose pitch (in elements) is a multiple of 32 lives at
`XGAddress2DTiledOffset(x, y, pitch, log2(bytes per element))` — the public XG formula, as in Xenia
`texture_address::Tiled2D`. Elements are 4×4 blocks for DXT1/DXT5/DXN, texels otherwise.

Evidence: decoded with this addressing, each mip level matches the box-filtered decoded base (the
"mip vs base" rows below), while the same bytes read linearly do not; rendered, tiled reads give
coherent images (`DETAIL_Mud01_AB`, `DET_Cloth64_NM`, `T_StreetTile01_NM`) and linear reads
scramble them. PROVEN.

### Endianness

After untiling: DXT1/DXT5/DXN and R5G6B5 swap every 16-bit word (`GPUENDIAN_8IN16`); X8R8G8B8
reverses every 32-bit word (`GPUENDIAN_8IN32`). The result is the little-endian PC layout. PROVEN by
the same decodes.

### Mip layout

Following Xenia's `GetGuestTextureLayout` / `GetPackedMipOffset` (`src/xenia/gpu/texture_util.cc`)
for a tiled 2D texture with packed mips, mips stored after the base:

* **Base level**: pitch = width in elements aligned to 32, rows = height in elements aligned to 32;
  storage rounded up to 4 KiB. Offset 0.
* **Mip level `L`**: sized from the power-of-two dimensions,
  `max(1, next_pow2(width) >> L)` × `max(1, next_pow2(height) >> L)`, in elements, each aligned to 32,
  rounded up to 4 KiB; levels follow the base one after another.
* **Packed tail**: from level `P = max(log2_ceil(min(width, height)) - 4, 0)` on, every level shares
  level `P`'s storage. Level `P + k` sits at texel offset: for `k < 3`, `(0, 16 >> k)` when
  `log2_ceil(width) > log2_ceil(height)`, else `(16 >> k, 0)`; for `k ≥ 3`,
  `((1 << (log2_ceil(width) - P)) >> (k - 2), 0)` when wider, else
  `(0, (1 << (log2_ceil(height) - P)) >> (k - 2))`. Offsets divide by 4 for block formats.

Evidence (`sab_probe proto2008-textures`):

* The layout's extent equals `unpacked` for **47,313 / 47,313** records, over every shape in the
  build — square, 2:1, 1:8, the non-power-of-two `RainSplash_NC` (1344×64 DXT5: base 180,224 +
  131,072 + 65,536 = 376,832), single-mip and full-chain. PROVEN.
* Every level of every record lies inside the image. PROVEN.
* Each decoded level, compared with the decoded base box-filtered to its size (mean absolute RGB
  difference), against a control read of the same level placed wrongly (non-packed mips read
  linearly; packed mips read at the tail origin):

  | levels | layout closer | tie (±0.5) | control closer |
  |---|---|---|---|
  | non-packed mips | 76,000 | 303 | 0 |
  | packed mips | 125,567 | 748 | 60 + 341 |

  The 341 are levels under 4 texels on a side (one DXT block, where the box filter and the block's
  colour endpoints disagree), all DXT1. The 60 are DXT1 alpha-tested cut-outs and near-black masks
  where both reads score in the same noisy band; rendered, their tails are the right images
  (`FO_Grass02_LOD`: the layout row is the plant sprites shrinking; the control row is not).
  INFERRED for those 401; PROVEN for the rest.
* No record in the build has a packed base (`P = 0` with more than one mip); the rule for it
  (base at its tail offset in its own storage, mips in one level-0-sized storage) is Xenia's and is
  implemented, but unexercised by data.

## Loose `.dds` files

615 files under `Bin360/` (the root and `Branches/*/`), all `DDS ` + 124-byte header:

| four-cc | files | |
|---|---|---|
| `DXT5` | 505 | 2D |
| `DXT3` | 66 | 2D |
| `DXT4` | 18 | 2D (block layout = DXT5) |
| `DXT1` | 17 + 9 | 2D + cube maps (`DiffuseCube.dds` and siblings) |

* Linear, little-endian blocks — a PC DDS. Every face and level of all 615 decodes. PROVEN.
* 611 files end exactly after the header's surfaces (per face, the mip chain largest first); 4
  (`ocraext12.dds` and three others) carry 12 extra trailing bytes. PROVEN.
* 479 slot records share a name with a loose file's stem (many are generic, such as `_1`). Every
  pair differs in mip count; at the base level 27 (DXT5, e.g. the blank 32×32 `E_0Blank_NM`) decode
  identical, 26 within a mean difference of 16, and 426 differ outright. PROVEN. So the loose files
  are not the slot textures' sources — INFERRED.

## Method

```
sab_probe proto2008-textures <Bin360>        # every census number above
cargo test -p sab_formats --features retail --test proto2008_retail   # every record parses, hashes,
                                                                     # spans its layout, reads
```

## Unknowns

* How the material shaders reconstruct normals from DXN (two channels) and from DXT5 / X8R8G8B8
  normal maps — a material/shader question, not a container one.
* The 4 bytes after `MSHC` in the trailer (the engine does not read them).
* Why the exporter behind `SBL8`/`SBLX` writes the stored length into `+0x0C`.
