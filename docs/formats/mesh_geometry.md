# MESH geometry (skinned vertex/index) — confirmed byte layout

Companion to [`skeleton.md`](skeleton.md). That doc
covers the container path and the MESH header + skeleton. This doc covers everything the skeleton
tool skips: the **MESH tail** (BoneRemaps / Streams / Primitives / DrawCalls) in the decompressed
MESH body, and the **vertex/index buffers** in the companion `.dat`.

All offsets little-endian. Confirmed against `CH_AL_SeanDevlin_01_GR` in `Global/Dynamic0.megapack`
(PC/GOG build) and PredatorCZ/SaboteurToolset (`mesh/mesh_to_gltf.cpp` `struct ::Read()` methods,
the format proxy table, `ProcessStream`, `ProcessMesh`). Empirical proof: the tail parse cursor lands
**exactly** on the last body byte (36600/36600, 0 leftover), UNORM8 weights sum to exactly 255,
R32G32B32 normals have unit length (1.0000), all indices < numVertices, and the assembled skin
composes to bind pose to 1.5e-7 m.

## Two zlib blobs per MSHA

The `MSHA` wrapper (276 bytes) carries two size pairs. The MESH body is blob 0; the **VB/IB `.dat`
is blob 1, immediately after blob 0**:

```
@msha+276              : blob0 = zlib(78 01), compressedSize0 -> uncompressedSize0   (MESH body)
@msha+276+compressedSize0 : blob1 = zlib(78 01), compressedSize1 -> uncompressedSize1   (.dat VB/IB)
```

Confirmed on Sean: blob0 `78 01 ED 7D…` 13753→36600 B; blob1 `78 01 8C BD…` 74242→142200 B.

## MESH tail — in the decompressed **body**, immediately after the skeleton block

Read in this order (SaboteurToolset `ProcessMesh`). Counts come from the MESH header:
`numBoneRemaps @208`, `numStreams (u16) @216`, `numPrimitives (u16) @218`, `numDrawCalls @232`.

### BoneRemaps (only if `numBoneRemaps > 0`)

```
u32 unk0        (== numBoneRemaps, guard)
u32 0
BoneRemap[numBoneRemaps]:
    f32 ibm[16]     (inverse bind matrix; the engine recomputes this, tool ignores it)
    u32 boneId      (index into skeleton.boneIds[])
```
Element size **68 bytes**. Sean: 12 remaps, `boneId` = {2,13,14,15,130,177,178,179,180,181,182,183}.

### Streams[numStreams] — vertex/index buffer descriptors (152 bytes each)

Field offsets within one Stream (the rest are null padding):

| off | field | Sean stream 0 |
|----:|-------|---------------|
| 24  | `numVertices` u32 | 3389 |
| 40  | `format` u32 (vertex bitfield) | `0x1B003106` |
| 88  | `vertexBufferOffset` u32 (into `.dat`) | 0 |
| 104 | `vertexBufferSize` u32 | 122004 |
| 120 | `vertexBufferStride` u32 | 36 |
| 128 | `indexBufferOffset` u32 (into `.dat`) | 122112 |
| 132 | `indexBufferSize` u32 | 20088 |
| 140 | `faceType` u32 (==1 → triangle list) | 1 |
| 144 | `numIndices` u32 | 10044 |

Indices are **u16** triangle lists in the `.dat` at `indexBufferOffset` (`numIndices` of them).

### Primitives[numPrimitives] — sub-mesh index ranges (100 bytes each)

