# 2008 Xbox 360 prototype — animation format

**Scope:** what the animation asset chain looks like on the 2008-05 Xbox 360 devkit build
(`game-files/Bin360/`), and how it differs from retail. Written as a companion to
`tools/sab_workshop/src/sources/prototype2008_anim.rs`, which enumerates the catalog and
parses the shared header today.

**Status:** header + catalog + one-clip round-trip **proven** on the real dump; per-frame
sample codec (WAV0 wavelet / DEL0 delta / NOC0 uncompressed) **not yet decoded**. See
"Open work" at the bottom for what a decoder needs to attack next.

---

## Retail vs. 2008 at a glance

| Aspect            | Retail PC (`AP0L`)              | 2008 Xbox 360 (`AP04ANIM`)          |
|-------------------|---------------------------------|-------------------------------------|
| Pack magic        | `L0PA` `MINA` (LE-swapped)      | `AP04ANIM` (natural / big-endian)   |
| Endianness        | little-endian                   | big-endian (Xenon / PowerPC)        |
| Middleware        | Havok 6.5.0-r1                  | Havok 4.6 (per `WildStar_d.map`)    |
| Clip container    | one big Havok packfile blob     | 1605 `ANM0*0` records + loose files |
| Compression       | `hkaSplineCompressedAnimation`  | `WAV0` / `DEL0` / `NOC0` (Pandemic) |
| Names             | `ANIM` block, per-record        | filename of the loose `.hkx` twin   |
| Bone binding      | `ANIM` per-track bone list      | not surfaced here (in FSM/PclAnimationBank) |
| Havok magic       | `57 E0 E0 57` present in blob   | **absent** — no Havok packfile      |
| `hka*` classnames | present (RTTI + registration)   | **absent** — no reflection strings  |

The header magic collision (`AP04ANIM` vs `AP0LANIM`) had the two look similar-shaped from
a distance. They are **not** — same Pandemic team tagged both AP-flavoured, but the on-disk
structure and the referenced middleware diverged wholesale.

---

## `Bin360/Animations.pack` — the outer container

```text
offset  size  meaning
0x00    8     magic  "AP04ANIM"           (natural ASCII; NOT byte-reversed as retail)
0x08    4    u32-BE  record count         (== 1605 on the shipped dump)
0x0C    ?    ??      outer per-record header (only partially reversed)
...          per-record body starts with an ANM0*0 magic (see below)
```

**The outer per-record header is only partially reversed.** A byte-scan for the ANM0
magic finds exactly 1605 record starts — matching the declared count byte-for-byte — so
enumeration is reliable without needing the outer header. But the layout of the ~12 bytes
between records (which look like some mix of a hash, a size, and a variant tag) has not
yet been decomposed cleanly enough to trust for record traversal from a directory table.

**Chosen strategy:** magic-scan for `ANM0(WAV|DEL|NOC)0` byte-by-byte, then read each
record's body starting at its magic. Slower than a directory walk, but hits every real
record and cannot mistake in-body bytes for a record start (the 8-byte magics are
distinctive enough that no false positive has been observed across the 15 MB pack).

See `scan_pack_records` in `prototype2008_anim.rs` for the implementation.

Pack-only records currently have no plaintext name — the outer header doesn't obviously
carry one, and the plaintext-name inventory sits in the loose-file directory instead. A
future pass can content-match a pack record's ANM0 body against the loose files' bodies
(a chunk of the middle of each is often byte-identical for the sampled overlap) to wire
up a name.

---

## `ANM0*0` record — the per-clip container

Every clip — whether packed inside `Animations.pack` or loose as a `.hkx` file — starts
with one of three magics and shares a 24-byte fixed header:

```text
offset  size  meaning
0x00    8     magic         "ANM0WAV0" | "ANM0DEL0" | "ANM0NOC0"
0x08    4     f32-BE  duration_s      the clip's playback duration in seconds
0x0C    4     u32-BE  num_tracks      number of transform tracks (0..~64 typical)
0x10    4     u32-BE  aux_count       codec-specific — wavelet: block count;
                                      delta: frame count (== round(dur * 30fps));
                                      NOC0: 0 for pose files
0x14    4     u32-BE  stride16        always 16 in the sampled WAV0 + DEL0 corpus
0x18    ...   codec-specific body
```

