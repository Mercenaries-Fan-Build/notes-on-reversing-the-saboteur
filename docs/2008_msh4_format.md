# MSH4 — the 2008 Xbox 360 mesh format

The mesh body found inside every `<hash>.meshes` slot of a `Bin360/` `.Pack`. Inflated
from the slot's inner zlib stream, prefixed by the ASCII magic `MSH4`. All numbers here
are byte-verified against `BP_AL_Sean_HD.Pack` (164159 B), `BP_AL_Sean_LB.Pack`
(110005 B), `BP_AL_Sean_GR.Pack` (141243 B), `BP_AL_Sean_UB.Pack` (186631 B).

> **Status**: framing proven and geometry decoded for **character parts** (Sean-shape:
> skinned meshes whose 22-char name plus NUL pads the payload to a shifted boundary). All
> four Sean parts round-trip through `parse_msh4` into non-empty `Smsh` with correct
> positions, indices in-range, and per-prim material hashes. Non-Sean meshes (weapons,
> props, other characters) use a second sub-format not yet decoded — see "Known format
> variants" below.

---

## Container path

```text
game-files/Bin360/<Region>/<Name>.Pack
  ├── SBL4 header (see docs/2008_subpack_format.md, and prototype2008_subpack.rs)
  ├── directory record: kind_hash == pandemic_hash("meshes")
  └── that record's slot bytes:
       ├── BE u32  n_records            (typically 1 per pack)
       ├── n × 24  { u32 hash, u32 off, u32 comp_size, u32 unc_size, u32 flag, u32 unk }
       └── n zlib streams (each `78 01…`) — each inflates to ONE MSH4 body
```

## Framing (proven)

```text
+0x00  char   magic[4]          = "MSH4"                                 // BE ASCII
+0x04  u32be  name_len          includes trailing \0
+0x08  char[] name[name_len]    ASCII, e.g. "CH_AL_SeanDevlin_01_HD\0"
       char[] pad_to_align_4    0-3 zero pad bytes so payload lands on a 4-byte boundary
                                (name_len=23 → 1 byte pad → payload at 0x20)

+PAYLOAD_START  … PAYLOAD (file header + N streams + skeleton) …

FILE_END-8  char   magic[4]     = "MSHC"                                 // BE ASCII
FILE_END-4  u32be  content_hash preserved verbatim for round-trip
```

## Payload encoding — the "BE u24 + pad byte" trick (Sean-shape meshes)

For meshes where the name pads the payload to a 4-byte boundary (Sean's four character
parts, and other 22-char names), every 4-byte integer in the file/stream/prim header is
written as a **big-endian 24-bit value in the first three bytes + a pad byte in the
fourth**. Reading it naively as `u32 BE` gives you `value << 8`; the actual value is
`(u32_be >> 8) & 0xFFFFFF`. `parse_msh4::rd24` reads exactly this.

## Payload layout — decoded

```text
FILE HEADER (5 fields = 20 B)
  [0] u24 num_streams
  [1] u24 (0)
  [2] u24 (0)
  [3] u24 (0)
  [4] u24 version (=1)

For each of num_streams streams (in file order):
  STREAM HEADER (6 fields = 24 B)
    [0] u24 format_tag        // retail-shape bitfield, see decode_format
    [1] u24 num_indices
    [2] u24 num_vertices
    [3] u24 format_extension  // seen as 0
    [4] u24 stride            // 24, 28, or 32 on the sampled bodies
    [5] u24 num_prims

  VB: num_vertices × stride bytes                (little-endian vertex data)
  IB: num_indices × 2 bytes                       (u16 LE triangle-list indices)

  PRIMS: num_prims × 28 B, each 7 fields:
    [0] u24 (0)                // reserved / local index base
    [1] u24 num_tris           // == num_indices_this_prim / 3
    [2] u24 material_hash      // 24-bit truncated pandemic_hash for the material
    [3] u24 (0)
    [4] u24 (0)
    [5] u24 index_start        // offset into THIS stream's IB where this prim's tris start
    [6] u24 num_indices        // == num_tris * 3

(After all streams: skeleton / bone-remaps / shadows / occluders — see WildStar_d
`ReadMesh @ 0x82658168`. Not decoded yet; not needed to render.)
```

## Vertex-attribute layout

The `format_tag` uses the SAME bitfield the retail engine's `Stream.format` uses (see
`meshload::decode_format`), only with different attribute *sizes* for the packed Xbox 360
formats — retail's `Normal` is float3 (12 B), MSH4's is DEC3N (4 B).

```text
bits 0..1: positionType   (=2 → half3 + 2-byte pad = 8 bytes)
bits 2..3: skinType       (=1 → adds u8x4 weights + u8x4 bone indices)
bits 4..7: numColors      (=0 on every sampled Sean part)
bits 8..11: numUVs        (1 or 2)
bit  12   : has_normal    (1 → adds a 4-byte DEC3N-packed normal)
bit  13   : has_tangent   (0 or 1 → adds a 4-byte DEC3N-packed tangent)
```

Observed formats and strides on Sean's four parts:

