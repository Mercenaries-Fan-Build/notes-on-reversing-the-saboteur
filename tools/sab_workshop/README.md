# sab_workshop

A native **wgpu + egui** modding workshop for *The Saboteur*. It has two halves:

- **Inspect** (page 1) — the character/animation viewer: a merged skinned mesh + skeleton, textures
  resolved straight out of the megapack, every clip searchable, decoded on demand from
  `Animations.pack` and played back **textured** with real-time GPU skinning under an orbit camera.
- **Templates · GameText · Icons** (pages 2–4) — the mod editor, built on the byte-verified
  `sab_formats` codecs. See [The mod-editor pages](#the-mod-editor-pages) below.

## The mod-editor pages

Pages 2–4 replace the old Textures/Materials/Rig tabs with a data editor over the game's own files:

- **Templates** — load a `GameTemplates.wsd` (AULB), search templates by name/type, and edit any
  property pair. A 4-byte value shows as int / float / hash; known property names (`Name`, `Model`,
  `Texture`, …) are resolved. Set a value as an int, float, raw hash, or **texture name** (which is
  hashed with `pandemic_hash` — that is exactly how a template references a texture). Save writes the
  file byte-faithfully.
- **GameText** — load a `Cinematics/Dialog/<Lang>/GameText.dlg`, browse/search all UI strings and VO
  subtitles, edit any string (any length), and **add a brand-new UI id** (keyed by
  `pandemic_hash("File_Text.Key")` — no Lua registration needed). Save re-emits the container and
  rebases its trailing `DNEC` section automatically.
- **Icons** — scan a megapack for its DTEX texture names and their `pandemic_hash`, and hash any name
  you plan to pack, so you can wire a custom icon into a template's texture value. (Packing the DTEX
  itself is done with `sab_dtex` / `sab_pack`; the format chain is documented in
  `../../docs/formats/gametext.md` and `../../docs/formats/gametemplates.md`.)

The original character/animation viewer is unchanged and is page 1.

![pipeline: load → resolve textures → decode-on-select → skin → render]

## Run

> **Use `--release`.** A debug build is far too slow for the 715 MB megapack sweep + BC decode
> (minutes vs ~1.4 s).

**A game install is the only input.** No generated files, no repo checkout — the startup character,
its rig, its textures and the whole clip list are read out of the packs. The install is found
automatically (GOG / Galaxy / Steam / Pandemic layouts) or set once on the **Settings** page.

```
cd tools/sab_workshop
cargo run --release
```

```
sab_workshop.exe    # or just run the built binary, from anywhere
```

Everything below is an override, not a requirement:

```
sab_workshop --game "D:/Games/The Saboteur"    # an install in an unusual place
sab_workshop --boot Mattias                    # open on someone other than Sean
sab_workshop --mesh <ported>.smsh              # inspect a loose SMSH, rigged from the install
sab_workshop --mesh <m>.smsh --skel <r>.skel   # ...and with its own rig file
sab_workshop --index anim_bone_map.json        # a generated clip catalog instead of the pack's
```

- `--boot <token>` — case-insensitive mesh-name token picking the startup character.
- `--megapack <path>` / `--pack <path>` — point at a single archive directly.
- `--char <token>` — which DTEX bundles are this character's (the `--mesh` texture path only).

Other flags:

- `--help` — usage + controls.
- `--selftest [N]` — **headless** verification (no window): assembles the startup model, decodes the
  N-th playable clip, runs skinning over several frames, resolves textures + prints the
  per-submesh assignment, and prints sanity stats. Useful on a machine with no display / GPU.
- `--texcheck [name]` — headless: bind every assembled asset's textures and report coverage.
- `--anim-sweep [N]` — headless: decode N clips and flag any that pose a limb implausibly.

## Controls

| Input            | Action                          |
|------------------|---------------------------------|
| LMB drag         | orbit (rotate)                  |
| MMB drag         | pan                             |
| mouse wheel      | zoom                            |
| Space            | play / pause                    |
| F10              | export the loaded asset as a bundle |
| Esc              | quit                            |
| Navigator (left) | search box + click a clip to play |
| Inspector (right)| Character stats · **Materials** (per-submesh texture picker) · Clip details |
| Verb bar (bottom) | **Export bundle** + the destination it writes to |
| Transport (bottom)| play/pause, loop, speed, time scrubber, grid + textures toggles |