| off | field | note |
|----:|-------|------|
| 4   | `const0` i32 | asserted == -1 |
| 48  | `bbox` (2×Vector4, 32 B) | per-primitive AABB |
| 80  | `streamIndex` u32 | which Stream |
| 88  | `indexOffset` u32 | start index (×2 = byte offset into that stream's IB) |
| 92  | `numFaces` u32 | |
| 96  | `numIndices` u32 | |

Sean: prim0 (off 0, 10044 idx = whole), prim1 (off 0, 6564), prim2 (off 6564, 3480). prim0 = prim1∪prim2.

### DrawCalls[numDrawCalls] (16 bytes each)

```
u32 primitiveIndex
u32 material   (StringHash → external .materials / WSAO, not resolved in v1)
u32 0
u16 parentBone
u16 lodFlags   (low 3 bits = in-mesh LOD level; bit 3 is set at load, never on disk)
```
Sean: 5 drawcalls, all `parentBone == 0`, materials 0xF26EC2DF / 0x512C225F / 0xB8B7E65A / 0x12B41B52 / 0x9030FA82.

`lodFlags`: the MESH loader `FUN_00650470` builds the OdinMesh in place over this table and counts
`count[(dc+0xe) & 7]++` per drawcall (OR-ing in bit 3 when the material's `+0x28 & 0x10` is set);
`FUN_00e0d160` (SetupLodSegmentCount) keeps those counts and `FUN_00e0ce80` draws one level's run.
Retail `Dynamic0.megapack`: levels 0–2 occur; every drawcall above level 0 (6716/6716) is in an
UNSKINNED mesh (vehicles, weapons, some props). Characters do not use in-mesh levels — their LOD is a
separate mesh named by the part template's `LowDetailModel` ([gametemplates.md](gametemplates.md)).

`lodFlags` bit 4 (0x10) marks a drawcall that belongs to a **damaged-state** part. Over every retail
template with damage parts, drawcalls under a `Pristine` part never carry it (22,051) and drawcalls
under a non-pristine part always do (17,551). No engine reader of the bit was found; the engine
selects damage geometry by `parentBone` instead — see
[damage_and_destruction.md](../damage_and_destruction.md). `parentBone` is therefore not only the
rigid-attachment bone: on a destructible it is the bone whose part owns the drawcall.

## Vertex format bitfield (`format` @ Stream+40)

`positionType:2 | skinType:2 | numColors:4 | numUVs:4 | normal:1 | tangent:1 | reserved | constTag:8`
with `constTag == 0x1B` (high byte). Attributes appear in the vertex in this fixed order, each at a
cumulative byte offset:

| Attribute | present when | encoding | bytes |
|-----------|--------------|----------|------:|
| Position | always | R16G16B16A16 **FLOAT (half4)**, use xyz | 8 |
| BoneWeights | `skinType != 0` | R8G8B8A8 **UNORM** (÷255) | 4 |
| BoneIndices | `skinType != 0` | R8G8B8A8 **UINT** (palette index) | 4 |
| Color × numColors | `numColors` | R8G8B8A8 UNORM | 4 each |
| UV × numUVs | `numUVs` | R16G16 **FLOAT (half2)** | 4 each |
| Normal | `normal` | R32G32B32 **FLOAT** | 12 |
| Tangent | `tangent` | R8G8B8A8 UNORM | 4 |

The bitfield decode reproduces the toolset's 18-entry proxy map exactly (verified on `0x1B001102`
and `0x1B003106`). Sean's `0x1B003106` = Pos + BoneWeights + BoneIndices + UV + Normal + Tangent,
offsets 0 / 8 / 12 / 16 / 20 / 32, stride **36** — matches the file stride byte-for-byte.

## Skin bone index → global skeleton bone (the crucial remap)

Per-vertex `BoneIndices` are **local palette indices** (0..numBoneRemaps-1), NOT skeleton bone ids.
Resolve each through the remap to a **global** skeleton bone (0..numBones-1), matching `sab_skeleton`
/ the `.skel` bone order:

```
global_bone = skeleton.boneIds[ boneRemaps[ localIndex ].boneId ]
```

`boneIds[]` is the `numBones`-length u8 array at the start of the skeleton block (identity on Sean, but
apply it — it may permute on other assets). Toolset ref: `boneId = skeleton.boneIds.at(b.boneId)`.
As in the toolset, any influence with weight 0 has its joint index zeroed; weights are renormalized.

Verified: Sean's per-vertex palette indices span 0..11 (all < 12), resolved globals ∈ {2,13,14,15,130,
177..183}, all < 191; vertex 0 → globals {181,183,180} = children of Bone_Chest, co-located with the
mesh geometry.

## Confidence table

| Claim | Status | Evidence |
|-------|--------|----------|
| Container path, MESH header, skeleton block | **CONFIRMED** | copied verbatim from `sab_skeleton`; cursor + hashes match |
| `.dat` VB/IB = blob1 (2nd zlib, right after blob0) | **CONFIRMED** | `78 01` header at msha+276+compressedSize0; inflates to uncompressedSize1 exactly |
| MESH tail order BoneRemaps→Streams→Primitives→DrawCalls | **CONFIRMED** | toolset `ProcessMesh`; parse cursor lands exactly at body end (0 leftover) |
| BoneRemap = ibm(64)+boneId(4), 68 B; guard u32==count | **CONFIRMED** | toolset `struct BoneRemap`; guard `unk0==12` held |
| Stream 152 B, field offsets (24/40/88/104/120/128/132/140/144) | **CONFIRMED** | toolset `Stream::Read`; every field sane vs real bytes |
| Primitive 100 B (const0==-1, streamIndex@80, indexOffset@88, numIndices@96) | **CONFIRMED** | toolset `Primitive::Read`; const0==-1 held on all 3 |
| DrawCall 16 B layout | **CONFIRMED** | toolset `Drawcall::Read`; fields sane |
| faceType==1 ⇒ u16 triangle-list indices | **CONFIRMED** | toolset assert + `ReadContainer(uint16)`; index max 3388 < 3389 |
| Vertex bitfield decode (positionType/skinType/numColors/numUVs/normal/tangent, tag 0x1B) | **CONFIRMED** | reproduces toolset proxy map; stride sum == file stride |
| Position half4, UV half2, Normal R32G32B32 f32, weights UNORM8, indices UINT8 | **CONFIRMED** | toolset proxies; normals unit-length 1.0000, weights sum 255, stride 36 |
| Skin remap `boneIds[boneRemaps[i].boneId]` → global bone | **CONFIRMED** | toolset skin loop; resolved globals all < 191, spatially co-located |
| BoneRemap.ibm bytes ignored (IBM recomputed as inverse(bindWorld)) | **CONFIRMED** | toolset uses `skeleton.ibms.at(boneId)`; our IBM composes to bind (dev 6e-7) |
| Material hashes → external WSAO `.materials` | INFERRED (not needed v1) | toolset comment; not resolved here (untextured) |
| `DrawCall.lodFlags & 7` = in-mesh LOD level | **CONFIRMED** | decomp `FUN_00650470` / `FUN_00e0d160` / `FUN_00e0ce80` (GOG exe); levels 0–2 in retail data |
| `DrawCall.lodFlags & 0x10` = drawcall of a non-pristine damage part | **CONFIRMED (data)** | 22,051 / 17,551 drawcalls split with no exception against template `Pristine` flags; no code reader found |
| `Stream.unk0 @136`, `Primitive.numFaces` semantics | INFERRED | present & read, exact meaning unused |
