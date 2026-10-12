# SBLA / ALBS sub-pack format (read + write)

An `SBLA` ("ALBS" LE) sub-pack is the per-asset bundle inside a `.megapack` entry (one megapack index
entry → one ALBS sub-pack). It holds typed tables of compressed blobs (an `MSHA`/`AHSM` mesh, `DTEX`
textures, physics, UI…), optionally preceded by a world-cell descriptor (a terrain heightfield and
placement counts). Re-derived 2026-10-12 from the engine's reader (GOG exe, decomp + objdump) and
checked on every retail sub-pack; it supersedes an earlier reading that framed the first table record
as a "header" (see [Correction](#correction-2026-10-12)).

## Byte layout (little-endian) — PROVEN

```
+0x00  char  magic[4] = "ALBS"   (u32 0x53424C41; FUN_00658870 also accepts 0x41444139 — unused in retail)
+0x04  u32   X                   byte length of the descriptor block that follows (NOT a flags word)
+0x08  u8    descriptor[X]       present only when X != 0 — see below
8+X    Record[N]                 N = Σ counts[0..9]; 24 bytes each, grouped by type (order below)
tbl_end = 8 + X + 24·N
tbl_end Blob[N]                  back to back in record order; each record advances the cursor by comp
                                 (EOF follows the last blob: 0 trailing bytes in every pack whose
                                 counts are known)

Record (24 bytes)
  +0x00  u32  hash      pandemic_hash of the sub-asset name; 0 on every type-8 record
  +0x04  u32  offset    relative to tbl_end; = running Σcomp of the preceding records EXCLUDING type 8
                        (records with comp == 0 own no bytes; their offset follows no rule found)
  +0x08  u32  comp      stored size (the loader's read length)
  +0x0C  u32  uncomp    inflated size
  +0x10  u32  ?         not established
  +0x14  u32  ?         not established
```

**`FUN_0065a900`** reads `X` at `+4` (`0x65a921`); if `X != 0` it parses the descriptor with
`FUN_0065a470` (`0x65a954`) and returns `X + 4 + Σ24·count`, to which every caller adds 4 — so blobs
start at `tbl_end` (`FUN_00660e10`, `FUN_009f2530`).

**The counts.** Nine per-type counts, from one of two places:

* `X != 0` — inside the descriptor, read by the same `FUN_00659f20` that reads a `global.map` record
  tail ([map6.md](map6.md)): a `u32` then **nine u32 counts** in one 0x24-byte read into `obj+0x12C`.
* `X == 0` — from the object's record in `global.map` / `France.map` (the same nine counts).

**Table order** is `DAT_01121574 = [6, 7, 0, 2, 8, 4, 3, 1, 5]` (read from the exe's `.data`): type-6
records first, then 7, 0, 2, 8, 4, 3, 1, 5.

| type | content | grade |
|---|---|---|
| 0 | mesh (`AHSM` blob heads) | PROVEN |
| 1 | texture (DTEX records) | PROVEN |
| 7 | flash/UI (the 28 `global.map` names: `CommonUI`, `Font_*`, …) | INFERRED |
| 2 | physics | SaboteurToolset naming, unverified |
| 8 | the separately streamed part: `comp == uncomp` on all 16,202 (`hash == 0` on the 15,860 checked before the cinematic packs were keyed); `FUN_00658870` seeks to `base + Σcomp(types 6,7,0,2)` and reads table 8 | PROVEN |
| 3, 4, 5, 6 | zero in every record whose counts are known | — |

**The descriptor block** (`FUN_0065a470`), the same serialisation as a `global.map` record tail:

1. If `obj.flags & 0x1C00 == 0x400` (`obj+0x108`, `0x65a49a`): a heightfield — the tag `'HEI1'` as
   a little-endian u32 (bytes `"1IEH"`; the engine reads and discards it), `u32 w`, `u32 h`, two `f32`,
   then `w·h` bytes.
2. `FUN_00658aad`: two lists, each `u32 n` + `n × 8` bytes.
3. `FUN_00659f20`: `u32` + the nine `u32` counts (0x24 bytes).
4. `FUN_00659f50`: `u32 n` + `4n`, `u32 m` + `4m`, `u32 L` + `L × (u32 id, u32 k, k × u32)`.

**Telling the variants apart** — `X == 0`: an object pack (counts in a `.map`). `X == 0x3C`: a
streamblock with an empty descriptor. `X ≥ 0xB4` with the `'HEI1'` tag (bytes `"1IEH"`) at `+8`: a
terrain cell. Every terrain cell in retail carries the tag; `sab_formats::sbla` selects the
heightfield by it and requires the descriptor to consume exactly `X` bytes.

**Which map object.** The engine looks an object pack up by its **pack path**: the map loader stores a
path on the object, `FUN_00658870` appends `.pack` (`.dynpack`, `.palettepack` for the global packs)
and the resolver bsearches the megapack on `pandemic_hash` of it — the entry's `crc`. The counts it
then uses are that object's (`obj+0x12C`, read by the same `FUN_00659f20`). So for a `.megapack` entry
the counts are those of the map object whose pack path hashes to the entry's `crc`
(`sab_formats::map6::pack_counts`; the paths are in
[world_megapack_key.md](world_megapack_key.md)). For a `.kiloPack` entry, whose `crc` matches no known
path, the entry's `index` is the object's record hash (105/105); for a DLC loose pack,
`pandemic_hash(file stem)` is (`map6::object_counts`).

## Retail survey

9,210 ALBS sub-packs (Dynamic0, Palettes0, Mega0/1/2, three kiloPacks, DLC megapacks and loose files):

| | packs | counts from |
|---|---:|---|
| `X ≥ 0xB4` terrain cells (Mega1 6,727, Mega2 1,004, `dlc01mega0` 7, BelleStart0 5) | 7,743 | descriptor |
| `X == 0x3C` (Mega0 172, BelleStart0 9, `dlc01mega0` 4, Palettes0 2) | 187 | descriptor |
| `X == 0`, megapack entries: `global.map` dynamics 759 + DLC 2, list 2 319; `France.map` lists 2 + 3 72; DLC `FRANCE.map` 5 | 1,157 | map object by `crc` |
| `X == 0`, kiloPack entries (BelleStart0 81, Start0 12, Start_German0 12) | 105 | map object by `index` |
| `X == 0`, DLC loose packs | 18 | map object by file stem |

**9,210 / 9,210 end exactly at EOF and rebuild byte for byte** (2026-10-12, `sab_formats`
`tests/sbla_retail.rs`). The `offset` rule holds for every one of the 459,756 records with
`comp != 0`; the 19 records with `comp == 0` (all type 1, hash 0) store other values. Every type-0
blob is exactly one MSHA record ([mesh_geometry.md](mesh_geometry.md)): 208,681 / 208,681.

The 39 `France/Mega0.megapack` cinematic packs (#10–12, 21–23, 28–31, 41–46, 61–72, 86–96) were
once listed here as unkeyed because they were matched by `index`: their `index` is the leading number
of the cinematic name (`102_cinb_farmintro` → `0x66`), and their counts are those of the `France.map`
list-3 record whose path `France\<name>.pack` hashes to the `crc`.

## Correction (2026-10-12)

This doc used to describe `+0x04` as a `variant` word (0 object / 0x3C streamblock), `+0x08..+0x1F` as a
header (`packNameHash`, `leadCompSize`, …), a directory at `0x20` or `0x44`, a "MIDDLE" region and
optional footers. For `X == 0` packs that "header" **is record 0** (`packNameHash` is its hash,
`leadCompSize` its comp) and "MIDDLE" is record 0's blob — which is why it gave the right blob ranges in
998 packs. It fails on packs with one record ("empty directory", 74) or none (34), on `comp == 0` slots
(ranges 24 bytes early), and on every terrain cell (`X` read as an "unknown flags variant": 7,743). The
"57 packs with a genuine small footer" were misparses: with known counts the engine finds no trailing
bytes. `0x44` was right for `X == 0x3C` only because `0x44 = 8 + 0x3C`. `sab_formats::sbla` now
implements the layout above (read and write); `tools/sab_sbla` still implements the old reading.

## Tooling — `tools/sab_sbla` (std-only; old reading — being replaced)

> These tools implement the superseded framing above. Their byte-identical round trips are real but
> prove only that regions are relayed verbatim; a splice into an `X == 0` pack with type-8 records, a
> `comp == 0` slot, or any terrain cell would be misplaced.

`info`/`list` · `roundtrip` (parse → re-derive the directory from block lengths → re-emit; asserts
byte-identical) · `rebuild` · `replace <recIdx> <blob.bin> <uncSize>` (swap a record's compressed blob,
recompute its compSize + all downstream offsets, re-emit) · `extract` · `scan <megapack>` (rebuild every
in-scope sub-pack).

## Validation (oracle) & scope
`scan` reports **0 in-scope mismatches**: Dynamic0 722/759, Mega0 226/244, plus Palettes0/Start0/Belle —
1000+ sub-packs rebuilt byte-identical. Splice proven: no-op `replace` → byte-identical; a size-changing
`replace` shifts all downstream offsets by Δ and re-parses cleanly. Composes with `sab_pack` (spliced
sub-pack → single-entry megapack → re-scan round-trips).

**Refused by the old tools:** terrain/heightfield packs (`X ≥ 0xB4`, `HEI1`) and small one-record
packs — not a different layout, as this said, but the same layout with a descriptor block and/or one
record (see above). In-engine load of an edited pack is INFERRED (byte-faithful) — confirm with the
x32dbg validation.