## Export bundles

`Export bundle` (verb bar, `F10`, or right-click an asset in the browser) writes the loaded character
to `<export dir>/<name>/` — the same lossless shape `mercs2_workshop` settled on, because the reason
for it is the same: **an editable format can only carry what it understands**, and the Saboteur MESH
record still holds things we have not reversed. So the bundle keeps both halves.

```
workshop_export/CH_AL_SeanDevlin_01_FX/
  model.glb        skinned bind pose, ONE PRIMITIVE PER SUBMESH, each bound to its material
  textures/*.png   the bound skins, plus their _N/_S siblings, BC-decoded and editable
  raw/*.msha       every source part's original on-disk record, byte-exact (the guarantee)
  manifest.json    the reassembly map: submesh → material hashes → texture → how we knew,
                   the bone table with name hashes, the source parts, and the FULL texture
                   pool the character offered (exported or not)
```

- Textures are referenced by relative `uri`, not embedded — repainting a skin is editing a PNG in
  place, not a re-export round trip.
- Bind pose only; no animation channels. The clip set is a separate axis (see the transport).
- Runs on a **worker**: assembling + BC-decoding takes seconds and used to freeze the window. The
  viewport stays live; a progress card shows what is in flight.
- Exports **what is on screen**, because the bundle carries the material bindings and those only
  exist for the loaded model. Right-clicking an unloaded asset loads it first, then exports once its
  texture resolve lands — the verb bar says so while that is pending.
- The destination is a click on the verb bar's `→ path`; it persists in `settings.json`. Empty means
  `workshop_export/` beside the working directory — no machine path is baked in.

## What it does

1. **Assemble** the startup character from `Dynamic0.megapack`: pick its parts (one per body slot,
   no LODs, head variants collapsed), inflate each `MSHA` → MESH, merge the geometry and build ONE
   union rig keyed by bone name-hash, with the bind pose corrected from the parts' own inverse-bind
   matrices. Same code path a click in the model browser takes — [`boot.rs`](src/boot.rs).
2. **Cover** the index buffer with a non-overlapping per-material draw list. `sab_mesh` emits one
   prim per *drawcall*, so ranges overlap (a coarse "whole" primitive plus its split children, and
   several materials/passes on one range). `formats::submesh_cover` keeps only **leaf** ranges and
   collapses duplicates — for Sean that's 34 prims → **14 submeshes** tiling all 114,402 indices.
3. **Resolve textures** the way the engine does: each submesh's drawcall material hash → its WSMA
   record in `France.materials` → the colour texture's name-hash → the DTEX record carrying it, found
   in the parts' own bundles first and then across the packs. Where the table has no answer, fall
   back to the name-suffix seed (HD→head, UB→jacket, LB→pants, GR→hand, HAT→hat). Decoding is lazy —
   only bound textures are BC-decoded. A saved pick overrides both (see below).
4. **List** every clip from the `ANIM` block at the front of `Animations.pack` — name, duration and
   the ordered per-track bone list, straight out of the install. By default only clips authored on
   this rig are shown (per-track values that are bone INDICES into it, 2155 of 2214); tick *show all*
   to include the rest (59 exotic-skeleton clips store bone name-HASHES). Searchable by name.
5. **Play** the selected clip: on click, the packfile is re-parsed and the clip's
   `hkaSplineCompressedAnimation` (the N-th spline anim in file order, where N is the clip's
   `index`) is decoded into a self-contained `SplineAnim`. Each frame it is sampled at the
   current time to per-track local transforms.
6. **Skin** on the GPU: the CPU composes `world[i] = world[parent] * local[i]` and uploads
   `jointMatrix[i] = world[i] * inv_bind[i]` (191 mat4) to a storage buffer; the vertex shader
   does `skinnedPos = Σ weightₖ · jointMatrix[jointₖ] · pos`.
