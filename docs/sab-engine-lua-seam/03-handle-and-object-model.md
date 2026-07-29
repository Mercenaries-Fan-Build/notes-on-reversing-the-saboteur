# The handle and object model

## What this establishes

A Lua-side handle in The Saboteur is **not** a pointer, an index, or a name hash. It is a **light userdata
carrying a 32-bit salted object ID**: 24 bits of slot index plus an 8-bit generation byte, minted by a
global ID allocator and torn down (with the generation bumped) when the object dies. The `FUN_004436f0`
call in the anchor is an ordered-map lookup — a red-black tree keyed by that ID — and the critical section
around it is not about Lua at all: the same map is read by engine threads that never touch a `lua_State`.

Consequence, stated up front because it is the question that matters for tooling: **handles are
session-scoped and are not stable across save/load, or even across a streaming reload within one session.**
The script corpus knows this and works around it by passing *names* and re-resolving them.

This doc covers the handle value, the ID allocator, the guarded map, the stale-handle path, and the
threading rationale. The marshalling primitives themselves (`FUN_006f8470`, `FUN_006f71a0`, `FUN_006f6ec0`)
and the registration-table anomaly around `0x00716a85` are the scope of the sibling docs in this directory
and are only used here, not re-derived.

---

## 1. What a handle is, numerically

The type check the anchor performs is a Lua type-tag comparison against **2**:

```c
// FUN_006f71a0 @0x006f71a0  — "is arg n a handle?"
bool FUN_006f71a0(undefined4 *param_1, undefined4 param_2)
{ return thunk_FUN_00466c30(*param_1, param_2) == 2; }
```

`FUN_00466c30 @0x00466c30` is `lua_type` (it returns `-1` for the "none" sentinel `&DAT_010a28ac`,
otherwise `TValue.tt`). Tag 2 is `LUA_TLIGHTUSERDATA` in the 5.0/5.1 tag order. The sibling checks in the
same family corroborate the ordering exactly: `FUN_006f7100` tests `== 0` (nil), `FUN_006f7120` tests
`== 1` (boolean), `FUN_006f71c0` tests `== 5` (table). The fetch is `lua_touserdata`:

```c
// FUN_004cc330 @0x004cc330 — lua_touserdata
if (piVar1[1] == 2) return *piVar1;        // light userdata: the raw stored word
if (piVar1[1] != 7) return 0;              // 7 = LUA_TUSERDATA
return *piVar1 + 0x18;                     // full userdata: skip the 24-byte Udata header
```

`FUN_006f6ec0 @0x006f6ec0` is the "fetch arg n if it is a handle" wrapper over the two. So the value the
binding takes off the stack is **one 32-bit word stored inline in the Lua TValue**, never dereferenced by
the binding — it is spilled to a stack local and the *address* of that local is handed to the map lookup
(`iVar2 = FUN_004436f0(&uStack_10)` in `FUN_00714230 @0x00714230`), because the map's find takes `uint*`.

The Lua corpus agrees from the other side. `docs/saboteur-luacd/src/Includes/WRAPPER_Util.lua:4` branches
on `type(a_vVariable) == "userdata"` to decide a value is already a handle, and on `"string"` to convert
one via `Util.GetHandleByName`. Light userdata reports as `"userdata"` to `type()` and compares by value
under `==`, which is why the corpus can freely test `if hObject == nil` (`WRAPPER_Util.lua:8`) and compare
handles for identity without a metatable ever being involved.

**Confidence: confirmed.** (Tag arithmetic and `lua_touserdata` shape are byte-level; the corpus `type()`
test is independent corroboration.)

## 2. The 32-bit value is a salted object ID (index + generation)

The same bit-slicing idiom appears at ~30 sites across the binary and is unmistakable:

```c
uVar2 = id & 0xffffff;                                        // 24-bit slot index
if ((uVar2 <= DAT_01321e98) &&                                // in range?
    ((char)(id >> 0x18) == *(char *)(DAT_01321e9c + uVar2)))  // generation byte still matches?
```

The allocator is `FUN_0044aa57 @0x0044aa57`. It pops a slot index off a free ring, reads that slot's
current generation byte, and packs the two:

