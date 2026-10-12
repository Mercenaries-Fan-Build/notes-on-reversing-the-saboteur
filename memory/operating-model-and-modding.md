---
name: operating-model-and-modding
description: How to run this project — double-blind RE studies, Rust tooling, agent-groups for assembly analysis, x32dbg deferred. Plus the modding/write-path program (the community's universal gap).
metadata:
  type: project
---

**★Operating model (user directive, 2026-07-12):**
- **Novel reverse engineering → double-blind studies.** ≥2 independent investigators (isolated
  worktrees, blind to each other) derive the same thing, then a third agent adjudicates the divergences
  against the decomp/real bytes. Proven on Havok 6.5, skeleton, mesh, ANIM binding. Use a hard oracle
  (round-trip byte-equality, cursor-consumes-file, unit-norm, count matches).
- **New durable tooling → Rust** (std-only where feasible; `flate2` for zlib). Small binaries under
  `tools/`, validated by round-trip. See [[symbol-map-methodology]], [[havok65-spline-decode]].
- **Assembly analysis → dispatch GROUPS of agents** to review/categorize/analyze `FUN_*` bodies and
  document ACTUAL engine behavior (deepen docs/symbol_map). Same pattern that produced the 15-subsystem
  map; scalable to on-demand deep dives.
- **x32dbg live debugging → deferred.** Reserve for later in-depth deep-dives (dynamic capture of
  samplers/solvers). Static decomp is the oracle for now.
- Work streams are **parallel**, not sequential — don't force a single priority.

**★Modding / write-path program (the community's universal gap).** Contact: the **Global Mod** author
(the ~only Saboteur mod). The community has extract/read (SabTool, SaboteurToolset, FileConvTool,
LuapExplorer) but NO write/repack. Undocumented even on the saboteur-team wiki: global.map, GameTemplates,
megapack WRITE, and the patch-override.

Our unique assets (from the clean decomp):
- **Patch-override layer** (`FUN_00e34f70`): engine auto-mounts `patchdynamic0.megapack` (prio 0x18704),
  `patchpalettes0.megapack` (0x186fa), `patchmega/patchmega0/patchmega%d.megapack` at ~1000× base
  priority; by-hash lookup, highest wins. ⇒ OVERRIDE existing assets with a small patch pack — NO base
  rebuild, NO global.map edit. Not documented anywhere in the community. See [[archive-and-patch-megapack]].
- **global.map = MAP6** (magic `0x4D415036`, loader `FUN_009f3370`; reads u32 magic via `FUN_0049ee70`).
  Loaded at boot beside `GameTemplates.wsd` and `France.map` (`FUN_009f75f0`). Files: `DLC/01/Global.map`,
  `DLC/01/FRANCE.map` (base copies inside packs).
- **GameTemplates.wsd** = magic `"AULB"` → u32 count → records `{i32 size, 2×i32 unk, len+name, len+type,
  count + (u32 hash BE, i32 dataSize, data)…}` (from Daniel-McCarthy/Saboteur-GameTemplates-Helper source;
  that tool is read-only). This is the disguise/vehicle/object definition file — what the user edits.
- Validated **readers to invert into writers**: sab_mesh/sab_skeleton (MESH), sges, DTEX.

**Write-path streams — wave 1 DONE (double-blind + adjudicated, 2026-07):**
- ✅ **megapack WRITE** → `tools/sab_pack` (49ee123). Format from `FUN_00e428c0` (self-verified): `"00PM"`
  + u32 count + count×20B index `{crc,index,size,u64 offset}` + a **count×8B {crc,index} MIRROR table our
  readers missed** (a writer MUST emit it) + 0x800-aligned verbatim SBLA blobs. `crc` = bsearch key =
  `pandemic_hash(external resource-DB PATH)`, NOT the asset — so repack/override COPY the base entry's
  crc/index (proved: identical bytes, different keys). Byte-identical round-trips ≤3.4 MB. `sab_pack patch`
  emits a `patchdynamic0.megapack` override (mounts prio 0x18704). ⚠️ in-engine load INFERRED, not live-tested.
- ✅ **global.map / MAP6** → `tools/sab_map6` (52b8ab8). Magic `0x4D415036`, loader `FUN_009f3370`. Header
  (16B) + records `{u32 name_hash=pandemic_hash(name), u16 len, name, vec3, vec3 (transform), u16, u16,
  u32 sub_count, sub_count×{u32 asset_hash,u16,u16}, 56B trailer(trailer[3]==sub_count)}`. Consumes
  Global.map exactly (356/356). Add an entry = append + bump record_count.

**Wave 2 — results:**
- ✅ **GameTemplates.wsd (`AULB`)** → `tools/sab_gametemplates` (0872bae). Loader `FUN_0162bfa0`. `"AULB"`
  + u32 entry_count; entries = contiguous Templates or 8-byte Markers (count as slots). Template =
  `{u32 total_size, u32 0, u32 1, len+name, len+type(=C++ class), u32 pair_count, pairs{u32 hash, u32
  size, data}}`. ★Pair hash = **little-endian** `pandemic_hash(property_name)` (community tool had it
  BE — both blind investigators corrected it). Data = LE f32/i32 or a 4-byte `pandemic_hash` asset-ref.
  Round-trips DLC (3969 B) + the full main DB (11072 entries, in `loosefiles_BinPC.pack`).
- ✅ **★MEGAPACK KEY DERIVATION (overturns wave-1!)** → `tools/sab_megapack_key` (9bc0a6b). The index
  entry is `{u32 path_crc; u32 name_crc; u32 size; u64 offset}` and BOTH keys ARE derivable from the
  resource name: `path_crc = pandemic_hash("global\\"+name+".dynpack")` (`.palettepack` for Palettes0),
  `name_crc = pandemic_hash(name)`. Confirmed 759/759 + 274/274; I self-reproduced #0 Act1_IntKey →
  0xD3EF69E0/0xB333DA43. Resolver `FUN_009ef620` routes on the suffix. ⇒ **brand-new assets ARE
  registerable** (not just overrides). Open: world/startup packs (Mega*/Start0) use a different path
  string for path_crc. This is the key hash — the crc "hashes an external path" IS recoverable.
  ADJUDICATED (2nd investigator diverged, claimed crc non-reproducible via a CinematicTextures
  counterexample — but that test OMITTED the `global\` prefix; `pandemic_hash("global\Cinematic`
  `Textures.dynpack")==0x0CCCD1DB` = real crc; verified on PauseMenu/CinematicTextures/Act1_IntKey/
  AMBCat_CellKey). 2nd investigator DID contribute: index=pandemic_hash(name) at full 759 scale (names
  reversed from the hash→string table in loosefiles_BinPC.pack), the resolver chain, and the loader
  sprintf sites that generalize the string: `<name>.dynpack` (FUN_009f2530), `<name>.palettepack`
  (FUN_009f1520), `France\EditNodes\<name>` (FUN_00a037f0).
- ✅ **DTEX** (textures, reskins) → `tools/sab_dtex` (cb3742a). No on-disk magic; length-prefixed records
  in SBLA; u32 nameLen+name (pandemic_hash=ALBS key), u32 format (DXT1/3/5/D3DFMT), flags, u16 w/h/mips,
  u32 uncompressedSize, u32 numStreams, {u32 compSize, zlib}×. Per-mip 24B descriptor; multi-stream split
  at 0x180000. Verified 12,559 reads + round-trip. Recompress isn't byte-exact (2009 deflate) → --preserve
  for exact; edited textures re-inflate fine. Remaining: SBLA sub-pack writer w/ ALBS-directory fixup for
  drop-in multi-texture reskins.

**Wave 3 (parallel double-blind, done):**
- ✅ **SBLA/ALBS sub-pack writer** → `tools/sab_sbla` (892e28f). ALBS header + N×24-byte self-delimiting
  directory {hash,offset,compSize,uncSize,0,flag}; N from offset-chain closure; loader `FUN_00658870`/
  `FUN_0065a900`/`FUN_006617d0`. Object (dir@0x20) + streamblock (dir@0x44) variants. Splice recomputes
  offsets; composes with sab_pack. `scan` = 0 in-scope mismatches on 1000+ real sub-packs. Closes drop-in
  multi-asset bundles. Doc: docs/formats/sbla_subpack.md.
- ✅ **World pack path_crc — SOLVED 2026-10-12** (docs/formats/world_megapack_key.md): every `.megapack`
  `crc` is `pandemic_hash` of a load-time pack path: streamblocks `France\<id>.pack` / `France\<2 digits>\<id>.pack`
  (`FUN_009f5a10`), region records `France\<name>.pack`, dynamics `Global\<name>.dynpack`, palettes
  `Global\<name>1.palettepack`. All 9,210 retail ALBS packs keyed and parsed. kiloPack `crc` still open.
  The earlier "stored PblCRC, preimage only in the cooker" conclusion was WRONG: it read the `"%s\\%s"`
  path builders as display names, and tested `France\1024.pack` where the path is `France\10\1024.pack`.
  ★LESSON (twice now): a negative is only as good as the forms tested — read the string builders in the
  decomp and test what they build before declaring a key unrecoverable.
- ✅ **PC symbol map** → `data/symbol_map/pc_symbol_map.tsv` (1,414 names) — see [[prototype-symbols-goldmine]].

**Still open / next:** wire name→key into `sab_pack` (compute keys, no hex); the world-pack `<name>` source
(x32dbg log-bp on `FUN_00658870`/`FUN_00db7c10`); shift-aware rescue of drifted classes in the symbol map.
**Live validation (x32dbg):** drop a `patchdynamic0.megapack`, breakpoint mount `FUN_00e34f70` + bsearch
`FUN_00e42740` — confirm it loads in-engine (still INFERRED, the last asterisk before shipping to the
community). See [[community-contribution-plan]].
