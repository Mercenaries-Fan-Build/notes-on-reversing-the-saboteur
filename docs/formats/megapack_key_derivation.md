# Megapack index key — SOLVED

> **Adjudication (double-blind, 2 investigators).** Both independently confirmed
> `index = pandemic_hash(name)` (759/759). They diverged on `crc`: one derived
> `crc = pandemic_hash("global\<name>.dynpack")`; the other reported it non-reproducible with a
> `CinematicTextures` counterexample. **Adjudicated in favour of the formula** — it reproduces the
> real crc on *every* entry tested, including the counterexample: `pandemic_hash("global\Cinematic`
> `Textures.dynpack") == 0x0CCCD1DB` (the real crc); the negative result had tested the string
> *without* the `global\` prefix. Cross-verified here on `PauseMenu`, `CinematicTextures`,
> `Act1_IntKey`, `AMBCat_CellKey` — all match.
>
> **Generalization (from the loader `sprintf` call sites the 2nd investigator found):** the pre-hash
> string is `"<name>.<packtype>"` under a folder — `FUN_009f2530` builds `"<name>.dynpack"`,
> `FUN_009f1520` builds `"<name>.palettepack"`, `FUN_00a037f0` builds `"France\EditNodes\<name>"`.
> Dynamic0/Palettes0 confirmed empirically; world/startup packs (`Mega*`/`Start0`) use a different
> string still to be pinned. `index` is always `pandemic_hash(name)` and is the by-name resolve key.

**Question:** exactly what string does the engine hash to produce a `.megapack` (MP00) entry's
lookup key, so a modder can register a *new* asset the engine will find by-hash?

**Answer (CONFIRMED — reproduces all 759 `Global\Dynamic0.megapack` keys, and 274 of `Palettes0`'s
321 entries — those resolvable via the rainbow table — from the string alone):**

```
entry (20 bytes on disk) = { u32 path_crc; u32 name_crc; u32 size; u64 offset }

