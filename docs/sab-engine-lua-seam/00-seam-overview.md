# The engine ↔ Lua seam: overview

*The front door. Read this first; then follow the links.*

The Saboteur's gameplay logic lives in Lua. Mission scripts, checkpoints, freeplay ambients, the
behaviour modules for soldiers and vehicles — all of it is script calling into a C++ engine across a
seam of ~898 bindings. This document explains that seam end to end: how a Lua call reaches C, how a
handle resolves to a game object, how the engine calls back into Lua, and where the `WRAPPER_*.lua`
layer sits in all of it.

Six sibling docs reverse the mechanics in depth. This one fuses them, and — importantly — **flags where
they contradict each other**. Three of those contradictions are load-bearing, and one of them (§8.1) is
the difference between a correct and an incorrect binding signature. A contradiction between two careful
agents is a finding, not an embarrassment; it usually means each was looking at a different subset.

| Doc | Covers |
|---|---|
| [01-registration-and-dispatch.md](01-registration-and-dispatch.md) | How a C++ function becomes a callable Lua name; the `LuaGlueFunctor` template; the 26 tables |
| [02-marshalling-abi.md](02-marshalling-abi.md) | How a binding reads its arguments; the `FUN_006f7xxx` primitives; Lua 5.1 identification |
| [03-handle-and-object-model.md](03-handle-and-object-model.md) | What a handle *is*; the ID allocator; the guarded map; stale-handle behaviour |
| [04-vm-lifecycle-and-script-objects.md](04-vm-lifecycle-and-script-objects.md) | The single VM; the script manager singleton; `require`; the frame tick |
| [05-engine-to-lua-callbacks.md](05-engine-to-lua-callbacks.md) | How C++ holds a Lua callback (by name, never by reference) |
| [06-lua-side-wrapper-layer.md](06-lua-side-wrapper-layer.md) | The caller side: namespace tables, `WRAPPER_*`, enum tables, cut surface |

Machine-readable output: [`data/lua_registration_map.tsv`](../../data/lua_registration_map.tsv) — 898 rows
of `{table, lua_name, cpp_symbol, family, shape, impl_va, …}`, produced by
[`tools/dump_lua_registration.py`](../../tools/dump_lua_registration.py) from the retail `.exe` alone.
**That file, not [`data/lua_bindings.txt`](../../data/lua_bindings.txt), is the authoritative name list.**

---

## 1. The shape of the whole thing, in one paragraph

There is exactly **one `lua_State`**, holding **stock Lua 5.1** (one customization: `lua_Number` is
`float`, so `TValue` is 8 bytes — [02 §2](02-marshalling-abi.md)). At level load the script manager
singleton builds it, then publishes **26 namespace tables** (`Actor`, `Util`, `Vehicle`, …) into `_G`
via stock `luaL_register`, before any script runs. Each table entry is a compiler-generated 32-byte
thunk wrapping one C++ function. When script calls `Actor.Ragdoll(hVitt)`, Lua calls that thunk, which
calls the real `ActorRagdoll(lua_State*)`; that function reads argument 1 off the Lua stack as light
userdata, resolves it through a generation-checked ID map to a game object, does the work, and pushes
nothing. Going the other way, the engine never holds a Lua function value — it stores **callback name
strings** and resolves `_G[table][field]` by string on every fire, from a once-per-frame event pump.

## 2. Lua → C: how a call arrives

**The name.** Every binding is one instantiation of a C++ class template, `LuaGlueFunctor0<&F>` (for
`void F(lua_State*)`) or `LuaGlueFunctor0R<int,&F>` (for `int F(lua_State*)`). MSVC's RTTI leaves the
C++ function symbol in the clear, which is where `lua_bindings.txt`'s 898 names came from — and that is
the seam's biggest trap: **those are C++ symbols, not Lua names.** 256 of 898 differ.
`ActorRagdoll` is not callable; `Actor.Ragdoll` is ([01 §Naming](01-registration-and-dispatch.md)).

**Registration.** There is no static `{name, fnptr}` table anywhere — [01](01-registration-and-dispatch.md)
established this by exhaustive scan. Registration is imperative: an 84-byte stanza per binding links a
node carrying the Lua name string into a per-table registry, 26 registries in all, one per
`Script\*.cpp` translation unit. The counts close exactly: 722 `LuaGlueFunctor0` + 176
`LuaGlueFunctor0R` = 898 = `|lua_bindings.txt|`, and the 26 tables sum to 898.

