# SBL-family sub-pack variants (2008-05 Xbox 360 prototype)

The initial probe (`docs/2008_subpack_format.md`) cracked `SBL4`, the most common sub-pack magic
(2612 of 4396 `.Pack` files). This follow-up widens the reader to the six remaining SBL-family
magics — `SBLL`, `SBL8`, `SBL6`, `SBLX`, `SBL0`, `SBL5` — covering another **1777 files**, i.e.
every `.Pack` on disk except the empty ones and the `MP00` / `AP04` outliers.

**Result: one parser, seven magics.** All seven variants use the identical 8-byte header +
24-byte record + contiguous big-endian data layout. The 4-byte magic suffix is a family tag,
not a format switch. Parsing succeeds on 4389/4389 SBL-family files.

Reader lives in `tools/sab_workshop/src/sources/prototype2008_subpack.rs` (`SubPack::parse` /
`.find` / `.slot_bytes` / `.magic_str`); the `Sbl4SubPack` / `Sbl4Record` type aliases keep
callers written against the SBL4-only name compiling. Wired into
`Prototype2008Source::open`'s material walk and `read_mesh_bytes`.

## Corpus stats

Every `.Pack` under `Bin360/` grouped by first-4-byte magic (from `magic_scan.py`):

| Magic  | Files   | Median size | Parse OK    | Verdict |
| ------ | ------- | ----------- | ----------- | ------- |
| SBL4   | 2612    |    783106 B | 2612/2612   | cracked (initial probe) |
| SBLL   | 1265    |    260304 B | **1265/1265** | **cracked** |
| SBL8   |  234    |      6715 B | **234/234**   | **cracked** |
| SBL6   |  157    |    744963 B | **157/157**   | **cracked** |
| SBLX   |  107    |    351407 B | **107/107**   | **cracked** |
| SBL0   |   10    |    163903 B | **10/10**     | **cracked** |
| SBL5   |    4    |     12338 B | **4/4**       | **cracked** |
| MP00   |    4    |     ~2 GB   | (out of scope) | different family — `mega*.pack` streamblocks |
| AP04   |    1    |     14.9 MB | (out of scope) | different family — `Animations.pack` (`AP04ANIM…`) |
| empty  |    2    |         0 B | (nothing to parse) | zero-length placeholders |
| **SBL-family total** | **4389** | | **4389/4389** | **100%** |

## Why one parser handles all seven

Ran an invariant-strict parse of every `.Pack` under `Bin360/` with the SBL4 rules:

* magic ∈ {`SBL0`,`SBL4`,`SBL5`,`SBL6`,`SBL8`,`SBLL`,`SBLX`}
* 1 ≤ nEntries ≤ 4096
* offset[0] == dir_end == 8 + n×24
* offset[i] == offset[i-1] + size[i-1] for i ≥ 1
* offset[n-1] + size[n-1] == file_size
* size == size_dup on every record
* flag == 1 on every record

**Every non-SBL4 file passes every invariant** (validator: `scratchpad/validate_shape.py`,
which iterates the entire tree and prints pass/fail per magic — 4389/4389 OK). No exceptions,
no size divergence, no non-contiguous data, no alternate record stride. The variants are
byte-identical in shape.

## Header/record layout (same as SBL4, tag varies)

```text
HEADER  (8 bytes)
  +0x00  char  magic[4] = one of {"SBL0","SBL4","SBL5","SBL6","SBL8","SBLL","SBLX"}
  +0x04  u32   nEntries

DIRECTORY  (starts at +0x08; nEntries × 24 bytes)
  +0x00  u32   kind_hash
  +0x04  u32   offset      ABSOLUTE file offset
  +0x08  u32   size
  +0x0C  u32   size_dup    duplicate of size
  +0x10  u32   flag        always 1
  +0x14  u32   block_hash

DATA  (starts at dir_end = 8 + nEntries*24; contiguous, in directory order)
```

## Per-variant witness

Each variant has a dedicated test (`prototype2008_subpack.rs::tests`) that opens a specific
real `.Pack` of that magic, parses it, and verifies the numbers I hand-copied out of the hex
dump. The witness files are small (all ≤ 21 kB) so the tests are cheap.

### SBLL — France/327680.Pack (40 bytes, single record)

```
0000  53 42 4C 4C 00 00 00 01 9A B5 A3 51 00 00 00 20   SBLL.......Q...
0010  00 00 00 08 00 00 00 08 00 00 00 01 00 00 00 00   ................
0020  00 00 00 00 00 00 00 00                           ........
```
nEntries=1, rec(kind=0x9AB5A351, off=0x20, size=8). Payload = 8 zero bytes at offset 0x20 =
dir_end. Contiguous, size==size_dup, flag=1. ✓

