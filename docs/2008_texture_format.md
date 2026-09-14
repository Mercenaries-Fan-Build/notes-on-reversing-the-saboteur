# 2008 Xbox 360 prototype — texture format

Status: **loose `.dds` files fully wired; `<hash>.textures` DXT1 + DXT5 slots wired via
Xbox 360 un-tile + BC1/BC3 decode; two uncompressed variants still return shaped errors.**

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

**Solved end-to-end for the two DXT variants (about 53% of records by count).** Both the
outer SBL-family sub-pack (see `docs/2008_subpack_variants.md`) and the inner per-texture
directory (in `prototype2008.rs::parse_texslot`) were solved earlier. This section documents
the innermost blob: the per-texture PAYLOAD that lives at each directory entry's `data_off /
data_size` window.

### Per-texture payload layout (BE throughout)

```text
+0        u32   name_len
+4        char  name[name_len]
+4+nl     u8[4] format_tag       // game-internal enum — see the table below
+8+nl     u32   flags            // small ints; meaning unresolved (mostly < 256)
+12+nl    u16   width
+14+nl    u16   height
+16+nl    u16   mips             // 1..log2(max(W,H))+1
+18+nl    u32   unc              // total uncompressed size of ALL streams packed
+22+nl    u32   ns               // number of zlib streams (always 1 in the current corpus)
per stream:
           u32   cs              // compressed size of THIS stream
           u8[cs] zlib_data
+N        u8[8] "MSHC…"          // 8-byte trailer; likely a hash, ignored by the reader
```

This is retail DTEX's shape, byte-swapped end-to-end for the 360. Retail's stream splitting
(0x180000-byte boundaries) isn't observed here — every real record is a single stream — but
`ns >= 1` is handled either way for future-proofing. The 8-byte `MSHC…` trailer past the
last stream is consistent across every record; it's assumed to be a fingerprint and is
never consumed by the decoder.

### Format tag table (proven)

The 4-byte `format_tag` was catalogued across every SBL-family `.Pack` in the corpus, then
each tag's `unc / (W·H)` ratio was matched to a specific Xbox 360 texture format by
counting tiles per mip (see next section). Result:

| tag              | format                | block bytes | corpus count | wired |
|------------------|-----------------------|-------------|--------------|-------|
| `1A 20 01 52`    | DXT1 (BC1)            | 8           | 14 037       | ✅ |
| `1A 20 01 54`    | DXT5 (BC3)            | 16          | 4 388        | ✅ |
| `1A 20 01 71`    | DXT5 (BC3), swizzled  | 16          | 6 563        | ✅ (bytes)  |
| `28 28 01 44`    | 16bpp uncompressed    | -           | 21 978       | ⛔ shaped error |
| `28 28 01 86`    | A8R8G8B8 uncompressed | -           | 347          | ⛔ shaped error |

The uncompressed variants would need a linear un-tile function (different macrotile
geometry than BC) plus a channel-order guess for the 16bpp tag (A8L8 vs R5G6B5 not yet
disambiguated). Left as a follow-up because DXT1 + DXT5 already cover every named material
slot we've traced.

**About `1A 20 01 71`.** The bytes decode as plain BC3, and the byte-level output is right
— but visually the RGBA looks pink/magenta with washed-out channels. That's because this
variant is stored as a **"DXT5 swizzled" normal map**: the X component sits in the alpha
channel, Y sits in green, and R/B carry padding or a third channel. The engine's shader
unpacks it at sample time; the raw block bytes we emit are the correct D3D9 BC3 payload.
The workshop shows it as-is for now — a future revision can add a per-tag "post-decode
swizzle" step to render the same texture as a viewable normal map.

### Xbox 360 tiling — how the mip pyramid is stored

The inflated buffer holds ALL mip levels laid out in Xbox 360 storage order. Every mip
whose `min(W,H) >= 32` gets its own base-tile allocation; smaller mips share one
packed-tail tile. The base tile is 32×32 blocks — 128×128 pixels for a BC format — and its
byte size is `32·32·block_bytes` (8192 for DXT1, 16384 for DXT5).

Worked example — a 512×512 DXT1 texture with 10 mip levels:

| mip | pixels    | blocks | tiles                          |
|-----|-----------|--------|--------------------------------|
| 0   | 512×512   | 128×128 | 4×4 base tiles = 16 tiles     |
| 1   | 256×256   | 64×64   | 2×2 base tiles = 4 tiles      |
| 2   | 128×128   | 32×32   | 1 base tile                    |
| 3   | 64×64     | 16×16   | 1 base tile (padded)           |
| 4   | 32×32     | 8×8     | 1 base tile (padded)           |
| 5-9 | 16×16..1×1| ..      | 1 packed-tail tile             |
|     |           |         | **total = 24 tiles = 196 608 B** ✓ |

This is what pins each format's identity: the `unc` field IS the sum of the per-mip tile
counts times the tile size. A texture tagged `1A 20 01 52` with the tile count that matches
BC1's 8-byte tile IS a BC1 texture; a `1A 20 01 54` matches the 16-byte BC3 tile every
time. That's the empirical anchor — no engine symbol was needed.

The packed-tail tile's back half is uninitialised (real records end in `BA AD F0 0D`
repeated) — do not consume those bytes as pixels.

### Un-tile + byte-swap

Within a base tile the block ordering follows the standard `XGAddress2DTiledOffset` XG
formula (Xbox SDK public reference; independently re-implemented in Wine/Xenia/XeXTool).
`prototype2008_texture.rs::xg_address_2d_tiled_offset` is a straight port; do not tweak
individual bit twiddles.

Every 16-bit word inside each BC block is byte-swapped on-disk (the GPU is big-endian).
BC1 blocks are 4 words; BC3 blocks are 8 words. The decoder un-swaps after un-tiling and
hands the resulting little-endian D3D9 block stream to the shared `dtex::decode_surface`
BC decoder that already services retail's DTEX and the loose Bin360 DDS files.

### Wiring today

`Prototype2008Source::load_texture(hash)`:

- Path A (loose `.dds`): unchanged — straight through `dds::decode_bytes` (615 files).
- Path B (slot texture): opens the containing `.Pack`, slices the exact
  `slot_off + data_off .. + data_size` window of the sub-record, and passes it to
  `prototype2008_texture::decode_slot_texture`. DXT1 / DXT5 (all three tags) round-trip to
  RGBA8; the two uncompressed variants surface a shaped error naming the tag.
- Cached in `tex_cache` so repeat lookups (preview + material-slot) don't re-open the file.

**Historical container context.** The 4 396 `.Pack` files across Bin360/ are wrapped:

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
