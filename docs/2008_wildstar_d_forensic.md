# WildStar_d.exe — full forensic + decompilation pass

*Generated 2026-09-13. Companion to [2008_build_inventory.md](2008_build_inventory.md) and [binary_recon.md](binary_recon.md).*

Full forensic pass on `game-files/Bin360/WildStar_d.exe` (the debug Xbox 360 PowerPC PE) and its paired 67 MB `WildStar_d.pdb`. Every output referenced below is on disk — no summaries stand alone.

**✅ Status (v4, 2026-09-13 14:03): full decomp landed.**
- `.text` coverage: **97.5%** (was 26% in v1, 35% in v3).
- Functions decompiled: **79,330** (48,667 → 71,680 → 79,330).
- Named coverage: **99.8%** (177/48k → 33% → 79,193/79,330).
- Only 137 `FUN_` remain; only 7 decomp fails (0.009%).
- Decomp file: **107.4 MB** (up from v1's 25.6 MB).

The winning recipe (implemented, reproducible):
1. **[GhidraXenon extension](https://github.com/freeqaz/ghidra-xenon-extension)** — drop into `<ghidra_install>/Ghidra/Extensions/GhidraXenon/`. Adds `PowerPC:BE:64:Xenon` language with **77 VMX128 opcodes** + AltiVec.
2. **[`tools/ghidra/PrepWildStarPreAnalysis.java`](../tools/ghidra/PrepWildStarPreAnalysis.java) pre-script** — disables the crashing `PdbUniversalAnalyzer` and walks `.pdata` (Xbox 360 stores full VA in BE, **not** RVA) to `createFunction()` at every one of 68,571 unwind anchors before auto-analysis.
3. **[`tools/ghidra/ApplyMapSymbolsWildStar.java`](../tools/ghidra/ApplyMapSymbolsWildStar.java) post-script** — adapted from GhidraXenon's own bundled `ApplyMapSymbols.java`. Parses the "Publics by Value" section of `Bin360/WildStar_d.map` (119,069 symbols) and renames each `FUN_` at the matching VA.
4. **[`tools/ghidra/DecompileWildStar.java`](../tools/ghidra/DecompileWildStar.java) post-script** — full-corpus decomp export.

Still deferred: **PDB types + function signatures.** Ghidra's `PdbUniversalAnalyzer` crashes at `DefaultPdbApplicator:299`; `pdb.exe` and `DIA2Dump` (which I built successfully from the VS BuildTools sample) both fail with `CoCreateInstance HRESULT 80040154` — msdia140.dll needs `regsvr32` in an admin shell. Fix: patch `DIA2Dump.cpp` to use `NoRegCoCreate` (loads msdia without registration) — ~10 lines of C++.

The PE / PDB-strings / linker-map / XEX forensic dumps remain complete and tool-independent.

## Machine-readable outputs (all under `output/`)

Ghidra decomp (v4, produced with GhidraXenon extension + PrepWildStar pre + ApplyMapSymbols post + DecompileWildStar post):
- [`output/_ghidra_wildstar/wildstar_d_all_functions_decomp.txt`](../output/_ghidra_wildstar/wildstar_d_all_functions_decomp.txt) — **79,330 functions, 107.4 MB**, PPC-with-VMX128 decomp of every non-thunk function in an exec block. **99.8% named** (79,193 named, only 137 `FUN_` remain, 7 decomp fails). Grep this file.
- [`output/_ghidra_wildstar/proj/`](../output/_ghidra_wildstar/proj/) — the Ghidra project; open `WildStarD.gpr` in the GUI. Uses `PowerPC:BE:64:Xenon` language.
- [`output/_ghidra_wildstar/analyze.log`](../output/_ghidra_wildstar/analyze.log) — full analyzeHeadless log (only 17 pcode WARNs vs v3's 17,463 — VMX128 is decoded).
- Preserved historical artefacts for comparison: `wildstar_d_all_functions_decomp.PARTIAL.txt` (v1), `wildstar_d_all_functions_decomp.v3.txt`, `analyze.v3.log`, `analyze.v4_ext_missing.log`, etc.

PE forensic (`output/wildstar_d_forensic/`):
- `pe_header.txt` — machine, ImageBase, entry, section counts, sizes.
- `pe_sections.tsv` — every section with VA / VSize / raw ptr / raw size / characteristics / entropy.
- `pe_imports.tsv` — 304 imports across 3 DLLs (ordinal-only).
- `pe_exports.tsv` — 176 exports (all Scaleform GFx `G*` classes).
- `pe_pdata_funcs.tsv` — **68,571 PPC64 unwind entries** (each a function boundary anchor with prolog length + instruction count).
- `pe_strings.txt` — 70,717 printable strings with VA prefix.
- `pe_xbld_metadata.txt` — raw hex of `.XBLD` (176 B) and `.XBMOVIE` (12 B) Xbox 360 sections.
- `summary.json` — one-stop machine-readable header.

PDB forensic (`output/wildstar_d_forensic/`):
- `pdb_msf_header.txt` — MSF v7 header: page size 1024, 68,291 pages, 281,896 B directory.
- `pdb_all_strings.txt` — **802,521 ASCII strings** with file offset.
- `pdb_source_paths.txt` — **5,774 unique source paths** (see subdir breakdown below).
- `pdb_ws_class_names.txt` — 2,029 candidate `WS*/Pbl*/Pcl*/Odin*/hk*/G*` class names.
- `pdb_function_symbols.txt` — 33,719 mangled C++ symbols (`??…`).
- `pdb_bucket_{file_paths,assertion_strings,lua_names,wildstar_paths,xenon_paths}.txt` — categorized subsets.
- `pdb_summary.json` — counts.

Linker map (`output/wildstar_d_forensic/`, parsed from `Bin360/WildStar_d.map`):
- `map_sections.tsv` — 12 loadable sections.
- `map_publics.tsv` — **99,617 public symbols** with `name / VA / section:offset / source lib:obj`.
- `map_by_lib.tsv` — symbol count per source library (see middleware census below).
- `map_ws_publics.tsv` — the 12,926 `WS*`-prefixed rows.
- `map_summary.json`.

XEX headers (`output/wildstar_d_forensic/xex_*.txt`) — every `.xex` in Bin360, decoded:
- `WildStar_d.xex` (20.5 MB, PE at 0x4000, 11 optional headers)
- `WildStar_p.xex` (13.3 MB, PE at 0x4000, 11 opt hdrs)
- `WildStar_p-org.xex` (11.1 MB, PE at 0x3000, 11 opt hdrs — older layout, pre-hotpatch)
- `Saboteur.xex` (11.1 MB, PE at 0x3000, 11 opt hdrs)
- `ReplayCore-xenon.xex` (2.7 MB, PE at 0x2000, 8 opt hdrs)
- `ReplayLauncher-Xenon.xex` (3.8 MB, PE at 0x2000, 10 opt hdrs)

The PE payload inside every `.xex` is encrypted/compressed; we can't read it directly without an xex unpacker. The raw PC-format `.exe` companions (`WildStar_d.exe`, `WildStar_p.exe`) contain the same code readable, which is what we decompiled. Only `Saboteur.xex` has no unwrapped `.exe` companion — unwrapping it is a follow-up.

## PE header — WildStar_d.exe

| Field | Value | Meaning |
|---|---|---|
| Size | 22,340,608 B (21.3 MB) | |
| Machine | `0x01F2` | `IMAGE_FILE_MACHINE_POWERPCFP` (Xenon / Xbox 360 PPC) |
| PE Magic | `0x010B` | PE32 (not PE32+; Xbox 360 is 32-bit-addressed PPC64) |
| ImageBase | `0x82000000` | Standard Xbox 360 loaded base |
| Entry VA | `0x8322E4A0` | ~9 MiB into `.text` |
| Sections | 12 | See table below |
| Timestamp | `0x4822549C` | 2008-05-07 18:17:16 UTC (linker; older than the 2008-05-22 mtime — the exe was built earlier than the drop was frozen) |

### Section table

| Section | VA (rel) | VSize | Raw size | Chars | Entropy | Notes |
|---|---:|---:|---:|---|---:|---|
| `.rdata`   | 0x00000400 | 2,795,028 | 2,795,520 | R    | 5.784 | consts, RTTI, vtables |
| `.pdata`   | 0x002AAC00 | 548,568 | 548,864   | R    | 6.024 | PPC64 unwind → 68,571 function entries |
| `BINKBSS`  | 0x00330C00 | 10,792 | 11,264     | R    | 6.351 | RAD Bink BSS |
| `.text`    | 0x00340000 | **17,174,272** | 17,174,528 | RX   | 6.171 | code |
| `BINK`     | 0x013A1000 | 80,792 | 80,896     | RX   | 6.589 | Bink code |
| `.data`    | 0x013C0000 | 6,844,516 | 259,584 | RW   | 4.973 | huge virtual, small raw = BSS-heavy |
| `.XBMOVIE` | 0x01A47200 | 12   | 512         | RW   | 0.037 | 12-byte Xbox video marker |
| `BINKDATA` | 0x01A47400 | 15,752 | 15,872     | RW   | 7.825 | Bink data |
| `.edata`   | 0x01A50000 | 7,544  | 7,680      | R    | 5.868 | 176 exports |
| `.idata`   | 0x01A60000 | 1,382  | 1,536      | RW   | 3.731 | 304 imports (ordinal-only) |
| `.XBLD`    | 0x01A70000 | 176  | 512         | R (Xbox-mark) | 1.662 | Xbox 360 build stamp |
| `.reloc`   | 0x01A70200 | 1,442,352 | 1,442,816 | R (Xbox-mark) | 6.957 | relocation table |

`.pdata` giving 68,571 function starts is the strongest ground-truth function boundary source we have — a second oracle beyond PDB and linker map.

## Imports and exports

**Imports:** all ordinal-only (no name strings), from 3 Xbox 360 system libraries:
- `xboxkrnl.exe@6274.0+1861.0` — Xbox 360 kernel (`Xam*`, `XNet*`, `XamUser*`)
- `xam.xex@6274.0+1861.0` — Xbox app manager (dashboard, sign-in, marketplace)
- `xbdm.xex@6274.0+1861.0` — Xbox debug monitor (only present in debug builds; ships enabled)

The `@6274.0+1861.0` tag identifies the XDK build revision — matches the **August 2007 Xenon XDK** in the PDB paths.

**Exports:** 176 names, all C++ mangled Scaleform GFx foundation classes (`GBufferedFile`, `GEvent`, `GImage`, `GLock`, `GMutex`, `GRefCountBase`, `GSemaphore`, `GSysFile`, `GThread`, `GWaitCondition`, `GZLibFile`, `GAcquireInterface`, `GDefaultAcquireInterface`). These are re-exported from the statically-linked Scaleform build.

## Middleware census (from linker-map source-library counts)

The `Lib:Object` column in the linker map identifies the origin of every public symbol. Top libraries by contributed symbol count:

| Rank | Library | Symbols | Meaning |
|---:|---|---:|---|
| — | *(LTCG merged, `f`)* | 27,070 | anonymous link-time-code-generation objects |
| 1 | `hkdynamics` | 2,515 | Havok Dynamics |
| 2 | `d3d9d` | 1,916 | Direct3D 9 debug |
| 3 | `hkcollide` | 1,673 | Havok Collision |
| 4 | `LIBCMTD` | 1,386 | MSVC C runtime debug |
| 5 | `AkAudioEngine` | 1,248 | Wwise (Audiokinetic) |
| 6 | `d3dx9d` | 1,111 | D3DX 9 debug |
| 7 | `hkutilities` | 1,105 | Havok utilities |
| 8 | `hkbase` | 945 | Havok base |
| 9 | `xaudiod` | 824 | Xbox 360 XAudio (debug) |
| 10 | `hkserialize` | 692 | Havok serialization |
| 11 | `AkLowerEngine` | 632 | Wwise lower engine (mixer/DSP) |
| 12 | `ProxyCentral` | 617 | *(Pandemic internal — unresolved)* |
| 13 | `gfx` | 589 | Scaleform GFx |
| 14 | `AkMusicEngine` | 507 | Wwise music engine |
| 15 | `hkinternal` | 463 | Havok internal |
| 16 | `hkvehicle` | 453 | Havok vehicle dynamics |
| 17 | `XAPILIBD` | 400 | Xbox 360 API library (debug) |
| 18 | `hkanimation` | 366 | Havok animation |
| 19 | `AkStreamMgr` | 269 | Wwise stream manager |
| 20 | `hkvisualize` | 260 | Havok visual debug |
| 21 | `hkscenedata` | 260 | Havok scene data |
| 22 | `xgraphicsd` | 257 | Xbox 360 graphics helper |
| 23 | `Odin` | 225 | Pandemic renderer (PC-side lib name) |
| 24 | `hkconstraintsolver` | 209 | Havok constraint solver |

Full third-party inventory: Havok (11 sublibs, ~8,900 symbols), Wwise (5 sublibs, ~2,900 symbols), Scaleform GFx (589), Direct3D 9 + D3DX 9 (3,000+), RAD Bink (via `BINK*` sections). Pandemic internal: `Odin` (renderer, 225 direct), `ProxyCentral` (617 — unknown role — investigate).

## Source-tree structure

From 5,774 unique `d:\...` paths in the PDB, sorted by count:

- **`d:\projects\wildstar\` — 3,488 files.** The main WildStar engine + game source. Subpaths seen include `code\wildstar\`, `pov\`, `build\temp\debugxbox 360\` (compiler intermediates: `.inj` files).
- **`d:\xenon\aug07\` — 1,758 files.** The Xbox 360 XDK (**August 2007**), so the game was built against XDK version 6274.0. Path style: `d:\xenon\aug07\core\private\xtl\graphics\d3d\api\xbox\obj\xbox\precomp.obj`.
- **`d:\hk\rc\` — 522 files.** Havok release candidate: `d:\hk\rc\build\code\current_release_complete\obj\xbox360_net_8-0\fulldebug_multithreaded\hkcompression\hkblockdecode.obj`. The `xbox360_net_8-0` config tag identifies the Havok build (retail Saboteur ships spline-compressed animations only, matching hkcompression; retail is documented as **Havok 6.5** in [binary_recon.md](binary_recon.md) — this earlier build may match or differ; verify against `hkVersion*` strings in a follow-up).
- Typos/outliers: 1 file each under `d:\projects\wbuild\`, `d:\projects\wiildstar\`, `d:\projects\wil\`, `d:\xeles\wstrain.cpp`, `d:\projec…`, `d:\projectsd\…`.

File type counts inside those paths: **3,822 `.cpp`, 6,289 `.h`, 1,044 `.inj`**. The `.inj` files are MSVC's per-cpp *incremental link injection* records — one per translation unit. So the WildStar codebase at freeze is ~11,000 files.

## PDB provenance highlights

Grep-friendly data left in the PDB:

- Full source paths (5,774) — every .cpp/.h/.obj/.inj/.lib/.pdb that fed the linker.
- Assertion strings — 7,092 rows in the `assertion_strings` bucket (`assertCRC`, `assertBuf`, `LuaAssert`, `pLuaGlueItemAssert`, `bAssert`, `pVerify`, `CacheNode::VerifyCachedData`, plus billing hooks `ACCOUNT_VERIFY_BILLING_INFO_REQUEST` etc. from XDK).
- 9,218 `Lua`-flavored strings — the Lua binding surface, larger than retail's 898 `LuaGlueFunctor` names because debug keeps more.
- 12,292 `wildstar`-containing strings and 9,690 `xenon`-containing strings — dev-machine paths and Xbox 360 SDK path fragments.
- ~2,029 candidate WS/Pbl/Pcl/Odin/hk/G class names in raw strings — the class inventory is **2.5× larger than retail's 823 named `WS*` classes**, because debug builds keep templates + inline classes that ship-time DCE strips.

Engineer names surfaced so far: **Justin Truman** (from `Bin360/Asserts.TXT`, author of a `WSJoystick::atoInputType` assert). Two more names live in `pdb_bucket_assertion_strings.txt` — look for `'s GENERAL ASSERT FAILED` patterns.

## Named function samples (from the joined decomp)

Cross-referencing the map into Ghidra's decomp yields 16,166 named functions. Sample calls that resolve in the named file:

- `?updatePhantom@hkVehicleRaycastWheelCollide@@…` (Havok vehicle raycast)
- `?integrateInternal@hkSimulation@@…` (Havok stepper)
- `?ComputeLPFCoefs@CAkSrcLpFilter@@…` (Wwise LPF)
- `?GetSpeakerVolumesdB@CAkSpeakerPan@@…` (Wwise mixer)
- `?SetVolumeThreshold@SoundEngine@AK@@…` (Wwise public API)
- `?FindMovie@WSHUDManager@@…` and `?DeleteMovie@WSHUDManager@@…` (GFx movie manager)
- `?PlayAudibleEvent@WSAudible@@…` and `?StopAudibleEvent@WSAudible@@…` (audio event surface)
- `?StartEngine@WSSoundVehicle@@…` (per-vehicle engine audio)
- `?Focus_UpdateCamera@WSFocusPtManager@@…` (camera focus system)
- `?UpdatePhantom@WSPhysicsVehicle@@…` (vehicle physics)
- `?SetInputVelocity@WSHumanAnimator@@…` (character animator input)
- `?CheckBlockAnim@WSHumanStateMelee@@…` (melee state machine block detection)
- `?SetupParticlesFromMesh@WSParticleClusterBlueprint@@…` (particle system)
- `?SetProperty@WSFoliageObject@@…` (foliage property system)
- `?EvalTrigger@WSMultiGridNode@@…` (multigrid trigger regions)

### New class family only seen on Xbox: `XenonOdin`

The `?SetContext@XenonOdin@@IAAJAAURenderContext@@@Z` symbol reveals **`XenonOdin`** — an Xbox-360-specific derived variant of the `Odin` renderer that isn't in retail PC. Sibling of PC's `Odin*` family (documented as ~38 renderer classes in [binary_recon.md](binary_recon.md)). Follow-up: enumerate every `XenonOdin*` symbol.

## ⚠️ v3 status: what improved, what's still missing

**v3 wall time:** 9 min 6 sec (13:01:50 → 13:10:56). Auto-analyze phase this time was **352 sec** (5.9 min) — the AutoAnalysisManager summary shows real per-analyzer work (Stack 78s, PPC Constant Reference 57s, Disassemble Entry Points 56s, Decompiler Switch Analysis 52s, Non-Returning Functions Discovered 38s). v1's 3.5 min was too fast because Ghidra had far fewer functions to analyze.

### v1 vs v3 vs v4 side-by-side

| Metric | v1 | v3 | **v4** |
|---|---:|---:|---:|
| Wall time | 5m 44s | 9m 06s | **20m 10s** (real analysis) |
| Functions decompiled | 48,667 | 71,680 | **79,330** |
| Total function bytes | 4.54 MB | 6.05 MB | **16.7 MB** |
| **`.text` coverage** | 26.4% | 35.2% | **97.5%** |
| Mean function size | 93 B | 84 B | **211 B** |
| Bodies ≥ 401 B (real large fns) | ~1,700 (3.5%) | 2,067 (3%) | **9,027 (11.4%)** |
| Named functions (final) | 177 (0.4%) | 23,231 (32%) | **79,193 (99.8%)** |
| Decomp fails | 27 | 31 | **7 (0.009%)** |
| VMX128 pcode WARNs | 12,225 | 17,463 | **~0** |
| Decomp file size | 25.6 MB | 35.0 MB | **107.4 MB** |

Function size distribution, v4:

| Size bucket | Count | Share |
|---|---:|---:|
| ≤ 12 B (branch/nop stub) | 953 | 1.2% |
| 13-40 B (small wrapper) | 13,324 | 16.8% |
| 41-100 B (short helper) | 32,353 | 40.8% |
| 101-400 B (real function) | 23,673 | 29.8% |
| 401-2000 B (large function) | 8,202 | 10.3% |
| > 2000 B (very large) | 825 | 1.0% |

Compare retail x86 Saboteur.exe: 36,935 fns / 12 MB .text / ~30% ≥ 200 B. Debug build has ~2× the function count (no inlining) and ~1.4× the code (asserts + checks), which matches.

### v3 largest named functions (from the joined decomp)

| Size (B) | VA | Function |
|---:|---|---|
| 10,612 | 0x82f148f0 | `inflate` (zlib inflate — statically linked) |
| 8,460 | 0x83112a78 | `PblInverse(PblMatrix4x4&, const PblMatrix4x4&)` |
| 5,844 | 0x82908410 | `WSAIPathfinderTaskStatic::Evaluate` |
| 5,740 | 0x82a3fb68 | `WSFarCloudState::SetPresets` |
| 4,552 | 0x82f09218 | `GStrokerAA::calcBevelJoin` (Scaleform GFx stroker) |
| 4,460 | 0x824e4218 | `WSGlobalHumanParams::SetProperty` |
| 4,440 | 0x8325be10 | `CReverbEffect::SetParam` (XAudio reverb) |
| 3,972 | 0x8331b4f8 | `hkMoppEarlyExitObbVirtualMachine::queryOBBOnTree` |
| 3,888 | 0x82cdf0b8 | `hkMoppSphereVirtualMachine::querySphereOnTree` |
| 3,768 | 0x83023bb8 | `CAkResampler::Interpolating_I16_Stereo` (Wwise resampler) |

Those 10 are the tallest tent-poles. Bodies at that size are real, substantial decomps. They're where cross-referencing is worthwhile.

### What's still wrong — the 65% `.text` gap

The core problem now is **not** function boundaries (fixed by the pre-script). It's that Ghidra's stock PowerPC processor doesn't decode Xenon's **VMX128 SIMD** instructions. When Ghidra hits a VMX128 opcode inside a function body, it emits a pcode error and truncates the function there. So `size=` on the decomp header is really "code up to the first VMX128 hit," not the real function extent.

Two data points:
- Only **9.0% of functions are ≥ 200 bytes** in v3, vs ~30% for retail's PC x86 decomp. Debug builds should have *bigger* mean bodies than retail, not smaller. That 3× gap is VMX128 truncation.
- **17,463 pcode WARNs** in the log, all of the form "Unable to resolve constructor at 0xXXXXXXXX" — that's the SLEIGH decoder saying "no instruction defined for this bytepattern," and every one of them is a VMX128 opcode.

Fixing this needs Ghidra's PPC processor extended with VMX128 SLEIGH. Options:
1. **Community VMX128 extension** — search Ghidra community repos for an existing `.sla` extension. Simplest if it exists; needs vetting.
2. **Build our own minimal VMX128 stub** — even a SLEIGH that just decodes each VMX128 opcode as `pcode: null` (nop for the decompiler) would prevent truncation. Bodies would decompile *without* the SIMD math, but they'd complete.
3. **Skip PPC entirely — use a PPC decompiler that has VMX128** — Xenia (Xbox 360 emulator) has a full PPC + VMX128 disassembler; extracting a name→body map via Xenia is possible but bigger lift.

Once VMX128 is handled, expected coverage jumps toward 80-90% and mean body toward 200 B+. That's the real remaining work.

## ⚠️ v1 report (kept for context)

The Ghidra run took **5 min 44 sec end-to-end**, which is 5-10× faster than expected. Investigating that anomaly revealed two serious problems that make this pass a **preliminary reconnaissance**, not a complete decompilation:

1. **PDB analyzer crashed with an exception**, not just "failed to attach." Log shows:
   ```
   ERROR (PdbUniversalAnalyzer) …
     at ghidra.app.util.pdb.pdbapplicator.DefaultPdbApplicator
       .applyDataTypesAndMainSymbolsAnalysis(DefaultPdbApplicator.java:299)
     at ghidra.app.plugin.core.analysis.PdbUniversalAnalyzer
       .doAnalysis(PdbUniversalAnalyzer.java:213)
     …
   ```
   Ghidra caught it and continued analysis without symbols/types. That's why only 177/48,667 functions carry PDB names (0.36%). The 33.6% coverage I claim below was rescued post-hoc from the plain-text linker map, which has names but no **types, function signatures, or source-line map** — all of which the PDB would have provided if it hadn't crashed. Likely cause: Ghidra 12.1's PDB Universal analyzer chokes on Xbox 360 PPC PDBs (endianness, or the ancient MSVC 8-era PDB layout with 1024-byte pages). Diagnosis + fix is a real follow-up, not a "one-liner script."

2. **Only 26.4% of `.text` was decompiled.** `.text` is 17.2 MB. The 48,667 functions Ghidra decompiled sum to only **4.5 MB** of code. **12.6 MB (73.6%) of the executable has no function defined at all.** `.pdata` gives us 68,571 function-start anchors — Ghidra classified only 48,667 as functions, and even most of *those* are short:

   | Size bucket | Count | Share |
   |---|---:|---:|
   | ≤ 12 B (1-3 PPC instrs — branch/nop stub) | 13,893 | 28.5% |
   | 13-40 B (small wrapper) | 7,546 | 15.5% |
   | 41-100 B (short helper) | 15,698 | 32.3% |
   | 101-400 B (real function) | 9,839 | 20.2% |
   | 401-2000 B (large function) | 1,637 | 3.4% |
   | > 2000 B (very large) | 54 | 0.1% |

   Only 11,530 items (23.7%) are ≥ 100 bytes. The interesting 12.6 MB of `.text` — presumably packed with the WSHuman/WSVehicle/WSMission/hkSimulation bodies — is dark. Auto-analysis finished in ~3.5 min instead of the expected 30-60 min because it did far less than a full pass.

3. **VMX128 SIMD instructions cause pcode errors.** Xbox 360's Xenon CPU has VMX128, Ghidra's stock PowerPC does not. 27 of 48,667 functions failed outright; more had partial pcode warnings. A community VMX128 processor extension exists.

4. **Linker map name coverage: 33.6%** — better read as "16,166 of 48,667 Ghidra functions matched a map VA." Because the map has 54,302 unique VAs, ~38,000 named symbols didn't land on any Ghidra function entry — many because Ghidra never created a function there (the 12.6 MB gap above), others because the map "publics" include vtables, string literals, and RTTI descriptors that aren't code.

5. **Addresses are Xbox-360 VAs** (base 0x82000000, big-endian PPC), not PC (base 0x00400000, little-endian x86). **Do not** cross-apply these VAs to retail — the [pc_symbol_map.tsv](../data/symbol_map/pc_symbol_map.tsv) already documents the name-transfer method: RTTI vtable slot alignment plus assert-string anchors, gated to layout-stable classes.

**What this run actually delivered**, honestly restated: a partial decomp of the easy quarter of `.text` (mostly small helpers and vtable thunks), plus complete, tool-independent forensic dumps of the PE, PDB (via raw string extraction), linker map, and every XEX header. The full-code decomp is the follow-up.

## Follow-ups — the actual re-run plan

**The right re-run has three phases, in order.** Skipping any of them just re-produces this pass.

### Phase 1 — force full function-boundary discovery from `.pdata`

The 12.6 MB gap is the biggest single problem. `.pdata` gives us 68,571 known function-start VAs. Write a Ghidra pre-script that iterates those VAs and calls `createFunction(va)` for each before auto-analysis runs. That way Ghidra's own analyzers work on the full function set instead of only the ~half it found on its own. Expected: `.text` coverage jumps from 26% toward 90%+; total decomp'd functions rises past 68k.

### Phase 2 — get the PDB loaded, or work around it

Ghidra 12.1's `PdbUniversalAnalyzer` crashes on this PDB (line 299 of `DefaultPdbApplicator.applyDataTypesAndMainSymbolsAnalysis`). Three routes:

1. **Try Ghidra's deprecated legacy PDB analyzer** (`Ghidra/Features/PDB/…/PdbAnalyzer.java`). It's slower but handles some older PDBs the Universal one refuses.
2. **Convert PDB → PDB.XML with Microsoft's `pdb.exe`** (part of the Ghidra PDB feature) then load the XML instead. Sidesteps the Universal parser's binary path entirely.
3. **File-level PDB inspection** — build the `DIA2Dump` sample under `C:/Program Files (x86)/Microsoft Visual Studio/18/BuildTools/DIA SDK/Samples/DIA2Dump/` against the installed `msdia140.dll`, run it on `WildStar_d.pdb`, ingest its text output back into Ghidra as a name/type map via a small Java script. Bulkier but bypasses Ghidra's PDB stack.

Any of these gets us **types** (struct layouts, class hierarchies, function signatures with parameter types) that the linker map fundamentally can't provide.

### Phase 3 — VMX128 processor extension + longer analysis timeout

Only then re-run headless with:
- The community VMX128 processor extension installed (fixes the 27 outright fails + hundreds of partial pcode warnings)
- `-analysisTimeoutPerFile 21600` (6h) — the current 3h was fine because analysis ran short, but if analysis actually does its job it will take real time
- Decompile timeout raised from 60s to 300s in `DecompileWildStar.java`

Expected total wall time: **hours, not minutes**. That's the honest number.

### Independent follow-ups (unblocked by the above)

- **Enumerate `XenonOdin*`** and diff against PC's `Odin*` to see the Xbox-only renderer surface. Doable now from the linker map alone.
- **Cross-reference `hk*` version strings** against retail's Havok 6.5 finding. Confirms or refutes middleware continuity 2008→2009.
- **Unwrap `Saboteur.xex`** with a stand-alone tool (`xextool.exe`, or `iXtreme/xex2` from the Xbox 360 SDK modding scene) so the ship game binary can be Ghidra-loaded like `WildStar_d.exe` was.
- **Investigate `ProxyCentral` (617 symbols)** — the only middleware library above 500 symbols that isn't Havok/Wwise/D3D/GFx/Bink/XDK. Probably a Pandemic-internal networking or telemetry lib.
- **Extend `pc_symbol_map.tsv`** with the linker map's name set — even without the re-run, the map alone gives us more candidate names than the previous join used.
- **Repeat for `WildStar_p.exe`** (profile build) once the recipe is proven on the debug build.