| fmt      | attrs                                                  | stride |
| -------- | ------------------------------------------------------ | -----: |
| `0x1106` | pos + skin + 1 UV + normal                             | 24     |
| `0x3106` | pos + skin + 1 UV + normal + tangent                   | 28     |
| `0x3206` | pos + skin + 2 UVs + normal + tangent                  | 32     |

All confirmed against real bytes: max index < num_vertices per prim, prim material hashes
decode to plausible 24-bit values, and vertex 0 of every stream decodes to a plausible
model-space position (face vertices at (~1.7, ~0.07, ~1.07) on Sean HD, torso vertices at
(~0.1, ~0.01, ~1.16) on Sean LB, back-body at (~0.95, ~-0.01, ~1.12) on Sean UB).

## Framing sanity check — Sean's four parts

| File            | size    | name (22 chars + NUL)    | payload@ | MSHC hash    | streams | verts | tris  |
| --------------- | ------- | ------------------------ | -------- | ------------ | ------: | ----: | ----: |
| `BP_AL_Sean_HD` | 164 159 | `CH_AL_SeanDevlin_01_HD` | `0x20`   | `0x8A70949E` |       4 |  3697 |  6462 |
| `BP_AL_Sean_LB` | 110 005 | `CH_AL_SeanDevlin_01_LB` | `0x20`   | `0x0B7607C7` |       2 |  2554 |  3583 |
| `BP_AL_Sean_GR` | 141 243 | `CH_AL_SeanDevlin_01_GR` | `0x20`   | (varies)     |       1 |  3883 |  3838 |
| `BP_AL_Sean_UB` | 186 631 | `CH_AL_SeanDevlin_01_UB` | `0x20`   | (varies)     |       1 |  4775 |  6934 |

All bounding boxes land within model space (`X`, `Z` ∈ [+0, +2], `Y` ∈ [−0.31, +0.18])
— matches what a human character shipped 1 unit off the origin should look like.

## Known format variants — open work

The encoding above (Sean-shape) is the ONE the parser decodes today. Meshes with different
name lengths (weapons, props, other characters) store the fields at a NON-shifted offset,
i.e. the counts are plain `u32 BE` at the exact post-name byte, without the `<<8` shift.
A non-Sean mesh comes back from `parse_msh4` as `MSH4: implausible num_streams N` where
`N` is the true count times 256 (or 65536, or 0, depending on the exact byte alignment of
`name_len % 4`). Coverage on a 40-mesh survey across the prototype: 10/40 decode cleanly
today; the other 30 use the alternative encoding.

The fix is straightforward: try both readers (aligned+rd24 first, unaligned+plain-u32
second) and pick the one whose `num_streams` falls in `1..=32`. Filed as follow-up because
it needs a control mesh from a non-Sean pack to confirm the alternative encoding's exact
byte layout for VB/IB/prims (positions match at a 1-byte offset within the vertex; the
IB endian and prim size need one more probe). The renderer already handles Sean's parts
today — a modder pointing the workshop at a Sean pack sees full geometry.

## Runtime-side references

`game-files/Bin360/WildStar_d.map` names the 2008 loader:

* `?ReadMesh@@YAXAAVPblFile@@_NAAV?$PblHandle@VOdinMesh@@@@PAVWSStreamBlock@@@Z`
  @ `0x82658168` in `WSLoaderXenon.obj` — the sibling WildStar_d engine's reader,
  99.8 % named. Ground truth for the file-header shape (5 fields), per-stream layout
  (6 fields), and per-prim layout (7 `Read32u` per prim → 28 B).
  Sibling: `output/_ghidra_wildstar/wildstar_d_all_functions_decomp.txt`, line **523552**.
* Same code family in Saboteur's `Saboteur.xex`, decomp at
  `output/_ghidra_saboteur_xex/saboteur_xex_all_functions_decomp.txt` line **427900**
  (Ghidra registers are aliased there, but the sequence of `Read32u` calls confirms the
  identical 5 + 6 + 7 field count).
* Runtime classes: `XenonMesh`, `XenonPrimitive`, `XenonSharedBuffers` (all in
  `docs/2008_binaries_decomp.md`).

## Gotchas

* **Endianness split** — the outer SBL4 container and the MSH4 header (magic, name_len,
  footer) are **big-endian**. The inner vertex and index buffers are **little-endian**
  (Xbox 360 D3D9 vertex/index buffers stay in the format the GPU consumes).
* **Payload alignment** — `name_len` includes the trailing `\0`; the payload starts on
  the next 4-byte boundary, and the SEAN-SHAPE meshes' fields shift their real value into
  the top 3 bytes of each 4-byte slot as a consequence.
* **`MSHC` is not `MSHA`** — the retail wrapper name is nothing like this footer. Do not
  cross-wire the two decoders; they share a family but not a byte layout.
* **Sean's material hashes are 24-bit** — every `material_hash` decoded from MSH4 prims is
  a truncation of `pandemic_hash(material_name)`. The workshop's material resolver uses
  the same 24-bit truncation to match against the material index.
