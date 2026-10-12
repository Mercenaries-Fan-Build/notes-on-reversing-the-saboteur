# The Saboteur (2009) — `EditNodes.pack` (magic `00ED`) format

`EditNodes.pack` is the game's **dynamic-object database**: the NPCs, vehicles, fences, path/traffic
nodes, spawners, sound and zone triggers, locators and door portals that missions **enable and spawn
at runtime** (via Lua). It is where "add an NPC / a car to this mission" actually happens.

There are two flavours on disk, same format:

```
France/loosefiles_BinPC.pack  ->  France\EditNodes\EditNodes.pack   2 606 468 B   767 entries  (main world)
DLC/NN/France/EditNodes/EditNodes.pack                                 ~37 KB      3 entries   (per DLC slot, standalone)
```

The main pack is **embedded** in `loosefiles_BinPC.pack` (a 128-byte-header loose-file container: a
`u32 name_crc`, `u32 size`, `char name[120]` header, the payload, then 0x10 alignment; see `sab_formats::loosefiles`). The DLC packs are
loose files you can open directly.

Parser/writer: [`sab_formats::editnodes`](../../tools/sab_formats/src/editnodes.rs) (shared library).
Inspect with [`sab_probe editnodes <info|list|tree>`](../../tools/sab_probe/README.md). All integers
are little-endian on PC (a byte-swapped console variant of the same layout exists — see below).

Most of this document is read off the engine rather than inferred from the data. The functions that
matter: **`FUN_0064b420`** loads the pack, **`FUN_0064b600`** opens one entry by hash,
**`FUN_00a005a0`** parses an entry's objects, and **`FUN_00dc1060`** is the loose-file reader that
the embedded copy arrives through.

---

## Byte layout

```
Pack
  char  magic[4] = "00ED"
  u32   entry_count
  entry_count × { u32 hash; u32 size; u32 offset }   -- directory; offset is ABSOLUTE
  ... entry bodies, contiguous, in directory order, from directory-end to EOF

Entry body. The root is one of:
  EditNode   (retail is 100% this):  u32 version = 0, u32 object_count, object_count × Object
  Container  (defensive; not seen in retail):  u32 tag(≠0), u32 child_count, child_count × <root>

Object                              -- exactly ONE level of nesting; not a recursive tree
  u32   object_tag     -- pandemic_hash(r"<container path w/o .wsd>\<editor name>") (see below)
  u32   param_count    -- a CHILD count
  param_count × Param

Param
  u32   tag            -- pandemic_hash(paramName), little-endian (see below)
  u32   byte_len       -- always a BYTE count
  u8    data[byte_len] -- verbatim; any structure inside is that tag's private encoding
```

The pack directory is contiguous: the first entry begins exactly at `8 + entry_count*12`, entries are
laid out in directory order (== ascending offset), and the last ends at EOF. A byte-faithful writer
recomputes the whole directory (offsets/sizes) and re-emits the bodies in order.

### The `tag` — `pandemic_hash(paramName)`, little-endian  ★

Each node's 4-byte `tag` is the **[`pandemic_hash`](gametext.md#pandemic_hash) of the parameter's
name, stored little-endian** — the exact convention GameText uses for a UI `asset_id`. So reading the
tag as an LE `u32` yields the hash directly, and `pandemic_hash("Position") == 0x05d7fc60` is the tag
you will find on a Position node.

The community inspector shipped an empirical `tag → name` dictionary; **250 of the 264 names now
hash-verify** against their tag under this convention (`tools/sab_formats/src/editnodes.rs`'s
`TAG_NAMES`, checked by the test `tag_names_sorted_and_mostly_preimages`).

### The names come from the exe — including the "placeholder" families ★

The parameter names are **string literals in `Saboteur.exe`**, in one block at `0x00c4e0b8` owned by
`WildStar/Managers/WSStreamingManager.cpp`:

```
Position  XAxis  YAxis  ZAxis  DynamicPathNodes  Node%.2d  DynamicFenceNodes
DynamicFenceFlags  FenceNode%.2d  FenceNodeFlag%.2d  FenceWallFlag%.2d
Script  Parent  NONE  Type  ClassName
```

Four of them are **`sprintf` format strings**, which is why `Node00…Node40`, `FenceNodeNN`,
`FenceNodeFlagNN` and `FenceWallFlagNN` were previously written off as unrecoverable placeholders.
They are real names, generated at runtime. Expanding the four formats over `i = 0…255` and hashing
reproduces **113 tags in the retail pack exactly** — `Node00…Node24`, `FenceNode00…27`,
`FenceNodeFlag00…27`, `FenceWallFlag00…27`.

