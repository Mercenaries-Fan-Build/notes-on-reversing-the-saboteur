# Notes on Reversing *The Saboteur* (2009)

Reverse-engineering notes, format specs, and tooling for **The Saboteur** — Pandemic Studios'
final game, built on the **WildStar / Odin** engine (a sibling of the Mercenaries 2 engine on the
shared Pebble core).

This is a **standalone** knowledge base. It draws on prior Mercenaries 2 reverse-engineering where
the engine lineage genuinely carries over, but The Saboteur is a **different game on a later engine
revision** — see [`docs/lineage_and_divergence.md`](docs/lineage_and_divergence.md) for exactly what
transfers and what does not. Do not assume a Mercs 2 fact holds here without checking that table.

## Why this game is a good RE target

- **The retail (GOG) `Saboteur.exe` is a clean, fully-unpacked binary.** No SecuROM wall: `.text`
  entropy over all 11.9 MB averages **6.49**, with no 64 KiB window above **6.84** — nowhere near the
  >7.5 of packed or encrypted code *(corrected 2026-07-24: was "flat ~6.2–6.7"; 22.5% of the 182
  64 KiB windows fall outside that band — see [`docs/binary_recon.md`](docs/binary_recon.md))*. The
  entry point is in normal code, and the leftover
  `.secu` section is inert. The whole engine is directly disassemblable.
- **2,765 RTTI class names in the clear** (`WS*` = WildStar engine, `Pbl*`/`Pcl*` = Pebble core),
  plus **898 named Lua bindings** and **321 uncompressed Lua 5.1 scripts**. Effectively a symbol map.
- We have a **full Ghidra decompilation: 36,935 functions** (regenerate via
  [`tools/ghidra/DecompileSaboteur.java`](tools/ghidra/DecompileSaboteur.java); the 54 MB output is
  gitignored — see that tool's notes).

## Middleware stack (confirmed from the binary)

| System | Middleware | Version |
|---|---|---|
| Rendering | Direct3D 9 (+D3DX9_39) | — |
| Physics/Animation | **Havok 6.5.0** (`Havok_65`) | ⚠️ NOT the 5.5 Mercs 2 uses |
| Audio | **Wwise** (custom `1KCP` package) | 2009-era |
| UI | Scaleform **GFx** | — |
| Facial | FaceFX | — |
| Video | Bink | — |
| Core lib | Pebble (`Pbl*`/`Pcl*`) | Pandemic lineage |

## Repository map

```
docs/
  lineage_and_divergence.md   ★ shared-vs-different vs Mercenaries 2 — READ FIRST
  binary_recon.md             the clean-exe recon (sections, RTTI, bindings, scripts)
  community_tooling.md        community tool landscape + where we can contribute
  mattias_port_plan.md        the staged character-port project
  tools/                      ★ THE MODDING TOOLSET — start at tools/README.md
    README.md                 what each tool is, and an "I want to…" index
    workflows/                cross-tool recipes: replace-a-texture, add-ui-text, port-a-character
  formats/                    byte-level specs behind the tools
    lua_scripts.md            ★ .luap packs — flat, uncompressed, the cheapest way in
    archive_and_models.md     MP00 megapack → SBLA → MSHA → flat MESH; patch-megapack override
    megapack_write.md · sbla_subpack.md · mesh_geometry.md · skeleton.md
    dtex_texture.md · gametext.md · gametemplates.md · editnodes.md · map6.md
    audio_1kcp.md             the 1KCP Wwise package + extraction pipeline
    animation_havok65.md      AP0L pack, Havok 6.5 — the community anim-decode gap, cracked
tools/                        the modding toolset — each crate's README is its manual
  sab_workshop/               the GUI: character/anim viewer + Templates/GameText/Icons editor
  sab_pack/ sab_dtex/ sab_sbla/ sab_gametemplates/                 read + write the game's formats
  sab_mesh/ sab_skeleton/ sab_havok65/ sab_animmeta/               asset extraction → glTF
  sab_probe/                  read-only interrogation (incl. gametext + editnodes inspection)
  sab_validator/              parses a mod the way the engine's mount path does
  sab_formats/                the shared codec library the others build on (gametext, editnodes,
                              dtex, mesh, sbla, …; ships the `sab_gametext` authoring bin)
  sab_probe/ sab_map6/ sab_asi/                                    inspection + live engine reads
  saboteur_audio/             Rust: 1KCP → .wem carve → vgmstream → WAV (all VO extracted)
  ghidra/DecompileSaboteur.java   headless full-binary decompile export
data/
  rtti_classes_all.txt (2765) · ws_engine_classes.txt (823) · lua_bindings.txt (898)
  havok_version_evidence.txt
memory/                       durable session notes (MEMORY.md is the index)
```

**Modding the game rather than reversing it?** Go straight to
[`docs/tools/README.md`](docs/tools/README.md) — the toolset index and the task-oriented guides.
Prebuilt Windows binaries are on the [Releases](../../releases) page.

## Status

| Area | State |
|---|---|
| Binary recon | ✅ done (clean exe, symbols recovered) |
| Full decomp | ✅ 36,935 functions (local, regenerable) |
| Audio / VO | ✅ extraction pipeline built; 80,872 WAV lines extracted (all 4 langs) |
| Lua scripts | ✅ `.luap` format cracked from decomp + verified vs retail; all 321 scripts decompiled (116,681 lines) |
| Archive/model format | ✅ documented (decomp + SaboteurToolset cross-ref) **and implemented** — `sab_pack`, `sab_sbla`, `sab_mesh`, `sab_skeleton`, `sab_map6` read them (several also write) *(corrected 2026-07-24: previously said "no reader written yet")* |
| Animation decode (Havok 6.5) | ✅ **cracked** — corpus is 100% `hkaSplineCompressedAnimation`; decoder in `tools/sab_havok65` decodes all 2,214 clips (0 failures). Was the flagship community-wide gap. |

## Provenance / assets

This repo contains **notes and tooling only**, no game assets. It assumes a local retail install
(paths reference `C:\GOG Games\The Saboteur`). Large regenerated outputs (the decomp text, extracted
audio, carved assets) are gitignored; each doc says how to regenerate them from your own copy.

## License

[MIT](LICENSE). Covers the notes and tooling in this repo only — not any game assets, which remain
the property of their respective rights holders.
