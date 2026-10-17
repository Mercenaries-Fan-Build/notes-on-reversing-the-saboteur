# 2008 prototype meshes (`MSH4` and its revisions)

A `meshes` slot record ([`2008_subpack_format.md`](2008_subpack_format.md)) is a zlib stream that
inflates to one mesh body, or (in `SBLX` packs) a raw variant record wrapping one. Reader:
[`sab_formats::proto2008::mesh`](../tools/sab_formats/src/proto2008/mesh.rs). Census:
`sab_probe proto2008-meshes <Bin360>`.

Engine reader: `ReadMesh(PblFile&, bool, PblHandle<OdinMesh>&, WSStreamBlock*)` —
`WildStar_p.exe` @0x8236def8 (the revision that loads this data; its decompile folds every value read
through `PblFile`'s virtual read, so the field order is read off the disassembly,
`tools/ghidra/ExportListing.java`) and `WildStar_d.exe` @0x82658168 (the `MSH0` revision, which
decompiles cleanly through `Read32u` and reads the same fields in the same order).

Grades: PROVEN (engine code, or checked on every body of the drop), INFERRED (consistent with every
body, no engine reader found), UNKNOWN.

## Correction

The previous version of this document, and the workshop decoder built on it, read every body one byte
late. The "big-endian u24 + pad byte" encoding, the "Sean-shape vs non-Sean" split, "little-endian vertex
and index buffers" and "24-bit material hashes" were all artefacts of that off-by-one. The fields follow
the name **directly**, as plain big-endian `u32`s, with no padding; vertex and index buffers are
big-endian; the material hash is the full 32-bit hash. This is why prototype meshes rendered shattered.

## Revisions

The magic's fourth byte is a format revision, and it matches the revision byte of the `SBL*` pack the
record sits in. PROVEN (every record):

| magic | records | packs | read by |
|---|---:|---|---|
| `MSH4` | 76,240 | `SBL4` | `WildStar_p` / `Saboteur.xex` (`ReadMesh` compares with `'MSH4'` @0x8236df58) |
| `MSHL` | 60,185 | `SBLL` | — |
| `MSH0` | 965 | `SBL6` (959), `SBL0` (6) | `WildStar_d` (compares with `'MSH0'`) |
| `MSH8` | 234 | `SBL8` | — |
| `MSHR` | 215 | `SBLX`, inside variant records | — |
| `MSH5` | 20 | `SBL5` | — |
| `MSH6` | 2 | `SBL6` | — |

`WildStar_p`'s `ReadMesh` rejects any magic other than `MSH4` with the assert "%s, Old Exported Mesh
Version" and loads nothing. **Every body of every revision parses with the `MSH4` layout below**, with
all of the reader's invariants (137,861 bodies, 0 failures). PROVEN.

## Body

```
+0      char    magic[4]
+4      u32     name_len            includes the NUL; ReadMesh asserts "Bad Mesh, name is huge" above 0x400
+8      char    name[name_len]
        u32     stream_count
        u32     occluder_count      0 in every body
        u32     shadow_count        0 in every body
        u32     reserved            read and discarded; 0 in every body
        u32     shadow_caster       != 0 sets WSModel+0x4F bit 7 = WSModel::IsShadowCaster
        Stream  streams[stream_count]
        [occluders, shadow volumes — absent]
        u32     bone_count          'CLOT' (0x434C4F54) makes ReadMesh return: cloth, none in the build
        Bone    bones[bone_count]
        u32     remap_count
        Remap   remaps[remap_count]
        f32     centre[3]; f32 half[3]; f32 radius      -> WSModel +0x20 .. +0x38
        tail
```

* **`shadow_caster`** — `WSModel::IsShadowCaster` (`WildStar_d.exe` @0x82472748) returns `+0x4F >> 7`,
  which `ReadMesh` sets from this word. True on 109,845 object bodies, 1,497 baked tiles and 24
  chunks; false on every collision body. PROVEN.
