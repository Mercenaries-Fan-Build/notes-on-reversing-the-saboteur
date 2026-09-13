# 2008 Xbox 360 prototype — texture format

Status: **loose `.dds` files solved & wired; `<hash>.textures` slots blocked on the SBL4 sub-pack
probe.**

## Loose `.dds` files (Bin360 root + Branches/*/)

**615 files total**, all with a valid `DDS ` (0x20534444) magic and a standard 124-byte D3D9
`DDS_HEADER`. Nothing about them is Xbox-360-specific on disk — they read as ordinary PC DDS.

### Formats seen

| fourCC | count | how we decode |
|--------|-------|---------------|
| DXT5   | 505   | BC3 block decoder (`dtex::decode_surface`) |
| DXT3   | 66    | BC2 block decoder |
| DXT1   | 26    | BC1 block decoder |
| DXT4   | 18    | BC3 block decoder (block layout is byte-identical to DXT5; only alpha semantic differs, which straight-alpha RGBA output collapses) |

Dimensions range from 16×16 to 512×512. Nine of the 615 are cubemaps (DDS_CAPS2_CUBEMAP bit
set) — the diffuse-probe library. The rest are 2D.

### Layout: **linear, little-endian DXT blocks. NOT tiled.**

The direct empirical test: run the standard PC BC1 decoder over `Bin360/CloudProj.dds` (512×512
DXT1, no mips) and eyeball the output.

- **LE decode (`dtex::decode_bc1_block` as-is)** → coherent cloud texture, bright base with
  darker cloud shapes. Mean R = 172.
- **BE-swap decode (byte-swap every 16-bit word, then BC1)** → scrambled purple/green mosaic
  with dropped alpha bits. Mean R = 165.
- **Full-file magic scan** → no `DXT1/3/5` or `DTEX` 4CCs anywhere in the file, no `DDS `
  duplicates — just the header, then 131 072 bytes of contiguous block data.

The DXT5 icon `HUD_all_detonator_icon.dds` gave the same verdict (a legible green detonator
grenade at 64×64 vs a scrambled quarter of one under BE-swap). This reader was proved out
against both a large DXT1 (CloudProj) and a small DXT5 (HUD icon) before wiring; the same
Python probe lives in the scratchpad for anyone who wants to reproduce.

So the on-disk `.dds` are almost certainly the authoring cache — the artist's PC-tool output —
that the engine tiles at load time when it pushes them to the Xbox's texture memory. We're
reading them at the same layer any PC DDS reader would, and it works.

**One inference we can NOT make from these files:** what the fully-tiled, GPU-resident layout
looks like. If some future asset stream turns out to be a raw dump of Xbox texture memory
(rather than a copy of the authoring source), that's a separate reader — 128×128 macro-tile
un-swizzle plus 16-bit word swap for BC1/3.

## `<hash>.textures` slots inside `.Pack` files

**Blocked — waiting on the SBL4 sub-pack probe.**

The 4 396 `.Pack` files across Bin360/ are wrapped:

| magic | count | family |
|-------|-------|--------|
| SBL4  | 2 612 | main sub-pack format |
| SBLL  | 1 265 | sibling |
| SBL8  | 234   | sibling |
| SBL6  | 157   | sibling |
| SBLX  | 107   | sibling |
| SBL0  | 10    | sibling |
| SBL5  | 4     | sibling |
| MP00  | 4     | mega-pack? |
| AP04  | 1     | ? |
| (empty) | 2   | truncated |

Every `.Pack` file starts with one of these 4-byte magics at offset 0. The `.Packlog` manifest
alongside it lists 4 logical "slots" (`.textures / .materials / .meshes / .physics`) with
byte-precise start/size into the file — but the payload past the wrapper header is opaque:

- Direct scan of `Global/Act1_IntKey.Pack` (999 201 B, SBL4) for `DDS `, `DXT1`, `DXT3`,
  `DXT5`, `DTEX`, `WSMA`, `MSHA` → **zero hits.**
- Only cleartext markers found: `WSAO` + `MTLD` at file offset 104, exactly at the
  packlog-declared "textures" slot start. That's the tail of the SBL4 header meeting the start
  of the material record, before whatever encoding/compression starts.
- Packlog slot boundaries after the first few hundred bytes land on random-looking bytes with
  no record structure visible.

That's consistent with an SBL4 wrapper that batches every record type (WSAO / MTLD / DTEX-ish /
MSHA / physics) behind a shared (probably LZ or bit-packed) stream, and the packlog's stated
per-slot offsets get reinterpreted at load time rather than pointing at plaintext record
boundaries.

**Concrete implication for the texture decoder:** even a trivial "raw XGF-tiled BC1" reader
can't reach the pixel bytes until the SBL4 wrapper is peeled off. There is no plaintext
`DXT1/3/5` 4CC and no `DDS ` header in any `<hash>.textures` slot of any `.Pack` I sampled. The
container is the whole barrier.

`Prototype2008Source::load_texture(hash)` reports this explicitly for any hash that appears in
the packlog tex-index:

    prototype 2008: texture <hash> lives inside an SBL4-wrapped .Pack slot; waiting on the
    SBL4 sub-pack probe (probe #1) — no plaintext DDS/DTEX bytes present in the slot

## Materials — out of scope for this probe

The 2008 build has no loose `France.materials`; per-block material records live inside the
same SBL4-wrapped `.Pack` files (probe #3's territory). What we can see today: the very first
bytes past the SBL4 header (file offset 104 of `Act1_IntKey.Pack`) are literal `WSAO ... MTLD
... DefaultLib.Material:0` — the WSAO record shape is at least partly identical to retail's.
The rest of the record and any per-material texture-hash slots are behind the wrapper.

## Wiring today

`Prototype2008Source`:

- **`open()`** walks `Bin360/**` for `*.dds`, keys each by `pandemic_hash(filename_stem)`,
  keeps the shallowest twin (Bin360 root wins over `Branches/*/`), and folds every hash→name
  entry into the global inventory (so a texture whose only appearance is a loose file still
  shows a plaintext name in the browser).
- **`list_textures()`** returns packlog-mined slot rows + one row per loose DDS. Loose rows
  use `key = u64::MAX - loose_idx` so consumers can tell the two decode paths apart from the
  handle alone.
- **`load_texture(hash)`** resolves loose DDS via `crate::dds::decode_bytes` (mip 0, face 0) →
  straight-alpha RGBA8; caches the result. For packlog-mined slots, returns the SBL4-blocked
  error. For unknown hashes, returns a distinct "no loose .dds and no packlog-mined slot"
  error so calling code can distinguish "we know but can't decode yet" from "we don't know
  this hash at all".

Two new tests in `sources/prototype2008.rs`:

- `loose_dds_decodes_to_rgba8` — decodes CloudProj (512×512 DXT1) end-to-end and asserts the
  mean-R-channel probe that would fail on tiled/swapped decodes.
- `load_texture_on_real_hash_either_decodes_or_stubs_explicitly` — walks `hash_names`,
  requires that at least one hash decodes (loose DDS) or hits the SBL4 stub (both are pass
  states; the guard is against silent success on undecodable hashes and against panics).
