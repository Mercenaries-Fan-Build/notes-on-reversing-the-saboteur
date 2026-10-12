# Hash → name table (`data/hash_names.tsv`)

The game addresses almost everything by `pandemic_hash` (`FUN_00dc1e20`; FNV-1a/32 with ASCII case
folding and a finalizer, [`sab_formats::hash`](../tools/sab_formats/src/hash.rs)). This page describes
the one table that names those hashes, how it is built, and how far each row can be trusted.

* **Table:** [`data/hash_names.tsv`](../data/hash_names.tsv) — 81,971 rows naming **81,964 of the
  138,564** hashes the retail game uses as keys (2026-10-12).
* **Library:** [`sab_formats::names`](../tools/sab_formats/src/names.rs) — `name(hash)` (best name),
  `names(hash)` (every name found), `table()`. `editnodes::tag_name` and
  `gametemplates::known_property_name` read it; there is no other name table in the library.
* **Regenerate:** `sab_probe hash-names <retail_dir> <Saboteur.exe> <proto2008_dir>
  <community_tools_dir> <community_research_dir> > data/hash_names.tsv` (≈15 s; the report goes to
  stderr). Byte-identical across runs on the same inputs. The GOG `Saboteur.exe`; the 2008 prototype
  folder holding `WildStar_*.pdb/.map`; `game-files/community-tools`; `game-files/community-research`.

## Columns

| column | meaning |
|---|---|
| `hash` | `0x%08x`; `pandemic_hash(name) == hash` for every row (test `every_row_is_a_preimage_and_the_table_is_sorted`) |
| `name` | spellings that differ only in case are one row (the hash folds case); the game-data spelling is kept |
| `grade` | `structural` / `data` / `text`, below |
| `chance` | `text` rows only: estimated probability that the name is a chance collision; `-` otherwise |
| `kinds` | where the game uses the hash as a key (below) |
| `sources` | where the name was found (below) |

A hash has more than one row when more than one name was found for it (7 hashes). `names::name`
picks the best grade, then the lowest `chance`.

## Targets: the hashes the game uses (`kinds`)

| kind | what | targets | named |
|---|---|---:|---:|
| `megapack.crc` | megapack entry `crc` — the pack path hash ([world_megapack_key.md](formats/world_megapack_key.md)) | 9,171 | 9,062 |
| `megapack.index` | megapack entry `index`, for entries that are not streamblocks (whose `index` is an id) | 1,168 | 1,119 |
| `albs.type0` … `albs.type7` | ALBS record hashes, by record type ([sbla_subpack.md](formats/sbla_subpack.md)) | type 0: 45,134; 1: 8,552; 2: 33,823; 3: 7,745; 4: 7,745; 7: 38 | 45,134; 8,552; 33,823; 420; 420; 38 |
| `map.record` | `global.map` / region-map record hashes ([map6.md](formats/map6.md)) | 1,492 | 1,492 |
| `map.tile` | region-map tile hashes | 29,571 | 0 |
| `editnodes.container` | EditNodes entry (container) hashes ([editnodes.md](formats/editnodes.md)) | 770 | 581 |
| `editnodes.object` | EditNodes object tags | 13,641 | 6,251 |
| `editnodes.param` | EditNodes param tags | 272 | 246 |
| `gametemplates.property` | GameTemplates property keys ([gametemplates.md](formats/gametemplates.md)) | 2,724 | 1,474 |
| `gametemplates.value` | GameTemplates 4-byte values (refs to templates, textures, `"none"` — and also plain ints and floats, which are not hashes) | 19,775 | 11,312 |
| `gametext.ui-id` | `asset_id` of UI text records (`key_len == 1`) | 4,037 | 1,237 |
| `gametext.scene` | DNEC scene hashes | 81 | 72 |

ALBS type-8 records (hash 0), streamblock `index` values (ids), and GameText VO records (whose
`asset_id` is an audio event id and whose key is only hashed at lookup) are not targets. Hashes of
other kinds — Wwise ids (FNV-1, [audio_1kcp.md](formats/audio_1kcp.md)), animation and Havok ids,
`.luap` keys — are not harvested yet.

## Sources (`sources`)

Each source is a family (the part before the first `.`) and a method.

| source | names |
|---|---|
| `albs.msha-name`, `albs.dtex-name` | the name stored in each MSHA / DTEX record |
| `map.name`, `map.name+list`, `map.pack-path`, `map.streamblock-path` | map record and tile names; `global.map` list names with their list digit (`name0` / `name1`, the stored record hash); each object's pack path and each streamblock's path ([world_megapack_key.md](formats/world_megapack_key.md)) |
| `loosefiles.name` | `loosefiles_BinPC.pack` entry names |
| `editnodes.string`, `.lua-string`, `.lua-crclist`, `.lua-name`, `.derived-container` | EditNodes string leaves; LuaParam string values, `LuaCrcList` items and parameter names; for a backslash path, `France\EditNodes\<parent>.wsd` |
| `gametemplates.name`, `.type`, `.string` | template names, template types, string-valued pairs |
| `engine-format.exe` | `Saboteur.exe` strings with exactly one `%.2d` (`Node%.2d`, `FenceNode%.2d`, `FenceNodeFlag%.2d`, `FenceWallFlag%.2d`, `Prompt%.2d`, …) expanded 00–99 |
| `luap.*`, `exe.*`, `proto2008.*` | `LuaScripts.luap`, `Saboteur.exe`, the 2008 prototype `.pdb` / `.map` files: `.string` = printable run, `.token` = identifier or phrase, `.variant` = a token with 1–5 leading bytes cut, or an `m_`/`s_`/`g_`/`bX` prefix dropped |
| `community-research.*` | quoted literals, lines and tokens of the files in `game-files/community-research` |
| `saboteurtoolset.strings` | SaboteurToolset's `saboteur_strings.txt` |
| `sabtool.hashes` | SabTool's `Hashes.txt` (the name after the colon; its stated hash is ignored) |
| `gametemplates_properties.tsv:dict/exe/sym` | the names in [`data/gametemplates_properties.tsv`](../data/gametemplates_properties.tsv), by their `name_src` |

