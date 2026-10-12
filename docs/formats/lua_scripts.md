# Lua script packs (`.luap`)

The `.luap` pack is the **cheapest entry point into the game's content** — a flat, uncompressed
container that sits loose in the install root and needs none of the megapack → SBLA → MSHA machinery
in [`archive_and_models.md`](archive_and_models.md).

Format derived from the loader in the clean decomp, then **confirmed byte-for-byte against retail**
(`tools/saboteur_lua`: 321/321 chunks valid, 321/321 hashes reproduced). Nothing below is guesswork
except where explicitly marked open.

## Containers

| File | Loader | Status |
|---|---|---|
| `LuaScripts.luap` | `FUN_00706670("LuaScripts.luap", 0)` | ✅ 5,004,097 bytes, **321 entries**, parsed |
| `LuaMissions.luap` | `FUN_00706670("LuaMissions.luap", 1)` | ⚠️ **not shipped in retail** — loader references it, file is absent from the install root |
| `Scripts\Modules` | `FUN_006fa920("Scripts\\Modules", 0, 1)` | ⚠️ directory loader; no such dir on disk. See the flag byte below — `Modules\*` ships *inside* `LuaScripts.luap` |
| `DLC/01/Scripts/*.lua` | — | plaintext source, not compiled |

Both `.luap` names go through the **same parser** with a differing second argument, so one reader
handles both if `LuaMissions.luap` ever turns up (it may live inside `loosefiles_BinPC.pack`).

## On-disk layout — `FUN_00706670` @ `0x00706670`

```
u32                count                  // 321
Descriptor[count]                          // 21 bytes each, PACKED
u8[..]             bytecode blob
```

⚠️ The descriptor is **21 bytes on disk**, not 24. The loader allocates a `count * 0x18` array, but
that `0x18` is the *in-memory* stride — the read is five `u32`s plus a **single byte**, so on disk the
entries are packed 21-wide. Reading them at 24-byte stride desyncs immediately.

