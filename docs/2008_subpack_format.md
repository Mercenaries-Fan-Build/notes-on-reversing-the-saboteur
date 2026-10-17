# 2008 prototype containers: `MP00`, `SBL*`, slot directories

The June 2008 Xbox 360 prototype (`Bin360/`, the "2008-05-20 pre-release" drop) stores its assets in
three nested big-endian containers:

```
mega*.pack (MP00)            an archive of whole files, each a byte copy of a loose file
  └─ <name>.Pack (SBL*)      a directory of typed slots
       ├─ materials          WSAO material list            (docs/2008_materials_format.md)
       ├─ meshes             slot directory → mesh bodies  (docs/2008_msh4_format.md)
       ├─ textures           slot directory → textures     (docs/2008_texture_format.md)
       ├─ wsd (repeats)      EditNodes entry, big-endian    (docs/formats/editnodes.md)
       └─ physics, PathGraphs, AIFences, FlashMovies
```

Readers: [`sab_formats::proto2008`](../tools/sab_formats/src/proto2008/mod.rs) — `mp00`, `sbl`,
`slot_dir`. Census: `sab_probe proto2008-packs <Bin360>`. Which binary reads which revision:
`sab_probe proto2008-binaries <Bin360>`.

**Grades.** PROVEN = read off the engine (`WildStar_p.exe` decomp, or the WildStar_d decomp where the
code is shared and the difference is stated) or checked over every file of the drop this session;
INFERRED = consistent with all data but no engine reader found; UNKNOWN = not established.

## Which build, which binary

* The folders `The Saboteur 2008-05-20 prototype/…/Bin360` and `The Saboteur 2008-06-26 prototype/Bin360`
  are byte-identical (17,148 files; the second adds only `.DS_Store`). `The Saboteur 2028-05-22
  prototype/Bin360` is a **damaged** copy of the same build: `Global/GB_WP_Shotgun_Hunting.Pack` differs
  from byte 999,425 and its `Global/mega.pack` copy no longer matches it; 843 `France/mega0.pack`
  entries match no loose file and one textures slot claims 2,231,123,016 records. PROVEN (byte compare,
  `proto2008-packs` on both).
* `build_info.txt`: "The Saboteur (Xbox 360) Pre-Release, May 20 18:08:22 2008".
* The four game binaries load different revisions of the containers (PROVEN, `proto2008-binaries`:
  each image's `lis`/`ori` FourCC constants, `.xex` files unwrapped with `sab_formats::xex`):

  | binary | original PE name | PE timestamp | SBL | mesh |
  |---|---|---|---|---|
  | `WildStar_d.exe` / `.xex` | `WildStar_d.exe` | 2008-05-07 | `SBL6` | `MSH0` |
  | `Saboteur.xex` | `WildStar_p.exe` | 2008-05-15 | `SBL4` | `MSH4` |
  | `WildStar_p.exe` / `.xex` | `WildStar_p-replay20.exe` | 2008-05-20 | `SBL4` | `MSH4` |
  | `WildStar_p-org.xex` | `WildStar_p.exe` | 2008-05-20 | `SBL4` | `MSH4` |

  `WSStreamBlock::LoadBlock` compares the magic with one constant (`WildStar_p.exe` @0x821a84a0 `SBL4`;
  `WildStar_d.exe` @0x823f6050 `SBL6`) and asserts on anything else. **The `WildStar_p` decomp is the
  oracle for this data**; `WildStar_d` is a neighbouring revision. Regenerate both with
  [`tools/ghidra`](../tools/ghidra/README.md).

## `MP00` — `mega*.pack`

```
+0x00  char  magic[4] = "MP00"
+0x04  u32   count
+0x08  { u32 hash; u32 size; u64 offset }[count]
8+16*count   the files, back to back in entry order; the archive ends after the last
```

* Every entry is a **byte copy of a loose file** under `Bin360/`, and `hash` is
  `pandemic_hash(<path relative to Bin360, backslashes>)`, e.g. `pandemic_hash(r"France\0.Pack")`:
  4,959 of the 4,960 entries of the four archives (`AnimationTest/mega0.pack` 38, `France/mega0.pack`
  1,549, `France/mega1.pack` 1,571, `Global/mega.pack` 1,802). PROVEN.
  The exception is one `Global/mega.pack` entry whose bytes equal a loose `.Pack` but whose hash is not
  `pandemic_hash` of that file's path in any of the six spellings tried (with `/` or `\`, with or
  without extension, file name, stem). UNKNOWN which path it names.
* Entries are not only `SBL*` sub-packs: 63 are empty files (`.pack`, `.txt`, `.wsd`) and 511 are the
  text `.wsd` mission files of `France/`. PROVEN.
* The directory is contiguous: entry 0 starts at `8 + 16*count` and the files tile the archive to EOF.
  PROVEN (`mp00::check_layout` on all four).
* `WSPackFile::Open` (`WildStar_p.exe` @0x823efe00) is the engine reader.

## `SBL*` — the sub-pack