**The three codecs match Havok-4.6's skeletal-animation compression classes** — this is
the same pre-6.x compression family the Mercs 2 codebase decoded (`hkaWaveletCompressed`,
`hkaDeltaCompressed`, `hkaInterleavedUncompressed`). See
`docs/mercs2-pdb-analysis/animation-skeleton.md` for the class-name tree.

**All scalar fields are big-endian**, in keeping with the Xenon binary. The static-header
plausibility gate in `Anim2008Header::parse` catches endian mistakes early: an accidental
LE read of the duration slot yields a NaN/infinity float in the general case, which fails
the `duration.is_finite()` check.

### Header field ranges — sampled corpus (`Bin360/animations/`)

Random sample of 12 files, mixed codecs:

```text
name                                  codec  size    dur     tracks  aux   stride
weapon_bazooka_turn_180_R.hkx         WAV0   2657    0.434    52      14     16
idle_loop8_rf.hkx                     WAV0   15549   2.700    58      82     16
sky_idle_1.hkx                        WAV0   4777    2.669    37      81     16
knee_to_idle.hkx                      WAV0   4329    0.733    58      12     16
Ledge_Backjump_prep_loop.hkx          NOC0   ...     ...      ...     ...    ...
conv_F_backpoint2.hkx                 WAV0   8278    2.436    46      74     16
mach_run_bk_rdy_Loop1_RF.hkx          WAV0   5163    0.400    57      13     16
ledge_hang_move_right_end.hkx         WAV0   5253    1.068    55      33     16
jump_run_loop_rf.hkx                  WAV0   1770    1.001    55      31     16
swim_dive_idle.hkx                    WAV0   1822    0.467    52      15     16
weapon_2H_LG_swing.hkx                WAV0   6750    1.335    54      41     16
dart_sitdwn_back.hkx                  WAV0   6358    0.533    55      17     16
aurora_dead_idle_drvr.hkx             NOC0   2728    0.000     0      52     ...
aurora_dead_idle_pass.hkx             NOC0   2728    0.000     0      52     ...
back_prone_pose.hkx                   NOC0   8808    0.133    36     180     ...
cine_A1M1_S1_Dierker.hkx              DEL0   222561  75.467   24    2265      8
cine_A1M1_S1_Jules.hkx                DEL0   29203   75.467   30    2265      8
cine_A1M1_S1_Sean.hkx                 DEL0   135269  75.467   28    2265      8
```

Observations:

* **WAV0**: track counts cluster 24–64 (typical human/humanoid rig). `aux_count` scales
  with duration but not linearly — a wavelet clip's block count depends on both the
  frame count and the sparsity of the compressed coefficients.
