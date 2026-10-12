# Lineage & Divergence: The Saboteur vs Mercenaries 2

**Read this before reusing anything from the Mercenaries 2 reverse-engineering.** Both games are
Pandemic Studios titles on the same engine *family* (Pebble core; Mercs 1 → Mercs 2 → Saboteur), but
The Saboteur is a **later engine revision** (internally "WildStar", exe strings say `Odin`). This doc
is the authority on what carries over unchanged, what is conceptually similar but structurally
different, and what is entirely new. Anything not in the **SHARED (identical)** column must be
re-verified against the Saboteur binary/assets before you rely on it.

## SHARED — identical, safe to reuse directly

| Thing | Evidence |
|---|---|
| **Hash algorithm** `pandemic_hash` (FNV-1a + `\|0x20` case-fold + `^0x2A`, `*prime` finalize) | ✅ **CONFIRMED against the binary**: `FUN_00dc1e20` @ `0x00dc1e20`. `hash("ANY")==0xED057225` reproduced; also reproduces `.luap` descriptor hashes 321/321. ⚠️ finalizer is `(h ^ 0x2A) * PRIME` — XOR **then** multiply; the reverse silently yields garbage. See [`formats/lua_scripts.md`](formats/lua_scripts.md). ⚠️ **Identical for ASCII only:** the Saboteur engine sign-extends each byte (`movsx` at `0x00dc1e33`), `mercs2_formats::hash::pandemic_hash_m2` zero-extends — names with a byte `>= 0x80` hash differently. No retail Saboteur template/mesh name has one; `sab_formats::hash` reproduces the engine exactly |
| **`sges` compression** (SEGS header, 8-byte chunk descriptors, raw deflate `-MAX_WBITS`, 64 KB sentinel) | Saboteur `compressed.hpp` == our `sges` decompressor |
| **Lua bytecode = LuaQ 5.1** | `LuaScripts.luap` entries start `1B 4C 75 61 51`; same ChunkSpy/unluac path |
| **f16 vertex positions** | MESH streams use half-float positions |
| **Pebble core library** (`Pbl*`/`Pcl*` classes, hash tables, streams, tasks) | RTTI names in exe; same lineage as Mercs 1 source |
| **CRC-32/BZIP2** kernel (poly `0x04C11DB7`, init/xorout `0xFFFFFFFF`, **non-reflected**) — ⚠️ same algorithm, **different input per game**: re-derive what it covers | Saboteur: the loose-file header's `+0x00` = `crc32_bzip2(lowercased name)`, ✅ **7/7 on retail** `loosefiles_BinPC.pack` ([`formats/editnodes.md`](formats/editnodes.md), `sab_formats::loosefiles`). Mercs 2 uses the same kernel over the **save body** (`ProfileHash`). Note this is *not* Mercs 2's `crc32_mercs2`/FFCS `CSUM`, which is **reflected** poly `0xEDB88320` and does **not** match anything here |
| General methodology: Ghidra headless flow, rainbow-table hash resolution, Wwise→vgmstream approach | our tooling |

## SIMILAR CONCEPT — different structure, DO NOT copy offsets/layout