That also repaired the table. Seven rows carried the right name on a **key that does not exist in the
retail pack at all** (a mangled byte order in the source dictionary — e.g. `FenceNodeFlag14` sat on
`0xb1b18009` instead of `0xb1388009`, `DynamicFenceFlags` on `0xe84fcffc` instead of `0xe85f4cfc`,
plus `ID1`, `SeatNameRef`, `Node18`, `FenceNode13`, `FenceNode18`). All seven old keys were verified
absent from the retail data before retargeting, and `FenceNode13`'s corrected key absorbed a row
previously labelled `Unknown_Vector_0cbcd8b7` — hence 265 rows became 264.

The 14 that still do not verify are genuine unknowns — `Unknown_*`, `WTFPortal*`, `UnknownTag`,
`Root`, `Raw64_*` — invented labels for real tags whose names were never recovered.

Two cautions when adding names. `pandemic_hash` **case-folds** (`|0x20` per byte), so a name is only
ever known up to case: hashing every string in the exe also "matches" `POSITION`, `flags`, `WIDTH`
and `iD2`, which are the same tags as `Position`, `Flags`, `Width`, `ID2`. And that exe-wide sweep
matched only **131 of the 13 669 distinct tags** in the pack — which is not a gap in the dictionary,
for the reason in the next section.

### Object nodes: the tag is `pandemic_hash` of a PATH ★

> **Correction (2026-07-29).** This section previously concluded that an object's tag was an opaque
> "per-instance unique id, not `pandemic_hash` of anything". That was wrong. It *is* a hash — of a
> path that does not appear anywhere in the file, which is why hashing the file's own strings never
> reproduced it. The evidence below was all correct; the conclusion drawn from it was not.

A node whose children include a `ClassName` is an **object**, and its tag is:

```
object tag = pandemic_hash( <container path, .wsd dropped> \ <object's editor name> )

  Util.GetHandleByName(r"Missions\freeplay\country\mis_giselle_rescue\main\Giselle")
    hashes exactly that string -> the tag of the Giselle object
```

Note this is hashed **differently from its own container** (previous section): a container carries
the `France\EditNodes\` prefix *and* the `.wsd`; an object carries **neither**.

**Verified against retail.** `LuaScripts.luap` stores uncompressed LuaQ 5.1 bytecode, so its string
constants are plain ASCII in the file. Of the path-like literals extracted from it, **1 632 reproduce
an object tag** in the embedded pack — and **0 collide with a directory hash**, so the two namespaces
are cleanly separated. Better, the relationship *composes*: for every one of those whose container is
in the base pack, dropping the last path component and hashing it as a container names the entry the
object actually lives in — **1 620 / 1 620, zero exceptions**. The remaining 12 name DLC or cut
containers.

**★ The editor name is not stored in the pack.** Only ~6.7% of objects have a `ClassName` equal to
their addressable name, and no other string param ever supplies it (brute-forced across every string
param of every object in a path-known entry). The name exists only inside the hash and in Lua. So it
**cannot be recovered** from an EditNodes file alone — but it can be *recomputed*, which is what lets
an editor mint an addressable clone.

This also explains the earlier evidence, all of which stands:

* 13 426 objects, **13 397 distinct tags** — nearly unique because each object's *path* is unique,
  not because the ids are arbitrary. This is why only 131 of 13 669 distinct tags are "named" by the
  parameter dictionary: the rest are path hashes, not parameter names.
* **640 `ClassName` values map to more than one tag** — the same NPC name in different mission
  containers yields different paths, so different hashes. `Spore_RS_Jules` appearing under four tags
  is four *placements*, not four ids for one thing.
* The 13 tags used more than once are **genuine collisions by identical preimage**: same name, same
  container — objects copy-pasted in the editor and never renamed. They are legal but *not
  individually addressable*, since `GetHandleByName` cannot distinguish them.
* Values spread across the whole `u32` range, as hashes do.

There are **three** name-ish things on an object, and the addressable one is never plain text:

```
tag_42f5cd35           {11 children}     <- the NAME, as a hash (see below)
  Type          str 'Spore'              <- the broad engine class
  ClassName     str 'Spore_RS_Jules'     <- the TEMPLATE/archetype -- NOT the instance name
  Position      vec3 (1069.56, 15.5119, -2053.57)
  XAxis/YAxis/ZAxis   vec3 …             <- rotation basis
