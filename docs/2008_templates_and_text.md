# 2008 Xbox 360 prototype — game templates (`BLU2`) and game text (version 3)

**Scope:** the 2008-05-20 build's object definitions (`Bin360/GameTemplates.wsd`) and its text
(`Bin360/Cinematics/Dialog/English/GameText.dlg`): their layout, the loaders that read them, and what
assembling characters and assets from the templates rests on. Retail formats:
[formats/gametemplates.md](formats/gametemplates.md), [formats/gametext.md](formats/gametext.md).
**Library:** [`sab_formats::gametemplates`](../tools/sab_formats/src/gametemplates.rs) and
[`sab_formats::gametext`](../tools/sab_formats/src/gametext.rs) read both revisions.

Grades: PROVEN = read in the decomp or checked over the whole file; INFERRED; UNKNOWN.

## `GameTemplates.wsd` — `BLU2`

The retail `AULB` layout with every word big-endian, pair data included.

```text
"BLU2"  u32 entry_count
entry_count × Entry: u32 size, u32 unk1, u32 blueprint_count, then blueprint_count ×
    { u32 name_len, name+NUL, u32 type_len, type+NUL, u32 pair_count,
      pair_count × { u32 property hash, u32 data_size, data } }
```

* **Loader.** `LoadTemplates` (WildStar_p @0x823695e8, WildStar_d @0x82653858) reads the magic, then
  per entry the size, a word and a blueprint count, and per blueprint the name, the type and a
  property set handed to `WSBlueprint::Create`. All three 2008 images build the constant `BLU2`
  (`lis`/`ori` at WildStar_d 0x826538C4, WildStar_p 0x82369658, Saboteur.xex 0x823653A8). PROVEN.
