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
`u32 crc`, `u32 size`, `char name[120]` header, the payload, then 0x10 alignment). The DLC packs are
loose files you can open directly.

Parser/writer: [`sab_formats::editnodes`](../../tools/sab_formats/src/editnodes.rs) (shared library).
Inspect with [`sab_probe editnodes <info|list|tree>`](../../tools/sab_probe/README.md). All integers
are little-endian.

---

## Byte layout

```
Pack
  char  magic[4] = "00ED"
  u32   entry_count
  entry_count × { u32 hash; u32 size; u32 offset }   -- directory; offset is ABSOLUTE
  ... entry bodies, contiguous, in directory order, from directory-end to EOF

Entry body (a node tree). The root is one of:
  EditNode   (retail is 100% this):  u32 version = 0, u32 object_count, object_count × Node
  Container  (defensive; not seen in retail):  u32 tag(≠0), u32 child_count, child_count × <root>

Node
  u32   tag            -- pandemic_hash(paramName), little-endian (see below)
  u32   n
  data
    - LEAF      : `n` is a BYTE count; `data` is `n` bytes
    - CONTAINER : `n` is a CHILD count; `n` Node children follow
```

The pack directory is contiguous: the first entry begins exactly at `8 + entry_count*12`, entries are
laid out in directory order (== ascending offset), and the last ends at EOF. A byte-faithful writer
recomputes the whole directory (offsets/sizes) and re-emits the bodies in order.

### The `tag` — `pandemic_hash(paramName)`, little-endian  ★

Each node's 4-byte `tag` is the **[`pandemic_hash`](gametext.md#pandemic_hash) of the parameter's
name, stored little-endian** — the exact convention GameText uses for a UI `asset_id`. So reading the
tag as an LE `u32` yields the hash directly, and `pandemic_hash("Position") == 0x05d7fc60` is the tag
you will find on a Position node.

The community inspector shipped an empirical `tag → name` dictionary; **242 of its 265 names
hash-verify** against their tag under this convention (`tools/sab_formats/src/editnodes.rs`'s
`TAG_NAMES`, checked by the test `tag_names_sorted_and_mostly_preimages`). The ~23 that do not are
positional/placeholder labels (`Node00…Node40`, `FenceNodeFlagNN`, `Unknown_*`, `Root`); their real
names were never recovered, and their byte order was resolved against the tags actually present in the
retail pack. Object-**class** container tags (the hash wrapping each object, e.g. a `Locator` or
`TeleporterDoorPointI2I` class) are *not* in the parameter dictionary and read as `tag_XXXXXXXX`.

### The one ambiguity: leaf vs container

Nothing in a node header distinguishes a leaf (`n` = byte length) from a container (`n` = child
count). The engine knows each tag's type from its class schema; the tools do not, so they **classify
by value**, replicating the inspector's `parseProperty` order:

| test (in order) | → kind |
|---|---|
| `n == 0` / `1` / `4` | empty / bool / raw32 (leaf) |
| `n == 8` and `tag == d788e5b2` | raw64 (leaf) |
| `data` is printable + trailing NUL | string (leaf) |
| `name\0` + 4-byte type marker + payload | LuaParam / "named" (leaf) |
| `n == 12` and 3 plausible floats (or a whitelisted vector tag) | vec3 (leaf) |
| otherwise | container of `n` children |

LuaParam type markers (little-endian bytes): float `a9 bd 93 2b`, bool `31 4f 44 1b` or `0f 00 00 00`,
string-list `f9 9d 05 b9`, string `c2 99 b7 f5`.

This heuristic is not guaranteed by the format, but it is **empirically exact on the retail data**: it
consumes every entry of both shipped packs to its declared size with zero desyncs (see Validation).
Because leaf `data` is kept verbatim, a mis-classification could only ever be detected as a
size/round-trip failure — and none occurs.

---

## Validation

`sab_formats::editnodes` over both retail packs (`sab_probe editnodes` / the module tests):

* **Exact byte consumption** — every entry's node tree consumes exactly its directory `size`; the
  entries tile the file from directory-end to EOF. **770 / 770 entries** across the 767-entry embedded
  pack and the 3-entry DLC pack, 0 desyncs.
* **Round-trip byte-identical** — re-emitting directory + bodies reproduces both packs exactly.
* **Tag dictionary** — 242 / 265 names reproduce their tag as `pandemic_hash(name)` little-endian.

---

## Confidence

| claim | status |
|-------|--------|
| pack = `"00ED"`, u32 count, `{hash,size,offset}` directory, bodies contiguous to EOF | **CONFIRMED** (exact-consume + byte-identical round-trip, both packs) |
| entry root = EditNode `{u32 ver=0, u32 count, Nodes}` | **CONFIRMED** (767/767 embedded + 3/3 DLC roots are version 0) |
| node = `{u32 tag, u32 n, data}`; `n` = byte-len (leaf) or child-count (container) | **CONFIRMED (framing)** — exact-consume proves the split is resolved correctly on all retail nodes |
| `tag == pandemic_hash(paramName)` little-endian | **CONFIRMED** (242/265 dictionary names hash-verify) |
| leaf/container + leaf-kind classification is schema-exact | **HEURISTIC** — empirically exact on retail data, but value-based, not guaranteed by the container itself |
| directory `hash` key's preimage | **UNKNOWN** — kept verbatim; not yet pinned to a name |
| what each object does at runtime (Lua enable/spawn semantics) | **OUT OF SCOPE HERE** — this doc is the container; see the mission Lua |