## Grades and the chance estimate

`pandemic_hash` is 32 bits, so a wrong string matches a target by chance with probability
`targets / 2^32` ≈ 1/31,000. Offering millions of strings therefore produces chance hits; the grade
says what else supports a row.

* **`structural`** (5,118 hashes) — an EditNodes object name `P\N` whose parent path, hashed as a
  container (`pandemic_hash("France\EditNodes\" + P + ".wsd")`), is the hash of an entry that holds
  the object. Two independent hashes agree: PROVEN.
* **`data`** (68,448) — some source is a string stored in the game data or built by the engine
  (families `albs`, `map`, `loosefiles`, `editnodes`, `gametemplates`, `engine-format`).
* **`text`** (8,398) — every source is outside the game data. `chance` is the lowest, over the row's
  sources and kinds, of: chance hits expected from that source on that kind (distinct candidates ×
  targets of the kind / 2³²) ÷ rows of that kind that only the source's family names. A source's
  `gametemplates_properties.tsv:*` rows use the candidate count of the method they were harvested
  with (`proto2008.variant`, `exe.variant`, `community-research.quoted`).

A `text` row is kept when its `chance` is ≤ 10%, or when sources of two families offer it whole (a
`.variant` is a fragment we cut, not a string found as such). The sum of `chance` over kept `text`
rows is **≈ 24 expected chance rows out of 8,398**. 112 rows were excluded (the report lists each);
they are prototype-symbol lines and fragments such as `"2193290 f   WSHUDManager"` and
`"CAkSound::Category"`.

Why per kind: chance hits land in proportion to a kind's size, so a cut fragment that names one of
2,724 property keys is far likelier to be right than one that names one of 29,571 map tiles. Why not
"two sources agree": a fragment such as `LDING` (from `BUILDING`) is in the exe, the scripts and the
symbols alike — several sources offering one string do not make its hash match less of a coincidence.

## What the harvest established

* **`LuaCrcList` values name EditNodes objects.** The 775 items of the 523 `LuaCrcList` parameters
  in the main pack are object paths (`Missions\act_1\factory\main\LOC_Spawn1`). They name 545 target
  hashes — 541 objects and 4 GameTemplates values — and **352 are `structural`**; 414 are named by no
  source outside EditNodes ([editnodes.md](formats/editnodes.md#luaparam-leaves)).
* **Object names are recoverable.** 6,251 of 13,641 EditNodes object tags are named, 5,118
  structurally. Structural rows by source (a row can have several): `game-files/community-research`
  lines 4,629, script strings 1,610, SabTool 810, `LuaCrcList` items 352, LuaParam strings 298.
* **UI text ids are `<Section>.<Key>`.** 1,237 UI `asset_id`s are named, e.g. `HUD.Saving`,
  `Controls.Gamepad`, `TutorialTip_Text.BridgeKiller_Freeplay` — whole strings in the exe and in
  SaboteurToolset's list.
* **The `global.map` list records' stored hash is `pandemic_hash(name + list digit)`** (325/325 each;
  [map6.md](formats/map6.md)).
* **The previous property-name table contained chance hits.** `data/gametemplates_properties.tsv` (and
  the 835-row table that was compiled from it) lists `72b20` for `0x4D165B66`,
  `8bcc10 f   AkAudioEngine` for `0x430217A0` and `_MinAirResistanceSpeed` for `0xE655DC80`; the
  table here ranks `Back Light Falloff`, `Spotlight Intensity` and `Fwd Fric HandBrake Scalar`
  (SabTool, lower `chance`) first and keeps both. Those names were cut from symbol strings
  (`s_GrassWindSpeed` → `WindSpeed`) against 2,723 keys, the same method as `proto2008.variant`.

## Open

* **Map tiles** (29,571) have no names; their hash is not `pandemic_hash` of the stored tile name.
* **ALBS type-3/4 records**: 420 of 7,745 are named, almost all by decimal numbers (`0x043EE0F7` =
  `"5377"` = 0x1501, `"262657"` = 0x40201) from SabTool and from cut symbol tokens. What the numbers
  are is not established; until it is, these rows are `text` with their `chance`.
* **109 of the 9,171 `megapack.crc` targets** are unnamed; the kiloPack keys are among the open ones
  ([world_megapack_key.md](formats/world_megapack_key.md#open)).
* **EditNodes objects** 7,390 and **containers** 189 unnamed; **property keys** 1,250 unnamed.
* Sources not harvested yet: the Wwise id table, `.luap` keys, animation names, GameText in other
  languages.
