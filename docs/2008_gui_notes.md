# 2008 Xbox 360 prototype — GUI notes

State of the workshop's exposure of the pre-release build, as landed by this worktree. This
document is the delta from "everything lives at the CLI" to "browse it in the app".

## What ships

### 1. Second-install settings field
`Settings.prototype_dir: String` (persisted in `settings.json` alongside `game_dir`). The retail
install path (`game_dir`) is untouched — the 2008 pointer is **additive**. Both roots can be
configured; each opens its own `Box<dyn GameSource>` under the hood.

Boot-time seed: if the setting points at a folder that fingerprints (has `Bin360/` under it, or
is `Bin360/` itself), the prototype source is opened at app launch. Failure is non-fatal — the
browser page shows the picker + the error string.

Two ways to change it:
- **Settings page** → "2008 Xbox 360 prototype" section: type or Browse.
- **Prototype page** → "Choose folder…" button. Same rfd dialog; same normalisation.

Both bounce through the app's main-thread picker path, since `rfd::FileDialog` blocks and can't
run inside a paint.

### 2. Rail page "2008"
Sixth entry on the rail (glyph `Ξ`, keyboard `5` — Settings moved to `6`). Independent of retail:
- **No wgpu scene** (Inspect owns the 3D viewport; Prototype hides it).
- **No verb bar / export button** (there's no MSH4 exporter yet).
- **No editor changelist** (this isn't a mod-editor page).

Just a browser. State lives in `src/browse_2008.rs::ProtoState`; the rendering module owns the
whole page and returns before `build_ui` even reaches the transport/nav/inspector shell.

### 3. Browser panel
Three tabs off the same `dyn GameSource`:
- **Textures** — every hash the source knows, packlog-mined slot rows + loose-DDS shadow rows
  merged. Names show first, hashes second.
- **Materials** — every UID in the parsed WSAO index (~700+ today, pending SBL4 unwrap for
  more).
- **Meshes** — every packlog-derived mesh entry.

Filter box accepts substrings of the friendly name OR the hex hash (0x-prefixed or bare). List
is virtualized (`show_rows`), so 2k textures scroll at 60fps.

### 4. Detail card
Clicks land here. What each kind shows:

- **Texture** → `source.load_texture(hash)` decoded to RGBA and painted (max 512px, scaled).
  Below: width / height / format / RGBA bytes. On failure (SBL4-wrapped slot, unknown hash),
  the reason is shown in-panel; the decode error is cached so it doesn't retry every frame.
- **Material** → `source.textures_for_material(uid)` yields `[d, s, n, wm]`. Each populated
  slot is a clickable chip that navigates to the corresponding texture (same page, same
  session state). Below the chip list, a 4-thumbnail row previews each slot at 64px.
- **Mesh** → `source.read_mesh_bytes(handle)` gives the SBL4 meshes slot. The card reports
  byte length, the u32 BE record count from the slot header, and inflates the first entry's
  zlib blob to confirm `MSH4` magic. No 3D preview — the inner MSH4 geometry format is a
  future probe (see below).

### 5. Trait addition
`GameSource::list_materials()` was added with a default `Vec::new()` impl. `Prototype2008Source`
overrides it (iterates `MaterialIndex::by_uid`); `RetailPcSource` inherits the default. Retail's
material enumeration has no equivalent yet — WSAO records are looked up by drawcall hash, not
enumerated — and that gap is left for a follow-up.

## What's stubbed

- **~40% of `.Pack` files use non-SBL4 sub-pack magics** (SBLL / SBL8 / SBL6 / SBLX / SBL0 /
  SBL5 / MP00 / AP04). Those slots' materials + textures + meshes are unreachable through the
  source, so their rows either:
  - Return `Err("...non-SBL4 container...")` from `read_mesh_bytes` (mesh detail shows this in
    the "Extract failed" card),
  - Are absent from the material index (`textures_for_material` returns `None` for their UIDs
    — the material detail says so),
  - Are absent from `list_textures` beyond the loose-DDS shadow entries.

  Fixing this is the non-SBL4 sub-pack probe (parallel worktree; explicitly not our scope).

- **MSH4 geometry**. The mesh detail card confirms the outer boundary + inflated `MSH4` magic
  but does not decode the inner geometry. No 3D preview. The task explicitly forbade attempting
  this — the format is unknown and rendering it needs a new decoder.

- **Xbox 360 `.xma` audio**. Not surfaced; audio playback is out of scope.

- **Animation index**. `GameSource` reserves `AssetKind::Animation` but neither source
  enumerates or resolves animations through the trait yet.

- **Retail material enumeration**. `list_materials()` default returns empty on retail because
  WSAO is drawcall-driven there. The prototype page shows all indexed materials because 2008
  parses them all at open time; retail would need a `Wsao::iter()` addition to match.

## What needs which probe

| Need | Blocking probe |
|---|---|
| Every 2008 `.Pack` slot readable | Non-SBL4 sub-pack decoder |
| MSH4 → geometry in the 3D viewer | MSH4 format probe |
| Retail materials in the browser tab | `Wsao::iter()` — trivial follow-up, cross-version |
| Animation clip list on 2008 | 2008 animation container probe |
| Xbox 360 `.xma` playback | New audio codec integration |

## Tests

`browse_2008::tests` — module tests, all skip cleanly when no `Bin360/` extract is present at
`c:/Users/Shadow/Desktop/notes-on-reversing-the-sabetour/game-files`:
- `opens_a_2008_folder_and_lists_assets_through_state` — the state layer opens a real source and
  its three listings each return more than a handful of handles. Guards the boot path + the
  tab-listing cache.
- `cloudproj_hash_round_trips_through_load_texture` — the known-good loose-DDS
  `CloudProj.dds` (512×512 DXT1) decodes correctly through `source.load_texture(hash)`. This
  is the click-a-texture-row-and-see-pixels contract.
- `selecting_a_material_gets_a_slot_vector_when_indexed` — an indexed material UID resolves to
  a slot vector with at least one populated slot. Guards the material detail card's payload.
- `search_needle_matches_hex_hash` — pure filter helper; hex hash acceptance without an install.
- `close_clears_source_and_selection` — Forget button semantics.

`settings::tests` unchanged apart from the round-trip test now carrying `prototype_dir` — a
Save/Load cycle preserves the second install pointer.

## File touched

- `src/settings.rs` — added `prototype_dir: String` + `prototype_root() -> Option<PathBuf>`.
- `src/source.rs` — added `list_materials()` (default empty).
- `src/sources/prototype2008.rs` — implemented `list_materials`.
- `src/browse_2008.rs` — NEW; state + rendering + tests.
- `src/main.rs` — registered the module.
- `src/app.rs` — added `Page::Prototype` (rail entry, key `5`, glyph `Ξ`; Settings moved to
  key `6`), threaded `ProtoState` through `build_ui`, added the "2008 Xbox 360 prototype"
  section to the Settings page, and gated the verb bar / wgpu scene draw to the Inspect page
  only (Prototype + Settings paint full-viewport opaque panels).

The retail flow through `Config.megapack` / `boot::load` / `spawn_tex_resolve` was not touched.
The 2008 layer is entirely additive.
