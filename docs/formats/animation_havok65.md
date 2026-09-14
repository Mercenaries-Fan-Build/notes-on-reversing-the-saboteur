# Animation — Havok 6.5 + the `AP0L` pack

Animation is the flagship open problem: the whole community can *extract* Saboteur animation but nobody
can *decode* it into keyframes. This doc records the ground truth and why we're well-positioned to crack
it — with an honest note on why our Mercs 2 decoder does **not** just drop in.

> **★ UPDATE (2026-07-12) — the format is 100% spline-compressed.** A class-name scan of retail
> `Animations.pack` (187 MB) found **`hkaSplineCompressedAnimation` × 9,709 and wavelet / delta /
> interleaved × 0.** The entire corpus is a *single* format — Havok 6.5 **spline-compressed** — which is
> a documented, community-understood format (HavokLib; Skyrim-era tooling), NOT the inverse-Haar wavelet
> that Mercs 2 leaned on (and never fully cracked). The wavelet framing below is therefore **superseded**:
> the decode target is spline only. The symbol map pins the sampler (`FUN_00eb7e00`
> `sampleAndDecompress` and friends — see [`../symbol_map/animation.md`](../symbol_map/animation.md)),
> and a double-blind two-investigator + validator agent effort is deriving the exact quantization. This
> doc will be rewritten with the validated spec once that lands. See memory `animation-100pct-spline`.

## ⚠️ Version: Havok **6.5.0**, not 5.5

Confirmed from the exe: `Havok-6.5.0-r1`, build path `d:\Projects\WildStar\Main\code\Havok_65\`
(see [`../../data/havok_version_evidence.txt`](../../data/havok_version_evidence.txt)). Mercenaries 2 is
Havok **5.5** (HK550). Consequences:

- Compressed-animation class names differ:
  - Saboteur (6.5): `hkaWaveletCompressedAnimation`, `hkaSplineCompressedAnimation`,
    `hkaDeltaCompressedAnimation`, `hkaInterleavedUncompressedAnimation`
  - Mercs 2 (5.5): `hkaWaveletSkeletalAnimation`, `hkaDeltaCompressedSkeletalAnimation`, …
- Saboteur adds **spline-compressed** animation (`hkaSplineCompressedAnimation`), which Mercs 2 didn't
  emphasize. Spline compression is Havok's dominant format in the 6.x era.
- **Struct offsets and quantization layout changed 5.5 → 6.5.** Our numerically-verified Mercs 2
  wavelet decoder is **not** byte-compatible here. What transfers is the *algorithm* (inverse-Haar
  wavelet lifting, static-mask DOF classification, quantization-format offset/scale/bitwidth), not the
  field layout.

## `AP0L` animation pack (from SaboteurToolset `animpack/anim_extract.cpp`)

`animations.pack` (magic `AP0L`) holds named block types, then one concatenated HKX blob:

| Block | Magic | Offset (retail PC) | Content | Status |
|---|---|---|---|---|
| `ANIM` | MINA | `0x000004` | clip metadata: duration, bone list (hashes), flags, `streamed` flag, `id` | ✅ `tools/sab_animmeta` |
| *(HKX blob)* | — | `0x0DECE1..0x8EE7E1` | 2214 `hkaSplineCompressedAnimation` clips | ✅ `tools/sab_havok65` |
| `INTV` | VTNI | `0x8EE7E5` | uninterruptible intervals | ✅ byte-exact |
| `SEQC` | CQES | `0x8EEB21` | 2506 animation **sequences** (instruction lists) | ✅ byte-exact |
| `TRAN` | NART | `0x92F033` | transitions (embedded sequences) | ✅ byte-exact |
| `EDGE` | EGDE | `0x942256` | edge trans-anims + distances (fixed blobs) | ✅ byte-exact |
| `BANK` | KNAB | `0x947156` | 80 animation **banks** (override tables, parent chain) | ✅ byte-exact |
| `ADD1` | 1DDA | `0x94E273` | 25 additive→base pairs | ✅ byte-exact |
| `ALPH` | HPLA | `0x94E3A7` | — | not decoded |
| `SSP0` | 0PSS | `0x954027` | streamed-animation offsets/sizes | not decoded |

After the metadata: `u32 numAnims; u32 hkSize;` then `hkSize` bytes written as one `animations.hkx`.

**★`ANIM.id == pandemic_hash(name)` for 3463/3463 records** — the clip id *is* the name hash. That is
the key `PclAnimationManager::GetAnimation(PblCRC)` → `PblHashTable::Find(u32)` looks up, and what makes
every cross-reference below possible. `pandemic_hash` is **case-folded**, so `ph("saboteur") ==
ph("Saboteur")` — bank and template names are indistinguishable by hash.

## ✅ SOLVED — metadata blocks + the character→animation binding

**How does the engine know which clips a character can play?** It does **not** read a per-character clip
list — none exists in shipped data (proven: no anim-set field on any AULB character template, by
value-space scan of all 644,154 pairs; Lua `AnimGroup`s are level-streaming residency only). **The state
machine is hardcoded C++** — ~40 `WSHumanState*` classes (`StateJump`, `StateCover`, `StateMelee`,
`StateClamber`, …). These blocks are the **lookup tables that code consults**, not a graph that owns
characters.

The deterministic chain (every link from symbols in `game-files/symbols/WildStar_d.map`):

```
WSHumanBlueprint::GetAnimBank()  -> PblCRC          # blueprint property; Sean's literal = "saboteur"
WSHuman::GetWeaponAnimBank()     -> "pistol" | "fists" | "fistblock" | "grenade_charge" | …
WSHumanAnimator::CalcAnimBank() -> CalcComboBank()  # sprintf("%s_%s", charBank, weaponBank)
                                                    #   -> pandemic_hash -> PclAnimationManager::GetBank()
                                                    #   -> if absent, FALL BACK to the raw weapon bank
