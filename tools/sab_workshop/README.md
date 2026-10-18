# sab_workshop

A native **wgpu + egui** workbench for *The Saboteur*'s assets. It reads **two builds the same way**:
the retail PC release (GOG / Steam) and the 2008-05 Xbox 360 prototype. Point it at either folder and
every page behaves identically — the differences between the builds live in one file each under
[`src/sources/`](src/sources/), behind one interface ([`src/source.rs`](src/source.rs)).

- **Inspect** (page 1) — models assembled from the build's own templates, textured from its own
  material data, every animation clip searchable and played on the loaded rig with GPU skinning, the
  rig as a tree, the model's texture pool as a contact sheet, a Materials panel to rebind any slot,
  and a lossless **export bundle** of what is on screen.
- **Strings · Objects · Icons** (pages 2–4) — the game's text, template table and texture pool. On
  retail they are also the mod editor (stage edits, publish to a DLC slot); the prototype is browsed
  and exported only (see [The editor pages](#the-editor-pages)).

## Run

> **Use `--release`.** A debug build is far too slow for the pack sweeps and BC decoding.

**A game folder is the only input.** No generated files, no repo checkout. The folder is **never
guessed**: set it once on the **Settings** page (or pass `--game`). Either build qualifies —
the retail install (it holds `Global/`, `France.materials`, `Saboteur.exe`) or the 2008 prototype
(the folder holding `Bin360/`, or `Bin360` itself under any name). It is saved in `settings.json`
under `%APPDATA%\sab_workshop\` on Windows and `$XDG_CONFIG_HOME/sab_workshop/` (else
`~/.config/sab_workshop/`) on macOS and Linux.

```
cd tools/sab_workshop
cargo run --release
```

Opening runs on a worker: the window is up at once, the browser shows a spinner until the build's
assets are listed, and the startup model appears as soon as it is assembled.

Overrides (none is required):

```
sab_workshop --game "D:/Games/The Saboteur"    # a build in an unusual place
sab_workshop --boot Mattias                    # open on an asset other than the build's player
sab_workshop --mesh <ported>.smsh              # inspect a loose SMSH, rigged on the player's rig
sab_workshop --mesh <m>.smsh --skel <r>.skel   # ...and with its own rig file
```

- `--boot <token>` — the first asset whose name contains the token (case-insensitive). Without it the
  app opens on the build's player: the one `Player` template's `FxBodySetup` (`FBS_RS_Sean` in both
  builds).
- Retail file overrides: `--megapack`, `--palettes`, `--wsao`, `--pack` (Animations.pack), `--index`
  (a generated `anim_bone_map.json` catalog), `--dtexdir` (loose `<hash>.dtex` textures, consulted
  before the packs). They name retail files, so opening the prototype with any of them is an error.

Headless commands (no window):

- `--export <asset> <out>` — write the asset's bundle, textured from the build's material data.
- `--selftest [N]` — open the build, boot its player, play the N-th clip on its rig, resolve its
  textures. Exits non-zero when anything fails, including an untextured boot model.
- `--texcheck [name]` — assemble and texture every asset whose name contains `name`; per-submesh
  bindings and a coverage tally. Exits non-zero when any asset fails to assemble or resolve.
- `--anim-sweep [N]` — decode N clips on the player's rig and flag any that pose a non-root bone
  implausibly. Exits non-zero when one is flagged or fails to load.
- `--source-info <dir>` — what the folder fingerprints as, and what opening it finds.

### Platforms

Windows, macOS and Linux build from the same source; the only OS-specific code is the positioned
file read in `pack.rs` (a `cfg(unix)` / `cfg(windows)` pair; any other target is a compile error).

| | compiles (`cargo check --all-targets`) | runs |
|---|---|---|
| Windows x86_64 (MSVC) | ✅ | the original development platform |
| macOS (Apple Silicon, Intel) | ✅ | ✅ Apple Silicon: the GUI (Metal) and every headless command |
| Linux x86_64 | ✅ | not yet run on a Linux machine |

Off Windows the UI uses egui's bundled faces instead of Segoe UI / Courier New, which are read from
`C:/Windows/Fonts` only when present.

### Tests that read game data

They are built only with the `retail` feature and read both builds from the repo-root
`.sab-local.toml` (`retail_dir`, `proto2008_dir`), failing with the missing key or path:

```
cargo test --release --features retail
```

Each game-gated test runs on **both builds** where the behaviour is shared: every character
assembles one variation per setup slot, the player boots as `FBS_RS_Sean`, every clip loads (2008:
all but the 30 loose clips `LoadAnimFile` rejects), clips bind to the player's rig by bone hash, a
character's bundle round-trips with its textures, every template pair round-trips through its editor.

## Controls

| Input            | Action                          |
|------------------|---------------------------------|
| LMB drag         | orbit (rotate)                  |
| MMB drag         | pan                             |
| mouse wheel      | zoom                            |
| Space            | play / pause                    |
| F10              | export the loaded asset as a bundle |
| 1–5              | switch page                     |
| Esc              | quit                            |
| Navigator (left) | `models` · `anims` · `textures` (the loaded model's pool) · `rig` (its bone tree), with a filter |
| Inspector (right)| Model card · Parts (click one to load it alone; its record is described) · Clip · the selected texture or bone · **Materials** |
| Verb bar (bottom) | **Export bundle** + the destination it writes to |
| Transport (bottom)| play/pause, loop, speed, time scrubber, grid / textures / lock-root toggles |

## How a model is made

1. **Assets come from the build's templates** ([`assets.rs`](src/assets.rs)). An
   `FxHumanBodySetup` lists slots of weighted variations of `FxHumanBodyPart` / `FxHumanHead`
   templates; each part names its `HighDetailModel` (level 0) and `LowDetailModel` (level 1), and the
   setup's combined model is level 2 (the first of slots 0–4 whose shown part names a
   `CombinedLODModel`, else the setup's own — [gametemplates.md](../../docs/formats/gametemplates.md)).
   Vehicles, weapons and props name their meshes directly. Meshes no template claims are `Ungrouped`;
   the prototype's world packs are listed under `World`, one asset per pack.
2. **A template's reference to a mesh resolves the build's way** (`GameSource::find_mesh`). Retail
   stores 6,807 mesh records under 1,559 names and every record of a name is a byte-identical copy
   (`retail_pc::tests::repeated_mesh_names_are_identical_copies`), so a name has one answer. The
   prototype loads a template's meshes from the pack named after it
   ([2008_templates_and_text.md](../../docs/2008_templates_and_text.md)): its own pack first, else the
   only copy in `Global/`; differing copies with none in the template's pack are reported, not picked.
3. **Parts merge onto one rig keyed by bone name hash** ([`meshload.rs`](src/meshload.rs) `merge`),
   the bind pose corrected from the parts' own inverse binds. Each level of detail is one contiguous
   span of the index buffer: a prim's level is its part's assembly level, or for a part at level 0
   its own drawcall's in-mesh level (retail `DrawCall +0xe & 7`, 2008 `OdinSegment` lod). A part at
   level 1 or 2 that also carries in-mesh levels is an error, not a guess. The viewport draws the
   finest level the model has; the bundle writes every level.
4. **Textures bind from the build's material data** (`GameSource::resolve_textures`). Retail: each
   submesh's drawcall material → its WSMA record in `France.materials` → `[diffuse, spec, normal,
   mask]` DTEX name hashes, found in the parts' own bundles, then every bundle of `Dynamic0` and
   `Palettes0`; a texture's role is the slot it is bound in. 2008: the material's `Color-Pass` —
   sampler 0 is the diffuse, the first normal-kind sampler the normal
   ([2008_materials_format.md](../../docs/2008_materials_format.md)); its other textures join the pool
   for the picker. Nothing is bound by name.
5. **Damage states come from the template's damage parts** ([damage_and_destruction.md](../../docs/damage_and_destruction.md)).
   A destructible's mesh holds its intact and its damaged pieces; each part names a bone and the
   group state it draws in (retail `Pristine`, 2008 `0xb7b68d44`, default every state), and a
   drawcall belongs to the nearest part bone at or above its bone. The viewer draws one state —
   intact first; the Model card switches states — and the bundle writes one object per state.
6. **Clips bind by bone name hash** ([`clip.rs`](src/clip.rs) `bind_to_rig`), as the engine's
   `FindBoneIndex` does. A retail clip whose `ANIM` record stores bone indices (every bound value below
   the biped's bone count) has them turned into the player rig's bone hashes when the build opens;
   exotic-skeleton clips already store hashes. A clip is listed as on the rig when every bound track
   names one of the loaded model's bones; *show all* lists the rest, which play with their unmatched
   tracks at bind pose. Changing the model rebinds the playing clip.

Retail has 984 assets and the prototype 3,381; every one of them assembles and resolves
(`--texcheck`). Two mesh shapes besides the skinned one load: a record with `numBones0 <= 1` has no
skeleton block and its tail follows the 244-byte header (SaboteurToolset `mesh_to_gltf.cpp`
`AppProcessFile`), and a record with no streams and an empty `.dat` is a bare rig (`ragdoll_b`,
`saboteur_fm`). Every MESH tail must end exactly at the end of its body.

## Materials: the build's own binding, with a picker over it

The Materials panel lists every submesh with its drawcall material hashes, its level, how its binding
was arrived at (`bound` by the material data or by you, `seeded` for a saved binding not marked a
decision, `unresolved`), and one picker per slot (diffuse / spec / normal / mask) over the model's
whole pool, role-matching entries first. A pick re-renders at once and is saved — beside the mesh for
a `--mesh` file, else in `materials/<model>.materials.json` next to `settings.json`, prototype models
filed under `proto2008_<model>` — and on the next load it outranks what the material data answered.

## Export bundles

`Export bundle` (verb bar, `F10`, or right-click an asset in the browser) writes the loaded model to
`<export dir>/<name>/`, re-assembled from the same build parts. **An editable format can only carry
what it understands**, so the bundle keeps both halves:

```
workshop_export/FBS_RS_Sean/
  model.glb        skinned bind pose, ONE MESH OBJECT PER LEVEL OF DETAIL (<name>_LOD0, _LOD1, …)
                   — and per damage state for a destructible (<name>_LOD0_state0, _state1) — on one
                   shared skin, ONE PRIMITIVE PER SUBMESH, each bound to its material
  textures/*.png   every bound slot (diffuse / normal / spec / mask) decoded and editable
  raw/             every source part's record exactly as the build stores it (retail `.msha`;
                   2008 `.zlib` or `.variant`) — the preservation guarantee
  manifest.json    the reassembly map: build and per-part location, submesh → level + material
                   hashes → per-slot texture → how we knew, the bone table with name hashes, `lods`,
                   `lod_notes` (references the build does not ship, parts that did not decode), and
                   the model's whole texture pool
```

- Bundle version 3: `source` names the build and each part's location, one entry per part and level;
  `lods[]` and `submeshes[]` carry `damage_state`, and `damage_parts` lists the template's parts.
- Each level is its own glTF scene (`LOD0` is the default) holding the skeleton and that level's
  object. Blender's importer makes them collections; toggle one off to see another.
- Textures are referenced by relative `uri`, not embedded — repainting a skin is editing a PNG.
- Bind pose only; no animation channels. A bare rig exports its bone hierarchy with no mesh.
- Runs on a **worker**; the viewport stays live and a progress card shows what is in flight.
- The bundle carries the bindings on screen. Right-clicking an unloaded asset loads it, then exports
  once its textures land. A `--mesh` model has no build parts to re-assemble and is not exportable.
- The destination is a click on the verb bar's `→ path`; it persists in `settings.json`.

Blender's importer reports fewer triangles than `lods[].triangles` for some characters: the game
ships double-sided triangles as a back face that reuses the front face's three vertices, and a
Blender mesh cannot hold two faces on the same vertices. The `.glb` carries every triangle.

## The editor pages

All three read the open build through the same interface: the language files it ships
(`GameText.dlg`, version 5 on retail and version 3 on the prototype — the prototype ships English
only), its template table (`AULB` / `BLU2`, values shown in the file's own byte order), and
its texture pool (retail: `Palettes0`; prototype: every texture of every pack plus the loose `.dds`
files). Every page searches and exports (strings and templates on screen, textures as PNG or DDS).

**Retail** is also the mod editor: the changelist records every edit with what it was, and Publish
writes a DLC overlay (edits of existing strings patch the base `GameText.dlg`, keeping retail as
`.sabbak`; see [`editor.rs`](src/editor.rs) for why). **The prototype is read-only**: its side panel
says so where the changelist would be, and an attempt to stage an edit is refused with a note. Its
template values are read by width alone, since the property types were measured on retail.

## Architecture

```
main.rs        CLI / config (--game/--boot/--mesh and the retail overrides), headless commands
source.rs      GameSource: the one interface every page talks to; version detection
sources/       retail_pc.rs, prototype2008.rs (+ prototype2008_anim.rs): one build each
assets.rs      template-assembled assets; the build's player
boot.rs        assembling a model; the startup model; a loose --mesh model
meshload.rs    retail MESH decoding; merge (one rig by bone hash, levels of detail laid out)
clip.rs        AnimClip, and binding a clip's tracks to a rig by bone hash
anim_index.rs  the retail clip catalog: Animations.pack's ANIM block (or a generated JSON)
resolve.rs     texture pools, per-slot bindings, the materials sidecar
bundle.rs      the export bundle (glTF writer + raw records + manifest)
app.rs         winit event loop; workers for open / assemble / textures / export; the egui shell
editor.rs      the Strings / Objects / Icons pages
render.rs      wgpu 0.20: skinned-mesh pipeline, per-submesh texture bind groups, grid
skinning.rs    pose composition: locals → world → jointMatrix = world · inv_bind
havok.rs       Havok 6.5 packfile + spline decoder (retail clips)
formats.rs     Bone, Smsh, Prim, submesh_cover
dtex.rs        retail DTEX decode;  pack.rs  megapack index reader;  wsao.rs  France.materials
gui.rs         winit-0.29 → egui-0.28 event bridge + egui-wgpu paint; `theme`
```

Data is right-handed, +Y up, metres (Havok/glTF) — the camera matches (`look_at_rh` +
`perspective_rh`).

## Provenance

`havok.rs` and `formats.rs` are copied from `tools/sab_havok65`; `dtex.rs` and `pack.rs` from
`tools/sab_dtex` and `tools/sab_pack`; the 2008 formats come from `sab_formats::proto2008`. The
egui↔winit bridge in `gui.rs` mirrors the Mercs2 workshop's hand-rolled bridge (egui-winit 0.28 pins
winit 0.30, which conflicts with winit 0.29); `gui::theme`, the shell and `bundle.rs` with its
verb-bar / worker / progress-card flow are adapted from that workshop. The BC1/BC3 block decoders
come from its `texpng.rs`.