```c
EnterCriticalSection(&DAT_01321e80);
...
uVar2 = *(uint *)(DAT_01321ea0 + DAT_01321ea8 * 4);   // pop index from the free ring
bVar1 = *(byte *)(DAT_01321e9c + uVar2);              // current generation for that slot
LeaveCriticalSection(&DAT_01321e80);
*param_1 = uVar2 | (uint)bVar1 << 0x18;               // id = index | generation<<24
```

The release path (`FUN_0068bad0 @0x0068bad0`, and an inlined twin inside `FUN_0062dbc0` at ~`0x0062e440`)
does the inverse and, critically, **increments the generation byte first**:

```c
uVar4 = id & 0xffffff;
pcVar1 = (char *)(DAT_01321e9c + uVar4);
*pcVar1 = *pcVar1 + '\x01';                           // every outstanding id for this slot is now stale
EnterCriticalSection(&DAT_01321e68);
*(uint *)(DAT_01321ea0 + DAT_01321ea4 * 4) = uVar4;   // push index back on the ring
...
LeaveCriticalSection(&DAT_01321e68);
*(undefined4 *)(iVar2 + 4 + (int)param_1) = 0;        // clear the object's own id slot
```

| Global | Role |
| --- | --- |
| `DAT_01321e98` | highest valid slot index (range check bound) |
| `DAT_01321e9c` | base of the **generation byte array**, one `char` per slot |
| `DAT_01321ea0` | base of the free-index ring (`u32[]`) |
| `DAT_01321ea4` | free ring **push** cursor (release side) |
| `DAT_01321ea8` | free ring **pop** cursor (allocate side) |
| `_DAT_01321eac` | live free count |
| `&DAT_01321e80` / `&DAT_01321e68` | separate critical sections for the pop and push sides |

Both cursors decrement and wrap to `DAT_01321e98`, i.e. it is a circular queue: freed indices go to the
back, so reuse is deferred as long as possible and the 8-bit generation has time to move on. With 24 bits
of index the pool tops out at 16,777,215 slots; the generation wraps at 256 reuses of the *same* slot, at
which point a sufficiently ancient stale handle can alias a new object. That is a real (if remote) ABA hole
in the design, not an artifact of the decompile.

**Confidence: confirmed** for the packing, the allocator, and the generation bump. **Inferred** that the
handle Lua receives is precisely this ID rather than a parallel ID space — see §3, where the map's insert
takes its key from the object's own ID field, and §7 for what is still open.

The 8-byte "weak pointer" that wraps an ID is visible in the clear at `FUN_0044ad10 @0x0044ad10`:
`{ u32 id @+0, void *cached @+4 }`, validated with the idiom above and **zeroing the cache on failure**.
The same struct is embedded at `+0x24` of a larger object (`FUN_0083a200`, `FUN_0083a740 @0x0083a740`) and
at `+0xc` of another (`FUN_00708d00 @0x00708d00`). This is the engine's generic weak reference, and the Lua
handle is its serialized half.

## 3. The guarded table: an ordered map, not a hash table

`FUN_004436f0 @0x004436f0` is a **red-black / binary search tree find**, not a hash lookup — it descends
comparing `*param_2` against a per-node key. Ghidra types the `this` pointer as `LPCRITICAL_SECTION`
because the map object *begins* with a 24-byte `CRITICAL_SECTION`; decoded, the container is:

| Offset | Field | Evidence |
| --- | --- | --- |
| `+0x00` | `CRITICAL_SECTION` (0x18 bytes) | `EnterCriticalSection(param_1)` at entry of find/insert/erase |
| `+0x18` | root node pointer | `param_1[1].DebugInfo` in `FUN_004436f0`, assigned on first insert in `FUN_00497f10` |
| `+0x1c` | element count | `param_1[1].LockCount = param_1[1].LockCount + 1` on successful insert |
| `+0x20` | **default value** returned on a miss | `param_1[1].RecursionCount` |

