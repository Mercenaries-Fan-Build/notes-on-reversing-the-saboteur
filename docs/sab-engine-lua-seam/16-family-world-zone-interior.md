# Family 16 — World: zones, interiors, water, traffic & streaming

> **Verified:** All 55 VAs re-checked against the decomp (39 present; the same 16 confirmed absent, and the
> three claimed export gaps reproduce exactly); all 14 disasm rows re-disassembled from retail
> `Saboteur.exe` and reproduce instruction-for-instruction; corpus citations and call counts (142/199/0/6)
> re-run and correct; no Mercenaries 2 import. **Corrected:** zone type **1 is `Cafe`, not cut content** —
> it is reached from Lua via `Trigger.CreateCafe` (`FUN_0074ae90` → `FUN_00809300` →
> `thunk_FUN_016152a0` → `FUN_004c9110(1)`) and shipped as [`CafeRegion.lua:4`](../saboteur-luacd/src/Modules/Behavior/Triggers/CafeRegion.lua);
> the "every `Zone.*` call site uses the `WtF_Zones` prefix" universal (35 of 50 do); "every creator takes
> one handle and nothing else" (six of eight do — the doc's own table gave the other two); a
> "five/four" miscount; the `ActivateAmbush` citation (a deferred task-table entry, not a call).
> **Closed:** the unnamed `Util.AddInterior` field is **`bHQ`** (read from the exe at `0x00fe5b9c`;
> corroborated by the corpus). Types 6 and 9 re-tested and stand.

*How Lua tells the engine what the world **is** — which volumes are hostile, which buildings you can walk
into, where the water sits, whether cars drive, and what is resident in memory.*

This family is the seam's **world-state** surface. Unlike `Actor.*` or `Combat.*`, almost nothing here
drives a per-frame behaviour; it configures long-lived global systems and then gets out of the way. That
shape shows up in the bodies: a striking number of these bindings are two or three instructions that poke
a field on a singleton.

Read [02-marshalling-abi.md](02-marshalling-abi.md) first — this doc assumes the decoder ring. It
cross-links [`docs/symbol_map/world-water.md`](../symbol_map/world-water.md) (the engine-side view of the
same subsystem) and [12-family-suspicion-wtf-alarm.md](12-family-suspicion-wtf-alarm.md) (which owns the
WTF semantics that `Zone.*` transports).

---

## Inclusion rule (auditable)

A binding is in this family if its **C++ symbol** (from
[`data/lua_registration_map.tsv`](../../data/lua_registration_map.tsv)) matches
`zone|interior|water|traffic|roadblock|ambush|stream`, **or** it lives in the `Zone` table, **or** it is a
world-residency (streaming) call whose symbol uses the engine's own vocabulary for a streamed unit —
`Block`, `ENTag`, `DynamicNode`, `WSDData`. That yields **55 bindings**.

Deliberate boundary calls, stated so the edge is inspectable:

- **Claimed, though a sibling could:** `Trigger.Create*Zone` and `Trigger.CreateRestrictedArea` (zone
  *typing* is this family's core mechanism — see *Zones are typed, not created*); `Util.SetNumWTFZones` / `Util.RecordWTFZoneFlipped`
  (already tabled by [doc 12](12-family-suspicion-wtf-alarm.md) — I claim them only to **close doc 12's
  `open` row on `RecordWTFZoneFlipped`**); `Train.TrainIsStreamedIn` /
  `Train.TrainRegisterStreamoutCallback` (deferred in substance to
  [`world-water.md`](../symbol_map/world-water.md)'s sibling `vehicle-train.md`).
- **Excluded, with reason:** `Checkpoint.SetCheckZone`, `SetExitZone`, `SetLinkedEnterZone`,
  `SetLinkedExitZone`, `SetInteriorRestrictedArea` — these match the regex but are checkpoint-graph
  configuration, and the checkpoint family owns the object they mutate. `FocusPt.SetInteriorPts` —
  likewise a FocusPt call that merely takes an interior name. `Trigger.CreateCafe` (`0x0074ae90`) is a
  zone-typing sibling *by shape* but not by name; noted here, tabled by the trigger family — though it is
  the ninth member of the `Create*` set in substance, and it is what stamps **zone type 1** (see *Zones are
  typed, not created*), so excluding it by regex is what let type 1 look like cut content.
- **Ambiguous, claimed anyway:** `Cin.SetCinematicStreaming` (a `Cin` binding, but it is a streaming
  control — and its body is the family's best finding: see Open question 1).
- **Adopted orphans, outside the rule:** the two `Damage` bindings (see [The `Damage` table](#the-damage-table--prop-damage-states))
  match **none** of the terms above — not the regex, not the `Zone` table, not the streaming vocabulary.
  They are docked here editorially, because they appeared in no family doc and because they resolve world
  objects through the same `FUN_0067c0a0` this family uses. They are **not** family-16 members by the rule
  and are counted separately throughout; the rule still yields **55**.

---

## Coverage honesty

**55 of 55 bindings in this family located. 0 confirmed by assertion string, 53 confirmed at byte level
(39 from decomp pseudocode + 14 from direct disassembly), 1 inferred, 1 not derived.**

> **Appended after the verify pass: the `Damage` table (+2), as adopted orphans.** [The `Damage` table —
> prop damage states](#the-damage-table--prop-damage-states) was added later, to close two bindings that
> appeared in no family doc. Both are located and confirmed at byte level from direct disassembly. They
> are **not** family-16 members by this doc's [inclusion rule](#inclusion-rule-auditable) — they match none
> of its terms — so they do **not** change the 55 above. Counted separately, the doc now carries **55 of 55
> family rows + 2 of 2 adopted `Damage` rows = 57 bindings documented; 55 confirmed at byte level
> (39 decomp + 16 disasm), 1 inferred, 1 not derived.**

Two numbers need explaining.

**Zero assertion strings.** I grepped all 55 names as quoted string literals across the 54 MB decomp. Not
one hit. This family carries *no* `Script\*.cpp` assertion sites at all — consistent with the cheat
sheet's warning that only 12 of 898 bindings have one. Every identity here rests on the tsv (read
byte-level from the exe, validated 12/12 independently), not on a string. Per the rules, that is **not**
a reason to downgrade — but it does mean **no source file or line number is recoverable for any row in
this family**, so the "Source (file:line)" column is `—` throughout. That column is dead weight for
family 16, and saying so is more useful than filling it with guesses.

**A decomp-export gap, and how I closed it.** 16 of the 55 fall in address ranges the decomp simply does
not contain — `saboteur_all_functions_decomp.txt` emits 35,819 `FUN_` entries, and has contiguous holes
(e.g. all of `0x00754000`–`0x00757900`, and `0x007518f8`–`0x00751cc0`, which happens to swallow every
interior getter). This is an **artefact of the export, not a fact about the binary**. Rather than mark 16
rows `open`, I disassembled 14 of them **directly out of retail `Saboteur.exe`** with capstone. Those rows
are marked `confirmed (disasm)` and their evidence cites the instruction, not pseudocode. Where a row says
`confirmed (decomp)` the evidence is Ghidra pseudocode at the cited VA.

The remaining 2 gap rows are both `Train.*`, which this family only borrows: `TrainIsStreamedIn` is
**inferred** (disassembled, but its argument goes through a train-specific resolver I did not read) and
`TrainRegisterStreamoutCallback` is **not derived** (identity from the tsv; body unread). Both are named
as such in the table. No row is a guess.

---

## The bindings

Namespaced form is what scripts actually call; `Binding` is the flat C++ symbol from
`lua_bindings.txt`. Return contracts come from the tsv `family` column, never from the body (§6 of the
ABI). `A0` = `LuaGlueFunctor0` (thunk hardcodes 1 result); `A0R` = `LuaGlueFunctor0R` (real result count).

### Zone — the WTF transport (2)

| Binding | Namespaced form | VA | Source | Signature | Confidence | Evidence |
|---|---|---|---|---|---|---|
| `ZoneEnable` | `Zone.Enable` | `0x007659a0` | — | `(vZone:string\|handle, bEnable:boolean, eEntChange:number) -> ()` — all three **mandatory** | confirmed (decomp) | Body: `isstr(1) \|\| isHANDLE(1)`, then `&&`-chained `isbool(2)` `&&` `isnum(3)`; tail `thunk_FUN_005bce30(bool, int)`. [`CFP_DockDestroy.lua:26`](../saboteur-luacd/src/Missions/CFP_DockDestroy.lua) `Zone.Enable("WtF_Zones\global\FP_LeHavre", true, cENT_IMMEDIATE)` — A0 |
| `ZoneSwitchState` | `Zone.SwitchState` | `0x00765aa0` | — | `(vZone:string\|handle, eState:number, eEntChange:number [, bFlag:boolean] [, sCallback:string [, self:table [, tUser:table]]]) -> ()` | confirmed (decomp) | Body: mandatory `isstr(1)\|\|isHANDLE(1)` `&&` `isnum(2)`; `toINT(2)`, `toINT(3)`; opt `isbool(4)`; opt `isstr(5)` → callback idiom (`cb_setname`, `cb_setargn(6)`, `istable(7)`); tail `thunk_FUN_01618370`. [`Connect_A3_M1b_ReturnToBelle.lua:237`](../saboteur-luacd/src/Missions/Connect_A3_M1b_ReturnToBelle.lua) passes 4 args — A0 |

Note `isnum(3)` is **not** in the mandatory chain for `SwitchState` (only `Enable` requires it), yet
`toINT(3)` runs unconditionally — so omitting arg 3 yields a silent `0` = `cENT_IMMEDIATE`. Every corpus
call passes it anyway.

### Interiors (16)

The interior manager singleton is **`DAT_014a9d10`** — confirmed by five independent
`mov ecx, dword ptr [0x14a9d10]` sites (`0x0075b480`, `0x0075c440`, `0x00751948`, `0x00751a79`,
`0x00751b1f`) and corroborated by every callee landing in `0x009c9000`–`0x009cb000`, the exact range
[`world-water.md`](../symbol_map/world-water.md) pins as `WSInteriorManager` (`FUN_009ca1a0`
`InteriorManager_TeleporterMain`, `FUN_009cb240`).

| Binding | Namespaced form | VA | Source | Signature | Confidence | Evidence |
|---|---|---|---|---|---|---|
| `AddInterior` | `Util.AddInterior` | `0x00750eb0` | — | `(tDef:table) -> ()` — see field list below | confirmed (decomp) | `istable(1)`; body reads named fields via `thunk_FUN_01639630`/`01639350`; tail `FUN_009cad00`. [`InteriorManager.lua`](../saboteur-luacd/src/Managers/InteriorManager.lua) — A0 |
| `EnterInterior` | `Util.EnterInterior` | `0x00751260` | — | `(sInterior:string [, sLocator:string] [, bA:boolean] [, bB:boolean])` **or** `(sInterior, sCallback:string [, self:table])` -> () | confirmed (decomp) | `isstr(1)` mandatory; arg2 is read **twice** — as a locator (`FUN_009c9280`/`009c9360`) *and* as a callback name (`toSTR(3)`→`cb_setname`); tail `FUN_009cb7c0(&int, bA, bB)`. [`Belle_Interior.lua:167`](../saboteur-luacd/src/Modules/InteriorLevels/Belle_Interior.lua) `Util.EnterInterior("Belle", sLocator, bDisableFadeIn)` — A0 |
| `ExitInterior` | `Util.ExitInterior` | `0x00751410` | — | `(sInterior:string [, sLocator:string] [, bA:boolean] [, bB:boolean]) -> ()`; arg 1 **defaults to the player's current interior** | confirmed (decomp) | Same shape as `EnterInterior`, but `FUN_009c91f0(&iStack_4)` supplies a fallback when arg 1 is absent/empty; tail `FUN_009cb910`. [`Belle_Interior.lua:196`](../saboteur-luacd/src/Modules/InteriorLevels/Belle_Interior.lua) — A0 |
| `SpawnInterior` | `Util.SpawnInterior` | `0x0074cd30` | — | `(sNode:string [, sCallback:string [, self:table [, tUser:table]]]) -> ()` | confirmed (decomp) | `gettop()` variadic; `_strrchr(path, '\\')` **strips the directory** off arg 1; `DAT_0143d044 = 1`; opt callback idiom at 2/3/4; tail `thunk_FUN_0162df80`. [`__UtilFunctions.lua:99,101`](../saboteur-luacd/src/Includes/__UtilFunctions.lua) — A0 |
| `UnloadInterior` | `Util.UnloadInterior` | `0x0074ce70` | — | `(sNode:string) -> ()` | confirmed (decomp) | `isstr(1)`; `DAT_0143d044 = 0`; tail `FUN_009f4e90`. [`__UtilFunctions.lua:108`](../saboteur-luacd/src/Includes/__UtilFunctions.lua) — A0 |
| `SetInteriorFloorData` | `Util.SetInteriorFloorData` | `0x00751550` | — | `(sInterior:string, nFloor:number, f1,f2,f3,f4,f5:number [, f6:number]) -> ()` — 7 mandatory, 8th optional | confirmed (decomp) | Seven `&&`-chained checks `isstr(1) && isnum(2..7)`; `toINT(2)`, `toFLOAT(3..7)`; `gettop() > 7` gates `toFLOAT(8)`, else default `DAT_00f7ac80`; tail `FUN_009c9c00`. [`InteriorManager.lua`](../saboteur-luacd/src/Managers/InteriorManager.lua) — A0 |
| `LockInteriorDoors` | `Util.LockInteriorDoors` | `0x00751cc0` | — | `(sInterior:string, bLocked:boolean) -> ()` — both mandatory | confirmed (decomp) | `isstr(1)` + non-empty + `isbool(2)`; tail `FUN_009c9770(&int, bool)`. **No corpus call** — A0 |
| `AddInteriorLoadCallback` | `Util.AddInteriorLoadCallback` | `0x00751700` | — | `(sInterior:string, sCallback:string [, self:table [, tUser:table [, bOneShot:boolean]]]) -> ()` | confirmed (decomp) | `isstr(1)` non-empty `&&` `isstr(2)`; callback idiom (`cb_setname`, `cb_setargn(3)`, `istable(4)`); **arg 5 selects the sink**: `isbool(5) && toBOOL(5)` → `FUN_009cac60`, else `FUN_009cac10`. [`StarterManager.lua:622`](../saboteur-luacd/src/Managers/StarterManager.lua) passes all 5 — A0 |
| `CancelInteriorLoadCallback` | `Util.CancelInteriorLoadCallback` | `0x00751840` | — | `(sInterior:string [, bOneShot:boolean]) -> ()` | confirmed (decomp) | `isstr(1)` non-empty; `isbool(2) && toBOOL(2)` → `FUN_009caf70`, else `FUN_009caee0` — the mirror of the `Add` sink split — A0 |
| `ClearAllInteriorLoadCallbacks` | `Util.ClearAllInteriorLoadCallbacks` | `0x0075b480` | — | `() -> ()` — takes **no** arguments | confirmed (disasm) | Whole body: `mov ecx,[0x14a9d10]; call 0x9caff0; mov eax,1; ret`. Never touches the `lua_State` — `inlined`, so `mov eax,1` **is** the nresults |
| `InteriorLoadSetDisableTeleport` | `Util.InteriorLoadSetDisableTeleport` | `0x0075c440` | — | `() -> ()` — **ignores every argument**, always calls `f(0)` | confirmed (disasm) | `call 0x6f8470` (result discarded), then `mov ecx,[0x14a9d10]; push 0; call 0x9c9530`. No `FUN_006f7*` check anywhere. Corpus agrees: [`InteriorManager.lua:931,958`](../saboteur-luacd/src/Managers/InteriorManager.lua) call it with **no args** |
| `GetPlayersInterior` | `Util.GetPlayersInterior` | `0x00751900` | — | `() -> sInterior:string \| nil` — always exactly 1 result | confirmed (disasm) | 0 args; `mov ecx,[0x14a9d10]; call 0x9c9b30` with a `0x80`-byte stack buffer → `al`; true → `call 0x6f7080` (pushstring) `mov eax,1`; false → `call 0x6f7010` (pushnil) `mov eax,1`. [`InteriorManager.lua:789`](../saboteur-luacd/src/Managers/InteriorManager.lua) — A0R |
| `IsPlayerInInterior` | `Util.IsPlayerInInterior` | `0x007519a0` | — | `() -> boolean` — always exactly 1 result | confirmed (disasm) | 0 args; `mov ecx,[0x14a9d10]; call 0x9c9100`; `push eax; call 0x6f7020` (pushboolean); `mov eax,1; ret`. [`InteriorManager.lua:971`](../saboteur-luacd/src/Managers/InteriorManager.lua) — A0R |
| `IsInteriorEnabled` | `Util.IsInteriorEnabled` | `0x00751a00` | — | `(sInterior:string) -> boolean`; **bad arg → 0 results** | confirmed (disasm) | `call 0x6f7160` (isstr) → `toSTR` → non-empty → `strdup` → `mov ecx,[0x14a9d10]; call 0x9c97a0`; pushboolean; `mov eax,1`. Failure path `0x00751a96`: `xor eax,eax; ret`. [`DoorTeleporter.lua:37`](../saboteur-luacd/src/Modules/Behavior/AttractionPts/DoorTeleporter.lua) — A0R |
| `GetInteriorScriptByName` | `Util.GetInteriorScriptByName` | `0x00751aa0` | — | `(sInterior:string) -> sScript:string \| nil`; **bad arg → 0 results** | confirmed (disasm) | isstr→toSTR→non-empty→strdup→`mov ecx,[0x14a9d10]; call 0x9c9ba0` with a `0x80` out-buffer; `al` → pushstring / pushnil, `mov eax,1`. Failure `0x00751b6b`: `xor eax,eax`. [`InteriorManager.lua:878`](../saboteur-luacd/src/Managers/InteriorManager.lua) — A0R |
| `GetInteriorNameByScript` | `Util.GetInteriorNameByScript` | `0x00751b80` | — | `(sScript:string) -> sInterior:string \| nil`; **bad arg → 0 results** | confirmed (disasm) | Exact mirror of the above via `call 0x9c9200`, `0x80` buffer. Failure `0x00751c5a`: `xor eax,eax`. [`DoorTeleporter.lua:36`](../saboteur-luacd/src/Modules/Behavior/AttractionPts/DoorTeleporter.lua) — A0R |

**`Util.AddInterior`'s table schema** is read straight out of string literals in `FUN_00750eb0` — the only
place in this family where the engine names its own fields:

| Field | Type | Read via |
|---|---|---|
| `sName` | string | `thunk_FUN_01639630(1,"sName",…)` |
| `sScript` | string | `thunk_FUN_01639630(1,"sScript",…)` |
| `sIntTeleLoc` | string | interior-side teleport locator |
| `sExtTeleLoc` | string | exterior-side teleport locator |
| `bUnlocked` | boolean | `thunk_FUN_01639350` → flag bit `0x02` |
| `bHQ` | boolean | `thunk_FUN_01639350` → flag bit `0x08`. Ghidra rendered the literal as `PTR_LAB_00fe5b9c` because the bytes `62 48 51 00` (`"bHQ\0"`) also parse as a plausible pointer (`0x00514862`); reading `0x00fe5b9c` in the exe gives the string outright — **confirmed** |
| `sWTFBP` | string | WTF **blueprint** — the direct link to [doc 12](12-family-suspicion-wtf-alarm.md) |

`bHQ` is corroborated from the caller side: the corpus sets it in seven per-interior tables
([`InteriorManager.lua:64,100,136,235,261,289,397`](../saboteur-luacd/src/Managers/InteriorManager.lua))
and reads it at [`InteriorManager.lua:551`](../saboteur-luacd/src/Managers/InteriorManager.lua)
(`if tInterior and tInterior.bHQ then`) — so the engine's field name and the script's key agree, and this
table is the one place the two sides of the seam are visibly the same schema.

### Zone typing — the `Trigger.Create*` set (8)

| Binding | Namespaced form | VA | Source | Signature | Confidence | Evidence |
|---|---|---|---|---|---|---|
| `TrigCreateRedZone` | `Trigger.CreateRedZone` | `0x0074b150` | — | `(hCtrl:handle) -> ()` | confirmed (decomp) | Handle idiom → `FUN_00a0fff0` → `FUN_004c9b70` → `SetZoneType(3)`. [`RedZone.lua:6`](../saboteur-luacd/src/Modules/Behavior/Triggers/RedZone.lua) — A0 |
| `TrigCreateHostileZone` | `Trigger.CreateHostileZone` | `0x0074b330` | — | `(hCtrl:handle) -> ()` | confirmed (decomp) | → `FUN_004c9bd0`: **guards** `*(obj+0x102) != 5` then `SetZoneType(5)` + `FUN_0077f000`. [`HostileZone.lua:6`](../saboteur-luacd/src/Modules/Behavior/Triggers/HostileZone.lua) — A0 |
| `TrigCreateNoFlyZone` | `Trigger.CreateNoFlyZone` | `0x0074b3f0` | — | `(hCtrl:handle) -> ()` | confirmed (decomp) | → `FUN_004c9c20`: `SetZoneType(7)`, then `(**(obj+0x1c) + 0x14)(0)` and `FUN_00822fb0` — the only creator that registers with a second system. [`NoFlyZone.lua:6`](../saboteur-luacd/src/Modules/Behavior/Triggers/NoFlyZone.lua) — A0 |
| `TrigCreateWorldBorderZone` | `Trigger.CreateWorldBorderZone` | `0x0074b4b0` | — | `(hCtrl:handle) -> ()` | confirmed (decomp) | → `FUN_004c9c50`: `SetZoneType(8)`; `*(obj+0x11c) \|= 0x10`. [`WorldBorder.lua:6`](../saboteur-luacd/src/Modules/Behavior/Triggers/WorldBorder.lua) — A0 |
| `TrigCreateFightBackZone` | `Trigger.CreateFightBackZone` | `0x0074b570` | — | `(hCtrl:handle) -> ()` | confirmed (decomp) | → `FUN_004c9c00` → `SetZoneType(0xb)`. **No corpus call** — A0 |
| `TrigCreateDeleteZone` | `Trigger.CreateDeleteZone` | `0x0074af50` | — | `(hCtrl:handle) -> ()` | confirmed (decomp) | → `FUN_004ca1c0`: `SetZoneType(10)` **plus** a full teardown — frees a `PblArray` (asserts against `pebble\src\PblArray.h:0x1fa`), drains a list, clears `+0x11d & 0xf7`, virtual `+0x10`. [`DeleteZone.lua:6`](../saboteur-luacd/src/Modules/Behavior/Triggers/DeleteZone.lua) — A0 |
| `TrigCreateSuspicionZone` | `Trigger.CreateSuspicionZone` | `0x0074b210` | — | `(hCtrl:handle [, bOnFoot:boolean=true] [, bInVeh:boolean=true]) -> ()` | confirmed (decomp) | → `thunk_FUN_016151b0` → `SetZoneType(4)`; then arg2→`+0x11d` bit `0x04`, arg3→bit `0x02`, **both default `'\x01'`**. [`SuspicionZonePed.lua:6`](../saboteur-luacd/src/Modules/Behavior/Triggers/SuspicionZonePed.lua) `(h, true, false)`; [`SuspicionZoneVeh.lua:6`](../saboteur-luacd/src/Modules/Behavior/Triggers/SuspicionZoneVeh.lua) `(h, false, true)` — A0. Also tabled by [doc 12](12-family-suspicion-wtf-alarm.md) |
| `TrigCreateRestrictedArea` | `Trigger.CreateRestrictedArea` | `0x0074b010` | — | `(hCtrl:handle [, nLevel:number=0] [, bOnFoot:boolean=true] [, bInVeh:boolean=true]) -> ()` | confirmed (decomp) | → `thunk_FUN_01615090(nLevel)` → `SetZoneType(2)`, `*(obj+0x103) = nLevel`; then the same `+0x11d` bit pair at args 3/4. [`RestrictedArea2..5.lua:6`](../saboteur-luacd/src/Modules/Behavior/Triggers/RestrictedArea2.lua) pass `2,3,4,5`; [`RestrictedAreaPed.lua:6`](../saboteur-luacd/src/Modules/Behavior/Triggers/RestrictedAreaPed.lua) `(h, 0, true, false)` — A0 |

### Roads & escape (4)

All four are byte-identical apart from the tail call, and all four resolve the handle through
`FUN_00a0ffa0` — a **different** resolver from the `Create*Zone` set's `FUN_00a0fff0`.

| Binding | Namespaced form | VA | Source | Signature | Confidence | Evidence |
|---|---|---|---|---|---|---|
| `AddRoadBlock` | `Trigger.AddRoadBlock` | `0x00749950` | — | `(hTrigger:handle) -> ()` | confirmed (decomp) | `isHANDLE(1)` → `toHANDLE(1)` → `FUN_00a0ffa0` → `FUN_0089a4e0`. [`RoadBlockZone.lua:6`](../saboteur-luacd/src/Modules/Behavior/Triggers/RoadBlockZone.lua) — A0 |
| `RemoveRoadBlock` | `Trigger.RemoveRoadBlock` | `0x007499d0` | — | `(hTrigger:handle) -> ()` | confirmed (decomp) | Same → `FUN_0089a550`. [`RoadBlockZone.lua:10`](../saboteur-luacd/src/Modules/Behavior/Triggers/RoadBlockZone.lua) — A0 |
| `AddNoEscSpawnZone` | `Trigger.AddNoEscSpawnZone` | `0x00749a50` | — | `(hCtrl:handle) -> ()` | confirmed (decomp) | Same → `FUN_0089a3b0`. [`NoEscSpawnZone.lua:6`](../saboteur-luacd/src/Modules/Behavior/Triggers/NoEscSpawnZone.lua) — A0 |
| `RemoveNoEscSpawnZone` | `Trigger.RemoveNoEscSpawnZone` | `0x00749ad0` | — | `(hCtrl:handle) -> ()` | confirmed (decomp) | Same → `FUN_0089a420`. [`NoEscSpawnZone.lua:10`](../saboteur-luacd/src/Modules/Behavior/Triggers/NoEscSpawnZone.lua) — A0 |

The four sinks `0x0089a3b0 / 0x0089a420 / 0x0089a4e0 / 0x0089a550` are consecutive small methods on one
object — add/remove pairs on a single road/escape manager. *Inferred*, from adjacency and pairing only.

### Traffic (5)

| Binding | Namespaced form | VA | Source | Signature | Confidence | Evidence |
|---|---|---|---|---|---|---|
| `VehicleAddToTraffic` | `Vehicle.AddToTraffic` | `0x0075e990` | — | `(vVehicle:handle\|string [, bA:boolean=false] [, bB:boolean=true]) -> ()`; **args 4–6 are checked and discarded** | confirmed (decomp) | `isHANDLE(1) \|\| isstr(1)` → `FUN_0067c0a0` → vtable `+0x194`; `isbool(2)`/`isbool(3)` with defaults `0`/`1`; then `isnum(4) && isnum(5)` gate a bare `FUN_006f7140(6)` whose result **is never fetched or used**; clears `*(obj+0x1144) & 0xfe`; tail `FUN_0090a820(obj, bB, bA, 0)`. [`Act_1_Escape.lua:197`](../saboteur-luacd/src/Missions/Act_1_Escape.lua) passes 1 arg — A0 |
| `VehicleRemoveFromTraffic` | `Vehicle.RemoveFromTraffic` | `0x0075eb20` | — | `(vVehicle:handle\|string) -> ()` | confirmed (decomp) | Same resolver + `+0x194`; tail `FUN_0090b9e0(obj, 1)`. **No corpus call** — A0 |
| `TrafficEnable` | `Vehicle.EnableTraffic` | `0x0075ebf0` | — | `(bEnable:boolean, [bImmediate:boolean=false]) -> ()` — arg 1 **mandatory** | confirmed (decomp) | `isbool(1)` gates everything; `isbool(2)` optional, default `0`; **idempotence guard** `if (bEnable != DAT_01134fa0)` before `FUN_00905f30`. [`__UtilFunctions.lua:634,641`](../saboteur-luacd/src/Includes/__UtilFunctions.lua) `EnableTraffic(false, true)` / `EnableTraffic(true)` — A0 |
| `IsTrafficEnabled` | `Vehicle.IsTrafficEnabled` | `0x0075ed10` | — | `() -> boolean` — always exactly 1 result | confirmed (decomp) | 0 args; reads `*(DAT_0143ede4 + 0x404)`; pushes **false** unless that value ∈ {0,5,6,7}; `return 1` on both paths. [`Act_1_Mission_2B.lua:124`](../saboteur-luacd/src/Missions/Act_1_Mission_2B.lua) — A0R |
| `TrafficAccidentResponse` | `Vehicle.TrafficAccidentResponse` | `0x0075ed90` | — | `(bEnable:boolean) -> ()` | confirmed (decomp) | Entire body: `isbool(1)` → `DAT_0111c988 = toBOOL(1)`. Writes a global and returns; nothing else. **No corpus call** — A0 |

`IsTrafficEnabled` does **not** read `DAT_01134fa0` — the global `EnableTraffic` writes. It reads a
different word entirely, and its "enabled" set `{0,5,6,7}` is not a boolean. The two are not inverses of
each other; see Open questions.

### Water (5)

| Binding | Namespaced form | VA | Source | Signature | Confidence | Evidence |
|---|---|---|---|---|---|---|
| `SetWaterLevel` | `Render.SetWaterLevel` | `0x0073ea10` | — | `(fLevel:number) -> ()` | confirmed (decomp) | `isnum(1)` → `toFLOAT(1)` → `*(float *)(DAT_0147da00 + 0x90) = v`. One global plane height; no zone, no handle. **No corpus call** — A0 |
| `ResetWaterLevel` | `Render.ResetWaterLevel` | `0x00740010` | — | `() -> ()` | confirmed (disasm) | Entire body, 5 instructions: `mov eax,[0x147da00]; fld dword ptr [eax+0x94]; fstp dword ptr [eax+0x90]; mov eax,1; ret`. **Byte-level proof that `+0x90` is current and `+0x94` is the stored default** — and thus independent confirmation of `SetWaterLevel`'s target. **No corpus call** |
| `RegisterWaterCallback` | `Vehicle.RegisterWaterCallback` | `0x007639d0` | — | `(hVehicle:handle, sCallback:string [, self:table [, tUser:table]]) -> ()` | confirmed (decomp) | `isHANDLE(1) && isstr(2)` mandatory; `FUN_00498440` → `FUN_0046c010` → vtable `+0x194`; callback idiom; tail `thunk_FUN_00623c60`. **No corpus call** — A0 |
| `RegisterWaterLoggedCallback` | `Vehicle.RegisterWaterLoggedCallback` | `0x00763af0` | — | `(hVehicle:handle, sCallback:string [, self:table [, tUser:table]]) -> ()` | confirmed (decomp) | Identical to the above **plus** an extra gate `FUN_0044a090()` after the handle resolve; tail `thunk_FUN_01623460`. [`WRAPPER_Event.lua:34`](../saboteur-luacd/src/Includes/WRAPPER_Event.lua) — A0 |
| `IsRagdollInWater` | `Actor.IsRagdollInWater` | `0x0070c300` | — | `(hActor:handle) -> boolean`; **bad arg → 0 results** | confirmed (disasm) | `call 0x6f71a0` (isHANDLE); fail → `xor eax,eax; pop edi; pop esi; ret` at `0x0070c34a` — `eax` is explicitly zeroed, i.e. 0 results; success → `toHANDLE(1)` → `call 0x67c0a0` → vtable **`+0x174`** → `FUN_004f37c0`. [`Paris_1_Mission_1B.lua:1108`](../saboteur-luacd/src/Missions/Paris_1_Mission_1B.lua) `if Actor.IsRagdollInWater(hNazi) == true then` — A0R |

Note both water **callbacks** live on `Vehicle`, both water **levels** live on `Render`, and the only
water **query** lives on `Actor`. There is no `Water` table. The subsystem is scattered by consumer, not
by concept.

### Ambush (1)

| Binding | Namespaced form | VA | Source | Signature | Confidence | Evidence |
|---|---|---|---|---|---|---|
| `ActivateAmbush` | `Util.ActivateAmbush` | `0x00753c30` | — | `(sConfig:string, bActive:boolean [, fA:number] [, fB:number]) -> ()` — args 1–2 **mandatory** | confirmed (decomp) | `isstr(1) && isbool(2)` `&&`-chained + non-empty; `toBOOL(2)`; opt `isnum(3)` (default `DAT_00f7ac80`), opt `isnum(4)` (default `_DAT_00fa0b58`); tail `thunk_FUN_0042ab87(sConfig, bActive, fA, fB, 1)`. Sole corpus site [`Paris_4_Mission_1.lua:2004`](../saboteur-luacd/src/Missions/Paris_4_Mission_1.lua), and it is **not a direct call** — it is a deferred task-table entry pairing the function reference `Util.ActivateAmbush` with an argument table `{"LaV_FightBackConfig1", true, 70}` (3 args, matching the 2-mandatory + 1-optional shape) — A0 |

The `sConfig` string names a record in the world's **`.ambush`** file — one of the per-world data files
[`world-water.md`](../symbol_map/world-water.md) lists `FUN_009906c0 World_LoadLevelDataFiles` as loading.
*Inferred* — from the file's existence and the argument being a config name, not from a traced read.

### Streaming & residency (11)

The streaming manager singleton is **`DAT_014ab260`**; a second tag registry sits at **`DAT_014aad80`**.

| Binding | Namespaced form | VA | Source | Signature | Confidence | Evidence |
|---|---|---|---|---|---|---|
| `IsBlockLoaded` | `Util.IsBlockLoaded` | `0x00757b00` | — | `(sPath:string) -> boolean`; **bad arg → 0 results** | confirmed (decomp) | `isstr(1)` → `toSTR` → `strdup` → `thunk_FUN_0162efb0`; `thunk_FUN_0043fbc6(r != 0)`; `return 1`. Bad arg: `return 0`. [`Act_3_Mission_2.lua:1450`](../saboteur-luacd/src/Missions/Act_3_Mission_2.lua) `Util.IsBlockLoaded("Missions\Act_3\Mission_2\main.wsd")` — A0R |
| `IsCustomTagLoaded` | `Util.IsCustomTagLoaded` | `0x0074d210` | — | `(sTag:string) -> boolean`; **bad arg → `false`, 1 result** | confirmed (disasm) | `call 0x6f7160`; fail → `push 0; call 0x6f7020; mov eax,1; ret` — pushes **false** rather than returning 0 results; success → `mov edi,[0x14aad80]` … `call 0x9e4040`. Contrast `IsBlockLoaded` directly above — A0R |
| `LoadStaticENTag` | `Util.LoadStaticENTag` | `0x0074d0b0` | — | `(sTag:string [, bForce:boolean=false]) -> ()` | confirmed (decomp) | `isstr(1)`; opt `isbool(2)`; tail `FUN_009ef570`. **142 corpus calls** — [`__UtilFunctions.lua:120`](../saboteur-luacd/src/Includes/__UtilFunctions.lua) — A0 |
| `UnloadStaticENTag` | `Util.UnloadStaticENTag` | `0x0074d150` | — | `(sTag:string [, bA:boolean] [, bB:boolean]) -> ()` — **two** optional flags | confirmed (decomp) | `isstr(1)`; opt `isbool(2)` **and** `isbool(3)`; tail `FUN_009ef5d0(a,b,c,d)`. **199 corpus calls**, but [`__UtilFunctions.lua:126`](../saboteur-luacd/src/Includes/__UtilFunctions.lua) only ever passes 2 — the third flag is **unreached from the shipped corpus** — A0 |
| `LoadDynamicNode` | `Util.LoadDynamicNode` | `0x00757950` | — | `(sNode:string [, sCallback:string [, self:table [, tUser:table]]]) -> ()` | confirmed (decomp) | `gettop()` variadic; `isstr(1)`; opt `isstr(2)` → callback idiom + `istable(4)`; tails `thunk_FUN_0162df80` and `FUN_009eda50(L,1,-1,0)` — A0 |
| `UnloadDynamicNode` | `Util.UnloadDynamicNode` | `0x0074d2c0` | — | `(sNode:string) -> ()` | confirmed (decomp) | `isstr(1)` → `toSTR(1)` → `FUN_009f0cd0` — A0 |
| `ReloadAllWSDData` | `Util.ReloadAllWSDData` | `0x0075b340` | — | `() -> ()` — takes **no** arguments | confirmed (disasm) | Whole body: `mov ecx,[0x14ab260]; call 0x9f57a0; mov ecx,[0x14ab260]; push 0; call 0x9ee800; mov eax,1; ret`. **No corpus call** |
| `DEBUGClearStreamblockChangeListTree` | `Util.DEBUGClearStreamblockChangeListTree` | `0x0075b360` | — | `() -> ()` — takes **no** arguments | confirmed (disasm) | Whole body: `mov ecx,[0x14ab260]; call 0x9f57a0; mov eax,1; ret` — **literally the first half of `ReloadAllWSDData`**. **No corpus call** |
| `SetCinematicStreaming` | `Cin.SetCinematicStreaming` | `0x0071e7f0` | — | `(…) -> ()` — **a no-op stub** | confirmed (disasm) | Whole body: `mov eax,1; ret`. Two instructions. It never reads the `lua_State`, never calls `FUN_006f8470`, never touches a singleton. Yet [`P3FP_FountainSniper.lua:624`](../saboteur-luacd/src/Missions/P3FP_FountainSniper.lua) ships a live `Cin.SetCinematicStreaming(true)` |
| `TrainIsStreamedIn` | `Train.TrainIsStreamedIn` | `0x0061f500` | — | `(vTrain) -> boolean` *(shape only)* | **inferred** | Disasm: `call 0x6f8470` → `call 0x61df50` (a train-specific arg resolver, not a standard `FUN_006f7*`) → `mov ecx,[0x147dbfc]` → `call 0x96ba30` → virtual `+0x18`. Arg typing **not derived** — resolver unread. Deferred to the train family — A0R |
| `TrainRegisterStreamoutCallback` | `Train.TrainRegisterStreamoutCallback` | `0x00623750` | — | *(callback shape presumed; not derived)* | **not derived** | In a decomp gap; not disassembled. tsv gives `A0`/`adapter`/`nresults=1` and thunk `0x00627500` — identity confirmed, body **open**. 6 corpus calls exist. Deferred to the train family |

### Freeplay & WTF bookkeeping (3)

| Binding | Namespaced form | VA | Source | Signature | Confidence | Evidence |
|---|---|---|---|---|---|---|
| `BlockZoneForSave` | `Freeplay.BlockZoneForSave` | `0x0072afd0` | — | `(sZoneCode:string [, bBlock:boolean=true]) -> ()` | confirmed (decomp) | `isstr(1)`; **prefixes the literal `"ARSHUDNames."`** to arg 1 via `FUN_00db4580`/`FUN_00db4a60` before `strdup`; `gettop() > 1` → `isbool(2)` **mandatory if present** (else early `return`), default `1`; tail `FUN_00985b00`. [`Connect_AmbientFP.lua:35`](../saboteur-luacd/src/Missions/Connect_AmbientFP.lua) `Freeplay.BlockZoneForSave("P1S", true)` — A0 |
| `SetNumWTFZones` | `Util.SetNumWTFZones` | `0x00753350` | — | `(nZones:number) -> ()` | confirmed (decomp) | `isnum(1)` → `toINT(1)` → `*(DAT_014aadcc + 0x19c) = n`. **No corpus call**. Already tabled by [doc 12](12-family-suspicion-wtf-alarm.md) — A0 |
| `RecordWTFZoneFlipped` | `Util.RecordWTFZoneFlipped` | `0x0075b3c0` | — | `() -> ()` — takes **no** arguments; increments a counter | confirmed (disasm) — **closes doc 12's `open` row** | Whole body: `mov eax,[0x14aadcc]; add dword ptr [eax+0x1a4], 1; mov eax,1; ret`. **No corpus call** |

---

## How the subsystem actually works

### The `Zone` table is not about zones — it is the WTF wire

Two bindings, and both take a path like `"WtF_Zones\global\FP_LeHavre"`. Of the **50** corpus call sites,
**35** pass a literal under that prefix
([`Act_3_Mission_1_E3.lua:1010`](../saboteur-luacd/src/Missions/Act_3_Mission_1_E3.lua),
[`Connect_P2Papers.lua:28`](../saboteur-luacd/src/Missions/Connect_P2Papers.lua), and so on). The other 15
are WTF-keyed too, but reach the name indirectly — a loop variable, a config field
(`tConfig.WTFZoneHigh`, `self.sWTFZoneName`), or a mission-local path
(`"Missions\paris_3\mission_1\WTF_Changes\ResistanceZones"`, twice). So the prefix is the dominant
convention rather than a rule the engine enforces — but *every* site is Will-to-Fight business, and the
`Zone` table exists solely to move a *region* between Will-to-Fight states. [Doc 12](12-family-suspicion-wtf-alarm.md)
owns what those states mean; this family owns the transport.

Both enums are pinned in the corpus, in [`Modules/__MagicNumbers.lua:175-184`](../saboteur-luacd/src/Modules/__MagicNumbers.lua):

| `eState` (arg 2) | Value | | `eEntChange` (arg 3) | Value |
|---|---:|---|---|---:|
| `cZONESTATE_LOWWTF` / `cZONESTATE_LOWCOLOR_LOWTAG` | 0 | | `cENT_IMMEDIATE` | 0 |
| `cZONESTATE_HIGHWTF` / `cZONESTATE_HIGHCOLOR_HIGHTAG` | 1 | | `cENT_DURINGSTREAM` | 1 |
| `cZONESTATE_LOWCOLOR_HIGHTAG` | 2 | | `cENT_NOCHANGE` | 2 |
| `cZONESTATE_HIGHCOLOR_LOWTAG` | 3 | | `cENT_REALLYNOCHANGE` | 3 |

The aliasing is the tell: `LOWWTF` and `LOWCOLOR_LOWTAG` are *the same number*. A "WTF zone" is really two
orthogonal switches — **colour** (the world's visual grade) and **tag** (which EditNode tag set is
resident) — that the designers usually flipped together, and named as one thing when they did. States 2
and 3 are the un-ganged combinations. And `eEntChange` is a **streaming instruction**, not a zone
property: it tells the engine *when* to swap the EditNodes — now, at the next stream boundary, or never.
That is why this family owns both zones and streaming: to the engine they are one operation.

`cENT_REALLYNOCHANGE` existing alongside `cENT_NOCHANGE` is the kind of enum name that only gets written
after a bug.

### Zones are typed, not created

No `Trigger.Create*Zone` binding creates any geometry — six of the eight take **one handle and nothing
else** (the other two, `CreateSuspicionZone` and `CreateRestrictedArea`, take the same handle plus the
flags tabled above). The volume already exists — placed in the editor, streamed in with the world. What
the binding does is **stamp a type byte on it**:

```c
// FUN_004c9110 @0x004c9110 — the shared type setter, __thiscall(volume, type)
if (*(byte *)(volume + 0x102) != type) {   // already this type? do nothing
  ...
  *(char *)(volume + 0x102) = (char)type;  // <-- the whole point
  ...
}
```

The six single-handle creators are identical but for their tail call, and *every* creator's tail call —
including the two flag-taking ones — is a thin wrapper around `FUN_004c9110(n)`. Better still, the setter's
own caller list enumerates those wrappers exhaustively (eleven of them), so the enum comes out whole rather
than one binding at a time — a table nobody could have guessed:

| Value | Zone type | Setter | Reached from Lua? |
|---:|---|---|---|
| 0 | *(plain / untyped)* | `FUN_004c9af0` | no — engine-internal only |
| 1 | Cafe | `FUN_016152a0` | `Trigger.CreateCafe` (`0x0074ae90`), via `FUN_00809300` |
| 2 | RestrictedArea | `FUN_01615090` | `Trigger.CreateRestrictedArea` |
| 3 | RedZone | `FUN_004c9b70` | `Trigger.CreateRedZone` |
| 4 | SuspicionZone | `FUN_016151b0` | `Trigger.CreateSuspicionZone` |
| 5 | HostileZone | `FUN_004c9bd0` | `Trigger.CreateHostileZone` |
| 6 | **open** | *(no setter found)* | — |
| 7 | NoFlyZone | `FUN_004c9c20` | `Trigger.CreateNoFlyZone` |
| 8 | WorldBorderZone | `FUN_004c9c50` | `Trigger.CreateWorldBorderZone` |
| 9 | **open** | `FUN_004c9c70` | no — zero callers; sets `+0x11c \|= 0x10`, same as WorldBorder |
| 10 | DeleteZone | `FUN_004ca1c0` | `Trigger.CreateDeleteZone` |
| 11 | FightBackZone | `FUN_004c9c00` | `Trigger.CreateFightBackZone` |

Types 6 and 9 are **holes in a live enum** — 9 has a complete, correct implementation with zero callers
(`FUN_004c9c70` is byte-identical to WorldBorder's `FUN_004c9c50` but for the constant), and 6 has no
setter at all: `FUN_004c9110`'s eleven callers cover every value *except* 6.

Type 1 is **not** a hole, and the caller list is what proves it. `FUN_016152a0` reports `callers=[]`, but
so do `FUN_01615090` (RestrictedArea) and `FUN_016151b0` (SuspicionZone), which are plainly Lua-reached:
in the `0x016xxxxx` overlay region Ghidra's caller lists are empty as a rule, so "zero callers" carries no
weight *there*. Following the thunk instead, `thunk_FUN_016152a0()` is called from `FUN_00809300`, whose
caller is `FUN_0074ae90` = `Trigger.CreateCafe` — and
[`CafeRegion.lua:4`](../saboteur-luacd/src/Modules/Behavior/Triggers/CafeRegion.lua) ships
`Trigger.CreateCafe(a_hTrigger)` in the same one-line behaviour-module pattern as `RedZone.lua`. Type 9's
zero-callers claim survives precisely because `FUN_004c9c70` sits at `0x004c…`, where its sibling setters
*do* carry populated caller lists, and no `thunk_FUN_004c9c70` exists anywhere in the decomp.

This design explains the corpus's shape. Every zone type is a five-line behaviour module that does
nothing but re-type the volume it is attached to:

```lua
-- Modules/Behavior/Triggers/RedZone.lua:6
function RedZone:OnEnter(a_hController)
  Trigger.CreateRedZone(a_hController)
end
```

A level designer places a box, attaches `RedZone.lua`, and on load Lua stamps the type. The zone system is
**entirely data-driven through a one-line script indirection** — which is also why
`RestrictedArea2/3/4/5.lua` exist as four separate files that differ only in an integer literal.

### Two bits, cross-confirmed

`Trigger.CreateSuspicionZone` and `Trigger.CreateRestrictedArea` both end by toggling two bits at
`+0x11d`, and the meaning is not guesswork — two *other* bindings in the `Trigger` table set exactly one
bit each:

| Bit | Set by | Set by (as an arg) |
|---|---|---|
| `+0x11d & 0x04` | `Trigger.SetAllowOnFoot` (`0x0074b730`) | `CreateSuspicionZone` arg 2, `CreateRestrictedArea` arg 3 |
| `+0x11d & 0x02` | `Trigger.SetAllowInVeh` (`0x0074b630`) | `CreateSuspicionZone` arg 3, `CreateRestrictedArea` arg 4 |

And the corpus lines up perfectly on both sides:
[`SuspicionZonePed.lua:6`](../saboteur-luacd/src/Modules/Behavior/Triggers/SuspicionZonePed.lua) → `(h, true, false)`;
[`SuspicionZoneVeh.lua:6`](../saboteur-luacd/src/Modules/Behavior/Triggers/SuspicionZoneVeh.lua) → `(h, false, true)`;
[`RestrictedAreaPed.lua:6`](../saboteur-luacd/src/Modules/Behavior/Triggers/RestrictedAreaPed.lua) → `(h, 0, true, false)`;
[`RestrictedAreaVeh.lua:6`](../saboteur-luacd/src/Modules/Behavior/Triggers/RestrictedAreaVeh.lua) → `(h, 0, false, true)`.

The bit assignment is **confirmed**. The English gloss — that these mean "this volume applies to actors
on foot / in vehicles" rather than "permits" — is **inferred**, from the `Ped`/`Veh` script naming. The
C++ name says *Allow*; the usage reads as *applies to*. Both readings fit the bytes.

### Interiors migrated out of Lua, and the corpus says so out loud

This family contains two generations of interior API sitting side by side, and the corpus contains a
developer's note explaining why:

> `Util.Assert(false, "CFRENCH InteriorManager.UnloadInteriorNode::INTERIOR NODES ARE NOW HANDLED BY WSINTERIORMANAGER , please don't use this function anymore")`
> — [`Managers/InteriorManager.lua:858`](../saboteur-luacd/src/Managers/InteriorManager.lua)

The **old** API is node-based and pushes paths at the streamer: `Util.SpawnInterior` / `Util.UnloadInterior`
set the global `DAT_0143d044` to 1/0 and hand a `.wsd`-ish node name to the generic node loader. Note
`SpawnInterior` calls `_strrchr(path, '\\')` and keeps only the basename — a script may pass a full path
and the engine will quietly discard the directory.

The **new** API is manager-based and speaks in interior *names*: `Util.AddInterior` registers a table,
then `EnterInterior` / `ExitInterior` / `IsInteriorEnabled` / `GetPlayersInterior` all route through
`DAT_014a9d10` = `WSInteriorManager`. The getters never traffic in handles at all — they take and return
**strings**, into fixed `0x80`-byte buffers. That is the [handle rationale](02-marshalling-abi.md) made
concrete: interiors outlive a session, so they are named, never handled.

This directly closes a gap that [`world-water.md`](../symbol_map/world-water.md) leaves open — it lists
"Lua binding native handlers (`SetWaterLevel`, `IsRagdollInWater`, `SpawnObject`, `EnterInterior`,
`SetInteriorFloorData`, …) are unresolved … the registration table was not located." The four named there
that belong to this family are now pinned — `SetWaterLevel` `0x0073ea10`, `IsRagdollInWater` `0x0070c300`,
`EnterInterior` `0x00751260`, `SetInteriorFloorData` `0x00751550` (the fifth, `SpawnObject`, is another
family's). The
corroboration runs both ways: `world-water.md` independently placed `WSInteriorManager` at `FUN_009ca1a0`
with `FUN_009cb240`, and **every** interior binding I read lands a call in `0x009c9000`–`0x009cb000`.

### Water is one float

The entire scriptable water surface is a global plane height:

```
SetWaterLevel:    *(float *)(DAT_0147da00 + 0x90) = arg1
ResetWaterLevel:   fld [eax+0x94]; fstp [eax+0x90]     ; default -> current
```

There is no per-zone water, no volume, no handle. `+0x94` holds the level loaded from data; `+0x90` is
what the renderer reads; `Reset` copies one to the other. Five instructions total. Given
[`world-water.md`](../symbol_map/world-water.md)'s account of the real water stack — `.waterctrl`
(`"WC07"`), `.waterflow`, `water_planes.ini` with up to 30 six-vertex quads — the scripting surface is a
**rounding error** on the subsystem. Lua can move one number; everything else is data.

And no shipped script moves it. `SetWaterLevel`, `ResetWaterLevel` and `RegisterWaterCallback` have **zero
corpus calls**. The one water binding scripts actually use, `RegisterWaterLoggedCallback`, is reached only
through [`WRAPPER_Event.lua:34`](../saboteur-luacd/src/Includes/WRAPPER_Event.lua) — where it is wired to
`EVENT_ActorDeath`, so that a vehicle "dying" and a vehicle drowning are the same event to a mission
script. That is the only water gameplay in the game's Lua.

### Traffic is a global with two different truths

`Vehicle.EnableTraffic` guards itself against redundant work (`if (bEnable != DAT_01134fa0)`) and is by
far the most-used binding in the family — ~30 mission scripts bracket their setpieces with
`EnableTraffic(false, true)` … `EnableTraffic(true)`. `Vehicle.TrafficAccidentResponse` is the family's
purest specimen of its genre: `isbool(1)`, write `DAT_0111c988`, return. Nothing reads it in this
function; the entire binding is a global poke.

But `Vehicle.IsTrafficEnabled` **does not read the global `EnableTraffic` writes**. It reads
`*(DAT_0143ede4 + 0x404)` and reports enabled only when that value is one of `{0, 5, 6, 7}`. So
`EnableTraffic(false)` followed by `IsTrafficEnabled()` is not guaranteed to return `false` — they are
looking at different state. [`Act_1_Mission_2B.lua:124`](../saboteur-luacd/src/Missions/Act_1_Mission_2B.lua)
relies on the query to decide whether to re-enable. Whether that ever misbehaves is **open** (§ below).

### A dead parameter, preserved

`Vehicle.AddToTraffic` type-checks arguments 4, 5 and 6 as numbers — and then never fetches them:

```c
cVar1 = FUN_006f7140(4);
if ((cVar1 != '\0') && (cVar1 = FUN_006f7140(5), cVar1 != '\0')) {
  FUN_006f7140(6);        // result assigned to nothing
}
                          // no FUN_006f7950 anywhere; the floats are never read
```

A three-float position argument that was removed from the engine call but whose *validation* survived. It
is inert — passing it or omitting it changes nothing — but it is a fossil of an earlier
`AddToTraffic(hVeh, bA, bB, x, y, z)`. Every corpus call passes one argument.

### The failure shape is not consistent, and callers can't tell

The ABI says bad arguments produce a silent `return 0`. This family shows that is a *tendency*, not a
rule. Three getters, three different answers to "you passed me garbage":

| Binding | Bad argument yields |
|---|---|
| `Util.IsInteriorEnabled` | **0 results** → the Lua expression is `nil` |
| `Util.IsBlockLoaded` | **0 results** → `nil` |
| `Util.IsCustomTagLoaded` | **`false`, 1 result** |

`if Util.IsBlockLoaded(x)` and `if Util.IsCustomTagLoaded(x)` behave identically in Lua (both falsy), which
is presumably why nobody noticed. But `local b = Util.IsBlockLoaded(x)` stores `nil` where the sibling
stores `false`, and no caller can distinguish "not loaded" from "you passed me a number".

---

## Open questions

1. **`Cin.SetCinematicStreaming` is a no-op that ships being called.** The body is `mov eax,1; ret` at
   `0x0071e7f0` — two instructions, no `lua_State` access — while
   [`P3FP_FountainSniper.lua:624`](../saboteur-luacd/src/Missions/P3FP_FountainSniper.lua) calls
   `Cin.SetCinematicStreaming(true)` in shipped mission code. Was the feature cut and the binding stubbed
   rather than unregistered (leaving the call harmless), or did the stub land by accident and a
   fountain-sniper streaming hitch ship with it? The registration is real; only the body is empty.
2. **`Vehicle.IsTrafficEnabled` reads different state from `Vehicle.EnableTraffic`.** `EnableTraffic`
   caches to `DAT_01134fa0`; `IsTrafficEnabled` tests `*(DAT_0143ede4 + 0x404) ∈ {0,5,6,7}`. What is that
   enum? `{0,5,6,7}` is not a boolean and not obviously a traffic state — it smells like a game-mode or
   level-state value that *implies* traffic. Until it is identified, `IsTrafficEnabled`'s exact contract
   is unproven, and so is whether
   [`Act_1_Mission_2B.lua:124-125`](../saboteur-luacd/src/Missions/Act_1_Mission_2B.lua) is correct.
3. **`Util.ReleaseInterior` does not exist.**
   [`__UtilFunctions.lua:114`](../saboteur-luacd/src/Includes/__UtilFunctions.lua) calls it; it is absent
   from all 898 rows of `lua_bindings.txt` and the tsv. The wrapper
   `__UtilFunctions.ReleaseInteriorNode` has **zero callers**, so the latent `attempt to call field
   'ReleaseInterior' (a nil value)` never fires — but it is a third-generation interior API that was
   designed, wrapped in Lua, and never built. What was it for? (Same pathology doc 12 found in
   `Experimental/`.)
4. **Zone types 6 and 9 are unclaimed.** 9 has a complete implementation (`FUN_004c9c70`, sets
   `+0x11c |= 0x10` exactly like WorldBorder) and zero callers — and unlike the `0x016xxxxx` setters, its
   empty caller list is trustworthy. 6 has no setter at all. Cut zone types, or engine-internal states set
   through a path I did not trace? *(Type 1 was listed here as cut on a "zero callers" reading that does
   not hold in the overlay region; it is `Cafe`, reached via `Trigger.CreateCafe` — see* Zones are typed,
   not created*.)*
5. **`Freeplay.BlockZoneForSave` prefixes `"ARSHUDNames."` to its argument.** So `BlockZoneForSave("P1S")`
   keys on `"ARSHUDNames.P1S"`. `ARSHUDNames` is a HUD/localisation namespace; why a *save-blocking* call
   keys through it is unexplained. Are freeplay zone codes (`P1S`, `CB`, `CF`, `PR`, `OS`) simply
   HUD-string ids reused as zone ids?
6. ~~**`Util.AddInterior`'s sixth table field is unnamed.**~~ **Closed.** The field is **`bHQ`** (flag bit
   `0x08`). `0x00fe5b9c` holds the bytes `62 48 51 00` = `"bHQ"`; Ghidra emitted `PTR_LAB_00fe5b9c` only
   because those four bytes also read as a plausible pointer. Confirmed independently by the corpus, which
   both writes and reads `bHQ` in `InteriorManager.lua`'s per-interior tables. See the schema table above.
7. **`Util.SetInteriorFloorData`'s five floats are unnamed.** `(sInterior, nFloor, f1..f5 [, f6])` — the
   arity and types are certain; what the floats *are* (a bounding box? a height range plus fade
   distances?) is not. `InteriorManager.lua`'s per-interior tables are the place to look.
8. **`Trigger.SetAllowOnFoot` / `SetAllowInVeh`: permit or apply?** The bits are confirmed; the semantics
   are inferred from script naming, and the C++ symbol ("Allow") and the usage ("Ped"/"Veh" variants) pull
   in different directions. Reading `FUN_004c8e10` or the volume's per-frame test would settle it.
9. **The four road/escape sinks are unidentified.** `0x0089a3b0` / `0x0089a420` / `0x0089a4e0` /
   `0x0089a550` are consecutive methods reached via `FUN_00a0ffa0`, a resolver distinct from the zone
   set's `FUN_00a0fff0`. Which manager owns them, and why roadblocks and no-escape-spawn share it, is
   untraced.
10. **`Train.TrainRegisterStreamoutCallback` (`0x00623750`) has no derived body**, and
    `Train.TrainIsStreamedIn`'s argument type is unknown — it resolves through a train-specific
    `0x0061df50` rather than a standard `FUN_006f7*` primitive. Both belong to the train family; flagged
    here so the omission is not silent.
11. **The decomp export has three contiguous holes**: `0x00754000`–`0x00757900`, `0x007518f8`–
    `0x00751cc0`, `0x0075b83d`–`0x0075c700`. Verified: **zero `==== … @0xVA` headers fall inside any of
    the three**. ⚠️ *(corrected 2026-07-24: an earlier revision of this note claimed the export was
    "missing ~1,100 functions". That number was bogus — it came from `36,935` total headers minus
    `35,819` `FUN_`-prefixed headers. Those `1,116` entries are **present**; they simply carry
    demangled or synthetic Ghidra names instead of the `FUN_` prefix — `caseD_8`, `default`,
    `~GAcquireInterface`, `` `default_constructor_closure' ``, `FID_conflict:operator==`,
    `CreateObject`, and so on. Only the three named holes are real gaps.)* The holes are worth fixing
    at the source: 16 of this family's 55 bindings fall in them and 14 needed direct disassembly from
    `Saboteur.exe` to recover, and
    other families are certainly hitting the same holes without noticing — a missing function looks
    exactly like a binding with no body.

---

## New primitives observed

Two push primitives not in the ABI cheat sheet's table, both confirmed by disassembly at multiple sites:

| Primitive | Lowers to | Seen at |
|---|---|---|
| `FUN_006f7020` | `lua_pushboolean(L, n)` | `0x007519ea`, `0x00751a87`, `0x0074d25f` |
| `FUN_006f7080` | `lua_pushstring(L, s)` | `0x0075196d`, `0x00751b47`, `0x00751c36` |

Singletons pinned by this family, for reuse:

| Global | Is | Evidence |
|---|---|---|
| `DAT_014a9d10` | `WSInteriorManager` | 5 × `mov ecx,[0x14a9d10]`; callees all in `0x009c9000`–`0x009cb000` (matches [`world-water.md`](../symbol_map/world-water.md)) |
| `DAT_014ab260` | streaming / streamblock manager | `0x0075b340`, `0x0075b360` |
| `DAT_014aad80` | custom-tag registry | `0x0074d274` |
| `DAT_014aadcc` | WTF zone stats block (`+0x19c` count, `+0x1a4` flipped) | `0x00753350`, `0x0075b3c0` (see [doc 12](12-family-suspicion-wtf-alarm.md)) |
| `DAT_0147da00` | water plane state (`+0x90` current, `+0x94` default) | `0x0073ea10`, `0x00740010` |
| `DAT_01134fa0` | traffic-enabled cache | `0x0075ebf0` |
| `DAT_0111c988` | traffic accident response flag | `0x0075ed90` |
| `DAT_0143d044` | interior-node spawn/unload flag | `0x0074cd30`, `0x0074ce70` |

---

## The `Damage` table — prop damage states

> **Scope.** This section post-dates the adversarial pass recorded at the top of this file; the
> `> **Verified:**` line there covers the original 55 rows only. Every claim below was re-derived from
> retail `Saboteur.exe` with pefile + capstone (image base `0x400000`) using
> [doc 19](19-family-ui-hud-tutorial.md)'s method, because **one of the two bodies is absent from the
> Ghidra export entirely** — a fresh instance of [open question 11](#open-questions).
>
> **Verified (this section only):** both `impl_va`s re-checked against the tsv `Damage` partition (2 rows,
> re-run) and both bodies re-disassembled from retail `Saboteur.exe` — they reproduce
> instruction-for-instruction, including the `0x00727f38` `movzx esi,al` **retail bug**, which the Ghidra
> body of `FUN_00727e80` independently corroborates (`uVar2 = FUN_006f7140(3)` feeding the `+0x54` vcall).
> `FUN_00dc1e20` re-derived as `pandemic_hash` from the bytes; the `+0xa8`/`+0x18`/`+0x20` record layout and
> the `-1` miss sentinel reproduce; the RTTI names, single-slot vtables (`0x00fe09c0`/`0x00fe09c8`), stubs,
> thunk, adapter and both name-slot stanzas (`0x007280c3`, `0x00728117`) all reproduce exactly. All 6 corpus
> call sites and the 0-caller and 0-bare-reference counts re-grepped and correct; the decomp hole
> (`FUN_00727e80` → `FUN_00728230`) reproduces; `lua_scripts.md:54`, `community_tooling.md:90`,
> `dump_lua_registration.py:11-22` and the 722/176 tsv split all check out. No Mercenaries 2 import.
> **Corrected:** the `Damage` rows match **none** of this doc's inclusion-rule terms — they are adopted
> orphans, not family-16 members, and the "57 of 57" coverage claim that contradicted the rule's own
> "yields 55 bindings" has been reworded; `SetDamageState`'s `-> nState` return was stated as `confirmed`
> in the table while the prose marked it inferred (downgraded in the row); `[component+0x54]` is the
> **vtable** slot, not a field on the component; the assertion-absence claim cited a decomp grep against a
> body the decomp does not contain (re-grounded on the disassembly); the `FUN_0066a320` listing dropped its
> null-record check at `0x0066a337`.

`Damage` is a two-binding table that appeared in no family doc. It lands here because doc 16 owns props and
world objects: both bindings resolve a **world object handle** through `FUN_0067c0a0` — the same
handle/string resolver this family already uses for `Vehicle.AddToTraffic` — and then poke a named
sub-part on it. It cross-links [`docs/symbol_map/damage-physics.md`](../symbol_map/damage-physics.md),
which owns the engine-side destruction stack (`WSDamageable`, `WSExplosion`, the ragdoll paths), and
[03-handle-and-object-model.md](03-handle-and-object-model.md) for the handle itself.

The table is small but live: **6 corpus call sites**, all of `SetDamageState`. It is also the most
defective pair of bindings in this doc — the setter has a **retail argument-marshalling bug** that makes
its third parameter inert.

> **Follow-up 2026-10-12 — [`damage_and_destruction.md`](../damage_and_destruction.md).** What a damage
> group *is* is now known: a GameTemplates `DamageGroups` entry whose parts name bones in the object's
> mesh; the state shows/hides those bones' drawcalls. That answers open question 4 (group names come
> from `GroupName` in the object's template — for 675/856 damage templates it is the Model mesh name,
> vehicles use piece names like `hood`, `wheel_fl`) and narrows 2 (retail code distinguishes only
> states 0 and 1, `FUN_006678e0`). The writers of the state are `FUN_00667d60` and the
> `0x01302230` queue drained by `FUN_00669c20` — whether `vfn[0x54]` feeds that queue is still inferred.

### The bindings

Rows are the complete `Damage` partition of
[`data/lua_registration_map.tsv`](../../data/lua_registration_map.tsv) (`awk -F'\t' '$1=="Damage"'` →
**2 rows**). `A0` = `LuaGlueFunctor0` (adapter hardcodes 1 result); `A0R` = `LuaGlueFunctor0R` (real
result count in `EAX`).

| Binding | VA | Shape | Signature | Confidence | Evidence |
|---|---|---|---|---|---|
| `Damage.SetDamageState` | `0x00727e80` | `adapter` (A0) | `(hObj:handle, sDamageGroup:string, nState:number) -> ()` — all three **mandatory** (any failing check → silent no-op); **`nState`'s value is discarded and the state written is always `1`**. *(The adapter reports 1 result off a body that pushes nothing; what Lua actually receives is **inferred** — see [What the RTTI says](#what-the-rtti-says--and-why-it-is-not-independent-corroboration).)* | confirmed (disasm) — args + bug; return value inferred | `isHANDLE(1)` @`0x727ec2`; `toHANDLE(1)` @`0x727ed3` → `call 0x67c0a0` @`0x727edd` → entity (`0` → bail `0x727f69`); vcall `[+0x1fc]` @`0x727eff` → damage component (`0` → bail); `isstr(2)` @`0x727f09`; `isnum(3)` @`0x727f16`; `toSTR(2)` @`0x727f25` → `call 0xdb7e10(s,1)` @`0x727f2f` → hash (the `1` is the `push 1` @`0x727f1f`, surviving `0x6f7a80`'s `ret 4`); **`call 0x6f7140(3)` @`0x727f38` + `movzx esi,al` @`0x727f3d`** — the *check*, not the fetch; tail vcall `component->vfn[0x54](hash, esi)` @`0x727f67` (`mov edx,[eax]`; `mov eax,[edx+0x54]`). No push → `ret` @`0x727f6c`. Adapter `0x00728180`: `call 0x727e80`; `mov eax,1; ret` |
| `Damage.GetDamageState` | `0x00727f70` | `jmp` (A0R) | `(hObj:handle, sDamageGroup:string) -> nState:number \| nil` — 2 args, **no third**; unknown group or bad arg → **0 results** | confirmed (disasm) | **Absent from the decomp export**; read from the exe. `isHANDLE(1)` @`0x727fb2`; `toHANDLE(1)` @`0x727fc6` → `call 0x67c0a0` @`0x727fcd`; vcall `[+0x1fc]` @`0x727fef`; `isstr(2)` @`0x727ff9`; `toSTR(2)` @`0x728008` → `call 0xdb7e10(s,1)` @`0x728012`; vcall `[+0x1fc]` @`0x728034` → `call 0x66a320` @`0x728038`; `cmp eax,-1` @`0x72803d` → `je 0x728053` (`xor eax,eax; ret` = 0 results); else `call 0x6f7040` (pushnumber) @`0x728045`, `mov eax,1; ret`. Thunk `0x007281a0`: `jmp 0x727f70` |

**2 of 2 located, 2 confirmed, 0 inferred, 0 not found.** Neither carries an EALA assertion string — and
because `GetDamageState` is *absent from the decomp*, a decomp grep cannot speak to it: the claim rests on
the disassembly instead. Neither body (`0x727e80`–`0x727f6c`, `0x727f70`–`0x728058`) references a single
`.rdata` string; the only immediate either pushes is `0x15c90`, an allocation size. So the `Source` column
other tables in this doc carry would be `—` for both; it is omitted rather than filled with guesses.

### The string is a hashed sub-part name, not an asset path

The brief question for this table was what the middle argument names. It is **not** a damage-model or
prop-template asset. Both bindings do the same thing with it, byte for byte:

```
toSTR(2)                       ; const char*, pointing into a live Lua TString
call 0xdb7e10(ptr, 1)          ; FUN_00db7e10 -> FUN_00db7c10 -> FUN_00dc1e20
```

`FUN_00dc1e20` **is `pandemic_hash`** — I re-derived it rather than importing the claim: at `0x00dc1e20`
it is FNV-1a with basis `0x811c9dc5` and prime `0x1000193`, a per-character `or ecx,0x20` case-fold, and
the `xor eax,0x2a; imul eax,prime` finaliser. That is the exact function transcribed at
[`docs/formats/lua_scripts.md:54`](../formats/lua_scripts.md). So **the damage-group string is hashed, and
the hash is case-folded — the argument is case-insensitive.** The `1` in `db7c10(str, 1)` additionally
registers the string in the reverse-lookup table at `0x014e1cbc` (a debug hash→string registry, torn down
via `atexit`); it does not affect the hash.

This corrects a reading worth flagging for other docs: [doc 02 §8](02-marshalling-abi.md) describes the
`toSTR` → `FUN_00db7e10` pairing as "copies it out of Lua's ownership before GC". That is the *effect*,
but the mechanism is not a `strdup` — `db7e10` stores a **4-byte hash**, not a string. The GC-safety is
incidental. (This is also the one-indirection-deeper `pandemic_hash` reach that
[README open question 6](README.md#what-is-still-open) describes for `GetHandleByName`, showing up in a
second, unrelated place.)

What the hash keys into is a **linked list on the damage component**, `FUN_0066a320`:

```
0066a320  mov ecx,[ecx+0xa8]      ; head of the damage-group list
0066a326  mov edx,[esp+4]         ; the hash
0066a330  test ecx,ecx / je fail
0066a334  mov eax,[ecx+8]         ; the group record
0066a337  test eax,eax / je fail  ; null record -> miss
0066a33b  cmp [eax+0x18],edx      ; record.key == hash ?
0066a33e  je  hit
0066a340  mov ecx,[ecx]           ; next
0066a342  jmp loop
hit:      mov eax,[eax+0x20] / ret 4    ; record.state
fail:     or  eax,0xffffffff / ret 4    ; -1
```

So a damageable object carries a list of named groups at `+0xa8`; each record holds a `pandemic_hash` key
at `+0x18` and an **integer state at `+0x20`**. Lookup is a linear scan, and a miss returns `-1` — which
is exactly the sentinel `GetDamageState` tests at `0x72803d`. The component itself is reached by a virtual
call at entity vtable `+0x1fc`, and both bindings bail if it is null: **objects with no damage component
silently do nothing.**

The corpus confirms the "named sub-part" reading and rules out "asset path". In
[`Act_1_Farm.lua:1125-1127`](../saboteur-luacd/src/Missions/Act_1_Farm.lua) the *handle* is fetched by the
full world path `...\burningbarn_props\Platform_Drop\Canal_Scaffold_Med(9)` while the *string* is the bare
leaf `"Canal_Scaffold_Med"` — the path and the instance suffix `(9)` both dropped.
[`Paris_1_Mission_6.lua:1231`](../saboteur-luacd/src/Missions/Paris_1_Mission_6.lua) passes the leaf
`"MN_LaVillette_Broken_BackWall01A"` against a handle whose path ends in that same leaf. And the scripts
name the parameter themselves: [`Act_1_Race.lua:690`](../saboteur-luacd/src/Missions/Act_1_Race.lua)
declares `BreakShit(a_hCar, a_sDamageGroup)`, and
[`Paris_1_Mission_6.lua:874`](../saboteur-luacd/src/Missions/Paris_1_Mission_6.lua) calls it `sPart`.

So: **`sDamageGroup` is an intra-object group name, hashed.** For single-part props it coincides with the
source mesh's leaf name, which is why the farm scaffolding passes what looks like a template name; for a
vehicle it is one of several damage groups on one object. *(Confirmed: the hash path and the list
structure. Inferred: that the group name is authored on the model and equals the mesh leaf name — that
follows from corpus naming and the lookup shape, not from reading the exporter or a shipped model.)*

### The setter is broken in retail: `nState` is discarded

`SetDamageState` validates argument 3 as a number and then **never fetches it**. At `0x00727f16` it calls
`FUN_006f7140(3)` = `lua_isnumber` as its guard — correct. But at `0x00727f38`, where the value should be
read, it calls **`FUN_006f7140(3)` a second time** and takes the result as the value:

```
00727f12  push 3 / mov ecx,esi / call 0x6f7140   ; isnum(3)  -- the guard
00727f1b  test al,al / je 0x727f69               ; not a number -> bail
...
00727f34  push 3 / mov ecx,esi / call 0x6f7140   ; isnum(3)  -- AGAIN, as the value
00727f3d  movzx esi,al                           ; esi = 0 or 1
...
00727f5a  push esi                               ; passed as the state
00727f67  call eax                               ; component->vfn[0x54](hash, esi)
```

The fetch the author meant is `FUN_006f7990` (`GetInt`) at `0x006f7990`, which — I read it to be sure — is
the isnumber-guarded `luaL_checkinteger` wrapper; `SetDamageState` never calls it. `FUN_006f7140`
normalises its result to exactly `0` or `1` (`neg eax; sbb eax,eax; neg eax` at `0x006f7150`), and the
guard at `0x727f1b` has *already* proven it non-zero on every path that reaches `0x727f38`. Therefore:

> **`Damage.SetDamageState` can only ever write state `1`.** The third argument's only effect is to be a
> number; `0`, `2`, `7` and `1` are indistinguishable, and state `0` is unreachable from Lua.

This is a genuine retail defect, not a decompiler artifact — I read it in the bytes, and Ghidra's export
renders the same thing (`uVar2 = FUN_006f7140(3)` feeding the tail vcall), which reads like a
decompilation error but is what shipped. It is the same idiom this doc already flagged in
`Vehicle.AddToTraffic` (`0x0075e990`), where "a bare `FUN_006f7140(6)` whose result is never fetched or
used" appears — there the stray check is dead; here it is *load-bearing*, which is worse. A
check-primitive standing in for a fetch-primitive looks like a copy/paste family this binding layer is
prone to, and it is worth a targeted sweep (see Q1 below).

**The bug is invisible in shipped content**, which is presumably why it survived: of the 6 call sites, the
5 concrete ones all pass the literal `1` already —
[`Act_1_Farm.lua:1127`](../saboteur-luacd/src/Missions/Act_1_Farm.lua),
[`:1131`](../saboteur-luacd/src/Missions/Act_1_Farm.lua),
[`Act_1_Race.lua:691`](../saboteur-luacd/src/Missions/Act_1_Race.lua),
[`Paris_1_Mission_6.lua:875`](../saboteur-luacd/src/Missions/Paris_1_Mission_6.lua),
[`:1231`](../saboteur-luacd/src/Missions/Paris_1_Mission_6.lua). The 6th,
[`DestructionSequence.lua:179`](../saboteur-luacd/src/Modules/Libraries/DestructionSequence.lua), forwards
`tCommandParameters[3]` from a `"SETDAMAGESTATE"` sequence command — and **no shipped sequence uses that
command**: grepping `SETDAMAGESTATE` across the corpus returns only the dispatcher's own `elseif` at line
178. So the only site that *could* have passed a value other than `1` is never exercised. All 6 counts
here are direct calls; there are no bare `Damage` table references in the corpus.

That call site is a useful corroboration of [doc 03](03-handle-and-object-model.md) in passing:
`DestructionSequence.CheckForHandle` (`:250-259`) branches on `type(a_vVariable) == "userdata"` — the Lua
corpus itself confirming that handles cross the seam as light userdata.

### The getter and setter disagree about the state domain

The pair is asymmetric in a way that is more interesting than either binding alone. `GetDamageState`
returns `[record+0x20]` **unclamped** — a full 32-bit integer, with `-1` reserved for "no such group",
pushed to Lua via `FUN_006f7040`. `SetDamageState` can write only `1`. So the engine's damage-group state
is a real multi-valued field that the Lua setter cannot address.

That asymmetry says the states are **engine-authored, script-triggered**: whatever populates `+0x20` with
values other than `1` is not the Lua seam. The destruction stack in
[`damage-physics.md`](../symbol_map/damage-physics.md) — `WSDamageable::ApplyDamage` (`FUN_00666b20`), the
health-subtraction core, driving destructibles — is the plausible writer, and note `FUN_0066a320` sits in
the same `0x0066xxxx` neighbourhood as `FUN_00666b20`. `Damage.SetDamageState` then reads as a **scripted
override**: a mission forcing a specific prop to its "broken" appearance on cue, in lockstep with a
cinematic. Every shipped call site fits that exactly — `Act_1_Farm:Explosion5` fires two of them
immediately after `Cin.PlayCinematic("A1M5_FarmDestruction_05", false)`
([`:1124-1131`](../saboteur-luacd/src/Missions/Act_1_Farm.lua)), and `Paris_1_Mission_6:BlowHole` is named
for it. The binding exists to say "this prop is now wrecked", not to select among wreck states — which is
very likely why nobody noticed the third argument never worked.

*(Inferred, and flagged as such: that `WSDamageable` is the writer of non-`1` states is a
neighbourhood-and-role argument, not a traced call chain. I did not find the store to `[record+0x20]`.)*

### What the RTTI says — and why it is not independent corroboration

Both rows' identity chains all the way from a **string in the binary** to `impl_va`, which is worth
recording because this doc's other 55 rows rest on the tsv alone. The MSVC RTTI type descriptors for the
two functor instantiations are decorated names that embed the C++ symbol *and its signature*:

| Vtable | Type descriptor | Decorated name |
|---|---|---|
| `0x00fe09c0` | `0x0112a500` | `.?AV?$LuaGlueFunctor0@$1?SetDamageState@@YAXPAUlua_State@@@Z@@` |
| `0x00fe09c8` | `0x0112a548` | `.?AV?$LuaGlueFunctor0R@H$1?GetDamageState@@YAHPAUlua_State@@@Z@@` |

Demangled: `LuaGlueFunctor0<&SetDamageState>` where `SetDamageState` is `void __cdecl (lua_State *)`
(`YAX…` — `X` = void), and `LuaGlueFunctor0R<int, &GetDamageState>` where `GetDamageState` is
`int __cdecl (lua_State *)` (`YAH…` — `H` = int). Each functor vtable is **a single slot** (`0x00fe09c0`
+ 8 = `0x00fe09c8`, and `[0x00fe09c4]` is the *next* COL, not a second method), and that slot is a getter
returning the `lua_CFunction`: `0x007281b0` = `mov eax,0x728180; ret`, `0x007281c0` =
`mov eax,0x7281a0; ret`. Following those gives `0x728180` (adapter → `call 0x727e80`) and `0x7281a0`
(`jmp 0x727f70`) — closing the loop onto both `impl_va`s. The Lua-visible names are separate literals,
stored into the template's static name slots by the registration stanzas: `0x007280c3` writes `0x00fe0934`
(`"SetDamageState"`) into `0x0142dbc0`, and `0x00728117` writes `0x00fe0954` (`"GetDamageState"`) into
`0x0142dbc4`, each followed by `call 0x6f6660` — the registrar of [doc 01](01-registration-and-dispatch.md).

**This is not an independent check of the registration map, and should not be counted as one.**
[`tools/dump_lua_registration.py:11-22`](../../tools/dump_lua_registration.py) already parses exactly this
structure — RTTI descriptor → COL → vtable → stub → thunk → impl, plus the stanza's name-slot store. My
reading is the dumper's own method executed by hand, so it confirms the **parser's output on these two
rows** and nothing more. It does not touch [README open question 1](README.md#what-is-still-open), which
asks for a *live* confirmation from a different mechanism (an x32dbg breakpoint on `0x006f6660`); that
remains the right test and is still open. What the exercise does establish is narrower and still useful:
the tsv's `family` column is the RTTI template class (all 898 rows carry a `vtable_va`; 722
`LuaGlueFunctor0`, 176 `LuaGlueFunctor0R`), and per-row spot-checks of the map are cheap — this one cost
about ten minutes and reproduced exactly.

It also sharpens [README open question 4](README.md#what-is-still-open), the "`mov eax,1` consequence".
`SetDamageState`'s implementation is declared **`void`** — that is now a byte-level fact from the RTTI
name (`YAX`), not an inference from a body that happens not to push — so it pushes nothing, while its
adapter at `0x00728180` unconditionally reports `1` result. Under stock Lua 5.1 `luaD_poscall` takes the
top `n` stack values as results, so with nothing pushed the value handed back is whatever sits at the
stack top: **argument 3**. That predicts `Damage.SetDamageState(h, "g", 7) == 7`. No corpus site uses the
return value, so nothing depends on it — but this is a clean, cheap test case for Q4, and the `void` half
of it is settled. *(The Lua-semantics half is **inferred**: I did not trace this build's `luaD_poscall`.)*

### Open questions

1. **How many other bindings substitute a check primitive for a fetch primitive?** `SetDamageState`
   (`0x00727f38`) passes `lua_isnumber`'s boolean where `FUN_006f7990`'s integer belongs, and this doc
   already found a stray `FUN_006f7140(6)` in `Vehicle.AddToTraffic` (`0x0075e990`). A mechanical sweep is
   well-defined and cheap: flag every binding where a `FUN_006f71x0`/`FUN_006f714x` result reaches a
   non-branch use (`movzx`/`push`) rather than a `test`/`je`. This is the highest-value item here — it is
   a *class* of retail bug, and every doc in this catalog derived signatures by reading these primitives
   as guards.
2. **What is the real damage-state domain?** `GetDamageState` returns an unclamped int from
   `[record+0x20]` with `-1` = absent. What values actually occur — is it `{0,1}` with the setter merely
   redundant, or a graded `0..N` wreck ladder? Nothing in the corpus reads it back (0 call sites), so the
   answer is not in the scripts. A breakpoint on `0x0066a344` (the hit path) while shelling a car would
   answer it in one session, and would also settle whether the setter's bug is cosmetic or material.
3. **Who writes `[record+0x20]` with a value other than `1`?** Not the Lua seam. The store was not
   located. `WSDamageable::ApplyDamage` (`FUN_00666b20`, per
   [`damage-physics.md`](../symbol_map/damage-physics.md)) is the neighbourhood-plausible writer —
   `FUN_0066a320` sits beside it — but the chain is untraced.
4. **Where do damage-group names come from?** The key is `pandemic_hash(name)`, case-folded, so any
   authored group name is recoverable by hashing candidates against the 733k-entry rainbow table noted in
   [`community_tooling.md:90`](../community_tooling.md). Dumping the `+0xa8` list for a known prop and
   reversing the hashes would produce the **first enumeration of shipped damage-group names** — and would
   directly test the inference that they equal the mesh leaf name. This is the tractable modder-facing win
   in this table.
5. **What owns the damage component at entity vtable `+0x1fc`?** Both bindings call it and bail on null,
   so it is the gate on whether an object is damageable at all — but the class was not identified. It
   should be nameable from RTTI, the same way the functors above were.
6. **`Damage.GetDamageState` has zero callers in 321 shipped scripts.** It is fully implemented, correctly
   marshalled, and returns a real value — the only member of this table that *works*. Was it for a
   destruction-state save/restore path that never shipped? This is the same pathology as
   `Util.ReleaseInterior` (Q3 of the original open questions above) inverted: there, script calls a
   binding that does not exist; here, a working binding no script calls. It also feeds
   [README open question 9](README.md#what-is-still-open) — the 2008 pre-release build would show whether
   it ever had callers.
7. **The decomp export hole is wider than recorded.** `0x00727f70` (`GetDamageState`) and every thunk,
   adapter and stub in `0x00727f6d`–`0x0072822f` are absent from `saboteur_all_functions_decomp.txt` — the
   export jumps straight from `FUN_00727e80` to `FUN_00728230`. Add this range to the runs listed in Q11
   of the original open questions. It swallowed **half of this two-row table**, and a next agent working
   `Sabotage` or `Sensory` (also undocumented, also in the `0x0072xxxx`–`0x0075xxxx` band) should assume
   the same and reach for `Saboteur.exe` first.

**Confidence: high** for both signatures, the `pandemic_hash` keying, the `+0xa8`/`+0x18`/`+0x20` record
layout, the `-1` sentinel, and the `SetDamageState` state-clamp bug — all read instruction-by-instruction
out of retail `Saboteur.exe` and independently reproduced against the Ghidra export where the export has
the body. **Inferred, and marked so in place:** that damage-group names are authored on the model and
equal the mesh leaf name; that `WSDamageable` writes the non-`1` states; and that the void-impl/`mov eax,1`
mismatch makes `SetDamageState` return its own third argument. **Open:** the state domain itself.
