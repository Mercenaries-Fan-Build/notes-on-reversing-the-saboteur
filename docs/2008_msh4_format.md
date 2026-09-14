# MSH4 — the 2008 Xbox 360 mesh format

The mesh body found inside every `<hash>.meshes` slot of a `Bin360/` `.Pack`. Inflated
from the slot's inner zlib stream, prefixed by the ASCII magic `MSH4`. All numbers here
are byte-verified against `BP_AL_Sean_HD.Pack` (164159 B) and `BP_AL_Sean_LB.Pack`
(110005 B) unless flagged otherwise.

> **Status**: framing is proven and round-trips through the parser
> (`tools/sab_workshop/src/sources/prototype2008_mesh.rs`). The vertex/index streams
> **decode is not landed yet** — I can prove they're in the payload region, catalogue
> their shape, and tell you exactly where the stream-table lives, but the exact byte
> layout of the stream descriptors is one probe short of shipping. What the parser
> returns today is a valid `Msh4Body` with empty geometry vectors and a `diagnostics`
> field describing what's present.

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

Every 2008 character-part pack shipped so far has `n_records == 1`; we haven't seen a
`.meshes` slot with multiple MSH4 records. The parser only reads the first record for
that reason (`inflate_first_msh4_stream`).

## MSH4 body layout — proven

```text
+0x00  char   magic[4]         = "MSH4"                                 // BE ASCII
+0x04  u32be  name_len         includes trailing \0; padded up to 16-B
+0x08  char[] name[name_len]   ASCII, e.g. "CH_AL_SeanDevlin_01_HD\0"
       char[] pad_to_align_16  zero pad — payload starts at the next 16-B boundary

+0x20  … PAYLOAD …             streams + skeleton + VB/IB + prims + drawcalls
                                (interior layout: see "Payload" below — open questions)

FILE_END-8  char   magic[4]    = "MSHC"                                 // BE ASCII
FILE_END-4  u32be  content_hash  a hash of some payload subset; not yet reproduced
```

### Framing check on the two Sean parts

| File            | size    | name              | name_len | payload range         | MSHC hash    |
| --------------- | ------- | ----------------- | -------- | --------------------- | ------------ |
| `BP_AL_Sean_HD` | 164 159 | `..._01_HD`       | 23       | `[0x20 .. 0x2812F)`   | `0x8A70949E` |
| `BP_AL_Sean_LB` | 110 005 | `..._01_LB`       | 23       | `[0x20 .. 0x1ADAD)`   | `0x0B7607C7` |

Both align perfectly: `name_end == 8 + 23 == 31 → payload_start = ceil(31/16)*16 = 32`
(`0x20`); footer at `file_end - 8`. The parser test `parses_real_sean_hd_mesh` pins
these bytes-for-bytes.

## Payload — what's known and what's missing

The payload is the interior between `+0x20` and the `MSHC` footer. It carries:

* **A multi-stream vertex/index block** (in-place — no companion `.dat`, unlike retail's
  `MSHA` which zlib-splits VB/IB into blob1). Sean HD's payload holds several distinct
  streams; the workshop parser's diagnostic scan reports:
  * **Vertex stride histogram** from repeated `FF XX XX XX XX` skin markers:
    `{24: 314, 28: 141, 32: 7}` on Sean HD. Three vertex formats coexist in one mesh.
  * **Index buffers** as long runs of `u16 LE < 4096`. Sean HD's biggest such region is
    at payload-relative `0x1BD9E` for **16 977 u16 words = 33 954 bytes** — one large
    stream. Two smaller regions match the pattern for smaller streams (~ 800 words each).
* **Skeleton matrices** — the tail of the payload, right before MSHC, is a dense block
  of `f32 BE` values with `3f 80 00 00` (= 1.0f) and `bf 80 00 00` (= -1.0f) recurring on
  a 64-byte cadence — the shape of `Bone::inv_bind` / `localTMS` retail uses.

### Vertex format (partial reconstruction)

At stride 24, the record layout appears to be:

| offset | bytes | field                     | notes                                     |
| -----: | ----: | ------------------------- | ----------------------------------------- |
|   0..5 |     6 | Position xyz (half3 LE)   | LE inside a BE container — Xbox D3D9      |
|   6..7 |     2 | padding / unused          | always `00 00` on Sean HD                 |
|   8..9 |     2 | ? (skinType extra field?) | always `00 00`                            |
|     10 |     1 | Bone weight (U8 UNORM)    | typically `FF` = single-bone (weight 1.0) |
|  11..14|     4 | Bone indices (U8×4)       | four palette indices                      |
|  15..19|     5 | tangent + tail            | still investigating                       |
|  20..23|     4 | Normal (DEC3N?)           | 10:10:10:2 packed                         |

