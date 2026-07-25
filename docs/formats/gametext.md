# The Saboteur (2009) — `GameText.dlg` (magic `TXTD` records) format

`GameText.dlg` is the game's **complete localized-text container**: every on-screen UI string
(objectives, mission names, tooltips, fail messages, garage/shop and object display names — the
strings `GameTemplates` and the Lua mission scripts reference) **and** every cinematic VO subtitle,
in one file per language.

There is one file per shipped language, all loose on disk:

```
Cinematics/Dialog/English/GameText.dlg   1 068 005 B   11 333 records
Cinematics/Dialog/French/GameText.dlg    1 120 831 B   11 335 records
Cinematics/Dialog/German/GameText.dlg    1 104 199 B   11 333 records
Cinematics/Dialog/Italian/GameText.dlg   1 071 241 B   11 333 records
Cinematics/Dialog/Polish/GameText.dlg    1 050 803 B   11 333 records
Cinematics/Dialog/Russian/GameText.dlg   1 026 329 B   11 333 records
```

All six **parse with exact byte consumption and re-emit byte-identical** (round-trip proven, see
[Validation](#validation)). There is also a sibling `Cinematics/Dialog/Random/RandomText.rnd`
(random ambient barks) loaded by the same subsystem; not covered here.

---

## Loader (decomp oracle)

`Cin.LoadGameTextFile` (`0x0071c810`) and the boot path both funnel through the region loader
`FUN_0095fb40 @0x0095fb40`, which builds the path with
`_sprintf(buf,"%s%sGameText.dlg", basePath, langDir)` (decomp line 797142) and calls the parser
**`FUN_0095f370 @0x0095f370`** with a create flag. `FUN_0095f370`:

1. reads the header, gates on `version == 5` (decomp: `if (unaff_EDI == 5)`);
2. reads `recordCount` and `totalStringCodeUnits`, allocates `recordCount × 0x24` in-memory entry
   records (36-byte `WSGameTextEntry`) and a `totalStringCodeUnits × 2` UTF-16 string heap;
3. per record, hashes the key with **`FUN_00dc1e20`** (= `pandemic_hash`, called at `0x0095f615`)
   to key the global store `DAT_0147db78`.

Lookup is `Cin.GetLocalizedText` (`0x0071c910`): `pandemic_hash(sTextID)` → `FUN_0095e4e0` on the
store. All integers are little-endian.

The 0x24-byte in-memory entry (`WSGameTextEntry`) lays out as:

| entry offset | holds | evidence |
|---|---|---|
| `+0x18` | **`asset_id`, verbatim from disk — the store's tree key** | `0095f5ac lea eax,[esi+0x18]` / `push eax` / `mov [eax],ecx` / `call 0x9603f0`; `FUN_009603f0` compares `*param_3` against `*(node+0x10)` |
| `+0x1c` | **`pandemic_hash(ascii key)` — the Wwise event id** | `0095f615 call 0xdc1e20` / `0095f61d mov [esi+0x1c],eax`; `FUN_0095df40` gates on `+0x1c` and passes it to the Wwise call `FUN_0091ae20` |
| `+0x20` | UTF-16 string pointer | `0095f65d mov [esi+0x20],ecx` |

Two consequences worth stating plainly, because the earlier revision of this doc had them backwards:

* **`asset_id` is the store lookup key for *every* record, UI and VO alike.** The tree insert at
  `0x0095f5b9` happens unconditionally, *before* the parser has even looked at `key_len`.
* **`pandemic_hash(vo_key)` is not a lookup key — it is the audio event id** that `Sound.PlayTextID`
  fires. Only records with `key_len > 1` get one (`0095f60b cmp eax,1 / jle`).

Note that Ghidra marks the whole record loop unreachable (`0x0095f559`…`0x0095f78b`), so the decomp is
useless here; the above is read from the retail image with a disassembler.

---

## Byte layout (little-endian)

```
Header (12 bytes)
  u32  version = 5
  u32  record_count
  u32  total_string_code_units      -- Σ str_len over the base records (NOT the DNEC sub-tables)

record_count × Record               -- contiguous, no padding
  char  magic[4] = "TXTD"
  u32   asset_id                     -- see "The asset_id / lookup key" below
  u16   key_len                      -- bytes of `key` INCLUDING its trailing NUL; 1 for UI text
  char  key[key_len]                 -- ASCII, NUL-terminated; for UI text this is a lone NUL (never absent)
  u16   str_len                      -- UTF-16 code units (NOT bytes), INCLUDING the NUL terminator
  u16   str[str_len]                 -- UTF-16LE, NUL-terminated

"DNEC" section                       -- per-cinematic-scene VO overlays (see below)
  u32   group_count
  group_count × { u32 scene_hash; u32 file_offset }   -- file_offset is ABSOLUTE
  group_count × SubTable                               -- in directory order (== ascending offset)
    u32   count                        -- TXTD records in this scene overlay
    u32   tsize                        -- Σ str_len over this sub-table (code units), == count's Σ
    count × Record                     -- same TXTD layout as the base records
    char  magic[4] = "DNEC"            -- 4-byte terminator (NOT a nested directory)
```

### The `asset_id` / lookup key  ★ the load-bearing fact for modding

Records come in **two kinds**, distinguished by `key_len`:

| kind | `key_len` | `key` | `asset_id` | store lookup key | Wwise event id | count (EN) |
|------|-----------|-------|------------|------------------|----------------|-----------:|
| **UI text** | `1` | a lone NUL | `pandemic_hash(fullDottedID)` | `asset_id` | *(none — not hashed)* | 4037 |
| **VO subtitle** | `>1` | `"vo_…"` ASCII + NUL | opaque id, **not** `pandemic_hash(key)` | `asset_id` | `pandemic_hash(key)` | 7296 |

**Both kinds are looked up by `asset_id`.** `key_len` selects only whether an audio event id is
derived; it does not change the lookup key.

* **UI text is keyed only by the hash of its dotted ID.** Proven: `pandemic_hash` of a dotted ID
  such as `"A1M0_Text.TASK_RaceJavier"` equals the record's `asset_id`, and the value is the on-screen
  string (`"Race Javier"`). **627 / 651** distinct `<File>_Text.<Key>` / `tooltips.<Key>` IDs harvested
  from `LuaScripts.luap` match a base-record `asset_id` exactly. The `<File>_Text` prefix is part of the
  hashed string, **not** a separate on-disk group — there is no per-file table for UI text; it is one
  flat namespace in the base records.
* **VO subtitles** additionally store an ASCII `vo_…` key, which the engine hashes into the Wwise
  event id. `asset_id ≠ pandemic_hash(key)` for VO records (0 / 7296 match across all six languages)
  — do not assume it is.

**To add a brand-new UI string:** append a record
`{ "TXTD", asset_id = pandemic_hash("MyFile_Text.MyKey"), key_len = 1, key = "\0", str_len, utf16le }`,
bump `record_count`, add `str_len` to `total_string_code_units`, and rebase the `DNEC` offsets (below).
`str_len` **must count the UTF-16 NUL terminator** — every retail record does, and
`total_string_code_units == Σ str_len` only holds with it included.

Because UI text lives in the always-loaded base records, **no Lua `LoadGameTextFile` registration is
required** for the new ID to resolve. *(That last clause is an inference from the load path, not an
observed runtime result — it has not been confirmed in-game.)*

### `DNEC` section — per-scene VO overlays

After the base records, magic `DNEC` (bytes `44 4E 45 43`) begins a directory:
`u32 group_count`, then `group_count × { u32 scene_hash, u32 file_offset }`. Each `scene_hash` is the
hash of a cinematic scene; some match a loose `<hash>.pov` file at the **install root** (not `Global/`,
which holds only the four `.megapack`s) — e.g. `2721ff0b`, `955a92d4`. Only 6 of the 81 `DNEC` scene
hashes have a loose `.pov`; the rest are presumably packed. Each `file_offset` is an **absolute file
offset** to a self-contained sub-table, and the sub-tables are laid out **in directory order (which is
also ascending offset order)** immediately after the directory, contiguous to EOF.

Each sub-table is `{ u32 count, u32 tsize, count × TXTD record, "DNEC" }` — note there is **no**
`version` field (an earlier revision of this doc misread the first group's `count==5` as `version=5`),
and it ends with a 4-byte `DNEC` **terminator**, not a nested directory. `tsize` is `Σ str_len` over
that sub-table's records (code units incl. NUL), exactly as the base header's `total_string_code_units`
is for the base records. These sub-tables carry the **per-scene cinematic VO subtitles the base section
does not** — 1312 extra records per language (EN), keyed by `asset_id` like every other record.

A byte-faithful writer reconstructs the whole section: base records, then `DNEC` + `group_count` +
directory (with **recomputed** absolute offsets), then each sub-table (recomputing `count`/`tsize`).
Because every offset is derived from the layout, editing a base **or** a DNEC record — changing size by
any `Δ` — just falls out; there is no separate rebasing step. Verified: `tools/sab_gametext` parses
the sub-tables into first-class editable records and round-trips all six languages byte-identically.

### `pandemic_hash`

Identical to the engine kernel `FUN_00dc1e20` and to `mercs2_formats::hash::pandemic_hash_m2`:
FNV-1a/32, basis `0x811C9DC5`, prime `0x01000193`, per-byte `|0x20` case-fold, finalize
`^0x2A` then `*prime`; empty string ⇒ 0. Sanity: `pandemic_hash("ANY") == 0xED057225`.

---

## Validation

The parser/writer lives in `tools/sab_formats/src/gametext.rs` (shared by the Workshop and validator);
`tools/sab_gametext` is a thin CLI over it (read / edit / add / write). Run over all six languages:

* **Exact byte consumption** — the base record loop + fully-parsed `DNEC` sub-tables consume every
  byte; cursor == filesize.
* **Round-trip byte-identical** — re-serializing header + base records + the reconstructed `DNEC`
  directory and sub-tables reproduces the original file exactly (all 6 languages), with the DNEC
  section now fully structured (not preserved as an opaque blob).
* **`total_string_code_units == Σ str_len`** over the base records — exact, all 6 languages.
* **Semantic** — 627 / 651 Lua-referenced dotted UI IDs hash to a base record `asset_id`; the ~24 misses
  are dynamically-constructed IDs or per-scene overlay strings, not format failures.

---

## Confidence

| claim | status |
|-------|--------|
| header `{u32 ver=5, u32 count, u32 totalCU}`, LE | **CONFIRMED** (bytes + parser `FUN_0095f370` version gate + alloc sizes) |
| record = `"TXTD"`, u32 asset_id, u16 key_len(incl NUL), key, u16 str_len(CU), utf16le | **CONFIRMED** (exact-consume + byte-identical round-trip, 6 langs) |
| strings are NUL-terminated, terminator counted in `str_len` | **CONFIRMED** (68 000/68 000 records across 6 langs end in a NUL code unit, 0 exceptions, 0 embedded NULs; `Σ str_len == total_string_code_units` only holds with it) |
| UI text: `key_len==1` (a lone NUL), never 0 | **CONFIRMED** (0 records with `key_len==0` in any language; 4037/4037 UI records are `key_len==1, key==\x00`; engine's own gate is `cmp eax,1 / jle` @`0x0095f60b`) |
| UI text: asset_id == `pandemic_hash(dottedID)` | **CONFIRMED** (627/651 semantic matches) |
| **`asset_id` is the store lookup key for ALL records** (UI and VO) | **CONFIRMED** (disasm `0x0095f5ac`–`0x0095f5b9`: `&entry+0x18` is passed as the tree key and the insert is unconditional, before `key_len` is read; `FUN_009603f0` compares `*param_3`) |
| VO: `pandemic_hash(key)` → `entry+0x1c` = the **Wwise event id**, not a lookup key | **CONFIRMED** (disasm `0x0095f615`/`0x0095f61d`; `FUN_0095df40` gates on `+0x1c` and passes it to `FUN_0091ae20`); `asset_id != pandemic_hash(key)` 0/7296 |
| `total_string_code_units == Σ str_len` (base) | **CONFIRMED** (exact, 6 langs) |
| `DNEC` = per-scene overlay directory of `{scene_hash, abs file_offset}` + sub-tables | **CONFIRMED** (framing + sub-table layout) |
| sub-table = `{u32 count, u32 tsize, count × TXTD, "DNEC" term}`, `tsize == Σ str_len`, dir order == physical order | **CONFIRMED** (6 langs: exact byte consumption to EOF, `tsize==Σ str_len`, ascending offsets, byte-identical round-trip with the section fully structured; 1312 sub-records/lang) |
