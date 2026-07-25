# The Saboteur modding tools

A toolset for modding **The Saboteur** (2009), built on a byte-level reverse-engineering of the
game's own formats. Every tool is a standalone Windows binary with no installer and no runtime
dependency; every format claim behind them is validated against retail data and, where it matters,
against the disassembled engine loader.

**Download:** the [Releases](../../../../releases) page.
`saboteur-modding-tools-<version>-win64.zip` has every shipped tool (see "Not shipped" below);
`sab_workshop-<version>-win64.zip` is just the GUI. These ship **no game data** — you need your own
copy of the game.

Every tool prints its usage when run with no arguments.

## I want to…

| …do this | use | guide |
|---|---|---|
| Look at a character, its rig, and play its animations | `sab_workshop` | — |
| Replace a texture or add a custom icon | `sab_pack` → `sab_dtex` → `sab_gametemplates` | [replace-a-texture](workflows/replace-a-texture.md) |
| Change UI text, a mission name, or a subtitle | `sab_workshop` / `sab_gametext` (bin) | [add-ui-text](workflows/add-ui-text.md) |
| Add an NPC or vehicle to a mission | `EditNodes.pack` — inspect with `sab_probe editnodes` | — |
| Retune an object — car speed, weapon damage, a light | `sab_gametemplates` | — |
| Put a new character in the game | `sab_mesh` + `sab_skeleton` + `sab_pack` | [port-a-character](workflows/port-a-character.md) |
| Export an animation to glTF | `sab_havok65` (+ `sab_animmeta`) | — |
| Check a mod will actually load, before launching | `sab_validator` | — |
| Find out why a mod *didn't* load | `sab_validator`, then `sab_probe` | — |
| Watch engine state in a running game | `sab_asi` | — |

## The tools

### Mod authoring

| tool | what it does |
|---|---|
| [`sab_workshop`](../../tools/sab_workshop/README.md) | **The GUI.** Character/animation viewer (textured, GPU-skinned, orbit camera) plus mod-editor pages for Templates, GameText and Icons. Start here. |
| [`sab_pack`](../../tools/sab_pack/README.md) | Read and **write** `.megapack` archives. Builds the patch-override packs that mods ship as. |
| [`sab_dtex`](../../tools/sab_dtex/README.md) | DTEX ⇄ DDS textures, with byte-faithful repack. |
| `sab_gametext` (a `sab_formats` bin) | `GameText.dlg` authoring — set/add UI strings and DNEC subtitles. Inspect read-only with `sab_probe gametext`. |
| [`sab_gametemplates`](../../tools/sab_gametemplates/README.md) | `GameTemplates.wsd` (AULB) — the object-definition layer: cars, weapons, props, lights. |
| [`sab_sbla`](../../tools/sab_sbla/README.md) | Splice an asset into a multi-asset `ALBS` sub-pack and fix the directory chain. |
| [`sab_validator`](../../tools/sab_validator/README.md) | Parses a mod the way the engine's mount path does and reports what would fail. **Run this before you launch.** |

### Assets in and out

| tool | what it does |
|---|---|
| [`sab_mesh`](../../tools/sab_mesh/README.md) | Extract skinned character geometry → `SMSH` + glTF. |
| [`sab_skeleton`](../../tools/sab_skeleton/README.md) | Extract a character skeleton (bone hierarchy + bind pose) from a MESH. |
| [`sab_havok65`](../../tools/sab_havok65/README.md) | Decode Havok 6.5 spline-compressed animation → glTF. Decodes all 2,214 retail clips. |
| [`sab_animmeta`](../../tools/sab_animmeta/README.md) | The `ANIM` track→bone binding that rigs a decoded clip onto a named skeleton. |
| `saboteur_audio` | 1KCP Wwise package → `.wem` → WAV. Pipeline documented in [`docs/formats/audio_1kcp.md`](../formats/audio_1kcp.md). **Not in the release bundle** — build it from source. |

### Inspection

| tool | what it does |
|---|---|
| [`sab_map6`](../../tools/sab_map6/README.md) | Read `MAP6` world/asset registries (`global.map`, `<region>.map`). |
| [`sab_probe`](../../tools/sab_probe/README.md) | Read-only questions about game assets — the rig/skeleton debugger. |
| [`sab_asi`](../../tools/sab_asi/README.md) | 32-bit DLL injected into a running `Saboteur.exe`; reads live engine state without a debugger. |

Not shipped: `sab_formats` (the shared codec library the others are built on), `sab_poc` and
`sab_megapack_key` (one-shot proofs, not workflow tools), and `saboteur_audio` *(added 2026-07-24 —
the release job discovers crates with a `tools/sab_*` glob, which `saboteur_audio` does not match, so
it is excluded even though it is not on the workflow's explicit skip list)*. Build those from source:
`cargo build --release --manifest-path tools/<crate>/Cargo.toml`.

## Ground rules

1. **Never overwrite a base archive.** Every mod here ships as a `patch*.megapack` override, which
   mounts above the base pack and is uninstalled by deleting one file. Keep your install clean.
2. **Validate before you launch.** `sab_validator` catches the authoring mistakes that otherwise
   present as a silent crash at load, or as an invisible character.
3. **Hashes are the joins.** Assets, properties, bones and text ids are all keyed by
   `pandemic_hash(name)`. Most tools expose a `hash` subcommand; that is usually how you connect
   two files that never mention each other by name.

## Format specs

The byte-level specs behind these tools are in [`docs/formats/`](../formats/) — read those if you
are writing your own tool, chasing a parse failure, or want to know how confident a given field is.
The rest of [`docs/`](../) is the reverse-engineering record, not tool documentation.