| Off | Type | Meaning | Evidence |
|---|---|---|---|
| `+0x00` | u32 | **name hash** — `pandemic_hash("d:\" + Scripts-relative + ".luac")` | ✅ 321/321. Key into `FUN_007069d0`; looked up by `FUN_00706910` (a red-black tree find). **[Solved — see below](#-solved-0x00-is-the-hash-of-the-runtime-lookup-name)** |
| `+0x04` | u32 | **`pandemic_hash(basename without extension)`** | ✅ reproduces **321/321** (`AttackAction`, `Checkpoint`, …) |
| `+0x08` | u32 | **absolute file offset** of the chunk | `FUN_007063f0`: `*(desc+8) + *(this+8)`, where `this+8` is the whole-file buffer |
| `+0x0C` | u32 | stored size | `== +0x10` on all 321 entries |
| `+0x10` | u32 | size handed to the Lua loader | `FUN_007063f0`: `*param_3 = *(desc+0x10)` |
| `+0x14` | u8 | **is-module flag** — `0` or `1` | 235× `0`, 86× `1`; **every** `flag==1` entry is under `Modules\` |

Offsets are **absolute into the file**, not relative to the blob: the loader seeks to 0 and reads the
*entire file* into one buffer, so `chunk = file[offset .. offset+size]`. Confirmed — the first chunk
starts at 6745, exactly `4 + 321*21`, and entries are perfectly contiguous.

Chunks are **stored plain**: `stored_size == size` on every entry and all 321 carry valid LuaQ magic,
so no decompression is needed on the default path. (`FUN_007063f0` has an alternate branch through
`FUN_00706090` when `*(this+0x3c0) != 0`; unexercised by retail data.)

## `pandemic_hash` — verified, with a finalizer gotcha

Exact transcription of `FUN_00dc1e20` @ `0x00dc1e20`:

```c
h = 0x811C9DC5;                                  // FNV-1a basis
for (c in str) h = ((c | 0x20) ^ h) * 0x1000193; // case-fold, FNV-1a
return (h ^ 0x2A) * 0x1000193;                   // finalize
```

✅ `hash("ANY") == 0xED057225` — the Mercs 2 lineage claim **holds**, confirmed against the Saboteur
binary rather than assumed.

> **Gotcha that cost an hour:** the finalizer is `(h ^ 0x2A) * PRIME`, i.e. XOR **then** multiply.
> `(h * PRIME) ^ 0x2A` is wrong and silently produces plausible-looking garbage. Case-folding is
> `| 0x20` on the raw byte, so `\` (0x5C) folds to `|` (0x7C) — it is *not* a `tolower()`.

## ✅ Solved: `+0x00` is the hash of the **runtime lookup name**

```
+0x00 == pandemic_hash("d:\" + <Scripts-relative path> + ".luac")
```

✅ Reproduces **321/321** against retail `LuaScripts.luap`, and round-trips from 26/26 real `require()`
strings in the Lua corpus. Worked example:
`pandemic_hash("d:\Scripts\Includes\WRAPPER_Util.luac") == 0xda8b14f5`.

The standing hypothesis above was right: it hashes the **runtime lookup name**, not the compile-time
debug path. The name is built by `FUN_00706190` @ `0x00706190` from three `.rdata` constants that this
doc previously flagged as "would settle it" — now read out of the image: `DAT_00fdc408` = `"d:\"`
(prefix) and `DAT_00fdc414` = `".luac"` (the extension the loader *substitutes* for `.lua`).

That extension swap is why the earlier sweep missed it: `Scripts`-relative **was** tried, but only ever
with the `.lua` the debug path carries. The preimage is neither the shipped path nor the shipped
extension — it is the `d:`-rooted `.luac` name the engine synthesizes at `require` time.

Lookup is a **red-black tree find** (`FUN_00706910`), not a hash bucket — see
[`../sab-engine-lua-seam/04-vm-lifecycle-and-script-objects.md`](../sab-engine-lua-seam/04-vm-lifecycle-and-script-objects.md),
which derives this and traces the full `require` path through `Util.LuaHook_Require`, spliced into stock
Lua 5.1's `package.loaders` at position 2.

## Payload

- Lua **5.1** (`\x1BLuaQ`), uncompressed, debug info intact — 321/321.
- Source paths all rooted at `@D:\projects\WildStar\pov\BinCommon\Scripts\...` (note the leading `@`,
  Lua's source marker).
- Top-level trees: `Experimental/`, `Includes/`, `Managers/`, `Missions/`, `Modules/`,
  `ScriptControllers/`.
- 28,022 unique strings harvested from the bytecode → the seed for the name→hash dictionary. The
  dictionary now exists: [`data/hash_names.tsv`](../../data/hash_names.tsv)
  ([hash_names.md](../hash_names.md)); the scripts' strings name 1,622 EditNodes objects there (9 of
  them named by no other source).

## Why start here

The megapack index is **hash-only — no path strings**. Extract it and you get files named
`0x8f3a21c4`; you need a name→hash dictionary before extraction *means* anything. Lua bytecode carries
asset, mission, and template names as string constants, so decompiling Lua **first feeds the dictionary
the megapack reader will need** — and pairs with the 898 named bindings in
[`../../data/lua_bindings.txt`](../../data/lua_bindings.txt) to make the 36,935-function decomp
navigable.

## Tooling

`tools/saboteur_lua` (Rust, std-only):

```
cargo run --release -- "C:\GOG Games\The Saboteur\LuaScripts.luap" <outdir>
```

Writes `chunks/` (bytecode, named by real source path), `index.tsv` (hash/offset/size/flag/source),
and `strings.txt`. Self-tests `pandemic_hash` and validates chunk contiguity before extracting.

## Status

- Format: ✅ confirmed against retail.
- `pandemic_hash`: ✅ confirmed (`FUN_00dc1e20`).
- Extractor: ✅ built, 321/321.
- Decompilation to source: ✅ done — 321 sources at [`../saboteur-luacd/src`](../saboteur-luacd/src).
- `+0x00` preimage: ✅ **solved** — `pandemic_hash("d:\" + Scripts-relative + ".luac")`, 321/321.

> The engine side of these scripts — all 898 bindings, the marshalling ABI, handles, and the callback
> path — is reversed in [`../sab-engine-lua-seam/`](../sab-engine-lua-seam/README.md).
