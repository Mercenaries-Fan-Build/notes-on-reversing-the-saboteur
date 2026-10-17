# 2008 Xbox 360 prototype — animations (`ANM0` clips, `Animations.pack`)

**Scope:** the animation clips of the 2008-05-20 Xbox 360 prototype (`Bin360/`) — loose `ANM0`
files and the `AP04 ANIM` pack — read, decoded and sampled the way the engine does.
**Library:** [`sab_formats::proto2008::anim`](../tools/sab_formats/src/proto2008/anim.rs) (one clip:
header, bone CRCs, decoded poses, sampling) and
[`sab_formats::proto2008::anim_pack`](../tools/sab_formats/src/proto2008/anim_pack.rs) (the pack).
**Engine oracle:** the WildStar_d build of the same code, whose decomp carries full symbols
(`output/_ghidra_wildstar/wildstar_d_all_functions_decomp.txt`); VAs below are WildStar_d.

**Status:** every clip parses to its last byte, decodes and samples — 3,590 loose clips the loose
loader accepts, all 1,605 pack clips; the 30 loose clips it rejects are listed below. The pack reads
to its last byte. PROVEN by the game gate `every_2008_animation_parses_and_decodes`
(`tools/sab_formats/tests/proto2008_retail.rs`) and the probe `sab_probe proto2008-anims` (Method).

## Retail vs. 2008

| | Retail PC | 2008 Xbox 360 |
|---|---|---|
| Pack | `Animations.pack`, `AP0L` + one Havok 6.5 packfile | `Animations.pack`, `AP04 ANIM`: records + tagged sections, no Havok packfile |
| Clip | `hkaSplineCompressedAnimation` | `ANM0` + `WAV0` / `DEL0` / `NOC0`: Pandemic's serialization of Havok 4.6 classes |
| Endianness | little | big-endian headers and `NOC0` transforms; little-endian tables inside `WAV0`/`DEL0` data |
| Track → bone | per-clip index list in the `ANIM` block | one bone name CRC per track, resolved with `PclSkeleton::FindBoneIndex` |
| Loose clips | none | 3,620 `ANM0` files (`animations/`, `ObjectAnimations/`, `TestAnims/`), `.hkx` extension |

## Where the clips are

| | WAV0 | DEL0 | NOC0 |
|---|---:|---:|---:|
| `animations/` | 3,090 | 282 | 214 |
| `ObjectAnimations/` | 9 | 3 | 21 |
| `TestAnims/` | — | 1 | — |
| `Animations.pack` | 1,501 | 6 | 98 |

Each pack record is keyed by `pandemic_hash` of the animation's name. **Every one of the 1,605 keys is
`pandemic_hash` of a loose clip's file stem**; 1,601 pack clips are byte-identical to that loose
file, 4 differ (`idle_smoke_stop`, `idle_smoke_flick`, `idle_smoke_flick2`, and `civ_F_idle1`, whose
loose file is a 24-byte empty `NOC0`). Two stems exist in both `animations/` and `ObjectAnimations/`
(`sabotage_valve_success_obj`, `sabotage_valve_failure_obj`). PROVEN (probe).

## `ANM0` clip

`PclAnimationManager::LoadAnimFile` @0x83197e50 (a loose file) and `LoadAnimFromPackfile`
@0x8319e6f8 (a pack record) read: `ANM0`, a codec word, the codec loader's fields, then the bone
list. All words big-endian.

```text
+0x00  "ANM0"
+0x04  codec        "WAV0" | "DEL0" | "NOC0"
+0x08  f32 duration (seconds)                       object +0x0C
+0x0C  i32 tracks                                   object +0x10
       codec fields and data (below)
       i32 n, n x u32 bone name CRC                 one per track (n == tracks in every clip)
```

### `WAV0` — `LoadWaveletAnim` @0x83197080 → `hkWaveletSkeletalAnimation`

```text
+0x10 i32 numberOfPoses   +0x14 i32 blockSize   +0x18 u8 maxBitWidth   +0x19 u8 preserved
+0x1A u32 numD, offsetIdx, scaleIdx, bitWidthIdx, staticMaskIdx, staticDOFsIdx,
          blockIndexIdx, blockIndexSize, quantizedDataIdx, quantizedDataSize
+0x42 i32 dataSize        +0x46 data[dataSize]
```

### `DEL0` — `LoadDeltaAnim` @0x83197620 → `hkDeltaCompressedSkeletalAnimation`