* **Occluders and shadow volumes.** The reader allocates `occluder_count` × 0x24-byte `OdinOccluder`s
  (`u32 nverts, nidx, nidx2, flag`, float3 vertices, two u16 index lists, `GenerateNormals`) and
  `shadow_count` × 0x20-byte `SHDW` records (tag `'SHDW'`, four u16 counts, a flag, then lists).
  Both counts are 0 in every body, so `mesh::parse` rejects a non-zero count rather than decode a
  layout no data exercises. PROVEN (counts).
* **The tail is not read by the engine.** `ReadMesh` returns after the seven bounds floats; neither
  decompile contains the `'MSHC'` constant. The tail is one of: `"MSHC"` + u32 (110,232 objects,
  23,000 chunks), a **little-endian** 4×4 matrix (3,518 baked tiles — written by the PC-side exporter),
  or nothing (1,111 baked collision bodies). PROVEN.

## Stream

```
u32 format, index_count, vertex_count, bone, stride, prim_count
u8  vertices[vertex_count * stride]     big-endian elements, Layout::of(format)
u16 indices[index_count]                big-endian
Prim prims[prim_count]                  ReadMesh asserts prim_count < 10000
```

* `bone` indexes the stream's bone (it becomes each segment's bone index). `stride` must equal the
  declaration's size — `ReadMesh` asserts it against `XenonSharedBuffers::GetStride`. PROVEN.

### Vertex layout — `XenonSharedBuffers::FindDecl`

`FindDecl` (`WildStar_p.exe` @0x829a7f10, `WildStar_d.exe` @0x83157798) builds the D3D vertex
declaration from the format word. Element types are `D3DDECLTYPE_*` values read from the
`WildStar_p.pdb` enum (`LF_ENUMERATE` records). In declaration order:

| condition | element | usage |
|---|---|---|
| `fmt&3 == 1`: bit 31 → `USHORT2` (4 B), else `FLOAT2` (8 B) | position | `USHORT2` uses `BLENDINDICES` |
| `fmt&3 == 2`: bit 24 → `FLOAT16_4` (8 B); else bit 31 → `USHORT2`; else `FLOAT3` (12 B) | position | |
| `fmt&3 == 3`: `FLOAT4` (16 B) | position | |
| bit 2 | `UBYTE4N` weights, then `UBYTE4` indices | `BLENDWEIGHT`, `BLENDINDICES` |
| bit 4, bit 5 | `UBYTE4N` each | `COLOR` 0, 1 |
| `(fmt>>8)&0xF` coordinate sets | bit 25 → `FLOAT16_2` (4 B), else `FLOAT2` | `TEXCOORD` n |
| bit 12 (bit 30 clear) | bits 12+26 → `DEC3N` (4 B), else `FLOAT3` | `NORMAL` |
| bit 13 (bit 30 clear) | `UBYTE4N` | `TANGENT` |
| bit 14 (bit 30 clear) | bits 14+28 → `DEC3N`, else `FLOAT3` | `BINORMAL` |
| bit 30 | normal, tangent, binormal each `FLOAT3` | |

`ReadMesh` creates its buffers with a zero texture-coordinate type word, so every set is type 0. Bit 29
(the `2Fxxxxxx` baked-tile formats) adds no element; `ReadMesh` only counts it into a global. PROVEN.

Element decoding:

* `FLOAT16_*`, `FLOAT*`: big-endian halves/floats in stored order. PROVEN by placement — normalised
  detail streams land on the terrain lattice (below).
* `DEC3N`: one big-endian u32, x = bits 0–9, y = 10–19, z = 20–29, signed, /511. PROVEN (face-normal
  agreement, previous session).
* `UBYTE4N` / `UBYTE4`: one big-endian u32, **x = the last byte** (x = byte 3, y = 2, z = 1, w = 0).
  PROVEN: read that way the tangent is perpendicular to the normal in 145,621 streams (mean |dot| 0.016
  against 0.484 in stored order); the 16,196 other streams have degenerate tangents in both orders.