Nodes are 20 bytes (`FUN_00490ab0(0x14)` in the insert) and live in a **shared global node pool**, addressed
by 16-bit indices rather than pointers: `node = idx * DAT_0150282c + DAT_0150283c` (element size / pool
base), with `0xffff` as the null sentinel. The inverse encode appears at e.g. `0x0043xxxx`
(`*(short *)(param_1 + 0xc) = (short)((uint)(param_2 - DAT_0150283c) / DAT_0150282c)`). Node layout:
`{ u32 key @+0, u32 value @+4, u16 left @+8, u16 right @+0xa, ... , flags @+0x10 }` — the `|= 2` on the
`+0x10` word during insert followed by `FUN_00dbe640(param_1 + 1)` is a red/black colour bit plus rebalance.
Because child links are `u16`, **the pool cannot exceed 65,534 nodes** engine-wide.

The insert is `FUN_00497f10 @0x00497f10`, and its rejection codes are the clearest statement of the key's
contract anywhere in the binary:

```c
undefined4 FUN_00497f10(map *this, uint *key, int *value)
  if (*key == 0)                          return 1;   // 0 is not a legal handle
  if (*key == DAT_012119cc)               return 2;   // one reserved key value
  if (*value == this[1].RecursionCount)   return 3;   // refuses to store the default (i.e. null)
  ... descend, on exact match             return 4;   // duplicate key
  if (node alloc failed)                  return 5;
  ...                                     return 0;
```

Erase is `FUN_00498300 @0x00498300`, which finds by key, unlinks, and frees the *value* under
`&DAT_0132bbc4`. Note its offsets: the map it operates on is embedded at `owner + 0x1020` with the default
at `owner + 0x1040` — exactly `+0x20` past the map base, matching the `DAT_0143db28[1].RecursionCount`
layout. Its sibling insert site (`FUN_00495720`, around `0x004956c5`/`0x004956fc`) passes as the key
`*(uint **)(*(int *)(piVar2[1] + 4) + 4 + (int)piVar2)` — a field reached through the object's virtual-base
table, and *the same expression* that `FUN_0068bad0` reads, generation-checks, and hands back to the ID
allocator. **The map key is the object's own salted ID.**

Every caller of `FUN_004436f0` reproduces the identical prologue, including the miss handling:

```c
EnterCriticalSection(DAT_0143db28);
iVar2 = FUN_004436f0(&uStack_10);
piVar3 = (iVar2 == 0) ? (int *)lpCriticalSection[1].RecursionCount   // the map's default value
                      : *(int **)(iVar2 + 4);                        // the stored object pointer
LeaveCriticalSection(lpCriticalSection);
```

`FUN_004439d0 @0x004439d0` and `FUN_00498447 @0x00498447` (reached via the thunk `FUN_00498440`) are simply
this idiom given a symbol; `FUN_00714090 @0x00714090` calls `FUN_00498440(handle)` where `FUN_00714230`
inlines it. They are the same operation.

**Confidence: confirmed** for the container shape, the node pool encoding, and the insert/erase contract.
**Inferred (strong)** that `DAT_0143db28` is this same map type owned by an object manager at
`manager + 0x1020` — the `+0x20` default offset matches to the byte, and the manager also carries a live
object array at `+0x1c` with its count at `+0x101c` (`FUN_00495720`). The literal write to `DAT_0143db28`
was not located; see Open questions.

There is a **second** map of identical shape at `DAT_01321e38`, read by `FUN_0067c0a0 @0x0067c0a0` with the
same handle value as key. Its address sits inside the ID-allocator's own global block
(`0x01321e38` … `0x01321eac`), which is why it reads as the allocator's canonical `id -> object` directory
while `DAT_0143db28` is the script-facing one. Several bindings resolve through `FUN_0067c0a0` instead
(`FUN_007090f0 @0x007090f0`, the `SaveLoad.cpp` bindings at `0x00742130`), and that path additionally
rejects objects whose `+0x18` flag byte lacks bit 3 — a liveness/visibility gate the raw map lookup does not
apply. **Confidence: inferred.**

## 4. `vtable+0x1c` and `FUN_0083a200`

After the lookup, every one of these bindings does the same two-step:

```c
if (((piVar3 != (int *)0x0) && (iVar2 = (**(code **)(*piVar3 + 0x1c))(), iVar2 != 0)) &&
    (iVar2 = FUN_0083a200(), iVar2 != 0)) { ... }
```