**Warning**: this is the leading candidate, not proven. Positions decode to plausible
model-space values (Sean's face vertices come out around `(1.5, 0.07, 1.07)`), but the
UV / normal / tangent split within bytes 16..23 needs a control vertex to lock in — the
`0x1B003106`-style format tag that retail uses to specify this exact layout hasn't been
located in the MSH4 header yet.

### Retail comparison

MSH4 is a sibling to retail's `MSHA` (see `tools/sab_workshop/src/meshload.rs` +
`docs/formats/mesh_geometry.md`):

| aspect             | retail `MSHA`                                     | 2008 `MSH4`                                  |
| ------------------ | ------------------------------------------------- | -------------------------------------------- |
| endianness         | LE header, LE geometry                            | **BE** header, **LE** vertex/index buffers   |
| header size        | fixed 276 B (name is `char[0x100]`)               | variable — 4B magic + BE `u32 name_len` + name |
| VB/IB storage      | separate zlib blob (`msha + 276 + comp0`)         | **inline** in the payload                    |
| footer             | none                                              | 8-byte `MSHC + u32` marker                   |
| vertex format tag  | `Stream.format` u32, MSB byte `0x1B`              | analogous tag exists but its offset in the payload is not yet fixed |
| stream count       | `u16 @216`                                        | multiple streams evident from stride histogram; count field TBD |
| index type         | u16 LE (`faceType == 1`)                          | u16 LE (matches — same D3D9 convention)      |

## Runtime-side clues (the actual loader)

`game-files/Bin360/WildStar_d.map` names the 2008 loader:

* `?ReadMesh@@YAXAAVPblFile@@_NAAV?$PblHandle@VOdinMesh@@@@PAVWSStreamBlock@@@Z`
  at `0x82658168` in `WSLoaderXenon.obj` — the reader body itself.
* `XenonMesh`, `XenonPrimitive`, `XenonSharedBuffers` — the runtime shape.
* `?CheckDecompressMeshBail@@YA_NAAVDecompressMeshData@@AAVReadMeshData@@@Z` @ `0x82665198`
  — the streaming/decompression gate.

Disassembling `ReadMesh` (Ghidra + `Saboteur.pdb` + the `.map` addresses) will name the
struct fields that describe each stream — the missing piece to promote MSH4 from
"framing decoded" to "geometry decoded".

## What the parser exposes today

```rust
pub fn parse_msh4(bytes: &[u8]) -> Result<Msh4Body, String>;

pub struct Msh4Body {
    pub name: String,             // proven
    pub content_hash: u32,        // proven — from MSHC footer
    pub payload_range: Range<usize>, // proven — window into the still-encoded region

    pub positions: Vec<[f32; 3]>, // empty for now
    pub normals: Vec<[f32; 3]>,   // empty for now
    pub uvs: Vec<[f32; 2]>,       // empty for now
    pub joints: Vec<[u16; 4]>,    // empty for now
    pub weights: Vec<[f32; 4]>,   // empty for now
    pub indices: Vec<u32>,        // empty for now
    pub prims: Vec<(u32, u32, u32)>, // empty for now

    pub diagnostics: Msh4Diagnostics, // proof-of-life scan of the payload
}
```

`Prototype2008Source::read_mesh_smsh(&handle)` is the on-ramp for the wgpu renderer —
returns a valid but empty `Smsh` today. Once the vertex/index/prim decode lands,
lighting up geometry is a one-line change inside `parse_msh4`.

## Gotchas

* **Endianness split** — the outer SBL4 container and the MSH4 header (magic, name_len,
  footer) are **big-endian**. The inner vertex and index buffers are **little-endian**
  (Xbox 360 D3D9 vertex/index buffers stay in the format the GPU consumes). Missing this
  is the fastest way to read plausible-looking but wrong numbers.
* **Position padding** — `name_len` includes the trailing `\0`; the payload starts on the
  next 16-byte boundary, not immediately after the name. The parser rounds up
  automatically; anyone hand-writing an offset needs to remember the pad.
* **Slot-directory offsets are BE and count from slot start**, not from the file. Fed
  the wrong endian these silently point into neighbouring records — the same class of
  bug that took a probe to unstick on the outer SBL container.
* **`MSHC` is not `MSHA`** — the retail wrapper name is nothing like this footer. Do not
  cross-wire the two decoders; they share a family but not a byte layout.
