# 2008 pre-release binaries — full decompilation pass

*Generated 2026-09-13. Companion to [2008_wildstar_d_forensic.md](2008_wildstar_d_forensic.md) (the deep forensic dive on `WildStar_d.exe`) and [2008_build_inventory.md](2008_build_inventory.md). Covers **all three** major PPC binaries in the pre-release drop.*

## Headline

Three Xbox 360 PowerPC binaries from `game-files/Bin360/` are fully decompiled to text, with names resolved from their paired PDBs / linker maps. Each is a self-contained cross-reference:

| Binary | Source | .text | Fns | Named | Mean body | ≥ 200 B | Wall | Decomp file |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| `WildStar_d.exe` | raw PE | 17.2 MB | **79,330** | **99.8%** | 211 B | 27.7% | 20 min | 107 MB |
| `WildStar_p.exe` | raw PE | ~9 MB | **31,784** | **99.7%** | 290 B | 35.3% | 18 min | 62 MB |
| `Saboteur.pe.bin` | XEX-unwrapped from `Saboteur.xex` | 9.6 MB | **45,772** | **46.8%** | 79 B | 8.3% | 8 min *(reused analysis)* | 63 MB |

Retail x86 `Saboteur.exe` for comparison: 36,935 fns / 12 MB `.text` / ~30% ≥ 200 B / all names via RTTI. All three PPC decomps here are richer.

## What each binary is

