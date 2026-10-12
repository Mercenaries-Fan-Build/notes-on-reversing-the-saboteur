# Format consolidation — moving every reader/writer into `sab_formats`

**Started 2026-10-12** on branch `workshop-lod-export`. The aim, the rules and each stage's status, so
the work can be resumed by anyone. Rules (also in [`AGENTS.md`](../../AGENTS.md)):

* `tools/sab_formats` is the one base library for game bytes (glam allowed; tests only — no probes, no
  CLIs). Apps and tools consume it and carry no private parser, hash or byte reader.
* Probes live in `tools/sab_probe`.
* A tool or module that has been replaced is deleted in the same pass.
* Local paths come from `.sab-local.toml`; game-gated tests use the `retail` feature.
* Every move is verified against the stage-2 goldens before the old code is deleted.

## Why

A survey found the same formats parsed in many places, disagreeing on facts: 11 megapack readers, 6 DTEX
parsers, 5 MESH walks, 10+ copies of `pandemic_hash` (seven of which hash `""` differently from the
library), three incompatible ALBS readings, two `global.map` readings. Most tools did not use
`sab_formats` at all.

## Stages

| # | stage | status |
|---|---|---|
| 0 | `game_paths` (`.sab-local.toml`, folder rules), `retail` feature | **done** (`c00b4a4`…`654041d`) |
| 1 | settle the facts the copies disagree on, from the engine | **done** — [`dtex_texture.md`](../formats/dtex_texture.md), [`sbla_subpack.md`](../formats/sbla_subpack.md), [`map6.md`](../formats/map6.md) |
| 2 | golden outputs of every tool, before anything moves | **done** — see below |
| 3 | shared primitives: one hash, one byte-reader set, one zlib helper | next |
| 4 | archive layer: megapack read+write, ALBS (engine layout) read+write, MSHA, loosefiles, global.map/France.map | |
| 5 | assets: MESH (body, skeleton, vertex formats, read+write), DTEX (any D3DFORMAT, BC codecs, DDS), WSAO, `.luap`, ANIM block, Havok (one packfile walker, one spline decoder), Wwise carving | |
| 6 | 2008 prototype formats out of `sab_workshop/src/sources/` | |
| 7 | consumers rewired (`sab_workshop`, `sab_poc`, `sab_probe`, `sab_validator`, `sab_havok65`); replaced tools deleted | |
| 8 | scratch probes into `sab_probe`; doc Method sections point at them | |

## Stage 2 — the goldens

Local only (game-derived, 5.5 GB): **`output/goldens/stage2/`** (git-ignored, like all of `output/`).