```

Worked example, verified end-to-end against retail. That object sits in the entry

```
pandemic_hash(r"France\EditNodes\Missions\act_1\characters\farm_jules.wsd") == 0x51a96dc4
```

and Lua addresses it as `Util.GetHandleByName(r"Missions\act_1\characters\farm_jules\jules")`, which
hashes to `0x42f5cd35` — the tag above. The instance name `jules` appears nowhere in the file.
`ClassName` is `Spore_RS_Jules` here *and* on unrelated placements elsewhere, so it identifies the
template, not the object.

> An earlier revision of this section called these two fields "inverted" and read `ClassName` as the
> instance name. That followed from the retracted opaque-id model and is **wrong**: `ClassName` and
> the addressable name coincide for only ~6.9% of objects.

**Consequence for authoring.** Cloning an object *with its tag intact* is legal — retail contains 13
such pairs — but the copy then shares its template's Lua name and neither is individually reachable
through `GetHandleByName`. To make a clone addressable, **retag it**:
`pandemic_hash(r"<container path without .wsd>\<new name>")`. There is no name field to edit; the tag
*is* the name. `sab_formats::editnodes` exposes this as
[`object_tag`](../../tools/sab_formats/src/editnodes.rs) /
[`Entry::duplicate_object_as`](../../tools/sab_formats/src/editnodes.rs), which also refuses a name
already present in the entry.

Because the inspector's keys are byte-order inconsistent, a few tags appear in it **twice, once per
byte order, with different names** — and only one of the two is a real preimage. `3ce51772` is the
worked example: listed as both `"ZoneTriggerRegionFlags"` (key `3ce51772`) and `"Flags"` (key
`7217e53c`, the byteswap). Only `pandemic_hash("Flags") == 0x3ce51772` holds;
`"ZoneTriggerRegionFlags"` hashes to `0x7c8731c6` and matches neither order, so it is a guess. The
rule when importing dictionary rows: **a row is genuine iff `pandemic_hash(name)` equals the key at
one of its two byte orders**, and a verifying row always beats a non-verifying one on the same tag. Object container tags are *not* parameter names and read as `tag_XXXXXXXX` — they are per-instance
ids, not hashes of a class or a name at all (see "Object nodes" below).

### leaf vs container — RESOLVED, and it was never ambiguous ★

Earlier versions of this doc treated the entry body as a **recursive tree** in which `n` might be a
byte length or a child count, with no header bit to tell them apart, and classified by inspecting the
value. That model was wrong. The engine's reader — **`FUN_00a005a0`**, selected over the text/XML
reader `FUN_009f9c80` by a flag in `FUN_00a035b0` — resolves it **positionally**:

```c
for (each of `object_count` objects) {
    id    = *(u32*)(p);                 // object header
    count = *(u32*)(p + 4);             // <-- CHILD count
    p += 8;
    for (i = 0; i < count; i++) {
        tag = *(u32*)(p);
        n   = *(u32*)(p + 4);
        data = p + 8;
        switch (tag) { … }              // compiled per-class schema, see below
        p = data + n;                   // <-- ALWAYS a byte count. no exceptions.
    }
}
```

So the depth decides, not the value: **root count = objects, object count = params, param count =
bytes.** There is exactly one level of nesting and the format is not recursive. A census of the
embedded pack confirms it exactly — **13 426 containers at depth 0 (every one an object), 138 850
leaves at depth 1, and zero containers at any depth below 0.**

Nothing that looks like nesting is nesting. `DynamicFenceNodes` is the instructive case: a 4-byte
leaf whose value is a *count* (`04 00 00 00`), with the fence nodes themselves stored as **flat
siblings** `FenceNode00…03`, `FenceNodeFlag00…03`, `FenceWallFlag00…03`. That is exactly why the
engine builds those names with `sprintf` and looks them up by hash in the object's flat param list.

`sab_formats::editnodes` now parses positionally (`parse_object` / `parse_param`); the value-based
classifier is gone from the parse path and survives only to *decode* a leaf for display. Both retail
packs still round-trip byte-identically (`sab_probe editnodes info` reports it).

**Why this mattered.** The old classifier shortcut `n == 0 | 1 | 4` to "leaf". An object with
**exactly 4 params** would therefore have been read as a 4-byte leaf, truncating the object and
desyncing everything after it — while the engine read the same bytes correctly. Retail never trips
this only by luck: the minimum param count in the shipped data is **5**. Authored data would hit it
immediately, so this was a live bug in any edit-and-repack workflow, not a theoretical one. Pinned by
the test `object_with_four_params_is_not_mistaken_for_a_leaf`.

### Two more facts from the engine's reader

* **A big-endian variant of this format exists.** Every scalar load in `FUN_00a005a0` has two paths
  selected by a flag (`local_18 & 1`): a direct LE read, or a 4-byte reversal. That is the console
  (PS3/X360) build reading the same layout byte-swapped — and the likely origin of the byte-order
  confusion baked into the community dictionary. PC data is always little-endian.
* **`Position` gets `w = 1.0`, the axis vectors get `w = 0.0`.** Each is a 12-byte leaf that the
  engine expands into a 16-byte vector, tagging `Position` as a point and `XAxis`/`YAxis`/`ZAxis` as
  directions.

The schema switch also independently confirms several dictionary entries, since the tags appear as
literal case constants: `0x5d78ede2` `ClassName`, `0x28e10525` `Type`, `0x42498680` `Script`,
`0x05d7fc60` `Position`, `0xfbb4bda8` `XAxis`, `0xd0c26e7d` `YAxis`, `0x4fb76002` `ZAxis`,
`0x6b9391d0` `DynamicPathNodes`, `0x1acf73a2` `DynamicFenceNodes`, `0xe85f4cfc` `DynamicFenceFlags`
— and `0x3ce51772` **`Flags`**, which is the corrected key from the byte-order repair above.

### Decoding a leaf's value (display only)

Once framing is positional, the value-based tests are no longer load-bearing — they only pick a
presentation for a leaf's bytes:

| test (in order) | → value |
|---|---|
| `n == 0` / `1` / `4` | empty / bool / raw32 |
| `n == 8` and `tag == d788e5b2` | raw64 |
| `data` is printable + trailing NUL | string |
| `name\0` + 4-byte type marker + payload | LuaParam / "named" |
| `n == 12` and 3 plausible floats (or a whitelisted vector tag) | vec3 |
| otherwise | opaque bytes |

LuaParam type markers (little-endian bytes): float `a9 bd 93 2b`, bool `31 4f 44 1b` or `0f 00 00 00`,
CRC list `f9 9d 05 b9`, string `c2 99 b7 f5`.

**The markers are hashed type names**, under the same convention as the tags — read the 4 marker
bytes as a little-endian `u32` and it is `pandemic_hash` of a `Lua<Type>` name:

| marker bytes | as LE u32 | preimage |
|---|---|---|
| `a9 bd 93 2b` | `0x2b93bda9` | `pandemic_hash("LuaFloat")` |
| `31 4f 44 1b` | `0x1b444f31` | `pandemic_hash("LuaBool")` |
| `c2 99 b7 f5` | `0xf5b799c2` | `pandemic_hash("LuaString")` |
| `f9 9d 05 b9` | `0xb9059df9` | `pandemic_hash("LuaCrcList")` (resolved 2026-10-12, from a hint by Katalizator; the exe holds the string `"LUACRCLIST"`, and the hash folds case) |
| `0f 00 00 00` | `0x0000000f` | not a hash — a small enum, alternate bool encoding |

The engine reads these in **`FUN_01634800`** @0x01634800 (and the near-copy `FUN_01634460`), for a
field whose hash is `0xDB0F705C` = `pandemic_hash("LuaParam")`: a name string (≤ 128 bytes), a `u32`
type hash, then by type (PROVEN, decomp lines 1762748–1762811):

| type | `pandemic_hash` of | payload | dropped when |
|---|---|---|---|
| `0x3290ba65` | `LuaCrc` | `u32` hash | it is `0x4ff9f863` = `pandemic_hash("none")` |
| `0x1b444f31` | `LuaBool` | 1 byte | it is 0 |
| `0x2b93bda9` | `LuaFloat` | 4 bytes | — |
| `0xf0676c78` | `LuaInt` | 4 bytes (same read as float) | — |
| `0xf5b799c2` | `LuaString` | string ≤ 128 bytes | it is empty or `"none"` (case-insensitive) |
| `0xb9059df9` | `LuaCrcList` | count, then count strings ≤ 128 bytes, stored as a Lua table indexed 1…count | — |

`0x404d1343` = `pandemic_hash("LuaTable")` is checked as a *field* hash in the same function, not as a
type. `FUN_0064bbc5` switches on the same type hashes when it writes parameters out, with the tag
strings `"LUACRC"` and `"LUACRCLIST"`. Not established: whether the list's count is a `u32` or `i32`
(read by `FUN_0045ef30`), and why bools sometimes carry `0x0000000f`.

An earlier version of this section said none of these hashes appear as literals in the decomp. That
was wrong: all of them are compared as constants in the three functions above.

A mis-decode here is cosmetic: leaf `data` is kept verbatim either way, so it can change how a value
is *shown* but never how the file is framed or rewritten.

---

## Writing: the splice-and-rebalance rules

`sab_formats::editnodes` writes by re-emitting the whole pack from the parsed tree, which is the
clean path. The community inspector instead edits the **bytes in place** and rebalances, and its rule
set is worth recording because it is the one that has actually been used to ship working edits:

1. **Splice** the new leaf `data` over `[node.start+8, node.end)` and write the new byte length to
   the node header's `n` field at `node.start+4`.
2. **Parent child count** — when a node is inserted into or deleted from a container, bump the
   container's `n` at `parent.start+4` (for a top-level entry, the pack's `entry_count` at `+4`).
3. **Directory rebalance** — for `delta = newLen - oldLen`, every directory entry whose body
   *contains* the splice point gets `size += delta`; every entry with `offset >= spliceEnd` gets
   `offset += delta`. Both fields are rewritten in the directory row.
4. **A new top-level entry** additionally inserts a 12-byte directory row, which shifts every body
   by a further +12 — the directory grows into the bodies, so all offsets must be rebased.

Rules 1–4 are **confirmed against the engine**, not merely against a round-trip:

* `FUN_0064b420` (the pack loader) reads `u32 magic == 0x44453030` (`"00ED"`), then `u32 count` into
  the object's entry count, then per entry — in this order — `hash`, `size`, `offset`, into a
  0x20-byte in-memory record.
* `FUN_0064b600` (open entry by hash) does a red-black-tree lookup on `hash`, then **seeks the pack
  stream to `entry.offset`** and installs `entry.size` as the sub-stream length. So the second u32 is
  the size and the third is an offset used directly as a seek — `{hash, size, offset}` and "absolute
  within the pack" are both settled, and nothing else in the file has to agree with them.

The pack is reached through the ordinary resource open (`FUN_009ef620`), which routes the extension
`.wsd` to this class — i.e. **`EditNodes.pack` is a pack of `.wsd` objects keyed by resource-name
hash**. That is also the most likely preimage for the still-unidentified directory `hash`.

### Repacking back into `loosefiles_BinPC.pack` — confirmed from the reader

The loose-file container is read by **`FUN_00dc1060`** (the engine's file-open; the embedded
`EditNodes.pack` is opened through exactly this path). It is not a directory-based archive — it is a
**sequential cursor with a one-entry lookahead**:

```
if (!header_cached) { seek(cursor); read(&hdr, 0x80); header_cached = true; }
if (name_matches(requested, hdr.name)) {
    substream.base = cursor + 0x80;  substream.len = hdr.size;
    cursor += 0x80 + hdr.size;
    if (hdr.size & 0xf) cursor += 0x10 - (hdr.size & 0xf);   // pad to 0x10
    header_cached = false;                                    // advance to the next header
} else {
    open the real file from disk;                             // cursor does NOT advance
}
```

That settles every structural claim: **128-byte header, payload at `+0x80`, `u32 size` at `+4`,
name at `+8`, next entry 0x10-aligned.** Three further facts fall out, two of which matter more than
the checksum:

1. **The leading `u32` is never read.** The header is pulled in as one 0x80-byte blob to
   `DAT_01502878`, and that symbol appears **exactly once in all 36,935 decompiled functions** — as
   the destination of that read. Only `+4` (size) and `+8` (name) are ever consumed.

   That was then confirmed *below* the decompiler, by scanning the whole PE for each field's address
   as a literal — so it does not depend on Ghidra having decompiled everything:

   | address | field | references in `Saboteur.exe` |
   |---|---|---|
   | `0x01502878` | the checksum | **1** — at `0xdc1100`, the destination pointer of `read(&buf, 0x80)` |
   | `0x0150287c` | `size` | 1 — the consumer |
   | `0x01502880` | `name` | 2 — the two comparisons |

   There is no second consumer hiding behind a differently-named symbol or an undecompiled function.
   Leaving the field stale is **safe, CONFIRMED**.

   **★ SOLVED — and it is a NAME hash, not a content checksum:**

   ```
   name_crc = CRC-32/BZIP2( name.to_ascii_lowercase() )
              poly 0x04C11DB7, init 0xFFFFFFFF, xorout 0xFFFFFFFF, NON-reflected (MSB-first)
   ```

   **7 / 7** on every retail entry. Implemented as
   [`sab_formats::loosefiles::name_crc`](../../tools/sab_formats/src/loosefiles.rs), pinned by a test
   carrying all seven `(name, crc)` pairs inline.

   | name | stored `+0x00` |
   |---|---|
   | `France.shaders` | `ee2e6431` |
   | `Cinematics\Conversations\Conversations.cnvpack` | `46350dcf` |
   | `Cinematics\Cinematics.cinpack` | `0f3c8f37` |
   | `global.map` | `6b37a7cb` |
   | `France.map` | `25b96f25` |
   | `France\EditNodes\EditNodes.pack` | `c965b7ea` |
   | `GameTemplates.wsd` | `12cdbde3` |

   Only **case** is normalised — separators are hashed as stored, so `\` and `/` give different
   values even though the *lookup* in `FUN_00dc1060` folds them.

   **How it was found — and why that does not make it a lineage argument.** The candidate came from
   the sibling Mercenaries 2 project, where the save-game `ProfileHash` is CRC-32/BZIP2
   (`mercs2_formats::save`). That mattered only because it is *non-reflected*, and every one of ~60
   earlier candidates here had been reflected — the obvious relative, `crc32_mercs2` (the FFCS `CSUM`
   algorithm), is reflected poly `0xEDB88320` and does not match. The normalisation then gave itself
   away: probing name-hash candidates hit `global.map` and nothing else, and `global.map` is the one
   retail name already entirely lowercase.

   **The proof is entirely Saboteur-side: 7/7 against this game's own retail data**, plus the
   published CRC-32/BZIP2 check value. Mercs 2 supplied a hypothesis, not evidence — per
   [`../AGENTS.md`](../../AGENTS.md)'s prime directive, a Mercs 2 hash is never carried over without
   being re-derived here. The kernel is now recorded in
   [`../lineage_and_divergence.md`](../lineage_and_divergence.md) (the authority on what transfers)
   with the caveat that the *input* differs per game: Saboteur hashes a name, Mercs 2 hashes a save
   body.

   **Consequence for writers — better than "leave it stale":** because it hashes the *name*, editing
   or resizing a payload **never invalidates it**. It only needs computing when an entry is added or
   renamed. Combined with the engine never reading it, there is now no scenario in which this field
   can break an edit.
2. **★ Entry order looks load-bearing — mechanism confirmed, consequence UNTESTED.** A *miss* does
   not advance the cursor, so the archive can only ever be consumed in packed order: it is a
   load-order-optimised stream, not a random-access directory. On a miss the engine falls back to
   opening the file from disk.

   All **7** entries are pack-only — verified against the filesystem, every one, not a sample:

   ```
   France.shaders                                  23 066 269
   Cinematics\Conversations\Conversations.cnvpack     509 273
   Cinematics\Cinematics.cinpack                   11 821 963
   global.map                                         284 388
   France.map                                       2 817 849
   France\EditNodes\EditNodes.pack                  2 606 468
   GameTemplates.wsd                                8 204 431
   ```

   There is exactly one loosefiles pack in the install; DLC slots ship none. The walk tiles the file
   exactly and no entry uses the `0xFFFFFFFF` sentinel.

   **What is NOT established:** that a miss is fatal. Nobody has traced what the caller does when the
   disk open fails, and **the game has never been run with a modified `loosefiles`**. The inference
   "reorder ⇒ unloadable asset" is a reading of the code plus a filesystem check, not an experiment.
   What *does* corroborate the cursor model is that retail works at all: with 7 pack-only entries and
   a strictly sequential cursor, the engine's request order must already match the packed order.

   Treat the edit classes differently, then:

   | edit | status |
   |---|---|
   | **resize an entry in place** | safe — every position is derived from the sizes at read time; no stored offsets to fix. This is the only loosefiles-level edit anyone has actually shipped. |
   | **append after the last entry** | expected harmless — worst case the new entry is never matched |
   | **insert in the middle / reorder** | expected to break every entry from that point on — **untested**; run the experiment before relying on either outcome |

   The experiment is cheap and decisive: back up `loosefiles_BinPC.pack`, swap two entries, launch.
   Until someone does that, this row is a prediction.
3. **`size == 0xFFFFFFFF` is a "load from disk" sentinel** — the cursor steps over the 128-byte
   header and the engine opens the real file instead.

Name matching is case-insensitive and separator-insensitive (`\` and `/` are folded together), with a
fallback that retries after stripping the requested path's first component.

### Orientation is three basis vectors, not a quaternion

Placeable objects carry `XAxis` / `YAxis` / `ZAxis`, each a 12-byte 3×f32 leaf, forming a rotation
basis alongside `Position`. A yaw of `θ` about the up axis is:

```
XAxis = ( cos θ, 0, -sin θ)
YAxis = ( 0,     1,  0    )
ZAxis = ( sin θ, 0,  cos θ)
```

i.e. **Y is up** and yaw turns in the XZ plane. This is the convention the inspector's yaw helper
writes, and it is how "rotate this spawned car" is expressed in the data.

---

## DLC override: a slot pack shadows the world per-entry, it does not replace it ★

The engine keeps an **array of EditNodes pack objects**, not one:

```
this + 0xE0A0   array of pack objects, stride 0x68
this + 0xE170   u8 count

