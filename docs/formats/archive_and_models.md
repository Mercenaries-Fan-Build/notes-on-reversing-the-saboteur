# Archives & Models

How The Saboteur stores and overrides assets, and the model (MESH) format. Cross-checked against the
clean decomp and PredatorCZ/SaboteurToolset. See [`../lineage_and_divergence.md`](../lineage_and_divergence.md)
for how each of these differs from Mercenaries 2 — **do not** assume Mercs 2 UCFX/FFCS layout here.

## Archive hierarchy

```
.megapack (magic MP00 / "00PM" LE)        outer archive; 64-bit offsets
  └── SBLA sub-pack (magic "ALBS" LE)      per-asset bundle (meshes/textures/physics/layout)
        └── MSHA wrapper                    wraps a MESH (metadata) + .dat (VB/IB) pair, with name + comp info
              └── MESH                       flat model
        └── DTEX                             standalone texture (see "DTEX texture blob" below)
  compression: sges (SEGS)                   same as Mercs 2, byte-identical
```

On-disk containers (retail install):

| File | Magic | Contents |
|---|---|---|
| `France\Mega0/1/2.megapack` | MP00 | world + models (~2.5 GB) |
| `Global\Dynamic0.megapack` | MP00 | dynamic objects (vehicles, characters) |
| `Global\Palettes0.megapack` | MP00 | palette/shared assets |
| `France\Start0.kiloPack`, `BelleStart0.kiloPack` | MP00 | startup bundles (same format, smaller) |
| `DLC\01\*.dynpack` | SBLA | loose DLC dynamic packs |
| `particle.pack` | FX01 | particle effects |
| `France\loosefiles_BinPC.pack` | (non-MP00) | loose binary configs/scripts |

Megapack index entry (`megapack.hpp`): `{ uint32 crc; uint32 index; uint32 size; uint64 offset; }`.
No embedded path strings (unlike Mercs 2 PTHS) — hash-only, resolved via an external string dictionary.

## Byte order & the two FourCC conventions  ★ read before writing anything

Everything numeric is **little-endian** — every `u16`/`u32`/`u64` (counts, sizes, offsets, CRCs, w/h/mips,
the `21` = B8G8R8A8 texture-format id). The one trap is the FourCC magics, and there are **two conventions
in the same files**, both verified against real bytes *and* the engine's own compare constants:

| Magic | Kind | Source constant | On disk | Engine compares (decomp) |
|---|---|---|---|---|
| megapack | Pandemic container | `'MP00'` = `0x4D503030` | **`00PM`** (reversed) | `iVar4 == 0x4d503030` |
| SBLA bundle | Pandemic container | `'SBLA'` = `0x53424C41` | **`ALBS`** (reversed) | `iVar3 == 0x53424c41` |
| global.map | Pandemic container | `'MAP6'` = `0x4D415036` | **`6PAM`** (reversed) | `iVar3 == 0x4d415036` |
| DTEX format | D3D `MAKEFOURCC` | `DXT1` = `0x31545844` | **`DXT1`** (forward) | `param_1 == 0x31545844` (also `DXT3`/`DXT5`) |

Pandemic's container tags are C multi-character constants (first char in the **high** byte), so LE
serialization writes them **reversed** on disk (`00PM`, `ALBS`, `6PAM` — this is why the repo's `MP00`/
`SBLA`/`MAP6` names look "backwards" versus a hex dump). The texture-format tags come from DirectX's
`MAKEFOURCC` (first char in the **low** byte), so they land **forward** (`DXT1`). Both describe the same 4
bytes from opposite ends.

> **Byte-right rule:** treat every magic as an opaque 4-byte sequence — read it raw, compare by bytes, write
> it back verbatim. Never rebuild a magic by packing its pretty name; you'd have to know which convention it
> follows. (This is exactly why a verbatim-copy rebuild round-trips and a name-reconstructed one may not.)

## Megapack on-disk layout — writing a loadable one

The reader-facing struct above is not the whole file. Verified byte-for-byte against `Dynamic0`,
`Palettes0`, and `Mega0`, and matched to the loader `FUN_00e428c0` @ `0x00e428c0`:

```
'00PM' | count:u32
  main TOC   : count × { crc:u32; index:u32; size:u32; offset:u64 }   (20 bytes)   ← the documented struct
  index tbl  : count × { crc:u32; index:u32 }                         (8 bytes)    ★ SECOND table, easily missed
  0xCB pad   → first blob (0x800-aligned)
  blobs        each at its TOC `offset`, 0x800-aligned, 0xCB filler between
```