name_crc = pandemic_hash(resourceName)                         # SabTool: FileEntry.Name
path_crc = pandemic_hash("global\" + resourceName + ".dynpack")   # Global\Dynamic0  (THE bsearch key)
path_crc = pandemic_hash("global\" + resourceName + ".palettepack") # Global\Palettes0
```

`path_crc` is the primary key the engine bsearches. `pandemic_hash` is FNV-1a/32 with case-fold and a
finalizer, exactly the engine kernel `FUN_00dc1e20`:

```
h = 0x811C9DC5
for each byte c:  h = ((c | 0x20) ^ h) * 0x1000193      # |0x20 => ASCII case-insensitive
return (h ^ 0x2A) * 0x1000193                            # empty/NULL string => 0
```

Sanity vector: `pandemic_hash("ANY") == 0xED057225`.

## The decisive proof

The two first `Dynamic0` entries whose embedded **mesh** is *both* `P_Keys_KeyRing_A` have **different**
keys — the smoking gun that the key is not the mesh/leaf name:

| resourceName | pre-hash string | path_crc (real) | name_crc (real) |
|---|---|---|---|
| `Act1_IntKey`    | `global\Act1_IntKey.dynpack`    | `0xD3EF69E0` | `0xB333DA43` |
| `AMBCat_CellKey` | `global\AMBCat_CellKey.dynpack` | `0x708B49A0` | `0x5E3513B5` |
| `AI_Alarm`       | `global\AI_Alarm.dynpack`       | `0x4CE0D45C` | `0xAE540515` |

Run the Rust proof: `cargo run -p sab_megapack_key -- "C:/GOG Games/The Saboteur/Global/Dynamic0.megapack"`
→ reproduces both u32s from the name and confirms each derived key is a real entry pointing at an `ALBS`
block. Bulk check (Python, against SabTool's `Hashes.txt` rainbow table): **759/759** Dynamic0 entries and
**274 of Palettes0's 321** entries match — 274 is the count whose names the rainbow table resolves, not
the pack's entry count. *(clarified 2026-07-24: `Global\Palettes0.megapack` declares 321 entries; the
remaining 47 are unverified only because their names are unknown.)*

## The decompilation chain (VAs)

1. **`FUN_00dc1e20` @0x00dc1e20** — the FNV-1a/32 `pandemic_hash` kernel (bytes above).
2. **`FUN_00db7c10` @0x00db7c10** — hash wrapper: `FUN_00dc1e20(str)` then, if flagged, registers
   `hash → str` in the debug string table via `FUN_00db7920` (`FUN_00db7e10` @0x00db7e10 is the
   store-into-object shim).
3. **`FUN_009ef620` @0x009ef620** — the by-name resource resolver. Routes on the request string's
   suffix via `FUN_00db4400(str,"dynpack")` / `("palettepack")` to pick the pack manager, then
   dispatches the hashed key to the megapack lookups.
4. **`FUN_00e350d0` @0x00e350d0 / `FUN_00e35140` @0x00e35140** — walk the mounted pack list.
5. **`FUN_00e42740` @0x00e42740** — the actual lookup: `bsearch(&key, entries, count, 0x18, cmp
   @LAB_00e42610)`; the comparator keys on offset 0 only → **`path_crc`**.
6. **Loader `FUN_00e428c0` @0x00e428c0** reads magic `MP00` (`0x4D503030`), a `u32` count, then per
   entry `u32 path_crc, u32 name_crc, u32 size, u64 offset` (20 bytes on disk), `qsort`s by
   `path_crc`, then reads a second `count*8` table (`BlockPathToNameCrcs`: `(path_crc, name_crc)`
   pairs, a redundant back-map).

Cross-checked against **BoBoBaSs84/Blumster SabTool** (`MegapackSerializer.cs`, `Hash.cs`): its reader
literally registers `global\<Name>.dynpack` and `global\<Name>.palettepack` for every entry, and its
`FileEntry` fields are `Path` (=`path_crc`) and `Name` (=`name_crc`). Independent agreement with the
binary.

## Registering a NEW dynamic asset (modder workflow)

1. Pick a `resourceName` (e.g. `MyMod_Crate`). Case doesn't matter (hash folds case).
2. Build the `SBLA`/`ALBS` bundle for it (mesh/texture/physics).
3. In your `patchdynamic0.megapack`, write the index entry with:
   * `path_crc = pandemic_hash("global\\MyMod_Crate.dynpack")`
   * `name_crc = pandemic_hash("MyMod_Crate")`
   * `size`, `offset` of your block. **On-disk entry order is free** — the loader `FUN_00e428c0`
     `_qsort`s the table in memory after reading it (`_qsort(param_1[0xf0], param_1[0xf1], 0x18,
     &LAB_00e42610)`) and only then does `FUN_00e42740` bsearch it. *(corrected 2026-07-24: this
     previously said "keep entries sorted by `path_crc` (engine bsearches)"; neither retail
     `Dynamic0.megapack` nor `Palettes0.megapack` is sorted by `path_crc` on disk — 381/758 and
     167/320 adjacent pairs respectively are descending.)*
   Also emit the second `(path_crc, name_crc)` back-table.
4. Drop it next to `Global\Dynamic0.megapack`. The engine mounts `patchdynamic0.megapack` at ~1000×
   priority, so any request for `global\MyMod_Crate.dynpack` resolves to your block.
5. Whatever references the asset (blueprint/ECS/Lua) must request that same `resourceName`, since the
   game asks the resource system for `global\<name>.dynpack` and the system hashes that string.

`name_crc` is **independently derivable** (`pandemic_hash(name)`) — it is *not* an ordinal index; the
struct field just happens to be named "index" in earlier notes.

## Confidence table

| Claim | Status |
|---|---|
| `path_crc = pandemic_hash("global\\"+name+".dynpack")` for `Global\Dynamic0` | **CONFIRMED — 759/759 real crcs reproduced** |
| `path_crc = pandemic_hash("global\\"+name+".palettepack")` for `Global\Palettes0` | **CONFIRMED — 274 of the pack's 321 entries reproduced** (the 274 whose names the rainbow table resolves; 0 mismatches) |
| `name_crc = pandemic_hash(name)` | **CONFIRMED — 759/759** |
| `pandemic_hash == FUN_00dc1e20` (FNV-1a/32 + case-fold + finalizer) | **CONFIRMED** (bytes + `"ANY"` vector) |
| On-disk entry = `{path_crc,name_crc,size,offset}` 20 bytes, key=path_crc | **CONFIRMED** (loader `FUN_00e428c0`, comparator `FUN_00e42740`) |
| World packs (`France\Mega*`) key form | **CONFIRMED (2026-10-12)** — `path_crc = pandemic_hash` of the streamblock path or `France\<name>.pack`; the Palettes0 `resourceName` is the `global.map` list-2 name plus `1`. See [world_megapack_key.md](world_megapack_key.md). The kiloPacks (`Start0`, `Start_German0`, `BelleStart0`) are still **UNKNOWN**. |
