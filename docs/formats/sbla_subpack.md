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
| 8 | the separately streamed part: `hash == 0` and `comp == uncomp` on all 15,860; `FUN_00658870` seeks to `base + Σcomp(types 6,7,0,2)` and reads table 8 | PROVEN |
| 3, 4, 5, 6 | zero in every record whose counts are known | — |

**The descriptor block** (`FUN_0065a470`), the same serialisation as a `global.map` record tail:

1. If `obj.flags & 0x1C00 == 0x400` (`0x65a49a`): a heightfield — `"HEI1"`, `u32 w`, `u32 h`, two
   `f32`, then `w·h` bytes.
2. `FUN_00658aad`: two lists, each `u32 n` + `n × 8` bytes.
3. `FUN_00659f20`: `u32` + the nine `u32` counts (0x24 bytes).
4. `FUN_00659f50`: `u32 n` + `4n`, `u32 m` + `4m`, `u32 L` + `L × (u32 id, u32 k, k × u32)`.

**Telling the variants apart** — `X == 0`: an object pack (counts in a `.map`). `X == 0x3C`: a
streamblock with an empty descriptor. `X ≥ 0xB4` with `"HEI1"` at `+8`: a terrain cell.

## Retail survey

9,200 ALBS sub-packs (Dynamic0, Palettes0, Mega0/1/2, three kiloPacks, DLC megapacks and loose files):

| | packs | engine rule |
|---|---:|---|
| `X ≥ 0xB4` terrain cells (all of Mega1 6,727 and Mega2 1,004; 12 elsewhere) | 7,743 | exact to EOF |
| `X == 0x3C` | 188 | exact to EOF |
| `X == 0`, counts from `global.map` (1,411 records) | 1,082 | exact to EOF |
| `X == 0`, counts in `France.map` (not yet parsed) | 187 | not checked |

**9,013 / 9,013 packs with known counts end exactly at EOF; the `offset` rule holds in all of them.**

## Correction (2026-10-12)

This doc used to describe `+0x04` as a `variant` word (0 object / 0x3C streamblock), `+0x08..+0x1F` as a
header (`packNameHash`, `leadCompSize`, …), a directory at `0x20` or `0x44`, a "MIDDLE" region and
optional footers. For `X == 0` packs that "header" **is record 0** (`packNameHash` is its hash,
`leadCompSize` its comp) and "MIDDLE" is record 0's blob — which is why it gave the right blob ranges in
998 packs. It fails on packs with one record ("empty directory", 74) or none (34), on `comp == 0` slots
(ranges 24 bytes early), and on every terrain cell (`X` read as an "unknown flags variant": 7,743). The
"57 packs with a genuine small footer" were misparses: with known counts the engine finds no trailing
bytes. `0x44` was right for `X == 0x3C` only because `0x44 = 8 + 0x3C`. `sab_formats::sbla` and
`tools/sab_sbla` implement the old reading and are being replaced.

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