| Concept | Mercenaries 2 | The Saboteur | Why re-derive |
|---|---|---|---|
| **Archive** | FFCS `.wad` (INDX/DATA/CSUM/ASET/PTHS chunks, 32-bit page offsets) | `MP00` megapack (`{crc,index,size,uint64 offset}` table) → `SBLA` sub-packs | Totally different container; 64-bit offsets; no PTHS |
| **Asset override / patch** | `vz-patch.wad` overlay + FFCS block injection | Built-in `patchmega0` / `patchdynamic0` / `patchpalettes0.megapack` mounted at ~1000× priority (hash wins) | Different mechanism — no block surgery needed |
| **Mesh container** | UCFX chunk tree (GEOM/MESH/PRMG/STRM/IBUF/HIER/INDX/SWIT) | Flat `MESH` header + arrays (streams/primitives/drawcalls) + companion `.dat` VB/IB, wrapped by `MSHA` | Flat vs chunk-tree; buffers externalized |
| **Index topology** | triangle **strips** + degenerate separators | triangle **lists** (`faceType==1`) | Different index decode |
| **Vertex layout** | implicit; stride-guess from `decl` | explicit `format` **bitfield** (`constTag 0x1B`, ~18 codes) | Read the bitfield, don't guess |
| **Materials** | inline `MTRL`/`PRMT` chunks (104-byte preamble) | external `.materials` files, `WSAO` blocks (WSMA/WSTX/WSPA/WSCP/…), drawcall→material by hash | Externalized + structured |
| **Textures** | DDS-in-UCFX (INFO/BODY, cross-block mip assembly) | standalone `DTEX` files (format/dims/mips + multi-stream zlib), embedded name | Self-contained format |
| **Lua storage** | BINN inside `sges`-compressed UCFX blocks | flat `LuaScripts.luap` (count + descriptors), **uncompressed** bytecode, source paths in debug info | Flat + uncompressed |
| **World/map** | FFCS block index + `low_res_terrain` grid | `MAP6` descriptors + `HEI1` heightfield + tile packs | Different world model |
| **DLC** | Xbox STFS → block injection into `vz-patch.wad` | `DLC/01` folder + `MAP6` + loose `.dynpack` (SBLA) | Folder/pack, not injection |
| **Audio package** | PWS streams + wavebanks (IMA/PCM), sounddb | Wwise `1KCP` package → `.wem` (Wwise Vorbis) | Entirely different audio stack |
| **Destruction states** | UCFX shard nodes `<shard>_pristine` / `_ruin` (+ `_interior`) under a SWIT orchestrator; named-state machine drives a per-segment state mask | GameTemplates damage groups → parts → mesh bones (`VO_*`), per-part `Pristine` flag; state 0/1 shows/hides the bones' drawcalls in one MESH ([`damage_and_destruction.md`](damage_and_destruction.md)) | Same idea — geometry on named nodes, state picks which draw — expressed in different containers and names. A sibling, not a copy |
| **Save format** | `.profile` zlib blob, `u32 ProfileHash` @0x00 = CRC-32/BZIP2 over `[4:]` | (unreversed here) | Re-derive from Saboteur — though the CRC kernel above is a reasonable first hypothesis to *test* |

## DIFFERENT — new/incompatible; treat as from-scratch

| Thing | Mercenaries 2 | The Saboteur | Impact |
|---|---|---|---|
| **DRM** | SecuROM v7 (VM-virtualized, 743-site devirt effort) | **none on retail GOG exe** (`.secu` inert) | Direct disassembly; skip the whole SecuROM playbook |
| **Havok** | **5.5** (`hkaWaveletSkeletalAnimation` etc., HK550 structs) | **6.5** (`hkaWaveletCompressedAnimation`, `hkaSplineCompressedAnimation`, +spline) | ⚠️ our verified 5.5 decoder does NOT transfer byte-for-byte; struct offsets & quantization changed. Algorithm (inverse-Haar lifting) is the same → re-derive 6.5 from the clean decomp |
| **Animation pack** | `animgroup` blocks in `vz.wad` | `AP0L` `animations.pack` (ANIM/SEQC/TRAN/EDGE/BANK/SSP0…) + one concatenated HKX blob | Full FSM graph exposed |
| **Engine codename / RTTI** | Mercs 2 engine (mostly `FUN_*`, SecuROM-obscured) | WildStar/Odin, **2,765 RTTI names in the clear** | Saboteur gives a symbol map Mercs 2 lacked |
| **UI** | Scaleform GFx **2.0.48** | Scaleform GFx **3.0.71** | ✅ resolved 2026-07-24 — a **major** version apart. The exe carries it in the clear at file offset `0xC672A0`: `gfxVersion\0\0` `3.0.71\0\0` `gfxArg\0\0` `gfxLanguage`. Assume no GFx API/struct detail transfers. |
| **Reimplementation** | active 64-bit Rust/wgpu reimpl program | not a goal here (yet) | N/A |

## Rule of thumb

- **Names, hashes, compression, Lua-VM, half-floats** → reuse.
- **Any binary layout, offset, struct, chunk tag, or Havok detail** → assume different; verify.
- **Studio techniques and broad mechanisms** (how destruction, LOD, streaming or a container family
  is organised) → often *siblings*: the same idea re-expressed. A Mercs 2 doc is a good map of what
  to look for; it is never evidence of how The Saboteur does it. Find the Saboteur's own version in
  the decomp/data, then record the pairing in the SIMILAR CONCEPT table.
- When in doubt, the **clean Saboteur decomp is the oracle** — the exe is unpacked, so the code that
  parses/produces the format is right there to read.
