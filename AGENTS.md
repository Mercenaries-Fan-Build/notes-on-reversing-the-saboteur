# Working in this repository (humans and AI agents)

This is a reverse-engineering knowledge base for **The Saboteur (2009)**. Read this before making
changes. It defines conventions and — importantly — how NOT to over-import from the Mercenaries 2 work
this project descends from.

## Prime directive: verify lineage before you reuse

The Saboteur runs a **later revision of the Pandemic engine** than Mercenaries 2. Some things are
byte-identical (the hash, `sges` compression); most concrete formats are **different** (megapack vs
FFCS WAD, Havok 6.5 vs 5.5, Wwise `1KCP` vs PWS wavebanks, flat MESH vs UCFX, WSAO vs MTRL).

Before stating that a Mercs 2 fact applies here, check [`docs/lineage_and_divergence.md`](docs/lineage_and_divergence.md).
If it isn't in the "shared" column, treat it as **unverified** and confirm against the Saboteur binary
or assets. Never copy a Mercs 2 offset, struct, or hash into a Saboteur doc without re-deriving it.

That rule is about *facts*, not *ideas*. Broader container families and studio techniques (how
destruction, LOD or streaming is organised) are often **siblings** — the same concept re-expressed in
different code and data. The Mercs 2 docs are a good map of what to look for; find the Saboteur's own
version in the decomp/data, prove it there, and record the pairing in the lineage doc's SIMILAR CONCEPT
table (e.g. destruction: Mercs 2 `_pristine`/`_ruin` nodes ↔ Saboteur `Pristine` damage parts).

## Ground truth, in priority order

1. **The clean decomp** — ⚠️ **it lives in the SIBLING repo, not this one:**
   `C:/Users/Shadow/Desktop/notes-on-the-released-game/output/_ghidra_saboteur/saboteur_all_functions_decomp.txt`
   (54 MB, 36,935 entries — 35,819 `FUN_*` plus 1,116 demangled/synthetic names; local/regenerable).
   There is **no** `output/_ghidra_saboteur/` under this repo; the old relative path here sent every
   agent looking in the wrong place. The exe is unpacked, so the decomp is authoritative for engine
   behavior. Grep it.
   It can also be **regenerated into this repo** (`output/_ghidra_saboteur/`, gitignored) with
   [`tools/ghidra/README.md`](tools/ghidra/README.md) — use the **GOG** exe, never the Steam one. On a
   machine where you have done that, grep the local copy; it reproduces the same 36,935 entries.
   Two caveats worth knowing before you conclude "it isn't there": Ghidra drops some bodies as
   *unreachable blocks* (e.g. the whole `GameText.dlg` record loop in `FUN_0095f370`), and a few
   FPU-heavy functions are absent entirely (`FUN_00f22470`). When the decomp is silent, disassemble
   `C:/GOG Games/The Saboteur/Saboteur.exe` directly — VA→file offset is `VA - 0x400E00` for `.text`
   (and for `.rdata`, which follows it with the same delta — handy for IAT slots and string literals).
2. **RTTI / bindings** — `data/rtti_classes_all.txt`, `data/ws_engine_classes.txt`, `data/lua_bindings.txt`.
3. **The retail install** — `C:\GOG Games\The Saboteur` (assets, `LuaScripts.luap`, `.pck`, megapacks).
   On a Mac it can be installed into a Wine prefix and launched to check a mod in-game — see
   [`docs/tools/workflows/run-on-macos.md`](docs/tools/workflows/run-on-macos.md).
4. **SaboteurToolset** (PredatorCZ) & **SabTool** — community format prior art to cross-check, not to trust blindly.
   Community tools live in `game-files/community-tools/` (SabTool's `Hashes.txt` is a large hash→name
   dictionary — verify any name with `pandemic_hash` before using it); notes shared with us by community
   members are in `game-files/community-research/`. Both are gitignored local copies.

## Conventions

- **Prefer Rust for tooling** (see `tools/saboteur_audio`). Small std-only binaries; document how to run.
- **`tools/sab_formats` is the base library.** Every reader/writer of game bytes lives there (one
  copy per format; it may use `glam` for skeleton/bind maths). It holds **no probes and no CLIs** —
  tests only. Apps and tools consume it; they never carry a private parser, hash, or byte reader.
- **The user-facing tooling lives in `tools/sab_tools`** — one crate for every command-line tool
  (pack/unpack, texture convert, templates, mesh/skeleton export, animation, audio, Lua, maps), each
  a thin layer over `sab_formats`. **`tools/sab_workshop` is the UI** over that tooling: presentation
  and editing, no format code of its own. Most of this work is format juggling; the split mirrors the
  Mercenaries 2 crates.
- **Probes and investigations live in `tools/sab_probe`** (`src/bin/<probe>.rs`), built on
  `sab_formats`. A probe whose numbers a doc cites is named in that doc's Method section.
- **Retire by deletion.** When a tool or module is replaced, delete it in the same pass — git keeps
  the history. A leftover copy gets taken as a "good example" by the next agent and contaminates the
  research.
- **Local paths come from `.sab-local.toml`** (repo root, gitignored; one `key = "path"` per line,
  like the Mercs 2 repo's `.mercs2-local.toml`). Game-gated tests are built only with the `retail`
  feature, read their paths from that file alone, and panic naming the missing file/key/path. No
  hardcoded machine paths anywhere.
- **Verify artifacts by hash**, not size/mtime, when you produce or deploy anything.
- **No meta commentary in code.** Comments state what the code does and the format facts it relies
  on (with evidence). Plans, stages, migrations, "kept until…", and research narration belong in the
  chat or in `docs/`, never in code.
- **Edit files with the editor tools, never with scripts.** No `perl -pi`, `sed -i`, heredoc
  `cat > file`, or splicing via `cp` from scratch. Shell is for building, running and reading.
- **Ask, don't assume** on unverified setup/intent facts. State the defaults you pick.
- **Minimal, faithful edits** to others' files; don't reword working prose.
- **Don't commit game assets or huge regenerated outputs** — they're gitignored. Commit the *method*
  to regenerate them, plus small reference dumps.
- Every format claim should cite its evidence: a decomp function VA, a byte offset in a named file, or
  a cross-reference to SaboteurToolset source.
- **Write it up as you go.** When a research pass establishes how a feature works, document it in
  `docs/` before digging further — grade each claim (PROVEN / INFERRED / SPECULATIVE), give a method to
  regenerate it, and list the unknowns. Cross-link the docs it answers or extends. The Mercs 2 repo's
  `docs/reverse_engineer/*_code_map.md` are the model for depth; [`docs/damage_and_destruction.md`](docs/damage_and_destruction.md)
  is an example here.

## What is deliberately NOT carried over from the Mercs 2 repo

- SecuROM devirtualization workflow — **irrelevant**; the Saboteur exe is already clean.
- The x32dbg live-debugging playbook tied to Mercs 2's SecuROM-unpacked image.
- The FFCS/WAD (`vz.wad`, `vz-patch.wad`) block-injection machinery — Saboteur uses megapacks + a
  built-in `patchmega*.megapack` override layer instead.
- The Mercs 2 64-bit Rust/wgpu reimplementation program.
- Havok **5.5** struct offsets — Saboteur is **6.5** (different layout).

Bring over *methodology* (Ghidra headless flow, hash pipeline, Wwise/vgmstream approach), not *facts*.

## Memory

`memory/` holds durable cross-session notes; `memory/MEMORY.md` is the one-line index. Add a note when
you establish something non-obvious and reusable; keep index lines short.