```text
+0x10 i32 numberOfPoses   +0x14 i32 blockSize   +0x18 u8 maxBitWidth   +0x19 u8 preserved
+0x1A u32 numD, offsetIdx, scaleIdx, bitWidthIdx, quantizedDataIdx, quantizedDataSize,
          staticMaskIdx, staticMaskSize, staticDOFsIdx, staticDOFsSize, totalBlockSize, lastBlockSize
+0x4A i32 dataSize        +0x4E data[dataSize]
```

### `NOC0` — `LoadUncompressedAnim` @0x83197be0 → `hkInterleavedSkeletalAnimation`

```text
+0x10 u32 numTransforms   +0x14 numTransforms x 48-byte hkQsTransform, big-endian:
      T.x T.y T.z T.w  R.x R.y R.z R.w  S.x S.y S.z S.w   (T.w, S.w unused)
      frame-major: transform [frame * tracks + track]; frames = numTransforms / tracks
```

### What the loaders reject (PROVEN, decomp)

| check | where | effect |
|---|---|---|
| `duration >= 1000.0` | `LoadWaveletAnim`, `LoadDeltaAnim` | clip not loaded |
| `dataSize` outside `1..=0x2FFFFF` | same | clip not loaded |
| `numTransforms >= 0x5555556` | `LoadUncompressedAnim` | allocation size −1 |
| `tracks` outside `1..=4999` | `LoadAnimFile` only — the pack loader does not check | loose clip not loaded |
| record end ≠ `start + size` | `LoadAnimPackFile` | assert |

The 30 loose clips `LoadAnimFile` rejects are all `NOC0` with 0 tracks: 20 `face_*` poses (6–18
transforms each), `aurora_dead_idle_drvr`/`_pass`, `cvr_low_idle_2`, `shrd_M_crowd_couple_01`,
`civ_F_idle1`/`2`, `civ_couple_wall_flirt_F`/`M` and `sabotage_valve_*_obj` (the last six with no
transforms). The library parses them; `Clip::check_loose` reports the rejection and `Clip::decode`
fails (frames = transforms / 0).

## The `WAV0` / `DEL0` data buffer

`handleEndian` (@0x83330948 wavelet, @0x833328b8 delta) byte-swaps the tables on load, so they are
**little-endian on disk**. Offsets are relative to the data buffer.

* **Track masks** at `staticMaskIdx` (0 in every clip): one `u16` per track. Bits 0–1, 2–3, 4–5 are
  the translation, rotation and scale group types; type 2 is identity (`recompose` writes zero / unit
  quaternion / unit scale and reads nothing). Bits 6–15 (`mask >> 6`) flag dynamic components:
  `4 2 1` = T.x T.y T.z, `0x40 0x20 0x10 0x8` = R.x R.y R.z R.w, `0x200 0x100 0x80` = S.x S.y S.z.
* **Static values** at `staticDOFsIdx`: one `f32` per component of a non-identity group that is not
  dynamic, in recompose order.
* **Per dynamic channel** (`numD` of them; the masks' dynamic bits always sum to `numD`): `f32`
  offset at `offsetIdx`, `f32` scale at `scaleIdx`, `u8` bit width at `bitWidthIdx` (1..=16).
* **`WAV0`**: a `u32` block index at `blockIndexIdx` (`blockIndexSize` = ⌈poses / blockSize⌉ entries,
  offsets into the quantized data), quantized data at `quantizedDataIdx`.
* **`DEL0`**: block `b` at `quantizedDataIdx + b * totalBlockSize`. `staticMaskSize` is 0 and
  `staticDOFsSize` / `lastBlockSize` are not the sizes of anything the decoder reads; the decoder
  derives every size from the masks and `numberOfPoses`.

`hkTrackAnalysis::recompose` @0x83338d18 builds each track's transform from the next dynamic value
where the mask bit is set and the next static value otherwise, in the order T.x T.y T.z, R.x R.y R.z
R.w, S.x S.y S.z.

Value sets in the build (probe): `WAV0` blockSize 16, preserved 0, maxBitWidth 11; `DEL0` blockSize
8 or 2, preserved 1, maxBitWidth 8. maxBitWidth is 0 exactly in the clips with no dynamic channels
(`numD` 0: 34 + 24 `WAV0`, 128 + 3 `DEL0`, loose + pack).

## Decoding

**`hkExpandReal`** @0x8333a458, shared: `preserved` raw `f32`s, then per sample
`(q + 0.5) * (scale / 2^bits) + offset`, `q` read LSB-first from a little-endian bit stream.
`hkCalcQuantizedSize` @0x8333b698: `preserved * 4 + ⌈bits * (n − preserved) / 8⌉` bytes per channel.

**`WAV0`** (`decompressBlockToCache` → `_decompress`): per block, per channel in order:

1. `hkBlockDecode` @0x8333aca0 — `preserved` raw words; a skip bitmap of `count` bits, LSB-first (set =
   coefficient not stored); the stored codes LSB-first from 16-bit little-endian words. A skipped
   coefficient takes the zero code `(int)(2^bits * −offset / scale)` (2^bits becomes 2^bits − 1). The
   source advances `⌈(read * bits + 32 * preserved + count) / 8⌉` bytes.
2. `hkExpandReal` over the decoded codes.
3. `hkInverseWaveletTransform` @0x833393f0 — an 8-sample basis multiply (coefficient 0 adds to every
   sample; rows 1–7 from `.rdata` 0x8229f030..0x8229f12f), then per doubling `hkUpdateReflect(−¼)`,
   `hkPredictReflect(+½)`, `hkMergeOddEven`. The basis rows equal inverse CDF 5/3 lifting from one
   sample (unit test `basis_equals_lifting_from_one_sample`).

Every block's channels end inside the block (the next block-index entry, or the end of the quantized
data): PROVEN for all 3,099 loose and 1,501 pack `WAV0` clips (the decoder fails otherwise).