PclAnimator::SetAnimationBank(PclAnimationBank*)
WSHumanAnimator::SetSequence(PblCRC)                # the CODE names the sequence, by CRC
PclAnimator::AnimationItem(PclAnimationSequence*, …)
    crc = seq->GetAnimation(i)                      # returns a CRC, not a pointer
    bank->OverrideAnimation(crc, bank)              # child-first remap up the parent chain
    mgr->GetAnimation(crc) -> PblHashTable::Find
```

The `"%s_%s"` / `"female%s"` / `"%s_lowWTF"` format strings sit in the `WSHumanAnimator.cpp` string pool
(file `0x71d68`). `"nazi_pistol"` appears **0×** as a literal — it is composed at runtime. The only
combos that resolve are nazi × {pistol, rifle, submach, machinegun, bazooka} = exactly the five `nazi_*`
banks. **No `saboteur_*` combo exists, so Sean always falls back to the raw weapon bank.**

### `BANK` (`KNAB`) — `PclAnimationBank`, 80 banks

```c
"KNAB" u32 bankCount;              // 80
bank[bankCount] {
  u32 nameHash;                    // pandemic_hash(name)
  u16 nameLen;                     // INCLUDES the NUL
  char name[nameLen];
  u32 parentBankHash;              // 0 = root  -> inheritance chain (SetParentBank)
  u32 entryCount;
  entry[entryCount] {              // SORTED ascending by key -> binary search
      u32 key;                     // generic/logical name hash  ("weapon_MG_aim_idle_low")
      u32 value;                   // character-specific clip id ("weapon_SG_aim_idle_low")
      u32 nExtra; u32 extra[nExtra];   // usually 0; VARIABLE LENGTH
  }
}
```
= `PclAnimationBank::AddOverride(PblCRC generic, PblCRC specific)` + `SetParentBank`.
Validated **80/80** name hashes, **80/80** parents resolve, **2087/2087** values are real `ANIM` ids,
keys ascending in every bank, walk lands exactly on `ADD1`.

**A bank is an OVERRIDE/remap table, not a clip list** — it records only where a character *differs*.
Child-first resolution proven: `shotgun` (parent `rifle`) shares 48 keys; 47 map identically and exactly
one differs (`weapon_MG_aim_idle_low` → SG vs RFL), so the child must win.

> **★Why `saboteur` (Sean's bank) is EMPTY (0 entries, no parent).** The generic vocabulary is written in
> **"MG" terms** — banks remap `weapon_MG_aim_walk_L_LF` → `Weapon_HG_aim_walk_L_lf`. **Sean *is* the base
> rig**, so his bank is the identity and every generic CRC passes straight through. This is why a
> rig-compatibility test (`bone_repr=="index" && subset_of_skeleton`, 2155/2214 pass) makes *every*
> humanoid clip look like Sean's.

Bank families: character/archetype (`saboteur`, `nazi`, `nazi_softened`, `nazi_drunk`, `FemaleNazi`,
`civilian`, `femaleCivilian`, `Skylar`, `BelleDorissGirl`), civilian variants (`civilian_flee{,2,3}`,
`civilian_umbrella{,_rain}`, `femalecivilian_bag`, `civilian_fishing`, …), Sean conversation idles
(`ConvIdle_Sean_Base`, `convIdle_Sean_{BaseL,BaseR,Crossed,CrossedL,CrossedR}` + `_Male_`/`_Nazi_`/
`_Female_` families), and weapon banks (`fists`, `fistblock`, `rifle`, `shotgun`, `pistol`, `machinegun`,
`submach`, `bazooka`, `grenade_charge`, `nazi_*`).

Weapon banks are **player** banks via `WSHuman::GetWeaponAnimBank()` (a second overlay) — do not treat
them as NPC-only; `nazi_rifle` etc. *are* nazi-only. The one template→bank link in shipped data: property
`0x29c2a39c` on **`Weapon` templates only** (92 of them, **0** on `Human`/`Player`) → `rifle`,
`machinegun`, `pistol`, `bazooka`, `submach`.

### `SEQC` (`CQES`) — 2506 sequences, **variable-length, byte-packed/unaligned**

```c
"CQES" u32 seqCount;               // 2506
seq[seqCount] {
  u32 nameHash;                    // SetName;  seqId == pandemic_hash(name)  (134/134 verified)
  u32 numInstructions;             // SetNumInstructions   (NOT a type field)
  instr[numInstructions] {
      u32 f0;                      // InstructionType opcode; the terminator instr's f0 == seq `type`
      s32 f4;                      // SIGNED (Read32), usually -1
      u32 numAnimations; u32 animHash[numAnimations];   // Instruction::AddAnimation
      u32 numTags;       u32 tagHash[numTags];          // Instruction::AddTag
  }                                // SetInstruction(i,&instr);  in-memory stride 0x30
  f32 animationLength;             // SetAnimationLength
  f32 movement[3];                 // SetAnimationMovement (PblVector3, ReadData 12)
  f32 animationRotation;           // SetAnimationRotation
  u32 type;                        // SetType(InstructionType)
  f32 aimMinMax[4];                // GetAimMinMax (ReadData 16)
  s8  numAimFrames[3];             // GetNumAimFrames (ReadData 3, char[3])
  PblVector2 aimFrames[i][numAimFrames[i]];   // 3 arrays, 8 B per entry
  u8  turnPositive;                // SetTurnPositive  -> seq+0x54 bit0
  u8  synchronized;                // SetSynchronized  -> seq+0x54 bit1
}
```
Records are **not** fixed-size — `CORNER_IDLE_LEFT` happens to be 93 B, `civ_drink_wine` is 149 B.
Verified: all 2506 parse and land exactly on `TRAN` @ `0x92F033`.

### `TRAN` / `EDGE` / `INTV` / `ADD1`

```c
// TRAN — the embedded sequence is conditional and always SetNumInstructions(1)
"NART" u32 containerCount;
container[] { u32 hash; u32 numTransitions;
  transition[] { u32 f0..f6; u32 seqNameHash;
                 if (seqNameHash) { u32 animHash; u32 tagHash; <SEQC tail from animationLength on> }
                 u32 g0,g1,g2; u16 nameLen; char name[nameLen]; } }

