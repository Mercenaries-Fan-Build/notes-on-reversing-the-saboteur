# SBL4 sub-pack format (2008-05 Xbox 360 prototype)

`SBL4` is the outer container inside every 2008-build `.Pack` file. It is the 2008 counterpart of
retail's `ALBS`/`SBLA` sub-pack (see `docs/formats/sbla_subpack.md`) — same family, small variant,
much simpler shape. This doc records the layout, how it differs from retail, and what the packlog
sibling really means.

Reader lives in `tools/sab_workshop/src/sources/prototype2008_subpack.rs`
(`Sbl4SubPack::parse` / `.find` / `.slot_bytes`), wired into
`Prototype2008Source::read_mesh_bytes`.

## Byte layout (big-endian)

The prototype targets the Xbox 360, so **every field in a 2008 `.Pack` is big-endian**. Reading it
as little-endian silently produces plausible-looking-but-wrong numbers — the 4-record magic word at
+0x04 (BE = 4, LE = 0x04000000) is the earmark.

```
HEADER  (8 bytes)
  +0x00  char  magic[4] = "SBL4"           (raw ASCII, byte order as written)
  +0x04  u32   nEntries

DIRECTORY  (starts at +0x08; nEntries * 24 bytes)
  +0x00  u32   kind_hash    pandemic_hash of the slot type name
                            ("materials", "meshes", "physics", "textures", ...)
  +0x04  u32   offset       ABSOLUTE file offset of this slot's bytes
  +0x08  u32   size         slot byte length
  +0x0C  u32   size_dup     duplicate of `size` (see notes)
  +0x10  u32   flag         always 1 in every SBL4 sub-pack sampled
  +0x14  u32   block_hash   the containing block's hash

DATA  (starts at dir_end = 8 + nEntries*24)
  slot bytes back-to-back, in the SAME order as the directory:
    offset[0]   == dir_end
    offset[i]   == offset[i-1] + size[i-1]
    offset[n-1] + size[n-1] == file_end
```

### Field notes

* **`kind_hash`** — the slot type as a pandemic_hash. The four hashes we recognize today, all
  verified against every SBL4 record across `Bin360/`:

  | slot type   | pandemic_hash |
  | ----------- | ------------- |
  | `materials` | `0xD72829F9`  |
  | `meshes`    | `0xFE5E3A56`  |
  | `physics`   | `0x4445EA18`  |
  | `textures`  | `0xA40D777D`  |

  Four other hashes turn up in animation/streamblock/anim-test packs and are still open —
  `0x9AB5A351`, `0xE1087B27`, `0xD3098461`, `0xB5D2FE96`. See "Open items".

* **`size_dup`** — the parser asserts `size == size_dup` on every record (holds in 2612/2612
  files under `Bin360/`). We suspect this is where retail's `compSize`/`uncSize` pair collapsed
  when the format dropped compression: SBL4 slots are stored raw, so both words carry the same
  value. If the two ever diverge in a build we don't have, treat it as a signal that this slot
  IS compressed and re-open the reader.

* **`block_hash`** — usually the same value for every record in a file (2457/2612). The 155
  files where records disagree are mostly animation banks (`AnimationTest/*.Pack`) and giant
  streamblocks in `Common.Pack` — see the `has_consistent_block_hash` flag on `Sbl4SubPack`
  before treating `block_hash` as an identity.

### Layout invariants (verified over every SBL4 sub-pack in `Bin360/`)

Over the 2612 SBL4 files, ALL of these held:

* Data offsets are strictly increasing: `offset[i] > offset[i-1]`.
* `size == size_dup` on every record.
* The data region covers exactly `[dir_end, file_end)` — no gaps, no overlap, no trailing footer.
* Directory is a flat array — no placeholder / section-boundary records.

The parser refuses any file that violates these, which is what surfaces the difference vs the
other SBL* variants (`SBLL`, `SBL8`, `SBL6`, `SBLX`, `SBL0`, `SBL5`, `MP00`, `AP04` — see below).

## How this differs from retail's `ALBS`/`SBLA`

| dimension            | retail `ALBS`                              | 2008 `SBL4`                              |
| -------------------- | ------------------------------------------ | ---------------------------------------- |
| endianness           | little                                     | **big**                                  |
| header length        | 0x20 (object) or 0x44 (streamblock)        | **0x08 (magic + nEntries)**              |
| header fields        | name_crc, aux0/aux1, flags, reserved words | none beyond nEntries                     |
| record stride        | 24 bytes                                   | 24 bytes (same)                          |
| record fields        | hash · offset · comp · unc · f4 · f5       | kind_hash · offset · size · size_dup · flag · block_hash |
| offset semantics     | **`dir_end`-relative** (retail bug, 2026-07) | **absolute file offset**                 |
| compression          | zlib per record                            | **none** at this level                   |
| block hash placement | in the header (`name_crc` field)           | **per record** (redundant but harmless)  |
| placeholder records  | `hash==0` marks section boundaries         | none — flat directory                    |
| self-delimited?      | yes (walks until running cursor breaks)    | no — `nEntries` in header                |

