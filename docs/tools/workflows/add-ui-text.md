# Add or change UI text

Mission names, objectives, tooltips, fail messages, shop and object display names, and every
cinematic subtitle live in one file per language. Changing them — or **adding entirely new strings
for your own mod** — needs one tool and no repacking.

**Tool:** the **GameText** page in `sab_workshop`, or the CLI (`sab_probe gametext` to inspect,
`sab_gametext` to write)
**Time:** seconds. **Reversible:** restore the backup.

---

## Where the text lives

```
<game>/Cinematics/Dialog/<Lang>/GameText.dlg
```

One container per language, holding both halves:

- **UI text** — everything the HUD and menus display. Keyed by `pandemic_hash("<File>_Text.<Key>")`.
- **VO subtitles** — cinematic lines. Keyed by an ASCII `vo_…` key; the `asset_id` is the audio
  event id.

`sab_gametext` round-trips **byte-identically on all six retail language files**, so the container
it writes is the container the engine expects.

## 1. Find the string

```sh
GAME="C:/GOG Games/The Saboteur"
DLG="$GAME/Cinematics/Dialog/English/GameText.dlg"

sab_probe gametext info $DLG                      # header + UI/VO/DNEC counts
sab_probe gametext list $DLG --ui --limit 40      # browse UI strings (--vo / --dnec too)
sab_probe gametext get  $DLG --id A1M0_Text.TASK_RaceJavier
# 0xafc7fd9c  "Race Javier"
```

*(Read-only inspection is `sab_probe gametext …`; the write commands below are the `sab_gametext`
authoring bin. Both drive the one parser in `sab_formats::gametext`.)*

## 2. Change it

```sh
sab_gametext set $DLG out.dlg --id A1M0_Text.TASK_RaceJavier --text "Beat Javier to Germany"
```

Strings are **any length** — the writer recomputes the header's string-heap size and rebases the
trailing `DNEC` (cinematic-overlay) section automatically. You are not limited to the original
character count.

## 3. Or add a new one

This is the part that makes custom missions and menus possible:

```sh
sab_gametext add out.dlg out2.dlg --id KatMod_Text.Obj1 --text "Blow the bridge"
```

Then reference it from Lua exactly like a stock string:

```lua
GetLocalizedText("KatMod_Text.Obj1")
```

**No `LoadGameTextFile` registration is needed.** A UI record's `asset_id` *is*
`pandemic_hash(dottedID)`, and UI text lives in the always-loaded base records — so the engine
resolves a new id the moment it's in the file. This is the single most useful fact about the format
for modders.

You can check the hash a name will produce without touching the file:

```sh
sab_probe gametext hash KatMod_Text.Obj1
```

## 4. Install

Back up the original, then replace it:

```sh
cp "$DLG" "$DLG.bak"
cp out2.dlg "$DLG"
```

There is no patch-override path for `GameText.dlg` the way there is for megapacks — it is a loose
file, so you edit it in place. **Keep the `.bak`**; that is your uninstall.

## Verify

```sh
sab_probe gametext roundtrip out2.dlg    # parse -> re-emit -> assert byte-identical
sab_probe gametext get       out2.dlg --id KatMod_Text.Obj1
```

`roundtrip` proves the container you produced is self-consistent before the game sees it.

## Notes

- **One file per language.** Editing `English/GameText.dlg` leaves the other five untouched — a
  player running the game in French will see the original string. Repeat per language you support.
- **VO subtitles** are read with `--vo` and keyed differently (ASCII `vo_…` key, hashed). Editing one
  changes the subtitle only; the audio is a separate asset.
- The GUI equivalent — browse, search, edit, add — is the **GameText** page in
  [`sab_workshop`](../../../tools/sab_workshop/README.md).

Format spec: [`docs/formats/gametext.md`](../../formats/gametext.md).
