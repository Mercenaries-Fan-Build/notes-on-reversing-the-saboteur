# sab_gametext

Reader/writer for **The Saboteur (2009) `GameText.dlg`** — the game's complete localized-text
container (one file per language under `Cinematics/Dialog/<Lang>/`). It holds every UI string
(objectives, mission names, tooltips, fail messages, shop/object display names — the text
`GameTemplates` and the Lua scripts reference) **and** every cinematic VO subtitle.

Format spec + confidence table: [`../../docs/formats/gametext.md`](../../docs/formats/gametext.md).
This is a **thin CLI over [`sab_formats::gametext`](../sab_formats/src/gametext.rs)** — the one parser/
writer the Workshop, validator and this tool all share (no duplicated format logic). Verified
**byte-identical round-trip on all six retail language files**, base **and** DNEC sections.

## Commands

```
sab_gametext hash  <string>                          pandemic_hash of a text id
sab_gametext info  <in.dlg>                           header + UI/VO counts + DNEC group/record counts
sab_gametext list  <in.dlg> [--ui|--vo|--dnec] [--limit N]   list records (asset_id, key, text preview)
sab_gametext get   <in.dlg> (--id <DottedID> | --hash 0x..) [--scene 0x..]   read one string
sab_gametext set   <in.dlg> <out.dlg> (--id <DottedID> | --hash 0x..) [--scene 0x..] --text "<STRING>"
                                                       overwrite an existing record's string (base or DNEC)
sab_gametext add   <in.dlg> <out.dlg> --id <DottedID> --text "<STRING>"
                                                       append a NEW UI-text record
sab_gametext add-dnec <in.dlg> <out.dlg> --scene 0x.. --text "<STRING>" (--key vo_.. | --hash 0x..)
                                                       append a NEW subtitle into a DNEC scene group
sab_gametext roundtrip <in.dlg>                        parse -> re-emit; assert byte-identical
```

`--dnec` lists the **per-scene cinematic VO subtitles** in the `DNEC` section (1312 extra records per
language that the base list does not show). `get`/`set` search the base records first, then the DNEC
sub-tables; pass `--scene 0x<sceneHash>` to disambiguate when the same `asset_id` appears in a
specific overlay group. The whole `DNEC` section is now parsed into first-class editable records —
edits to base **or** DNEC strings round-trip byte-identically.

## How UI text is keyed (the modding-relevant fact)

A UI string is looked up by `pandemic_hash("<File>_Text.<Key>")` (e.g.
`GetLocalizedText("A1M0_Text.TASK_RaceJavier")`). On disk that record has an **empty key** and its
`asset_id` **is** that hash. So `add --id A1M0_Text.MyKey --text "…"` writes a record the engine will
resolve immediately — **no Lua `LoadGameTextFile` registration is required** (UI text lives in the
always-loaded base records). VO subtitles instead carry an ascii `vo_…` key and are looked up by
`pandemic_hash(key)`; their `asset_id` is the audio event id.

## Examples

```
$ sab_gametext get   English/GameText.dlg --id A1M0_Text.TASK_RaceJavier
0xafc7fd9c  "Race Javier"

$ sab_gametext set   English/GameText.dlg out.dlg --id A1M0_Text.TASK_RaceJavier --text "Beat Javier to Germany"
$ sab_gametext add   out.dlg out2.dlg --id KatMod_Text.Obj1 --text "Blow the bridge"
```

```
$ sab_gametext list  English/GameText.dlg --dnec --limit 2
scene 0x46bc4897  0x2987393a vo_000_Belle_WeaponCheck_DorrisGirl1_01  "Oh, pardon."
scene 0x46bc4897  0xf9a144af vo_000_Belle_WeaponCheck_DorrisGirl1_02  "No weapons inside, handsome. "

$ sab_gametext set   English/GameText.dlg out.dlg --hash 0x2987393a --scene 0x46bc4897 --text "Oh, pardon me!"
```

Ship an edited `GameText.dlg` by placing it at `Cinematics/Dialog/<Lang>/GameText.dlg` (back up the
original first). Editing strings and adding UI ids are both any-length; the writer recomputes the
header string-heap size and rebuilds the `DNEC` (cinematic-overlay) directory and sub-table headers
automatically, so a size change anywhere re-derives every absolute offset.
