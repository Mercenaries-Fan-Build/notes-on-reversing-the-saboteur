# 2008 pre-release Xbox 360 build — inventory & retail diff

*Generated 2026-09-13. Source of truth for machine-readable data: [2008_build_inventory.json](2008_build_inventory.json) (structured summary + notable-file callouts) and [2008_build_inventory_files.tsv](2008_build_inventory_files.tsv) (per-file `bytes / mtime_utc / rel_path` for all 17,148 files).*

Companion to [lineage_and_divergence.md](lineage_and_divergence.md) and [binary_recon.md](binary_recon.md).

## What this is

A pre-release Xbox 360 build of The Saboteur, extracted from `game-files/The Saboteur (Jun 26, 2008 prototype).rar` (14.27 GB compressed) into `game-files/Bin360/` (17,148 files, 13.29 GiB on disk).

Two dates coexist in this drop:

| Source | Says | Meaning |
|---|---|---|
| `Bin360/symbols/build_info.txt` | `Pre-Release / May 20 18:08:22 2008` | The build label engineering stamped on the content freeze |
| File mtimes | **17,116 / 17,148** files at `2008-05-22` | The content freeze itself (assets + shipping xex + PDBs) |
| File mtimes on 7 files | `2008-06-24` and `2008-06-26` | Later rebuild of `WildStar_p.xex`, `ReplayCore-xenon.xex`, `ReplayLauncher-Xenon.xex`, `Replay/*`, and `MemoryPools_v3.ini` on top of the same content |
| 2 files at `2013-09-04` | `Asserts.TXT` (190 B), `Last_cam_start` (31 B) | Written by whoever ran the build after the community got it, not part of the drop |

The `symbols/` sibling directory, `extract_symbols.py`, `saboteur_toolkit/` and `communit-world-editor/` in the parent `game-files/` folder are the user's own workbench, not part of the pre-release drop. Note `Bin360/Saboteur.pdb` (45,026,304 B) is **not** the same file as `symbols/Saboteur.pdb` (16,788,480 B) — the cached one was extracted from an earlier `2008-05-20 prototype.7z` and is stale relative to the current `.rar` drop.

## Platform

Two toolchains are present side by side, which is what makes this drop unusual:

- **Xbox 360 ship binaries**: `Saboteur.xex` starts with `XEX2` (11.15 MB, the game itself). `ReplayCore-xenon.xex`, `ReplayLauncher-Xenon.xex`, and `WildStar_p.xex` / `WildStar_d.xex` are also Xenon (Xbox 360 CPU codename). Audio is `.xma` (Xbox 360 codec, 3033 files, 314 MB).
- **Windows PC devkit binaries**: `WildStar_d.exe` (21.8 MB) and `WildStar_p.exe` (11.8 MB) start with `MZ` — they are Win32 PE. Together with `WildStar_d.pdb` (66.7 MB), `WildStar_p.pdb` (42.9 MB), their `.map` linker maps and `.xdb` Xbox debug info, these are the engine tools built against the same content, not a PC port of the game.

So `Bin360` is best read as a **Pandemic build-machine drop**: the shipping Xbox executable plus the PC-side dev toolchain that produced it, plus the content in loose form. Retail is only the Xbox ship exe's PC port and the packed content.

## Provenance details

Two artifacts leak the internal build layout:

- `Asserts.TXT` (an assert log left over from a previous run):
  > `D:\projects\WildStar\Code\WildStar\GameLayer\WSJoystick.cpp(361): Justin Truman's GENERAL ASSERT FAILED: !a_AssertIfNotFound in WSJoystick::atoInputType : Could not find InputType ecc32b15`

  Confirms **engine codename `WildStar`**, source root **`D:\projects\WildStar\Code\WildStar\`**, at least one engineer name (**Justin Truman**), and the C++ class prefix **`WS`** (WSJoystick, etc.) which matches the `WildStar` codename.

- Every `.packlog` (1399 of them, 130–706 bytes each) is a plain-text pack manifest describing what was concatenated to build each `.Pack`, e.g.:
  > `File: D:\projects\wildstar\art\Cache\PackData\Xbox 360\Global\979087517.textures / Time: 05.01.2008 04:20:28 / Start: 7732 / Size: 638`

  This is a **hash-name → pack-slot map** that we haven't had before. The cache layout is `D:\projects\wildstar\art\Cache\PackData\Xbox 360\<Region>\<pandemic-hash>.<type>`, where `<type>` is `textures`, `materials`, `meshes`, or `physics` — one hash-named record per asset class per object. **Mining the 1399 packlogs would give us the full hash→friendly-name mapping for every packed asset in the build**, which we can then reuse for retail's `.megapack` (same hash scheme).

Perforce integration is confirmed by the zero-byte marker file `haveitperforce.txt` at the root.

## What's here that retail doesn't have

### 1. Debug symbols and linker maps

| File | Size | Notes |
|---|---|---|
| `Saboteur.pdb` | 45.0 MB | PC PDB for the game symbols. Retail has none. |
| `WildStar_d.pdb` | 66.7 MB | Debug build of the engine tools. |
| `WildStar_p.pdb` | 42.9 MB | Profile build of the engine tools. |
| `WildStar_d.map` / `WildStar_p.map` | 13.4 / 8.4 MB | Text linker maps (grep-friendly section+symbol layout). |
| `Saboteur.xex` / `Saboteur.exe` | 11.1 / 4.6 MB | Ship Xbox exe + the PC devkit exe that produced content-adjacent tools. |
| `WildStar_d.xex` / `WildStar_p.xex` / `WildStar_p-org.xex` | 20.5 / 13.3 / 11.1 MB | Xbox engine tool builds; `-org.xex` looks like the original before a hot-patch. |
| `ReplayCore-xenon.xex` / `ReplayLauncher-Xenon.xex` | 2.7 / 3.8 MB | Standalone replay recorder tool (rebuilt 2008-06-26). |

Combined, this is **~170 MB of PDBs / maps** describing name → function/RVA for the entire engine — the single biggest lift this drop offers.

### 2. Complete Lua source tree

Retail ships one compiled bundle: `LuaScripts.luap` (4.8 MB). This build ships **475 loose `.lua` files** across `Scripts/`, `(Obsolete)/`, and `Branches/WildStar-MS/`:

- `Scripts/Experimental/` — 33 files
- `Scripts/Includes/` — 7 files (`WRAPPER_*.lua`, `__UtilFunctions.lua`, `__SabMissionIncludes.lua`, `List.lua`)
- `Scripts/Missions/` — 143 files (every mission and freeplay contract: `Act_1_Factory.lua`, `Act_3_Mission_5.lua`, `FP_FightClub_A.lua`, `Freeplay_AssassinationLH.lua`, `LeHavre_Freeplay_*.lua`, `MISSION_*.lua`, …)
- `Scripts/Modules/` — 126 files across `Behavior/`, `AttractionPts/`, `Human/`, `Player/`, `Props/`, `ScriptControllers/`, `Triggers/`, `Vehicles/`, plus top-level `GameStateManager.lua`, `GameTips.lua`, `HUDHY.lua`, `TrainMGR.lua`, `VR_Mission*.lua`, `TradeMenu.lua`, `RacingData.lua`, `__MagicNumbers.lua`, `__MissionTypes.lua`, `__SoldierSettings.lua`, …
- `Scripts/Old Map Files/` — 3 files
- `(Obsolete)/` — abandoned Lua under `Areas`, `Missions`, `Global`, `Conversation`, plus `SabTaskConvNode.lua`

This is the ground truth for [saboteur-luacd](saboteur-luacd/) — the compiled bundle we've been decompiling is derived from this exact source tree (fifteen months earlier, but the module names and lua library naming will match closely).

### 3. Pack build manifests

**1399 `.packlog` files, ~750 KB total.** As described above, each maps a `.Pack` file's contents to `D:\projects\wildstar\art\Cache\PackData\Xbox 360\<Region>\<pandemic-hash>.<type>`. This is a ready-made hash-name inventory for every packed object; parsing it into a lookup table is a cheap follow-up.

### 4. Loose per-object assets

Retail bakes everything into `.megapack`. This build has the assets loose, keyed by pandemic-hash filenames:

| Extension | Count | Notes |
|---|---:|---|
| `.wsd` | 582 | Game-template / prop-definition documents (retail: baked into megapacks + one root `.wsd` chain) |
| `.cnv` | 1340 | Conversation records (retail: bundled into `Cinematics/Dialog/` binary) |
| `.cin` | 154 | Cinematic scripts |
| `.fx` | 232 | Shader files |
| `.hkx` | 4499 | Havok animation clips (retail: packed) |
| `.xma` | 3033 | Xbox 360 audio streams (retail: `.pck` bundles) |
| `.bnk` | 65 | Wwise soundbanks (retail: `.pck`) |
| Numeric `<n>.Pack` | ~830 in France/Global | Per-block .Pack files, e.g. `Global/City_Temp1.Pack` + `.Packlog` twin |

### 5. Alternate branch and obsolete tree

- `Branches/WildStar-MS/` — 1221 files, 37 MB. Looks like a Perforce-branch mirror carrying its own `Cinematics/`, `Effects/`, `HudArt/`, `GameObjects.wsd`, `GameOrdnance.wsd`, `GameWeapons.wsd`, and a fleet of `.txt` sequence tables: `AnimEvents.txt`, `AnimSettings.txt`, `ClamberSequences.txt`, `ClamberTransitions.txt`, `GrabSequences.txt`, `GrabTransitions.txt`, `JumpSequences.txt`, `JumpTransitions.txt`, `LODExceptions.txt`. Rich source for how animation-state transitions were authored.
- `(Obsolete)/` — 42 dead files (mission Lua, area configs, one `Global.wsd`, one `SabTaskConvNode.lua`). Worth a diff against Scripts/ for renamed/removed content.

### 6. Test / dev maps

- `AnimationTest/` — full map dir (39 files, 158 MB), plus root `AnimationTest.map`, `AnimationTest.shaders` (2.2 MB), `AnimationTest.ShaderMapping`, `AnimationTest.aical/.aipts/.cinsplines/.defen/.foliage/.hqpoints/.water`. A live test level.
- `SultanOfSwing/` — 3 files, plus root `SultanOfSwing.*` set (`.aical`, `.aipts`, `.cinsplines`, `.defen`, `.foliage`, `.hqpoints`, `.map`, `.shaders`, `.water`). Another test map, mostly stub.
- Root `ParisTest.aical/.aipts` and `ParisTest_Large.aical/.aipts` — leftover Paris test regions.
- Root `TestAnims/` — a single file.

### 7. Dev-config `.txt` and version churn

At the Bin360 root:

- `Asserts.TXT` — assert log (see Provenance).
- `tuner.txt` — 37 KB of runtime tuning parameters (Physics.*, Graphics.Lighting.*, per-material skin/ambient/specular, night-time toggles). Retail has this too but the format+contents are worth diffing.
- `hud_config.txt` — HUD config v2 with `StaticAllocatorSizes` / `StaticAllocatorCounts` for every UI shape (`HelpFont`, `MainOutlinedRects`, `CallView::GpuGraph`, `MainSolidLines`, …). Also embeds engine command line: `ElapsedDriverArgs --wfi`.
- `LODExceptions.txt` — 1106 bytes listing animation names that opt out of LOD (`Sprint_Loop*_LF/RF`, `Mach_Walk_Loop1_LF`, `nazi_rifle_walk_rdy_LF`, …).
- `ItemValues.txt` + `ItemValues.xls` — item price table. **The Excel source ships next to the exported .txt.** Every currency value in the game.
- `ModelInfo.txt` — model → attribute tags (`ZCULL`, `FOLIAGE`) for `City1_*`, `FO_City_*`.
- `MemoryPools.ini`, `MemoryPools_v2.ini`, `MemoryPools_v3.ini` — three revisions of the pool sizing.
- `InputTemplates.txt` through `InputTemplates_v6.txt` — six versions of the input binding template.
- `ai.ini`, `command.ini`, `fonts.ini`, `SoundBanks.ini` — engine config.
- `haveitperforce.txt` — 0-byte Perforce sentinel.

### 8. Root-only additions

- `global.strings` — **78.3 MB**, biggest single non-binary at the root. Global localized string table in loose form (retail has this baked in).
- `Global.map`, `France.map`, `AnimationTest.map`, `SultanOfSwing.map` — linker-`.map`-style but really level maps (`MAP7` magic + level name). Loose.
- `GameTemplates.wsd` — 3.6 MB, plus **`GameTemplates.wsdlog` (755 KB)** — a build log of what went into the template DB. Useful diff pair.
- `GameTemplatesDelta.wsd` (5.4 KB) — a delta patch on top, not seen in retail.
- `GameObjects.wsd`, `GameOrdnance.wsd`, `GameWeapons.wsd`, `foliage.wsd` — small game-DB files.

## Retail vs Bin360 (top-level)

| Dir | Bin360 | Retail | Note |
|---|---:|---:|---|
| `France/` | 6987 MB (3122 files) | 2702 MB (7 files) | Loose per-block `.Pack` files vs 7 packed `.megapack` |
| `Global/` | 3821 MB (3202 files) | 1051 MB (3 files) | Same shape as France |
| `Bink/` | 1342 MB (13) | 892 MB (13) | Both have 13 videos; Bin360's are larger (uncompressed?) |
| `sound/` (Bin360) vs `Sound/` (retail) | 747 MB (3103) | 2278 MB (8) | Retail has *more* audio: full loc packs baked into `.pck`. Bin360 is early build with fewer languages. |
| `AnimationTest/` | 158 MB (39) | — | test map |
| `HudArt/` | 45 MB (506) | — | HUD source art |
| `animations/` | 45 MB (3586) | — | loose animations |
| `Branches/` | 39 MB (1221) | — | alt engine tree |
| `Effects/` | 1.8 MB (215) | — | loose particle FX |
| `Scripts/` | 4.1 MB (313) | — | Lua source |
| `ObjectAnimations/` | 1.4 MB (33) | — | |
| `Replay/` | 6.8 MB (25) | — | replay tool assets |
| `(Obsolete)/` | 292 KB (42) | — | dead content |
| `AnimText/` | 393 KB (54) | 60 KB (6) | Bin360 has ~9× more |
| `FaceFx/` (Bin360) vs `FaceFX/` (retail) | 16 MB (9) | 88 MB (10) | Retail has more face rigs |
| `Cinematics/` | 2.9 MB (1497) | 7.2 MB (9) | Retail bundles them; Bin360 has 1497 loose `.cin`/`.cnv`/etc |
| DLC / G15 / Recordings | — | 173 / 0.02 / 0.3 MB | Retail-only: shipped DLC packs, Logitech G15 LCD assets, small recordings dir |

**Only in Bin360**: `(Obsolete)`, `AnimationTest`, `Branches`, `Effects`, `HudArt`, `ObjectAnimations`, `Replay`, `Scripts`, `SultanOfSwing`, `TestAnims`, `animations`, `sound` (lowercase).

**Only in retail**: `DLC`, `G15`, `Recordings`, `Sound` (capitalized).

**Extensions only in Bin360**: `.aical`, `.aipts`, `.airoads`, `.cin`, `.cnv`, `.fnt`, `.fx`, `.hkx`, `.info`, `.packlog`, `.pdb`, `.railway`, `.shaders`, `.str`, `.trigs`, `.wsd`, `.wsdlog`, `.xdb`, `.xex`, `.xma`, `.xml`, and dozens of loose `.dds`/`.tga` textures.

**Extensions only in retail**: `.ambush`, `.cinpack`, `.dll`, `.freeplay`, `.GpsGraph`, `.hei`, `.kilopack`, `.luap` (compiled Lua bundle), `.materials`, `.megapack`, `.rndnodes`, `.sabbak`, `.spore`, `.ttf`/`.otf`, `.waterctrl`, `.waterflow`. Some of these (`.hei`, `.freeplay`, `.rndnodes`, `.spore`, `.ambush`, `.waterctrl`, `.waterflow`) are formats added between May 2008 and December 2009 — the retail loose-`France.*` file set is bigger and richer than the pre-release set.

## Where the ~6 GB delta lives

`13.29 GiB (Bin360) − 7.25 GiB (retail) = 6.04 GiB`. The user said "over 25 GB decompressed" — that count likely includes the source .rar plus the extracted tree; the extracted tree itself is 13.29 GiB. The delta breaks down as:

| Bucket | Delta | Explanation |
|---|---:|---|
| `France/` per-block `.Pack` files | +4.3 GiB | ~830 numeric-hash `.Pack` files (16 MB avg) that retail concatenates into `.megapack`. Redundant, but the packlog twins are the value. |
| `Global/` per-block `.Pack` files | +2.8 GiB | Same shape — LaVillette1.Pack, Citadel1.Pack, StEustache1.Pack, MoriniFarm1.Pack, City11.Pack, Gondola_Station1.Pack, Cliffs1.Pack, P_Cliff1.Pack, BelleDeNuit1.Pack, … named per-block. |
| `Bink/` | +0.45 GiB | Same 13 videos, larger (looks like less compression). |
| `sound/` | **−1.5 GiB** | Retail has *more* audio (full localization); Bin360 is early build with fewer languages, so this is negative delta. |
| Everything else combined | +~200 MiB | AnimationTest (158 MB) + loose HudArt (45 MB) + Branches (37 MB) + PDBs (170 MB net vs no retail PDBs) + Scripts (4 MB) + loose Effects/animations/CIN. |

## Suggested follow-ups (ranked by cheap-to-do vs value)

1. **Extract symbols from `Saboteur.pdb` (45 MB) and both `WildStar_*.pdb`** — feed to Ghidra / dumpbin. Names for the entire engine and game code path. Highest-value single action. Note the parent `game-files/symbols/` folder already has PDBs but `Saboteur.pdb` there is the *older* 2008-05-20 copy; use `Bin360/Saboteur.pdb` for the current one.
2. **Parse all 1399 `.packlog` files into one hash→friendly-name table**. Trivial (they're plain text). Immediately unlocks retail's `.megapack` object naming.
3. **Diff `Scripts/` against the current `saboteur-luacd/` decomp output**. Ground truth for module/function names that our decomp only guesses at.
4. **`WildStar_d.map` / `WildStar_p.map` grep pass**. Text linker maps — cheapest possible symbol grab. Section + RVA + demangled name for every symbol.
5. **Diff `Bin360/GameTemplates.wsd` (3.6 MB) against retail's baked template DB**. May reveal earlier property types + template IDs that got renamed. `GameTemplates.wsdlog` and `GameTemplatesDelta.wsd` give even more.
6. **Diff `(Obsolete)/` against `Scripts/`** — what got cut between May 2008 and ship. Doubles as a "what mission ideas existed" survey.
7. **Compare `Bin360/global.strings` (78 MB) against retail `GameText.dlg`** — earlier string set may still be in-use hashes that we haven't matched. See [gametext-dlg-format](../../.claude/projects/c--Users-Shadow-Desktop-notes-on-reversing-the-sabetour/memory/gametext-dlg-format.md).
8. **Parse `Branches/WildStar-MS/*Sequences.txt` and `*Transitions.txt`** — animation state machine authoring format. Could inform how retail's runtime FSM is structured.
9. **Ingest `Asserts.TXT` + `haveitperforce.txt` + packlog paths** into `docs/lineage_and_divergence.md` as first-class provenance evidence.
10. **Don't run it.** Xbox 360 xex without a devkit is not useful; the loose PC exes (`WildStar_*.exe`) are engine tools that need the WildStar toolchain to boot. This is diff material, not a runnable target — see [gog-install-is-the-only-target](../../.claude/projects/c--Users-Shadow-Desktop-notes-on-reversing-the-sabetour/memory/gog-install-is-the-only-target.md).