7. **Render** one draw per submesh, each binding its own diffuse (1×1 white when unassigned),
   modulated by two-sided lambert, with a ground grid, under an orbit camera.

## Materials: the engine's own binding, with a picker over it

A prim carries a `materialHash` — a `pandemic_hash` of a name in the **WSAO** material library, which
names that material's textures. That library IS in the retail PC install, as the loose file
`France.materials` at the game root (an earlier note here said it was absent; it was looked for
inside the archives, and it is not in them). So the binding is the game's own:

`drawcall materialHash` → WSMA record → WSTX slot 0 (colour) → DTEX by name-hash.

That covers every submesh of every assembled character (`--texcheck FBS_RS_Sean`: 186/186 bound).
Two things sit on top of it:

- a **name-suffix auto-seed**, used ONLY where WSAO cannot answer (a submesh with no material, or a
  material absent from the table) — a heuristic, and wrong wherever one part uses several textures.
- the **Materials picker** in the inspector: reassign any submesh to any texture in the character's
  bundles (accessories like `CH_AC_Eyes_*` / `CH_AC_Mouth` included). A pick is a decision, so it
  outranks both of the above and persists — beside the mesh for a `--mesh` file, else in
  `%APPDATA%/sab_workshop/materials/<model>.materials.json`, keyed by model so one character's
  edits never land on another's.

## Architecture

```
main.rs        CLI / config (--game/--boot/--mesh overrides), --selftest, --help
boot.rs        the startup model: a character assembled from the install, or a loose --mesh file
bone_names.rs  bone name-hash -> name (a MESH stores only the hash)
app.rs         winit event loop; owns all state; load → resolve textures → decode-on-select →
               skin → render; the egui shell (command bar, navigator, inspector, status,
               transport); the Materials picker + sidecar persistence; mouse camera control
render.rs      wgpu 0.20: surface/device, depth, skinned-mesh pipeline (WGSL skinning in the
               vertex shader, diffuse sampled over UV), per-submesh texture bind groups + white
               fallback, grid pipeline, camera uniform + joint storage buffer
skinning.rs    pose composition: bind/anim locals → world matrices → jointMatrix = world·inv_bind
camera.rs      orbit camera (yaw/pitch/distance, look_at_rh + perspective_rh)
gui.rs         hand-rolled winit-0.29 → egui-0.28 event bridge (+ OS clipboard & cursor delivery)
               + egui-wgpu paint; `theme` — the shared visual system (palette, fonts, widgets)
havok.rs       COPIED from tools/sab_havok65: AP0L + Havok 6.5 packfile parse, spline decoder
               (Packfile, parse_ap0l, read_spline_anim, SplineAnim::sample_at → QsTransform)
formats.rs     COPIED from tools/sab_havok65: read_smsh, read_skel, Bone, Smsh; + the prims block
               (Prim) and `submesh_cover` (the non-overlapping per-material draw list)
dtex.rs        COPIED from tools/sab_dtex (parse/payload/mips/find_records) + BC1/BC2/BC3 and
               uncompressed decoders (BC1/BC3 from the Mercs2 workshop's texpng) → CpuTexture
editor.rs      the mod-editor pages (Templates / GameText / Icons) over the sab_formats codecs;
               each is a self-contained CentralPanel with a path field, Load/Save, and edit forms
pack.rs        COPIED from tools/sab_pack: megapack index reader + pandemic_hash
resolve.rs     character texture resolution: bundle sweep → DTEX records → role classify →
               body-part auto-seed → materials sidecar load/save (per model)
anim_index.rs  the clip catalog: the pack's own ANIM block (or a generated anim_bone_map.json)
```

Data is right-handed, +Y up, metres (Havok/glTF) — the camera matches (`look_at_rh` +
`perspective_rh`).

### Known gaps / shortcuts

- **Materials resolve through WSAO** (`France.materials`) — see *Materials* above. The name-suffix
  heuristic only fills in where the table has no answer, and any slot can be fixed by hand.
- **Diffuse only.** The resolved `_N`/`_S`/`_WM` maps are listed and pickable but not yet used by
  the shader (no normal/spec lighting); textures are uploaded as `Rgba8Unorm` (no sRGB decode), to
  stay consistent with the flat/grid passes.
