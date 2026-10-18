# MESH skeleton — confirmed byte layout

All offsets are into the **decompressed** MESH body (the zlib payload after the 276-byte `MSHA`
header; there is **no** `MESH` magic in the decompressed body — SaboteurToolset synthesises it).
Confirmed against `CH_AL_SeanDevlin_01_GR` in `Global/Dynamic0.megapack` (PC/GOG build) and the
PredatorCZ/SaboteurToolset reference. `u32`/`u16`/`i16`/`f32` are little-endian.

## Container path to a MESH body

```
.megapack  magic "00PM" @0, u32 fileCount @4, then fileCount × index entry @8:
             { u32 crc; u32 index; u32 size; u64 offset }          (20 bytes, packed)
  @entry.offset:  SBLA sub-pack, magic "ALBS", u32 0                (uncompressed container)
     within it:   MSHA wrapper (byte-packed, NOT aligned):
                    u32 id "AHSM"; u32 uncompressedSize0; u32 uncompressedSize1;
                    u32 compressedSize0; u32 compressedSize1; char name[0x100]   (276 bytes)
       @+276:      MESH body: zlib stream (0x78 0x01), compressedSize0 → uncompressedSize0 bytes
       @+276+c0:   companion .dat (VB/IB): zlib, compressedSize1 → uncompressedSize1
```

The MSHA `name` is plaintext ASCII (e.g. `CH_AL_SeanDevlin_01_GR`). On this PC build the payload is
plain **zlib**, not `sges` (the megapack contains zero `sges` blocks). The reader scans the whole file
for `AHSM` byte-wise (headers are not aligned).

## MESH header (244 bytes, offset 0)

| off | size | field |
|----:|-----:|-------|
| 0   | 76   | 19 × u32 = 0 |
| 76  | 28   | BBOX_: `Vector min` (3×f32) + `Vector4 max` (4×f32) |
| 104 | 44   | 11 × u32 = 0 |
| 148 | 4    | `name` (StringHash u32) |
| 152 | 32   | 8 × u32 = 0 |
| 184 | 4    | `unk0` |
| 188 | 16   | 4 × u32 = 0 |
| **204** | 4 | **`numBones0`** (u32) |
| 208 | 4    | `numBoneRemaps` (u32) |
| 212 | 4    | u32 = 0 |
| 216 | 2    | `numStreams` (u16) |
| 218 | 2    | `numPrimitives` (u16) |
| 220 | 12   | 3 × u32 = 0 |
| 232 | 4    | `numDrawCalls` (u32) |
| 236 | 8    | 2 × u32 = 0 |

## Skeleton block (present iff `numBones0 > 1`), offset 244

Without it (`numBones0 <= 1`) the MESH tail ([mesh_geometry.md](mesh_geometry.md)) starts at 244 and
`numBoneRemaps` is 0 (SaboteurToolset `mesh_to_gltf.cpp` `AppProcessFile`). A record whose `.dat` is
empty has no streams: a bare skeleton (`ragdoll_b`, `saboteur_fm`, `Bird`). `sab_workshop` reads
every Dynamic0 mesh the retail assets reference both ways, with the tail ending exactly at the end
of the body (`--texcheck`: 984 of 984 assets).

Header, 11 × u32 @244 (44 bytes):

| idx | field |
|----:|-------|
| 0 | `numUnkBones0` (count of trailing null-`u8` pad after `boneIds`) |
| 1,2 | 0 |
| 3 | **`numBones`** (== numBones0) |
| 4 | `numUnkBones1` (if ≠0, one trailing null `u16` after the block) |
| 5 | `numBones` (dup) |
| 6 | 0 |
| 7 | `numBones` (dup) |
| 8,9,10 | 0 |

Arrays follow, in order (let `N = numBones`), starting @288:

| array | element | size |
|-------|---------|-----:|
| `boneIds` | u8 | N |
| *(pad)* | u8 = 0 | numUnkBones0 |
| `localTMS` | 4×4 f32 matrix | 64·N |
| `bones` | Bone (below) | 64·N |
| `transforms` | RTSValue: translation(4f) + rotation(4f, **xyzw**) + scale(4f) | 48·N |
| `parentIds` | **i16** (-1 = root) | 2·N |
| *(pad)* | u32 = 0 | 4·N |
| *(pad)* | u16 = 0 | 2 if numUnkBones1≠0 |

**Bone** (64 bytes): `u32 boneName0` @0; 4×u32=0 @4; `u32 boneName1` @20; u32=0 @24; `u32 unk0` @28;
BBOX (2×`Vector4`) @32.

> **Fully pinned for *writing* (verified on all 168 bones of `CH_AL_SeanDevlin_01_HD`, 2026-07-17):**
> - `boneName1` @20 **== `boneName0`** (168/168 identical) — the hash stored twice, not a second name.
> - `unk0` @28 **== 0** (constant across every bone).
> - BBOX @32 is `min:Vector4` then `max:Vector4`. Every non-root bone uses the **empty sentinel**
>   `min=(0,0,0,0)`, `max=(-10000,-10000,-10000,-10000)`; only the root (`GlobalSRT`) carries a real box.
> - `boneIds` @288 is plain **identity `0..N-1`** for a whole-skeleton mesh.
>
> Consequence: a record synthesized as `{hash, 16×0, hash, 0, 0, empty-sentinel}` reproduces the real
> bytes for **167/168** bones (root differs only in its BBOX). So the **entire skeleton section is
> authorable from a `.skel`** (names→hashes, RTS→`transforms`+`localTMS`, parents→`parentIds`) with **no
> donor MESH needed** — the basis for the Mattias-port MESH encoder (`docs/mattias_port_plan.md` Stage 2b).

After the skeleton come `BoneRemaps[numBoneRemaps]` (each `{ 4×4 f32 ibm; u32 boneId }`), `Streams`,
`Primitives`, `DrawCalls` — not needed for the skeleton.

## Semantics (empirically pinned)

- **name_hash** = `boneName0` = `pandemic_hash(name)`. Verified: `0xCBC1EB51`=`GlobalSRT` (root),
  `0x24C5009C`=`Bone_Hips`, `0x4C7733ED`=`Bone_Chest`, `0x705C4508`=`Bone_Head`, … These exact hashes
  key the animation tracks (they occur in `Animations.pack`'s ANIM bone list, e.g. offset ~1807).
- **parentIds** form a single-rooted forward tree (root `parent == -1`; every child index > parent).
- **local bind** = `transforms[i]` (t / r-xyzw / s), relative to parent. It equals `localTMS[i]`
  decomposed (agreement 1.6 × 10⁻⁷).
- **localTMS** is 16 f32 **row-major with translation in the last row** (indices 12,13,14). Transpose to
  column-vector math (translation → column 3), then `world[i] = world[parent] · localTMS[i]`;
  `inv_bind[i] = world[i]⁻¹`. Composing yields a standing humanoid (Y span 0 → 1.78 m; pelvis Y ≈ 1.06 m).