`FUN_0083a200 @0x0083a200` is fully decoded and is **the weak-reference resolver**:

```c
undefined4 FUN_0083a200(int param_1)
{
  iVar1 = *(int *)(param_1 + 0x28);                    // cached target pointer
  if (iVar1 != 0) {
    uVar2 = *(uint *)(param_1 + 0x24) & 0xffffff;      // the id sitting next to it
    if ((uVar2 <= DAT_01321e98) &&
        ((char)(*(uint *)(param_1 + 0x24) >> 0x18) == *(char *)(DAT_01321e9c + uVar2))) {
      return (**(code **)(... iVar1 ... + 0x174))();   // live: virtual accessor on the target
    }
    *(undefined4 *)(param_1 + 0x28) = 0;               // dead: poison the cache, never again
  }
  return 0;
}
```

So `FUN_0083a200` takes an object that **embeds the 8-byte weak reference at `+0x24`**, re-validates the
generation, and returns the live target through a virtual accessor at slot `+0x174` (or `0` if the referent
has been destroyed since the reference was taken). It is the same code as `FUN_0044ad10` and the inlined
copy in `FUN_0083a740 @0x0083a740`, differing only in the struct offset. **Confidence: confirmed** for the
body; **inferred** that its `this` in the anchor is the value just returned by `vtable+0x1c` (register-passed
`ECX`, which Ghidra elided).

`vtable+0x1c` (slot 7) is therefore an accessor returning **the weak-reference-holding wrapper for the
engine object**, i.e. the map does not store the game object directly — it stores a script proxy, and slot 7
is the proxy's "get the thing I point at" method. Supporting evidence beyond the anchor:
`FUN_00714090 @0x00714090` uses the same slot's return value and immediately reads `+0x140` from it;
`FUN_00493320 @0x00493320` uses it and then calls `vtable+0x78` on the result for a boolean. A reasonable
name is `GetTargetRef()` / `GetObjectRef()`. **Confidence: inferred, and the naming is a proposal.** No
assertion string names this slot; I did not read a vtable to confirm it, and it should not be treated as
settled.

The dual-check structure has a real meaning worth stating plainly: **there are two independent liveness
gates.** The map answers "does this ID still name a registered proxy?" and the weak reference answers "is
the object that proxy points at still alive?" A handle can pass the first and fail the second.

## 5. What happens on a stale or dead handle

Nothing. That is the finding.

The `piVar3 == 0` path — miss in the map, so `piVar3` takes the map's default (`+0x20`, null for this
instance, which `FUN_00497f10` return code 3 shows can never be inserted as a real value) — falls straight
out of the `if`, and the binding returns to Lua **having pushed no results and raised no error**. Same for
`vtable+0x1c` returning 0, and same for `FUN_0083a200` returning 0. There is no `lua_error`, no assertion,
no log line on any of the three failure edges in `FUN_00714230`; the EALA assertion string in that function
is on the *success* path, describing the call being made, not a check.

From Lua, a stale handle is therefore indistinguishable from a no-op. `Actor.Ragdoll(hDeadGuy)` on a
freed actor silently does nothing. This explains the corpus's defensive style: `WRAPPER_SanityCheck`
(`WRAPPER_Util.lua:35`) exists precisely because the engine will not tell you, and it can only check for
`nil` — it cannot detect a *stale* handle, because a stale handle is still a perfectly good light userdata.
`Util.IsHandleValid` / `IsObjectHandleValid` (`data/lua_bindings.txt:386,396`) are the only way for a script
to distinguish the two, and they are the bindings that would expose the §4 double gate.

**Confidence: confirmed** (three control-flow edges in `FUN_00714230`, `FUN_00714170`, `FUN_00714320`, all
identical).

## 6. Why the critical section

Not because Lua is re-entered from many threads with a shared `lua_State` — but because **the handle map is
engine-wide state and the engine is threaded.** The evidence is on the *non*-binding side of the map:
`FUN_00493320 @0x00493320` (1,179 bytes of actor state copying), `FUN_00504f10 @0x00504f10`, and
`FUN_00690440 @0x00690440` all take `DAT_0143db28` and do lookups with no `lua_State` anywhere in sight.
The lock protects the tree from concurrent mutation by whichever engine thread is streaming, spawning, or
destroying objects while a binding is walking it. Note the lock is released *before* the object is used —
the map is guarded, the object is not.