* `goldens/<tool>/manifest.tsv` — one row per output: `tool, command, output, sha256, bytes,
  deterministic, notes`. The command is the exact argv; paths in it are this machine's
  (`.sab-local.toml`'s `retail_dir` and the old scratch location of `goldens/`).
* `goldens/<tool>/run1/…`, `run2/…` — every command was run twice; the outputs themselves.
* `goldens/_inputs/` — inputs carved for the runs (`global.map`, `France.map`, the main GameTemplates
  block from `France/loosefiles_BinPC.pack`; a Sean skeleton/mesh) with `provenance.txt` (binary and
  input hashes; commit `ddc9405`).

Coverage: every subcommand of `sab_pack`, `sab_sbla`, `sab_dtex`, `sab_mesh`, `sab_skeleton`,
`sab_megapack_key`, `sab_map6`, `sab_gametemplates`, `saboteur_lua`, `saboteur_audio`, `sab_animmeta`,
`sab_havok65`, `sab_probe`, `sab_validator`, `sab_poc` (what can run), the `sab_formats` bins and test
suite (with and without `retail`), and `sab_workshop`'s test suite and headless modes — 31,750 rows.

**Determinism.** Identical across both runs except: panic messages (they carry the OS thread id); test
suites' timing lines (the golden is the sorted test-name/verdict list, `tests.normalized`);
`sab_probe parts` (label order from HashMap iteration); `sab_workshop --source-info` on the prototype
(`.take(200)` over a HashMap). A replacement should make those deterministic.

**How to verify a move.** Re-run the manifest row's command with the replacement (same inputs), hash
the output, compare. A difference is either a bug in the move or a deliberate fix — and a deliberate fix
is named in the commit and in the table below.

### Goldens that are known wrong (expected to change)

These record today's behaviour, which the engine-derived formats show to be wrong or silent. The
replacement must differ here, loudly:

| tool | today | right behaviour |
|---|---|---|
| `sab_map6` on main `global.map` | 460 MB of garbage lines, exit 0 | engine layout ([map6.md](../formats/map6.md)) |
| `sab_map6` on both France maps | desync at record 0 / 9, exit 0 | engine layout |
| `sab_dtex list`/`carve` | skips A4R4G4B4 (`0x1A`) textures silently (0 textures in Mega2) | any D3DFORMAT ([dtex_texture.md](../formats/dtex_texture.md)) |
| `sab_dtex extract` on `0x1A` | "unknown format", exit 1 | decode A4R4G4B4 |
| `sab_sbla scan` | 0 of Mega1/Mega2 processed, exit 0; first record framed as "MIDDLE" (unreplaceable) | engine ALBS layout ([sbla_subpack.md](../formats/sbla_subpack.md)) |
| `sab_sbla replace` | `0x1000` parsed as decimal 1000; `-` with a bad index panics | hex parsed as hex; an error, not a panic |
| `sab_mesh` | missing parts skipped and bad weight sums reported, exit 0 | an error |
| `sab_havok65 gltf N` (N out of range) | error printed, exit 0 | non-zero exit |
| `saboteur_audio` without vgmstream | deletes the `.wem` dir, manifest points at missing files, exit 0 | an error before deleting anything |
| `sab_megapack_key` | exit 0 when every key is NOT FOUND | non-zero exit |
| `sab_probe gametext get` (missing record), `sab_gametext` (no subcommand), `sab_poc inventory` (nothing found) | exit 0 | non-zero exit |
| `sab_probe tex-*` | writes `./dtex_index.tsv` into the working directory | read-only, as documented |
| `sab_formats` / `sab_workshop` tests | 5 + 2 tests print "skip" and pass | `retail` feature, fail naming what is missing |
| `sab_workshop --selftest` | "OK" with 0/16 submeshes textured | fail |
| `sab_workshop --anim-sweep` | 10/200 clips BAD, exit 0 | non-zero exit |

Also noted, not yet judged: `sab_workshop --source-info` reports 0 textures on retail; some `--texcheck`
slot labels look swapped (`spec=…_NM`); `sab_validator` reports 53 fatals on untouched
`dlc01mega0.megapack` (same rule as the documented BelleStart0 false positives); `saboteur_lua` writes 86
chunks with `flag != 0` as stored (all are valid LuaQ).

### Not captured

* WAV decoding (`vgmstream-cli` absent); the French/German/Italian audio packs (not in the Steam install).
* `sab_havok65 validate-hct`, `anim_patch_saboteur roundtrip` and frame edits — need Havok's AssetCc2 /
  an HCT XML that is not on this machine.
* `sab_poc` Mattias / 50 Cent commands — their source assets exist only on another machine; `deploy`
  commands were not run (they write into the install).
* `sab_pack`/`sab_dtex` per-entry loops over Mega1 (covered by list/scan instead).
* Prototype formats in the single-format tools (they reject `MAP7`, `BLU2`, `AP04` — rejections captured).

## Where the command lines go (decided 2026-10-12)

The user-facing tools — `sab_pack`, `sab_dtex`, `sab_gametemplates`, `sab_mesh`, `sab_skeleton`,
`sab_havok65`, `sab_animmeta`, `saboteur_audio`, `saboteur_lua`, `sab_map6`, `sab_sbla` — become one
crate, **`tools/sab_tools`**, a thin command-line layer over `sab_formats`. `sab_workshop` is the UI over
the same tooling (presentation and editing). Each old tool folder is deleted once its `sab_tools`
replacement reproduces its goldens (or differs only by a listed fix).