slot 0  base world  France\EditNodes\EditNodes.pack          FUN_009f4320  -> sets count = 1
slot 1  DLC         <dlcroot>\France\EditNodes\EditNodes.pack FUN_009906c0 -> FUN_009eea90(path, 1),
                                                                              count += 1  -> 2
```

Lookup (`FUN_009ef620`) walks it **top-down and returns the first hit**:

```asm
mov   al,[edi+0xe170]        ; count
movzx esi,al
sub   esi,1                  ; i = count-1
js    bail
loop: mov ecx,esi
      imul ecx,ecx,0x68      ; stride
      lea  edi,[ecx+edi+0xe0a0]
      call FUN_0064b600      ; find entry by hash in slot i
      jne  found             ; first hit wins; else i-- and retry
```

Three consequences, all read off the mount sites (`0x009f4c08`, `0x009eeab1`) and the lookup
(`0x009ef808`):

1. **A DLC pack does not wipe the world.** `FUN_0064b420` destroys the entry array of the object it
   loads into — but slot 1 is a *different* object (`base + 1*0x68`), so the 767 base entries survive.
   The worry that shipping a 3-entry DLC pack would collapse the world to 3 entries is unfounded.
2. **Later slot wins, per directory hash.** A DLC entry shadows the base entry of the same hash; any
   hash the DLC pack lacks falls straight through to the base. This is a genuine **additive override**
   — and it is the *opposite* of the GameText/DNEC situation, where a DLC slot cannot override an
   existing hash and edits must patch base.
3. **Exactly two slots.** The index is computed `setne bl` → clamped to 0/1, so at most one DLC
   EditNodes pack is ever mounted. That composes with the engine mounting exactly one DLC slot of
   01–04 anyway.

**★ The one constraint that shapes an editor: override granularity is the ENTRY, not the object.**
A shadowed entry is replaced wholesale, so changing one NPC means shipping that entry's *entire*
object list. A publish step is therefore: copy the base entry verbatim → apply the edits → emit a DLC
pack containing only the touched entries. Untouched entries must be left out, not copied, so they
keep falling through to base.

---

## Validation

`sab_formats::editnodes` over both retail packs (`sab_probe editnodes` / the module tests):

* **Exact byte consumption** — every entry's body consumes exactly its directory `size`; the entries
  tile the file from directory-end to EOF. **770 / 770 entries** across the 767-entry embedded pack
  and the 3-entry DLC pack, 0 desyncs. Under positional framing this is now structural rather than
  lucky: a param's `byte_len` is taken at face value, so the only way to desync is a genuinely
  malformed file.
* **Round-trip byte-identical** — re-emitting directory + bodies reproduces both packs exactly.
  `sab_probe editnodes info` prints the result, so it is checked on every run rather than asserted
  here:

  ```
  EditNodes.pack: 767 entries, 2606468 bytes
  round-trip: byte-identical (2606468 bytes)
  nodes=152276 (containers=13426, leaves=138850; LuaParam=4548)
  ```

* **Depth census** — 13 426 containers at depth 0 (all objects), 138 850 leaves at depth 1, **0
  containers below depth 0**. This is the measurement that proves the body is one level deep.
* **Tag dictionary** — 250 / 264 names reproduce their tag as `pandemic_hash(name)` little-endian;
  113 pack tags are reproduced from the exe's own `sprintf` name templates.

---

## Confidence

| claim | status |
|-------|--------|
| pack = `"00ED"`, u32 count, `{hash,size,offset}` directory, bodies contiguous to EOF | **CONFIRMED** (exact-consume + byte-identical round-trip, both packs; field roles read off `FUN_0064b420` + `FUN_0064b600`) |
| `offset` is an absolute seek within the pack; `size` is the sub-stream length | **CONFIRMED** (`FUN_0064b600` seeks to `entry.offset`, installs `entry.size` as the length) |
| entries are `.wsd` objects; the pack is the `.wsd` provider | **CONFIRMED** (`FUN_009ef620` routes the `.wsd` extension to this class) |
| packs are an ARRAY (base + DLC, stride 0x68 @ `+0xE0A0`, count @ `+0xE170`, max 2) | **CONFIRMED** — disassembly of the two mount sites and the lookup |
| a DLC pack shadows base entries per-hash and does NOT replace the world | **CONFIRMED** — lookup walks `count-1 → 0`, first hit wins; slots are distinct objects |
| override granularity is the whole ENTRY, not the object | **CONFIRMED** — the shadowing happens at the directory-hash lookup, so a shadowed entry is replaced wholesale |
| loose-file container: 0x80 header, `size` at +4, name at +8, payload at +0x80, next entry 0x10-aligned | **CONFIRMED** (`FUN_00dc1060`) |
| the loose-file header's leading `u32` is never read by the engine | **CONFIRMED** — one reference in the whole PE (byte-level scan for `0x01502878`), the destination of the header read |
| that field is `CRC-32/BZIP2(lowercased name)` — a NAME hash, not a content checksum | **CONFIRMED** — 7/7 retail entries; `sab_formats::loosefiles::name_crc`. So a payload edit can never invalidate it |
| loose-file reader is a sequential cursor; a miss does not advance it | **CONFIRMED** (`FUN_00dc1060`) |
| all 7 loose-file entries are pack-only (no on-disk fallback copy) | **CONFIRMED** (filesystem, all 7) |
| therefore reordering/inserting makes those assets unloadable | **INFERRED, UNTESTED** — the failure path of the disk fallback was never traced and the game has never been run with a modified `loosefiles`. Resize-in-place is unaffected either way. |
| `size == 0xFFFFFFFF` means "open the real file from disk" | **CONFIRMED** (`FUN_00dc1060`) |
| LuaParam markers are `pandemic_hash("LuaFloat"/"LuaBool"/"LuaString"/"LuaCrcList")` | **CONFIRMED** (4/4 preimages reproduce exactly; reader `FUN_01634800`) |
| entry root = EditNode `{u32 ver=0, u32 count, Nodes}` | **CONFIRMED** (767/767 embedded + 3/3 DLC roots are version 0) |
| node = `{u32 tag, u32 n, data}`; `n` = byte-len (leaf) or child-count (container) | **CONFIRMED (framing)** — exact-consume proves the split is resolved correctly on all retail nodes |
| `tag == pandemic_hash(paramName)` little-endian, for PARAMETER nodes | **CONFIRMED** (250/264 dictionary names hash-verify; the `NodeNN`/`FenceNN` families reproduce 113 pack tags from the exe's own `sprintf` templates) |
| ~~an OBJECT node's tag is a per-instance unique id, NOT a name hash~~ | **RETRACTED 2026-07-29** — it is `pandemic_hash` of the object's full path; see the row above. The observations were right, the conclusion was wrong: hashing only strings *stored in the file* could never find it |
| `Type` is the broad engine class; `ClassName` is the TEMPLATE/archetype | **CONFIRMED** (retail data: `Type='Spore'` / `ClassName='Spore_RS_Jules'`) |
| ~~`ClassName` is the object's instance name~~ | **RETRACTED 2026-07-29** — a corollary of the retracted opaque-id model. `ClassName` equals the addressable name for only ~6.9% of objects; the instance name is not stored at all (row below) |
| cloning an object with its tag intact is legal but leaves the copy un-addressable | **CONFIRMED** — retail has 13 such collisions; both objects answer to one `GetHandleByName` name. Retag via `object_tag` to make a clone addressable |
| framing is positional: root count = objects, object count = params, param count = BYTES | **CONFIRMED** (`FUN_00a005a0` advances `p = data + n` unconditionally; depth census finds 0 containers below depth 0 in 152 276 nodes) |
| the entry body is NOT a recursive tree | **CONFIRMED** — exactly one level of nesting |
| a big-endian (console) variant of the layout exists | **CONFIRMED** — every scalar load in `FUN_00a005a0` has a byte-reversing twin behind one flag |
| `Position` expands to `w=1.0`, `XAxis`/`YAxis`/`ZAxis` to `w=0.0` | **CONFIRMED** (`FUN_00a005a0`) |
| leaf VALUE decoding (string / vec3 / LuaParam / raw) | **HEURISTIC** — value-based, but display-only now that framing is positional; cannot corrupt a rewrite |
| directory `hash` = `pandemic_hash(r"France\EditNodes\" + path + ".wsd")` | **CONFIRMED** — 97/106 `.wsd` literals in `LuaScripts.luap` reproduce a retail directory hash |
| object tag = `pandemic_hash(r"<container path w/o .wsd>\<editor name>")` | **CONFIRMED** — 1 632 Lua literals reproduce an object tag, 0 collide with a directory hash, and 1 620/1 620 resolve into exactly the container their path predicts |
| the object's editor name is NOT stored in the pack | **CONFIRMED** — only ~6.7% equal `ClassName`; no other string param ever supplies it |
| `XAxis`/`YAxis`/`ZAxis` are a rotation basis, Y-up, yaw in XZ | **STRONG** — the inspector's yaw helper writes exactly this and the results place correctly in-game |
| the per-tag schema (which tag means what, and its type) | **PARTIAL** — `FUN_00a005a0`'s switch names ~20 tags for the streaming-object class; other `.wsd` classes have their own switches, not yet read |
| what each object does at runtime (Lua enable/spawn semantics) | **OUT OF SCOPE HERE** — this doc is the container; see the mission Lua |