The dominant SBLL shape (1257/1265) is 2-record `materials + meshes`; slot 0 always starts
with the `WSAO` magic — a straight-through payload identical to what a 2-slot SBL4 would
carry. Same content, different family tag.

### SBL8 — Global/A1Esc_Human_SS_Grenadier_RPG.Pack (6715 bytes, 4 records)

```
0000  53 42 4C 38 00 00 00 04 A4 0D 77 7D 00 00 00 68   SBL8......w}...h
0010  00 00 16 DA 00 00 16 DA 00 00 00 01 36 EC 2E 15   ............6...
0020  D7 28 29 F9 00 00 17 42 00 00 01 27 00 00 01 27   .()....B...'...'
...
```
nEntries=4, records = [textures@0x68 sz=0x16DA, materials@0x1742 sz=0x127, meshes@0x1869
sz=0x1A1, physics@0x1A0A sz=0x31]. Every record has block_hash=0x36EC2E15 (consistent).
Materials slot starts `WSAO ... MTLD`. ✓

All 234 SBL8 files are 4-record with this same slot set. Median size 6715 B — smaller than
SBL4 median, but the layout math is identical.

### SBL6 — France/1239297.Pack (164 bytes, 3 records)

```
0000  53 42 4C 36 00 00 00 03 9A B5 A3 51 00 00 00 50   SBL6.......Q...P
0010  00 00 00 08 00 00 00 08 00 00 00 01 00 00 00 00   ................
0020  E1 08 7B 27 00 00 00 58 00 00 00 24 00 00 00 24   ..{'...X...$...$
0030  00 00 00 01 00 00 00 00 D3 09 84 61 00 00 00 7C   ...........a...|
...
```
nEntries=3, records at offsets 0x50 / 0x58 / 0x7C, sizes 8 / 0x24 / 0x28. Last record ends
at 0x7C+0x28 = 0xA4 = 164 = file size. ✓

The 157 SBL6 files split three ways: 92 single-record (unknown-type slot only), 55 four-record
(same shape as SBL4's canonical materials/meshes/physics/textures), and a handful of 2/3-rec.

### SBLX — Global/barrel_of_boom_massivePush.Pack (21197 bytes, 4 records)

```
0000  53 42 4C 58 00 00 00 04 A4 0D 77 7D 00 00 00 68   SBLX......w}...h
0010  00 00 27 8D 00 00 27 8D 00 00 00 01 36 08 D6 DC   ..'...'.....6...
...
```
nEntries=4, records = [textures@0x68 sz=0x278D, materials@0x27F5 sz=0x146, …]. Materials
slot starts `WSAO`. ✓

Squib / effect blocks; 93/107 are 4-record like this one. Same layout as SBL4/SBL8.

### SBL0 — Global/Dock_Post0.Pack (60 bytes, single textures record)

```
0000  53 42 4C 30 00 00 00 01 A4 0D 77 7D 00 00 00 20   SBL0......w}... 
0010  00 00 00 1C 00 00 00 1C 00 00 00 01 09 2D 3C FF   .............-<.
...
```
nEntries=1, rec(kind=textures, off=0x20, size=0x1C, block_hash=0x092D3CFF). ✓

Ten files total, 6/10 are single-record textures-only.

### SBL5 — SultanOfSwing/16776960.Pack (4385 bytes, 3 records)

```
0000  53 42 4C 35 00 00 00 03 D7 28 29 F9 00 00 00 50   SBL5.....()....P
0010  00 00 01 3E 00 00 01 3E 00 00 00 01 00 00 00 00   ...>...>........
0020  FE 5E 3A 56 00 00 01 8E 00 00 0E FF 00 00 0E FF   .^:V............
0030  00 00 00 01 00 00 00 00 9A B5 A3 51 00 00 10 8D   ...........Q....
0040  00 00 00 94 00 00 00 94 00 00 00 01 00 00 00 00   ................
0050  57 53 41 4F 00 00 00 01 4D 54 4C 44 10 00 A8 A2   WSAO....MTLD....
```
nEntries=3, records at 0x50 (materials) / 0x18E (meshes) / 0x108D (0x9AB5A351). Materials
starts `WSAO...MTLD` at offset 0x50, exactly at dir_end. ✓

Only 4 files total (SultanOfSwing DLC + France 387074.Pack). Same layout as everyone else.

## What the 4th magic byte means (unresolved)

The variance in the last byte doesn't correlate with anything we can measure from the payload:
same slot types, same layouts, same block-hash placement. Some hypotheses that DID NOT hold
up when tested:

* **"Slot count encoded in the tag"**: `SBL4` isn't always 4-slot (histogram: 4-rec=1096, 7-rec=760,
  1-rec=479, …), and `SBL8` is always 4-rec, not 8. `SBL0` isn't 0-slot. So the digit isn't a
  count.