That said, the script manager itself is thread-indexed. `FUN_006f6f10 @0x006f6f10` reads a thread slot out
of TLS and uses it to subscript per-thread arrays inside the manager:

```c
pvVar3 = TlsGetValue(DAT_0148e2c0);
uVar2 = *(uint *)(param_1 + 0x218 + (int)pvVar3 * 4);
puVar1 = (undefined4 *)(*(int *)(param_1 + 0x204 + ((int)pvVar3 + -1) * 4) + uVar2 * 8);
```

`DAT_0148e2c0` is a global TLS index used all over the engine as a small thread ordinal. So the script
manager is at minimum *reachable* from more than one thread and keeps per-thread argument scratch. Against
that, the frame-setup primitive `FUN_006f8470 @0x006f8470` searches an array at `manager + 0x124` whose
loop bound is **1** — a single `lua_State` context slot in the retail build:

```c
puVar2 = (undefined4 *)(param_1 + 0x124);
do { if (*(int *)*puVar2 == param_2) return *(undefined4 *)(param_1 + 0x124 + iVar1 * 4);
     iVar1++; puVar2++; } while (iVar1 < 1);
```

So: one VM, one script context, marshalling scratch that is nonetheless per-thread, and a map that is
shared with the rest of the engine. **The critical section is about the map's other users, not about Lua.**
**Confidence: inferred** (strong for the "other users" claim, which is direct; weaker on the exact thread
topology, which needs a live debugger and is listed as open).

## 7. Save/load, and the relationship to `pandemic_hash`

Handles cannot survive a save. The ID's index half comes from a runtime free ring whose order depends on
allocation history, and the generation half is bumped on every destruction — nothing about either is
derived from content. Persisting a handle would persist a number that, next session, names a different
object or nothing at all. Two independent corroborations from the corpus:

- The `Script\Interface\SaveLoad.cpp` bindings take **strings**, not handles, for the things they must
  re-find later: `FUN_00741c10 @0x00741c10` (`"SaveLoadSetupSpecialLuaTimerCallback"`, line `0xfc`) reads
  arg 2 with `FUN_006f7a80` (the string accessor, gated by `FUN_006f7160` = `lua_isstring`).
- `WRAPPER_CheckForHandle` (`WRAPPER_Util.lua:1`) exists so that **every** wrapper entry point accepts a
  *name* and re-resolves it through `Util.GetHandleByName` at call time. Mission scripts store the name;
  they resolve to a handle only for the duration of a call. `__UtilFunctions.lua:286` and
  `WRAPPER_Actor.lua:137` (`Util.GetHandleByName("Saboteur")` — resolved fresh, every time, for the player
  who unquestionably exists) are the idiom in the wild.

The round trip closes: `Util.GetNameFromHandle` (`data/lua_bindings.txt:275`, used at e.g.
`docs/saboteur-luacd/src/Experimental/Checkpoint.lua:99`) recovers a name from a handle, so the registry
retains the name→ID association in both directions.

**On `pandemic_hash`:** the handle **is not a name hash** — that stands, and §1 is why: the ID is
light userdata built from a free-ring index plus a generation counter, with nothing content-derived in
either half.

