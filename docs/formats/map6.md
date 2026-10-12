# The Saboteur (2009) — `MAP6` world maps: `global.map` and `France.map`

Two `MAP6` files describe the world's objects: **`global.map`** (the boot-time registry of
non-placed objects — abilities, disguises, UI, …) and **`France.map`** (the region: terrain tiles, paths
and placed objects). Each object record carries the per-type blob counts its ALBS sub-pack needs
([sbla_subpack.md](sbla_subpack.md)). Re-derived 2026-10-12 from the engine's loaders and checked by
exact consumption of every retail file; supersedes an earlier version (see
[Correction](#correction-2026-10-12)).

All integers little-endian.

## Where the files are and who loads them — PROVEN

| file | where | loader |
|---|---|---|
| `global.map` (284,388 B) | inside `France/loosefiles_BinPC.pack` | `FUN_009f2df0`, called at `0x43c664` with `"global.map"` (`0xf7caec`) |
| `France.map` (2,817,849 B) | inside `France/loosefiles_BinPC.pack` | `FUN_009f4320`, called at `0x43c6a3` with `"%s.map"` (`0xf7caf8`) |
| `DLC/01/Global.map` (356 B) | loose | `FUN_009f3370` (via `FUN_009906c0`, `%s\\global.map`) |
| `DLC/01/FRANCE.map` (2,022 B) | loose | `FUN_009f75f0` (via `FUN_009906c0`, `%s\\%s.map`) |

The DLC loaders *add* to the main registry (e.g. `numDynamics` is accumulated into `this+0xA0`).

## The object record (shared by both files)

```
u32   hash          see below
u16   nameLen       including the NUL
u8    name[nameLen]
f32   a[3]          (France: bbox min)
f32   b[3]          (France: bbox max)
u16   u0
u16   u1
-- FUN_00658aa0 --
u32   n1;  n1 × { u32, u32 }      stored at +0x120; the 2nd word is summed into +0xF0
u32   n2;  n2 × { u32, u32 }      stored at +0x128
-- FUN_00659f20 / FUN_00659f50 --
u32   ?
u32   counts[9]                   one 0x24-byte read into +0x12C: the per-type ALBS blob counts
u32   nA;  nA × u32
u32   nB;  nB × u32               only the low 16 bits of nB are used
u32   nC;  nC × { u32, u32 k, k × u32 }
```

The tail is the same serialisation as an ALBS descriptor block. In retail, `nA = nB = nC = 0` in all 1,411
`global.map` records (PROVEN).

**`hash`** (checked on every retail record, 2026-10-12, `sab_probe albs-keys`): `pandemic_hash(name)` for
`global.map` dynamics and region-map list records; `pandemic_hash(name + "0")` / `(name + "1")` for
`global.map` list 1 / list 2 (325/325 each; `FUN_009f2df0` builds the object's name with `"%s%d"`). A
region tile's `hash` is none of these (0 of 21,678); not established.

**Pack path.** Each object's loader also stores a path on it (`"%s\\%s"` via `FUN_006586c0`), and the
megapack entry of the object's ALBS sub-pack is found by `pandemic_hash` of that path plus the pack
type: [world_megapack_key.md](world_megapack_key.md). `sab_formats::map6::pack_counts` gives each
object's counts keyed that way.

## `global.map` — PROVEN (`FUN_009f2df0`, decomp 884991–885091; `FUN_009f3370`, 885182–885357)

```
u32   magic = 0x4D415036 ("6PAM" on disk)
u32   numDynamics                       → this+0xA0
u32   count1;  count1 × Record
u32   count2;  count2 × Record
      numDynamics × Record              read by FUN_009f2a00
```

Retail: `numDynamics` 761, lists 325 + 325 — 1,411 records, consumed to the last byte. `DLC/01/Global.map`:
`numDynamics` 2 (`MM_Belle_VIP`, `KnifeThrow`; hashes `0x315C74B2`, `0xFB5E3070` verified), both lists
empty — 356 / 356 bytes.

## `France.map` — PROVEN (`FUN_009f4320`, decomp 885928–885952+; DLC `FUN_009f75f0`, 887940–887959)

Main file:

```
u32   magic
char  regionName[]                      NUL-terminated ("France")
u32   numTiles
u32   ?                                 → +0xDA84
u32   numPaths
      if numPaths > 0: u32, then numPaths × Path
      3 × (f32[3], f32[3])  and  3 × (u16, u16)
List 1  u32 count; count × Tile         FUN_009f3900
List 2  u32 count; count × Record       FUN_009f3bf0
List 3  u32 count; count × Record       FUN_009f3fa0
```

`DLC/01/FRANCE.map`: magic, `"FRANCE"`, `u32 0` (read by `FUN_00427cb0`), then the three lists.

**Tile** (`FUN_009f3900`): `u32`, `u16 nameLen`, optional name, 24 bytes, `u16`, `u16 selector`; when
`selector ≥ 2` the tile carries the full record tail above (the "extended" tiles).

**Path** — INFERRED: the decomp drops the arguments inside the path loop. SaboteurToolset's
`france_extract` structure (three length-prefixed strings + 48 bytes) consumes all 1,733 paths exactly.

Retail `France.map`: 1,733 paths, 29,572 tiles (21,669 extended), 13 + 61 records — consumed to the last
byte. `DLC/01/FRANCE.map`: 20 tiles (9 extended), 1 + 4 records — exact.

## Readers in this repo

| reader | verdict |
|---|---|
| `tools/sab_poc/src/albs.rs` (global.map) | right for `global.map` (main and DLC); skips the `nA/nB/nC` lists as a fixed 52 bytes, which holds only because they are empty in retail, and panics instead of erroring past EOF |
| SaboteurToolset `global_extract` / `france_extract` | exact on all four files |
| `tools/sab_map6` | **wrong**: its global parser fits only `DLC/01/Global.map` (both lists empty) and fails on the main file at `0xACFF`; its region parser loses sync on both France files |

## Correction (2026-10-12)

This doc used to say `global.map` was loaded by `FUN_009f3370` and France by `FUN_009f75f0` — those are
the **DLC** loaders. The "8 reserved bytes" after `record_count` are the two list counts (zero in the DLC
file). The "fixed 56-byte trailer" is `n2` + `u32` + the nine counts + `nA/nB/nC`, which totals 56 only
when every list is empty; its "`+0x0C` echoes `entry_count`" was a coincidence of `counts[1]` (texture
count). `France.map` was left undecoded; it is now consumed exactly.

## Adding a `global.map` record (for modders)

Append a Record to the `numDynamics` list and bump `numDynamics`: `hash = pandemic_hash(name)`,
`nameLen` (incl. NUL) + name, 24 zero bytes, `u0`/`u1` (copy an existing record of the same kind),
`n1`/`n2` lists, then the tail with `counts[9]` equal to the blob counts of the object's ALBS sub-pack
and empty `nA/nB/nC`.

## Method

`stage1_map/map_probe` (std-only Rust; to be moved into `tools/sab_probe`) parses each file with the
engine-derived layout, the `sab_poc` layout and the `sab_map6` layout and reports whether each consumes
the file exactly.