- **The second `(crc,index)` table is real and engine-loaded** — the loader allocates a `count*8` array
  (`param_1[0xf6]`) and fills it in a read loop *after* the main TOC; on disk it mirrors the TOC's
  `(crc,index)` in the same order. A rebuild that emits `header + TOC + blobs` and treats the rest as
  padding **drops it**, producing a file that *inspects* fine (reading only needs the main TOC) but is
  malformed for load. *(Table existence & engine read: confirmed. That omitting it is fatal: strong
  inference, not yet game-tested.)* Both known community writers currently omit it (`toc_start =
  align(8 + count*20)`), and neither self-tests a full-megapack byte match — see
  [`../community_tooling.md`](../community_tooling.md).
- **`offset` is `u64`** (record is 20 bytes, not 16). Packing it as `u32` desyncs every entry after the first.
- **The `0xCB` inter-blob filler is inert** — the loader fetches each blob by TOC `offset`+`size` and never
  reads the gaps; `0x00` filler loads fine. The pad *value* is not a correctness constraint (matching vanilla
  is only cosmetic).
- **`crc` is a lookup key, not a content checksum** — reuse each entry's original `crc` for a modified blob
  (the patch-layer approach depends on this).
- Resizing a blob means recomputing **all** later `offset`s to `0x800` alignment and rewriting both tables.

### DTEX texture blob — the decompressed contract
Inside an SBLA bundle a texture is a DTEX blob: `u32 nameLen; char name[]; u32 format(DXT1/DXT5/21);
u32 unk; u16 w,h,mips; u32 uncompressedSize; u32 numStreams; numStreams × { u32 len; zlib(chunk) }`.
Two details that bite (documented by the community "Saboteur Toolkit"; **byte-verified here** on retail
`Dynamic0`): concatenating the decompressed streams gives one logical stream with a **24-byte header before
*each* mip** (`{mipIdx,w,h,0,1,mipSize}`), so `uncompressedSize == Σ mipSizes + 24·numMips`; and streams are
fixed **1.5 MiB (`0x180000`) uncompressed chunks**, each zlib'd separately. Get the interleave wrong and the
engine's mip walk desyncs and it **crashes before the menu**. The engine only consumes the *decompressed*
stream, so a modified texture need not reproduce vanilla compressed bytes — the integrity gate is this
invariant, not a byte match.

> **Cross-SKU note:** the asset containers are byte-identical between the GOG and Steam builds
> (`Dynamic0`/`Palettes0` sha256-equal, `global.map` identical). `loosefiles_BinPC.pack` differs by exactly
> one 20-byte high-entropy region inside `France.shaders` (a per-copy DRM/SKU watermark) — irrelevant to any
> megapack/texture mod, but a full `loosefiles` rebuilder would need to preserve it per SKU. So a byte-right
> mod built against one storefront is byte-right on the other.

## Asset override — the built-in patch layer  ★ the "vz-patch.wad parallel"

Decomp-confirmed (mounter `FUN_00e34f70(name, 1, 0x600, 0x180, priority)`; caller ~`FUN_009f2xxx`).
Right after mounting each base pack, the engine unconditionally looks for a `patch*` sibling and mounts
it at a **~1000× higher priority**, so it wins the by-hash lookup:

| Base pack | Patch overlay | Priority (base → patch) |
|---|---|---|
| `France\Mega0.megapack` … | `patchmega0.megapack` / `patchmega%d.megapack` | — |
| `Global\Dynamic0.megapack` | `patchdynamic0.megapack` | 100 → 100,100 (0x18704) |
| `Global\Palettes0.megapack` | `patchpalettes0.megapack` | 90 → 100,090 (0x186fa) |

Each mount is guarded by a "slot == -1" check (mount once). This is "highest-priority-wins" like Mercs 1's
`RedVirtualDisk` "last-opened-file wins."

**To mod an asset:** build a megapack containing the replacement `SBLA`/`MSHA` under the **same asset
hash**, name it `patchmega0.megapack` (or `patchdynamic0`/`patchpalettes0`), drop it next to the base
pack. No base rebuild, no block-injection surgery — much cleaner than Mercs 2's WAD overlay. For the exact
byte layout a writer must reproduce (including the second index table both community tools miss), see
"Megapack on-disk layout" above; for the community texture writer, see
[`../community_tooling.md`](../community_tooling.md).