* **Markers.** The retail "marker" (`size 8` and eight zero bytes) is an entry with zero blueprints.
  PROVEN (the loader's blueprint loop).
* **The file.** 3,680,911 bytes; 5,998 entries = 5,803 templates + 195 markers; every template has
  `unk1 = 0`, `blueprint_count = 1`. It parses to its last byte and writes back byte-identical.
  PROVEN (game gate `templates_and_game_text_read_and_round_trip`).
* **Values.** A 4-byte value is a big-endian word: a `pandemic_hash` reference reads as the hash of
  the name it names (e.g. `FBP_RS_Sean_UB`'s `HighDetailModel` = `pandemic_hash("CH_AL_SeanDevlin_01_UB")`).
  PROVEN (every reference below resolves).

### Assembling from the templates

The same template system as retail. Counts from `sab_probe proto2008-templates`:

* **Body setups.** 114 `FxHumanBodySetup` templates use the retail stream: slot keys `UpperBody`,
  `LowerBody`, `GearA`–`GearC`, `0x37A9525D` and `Head`, each followed by its variations
  (`FxHumanBodyPartVariation` / `FxHumanHeadVariation`) holding `BodyPart` / `Face` references. Of 870
  part references, 569 `BodyPart` and 298 `Face` name an `FxHumanBodyPart` / `FxHumanHead` template, 3
  name no template; all 870 sit inside a variation. PROVEN.
* **Part models.** Every `FxHumanBodyPart` / `FxHumanHead` `HighDetailModel` and `LowDetailModel` names
  a `meshes` record (212 + 80 high, 212 + 79 low) or no record at all (9 + 2 high, 9 + 3 low). Each
  record named is stored in the pack named after the part template (`FBP_RS_Sean_UB` →
  `Global/FBP_RS_Sean_UB.Pack`), and every copy of it in the build is byte-identical. PROVEN.
* **Why the pack named after the template.** `WSBlueprintManager::RequestDynamicBlueprint`
  (WildStar_d @0x8235cc68) calls `WSStreamingManager::LoadDynamic` with the blueprint's id, which
  finds the dynamic stream block keyed by that id. PROVEN (decomp). The blocks are the `Global/`
  packs named after templates (1,181 of 1,803 `Global/*.Pack` stems are a template name).
* **Direct references.** Templates of other types name meshes directly (`Prop` 1,589, `Weapon` 109,
  vehicles, …). Of those references: 1,588 name a record in the template's own pack; 318 name a record
  only elsewhere in `Global/`, whose copies are identical; 5 name a record with differing copies in
  `Global/`; 2,560 name a record only in world packs (`France/`). PROVEN.
* **The player.** The one `Player` template, `Saboteur`, names `FxBodySetup` = `FBS_RS_Sean`, as the
  retail `Player` template does. PROVEN (both files).
* **Combined models.** 45 of the 114 body setups name a `CombinedLODModel` themselves (retail: none
  does); some, such as `FBS_CIVILIAN_FEMALE`, have every slot empty and that model as their only mesh.
  The engine draws it as the figure's level 2 — [gametemplates.md](formats/gametemplates.md). PROVEN.
* **Damage parts.** State the group state they draw in with `0xb7b68d44` instead of retail's
  `Pristine`; without it a part draws in every state —
  [damage_and_destruction.md](damage_and_destruction.md). PROVEN.
* **Copies.** 137,861 `meshes` records hold 47,197 names; 46,802 names have byte-identical copies
  wherever they occur, 395 do not (world building pieces such as `BD_NOR_*`). Of the 1,199 names with a
  copy in `Global/`, 17 have differing `Global/` copies. PROVEN.

So an asset assembles from the pack named after each template that names a mesh, else from any
`Global/` copy when they are all the same record; a reference to a name with differing `Global/`
copies, or to a world-only mesh, is reported as not available, as retail's workshop reports a
reference to a mesh outside `Dynamic0.megapack`.

## `GameText.dlg` — version 3

```text
Header (big-endian): u32 version = 3, u32 record_count, u32 total_text_bytes
record_count × { "DTXT", u32 asset_id, u16 key_len (incl NUL), key, u16 str_len (bytes, incl NUL), str }
"CEND"
```

* **Loader.** `WSTextManager::LoadTextFiles` (WildStar_d @0x82a3bfc8, WildStar_p @0x826350c0): checks
  version 3, allocates `record_count` entries and `total_text_bytes` of text, then reads tagged records
  until `CEND`: `DTXT` → `asset_id` (the lookup key), the key (when longer than its NUL, `PblHash(key)`
  is kept too) and `str_len` text bytes. PROVEN (WildStar_d decomp; all three images build `DTXT`).
* **The file.** English only (`Random/RandomText.rnd` is the other file in `Dialog/`). 379,243 bytes,
  5,434 records: 1,531 keyless (UI) and 3,903 keyed (VO lines). It parses to its last byte, the header
  total equals the text bytes, and it writes back byte-identical. PROVEN.
* **Text encoding: Windows-1252.** 736 text bytes are above 0x7F: `0x85` (`…`, 200 uses, "I see you,
  Irish…"), `0x91`–`0x94` (curly quotes, "Chippin’", "“uncle”"), `0x96` (`–`), and accented letters of
  the French and German barks (`0xE9` in "Résistants", `0xFC` in "drüben"). Latin-1 would make the
  0x80–0x9F bytes control characters. PROVEN by content.
* **Ids.** UI ids hash the dotted id without retail's `_Text`: a weapon's `Display Name`
  0xF92BB2F5 = `pandemic_hash("ItemNames.WP_PS_Luger")`, record text "Luger". INFERRED (one check).

## Method

```
cargo test --release --features retail --test proto2008_retail     # tools/sab_formats
cargo run --release -p sab_probe -- proto2008-templates <Bin360>
cargo run --release -p sab_probe -- template-lod-damage <Bin360>/GameTemplates.wsd
```

## Unknowns

* `unk1` (0 in every template of both builds).
* Slot key `0x37A9525D` has no known name (as in retail).
* Whether every UI id follows the `<Section>.<Key>` form without `_Text`.