The absolute-offset semantics is the concrete simplification worth naming: retail's `sab_sbla
replace` bug (blobs written `dir_end` bytes too early) is impossible in SBL4 because the offset
IS the file offset.

## Not the same as the `.Packlog`

The `.Packlog` sitting next to each `.Pack` is a build-time manifest with the SAME four types
(`textures`, `materials`, `meshes`, `physics`) as `Start:` / `Size:` fields, but **those numbers
are NOT the on-disk layout**. Concrete example — `Global/Act1_IntKey.Pack`:

| slot      | packlog `Start:` | packlog `Size:` | SBL4 offset | SBL4 size |
| --------- | ---------------- | --------------- | ----------- | --------- |
| materials | 987255           | 299             | 104         | 310       |
| meshes    | 987554           | 12572           | 414         | 11266     |
| physics   | 1000126          | 330             | 11680       | 370       |
| textures  | 104              | 987151          | 12050       | 987151    |

Sizes are close but different (probably because the packer adds per-slot wrappers), and the
ORDER on disk (materials → meshes → physics → textures) is not the packlog's declared order
(textures first). Trusting the packlog to seek inside a `.Pack` reads bytes from the wrong slot,
usually landing partway through the textures blob and returning nonsense — which is exactly the
bug the initial `Prototype2008Source::read_mesh_bytes` had.

The packlog is still authoritative about **names**: `.Pack` files have no plaintext strings, so
the packlog's leaf name + extension are how we recover a friendly type per slot. The `Start:` /
`Size:` fields are best treated as build-tool metadata, not as loader inputs.

## Corpus stats

* 4396 `.Pack` files total under `Bin360/`.
* 2612 open cleanly as SBL4 (this parser). Regions: `Global/` 1361, `France/` 1217,
  `AnimationTest/` 34.
* Record-count histogram: 4 recs=1096, 7=760, 1=479, 5=109, 8=47, 9=39, 6=25, 11=17,
  10=10, 14=6, 2=6, 12=5, 3=4, 13=4, 15=3, 16=1, 17=1.
* Slot hits across recognized names: `textures` 2590, `meshes` 1891, `materials` 1891,
  `physics` 1773. Everything else is currently in the "unknown hash1" bucket.
* Non-SBL4 magics in the remaining 1784 files: `SBLL` (1265), `SBL8` (234), `SBL6` (157),
  `SBLX` (107), `SBL0` (10), `SBL5` (4), `MP00` (4), `AP04` (1). Not decoded here.

## What the meshes slot itself contains (bonus, out of this probe's scope)

Peeking inside so the next probe has a runway: a `meshes` slot is a mini-directory of one or
more MSH4 records, all big-endian, with zlib bodies —

```
  +0x00  u32   n_meshes
  +0x04  n × 24-byte records (hash, offset, comp, uncomp, flag=1, aux=0), all BE
              — offset is relative to the slot start
              — comp = zlib-compressed size, uncomp = inflated size
  ...    n zlib streams starting at each record's offset
```

Inflating one stream yields a `MSH4`-prefixed record: `'M''S''H''4'  <u32 BE name_len> 
<ASCII name>  ...`. Real names include `P_Keys_KeyRing_A` (the same asset that ships in retail's
`Dynamic0` #0), `saboteur` (~593 kB — likely the main-character body), `ragdoll_b`, and dozens
of `OccMed_Debris_*` / `PGA_DynDebris_*` variants. The name hash matches
`pandemic_hash(<asset name>)` — same kernel as retail. Fully decoding a MSH4 body into
vertices/bones is the next probe, not this one.

Materials slots start with the ASCII magic `WSAO` (same as retail's `France.materials`), so
material bindings likely reuse retail's WSAO shape verbatim once someone opens one.

## Open items

* Four unknown slot-type hashes: `0x9AB5A351`, `0xE1087B27`, `0xD3098461`, `0xB5D2FE96`. Not
  in the brute-force candidate list of ~140 English words we tried. Payloads suggest they're
  streamblock/anim data (`AnimationTest_Streamblock_0_baked_18`, nested 12-record directories).
  Worth mining string literals out of `WildStar_d.pdb` (already at 99.8% named — see
  `docs/2008_binaries_decomp.md`) or `.map` linker files under `game-files/Bin360/symbols/`
  for candidate slot-type strings.
* The other SBL* magics (`SBLL`, `SBL8`, `SBL6`, `SBLX`, `SBL0`, `SBL5`, `MP00`, `AP04`) are
  separate containers. Each will need its own reader. `SBLL` alone is 1265 files (nearly half
  the remainder), so it's the highest-leverage next target.
* MSH4 inner geometry parser (mirroring `meshload.rs`'s MSHA path) — required before the
  workshop can actually render a prototype character.

## Tooling

`Sbl4SubPack::parse` is the whole surface: byte slice in, records out, one helper to slice a
slot's bytes back out of the buffer. No CLI, no round-trip splicing (there's no compression
here so a rebuild would trivially be the input); if we ever need editing, the parser hands you
everything you need to lay one down by concatenation.