* Terrain `8F000001` (`USHORT2`, 4 bytes): the first big-endian u16 is a height, the second a lattice
  index (below).

Formats present (stride): `0F000002` (8), `0E000002` (12), `0F001102` (16), `0F001106` (24),
`0F001112` (20), `0F003102` (20), `0F003106` (28), `0F003112` (24), `0F003202` (24), `0F003206` (32),
`0F003302` (28), `0F003402` (32), `0F003512` (40), `2F000102` (12), `2F002102` (16), `8F000001` (4) and
others — the full list with counts is printed by `proto2008-meshes`; `Layout::of` reproduces every
stored stride.

### Normalised positions

Every `FLOAT16_4` position stream **without a normal** (17,086: baked tiles and chunk detail streams)
lies in [0,1]³; every stream that is not normalised has a normal (166,364). 898 streams with normals
also lie in [0,1]³ (small props such as `P_Global_A_Crate_X1YHalfB_DAM`). INFERRED: the dequantisation
happens in the vertex shader (no CPU code builds it; see Unknowns). The model-space position is
`box_min + q · box_size` of the stream bone's `PclAABB` (`centre ∓ half`).

### Terrain chunks — `8F000001`

`u32 = (u16 height << 16) | u16 lattice`; `Zq = lattice / 17`, `Xq = lattice % 17` (a 17×17 lattice).
`x = origin.x + Xq · 2.34375`, `z = origin.z + Zq · 2.34375`, where `origin` is the stream bone's
chained translation (= the box minimum in every chunk), and `y = box_min.y + height / 65535 · 2·half.y`.
PROVEN by data: 466,644 shared lattice points between chunks of one pack, 970 disagree (all within one
height quantum inside a block). The step 2.34375 = 37.5 / 16 (chunks sit on a 37.5 m grid): the
constant is not in CPU code (`WildStar_p` decomp searched for 2.34375, 37.5 and their bit patterns) —
the terrain vertex shader applies it.

## Prim — one `OdinSegment`

```
u32 strip            != 0 selects a triangle strip (D3DPT 6) over a list (4); 0 in every prim
u32 triangle_count   asserted equal to the XenonPrimitive's primitive count
u32 material         OdinMaterialManager::GetMaterial(hash) — "Mesh %s has a material %d that doesn't exist"
u32 lod              segment flags & 7 = OdinSegment::GetLodLevel
u32 segment_bits     stored at segment flag bits 4..7
u32 index_start
u32 index_count      asserted != 0
```

* `material` is the full 32-bit hash of a `WSAO` material (`docs/2008_materials_format.md`). PROVEN.
* `lod`: `OdinSegment::GetLodLevel` (`WildStar_d.exe` @0x83175d80) returns `flags & 7`;
  `OdinMesh::SetupLodSegmentCount` counts segments per LOD into four slots. Values: 0 (objects,
  chunks), 1 (every baked-tile prim, 12,961; 1,913 object prims), 2 (57). PROVEN.
* `segment_bits`: `ReadMesh` writes `lod | segment_bits << 4` into the segment's flag word (and sets bit
  3 when the material's flags have 0x10). The engine tests flag bits `& 7` (LOD), `& 8`
  (`OdinMesh::IsBoneRendered`, `Submit`) and `& 0x80` (`OdinMeshInstance::InstanceSegment`: a cloned
  primitive); no code reads bits 4–6 (both decompiles searched). Values: 1 on 17,718 of 23,584 prims of
  `*_DAM` meshes and on 10,159 other object prims; otherwise 0. PROVEN (no reader); the authoring
  meaning is UNKNOWN.

## Bone (77 bytes) and remap (68 bytes)

```
Bone:   u32 hash; i32 parent; f32 translation[3]; f32 rotation[4] (quaternion); f32 scale[3];
        f32 centre[3]; f32 half[4]                   -> PclAABB
        u8  export_flags                              -> PclBone::Setup(..., flags)
Remap:  u32 bone; f32 matrix[16]                      -> OdinMesh +0x14
```

