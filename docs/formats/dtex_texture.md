# DTEX — The Saboteur (2009) texture format

Byte-level spec for the standalone texture ("DTEX") stored inside `SBLA`/`ALBS` sub-packs, with
a read/write path (`tools/sab_dtex`). Reverse-engineered from real bytes in
`Global/Palettes0.megapack` + `Global/Dynamic0.megapack` and corroborated against the clean
Saboteur.exe decomp. **Validated on 12,559 textures** (see [Validation](#validation)).

> There is **no `DTEX` 4CC on disk.** The docs' name is conceptual. A texture is a
> length-prefixed record living inside an `ALBS` sub-pack. `tools/sab_dtex` treats the exact
> bytes of one such record as a standalone `.dtex` file.

## Container context

```
.megapack (00PM)                       outer archive (see archive_and_models.md, tools/sab_pack)
  └── SBLA sub-pack ("ALBS")           per-asset bundle; a texture bundle holds 1..N textures
        └── texture record  ×N         ← the "DTEX" — spec below, packed back-to-back, no padding
```

A sub-pack begins `41 4C 42 53` ("ALBS"), `u32 0`, `u32 nameHash` (pandemic_hash of the pack
name), `u32 0`, then two size words and a small resource directory, then the texture records.
`sab_dtex list`/`carve` locate records by the `{u32 nameLen}{ascii name}{format}` signature
(robust; used to carve the 12,559-texture corpus with zero misses).

## Texture record layout (little-endian)

Let `N = 4 + nameLen` (byte offset of the `format` field).

| Off | Type | Field | Notes |
|---|---|---|---|
| `+0x00` | u32 | `nameLen` | length of the ASCII name |
| `+0x04` | char[nameLen] | `name` | e.g. `"Barge_Wall01_NM"` — plaintext, not hashed |
| `N+0x00` | u32 | `format` | D3DFMT int **or** ASCII 4CC (see table) |
| `N+0x04` | u32 | `flags` | usage bitfield (see below) |
| `N+0x08` | u16 | `width` | top-mip width |
| `N+0x0A` | u16 | `height` | top-mip height |
| `N+0x0C` | u16 | `mipCount` | number of mip levels |
| `N+0x0E` | u32 | `uncompressedSize` | = Σ over streams of decompressed length |
| `N+0x12` | u32 | `numStreams` | zlib streams that follow |
| `N+0x16` | — | `stream[numStreams]` | each: `u32 compressedSize` + `compressedSize` bytes of zlib |

Concatenating every decompressed stream yields the **mip payload**: for each mip level, in order
largest→smallest, a **24-byte descriptor** followed by that mip's pixel bytes.

### MipDesc (24 bytes)

| Off | Type | Field | Value |
|---|---|---|---|
| `+0x00` | u32 | `mipIndex` | 0,1,2,… |
| `+0x04` | u32 | `width` | `max(1, topWidth >> mipIndex)` |
| `+0x08` | u32 | `height` | `max(1, topHeight >> mipIndex)` |
| `+0x0C` | u32 | `pad0` | always `0` |
| `+0x10` | u32 | `one` | always `1` (slice/depth count) |
| `+0x14` | u32 | `mipDataSize` | bytes of pixel data that follow |

Therefore `uncompressedSize == Σ_mips (24 + mipDataSize)` — this identity holds exactly for all
12,559 corpus textures and is the key to reconstructing the payload on write.

### Streams

`numStreams` is 1 for almost every texture. Large textures split the mip payload at **`0x180000`
(1,572,864) uncompressed-byte boundaries**, each chunk zlib-compressed independently, and the
reader simply concatenates. Example: `BDNuit_Screen01_AB` (DXT5 2048², 12 mips, payload
5,592,720) → 4 streams `[1572864,1572864,1572864,874128]`. The split can fall mid-mip; concatenate
first, then walk descriptors.

Every stream is a standard **zlib** wrapper (`78 01`, CMF=0x78/32K window, FLEVEL=fast) —
`u16 header + raw DEFLATE + u32 adler32`.

## Format codes

**`format` is a Direct3D 9 `D3DFORMAT`, passed through untouched (PROVEN, 2026-10-12).** The engine
never maps it: the header reader (`0x660470` region) stores the u32 (`0x6604e1..e8`) into the texture
descriptor at `+0xC` (`0x660a66`/`0x660ab8`); the upload callback `0x65fa90` pushes `desc+0xC`
(`0x65faf8`) to renderer vtable `+0x150` (`0x65fafd`, vtable `0x0108007c` → `0x00dd9d80`), which
forwards it to `FUN_00dedac0`, whose device call `+0x5c` takes `(w, h, levels, usage, format, pool)`
— `IDirect3DDevice9::CreateTexture` (vtable index 23; INFERRED from slot + argument shape). So any
D3DFORMAT is legal, and the DXT codes are the D3D `MAKEFOURCC` values. The engine never derives
bytes-per-pixel either: both mip-upload loops take the row pitch as `MipDesc.dataSize / height`
(`0x65fbcd..d5`, `0x6501c4..cc`). `FUN_00dee1c0` only asks "is this DXT1/3/5".

> **Correction:** this section used to call `FUN_009bb910` "the engine's format map". It is the
> Scaleform GFx image path's D3DFORMAT → `GImage` format mapper (only caller `FUN_009bf880`); it plays
> no part in loading a DTEX.

Retail census (all 7 Steam megapacks, every record inflated and mip-walked, no format whitelist):

| `format` | D3DFORMAT | bytes/px or /4×4 block | Retail records | Grade |
|---|---|---|---:|---|
| `0x31545844` | `DXT1` | 8 / block | 14,557 | PROVEN |
| `0x35545844` | `DXT5` | 16 / block | 2,515 | PROVEN |
| `0x1A` | `A4R4G4B4` | 2 / px | **3,440** (`France_*_Mask`, 160², flags `0xA0`) | PROVEN (mip0 = w·h·2 on all) |
| `0x15` | `A8R8G8B8` | 4 / px | 20 | PROVEN |
| `0x33545844` | `DXT3` | 16 / block | 0 | INFERRED (passthrough) |
| `0x16` | `X8R8G8B8` | 4 / px | 0 | INFERRED (passthrough) |
| `0x17` | `R5G6B5` | 2 / px | 0 | INFERRED |
| `0x14` | `R8G8B8` | **3** / px | 0 | INFERRED |
| `0x1C` / `0x32` | `A8` / `L8` | 1 / px | 0 | INFERRED |
| `0x21` | `X8B8G8R8` | 4 / px | 0 | INFERRED |

20,532 records in all. **`0x1A` was missing from every reader in the repo**, whose whitelist scanners
silently skipped all 3,440 mask textures (17%); earlier tables here also had `0x14` as X8R8G8B8 and
`0x16` as R5G6B5 — both wrong per the D3DFORMAT enum. `mipDataSize` in each descriptor equals the
format formula: DXT `ceil(w/4)·ceil(h/4)·blockBytes`, uncompressed `w·h·bytesPerPixel`.

## Flags (`format+0x04`) — INFERRED

Usage bitfield; observed values in Palettes0 and their correlation with texture-name suffix:

| flags | correlates with | count |
|---|---|---|
| `0x6` | base/diffuse (DXT1) | 1291 |
| `0x7` | `_NM` normal maps (DXT1) | 1134 |
| `0xe` | `_S` specular (DXT1) | 819 |
| `0x16` | DXT5 (alpha) diffuse | 213 |
| `0x26`,`0x46`,`0x85`,`0x86` | other/atlas | rare |

Reading: low bits look like a usage class (`0x6` base, `|1` normal, `|8` spec, `|0x10` DXT5/alpha);
`0x40`/`0x80` are set on a handful. Not needed to decode pixels — `sab_dtex` preserves it verbatim
from the template on write.

## `tools/sab_dtex`

```
info    <in.dtex>                      header + mip table
extract <in.dtex> <out.dds>            decode → DDS (DXT1/3/5 via FOURCC, A8R8G8B8/A8/L8 via masks)
pack    <in.dds> <template.dtex> <out.dtex> [--preserve]
                                       rebuild; --preserve reuses template's exact zlib streams
list    <in.sub>                       enumerate texture records in an ALBS sub-pack
carve   <in.sub> <name> <out.dtex>     dump one record verbatim
roundtrip <in.dtex>                    the 3-oracle self-test below
```

Modding flow: `sab_pack extract` a sub-pack → `sab_dtex carve`/`extract` to DDS → edit → `pack` →
splice back into the sub-pack and rebuild the `patchpalettes0.megapack` with `sab_pack`.

## Validation

`sab_dtex roundtrip` runs three oracles per texture:

1. **Descriptor rebuild** — regenerate the 24-byte descriptors from decoded pixels; the rebuilt
   payload must be **byte-identical** to the original decompressed payload.
2. **Container rebuild** — reserialize the whole record reusing the original zlib streams; must be
   **byte-identical** to the on-disk record.
3. **Semantic** — recompress with modern zlib, re-decode; pixels must be identical.

Results:

- **Rust `roundtrip`: 228 / 228** carved records (DXT1, DXT5, A8R8G8B8, single- and 4-stream,
  32² … 2048²) pass all three oracles.
- **Full-corpus decode (Python model of the same spec): 12,559 / 12,559** textures
  (Palettes0 3,579 + Dynamic0 8,980) inflate to exactly `uncompressedSize` and match the
  descriptor/format model with **zero** parse failures.
- `pack --preserve` reproduces a carved DTEX **byte-for-byte**; `extract`→edit→`pack`→`extract`
  preserves edits; the emitted DDS opens in standard tools (verified 128² DXT5 → RGBA PNG via PIL).

### Honest limitation — zlib deflate reproduction

The 2009 build's DEFLATE output is **not reproducible** by modern zlib/miniz: the adler32 trailers
match (uncompressed data is identical) but no `level × memLevel × strategy` combination reproduces
the deflate body — the match-finder differs by zlib version. Consequences:

- Fully byte-identical repack requires `--preserve` (reuse original streams) — proves the container
  model is complete.
- `pack` on **edited** pixels recompresses; the DTEX is valid and engine-loadable (the loader only
  `zlib.decompress`es) but a few % larger and not byte-identical to a hypothetical original.

## Confidence

| Claim | Status | Evidence |
|---|---|---|
| Record framing (nameLen, name, format, flags, w, h, mip, unc, nStreams) | **CONFIRMED** | 12,559 textures parse; `uncompressedSize` identity exact |
| 24-byte MipDesc `{idx,w,h,0,1,size}` | **CONFIRMED** | every mip matches format formula; sums to `unc` |
| Multi-stream = payload split at `0x180000`, concatenated | **CONFIRMED** | 4-stream texture decompresses to exact `unc` |
| Streams are standard zlib | **CONFIRMED** | inflate + adler32 verified on all 12,559 |
| `format` = D3DFORMAT passed to `CreateTexture` unmapped | **CONFIRMED** | `0x660470` region → desc `+0xC` → `0x65fa90` → vtable `+0x150` → `FUN_00dedac0`; census of 20,532 retail records (§Format codes) |
| ALBS magic `0x53424C41` dispatch | **CONFIRMED** | decomp @0x00e34f70-region; sub-pack bytes |
| `flags` semantics (usage bits) | **INFERRED** | name-suffix correlation only; preserved verbatim on write |
| A4R4G4B4 (`0x1A`) | **CONFIRMED** | 3,440 retail mask textures, mip0 = w·h·2 on every one |
| DXT3 / X8R8G8B8 / R5G6B5 / R8G8B8 / A8 / L8 decode | **INFERRED** | D3DFORMAT passthrough; not exercised (absent from retail) |
| DEFLATE byte-reproduction | **NOT ACHIEVABLE** | modern zlib can't match 2009 encoder (adler ok, body differs) |