**`DEL0`** (`_decompress` @0x83331c90): block `b` holds `min(blockSize, poses − b * blockSize)`
frames; per channel, `hkExpandReal` over them, then `hkInverseDeltaTransform` @0x8333be58 (running sum
`x[i] += x[i − 1]`). Full blocks take exactly `totalBlockSize` bytes in every clip.

**`NOC0`**: the transforms as stored.

### How good the decoded rotations are

`|‖q‖ − 1|` over every stored pose (probe): `NOC0` ≤ 0.0001; `DEL0` ≤ 0.0146 (235 of 12.4 M
transforms above 0.01); `WAV0` ≤ 0.0393 (827 of 7.8 M above 0.01). The compressed codecs store
quaternion components independently, so a non-unit result is the codec's quantization error; the
engine normalizes the pose after sampling (below).

There is **no uncompressed twin of any compressed clip** in the build, so the compressed decoders
have no numeric ground truth: their correctness rests on reproducing the engine's functions step by
step, every block's bytes being consumed exactly, and the rotation norms above. A second,
independently structured implementation (the research decoders the library replaced) gives
bit-identical poses and samples on every loose `WAV0`/`DEL0` clip (525,833 poses, 23,695 samples).

## Sampling — `samplePartialPose`

Wavelet @0x8332fdc0, delta @0x83331e08, interleaved @0x83332c10. All assert `time >= 0` and
`duration > FLT_EPSILON` (0x34000000); the compressed ones assert `numberOfPoses >= 2`, the
interleaved one `numTransforms >= 1`. `Animation::sample` returns an error for each.

* **Compressed** — `getFrameAndDelta` (@0x8332ed98, @0x83330df8): `x = time / (duration / (poses − 1))`,
  frame `min((int)x, poses − 2)`, delta `min(x − frame, 1)`. Each dynamic value is
  `(1 − delta) * v[frame] + delta * v[frame + 1]` (the next frame may be the first sample of the next
  block), then `recompose`. Rotations come out unnormalized.
* **Interleaved** — for `time < duration`: the same `x`, frame `(int)x`, delta `x − frame`, and
  `hkQsTransform::setInterpolate4` @0x83333430 between frames `frame` and `frame + 1`: translation
  and scale interpolated linearly; rotation `(1 − t) * a + t' * b` with `t' = −t` when
  `dot(a, b) < 0`, then normalized. For `time >= duration`: the last `tracks` transforms.
* The engine then normalizes every rotation of the pose: `PclAnimator::SetupPose` @0x831ba6a0 calls
  `PclPose::NormalizeQuats` @0x831aca70. A consumer of `Animation::sample` does the same.

Every loose clip the loader accepts and every pack clip passes the sampler's preconditions (gate).

## Bone binding