* **DEL0**: cinematic clips only. Track counts are small (a handful of facial /
  cinematography channels), frame counts are big (`aux_count == 75.467 s × 30 fps = 2264`,
  off-by-one from the header's 2265 — the "+1" is standard first-frame framing).
* **NOC0**: static poses (`aurora_dead_idle_*`, `back_prone_pose`). Duration is often 0;
  `num_tracks` reads as 0 for the pure-pose files, with the "track count equivalent"
  living in the `aux_count` slot. A codec-aware NOC0 reader will need to swap the
  interpretation of these two fields.

### Loose-tree codec breakdown

Counted across `Bin360/animations/` (the primary animation directory):

```text
ANM0WAV0  3090   (86%)   — wavelet compressed, the workhorse
ANM0DEL0   282    (8%)   — delta compressed, mostly cinematics
ANM0NOC0   214    (6%)   — uncompressed / static poses
------
total     3586
```

Plus 33 more in `ObjectAnimations/` and 1 in `TestAnims/`. Some directories
(`AnimationTest/`, `SultanOfSwing/`) contain packed subassets, not loose `.hkx` files.

---

## Directory layout

Where the assets live on disk (`game-files/Bin360/`):

```text
Animations.pack                 — the curated pack, 15 MB, 1605 records
animations/                     — the loose authoring tree, 3586 .hkx files
ObjectAnimations/               — object-attached anims, 33 .hkx
TestAnims/                      — QA fixtures, 1 .hkx
AnimText/                       — human-readable metadata files:
    AnimEvents.txt              — per-clip event track (footsteps, weapon fires)
    AnimIntervals.txt           — sub-ranges within a clip
    AnimMasks.txt               — bone-mask filters
    AnimOffset.txt              — per-clip root-motion offset
    AnimSettings.txt            — hand/foot lock ranges, IK toggles
    AdditiveAnims_v1.txt        — additive-blend clip pairs
    EdgeTransitions_v5..v10.txt — ledge-navigation transition graph
    MeleeCombos_v2.txt          — melee combo tables
    MeleeReactions_v1..v3.txt   — reaction-anim picker
    StaticPoses.txt             — pose asset ids
    SeqTrans                    — sequence transition manifest
    meleeattacks_v15.txt        — attack-animation registry
    meleestates_v6..v9.txt      — melee state machine data
```

The `AnimText/` files are pipe-separated / tab-separated tables ready to be cross-
referenced with clip names. They are the closest thing this build has to the FSM data
retail bakes into `Animations.pack`.

**Symbol trail:** `WildStar_d.map` places the loaders in `Puckel:PclAnimationManager.obj`
(`d:/projects/wildstar/code/puckel/`):

```text
LoadAnimFile         @ 0x83197E50   (opens one .hkx file)
LoadAnimation        @ 0x8319E288   (name-driven)
LoadAnimation (disk) @ 0x8319E580   (PblDiscFile + PblCRC)
LoadAnimFromPackfile @ 0x8319E6F8   (reads one record from an open pack)
LoadAnimPackFile     @ 0x8319EBF8   (opens Animations.pack itself)
```

Class names (from the same map): `PclAnimation`, `PclAnimationSequence`, `PclAnimationBank`,
`PclAnimator`, `PclAnimEventParam`, `PclCinematicAnimationSequence`, `PclAnimInterval`,
`PclAnimationUserData`, `PclTransitionContainer`. String evidence points at
`d:/projects/wildstar/Havok_46/include/animation/hk...` — the middleware version tag is
Havok 4.6, older than both Mercs 2's 5.5 and retail Saboteur's 6.5.

---

## What `prototype2008_anim.rs` currently provides

1. **`Anim2008Header::parse`** — the 24-byte fixed-header reader.
2. **`Anim2008Catalog::open`** — walks the loose tree (3620 clips) and byte-scans the
   pack (1605 records); returns a `Vec<Clip2008Info>` with per-clip name (loose) or
   synthetic offset key (pack), codec, duration, track count, and source location.
3. **`Anim2008Catalog::load_clip(hash)`** — round-trips a clip by handle: fetches the
   full record bytes and re-parses the header. Ends where a per-frame codec decoder
   would take over.
4. **`GameSource::list_animations`** trait method — implemented by `Prototype2008Source`
   to expose the catalog to `app.rs`'s Nav Anims tab; default `Vec::new()` for retail
   (which uses the existing `AnimCatalog` + `havok` path instead).

Tests in the module:
* `parses_all_three_codec_variants` — synthetic header round-trip for WAV0/DEL0/NOC0.
* `rejects_a_file_that_is_not_an_anm0_record` — magic gate.
* `rejects_an_impossible_duration` — endian sanity gate.
* `catalog_enumerates_loose_and_packed_clips` — real-file scan; asserts >3000 loose +
  ~1605 pack records with plausible names/durations.
* `load_clip_round_trips_a_known_loose_entry` — hash lookup + header re-parse.
* `pack_scan_matches_declared_record_count` — the 1605-from-header vs 1605-from-scan
  agreement.

---

## Per-frame sampling — status

`Clip2008::sample_track(track_idx, time_s)` returns a `SampledTransform { pos, rot, scale }`
per (clip, track, time). Coverage:

| Codec | Header decoded? | Frames decoded? | Notes |
|-------|-----------------|-----------------|-------|
| **NOC0** (interleaved) | YES | **YES** | Full frame-major `hkQsTransform` array + slerp/lerp interpolation. Layout verified byte-for-byte on 214 loose files; every returned quaternion is unit and every scale is ≈identity across the sampled corpus. |
| **WAV0** (wavelet)     | YES (238/238 on the sampled corpus) | Blocked | Extended HK-4.6 derived-struct header parses cleanly (`Wav0Header::parse` in `sources/prototype2008_anim_havok46.rs`) — QuantizationFormat + all section indices + contiguity gates. Havok wavelet math (`wv_entropy_unpack`, `wv_dequant`, `wv_inverse`, `wv_quat_w_sentinel`, `static_mask_dynamic`) ported from the numerically-validated Mercs 2 HK-5.5 decoder. Blocked on `block_size == 16`: every shipped 2008 wavelet clip uses 16-sample blocks, but the ported `wv_inverse` implements only the 8-sample basis. Larger blocks need the un-ported predict/update/deinterleave lifting passes (Havok `FUN_009fe250/9fe180/9fe4b0` in Mercs 2's decomp). |
| **DEL0** (delta)       | YES (282/282 on the sampled corpus) | NO | Extended header parses cleanly (`Del0Header::parse`). The delta bit-stream shape is different from wavelet and is NOT validated in the sibling Mercs 2 workspace either (Mercs 2's decoder stubs delta as well). Returns `Err` with a diagnostic pointing at `samplePartialWithDataChunks` @ 0x82a58cc8. |

**Struct-layout diff HK 4.6 ↔ HK 5.5** (see the module doc of
`sources/prototype2008_anim_havok46.rs` for the full table). Key point: HK 5.5
added `m_numberOfFloatTracks` to `hkaAnimation`, shifting every derived field
down by 4 bytes. Pandemic's 24-byte pre-header substitutes for the 32-byte
`hkReferencedObject` + `hkSkeletalAnimation` base; the derived-struct body
follows immediately, and the data buffer starts at file+0x54 (WAV0) / file+0x5C
(DEL0). Section-index fields are stored as `u16-BE` in the high half of a
4-byte slot (empirically low half is always zero on the corpus — a compact
wire encoding for the runtime `u32` memory field).

### NOC0 on-disk layout

```text
+0x00  8   magic         "ANM0NOC0"
+0x08  4   f32 BE        duration_s
+0x0C  4   u32 BE        num_transform_tracks  (0 for the static-pose special case)
+0x10  4   u32 BE        aux_count             (= eff_tracks × frames)
+0x14  4   u32 BE        stride/aux            (unused by the sampler)
+0x18  N   hkQsTransform aux entries, 48 bytes each, frame-major
                          transforms[f * eff_tracks + t]
                          each 48B = 12 f32 BE: [T.x,T.y,T.z, R.x,R.y,R.z,R.w, S.x,S.y,S.z, pad, pad]
+trail eff_tracks*4      per-track descriptor (pandemic_hash of the driven bone)

  eff_tracks = num_tracks if num_tracks > 0 else aux_count
  frames     = aux_count / eff_tracks    (integer; static-pose = 1)
```

Verified on `aurora_dead_idle_drvr`, `back_prone_pose`, `Bugatti_drive_turn_left`,
`KD_head_front`, `Rope_horiz_down_long_start`, `Ledge_Backjump_prep_loop` — every file
size matches `24 + aux×48 + eff_tracks×4`, and every 12-float transform decodes to a
unit quaternion within 1e-4.

## Open work — WAV0 and DEL0

To play a WAV0/DEL0 clip against a rig, someone still needs to:

1. **Reverse the wavelet + delta codec bodies.** These are Pandemic-derived compression
   schemes that share the Havok-4.6 wavelet/delta compression classes (`hkWaveletSkeletalAnimation`,
   `hkDeltaCompressedSkeletalAnimation` — publics present in `saboteur_pdb_p.txt`). The
   Mercs 2 corpus has a **numerically-verified inverse-Haar wavelet decoder** for HK 5.5
   (`mercs2_formats/src/anim.rs`) — the 4.6 → 5.5 layout is CLOSE but not identical, and
   the outer Pandemic 24-byte header replaces Havok's serialized `hkReferencedObject` +
   `hkaAnimation` base, so the port needs to re-derive the on-disk struct offsets. The
   Saboteur.xex Ghidra decomp names every relevant function (`samplePartialPose`,
   `samplePartialWithDataChunks`, `decompressBlockToCache`, `handleEndian`) — the
   bitstream math is inlined PPC VMX128 that resists automated recovery but reads cleanly
   from disassembly.
2. **Reverse the outer per-record header.** Would let us drop the magic-scan in favour
   of a directory walk, and — more importantly — surface the plaintext name for the
   1605 pack records instead of the synthetic offset key.
3. **Wire the bone binding.** For NOC0 the per-track u32 trailer gives us track→bone
   hashes directly (via `Clip2008::track_descriptors_noc0`); a rig hash-name table
   resolves those to bone names. WAV0/DEL0 likely have the same trailer layout — worth
   confirming once the codec bodies are decoded.
4. **Content-match loose → pack.** Even without solving (2), a name→pack-offset lookup
   is achievable by hashing the ANM0 body bytes of each loose file and searching the
   pack for the same window. Some loose files are byte-identical to pack records; some
   are earlier revisions. A hash-match would surface plaintext names for the loose-
   matching subset (empirically ~40% based on a random sample).