- **Startup is non-blocking and progressive.** The megapacks are **memory-mapped** (`open` is instant —
  no 715 MB read, only touched pages fault in), and the heavy work runs on a **worker thread**
  (`background_load`) that **streams results in stages** (`BgMsg`): clip list → model list → character
  textures → megapacks (click-to-load) → `Animations.pack` (playback). The window is interactive at once
  and each section fills in as it lands.
- **The model list is near-instant.** `list_meshes` walks the **MSHA chain** — the mesh headers in a
  bundle are contiguous (`next = pos + 276 + compSize0 + compSize1`), so it reads only the 276-byte
  headers (~2 MB for all 6807 meshes) and skips every compressed blob. Verified bit-for-bit against the
  old full-file scan (6807 == 6807 across 759 bundles).
- **The character texture scan won't freeze the UI.** Finding the boot character's textures needs a
  full-content token scan (the name lives deep in its bundle), so it can't be windowed — but it now
  reads each sub-pack via a **buffered, reused-buffer file read** (`Megapack::read_into`) instead of
  mmap-faulting all 714 MB into the working set, and **yields** periodically. It runs as the last
  background stage. **Clicking a model** in the browser is the same: the mesh loads and shows instantly
  (untextured), and its texture resolve — which for a character is that same whole-pack token scan —
  runs on a worker (`resolve_model_textures`) and streams in, so a click never blocks the UI. Debug
  builds are far slower at BC decode — use `--release`.

## The model browser (assembled assets, from the game's own data)

The Inspect navigator lists **assembled assets**, not raw mesh parts — and the grouping is the game's,
not a heuristic. `src/assets.rs` reads `GameTemplates` and follows the real references:
`FxHumanBodySetup` (a character) → `FxHumanHead` + `FxHumanBodyPart` → mesh; `Weapon` / `CAR` / `Prop` /
`Ammo` reference their meshes directly. So the 6807 mesh parts fold into **979 assets** — Characters
(268), Props (225), Weapons (155), Vehicles (100) — plus **Ungrouped** (231 meshes no template claims;
never hidden). Proven: `FBS_RS_Sean` → FX, FM, HD, UB, LB, HAT, GR, Hair.

- **Click an asset** → loads its primary part into the viewer and fills the right inspector's **Parts**
  panel with every part; **click a part** to load/inspect it individually.
- **Right-click an asset → Move to group** to override its category; saved to
  `sab_workshop_model_groups.json` next to the game, winning over the template default on later runs.
- The clip list re-parses the Havok packfile on each *selection* (cheap; it only resolves the
  object's `hkArray`s). Per-frame sampling does not re-parse.
- The Havok decoder's multi-block path and non-THREECOMP40 rotation quantizations are present
  but untested — the Saboteur corpus is uniform single-block THREECOMP40 (see `havok.rs`).
- No animation blending / no playback of multiple clips at once.

## Provenance

The format code in `havok.rs` and `formats.rs` is copied verbatim (CLI/diagnostics stripped)
from `tools/sab_havok65` — the validated decoder + readers; `dtex.rs` and `pack.rs` likewise from
`tools/sab_dtex` and `tools/sab_pack`. The egui↔winit bridge in `gui.rs` mirrors the Mercs2
workshop's hand-rolled bridge (egui-winit 0.28 pins winit 0.30, which conflicts with winit 0.29),
including its clipboard/cursor delivery; `gui::theme` and the command-bar / navigator / inspector /
status / transport shell are adapted from the Mercs2 workshop's rewrite; `bundle.rs` and the
verb-bar / background-worker / progress-card export flow are ported from that workshop's `bundle.rs`
and `Exporter` (its glTF is a `model.gltf` + `model.bin` directory and carries animation channels —
here it is a single `model.glb` at bind pose, so the existing port pipeline keeps reading one file) (its 4-way activity rail and
Unreal-style Details vec3 scrub widgets are omitted — sab is a single character/anim view). The
BC1/BC3 block decoders come from that workshop's `texpng.rs`.