The trailer lists one bone name CRC per track (count == tracks in all 5,195 accepted clips). With the
record's object flag 0 (a character animation), the loader resolves each with
`PclSkeleton::FindBoneIndex` into the `i16` track → bone table it hands `PclAnimation`; with the flag
set (an object animation) it keeps the CRCs and stores −1 indices. Loose clips are loaded as
character animations unless the caller passes the object flag (`LoadAnimFile`'s last argument).
PROVEN (decomp of both loaders).

## `Animations.pack` — `LoadAnimPackFile` @0x8319ebf8

```text
"AP04ANIM"  i32 n   n x record   i32 blendBoneException
"INTV" "SEQC" "TRAN" "EDGE" "DIST" "BANK" "ADD0" "ALPH"     (each required, in this order)
```

The pack reads to its last byte (14,917,957). Counts are signed and loop `for (i = 0; i < n; i++)`.

**Record:** `u32` key, `u8` object flag, `i32` clip size, the `ANM0` clip (the loader asserts it ends
at start + size), then the `PclAnimation` members: `f32` motion length (+0x20), 3 `f32` movement
(+0x24), `f32` rotation (+0x30), `f32` start aim, end aim, aim pitch (+0x34..+0x3C), `u8` turn
positive (bit 7 of +0x7C); `i32` event count, each `{i32 frame, u32 name CRC, 4 x (u32 parameter CRC,
f32 value)}`; `i32` offset count, each `{3 f32, f32}` (`PclAnimOffset`). Object flag set on 2
records; 822 events, 205 offsets. If the clip fails to load, the loader skips these members and the
stream is misread — the library fails the pack instead.

| section | layout | in the build |
|---|---|---|
| `INTV` | n x `{u32 type CRC, i16, i16, n x u32 animation key}` — a `PclAnimInterval` (the two `i16`s at +0x0C, +0x0E) added to each animation; only `UnInterruptible` (0x501B960D) is accepted | 3: values (0, 10), (0, 5), (0, 30) |
| `SEQC` | n x `{u32 name CRC, n x instruction {u32 word0, i32 word1, n x u32 animation key, n x u32 tag CRC}, body}`; body = `f32` length, 3 `f32` movement, `f32` rotation, `u32` type, 16-byte `AimMinMax`, 3 signed byte counts, that many 8-byte aim frames per count, `u8` turn positive, `u8` synchronized | 1,944 sequences, 4,181 instructions |
| `TRAN` | n x container `{u32 name CRC, n x transition}`, below | 1 container, 286 transitions |
| `EDGE` | 0x1F80 + 0x360 bytes copied into `PclEdgeTransData` | |
| `DIST` | 0x60 bytes | |
| `BANK` | n x `{u32 name CRC, i16 length, name bytes, u32 parent CRC (0 = none), n x (u32, u32) AddOverride}` | 24 banks; name CRC = `pandemic_hash(name)` for all |
| `ADD0` | n x `(u32, u32)` `AdditiveAnimNamePair` | 0 |
| `ALPH` | n x u32 → manager +0x88; n x u32 → manager +0x7C | 1,942 (every one a `SEQC` name); 1,605 (exactly the record keys, in another order) |

**Transition** (`PclTransition`, 0xBC bytes): `u32` from sequence (+0x00), from tag (+0x04), to
sequence (+0x08), to tag (+0x0C); `f32` minimum time (+0x10); `u32` inline type (+0x14), `f32` inline
time (+0x18); `u32` inline sequence name (+0x1C) — when nonzero, `u32` animation key, `u32` tag, and a
`SEQC` body follow; `u32` type (+0x20), `f32` time (+0x24), `i32` instruction index (+0x28), `i16`
length and that many label bytes (+0x2C, NUL-terminated, e.g.
`[TRANS]'LADDER_IDLE'(left_foot)->'ANY'(left_foot)`).
`PclTransition::Match` @0x831c1d18 applies a transition when the target sequence's name is the to
sequence (or `ANY`), the current one's is the from sequence (or `ANY`), the target sequence has the to
tag and the current instruction the from tag (0 = no condition), and the time is at least the
minimum. Types (`GenerateDebugString` @0x831c25f8): 0 SET, 1 FADE, 2 QUEUE, 3 SYNC, 4 QUEUE_FADE.
In the build: type SET 81, FADE 155, QUEUE 2, SYNC 46, QUEUE_FADE 2; 90 transitions carry an inline
sequence, whose animation is a record key in all 90.

## Method

* Game gate: `cargo test --release --features retail --test proto2008_retail` in `tools/sab_formats`
  (reads `proto2008_dir` from `.sab-local.toml`); asserts the counts on this page.
* Probe: `cargo run --release -p sab_probe -- proto2008-anims <Bin360>` — directories × codecs, the
  rejected clips, header value sets and ranges, rotation norms, record flags, twins, section
  statistics.

## Unknowns

* What the `INTV` values, the instruction words, the `SEQC` type (two sequences hold ASCII-looking
  values, 0x65676574 and 0x656A656D) and the `EDGE` / `DIST` blocks mean.
* What reads the `ALPH` lists at manager +0x88 / +0x7C.
* The exact operation order of `hkVector4::setInterpolate4` (`a + t(b − a)` is used; the decomp is
  VMX and was not read through). Rounding-level only. INFERRED.
* Whether the 309 loose and 103 pack clips whose pose count is not `round(duration × 30) + 1` were
  authored at another rate.