* `ReadMesh` builds a `PclSkeleton` (`bone_count == 1` shares `PclSkeleton::ms_pOneBoneSkeleton`) and
  for each bone a `PblTransformSRT` → `PblMatrix4x4`, `CreateBone(index, parent, srt, matrix)`, then
  `PclBone::Setup(crc, 0, aabb, BoneMappingNameInvert(hash), export_flags)`. PROVEN.
* `export_flags` is `PclBone::GetExportFlags` (+0x1C), read only by `WSHavokModel::Setup`
  (`WildStar_d.exe` @0x829e1660) to choose the bones it matches to physics bodies (bits 1, 2, 4).
  Values: 0, 1, 2, 4, 5, 6 — the non-zero ones only on breakable/destructible objects. PROVEN (reader);
  per-bit meaning INFERRED from that reader only.
* Parents precede children (`parent < index`) in every body. PROVEN.

## Variant records (`SBLX`)

The 430 raw `meshes` records of `SBLX` packs are:

```
u32 count
{ u32 material; u32 key; u32 n; { u32, u32 }[n] }[count]
zero bytes to the next 0x800 boundary
an MSHR body
```

`material` is a material UID of the same pack's `WSAO` slot (e.g. `0x45F35344` =
`DefaultLib.LB_C2_pants:0` in `Global/BP_CV_RaceDrv_LB_Blue.Pack`), and the pairs repeat that
material's texture references. PROVEN for the framing (all 215 records with a body parse). No engine
in the drop reads `SBLX` packs; the pairs' meaning is the material format's (see the materials doc).

## Placement

The `wsd` slot's EditNodes objects place meshes by name (`ClassName`), with `Position` and
`XAxis`/`YAxis`/`ZAxis` (`docs/formats/editnodes.md`, big-endian here). Over every pack:

* **Objects**: unit axes; `Position` is the instance position. PROVEN (91,438 objects).
* **Baked tiles**: the axes carry the tile's size (e.g. lengths 141.5, 22.7, 100.0) and the root bone's
  box is the unit cube, so `world = Position + q.x·X + q.y·Y + q.z·Z` for a normalised position `q`.
  PROVEN (3,516 tiles).
* **Chunks**: unit axes and `Position` equal to bone 0's translation (22,835 of 22,993; the rest are
  chunk-named objects). The streams' boxes are already in the chunk's frame, so `world = Position +
  (local − bone0.translation)` reproduces the terrain lattice. PROVEN (data); whether the engine drops
  the root bone's translation or the instance position cannot be told apart from the data, since they
  are equal.
* The baked tiles' little-endian tail matrix is the exporter's copy of the same transform (8,272 of
  12,559 detail vertices within 0.5 m of the terrain, previous session).

## Directory keys

`MSH4` records are keyed `pandemic_hash(mesh name)` (76,240 of 76,240). The other revisions' keys are
not: not `pandemic_hash` of the name with `.mesh`/`.meshes`/`.msh` suffixes, a `meshes\` prefix, the
pack stem, or upper/lower case; not CRC-32; not FNV-1/1a with or without the final mix; not the body's
`MSHC` hash. PROVEN (negative); preimage UNKNOWN. Bodies are identified by their own name.

## Method

```
sab_probe proto2008-meshes <Bin360>      # every record; formats, prims, bones, tails, placement
analyzeHeadless <proj> WildStarP -process WildStar_p.exe -noanalysis -readOnly \
  -scriptPath tools/ghidra -postScript ExportListing.java <out> 0x8236def8   # ReadMesh disassembly
```

## Unknowns

* Where the shader gets the terrain lattice step and the unit-cube dequantisation (the `.shaders` banks
  hold no float literals in the clear).
* The `w` lane of the bit-29 baked-tile positions (values ≈ 163–172): it reaches the vertex shader as
  `position.w`.
* `segment_bits`' authoring meaning; the older revisions' directory key preimage.