// EDGE — fixed-size bulk ReadData into PclEdgeTransData (4+18720+1400+4+96 = 0x4F00 = exact gap)
"EGDE" u8 transAnims[18720]; u8 unk[1400]; "TSID" u8 distances[96];
//   transAnims = PclEdgeTransAnims::{GetStartAnim,GetMoveAnim,GetEndAnim} triples (u32×3, 1685, 149 distinct)

// INTV — 49/49 records, 828/828 bytes; 58/58 payload u32s are clip ids
"VTNI" u32 count;                  // 49
interval[] { u32 tagCRC;           // 0x501b960d == ph("uninterruptible")  (PclAnimation::AddUnInterruptibleInterval)
             u16 a;                // always 0
             u16 b;                // ∈ {5,6,10,15,20,30,40,100}
             u32 numAnims; u32 animHash[numAnims]; }

// ADD1 — 25 records, 308/308 bytes (PclAnimation::GetBaseAnimForAdditive)
"1DDA" u32 count;                  // 25
rec[] { u32 additiveAnim; u32 baseAnim; u32 flags; }
```

### Which clips belong to a character

Cross-referencing every u32 window against the 3463 clip ids and 80 bank hashes (chance per window:
clip 8.1e-5 %, bank 1.9e-6 %):

| Block | windows | clip-id hits | bank-hash hits |
|---|---|---|---|
| `SEQC` | 263,439 | 4396 | **0** |
| `EDGE` | 20,221 | 38 | **0** |
| `INTV` | 825 | 58 | **0** |
| `TRAN` | 78,368 | 344 | 4 → `rifle, machinegun, pistol, submach` |

`SEQC`/`EDGE`/`INTV` are **character-agnostic**. Exact partition of all clips:
**2522 (`SEQC`) + 879 (`BANK` values only) + 62 (`TRAN` only) = 3463, zero uncovered.**

**Sean (`saboteur`): 3115 / 3463 reachable** across all his banks (`saboteur` + 6× `ConvIdle_Sean_*` +
weapon banks); **2584 unarmed** (= 2522 + 62). The 348 excluded are reachable only through other
characters' banks: `civ_` 178, `nazi_` 110, `shrd_` 15, `sky_` 7.

> **⚠️ 3115 is a DATA-level upper bound**, not the final answer. It assumes Sean's code may play all 2506
> sequences, so it still contains ~273 `nazi_*` clips reachable via the shared `pistol`/`rifle` banks
> through AI cover sequences his code never selects. The residual narrowing is a **code** property —
> which `WSHumanState*` calls `SetSequence(CRC)` — and cannot be resolved from the pack. Now that `SEQC`
> is exact, that chain (`WSHumanState*` → `SetSequence` → `seq->GetAnimation(i)` → clips) is closed and
> tractable by disassembly.

### ⚠️ Two traps that will waste your time

1. **`LoadSequences`/`LoadBanks`/`LoadTransitions`/`LoadAnimIntervals`/`LoadAdditives`/`LoadEdge*` do NOT
   parse this pack.** They open a **`PblParser`** — a *text* parser (`ReadChar`/`PeekChar`/`ReadToken`/
   `GetFileText`/`SkipLine`) for the tab-separated `AnimText/*.txt` sources. The binary reader is
   **`PclAnimationManager::LoadAnimPackFile`**, one huge function using `PblFile::Read32u/Read32/Read16/
   Read8/ReadFloat` + `PblDiscFile::ReadData`, which calls every setter named above.
2. **Use the retail PC x86 exe for layout.** `game-files/symbols/Saboteur.exe` is an **XEX2 (Xbox 360)**
   (its PDB uses `QAA`/`__cdecl` mangling), and all three prototype binaries are Xenon PPC. The **2008
   prototype is an OLDER format version** — magic `AP04`, block `ADD0`, BANK entries only 8 B with **no
   `nExtra`/`extra[]`** — and **cannot** yield the retail layout. Correct target:
   `C:/GOG Games/The Saboteur/Saboteur.exe` (imgbase `0x400000`), `LoadAnimPackFile` @ **VA `0x00e2f810`
   / file `0xa2ea10`** *(file offset corrected 2026-07-24; `0x00e2f810 - 0x400E00 = 0xA2EA10`, where the
   `6a ff 68 … 81 ec 94 06 00 00` SEH prologue actually starts — the old `0xa2e410` landed mid-function)*. Use the symbol-rich PPC debug build (`WildStar_d.exe`/`.map`) only to *name*
   fields.

### Not decoded

`ALPH`, `SSP0`, `ANMA`; instruction `f0`/`f4` names (public members, no setter symbols — `Instruction`
exposes only `Add/Remove/HasAnimation/Tag`); `TRAN` `f0..f6`/`g0..g2` names (retail inlines the setters);
`INTV` `a`/`b` (→ `PclAnimInterval+0x0c/+0x0e`, almost certainly start/end frame, unproven); `ADD1`'s 3rd
u32 (`AdditiveAnimNamePair` implies only 2 hashes); `EDGE` internals (the 18720/1400/96 splits need
`PclEdgeTransData` field analysis); `INTV` tag `0xe6b5c7cd` (no preimage — these CRCs are compile-time
constants, and even `"uninterruptible"` itself is absent from the shipped binaries).

## ✅ SOLVED — the validated spline decode

The community blocker (SaboteurToolset: *"no way … to convert extracted hkx files because of separated
metadata"*) is resolved. The whole corpus is `hkaSplineCompressedAnimation` (see the update banner
above), a documented Havok format. It was reverse-engineered by a **double-blind protocol** — two
investigators independently derived and empirically decoded the format (each decoding the entire real
`Animations.pack`, 0 failures), then a third agent adjudicated the two points their validation could not
distinguish, resolving them by **disassembling `Saboteur.exe` directly** (the THREECOMP40 unpacker
`FUN_00f22470` is FPU-heavy and absent from the Ghidra decomp). Reference decoder: `tools/sab_havok65`
(std-only Rust; `cargo run --release -- "…/Animations.pack" all` → 2214/2214 clean).

### AP0L → HKX carve
`u32 numAnims; u32 hkSize;` immediately precede the first Havok packfile magic `57 e0 e0 57`; the blob is
`hkSize` bytes. Retail main blob: magic at file `0xDECE1`, `numAnims=2214`, `hkSize=0x80FB00`; self-check
`0xDECE1 + hkSize` lands exactly on the packfile's declared end. (The pack holds **7,495** `57 e0 e0 57` magics in total = this 1 main blob + **7,494** further packfiles,
which are per-clip *streamed* sub-animations; the `hkaSplineCompressedAnimation` class-string count
9,709 = **2,215** inside the main blob + one per streamed packfile (7,494).) *(corrected 2026-07-24:
both terms were previously off by one — "7,495 further" and "2,214 +". The 9,709 total was right, and
the 2,214 animation **clips** figure is a separate, correct claim: the main blob carries 2,214 clips but
2,215 copies of the class string.)*

### Havok 6.5 packfile (LE, 32-bit)
Header: magic `0x57E0E057 0x10C0C010`, fileVersion 6, layoutRules `04 01 00 01` (4-byte pointers, LE),
3 sections `__classnames__`/`__types__`/`__data__`. Section header = **20-byte tag + 7×u32**
`[absDataStart, localFixups, globalFixups, virtualFixups, exports, imports, end]` (section-relative).
`data_pk = body0 + classnames.end + types.end`; `__data__` absDataStart `0x160`. Local fixups (8-byte
`src,dst`) relocate hkArray pointers; virtual fixups (12-byte `src,sec,classNameOff`) bind objects → class.
Structurally identical to the Mercs 2 (5.5) walker.

### `hkaSplineCompressedAnimation` struct (runtime offsets)
From ctor `FUN_00eb5de0` / dtor `FUN_00eb7740`, confirmed against real bytes:

| Off | Field | | Off | Field |
|---|---|---|---|---|
| 0x08 | type = 5 (spline) | | 0x34 | blockDuration (f32) |
| 0x0C | duration (f32) | | 0x38 | blockInverseDuration (f32) |
| 0x10 | numTransformTracks | | 0x3C | frameDuration (f32) |
| 0x14 | numFloatTracks | | 0x40 | blockOffsets hkArray |
| 0x24 | numFrames | | 0x4C | floatBlockOffsets hkArray |
| 0x28 | numBlocks | | 0x58 | transformOffsets hkArray (empty in corpus) |
| 0x2C | maxFramesPerBlock | | 0x64 | floatOffsets hkArray |
| 0x30 | **maskAndQuantizationSize** | | 0x70 | data hkArray (u8) |

hkArray = `{ptr(local-fixup), i32 size, u32 capFlags}`. Self-consistent for all 2214 clips:
`frameDuration == duration/(numFrames−1)` and `blockDuration == (maxFramesPerBlock−1)·frameDuration`.

### Block + per-track decode
Per block (`data + blockOffsets[b]`): `numTracks` × 4-byte mask `[ctrl, transMask, rotMask, scaleMask]`,
then per-track spline data starting at **`blockStart + maskAndQuantizationSize`** (0x30 — *not* `numTracks·4`;
it is padded for large skeletons). `transformOffsets` is empty in the corpus ⇒ tracks are parsed
**sequentially**. Control byte: `transType = ctrl&3`, `rotType = (ctrl>>2)&0xf`, `scaleType = ctrl>>6`.
Channel decode order and output slots: **translation@0x00, rotation@0x10, scale@0x20**, each 4-byte aligned.
(The corpus is uniformly `ctrl = 0x45` → all type 1.)

- **Translation / scale** (`FUN_00eb73a0`): per component — static bit ⇒ one f32; spline bit ⇒ f32 `min,max`
  + quantized control points (8-bit ÷255 or 16-bit ÷65535), value `= min + q·(max−min)`.
- **Rotation** = **THREECOMP40** (5 bytes/CP): 40-bit LE word; three 12-bit small comps at bits
  `[0:12)/[12:24)/[24:36)`, each `value = (raw − 2047)/(2047·√2)`; **bits 36-37 = index of the omitted
  (largest) component; bit 38 = its sign; bit 39 unused**; `largest = ±√(1 − Σsmall²)`. (The sign bit and
  the exact dequant were the adjudicated corrections — unit-norm alone cannot catch a wrong sign.)
- **NURBS**: `u16 numItems(=numCP−1); u8 degree; u8 knots[numItems+degree+2]` (clamped B-spline, de Boor).

### Sampling at arbitrary time `t` (faithful to `FUN_00eb8120`)
```
t          = clamp(t, 0, duration)
blockIndex = clamp(trunc(t · blockInverseDuration), 0, numBlocks−1)
blockTime  = t − blockIndex · blockDuration                       # block-local SECONDS
frameInBlk = trunc(blockTime · blockInverseDuration · (maxFramesPerBlock−1))  # integer, findSpan only
span       = findSpan(frameInBlk, degree, rawByteKnots)           # INTEGER domain
knots[i]   = rawByteKnots[i] · frameDuration                      # scaled to seconds
value      = deBoor(u = blockTime, degree, knots, controlPoints)  # CONTINUOUS seconds
```

### Verified vs open
- **CONFIRMED** (whole corpus, 2214 clips / 133,531 tracks, 0 failures / 0 non-unit quats): everything above.
- **Open** (corpus is uniform `ctrl=0x45`, single-block): the **multi-block** blend path, rotation quant
  types **0/2/3/4/5** (POLAR32/THREECOMP48/24/STRAIGHT16/UNCOMPRESSED), and the **16-bit** translation/scale
  path — structurally present, decoders identified by table+size, but not exercised by this pack. Close via
  out-of-corpus assets or a live x32dbg capture of `FUN_00eb7e00`. See memory `havok65-spline-decode`.

### Layered validation — bytes vs sampler

There are two validation layers around the decoder, and it's worth being
explicit about what each proves and what neither does:

1. **Packfile bytes** — `sab_havok65 validate-hct <pack> <retail_anim.xml>`
   (`src/hct_validate.rs`). AssetCc2 6.5 emits an XML view of every
   `hkaSplineCompressedAnimation`; the harness pairs each XML clip to its
   packfile counterpart by fingerprint and diffs every scalar (int-exact),
   every offset array (int-exact), and the whole `data` blob (byte-exact).
   **All 2214 clips clean.** This proves our packfile reader and the block
   layout offsets are exactly what HCT sees.

2. **Sampler math** — `sab_havok65 validate-trs <pack>`
   (`src/trs_validate.rs`). Exercises the per-frame path (block indexing,
   `findSpan`, THREECOMP40 unpack, Cox-de Boor blend, hemisphere flip,
   post-blend unit-norm) via **four mathematical/spec properties** that
   must hold on any correct implementation:
   - Unit quaternion + finite over every sampled frame.
   - Static-track invariance: a track with no spline bits returns the
     same T/R/S at every t.
   - **Convex-hull confinement for T + S:** for every t, each sampled
     component lies inside `[min, max]` over the (p+1) local control
     points at the corresponding knot span — a hard property of Cox-de
     Boor. Rotation has the same test but with a 5 % widen to absorb the
     post-blend renormalization.
   - **Clamped-knot endpoint match:** where the knot vector is clamped
     (the HCT-emitted pattern), Cox-de Boor at u=U[p] equals cps[0] and
     at u=U[n+1] equals cps[n]. The sampler must reproduce that
     endpoint at t=0 and at the last within-block frame; the reference
     side unpacks the endpoint CP through an independent THREECOMP40
     implementation (`unpack_quat_ref` in `trs_validate.rs`).

   Corpus results (1,172,891 frame-samples across 2,214 clips):

   | property                                            | max deviation | violations                |
   |-----------------------------------------------------|---------------|---------------------------|
   | unit quaternion (`‖q‖ − 1`)                         | 1.19e-7       | 0 / 1,172,891             |
   | non-finite T/R/S components                         | —             | 0                         |
   | static-track invariance                             | 1.79e-7       | 0                         |
   | convex-hull confinement, **T + S (strict)**         | 0             | 0 / 137,868               |
   | convex-hull confinement, all channels (rot widened) | 1.75e-3       | 129 / 2,302,400 (soft)    |
   | clamped-knot endpoint match                         | 2.38e-7       | 0 / 130,724               |
   | `sample_at` determinism (2×)                        | 0             | 0                         |

   The 129 soft rotation-hull overshoots are all from the sampler's
   post-blend renormalization scaling a component slightly outside its
   raw hull (a mathematical consequence of dividing by `‖q‖` when `‖q‖ <
   1`), never above 1.75e-3 — well below HCT's own rotation
   re-quantization budget of `0.001` for encoding (memory
   `[[havok-anim-toolchain-roundtrip-proven]]`; the encode step is the
   tighter bound). The 129 checks are counted separately from the
   strict T+S check, which stays at zero.

   The endpoint match is the strongest single test in the file: 130,724
   independent point-checks against an alternate-code-path THREECOMP40
   implementation, all inside 2.4e-7. It exercises the block index, the
   knot-span search, the CP indexing, the quaternion unpacker, the
   hemisphere flip, and the boundary behaviour of Cox-de Boor in one
   check per clamped spline track per endpoint.

### Not covered by either layer

An **external per-frame TRS oracle** was investigated for retail clips
and neither route yielded one:

- `hkxcmd` (community sampler) is not on disk in
  `C:/Program Files (x86)/Havok/`, `C:/Users/Shadow/Downloads/`, or the
  sibling `notes-on-the-released-game/tools/`. The Mercs 2 Havok work
  is 5.5 collision (no anim sampler).
- HCT 6.5's authoring pipeline in `hctFilterAnimation.dll` is one-way.
  Every shipped filter (`CreateAnimations`, `SplineCompressionFilter`,
  `WaveletCompressionFilter`, `DeltaCompressionFilter`,
  `ExtractMotionFilter`, `LoopAnimationFilter`,
  `RotateAnimationFilter`, `FootstepAnalysisFilter`,
  `CreateAdditiveAnimationFilter`) operates from interleaved *toward*
  compressed; none of them decompresses. `AssetCc2 --rulesRULES`
  handles cross-version packfile compat only.
- Havok SDK PC 6.5.0-r1 archive at
  `C:/Users/Shadow/Downloads/havok-sdk-pc-6.5.0-r1.7z` ships headers
  and .inl only (0 .lib files), so a custom filter DLL or a
  standalone caller of `hkaSplineCompressedAnimation::sampleTracks()`
  cannot be built.

So absolute correctness of a **mid-span** sample against an external
authority is unproven. The property tests bracket the answer (convex
hull, endpoint identity, unit norm) and the retail runtime has been
its own oracle at build/preview time on every rig it plays back with
`sab_workshop`. See memory `[[havok65-trs-oracle-gap]]`.

## Previewing — glTF export + the live workshop (skeleton gap CLOSED)

`sab_havok65` exports any clip to a self-contained binary **glTF** (`.glb`):
`sab_havok65 "…/Animations.pack" gltf <index> out.glb`. No coordinate conversion — Havok and glTF are
both RH, +Y-up, metres, quaternion `(x,y,z,w)`, so the decoded transforms export verbatim. That export
is **skeleton-less** (flat nodes) because the animation pack has **no skeleton** — `hkaSkeleton` /
`hkaAnimationBinding` counts are 0; each track becomes its own node driven by its local TRS.

**The phase-2 skeleton prerequisite is now done.** `tools/sab_mesh` reads the character `MSHA`/`MESH`
skeleton out of `Dynamic0.megapack` (parent indices, bind pose, bone name-hashes matching the AP0L `ANIM`
track hashes) — see [`mesh_geometry.md`](mesh_geometry.md). **`tools/sab_workshop`** is the live
wgpu+egui previewer: it re-parents the decoded per-track TRS onto the real hierarchy and plays clips with
GPU skinning on the textured character (DTEX resolved in-process from the megapack), with a browser that
loads any skinned model from the pack. No decode change was needed, exactly as predicted above.

Two things that bite when previewing, both handled in `sab_workshop`:

- **Root motion.** Clips animate the root (`GlobalSRT`) travelling metres — the game applies that to the
  **entity**, not the mesh. In a fixed-camera previewer it simply walks the character out of frame and
  *looks* like a broken decode (it is not). `sab_workshop` keeps the root's bind translation by default
  (`lock root`), honouring animated rotation/scale, so clips play in place.
- **Rigid accessories.** Hats/props are UNSKINNED geometry authored at the origin, parented via the MESH
  drawcall's `parentBone` (e.g. Sean's hat → bone 18 `Bone_Head`). Dropping that field parks them at the
  world origin. See [`mesh_geometry.md`](mesh_geometry.md) (SMSH v2 carries `parent_bone`).

**Still open for the previewer:** filtering the clip list to a character. A rig-compatibility test is
*not* the answer (2155/2214 pass — see the BANK section). The deterministic answer is bank + sequence
reachability (Sean = 3115/3463 at the data level), with the exact set gated on the hardcoded
`WSHumanState*` classes.