* **"Compression tier"**: no slot in any variant is compressed at the sub-pack level; every
  slot is stored raw with `size == size_dup`. Compression happens INSIDE `meshes` slots (zlib
  streams around MSH4 records) but that's the same regardless of the outer magic.
* **"Content mix"**: SBL4 files carry all four canonical types; SBL8 files also carry all
  four. SBLL is dominated by 2-record (`materials + meshes`) but includes 3-record variants.
  There's no clean slot-set → magic mapping.

Best guess without additional evidence: the tag records the **build stage** or **source
project** the block came from — SBL4 vs SBL8 correlate loosely with region (`Global/`
vs `Global/` character overlays), SBLX with squibs, SBL5 with the SultanOfSwing DLC. That
would make the tag cosmetic, which matches what the byte layout says.

Grepping `WildStar_d.pdb` (99.8% named per the memory index) for `SBL8` / `SBLL` / `SBundle`
symbols might resolve it. Not blocking anything downstream — the parser works either way.

## Caller-side changes

`Prototype2008Source::open`'s material walk now calls `SubPack::parse` (see
`tools/sab_workshop/src/sources/prototype2008.rs`) and tracks a per-magic counter that
surfaces in the fingerprint:

```
material bindings: 5333 materials indexed from 861/861 readable .materials slots
(sub-pack magics: [SBL4=846, SBL8=11, SBL0=2, SBL6=1, SBLX=1];
 non-family .Pack files skipped: 2; read errors: 0)
```

Every packlog-referenced materials pack now unwraps (post-widening: 861/861; pre-widening:
844/861, with 17 skipped as "non-SBL4"). The 2 skipped files are the two zero-byte `.Pack`
placeholders (`Global/CH_CV_M_Casual01.Pack` and `Global/Human_SS_Grenadier_RPG.Pack`).

`read_mesh_bytes` similarly widened. The `read_mesh_bytes_extracts_the_sbl4_meshes_slot`
test still round-trips: 3/3 probed handles yield a zlib stream that inflates to a MSH4
record — the SBL* container's shape guarantees the meshes slot boundary is right regardless
of the outer magic.

## Follow-ups (not this probe)

* **Packlog coverage is only ~24% of materials on disk.** The material walk iterates
  packlog-declared `.materials` entries, so it only sees 861 unique packs — but 3552 SBL-family
  `.Pack` files ship a materials slot (`SBL4` 1891 + `SBLL` 1257 + `SBL8` 234 + `SBLX` 105
  + `SBL6` 57 + `SBL0` 4 + `SBL5` 4). Widening the walk to enumerate every `.Pack` on disk
  would ~4x the material yield and pull in the full SBLL/SBLX population. Cheap change; kept
  separate from this probe so the widening lands in isolation.
* **The four unknown slot-type hashes** (`0x9AB5A351`, `0xE1087B27`, `0xD3098461`, `0xB5D2FE96`)
  from `docs/2008_subpack_format.md` remain unnamed; the widening doesn't change the picture
  — they surface across every variant with the same payload shapes.
* **`MP00`** mega streamblocks (four files, one over 2 GB) are a genuinely different family
  — starts differently, orders-of-magnitude larger. Needs its own probe.
* **`AP04ANIM`** is `Animations.pack` (14.9 MB); starts `41 50 30 34 41 4E 49 4D`, exposes an
  ANM0/WAV0 sub-structure at offset 0x18. Different family. Its own probe.
* **What the 4th magic byte encodes** is a puzzle worth chasing — `WildStar_d.pdb` grep should
  resolve it. Not blocking.

## Test coverage (this probe)

`tools/sab_workshop/src/sources/prototype2008_subpack.rs::tests`:

* `parses_act1_intkey_pack` / `find_and_slice_materials_slot` — legacy SBL4 witnesses, still
  green.
* `sbll_variant_parses_single_slot_pack`
* `sbll_variant_materials_slot_starts_with_wsao` — auto-discovers a 2-record SBLL
* `sbl8_variant_parses_known_pack`
* `sbl6_variant_parses_multi_record_pack`
* `sblx_variant_parses_known_pack`
* `sbl0_variant_parses_single_slot_textures`
* `sbl5_variant_parses_known_pack`
* `corpus_wide_sbl_family_parses_100_percent` — walks every `.Pack` under `Bin360/`, groups
  by magic, asserts 100% parse-success per magic AND the per-magic total counts match the
  numbers in this doc's table (so any silent corpus swap gets caught).
* `parses_a_hand_crafted_two_slot_sub_pack` — synthetic (runs anywhere).
* `rejects_wrong_magic_and_truncation` — now checks that `AP04` and `MP00` are refused, not
  just an unknown-magic string.

All pass on the current `Bin360/` snapshot.