> **Correction (2026-07-29).** This section previously added that "the name→handle path does not appear
> to run through `FUN_00dc1e20`". That second claim is **retracted**. The *lookup key* is
> `pandemic_hash`: EditNodes objects are keyed by `pandemic_hash` of their full editor path, and
> **1 632** path literals in `LuaScripts.luap` reproduce a retail object tag exactly — 0 of them
> colliding with the container namespace, and 1 620/1 620 resolving into precisely the container their
> path predicts. Those are the same strings mission Lua hands to `Util.GetHandleByName`, so
> `GetHandleByName(r"Missions\...\main\Giselle")` hashes the string it is given and the object's tag
> *is* its name. See [editnodes.md](../formats/editnodes.md#object-nodes-the-tag-is-pandemic_hash-of-a-path-).

The old argument was that `FUN_00dc1e20` has only ten callers, none in the `0x0070f000`–`0x0076xxxx`
binding block and none touching `DAT_0143db28`. That is a *static call-graph* argument and it
under-counts: [16](16-family-world-zone-interior.md) finds the same hash reached one level of
indirection deeper for damage-group names, and [10 §"handle"](10-family-actor-human.md) had already
flagged the negative claim as too strong for named world objects — reaching `pandemic_hash` via
`FUN_00db7c10` rather than by a direct call, which is exactly what a direct-caller census misses.

**Confidence: confirmed** that the handle value is not a hash, and **confirmed** that name resolution
is keyed by `pandemic_hash`. Still **open**: the instruction path inside `0x00758b30` — that the key is
a `pandemic_hash` is established from the data both sides agree on, not from tracing the callee.

## 8. Cross-links

- Container format and `pandemic_hash`: [docs/formats/lua_scripts.md](../formats/lua_scripts.md)
  (`FUN_00dc1e20` confirmed there; do not re-derive).
- Object/actor subsystems the resolved pointers land in:
  [docs/symbol_map/README.md](../symbol_map/README.md), and in particular
  [ai-behavior.md](../symbol_map/ai-behavior.md), [damage-physics.md](../symbol_map/damage-physics.md),
  [vehicle-train.md](../symbol_map/vehicle-train.md) for the classes reached through `vtable+0x1c`.
- Class inventories: `data/rtti_classes_all.txt`, `data/ws_engine_classes.txt`.
- The marshalling primitives, the namespace bridge (`WRAPPER_*.lua`), and the registration table around
  `0x00716a85` are the scope of the sibling docs in this directory and are deliberately not duplicated here.

---

## Open questions

1. **Where is `DAT_0143db28` written?** No store to it survives in the decomp text; only 279 reads. The
   `manager + 0x1020` identification in §3 is a layout match, not a proof. Finding the store pins the owner
   class by RTTI and would upgrade most of §3 from inferred to confirmed.
2. **What is `DAT_012119cc`** — the one key value `FUN_00497f10` reserves and refuses to insert? A
   "null handle" constant, or a specific singleton's ID? It is guarded by a lazy-init flag
   (`_DAT_012119d0 & 1`, set once with `FUN_00db7e10(&DAT_00f8c177,1)`), which suggests it is *computed*
   at first use rather than a compile-time constant.
3. **`vtable+0x1c` is unnamed.** The proposal in §4 rests on usage, not on a vtable read or an assertion
   string. Reading the vtable of a known RTTI class in the 0x1c slot would settle it and is cheap.
4. **How does `GetHandleByName` resolve a string?** **Partly answered (2026-07-29)** — the key is
   `pandemic_hash(name)`, see §7. Two pieces remain. (a) The binding could not be located by string:
   none of `GetHandleByName`, `GetNameFromHandle`, `GetIntFromHandle`, `IsHandleValid`,
   `IsObjectHandleValid`, `GetMeleeHandleByName` appears as a literal in the decomp, so these carry no
   assertion and must be found through the registration table — though `0x00758b30` is now known from
   [00](00-seam-overview.md). (b) `GetNameFromHandle` proves a reverse map exists, and *that* direction
   is genuinely interesting now: the name is not stored in EditNodes, so either the registry keeps the
   string at spawn time or the reverse map is lossy.
5. **`GetIntFromHandle`** (`data/lua_bindings.txt:267`) is suggestive — it may expose the raw 32-bit ID to
   script. If so, it is the cheapest possible confirmation of §2 from a live game: print it and check the
   top byte increments as objects of the same slot are destroyed and respawned.
6. **Thread topology.** §6 shows the map has non-Lua readers and that the script manager keeps per-thread
   scratch, but whether a binding ever *executes* off the main thread is not established from static
   analysis. An `x32dbg` breakpoint on `FUN_006f8470` with a thread-ID log would answer it in one run.
7. **Generation wraparound.** 8 bits means a slot reused 256 times can validate an ancient stale handle.
   Whether the free ring's depth makes this unreachable in practice depends on the ring's capacity
   (`DAT_01321e98`), which is a runtime value not recoverable from the decomp text.