## MESH format (from SaboteurToolset `mesh/mesh_to_gltf.cpp`, cross-checked vs decomp)

Flat binary (no chunk tree):

```
MESH header:  BBOX, name(hash), numBones0, numBoneRemaps, numStreams, numPrimitives, numDrawCalls
[skeleton]:   boneIds[], localTMS[], bones[], transforms[], parentIds[]   (if skinned)
BoneRemaps[]  (if skinned)
Streams[]     vertex/index buffer descriptors (offsets into companion .dat)
Primitives[]  sub-mesh draw ranges (per-primitive BBOX)
DrawCalls[]   { uint32 primitiveIndex; hash material; uint32 0; uint16 parentBone; uint16 unk; }
              -- 16 bytes each. (Corrected 2026-07-24: the u32 zero between `material` and
              -- `parentBone` was missing here, making the record read as 12 bytes. The 16-byte
              -- form is what mesh_geometry.md documents and is what makes Sean's tail cursor
              -- land exactly on 36600/36600 with 0 leftover.)
```

Stream descriptor: `{ numVertices, format, vbOffset, vbSize, vbStride, ibOffset, ibSize, faceType, numIndices }`.
`faceType == 1` always → **triangle lists** (no strips, unlike Mercs 2).

Vertex `format` is an explicit **bitfield** (not stride-guessed):
`positionType:2 | skinType:2 | numColors:4 | numUVs:4 | normal:1 | tangent:1 | reserved:10 | constTag:8`
with `constTag == 0x1B`. ~18 known codes, e.g.:

| Code | Layout |
|---|---|
| `0x1b001102` | Pos(f16) + UV + Normal |
| `0x1b003102` | Pos(f16) + UV + Normal + Tangent |
| `0x1b001106` | Pos(f16) + BoneWeights + BoneIndices + UV + Normal |
| `0x1b003106` | + Tangent |

- Positions: half-float (R16G16B16A16). Skinning: 4-bone, UNORM8 weights / UINT8 indices.
- Materials are **external** — the drawcall references a material by hash into `.materials` (`WSAO`)
  files, not an inline chunk. LOD/damage-state handling not yet observed in the toolset (TBD).
- **LOD (2026-10-11, decomp + retail data):** two mechanisms. In-mesh levels are the low 3 bits of
  `DrawCall +0xe`, used only by unskinned meshes (vehicles, weapons, props) — see
  [mesh_geometry.md](mesh_geometry.md). Characters instead swap whole meshes: each body-part
  template names a `HighDetailModel` and a `LowDetailModel` (usually the `_LOD` mesh) — see
  [gametemplates.md](gametemplates.md).
- **Damage states (2026-10-12):** a destructible is one mesh whose pieces hang off bones; GameTemplates
  damage groups name the bones and flag the pristine ones, and the state shows/hides their drawcalls.
  Separately-modelled `_DAM`/`_Destroyed` twins also exist for some world objects — see
  [damage_and_destruction.md](../damage_and_destruction.md).

## Status / next

✅ **Superseded 2026-07-24.** This section used to say "no Rust reader written yet" and propose
validating via SaboteurToolset. Both the readers **and** the writers now exist in this repo:

| Layer | Tool | Notes |
|---|---|---|
| MP00 megapack | [`sab_pack`](../../tools/sab_pack/README.md) | list / extract / pack / roundtrip / **patch** |
| ALBS sub-pack | [`sab_sbla`](../../tools/sab_sbla/README.md) | list / rebuild / **replace** / scan — byte-identical rebuild over 1042 sub-packs |
| MESH / MSHA | [`sab_mesh`](../../tools/sab_mesh/README.md), [`sab_skeleton`](../../tools/sab_skeleton/README.md) | decode → glTF |
| DTEX | [`sab_dtex`](../../tools/sab_dtex/README.md) | DTEX ⇄ DDS, 12,426/12,426 retail textures decode |
| MAP6 | [`sab_map6`](../../tools/sab_map6/README.md) | |
| shared codecs | `tools/sab_formats` | the library the others build on (no README yet — see the module docs in `src/`) |
| mod audit | [`sab_validator`](../../tools/sab_validator/README.md) | parses a mod the way the engine's mount path does |

Remaining genuine gap in *this* area: how world placements swap an intact mesh for its separately
modelled `_DAM` twin (needs `France.map` placements decoded — see
[damage_and_destruction.md](../damage_and_destruction.md) §10). LOD and in-mesh damage states are
documented above.