```
+0x00  char  magic[4]       "SBL" + revision byte
+0x04  u32   count
+0x08  WSStreamTOC[count]   24 bytes each
8+24*count   slot bytes, back to back in record order; the sub-pack ends after the last

WSStreamTOC (read by WSStreamTOC::ReadMe, WildStar_p.exe @0x8219ffb0: six u32 in order)
  +0x00  u32  kind       pandemic_hash of the slot type name
  +0x04  u32  offset     absolute offset of the slot in the sub-pack
  +0x08  u32  size       slot length; ReadTOCGroup drops a TOC whose size is 0
  +0x0C  u32  size_dup   == size in every record
  +0x10  u32  flag       1 in every record
  +0x14  u32  id         0, or a hash naming the slot
```

* **The fourth magic byte is a format revision**, not a variant tag: each engine accepts exactly one
  (table above). The data mixes seven: 2,612 `SBL4`, 1,265 `SBLL`, 234 `SBL8`, 157 `SBL6`, 107 `SBLX`,
  10 `SBL0`, 4 `SBL5` loose files, plus 4 `MP00`, 1 `AP04` (`Animations.pack`) and 2 empty files.
  All seven `SBL*` revisions share this layout: every file parses with every invariant (slots tile the
  sub-pack, `size == size_dup`, `flag == 1`). PROVEN. The mesh bodies inside carry the same revision
  letter (`docs/2008_msh4_format.md`).
* **Slot kinds.** `ReadStreamBlockData::Init` (`WildStar_d.exe` @0x8266c6a8; `WildStar_p.exe`
  @0x8237e4e8) dispatches on nine kind hashes; eight occur in the data (`SlotKind`):

  | kind hash | name | reader |
  |---|---|---|
  | `0xD72829F9` | `materials` | `ReadMaterialData` |
  | `0xFE5E3A56` | `meshes` | `ReadMeshData` |
  | `0x4445EA18` | `physics` | `ReadPhysicsData` |
  | `0xA40D777D` | `textures` | `ReadTextureData` (processed after the other kinds in `LoadBlock`) |
  | `0x9AB5A351` | `wsd` | `ReadWSDData` |
  | `0xE1087B27` | `PathGraphs` | `ReadPathGraphData` |
  | `0xD3098461` | `AIFences` | `ReadAIFenceData` |
  | `0xB5D2FE96` | `FlashMovies` | `ReadFlashMovieData` |
  | `0xDD62BA1A` | `sound` | `ReadSoundBankData` — no slot in the build |

  Names: the strings in `WildStar_d.exe` that hash to the kinds (the reader names fix the meaning).
  PROVEN.
* **`id`** (formerly called `block_hash`): 0 on most records. In `wsd` slots it names the slot's
  layer — the five ids of `France/0.Pack`'s `wsd` slots are `pandemic_hash` of `HDV_Mission`, `prison`,
  `Enable_DierkerStatue_HDV`, `HDV_Occupation`, `HDV_Inspired` (and the sixth slot's id is 0, the base
  layer); the same numbers appear in decimal inside the layer's object names
  (`France_Streamblock_0_baked_21--747978868`, 747978868 = 0x2C954074 = `prison`). PROVEN for those
  five. The non-zero ids on the other slot kinds (counting loose files and MP00 copies together:
  1,260 each on materials, meshes and physics, 1,830 textures, 110 FlashMovies) are none of the names
  in `data/hash_names.tsv`. UNKNOWN preimage.
* **Only `wsd` slots repeat** in one sub-pack — 310 sub-packs carry up to 11 (one per layer). PROVEN.
* Slot-type sets per revision (loose files; MP00 copies are identical) are printed by
  `proto2008-packs`; the commonest are `SBL4` `materials+meshes+physics+textures` (775) and
  `materials+meshes+physics+wsd+PathGraphs+AIFences+textures` (757), `SBLL` `materials+meshes` (1,257),
  `SBL8` `textures+materials+meshes+physics` (234).

## Slot directories (`meshes`, `textures`)

```
+0x00  u32          count
+0x04  WSStreamTOC  records[count]     the same 24-byte record, read by WSStreamBlock::ReadTOCGroup
                                        (WildStar_p.exe @0x821a3448), offsets relative to the slot
```

* `+0x08` is the stored length, `+0x0C` the unpacked length, `+0x10` is 1, `+0x14` is 0 (meshes) or
  0/1 (textures: 11,358 records 1). PROVEN over all 7,101 `meshes` and 6,156 `textures` slots.
  In `textures` slots `+0x0C` is the memory image size or the stored length depending on the
  sub-pack revision, and `+0x14 = 1` marks a texture `UnloadNormalMaps` may evict
  (`docs/2008_texture_format.md`).
* Records are contiguous from the end of the directory to the end of the slot; all-zero records occur
  only at the end of the list (17 `meshes`, 38 `textures` slots) and `ReadTOCGroup` drops them (size 0).
  PROVEN.
* `meshes`: 275,288 records are zlib streams (`78 01`, `stored < unpacked`); the 430 others
  (`stored == unpacked`, all in `SBLX` packs) are stored raw — they are variant records
  (`docs/2008_msh4_format.md`). PROVEN.
* `textures`: payloads are stored raw (their streams are zlib inside, `docs/2008_texture_format.md`);
  the 1,181 records whose `+0x0C` equals `+0x08` are those of the `SBL8` and `SBLX` sub-packs.

## Method

```
sab_probe proto2008-binaries <Bin360>   # FourCCs per binary
sab_probe proto2008-packs    <Bin360>   # every .pack, MP00 entry and slot directory
```

## Unknowns

* The preimage of non-zero `id` words outside `wsd` slots.
* The path one `Global/mega.pack` entry's hash names.
