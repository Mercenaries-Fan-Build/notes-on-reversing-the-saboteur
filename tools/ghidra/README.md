# Ghidra: full-binary decompile export

`DecompileSaboteur.java` decompiles every non-thunk function in `Saboteur.exe` to one text file,
`saboteur_all_functions_decomp.txt` (~52 MB, gitignored), in the directory given as its script argument
— conventionally `output/_ghidra_saboteur/` in this checkout. ⚠️ Older notes cite a copy in the sibling
repo at `C:/Users/Shadow/Desktop/notes-on-the-released-game/output/_ghidra_saboteur/`; on a machine
where you have regenerated it here, grep this repo's `output/` instead. Because the retail
exe is unpacked and RTTI-rich, Ghidra's auto-analysis recovers thousands of class/function names, so the
export comes out substantially pre-named.

## Prereqs
- Ghidra 12.x + a JDK 21 (Temurin used originally).
- The **GOG** `Saboteur.exe` (1.3 hotfix 22140, sha256
  `628cdbb64a5317fef66d3c2fe87ab063b0c659dfc92f5882b85828f90568e4ef`). The **Steam** exe will not do: it
  carries `.secu` + `.bind` (SteamStub) sections and an encrypted `.text`, and Ghidra recovers only
  ~1,250 functions from it. Without a Windows machine, pull the exe out of the GOG installer with
  `innoextract --include Saboteur.exe --language en-US -d <dir> setup_the_saboteur_1.3_hotfix_(22140).exe`.

## Run (headless)
The output directory is the script's one required argument; it fails if it is missing.
```
analyzeHeadless <projectDir> SaboteurGOG \
  -import <path>/Saboteur.exe \
  -scriptPath tools/ghidra \
  -postScript DecompileSaboteur.java <checkout>/output/_ghidra_saboteur \
  -analysisTimeoutPerFile 5400 -max-cpu 6
```
Import + auto-analyze + the post-script export run in one pass (~30–90 min).

**macOS (Homebrew Ghidra + asdf Java):** `analyzeHeadless` is not on `PATH`; it is at
`$(brew --prefix)/Cellar/ghidra/<version>/libexec/support/analyzeHeadless`. Ghidra's launcher calls the
`java` shim, so with asdf and no `.tool-versions` you must set all three of
`ASDF_JAVA_VERSION=<temurin-21 version>`, `JAVA_HOME=$(asdf where java <version>)` and
`PATH="$JAVA_HOME/bin:$PATH"` — otherwise it prints "Unable to prompt user for JDK path, no TTY" and
**still exits 0**. Always check the log and the output header, not the exit code.

## Output
- `# Saboteur ALL export: 36935 functions` header, then per-function blocks:
  `==== <name> @0xADDR  size=N  callers=[...] ====` followed by the decompiled C.
- Last run (2026-10-11, Ghidra 12.1.4, GOG exe above): 36,935 functions, 1 decomp failure, 0 exceptions —
  identical counts to the original Windows export.

Grep this file as the engine oracle (it's authoritative — the exe is unpacked). It is intentionally
gitignored (too large); regenerate locally.

# The 2008 Xbox 360 prototype binaries

The prototype's `Bin360/` ships two raw PowerPC PEs with linker maps — `WildStar_d.exe` (debug, PE
timestamp 2008-05-07) and `WildStar_p.exe` (profile, 2008-05-20) — plus `.xex` wrappers. They load
different asset revisions: `WildStar_p` (and `Saboteur.xex`, `WildStar_p-org.xex`) reads `SBL4` packs and
`MSH4` meshes, the revision the shipped data uses; `WildStar_d` reads `SBL6` / `MSH0`
(`sab_probe proto2008-binaries`). **Grep the `WildStar_p` export for 2008 format questions.**

Scripts (all take their paths as script arguments and fail without them):
- `PrepWildStarPreAnalysis.java` (pre-script, no args) — disables the crashing PDB analyzer and creates a
  function at every `.pdata` entry (Xbox 360 `.pdata` holds full big-endian VAs).
- `ApplyMapSymbolsWildStar.java <map>` — names functions from the linker map's "Publics by Value".
- `DecompileWildStar.java <out file>` — the full export, same format as `DecompileSaboteur.java`, with the
  decompiler's unreachable-block elimination **off**: with it on, a value read through `PblFile`'s virtual
  read into a stack local folds to its initial value and the branch testing it disappears (all of
  `ReadMesh` and `LoadBlock`'s `SBL4` check in `WildStar_p`).
- `ExportListing.java <out file> <name|0xaddr>…` — disassembly of the named functions, with the symbols
  each instruction references. Even with elimination off, `WildStar_p`'s inlined `PblFile` reads leave
  constant-folded conditions; read the field order off the listing.

Prereq: the GhidraXenon extension (`PowerPC:BE:64:Xenon`, VMX128), loaded from a settings directory so
the Ghidra install is untouched — unzip the extension into
`<out>/ghidra_settings/<user>-ghidra/ghidra_12.1.4_PUBLIC/Extensions/` and pass
`-Dapplication.settingsdir=<out>/ghidra_settings` in `GHIDRA_HEADLESS_JAVA_OPTIONS`.

```
analyzeHeadless <out>/proj WildStarP -import <Bin360>/WildStar_p.exe \
  -processor PowerPC:BE:64:Xenon -cspec default -scriptPath tools/ghidra \
  -preScript PrepWildStarPreAnalysis.java \
  -postScript ApplyMapSymbolsWildStar.java <Bin360>/WildStar_p.map \
  -postScript DecompileWildStar.java <out>/wildstar_p_all_functions_decomp.txt -max-cpu 8
```
Re-export from the saved project without re-analysing:
`analyzeHeadless <out>/proj WildStarP -process WildStar_p.exe -noanalysis -readOnly -scriptPath tools/ghidra -postScript DecompileWildStar.java <out file>`.

Last runs (2026-10-17, Ghidra 12.1.4, macOS): `WildStar_d.exe` 79,330 functions, 137 unnamed, 1 decomp
failure; `WildStar_p.exe` 31,788 functions, 103 unnamed, 0 failures. Check the log, the header line and
these counts, not the exit code.