- **`WildStar_d.exe`** — the **debug build of the engine tools** (17 MB `.text`, unstripped, `-Od`, XDK aug07 debug configs). Richest per-function detail, biggest surface area, most names.
- **`WildStar_p.exe`** — the **profile build of the engine tools** (~11 MB, `-O1` `-Zi`). Fewer functions (no per-inline symbol) but same subsystem coverage. Best for "what does the release-shape code look like" reads.
- **`Saboteur.pe.bin`** — **the actual shipping game code**, unwrapped from the encrypted-devkit `Saboteur.xex` via [sibling project's `xex_unpack.py`](../../notes-on-the-released-game/tools/xex_unpack.py) with the devkit AES key. This is the primary reverse-engineering target: what the retail 2009 x86 `Saboteur.exe` is descended from.

## The reproducible recipe (what makes v4-quality decomp possible)

Every gap that stood between "raw exit code 0" and "actually named + fully-body'd decomp" is closed now. Full workflow, replay-verified per binary:

1. **Install [GhidraXenon](https://github.com/freeqaz/ghidra-xenon-extension) v12.1** into `<ghidra_install>/Ghidra/Extensions/GhidraXenon/`. **Not** `<install>/Extensions/Ghidra/` (bundled staging, ignored by classloader) and **not** `~/.ghidra/.ghidra_12.1_PUBLIC/Extensions/` (user-installed, needs GUI acceptance). Adds `PowerPC:BE:64:Xenon` language with 77 VMX128 opcodes + AltiVec. Without this, ~65% of `.text` decompilation truncates at the first VMX128 SIMD op.

2. **Import with `-processor "PowerPC:BE:64:Xenon"`** and `-cspec "default"`. For XEX files, first unwrap with `python tools/xex_unpack.py <xex> --out <pe.bin> --devkit` (dev-signed binaries use the zero KEK). The 2008 prototypes are all dev-signed.

3. **Pre-script [`tools/ghidra/PrepWildStarPreAnalysis.java`](../tools/ghidra/PrepWildStarPreAnalysis.java)** — disables the crashing `PdbUniversalAnalyzer` (Ghidra 12.1 bug: `DefaultPdbApplicator:299` throws on these Xbox 360 PDBs) and walks the `.pdata` section, calling `createFunction()` at every unwind anchor. **Xbox 360 `.pdata` stores full VA in big-endian**, not RVA — the pre-script reads it correctly.

4. **Post-script for names** — one of two, matching the binary's name source:
   - `.map` file available (WildStar_d, WildStar_p): [`ApplyMapSymbolsWildStar[P].java`](../tools/ghidra/ApplyMapSymbolsWildStar.java) — parses the linker map's "Publics by Value" and renames every `FUN_` at the matching VA.
   - PDB only, no `.map` (Saboteur): [`ApplyDia2DumpSymbols.java`](../tools/ghidra/ApplyDia2DumpSymbols.java) — parses `output/pdb_dumps/*_p.txt` (from patched DIA2Dump, see step 5) and applies mangled names.

5. **PDB access without admin: patched DIA2Dump** at `scratchpad/dia2dump/x64/Release/DIA2Dump.exe`. Original VS-BuildTools sample requires `regsvr32 msdia140.dll` (admin). Patched to `LoadLibraryW("msdia140.dll") + DllGetClassObject` (`NoRegCoCreate` equivalent), and the x64 msdia140.dll is copied alongside from `Common7/IDE/PrivateAssemblies/amd64/`. Runs against WildStar_d.pdb (101k publics), WildStar_p.pdb (86k publics), Saboteur.pdb (71k publics) in ~1 sec each.

6. **Post-script `DecompileWildStar[P|Saboteur].java`** — full-corpus decomp of every non-thunk function in an executable block, sorted by VA, header line `==== <name> @0xADDR size=N callers=[…] ====` followed by decompiled C.

## Machine-readable outputs

Ghidra decomps (grep these for reverse-engineering work):
- [`output/_ghidra_wildstar/wildstar_d_all_functions_decomp.txt`](../output/_ghidra_wildstar/wildstar_d_all_functions_decomp.txt) — WildStar_d (107 MB, 99.8% named)
- [`output/_ghidra_wildstar_p/wildstar_p_all_functions_decomp.txt`](../output/_ghidra_wildstar_p/wildstar_p_all_functions_decomp.txt) — WildStar_p (62 MB, 99.7% named)
- [`output/_ghidra_saboteur_xex/saboteur_xex_all_functions_decomp.txt`](../output/_ghidra_saboteur_xex/saboteur_xex_all_functions_decomp.txt) — Saboteur.pe (64 MB, 46.8% named)

Ghidra projects (open in the GUI for interactive work):
- `output/_ghidra_wildstar/proj/WildStarD.gpr`
- `output/_ghidra_wildstar_p/proj/WildStarP.gpr`
- `output/_ghidra_saboteur_xex/proj/SaboteurXex.gpr`

PDB publics dumps (grep-able name/VA index for each PDB):
- [`output/pdb_dumps/wildstar_d_pdb_p.txt`](../output/pdb_dumps/wildstar_d_pdb_p.txt) — 101,807 symbols, 19 MB
- [`output/pdb_dumps/wildstar_p_pdb_p.txt`](../output/pdb_dumps/wildstar_p_pdb_p.txt) — ~85k symbols, 12 MB
- [`output/pdb_dumps/saboteur_pdb_p.txt`](../output/pdb_dumps/saboteur_pdb_p.txt) — 71,487 symbols, 12 MB
- [`output/pdb_dumps/saboteur_pdb_s.txt`](../output/pdb_dumps/saboteur_pdb_s.txt) — 481,563-line compilation-unit metadata (module-level detail, no per-fn VA)

Forensic dumps for WildStar_d only (structured TSVs — sections, imports, .pdata, strings, etc.):
- [`output/wildstar_d_forensic/`](../output/wildstar_d_forensic/) — see [2008_wildstar_d_forensic.md](2008_wildstar_d_forensic.md) for the full index

Preserved older-run artefacts under `output/_ghidra_wildstar/archive_stale/` and `output/_ghidra_saboteur_xex/archive_stale/`.

## Gotchas that ate real time

1. **XEXLoaderWV extension refuses to load into Ghidra 12.1.** Its extension.properties says `version=13.0.0` (typo) and even after correcting the file, Ghidra's LoaderService doesn't register the class. Workaround: skip XEXLoaderWV entirely, use `xex_unpack.py` to produce a raw PE, load that as PE with `-processor "PowerPC:BE:64:Xenon"`.
2. **Xbox 360 `.pdata` stores the full VA in big-endian, not an RVA.** Bytes `82 34 00 00` at `.pdata` offset 0 are `0x82340000` (= `.text` start VA), *not* imagebase + 0x82340000. My first pre-script added imagebase twice and skipped all 68,571 entries.
3. **Windows paths in Java string literals need `\\` or forward slashes.** `"C:\Users\..."` in Java is a compile error (`\U`, `\S` invalid escapes). Use forward slashes — Java's `File` accepts them on Windows.
4. **Ghidra script filename must match public class name.** `ApplyMapSymbols.java` must have `public class ApplyMapSymbols`, not `ApplyMapSymbolsWildStar` inside the file.
5. **Extension version string vs `application.version`.** GhidraXenon (`version=12.1`) loads; XEXLoaderWV (`version=13.0.0`) doesn't — even after sed-fixing the properties. The check must happen somewhere else too.
6. **DIA2Dump demangled names contain backticks** (`` `RTTI Complete Object Locator' ``) which Ghidra's `SymbolUtilities.validateName` rejects. Use the mangled name (`??_R4ClassName@@6B@` — safe chars) and let Ghidra's own Microsoft demangler handle display.
7. **VS 2026 BuildTools shipped only the v145 toolset in `VC/Tools/MSVC/`** despite the `MSBuild/Microsoft/VC/{v145,v150,v160,v170,v180}` prop dirs suggesting more. Force `/p:PlatformToolset=v145` on the DIA2Dump build.
8. **`msdia140.dll` next to `pdb.exe` / `DIA2Dump.exe` must be the same bitness as the exe.** VS ships an x86 copy in `Common7/IDE/msdia140.dll` and an x64 copy under `Common7/IDE/PrivateAssemblies/amd64/`. Mismatch → `LoadLibrary` returns `ERROR_BAD_EXE_FORMAT` (GLE 193).
9. **`analyzeHeadless` exit code 0 doesn't mean useful output.** Every run needs a sanity check:
   - Wall clock (17 MB `.text` should be ≥ 15 min, not 5)
   - AutoAnalysisManager summary in the log (each of Stack / PPC Constant Ref / Disassemble Entry should be tens of seconds)
   - WARN counts by class (17k+ "Unable to resolve constructor" = VMX128 issue, not language)
   - Function size histogram (≥ 25% of functions should be ≥ 200 B for real code density)

## Known limits (open follow-ups)

### Sab.pe naming coverage is capped at ~47%
DIA2Dump's `-p` (publics) covers exported / RTTI-visible symbols. **Static-scope functions** (compilation-unit-local, most compiler helpers, most inlines) aren't in publics. That's the 53% we can't name from `-p` alone. WildStar_d/_p hit 99.8% because their linker `.map` files list *all* symbols including statics — publics is a strict subset. To close this on Sab.pe, we need one of:
- A custom DIA-walking tool that iterates `IDiaSymbol` children of each compiland (`SymTagFunction` with location `LocIsStatic`). Order of hours.
- A `Saboteur.map` — doesn't exist in the drop.

### PDB types + function signatures still unused
`ApplyDia2DumpSymbols.java` currently only takes names. DIA2Dump `-t` dumps types (structs, classes, enums) and `-s` dumps per-compiland detail with parameter types. Ingesting these into Ghidra would give **typed local variables + typed function signatures** in the decomp — a huge quality bump on top of what we have. Order of a day: parse types → build Ghidra `DataType` graph → apply.

### VMX128 semantics are decoded but not modeled
GhidraXenon gives us disassembly + boundary preservation through VMX128 blocks — SIMD instructions no longer truncate function bodies. But the SIMD math itself decompiles as opaque intrinsics or `?` in the C output. For heavy math (rendering, physics, audio DSP), the scalar wrapping is still readable and useful; the vector kernels aren't. Xenia has real VMX128 semantics — bridging would be a follow-up.

### `pc_symbol_map.tsv` not extended
Retail's [`data/symbol_map/pc_symbol_map.tsv`](../data/symbol_map/pc_symbol_map.tsv) (1,414 named retail x86 functions) was built from an older WildStar_d PDB via RTTI cross-reference. The current drop's larger name set (79k WildStar_d + 45k Saboteur, all with types) could extend that map — but the RTTI cross-reference machinery in [`tools/xsym/`](../tools/xsym/) reads from `game-files/symbols/` which holds the older PDBs. Repointing + rerunning is unstarted this session.

### Replay tools untouched
`ReplayCore-xenon.xex` (2.7 MB) and `ReplayLauncher-Xenon.xex` (3.8 MB) unwrapped to `scratchpad/ReplayCore-xenon.pe.bin` and `ReplayLauncher-Xenon.pe.bin` (both dev-signed, LZX-basic compression). No PDBs for them in the drop → naming would come from RTTI + strings only. Not decompiled this session.