**Publication (new — see [§7](#7-what-this-fusion-added)).** At init, `FUN_006f8a90 @0x006f8a90` walks the
registries and calls `FUN_006f6690 @0x006f6690` on each, which flattens the intrusive list into a
NULL-terminated `luaL_Reg[]` array and hands it to stock **`luaL_register(L, "<TableName>", regs, 0)`**.
That creates or reuses the global table. This is why the Lua corpus's `if not Vehicle then Vehicle = {} end`
guard never fires ([06 §2](06-lua-side-wrapper-layer.md)) — the table already exists.

**Dispatch.** What `luaL_register` installs is the **thunk**, so the thunk *is* the `lua_CFunction`.
Three shapes exist, and **the shape decides the binding's return contract** — this is the single most
important fact for reading any binding, and the three docs disagreed about it (see [§8.1](#81-how-many-results-does-a-binding-return)):

| family | shape | n | thunk code | nresults |
|---|---|---:|---|---|
| `LuaGlueFunctor0` | `adapter` | 640 | `push L; call F; add esp,4; mov eax,1; ret` | **always 1** |
| `LuaGlueFunctor0` | `inlined` | 82 | body inline, then `mov eax,1; ret` | **always 1** |
| `LuaGlueFunctor0R` | `jmp` | 172 | `jmp F` | **F's own EAX** |
| `LuaGlueFunctor0R` | `inlined` | 4 | body inline | F's own EAX |

The long-standing "callers inside no exported function" anomaly (calls from `0x00716a85`, `0x00716b25`)
is fully explained: that region is the **thunk array**, and each reported caller VA is arithmetically
`thunk+5` — the address of the `call`. Ghidra created no functions there because the only reference to
each thunk is a data pointer in a vtable ([01 §Anomaly](01-registration-and-dispatch.md)).

## 3. Reading arguments: the marshalling layer

Inside `F`, the recurring `FUN_006f7xxx` calls are not a Pandemic convention. They are one-line
`__thiscall` methods on a thin wrapper class whose field 0 is the `lua_State*`; each lowers to exactly
one stock Lua C API call. Ghidra hides the `this` pointer (it lives in ECX), which is why they look like
bare `FUN_006f71a0(1)` — **that `1` is a plain 1-based Lua stack index**, and the call means
`lua_type(L,1) == LUA_TLIGHTUSERDATA` ([02 §4](02-marshalling-abi.md)).

The decoder ring is in the [cheat sheet](#cheat-sheet-pointer) and in [02 §5](02-marshalling-abi.md). Two
properties matter architecturally. First, because `lua_type` returns `LUA_TNONE` for an absent index,
every check is a **combined presence-and-type test** — a missing argument is indistinguishable from a
wrong-typed one. Second, every *fetch* re-checks the type internally and **returns zero on mismatch
rather than throwing**. That is the wrapper's only editorial decision over stock Lua, and it means bad
arguments produce silence, not errors.

## 4. Handles: what they are and how they resolve

A handle is **light userdata carrying a 32-bit salted object ID**: 24 bits of slot index, 8 bits of
generation ([03 §2](03-handle-and-object-model.md)). It is minted by a global ring allocator
(`FUN_0044aa57`) and invalidated by a generation bump on destruction (`FUN_0068bad0`). Resolution is a
red-black-tree find (`FUN_004436f0`) under a critical section, and then usually a second gate: a weak
reference (`FUN_0083a200`) that re-validates the generation and returns the live target or zero.

Three consequences a tools author must internalise:

- **There are two independent liveness gates.** The map answers "does this ID still name a registered
  proxy?"; the weak ref answers "is the object it points at still alive?" A handle can pass one and fail
  the other.
- **A stale handle does nothing at all.** All three failure edges return to Lua with no error, no log,
  no pushed result. `Actor.Ragdoll(hDeadGuy)` is a silent no-op. This is why the corpus is so defensive,
  and why `WRAPPER_SanityCheck` can only catch `nil` — a stale handle is still perfectly good light
  userdata ([03 §5](03-handle-and-object-model.md)).
- **Handles do not survive save/load**, or even a streaming reload. Nothing about the ID is
  content-derived. This is precisely why the script layer passes *names* and re-resolves them through
  `Util.GetHandleByName` on every call ([03 §7](03-handle-and-object-model.md)).

The critical section is *not* about Lua being multi-threaded. The same map is read by engine threads
that never touch a `lua_State`; the lock protects the tree, and is released before the object is used.

## 5. C → Lua: callbacks, the tick, and the FSM

**The engine never stores a Lua function value.** There is no `luaL_ref` anywhere in the callback family
([05 §3](05-engine-to-lua-callbacks.md)). A callback is registered as a **name string** plus an optional
`self` table and user table:

```
Vehicle.SetDeathCallback(hObject, "OnDeath", self, tUserTable)
```

and resolved by name at fire time. Names are dotted and split on `'.'` into table and field
(`FUN_0070a180`). This makes the entire callback surface late-bound, and it is why
`Util.BroadcastFunction(h, "AttackTarget", {…})` and `SetDeathCallback` are the same mechanism wearing
different hats.

**Script objects are never ticked.** There is no `obj:Update(dt)`. Once per frame `FUN_00439ff0` calls
`FUN_006fb2c0`, which drains per-event-type, lock-guarded queues and dispatches named handlers
(`OnStateChange`, `OnVehicleEnter`, `OnPurchase`, …). Producers are other threads; the `lua_State` is
entered only from this pump ([04 §6](04-vm-lifecycle-and-script-objects.md)). `OnEnter`/`OnExit` are not
part of the tick at all — they are edge-triggered by the controller FSM (`FUN_0083ad20`). The idiomatic
consequence is visible in `ScriptControllers/PoisonField.lua:5`: `OnEnter` does no work, it *subscribes*.

Engine→Lua dispatch resolves `_G[obj][method]` **by string on every call, with zero caching**
(`FUN_01611c60`), under `lua_pcall` with **`errfunc = 0`** — so in-game Lua errors carry no traceback,
just a bare message in the 256-byte buffer `DAT_0142cfb8` ([04 §2, §8](04-vm-lifecycle-and-script-objects.md)).
A renamed global silently no-ops rather than crashing.

## 6. Where the wrapper layer sits

The most common misreading is that `WRAPPER_*.lua` is the namespace layer. It is not
([06 §6](06-lua-side-wrapper-layer.md)). The namespace tables come from the engine (§2). The wrappers
are a flat, SHOUTY layer *on top of* them, solving three problems: **string-or-handle coercion**
(`WRAPPER_CheckForHandle` — why designers can write `ACTOR_WalkToObject("Sean","Cafe")`), **composition**
(one `ACTOR_WalkPathOnce` = three `Nav.*` calls plus an enum), and **event-struct construction**
(`WRAPPER_Event.lua` is 712 lines of the same `Util.CreateEvent` skeleton).

So the full stack, top to bottom:

```
mission script          ACTOR_WalkToObject("Sean", "Cafe")     <- WRAPPER_*.lua, flat, Lua
namespace table         Nav.SetScriptedPath(hSean, ...)        <- engine-injected via luaL_register
thunk                   0x00716b20  push L; call F; mov eax,1  <- LuaGlueFunctor0
implementation          F(lua_State*)                          <- reads args via FUN_006f7xxx
handle resolution       FUN_004436f0 -> vtable+0x1c -> FUN_0083a200
engine object
```

Modding consequence, worth stating because it follows from §2 and is confirmed twice: **a modder cannot
add a C binding from script**, but **any namespace table can be freely patched or extended** — they are
ordinary globals, published before any script runs, with no `__newindex` and no readonly metatable.

---

## 7. What this fusion added

Four results that none of the six docs had alone. All were derived while cross-checking their
disagreements, and each is byte-level or mechanically reproducible.

**7.1 The registry drain is `FUN_006f6690` — doc 01's biggest open question, closed.**
`FUN_006f8a90 @0x006f8a90` (`callers=[0x006fab46(FUN_006faad0)]` — the manager Init that
[04 §4](04-vm-lifecycle-and-script-objects.md) transcribed) runs two loops: one over a `+0x70`/`+0x74`
count/array of function pointers, then one over `manager+0x04` registries calling
`FUN_006f6690(registry, L)`. That function:

```c
arr = malloc((registry[+0x10 /*count*/] + 1) * 8);   // luaL_Reg[] : {const char* name; lua_CFunction f}
registry[+0x00] = arr;
for (node in list at registry[+0x04]) {
    arr[i].func = (**(code **)**(undefined4 **)node)();   // virtual call -> the getter stub -> the thunk
    arr[i].name = *(node + 4);                            // the Lua name string
    i++;
}
arr[i] = {0, 0};
thunk_FUN_015fd2d0(L, registry + 0x14, arr, 0);      // luaL_register(L, "<TableName>", regs, 0)
FUN_006f67f0(registry);                              // walk the list, unlink, free every node
```

`FUN_015fd2d0` is stock `luaL_openlib`/`luaL_register` — identified by its `"_LOADED"` and
`"name conflict for module '%s'"` literals, which are `lauxlib.c` verbatim. So: **the namespace tables
are created by stock `luaL_register` at VM init, in registry-array order, and the intrusive list is
freed immediately afterward** — it is a build-time staging structure only. This simultaneously closes
[01 Q2](01-registration-and-dispatch.md#open-questions) (drain point and order),
[06 Q4](06-lua-side-wrapper-layer.md#open-questions) (how tables are injected — `luaL_register` at init,
before any script), and confirms doc 04's guessed comment `FUN_006f8a90(uVar4); // register bindings`.
**Confidence: confirmed** (bodies read; the `luaL_Reg` 8-byte stride, the NULL terminator, and the
4-argument `luaL_openlib` call all agree).

**7.2 The 0x3c-byte registry is fully decoded — doc 01 Q3, closed.** `FUN_01638da0 @0x01638da0` is the
ctor:

| Offset | Field | Evidence |
|---|---|---|
| `+0x00` | `luaL_Reg*`, built at publish | untouched by ctor; written by `FUN_006f6690` |
| `+0x04` | circular list head, self-linked | `*(this+4) = this+4` |
| `+0x08` | tail pointer (`= this+4`) | `*(this+8) = this+4` |
| `+0x0c` | node counter (teardown) | zeroed by ctor; used by `FUN_006f67f0` |
| `+0x10` | registered-binding count | zeroed by ctor; bumped by `FUN_006f6660` |
| `+0x14`–`+0x3b` | `char name[0x28]` — the **table name**, inline | `_strncpy(this+0x14, param_2, 0x27)`; NUL at `+0x3b` |

`0x14 + 0x28 = 0x3c`, matching the `operator new(0x3c)` exactly — independent confirmation the decode is
right. **Confidence: confirmed.**

**7.3 Doc 01's SecuROM blocker is wrong.** [01](01-registration-and-dispatch.md#how-the-surface-is-enumerated-at-boot)
reports that `FUN_006f6620` is `jmp 0x01638da0`, that `0x01638da0` lies inside the `.secu` section, and
concludes "the registry ctor body has been relocated behind the protection… the consumption side is not
[readable] statically." **It is readable.** `FUN_01638da0` is in the decomp with `size=56` and a
perfectly coherent body (7.2) — and its decode is corroborated by matching `new(0x3c)` to the byte.
Docs [04](04-vm-lifecycle-and-script-objects.md) and [05](05-engine-to-lua-callbacks.md) had already
transcribed neighbours in the same range (`FUN_01639500` the VM ctor, `FUN_015fe1a0` `luaL_loadstring`,
`FUN_016395d0` the pcall wrapper) without remarking on it. The decomp contains 162 `FUN_0163xxxx` and 49
`FUN_015fxxxx` functions. Whatever the `.secu` section is, **the code Ghidra recovered from that range
is real and usable**; a `jmp` into it is an ordinary incremental-linker thunk, not a protection wall.
This is what unblocked 7.1. **Confidence: confirmed** (the function is present and its body reconciles
with independent byte-level facts).

**7.4 The dumper's pairing is independently validated, 12/12.** Cross-checking every function that
contains both a `Script\*.cpp` assertion string and a name from `lua_bindings.txt` against
`lua_registration_map.tsv`:

| assertion @VA | C++ symbol in assertion | map says |
|---|---|---|
| `0x00714230` | `ActorRagdoll` | `Actor.Ragdoll`, impl `0x00714230` ✓ |
| `0x0073ab70` | `MissionTeleportPlayerToLocator` | `Object.PlayerTeleportToLocator`, impl `0x0073ab70` ✓ |
| `0x00741c10` | `SaveLoadSetupSpecialLuaTimerCallback` | `SaveLoad.SetupSpecialLuaTimerCallback` ✓ |
| `0x00736b90` | `EnterFormation` | `Nav.EnterFormation` ✓ |
| … 8 more | | all ✓ |

12 distinct bindings, 12 matches, 0 mismatches. (`SaveLoadLoadCheckpoint` appears as a string in two
functions, `0x007411c0` and `0x00741e90`; the map picks `0x00741e90`. That is a duplicated string, not a
pairing error.) Notably this **confirms doc 06's weakest naming inference** —
`Object.PlayerTeleportToLocator` ← `MissionTeleportPlayerToLocator`, which it flagged as its least
supported guess because the interface prefix changes. **Confidence: confirmed.** The tsv can be trusted.

---

## 8. Contradictions between the six docs

### 8.1 How many results does a binding return?

The most consequential disagreement, because it changes signatures:

- [01](01-registration-and-dispatch.md) found three thunk shapes and left "what is `mov eax, 1`?" as its
  **most load-bearing open item**, explicitly declining to guess.
- [02 §6](02-marshalling-abi.md) says bindings are stock `lua_CFunction`s that report arity by returning
  the count in EAX, proving it with `FUN_00710750`, which returns `0` on bad args and `1` after a push.
- [05 §1](05-engine-to-lua-callbacks.md) says the thunk hardcodes `return 1`, therefore **"every binding
  returns exactly one result, always"**, and that a binding pushing nothing returns an unspecified stack
  slot.

**Resolution: the `family` column decides, and both 02 and 05 generalised from their own subset.**
Doc 02's own proof function is the tell — `FUN_00710750` is `Actor.GetActorDist`, and the map types it
`LuaGlueFunctor0R / jmp / nresults=eax`. It is one of the 172 `R` bindings, whose thunk is a bare `jmp`
so the implementation's own EAX *is* nresults. Doc 05 only examined `LuaGlueFunctor0` bindings, whose
adapter really does hardcode `mov eax,1`. So:

> **722 of 898 bindings (`LuaGlueFunctor0` = 640 `adapter` + 82 `inlined`) always claim exactly 1
> result regardless of what they pushed. 176 (`LuaGlueFunctor0R` = 172 `jmp` + 4 `inlined`) return
> their own count.** Doc 05's "always, every binding" is wrong for 20% of the surface; doc 02's "arity
> is the EAX return" is mechanically true for all, but only *variable* for those 176.

This also answers doc 01's open question in the affirmative: since `luaL_register` installs the thunk
directly as the `lua_CFunction` (§7.1), `mov eax, 1` **is** genuinely `nresults`, not a `bool handled`
convention. The consequence doc 05 flagged is real and now sharper: a void binding that pushes nothing
still claims one result, so Lua reads the topmost stack slot — which, having pushed nothing, is the
binding's own last argument. *(That last step is **inferred** from stock 5.1 `luaD_poscall` semantics;
it has not been observed live and would take one breakpoint to confirm.)*

### 8.2 Is `lua_bindings.txt` a census or a floor?

- [01](01-registration-and-dispatch.md): the counts close exactly (899 RTTI descriptors = 898 stanzas =
  26 tables = 898 names). 59 corpus calls don't resolve; they are most likely **dev-build bindings cut
  from retail** — the corpus is decompiled *development* script, the binary is retail.
- [06 §5](06-lua-side-wrapper-layer.md): 46 live namespaced calls resolve to no binding under any rule
  and are not Lua-defined, therefore **"the 898 list is a floor, not a census"** and a second
  registration mechanism probably exists. Its load-bearing example is `Util.SetDisableControls`,
  "definitively engine-side, definitively called, definitively not in the 898".

**Resolution: mostly doc 01, but not entirely.** Testing doc 06's own orphan list against the byte-level
map, **5 of 29 resolve** — doc 06's correspondence heuristic simply failed on them:

| doc 06 called it an orphan | actually registered as |
|---|---|
| `Util.SetDisableControls` | C++ `SetDisableControlsTable`, impl `0x00750110` |
| `Actor.ExitSpecialKillMode` | C++ `PlayerKillModeCancel`, impl `0x007163c0` |
| `Vehicle.SetupRace` | C++ `SetPlayerRoad`, impl `0x0075ffb0` |
| `SaveLoad.ClearCheckpoint` | C++ `SaveLoadClearcheckpoint`, impl `0x00741ed0` |
| `AttractionPt.EnableBroadcast` | C++ `AttractionPtEnable`, impl `0x00717790` |

Doc 06's single strongest argument is therefore **refuted by its own example**: `SetDisableControls` is
registered, under a C++ symbol (`SetDisableControlsTable`) that doc 06 saw and dismissed as a different
binding. With that gone, the case for a second mechanism is weak, and doc 01's exact count closure makes
it hard to sustain. **But 24 orphans remain genuinely absent** from the map (`Combat.TakeCover`,
`Util.FindObjectHandle`, `Actor.SetUserFlag`, `Nav.MoveToSchedulePoint`, …), so doc 06's *observation*
survives even though its *conclusion* does not. The best reading — **inferred**, not proven — is doc
01's: those are dev bindings cut from retail, and calling them in the shipped game would raise "attempt
to call a nil value". The 2008 pre-release build (tracked as a separate lead) settles it directly.

### 8.3 Is the Lua name recoverable from the binary, or only from the corpus?

- [05 §5](05-engine-to-lua-callbacks.md): "Only the corpus is authoritative for the Lua-visible name."
- [06 §1](06-lua-side-wrapper-layer.md): "Every mapping below is recovered by correspondence between the
  corpus and the C++ names, not read directly from a registration site. **That is the central caveat of
  this document.**" Its §3 then classifies all 898 heuristically (identity 448 / prefixed 131 / permuted
  25 / near 122 / unreferenced 165).

**Resolution: doc 01 supersedes both.** The Lua name is a plain `.rdata` string in each registration
stanza, and the tsv reads it directly, validated 12/12 (§7.4). Docs 05 and 06 were both written against
the assumption that the registration site was unreadable — the same assumption §7.3 disproves. **Doc
06's §3 class counts and doc 05's §6 "table open" rows should be treated as superseded**, not as
independent corroboration. Where doc 06 guessed a rename it was often right (`Util.CreateEvent` ←
`CreateEventA`; `Render.HeatShimmerFilter` ← `HeatShimmerFilterCallback`, both in the map), but the
guessing is no longer necessary. Doc 05's six "unresolved registrars" are all in the map, too
(`Util.ClearDisguiseCallback` `0x0075d1f0`; `Render.Rain` ← C++ `RainCallback` `0x0073d410`; …).

### 8.4 The assertion-string method

The project brief — and [02 §7 step 2](02-marshalling-abi.md) — say to find a binding's name by scanning
for an EALA `Script\*.cpp` assertion, where "the next string literal is the **Lua-visible binding name**".
[05](05-engine-to-lua-callbacks.md) and [06 §1](06-lua-side-wrapper-layer.md) independently show this is
wrong twice over: **only 12 of 898** names appear as quoted strings anywhere in the 54 MB decomp (21
assertion sites total), and the string is the **C++ symbol**, not the Lua name. `ActorRagdoll` is a real
anchor but it does not generalise. Doc 02's recipe step 2 should be read as superseded by the tsv; the
cheat sheet below corrects it.

### 8.5 Smaller ones, resolved

| Tension | Resolution |
|---|---|
| `FUN_006f8470` = "frame setup" ([05 §2](05-engine-to-lua-callbacks.md), and the brief) vs. "state→wrapper lookup" ([02 §4](02-marshalling-abi.md)) | **02 wins** — it read the body: a 1-slot linear search at `mgr+0x124` for the wrapper whose field 0 == `L`. §7.1 corroborates (`FUN_006f6690(*param_2)` passes `wrapper[0]` where `luaL_register` wants `L`). |
| `DAT_0142d324` "not an s_name slot, otherwise unidentified" ([01](01-registration-and-dispatch.md)) | Answered by [04 §3](04-vm-lifecycle-and-script-objects.md): the **script manager singleton**, `0x15c90` bytes — which also corrects `symbol_map/task-managers.md`'s "world object" / `WSContext::GetGlobal` reading. |
| "Is the callback invoked under `lua_pcall` or bare `lua_call`?" — [05's highest-value open question](05-engine-to-lua-callbacks.md#open-questions), which could not find `lua_pcall` | [04 §2](04-vm-lifecycle-and-script-objects.md) pins `lua_pcall = FUN_00408310` and the wrapper `FUN_016395d0` = `lua_pcall(L, nargs, LUA_MULTRET, 0)`. So the `_G[obj][method]` path **is** protected, with no traceback. Whether the `FUN_0070a180`-stored callback name path uses that same invoker is **inferred, not confirmed**. |
| `GetHandleByName`/`GetSelf` "cannot be located, they carry no assertion string" ([03 Q4](03-handle-and-object-model.md#open-questions), [04 Q5](04-vm-lifecycle-and-script-objects.md#open-questions)) | In the map: `Util.GetHandleByName` impl `0x00758b30`; `Actor.GetSelf` impl `0x007126c0`; `Util.GetIntFromHandle` `0x0074c720`; `Util.IsHandleValid` `0x00758cd0`. All `jmp` family. *How* `GetHandleByName` maps a string to an ID: the key is `pandemic_hash(name)` (2026-07-29, [03 §7](03-handle-and-object-model.md)); the instruction path inside `0x00758b30` is still untraced. |
| Resolution rates: 663/898 ([05](05-engine-to-lua-callbacks.md)) vs 898/898 ([01](01-registration-and-dispatch.md)) | Not a contradiction — different methods. 05 walked name-string→vftable and missed the `inlined` shapes; 01 walked RTTI→COL→vtable→stub→thunk and closed. |

---

## 9. What is still open

Ranked by value:

1. **Are the 24 remaining orphan calls (§8.2) cut dev bindings?** Directly testable against the 2008
   pre-release build. Settles whether `lua_bindings.txt`+the map is a census.
2. **Do engine→Lua *callbacks* (the `FUN_0070a180` name path) go through the same protected invoker as
   `_G[obj][method]`?** Inferred yes; unconfirmed. Determines whether a script error inside `OnDeath`
   aborts the frame or is swallowed. One breakpoint on `FUN_0045ee96` (`luaD_pcall`) with a
   deliberately-erroring callback answers it.
3. **Is the old `lua_State` closed on reload?** `lua_close` has `callers=[]`. If nothing closes it, every
   level reload leaks a full VM ([04 Q1](04-vm-lifecycle-and-script-objects.md#open-questions)).
4. **~~How does `GetHandleByName` resolve a string to an ID?~~** **Largely ANSWERED 2026-07-29.** The
   lookup key *is* `pandemic_hash` of the name string — 1 632 path literals in `LuaScripts.luap`
   reproduce a retail EditNodes object tag, 0 colliding with the container namespace
   ([editnodes.md](../formats/editnodes.md), [03 §7](03-handle-and-object-model.md)). The earlier
   "it is *not* `pandemic_hash`" is retracted. What remains open is narrower: the instruction path
   inside `0x00758b30`, and whether non-EditNodes objects register under the same keying.
5. **What is `vtable+0x1c`?** The proposed `GetTargetRef()` rests on usage only. Reading slot 7 of a
   known RTTI class's vtable is cheap.
6. **Where is `DAT_0143db28` written?** Finding the store pins the handle map's owner class by RTTI
   ([03 Q1](03-handle-and-object-model.md#open-questions)).
7. **The `mov eax,1` consequence** (§8.1): does a void binding really return its own last argument to
   Lua? Trivial to check live.
8. `FUN_006fb2c0`'s full handler set is not exhausted ([04 Q8](04-vm-lifecycle-and-script-objects.md#open-questions)) —
   a full read completes the engine→Lua event vocabulary.

The x32dbg MCP tooling is available in this environment, and items 2, 3 and 7 are all single-breakpoint
questions.

---

## Cheat-sheet pointer

The dense, standalone ABI digest for reading an arbitrary binding's decomp body and deriving its
signature — primitives, arg-index convention, type tags, the handle idiom, the return-push idiom — lives
with the [02-marshalling-abi.md](02-marshalling-abi.md) decoder ring, **with these corrections from this
document applied**: the return contract depends on the `family` column of
[`data/lua_registration_map.tsv`](../../data/lua_registration_map.tsv) (§8.1), and the assertion-string
method names the C++ symbol, not the Lua name, and exists for only 12 of 898 bindings (§8.4).

## Sources

Every function body cited was read from
`C:/Users/Shadow/Desktop/notes-on-the-released-game/output/_ghidra_saboteur/saboteur_all_functions_decomp.txt`
(36,935 functions; not in this repo). Names and VAs are from
[`data/lua_registration_map.tsv`](../../data/lua_registration_map.tsv), regenerable from retail
`Saboteur.exe` with [`tools/dump_lua_registration.py`](../../tools/dump_lua_registration.py). Corpus
citations are file:line into [`docs/saboteur-luacd/src`](../saboteur-luacd/src). No fact here is carried
over from Mercenaries 2 work.
