# The Saboteur — the engine ↔ Lua seam

The Saboteur's gameplay logic is Lua. Missions, checkpoints, freeplay ambients, soldier and vehicle
behaviour modules — all of it is script calling into the WildStar/Odin C++ engine across a seam of **898
bindings in 26 tables**. This catalog reverses that seam end to end: how a Lua call reaches C, how a
handle resolves to a game object, how the engine calls back into script, and what every binding's
signature actually is. Start at [00-seam-overview.md](00-seam-overview.md), which fuses the mechanics
docs and — deliberately — flags where they contradict each other.

The seam is the richest vein in this binary because **both sides survived**. The retail exe holds the C
implementations, and [`docs/saboteur-luacd/src`](../saboteur-luacd/src) holds 321 decompiled Lua sources
that *call* them. Almost nowhere else in a stripped 2009 binary can a proposed signature be checked
against a shipped caller. That is what most of the "inferred" tier below rests on, and it is why the
family docs cite `file:line` as often as they cite a VA.

> **Key enabler — and its limit.** This catalog does **not** rest on assertion strings. The engine symbol
> map ([`docs/symbol_map`](../symbol_map/README.md)) could lean on EALA's `__FILE__`/`__FUNCTION__`
> literals; the seam cannot, because the binding layer is template-generated and almost never asserts.
> Counted mechanically over the 54 MB decomp: of 898 bindings, **12 carry a true EALA assertion string in
> their own body** (name + `C:\EALA-BUILD-SAB1\...` path), and 10 more `Vehicle.*` race callbacks
> self-name via a plain dotted literal — **22 self-identifying, 876 not**. Identity for the other 876
> comes from [`data/lua_registration_map.tsv`](../../data/lua_registration_map.tsv), reproduced from the
> retail exe's registration code by [`tools/dump_lua_registration.py`](../../tools/dump_lua_registration.py).

> **Confidence tiers — read before trusting a row.** The registration map is byte-level proof of
> *name ↔ VA*, which AGENTS.md accepts alongside assertion strings — but it is a **derived artifact**, not
> a string in the binary, and it is a single point of failure for 876 rows. If the dumper's stanza parser
> is wrong, it is wrong everywhere at once, and no family doc would catch it: each verifier checked its
> rows *against the same map*. That risk is uncorrelated with everything else here and is the largest
> systemic weakness in this catalog. Per-doc "confirmed" therefore usually means *registration-map
> identity + body read + primitives unambiguous*, not *assertion-anchored*. Docs 12 and 20 state this
> explicitly; treat it as applying to all of them.

## The mechanics (read in order)

| Doc | Establishes | Confidence |
|---|---|---|
| [00-seam-overview.md](00-seam-overview.md) | The front door; fuses 01–06 and adjudicates their contradictions | high |
| [01-registration-and-dispatch.md](01-registration-and-dispatch.md) | `LuaGlueFunctor0<&F>` RTTI generates the surface; no static `{name,fnptr}` table exists — registration is imperative, 84 bytes/binding, into 26 registries. The long-standing "callers inside no exported function" anomaly is resolved: 32-byte adapter thunks Ghidra never made functions for | high |
| [02-marshalling-abi.md](02-marshalling-abi.md) | It is unmodified **Lua 5.1**, statically linked and stripped, behind a one-method-per-call `__thiscall` shim. Corrects two premises of the original brief | high |
| [03-handle-and-object-model.md](03-handle-and-object-model.md) | A handle is light userdata carrying a salted 32-bit ID (24-bit slot + 8-bit generation); `FUN_004436f0` is a red-black tree find, not a hash. Handles are session-scoped and cannot survive save/load | high |
| [04-vm-lifecycle-and-script-objects.md](04-vm-lifecycle-and-script-objects.md) | Exactly one `lua_State`, owned by the 0x15c90-byte **script manager** — refuting [`task-managers.md`](../symbol_map/task-managers.md)'s "world object". Also closes the standing open question in [`lua_scripts.md`](../formats/lua_scripts.md): descriptor +0x00 is `pandemic_hash("d:\" + path + ".luac")`, 321/321 forward | high |
| [05-engine-to-lua-callbacks.md](05-engine-to-lua-callbacks.md) | **Lua function values are never stored.** Callbacks are a name string + optional self/user table, resolved by name at fire time. No registry ref anywhere | high |
| [06-lua-side-wrapper-layer.md](06-lua-side-wrapper-layer.md) | Namespace tables are engine-injected and merely *extended* in Lua — modders can patch existing tables but cannot add bindings from script | high |

## The families

Every row below was written by one agent and then attacked by a second. **"Corrected" is the good outcome**
— it means a verifier independently reproduced the claims, found specific defects, and fixed them in
place. Each doc carries a `> **Verified:**` line recording what was re-checked.

| Family | Doc | Bindings | Confidence | Verdict |
|---|---|---|---|---|
| Actor / human / disguise / ragdoll | [10-family-actor-human.md](10-family-actor-human.md) | 111 | high | corrected |
| AI: squad, combat, target, formation | [11-family-ai-squad-combat.md](11-family-ai-squad-combat.md) | 99 | high | corrected |
| Suspicion, escalation, will-to-fight | [12-family-suspicion-wtf-alarm.md](12-family-suspicion-wtf-alarm.md) | 56 | high | corrected |
| Vehicle / train / plane / racer | [13-family-vehicle-train-plane.md](13-family-vehicle-train-plane.md) | 125 | high | corrected |
| Navigation, movement, pathing | [14-family-navigation-movement.md](14-family-navigation-movement.md) | 23 | high | corrected |
| Mission / objective / checkpoint / task | [15-family-mission-objective-task.md](15-family-mission-objective-task.md) | 73 | high | corrected |
| World: zones, interiors, water, streaming | [16-family-world-zone-interior.md](16-family-world-zone-interior.md) | 57 | high | corrected |
| Sound, voice, conversation | [17-family-sound-voice-conversation.md](17-family-sound-voice-conversation.md) | 38 | high | corrected |
| Cinematics / camera / face | [18-family-cinematics-camera-face.md](18-family-cinematics-camera-face.md) | 16 | high | corrected |
| UI / HUD / tutorial / text (Scaleform) | [19-family-ui-hud-tutorial.md](19-family-ui-hud-tutorial.md) | 78 | high | corrected |
| Inventory, perks, shops, collectables | [20-family-inventory-perks-shop.md](20-family-inventory-perks-shop.md) | 37 | high | corrected |
| Freeplay: ambient events & tags | [20-family-inventory-perks-shop.md](20-family-inventory-perks-shop.md#the-freeplay-table--ambient-events) | 17 | high | corrected |
| Utility / SaveLoad / Object / Trigger | [21-family-utility-saveload-object.md](21-family-utility-saveload-object.md) | 253 | high | corrected |
| Point primitives: attraction, focus, sabotage, searchlight | [22-family-attraction-focus-sabotage-points.md](22-family-attraction-focus-sabotage-points.md) | 53 | high | corrected |
| Render: weather, FX lifetime, colour, light | [23-family-render-weather-fx.md](23-family-render-weather-fx.md) | 19 | high | corrected |

Rows overlap by design — a binding pinned in one doc's table may be cited in another's (doc
[23](23-family-render-weather-fx.md) re-partitions all 42 `Render` rows but documents only the 19 no
other doc claimed), so this column does not sum to 898. The [Coverage](#coverage) table below is the
non-overlapping count.

All fourteen docs have now been through the pass; **all fourteen came back "corrected"**, which is the base
rate this catalog should be read against. Docs 13 and 19 were verified last, out of band, after their
verifiers died on a tooling fault in the original run — their passes were the most aggressive of the set
(both re-derived byte-level claims straight from `Saboteur.exe` with pefile + capstone rather than trusting
the decomp) and they found, among other things, a widget slot documented as empty that is really
`WSHUDLanguageSelect`, two rows whose own cited call sites refuted their signatures, and three retail
do-nothing stubs behind a dead shipped script path ([13 §Dead paths](13-family-vehicle-train-plane.md)).

That exercise also produced the most important caveat here: **the decomp is a lossy view of the exe.** Both
verifiers hit claims that looked fabricated because Ghidra had renamed a thunk or rendered an argument
index in hex, but that were true in the bytes. Where a family doc's byte-level reading and the 54 MB decomp
disagree, re-read the exe before believing the decomp.

## Coverage

Counted from the docs' own tables against the 898-row registration map, not estimated:

| | Bindings | |
|---|---|---|
| Pinned in a family doc table | **898** | 100% of 898 |
| No family doc at all | **0** | — |
| — of which, covered by an adversarially verified doc | **898** | all of it |
| Self-identifying in the binary (EALA assert) | **12** | the only assertion-anchored rows |
| Self-identifying (dotted literal, `Vehicle.*`) | 10 | different mechanism, not an assert |
| Identity from the registration map alone | **876** | byte-level, but single-sourced |

**All 26 tables are complete, and the orphan gap is closed.** Re-run 2026-07-16 with
[99 §6](99-gaps-and-next-steps.md#6-reproducing-the-orphan-census)'s strict matcher — distinctive
`cpp_symbol` token, qualified `Table.LuaName`, or `impl_va`; a bare `lua_name` match is rejected —
over docs 10–23: **0 orphans**, down from 86. The companion metric "appears in no document at all,
including 00–06" was 73 and is now necessarily 0 too. The eight tables that were open:

| Table | Total | Was missing | Closed by |
|---|---|---|---|
| `AttractionPt` | 25 | 25 | [22](22-family-attraction-focus-sabotage-points.md) |
| `Render` | 42 | 19 | [23](23-family-render-weather-fx.md) |
| `Freeplay` | 21 | 17 | [20 § Freeplay](20-family-inventory-perks-shop.md#the-freeplay-table--ambient-events) |
| `FocusPt` | 18 | 12 | [22](22-family-attraction-focus-sabotage-points.md) |
| `Sabotage` | 8 | 8 | [22](22-family-attraction-focus-sabotage-points.md) |
| `Sensory` | 8 | 8 | [11](11-family-ai-squad-combat.md) |
| `Damage` | 2 | 2 | [16](16-family-world-zone-interior.md) — adopted orphans, counted separately from its 55 family rows |
| `Searchlight` | 2 | 2 | [22](22-family-attraction-focus-sabotage-points.md) |

The two independent measures now agree: the strict matcher's 86 orphans (which counted `Render` at 15)
and this table's 93 "no family doc" rows (which counted `Render` at 19, by the claimant docs' own
inclusion rules) were always nested, and both are now 0. The 93 reconcile exactly: 47 to
[22](22-family-attraction-focus-sabotage-points.md), 19 to [23](23-family-render-weather-fx.md), 17 to
[20](20-family-inventory-perks-shop.md), 8 to [11](11-family-ai-squad-combat.md), 2 to
[16](16-family-world-zone-interior.md).

`AttractionPt`, `Sensory` and `Sabotage` were never obscure — they are the ambient-life, perception and
demolition surfaces, i.e. the parts of the game The Saboteur is named after. Their absence was a
scoping accident of how the families were cut, and [99 §2](99-gaps-and-next-steps.md#2-why-the-orphans-are-whole-tables--the-diagnosis)
diagnoses it: the binary is partitioned by table, the docs were partitioned by topic. **Coverage being
100% today does not make that structural failure mode go away** — it was invisible to every doc's own
inclusion rule once, and nothing yet stops it recurring. 99 §2's recommendation stands: make the census
a CI invariant, not a thing an agent happens to re-run.

## Method & provenance

Four stages, each feeding the next:

1. **Mechanics first** (01–06). Reverse the machinery — registration, ABI, handles, VM, callbacks,
   wrappers — before any binding. Every later doc inherits the `FUN_006f7xxx` decoder ring from
   [02](02-marshalling-abi.md).
2. **ABI digest** ([00](00-seam-overview.md)). Fuse the six and adjudicate their disagreements. Three
   contradictions were load-bearing; one determines whether a signature is right at all.
3. **Family sweep** (10–21). One agent per table group: every binding gets a VA, a signature, a tier and,
   where it exists, a shipped Lua caller.
4. **Adversarial verify.** A second agent tries to *break* the doc — reproducing counts, re-disassembling
   from `Saboteur.exe` with capstone, re-grepping every citation — and edits defects in place.

Stage 4 earned its cost. It caught a fabricated "no corpus call site" three lines from a call the author
had cited; a "cut content" zone type that ships as `Trigger.CreateCafe`; an inferred premise silently
promoted to fact to carry a headline; and a great many corpus counts that merged direct calls with bare
table-value references. It also *vindicated* the byte-level work more than once — two suspected
fabrications turned out to be Ghidra artifacts, with the doc right and the decomp misleading.

Evidence rules are [AGENTS.md](../../AGENTS.md)'s: a VA, a byte offset, or a corpus `file:line` for every
claim; no Mercenaries 2 imports (verifiers swept for these and found none); honest tiering.

## What is still open

Ranked by value. Items 1–2 are what a next agent should attack; 3–4 are what most changes a modder's
mental model. **The orphan gap that dominated this list is closed** — all 898 bindings are now documented
(see [Coverage](#coverage)) — so item 2 is its successor: stopping it from re-opening.

1. **Validate the registration map independently.** 876 rows hang off one tool. One x32dbg breakpoint on
   `0x006f6660` during startup, logging `ecx` and the pushed name, would confirm the whole map from a live
   process — and per [05 Q2](05-engine-to-lua-callbacks.md#open-questions) also mechanically resolve every
   remaining "table open" row. Highest leverage in the catalog. **Now the only item that can invalidate the
   catalog wholesale**, and with the binding surface fully documented it is the last systemic risk left.
2. **Make the coverage census a CI invariant.** `tools/check_seam_coverage.py`, asserting every tsv row is
   claimed by ≥1 family doc, with [99 §6](99-gaps-and-next-steps.md#6-reproducing-the-orphan-census)'s
   strict matcher as its core. The orphan gap is closed *today*; nothing prevents the next doc from
   re-opening it, because the failure was invisible to every inclusion rule that caused it.
   ([99 §2](99-gaps-and-next-steps.md#2-why-the-orphans-are-whole-tables--the-diagnosis))
3. **Protected or not?** Is the by-name callback invocation wrapped in `lua_pcall`? A script error inside
   `OnDeath` either aborts the frame or is swallowed, and nobody knows which. The invoker lives in the
   un-recovered region; a breakpoint on `FUN_0045ee96` (`luaD_pcall`) with a deliberately-erroring callback
   settles it. ([05 Q1](05-engine-to-lua-callbacks.md#open-questions), [00 §9](00-seam-overview.md))
4. **The `mov eax,1` consequence.** Every void-binding adapter returns 1. If that is `nresults`, a void
   binding may hand its own last argument back to Lua. Trivial to check live; affects every signature's
   return column. ([01 Q1](01-registration-and-dispatch.md#open-questions))
5. **Is the old `lua_State` closed on reload?** `lua_close` has `callers=[]`. If nothing closes it, every
   level reload leaks a full VM. ([04 Q1](04-vm-lifecycle-and-script-objects.md#open-questions))
6. **~~How does `GetHandleByName` resolve a string?~~** **RESOLVED 2026-07-29 — in doc 10's favour.**
   The tension recorded here (03 §7's "not `pandemic_hash`" versus doc 10's finding that the census
   only checked *direct* callers, the path reaching `pandemic_hash` one indirection deeper via
   `FUN_00db7c10`) is settled from the data: EditNodes objects are keyed by `pandemic_hash` of their
   full editor path, and **1 632** path literals in `LuaScripts.luap` — the very strings Lua passes to
   `Util.GetHandleByName` — reproduce a retail object tag, 0 colliding with the container namespace
   ([editnodes.md](../formats/editnodes.md)). 03 §7 is corrected; the handle *value* is still not a
   hash. Residual: the instruction path inside `0x00758b30`.
7. **What is `vtable+0x1c`?** The proposed `GetTargetRef()` rests on usage alone. Reading slot 7 of a known
   RTTI class's vtable is cheap and would firm up [03 §4](03-handle-and-object-model.md).
8. **Where is `DAT_0143db28` written?** 279 reads, no surviving store. Finding it pins the handle map's
   owner class by RTTI. ([03 Q1](03-handle-and-object-model.md#open-questions))
9. ~~**Is the 898 list a census or a floor?**~~ ✅ **CLOSED 2026-07-24 — it is a census.** [06](06-lua-side-wrapper-layer.md)'s
   "46 live namespaced calls with no binding" was a matcher artifact: it compared Lua names against
   **C++ symbols**, and 256 of the 898 differ. At least 5 of its 28 named orphans are in
   `data/lua_registration_map.tsv`, including its own load-bearing example `Util.SetDisableControls`
   (C++ `SetDisableControlsTable`). ≈24 genuine orphans remain. The "≥165 unreachable" figure rests on
   the same matcher and **needs recomputing** as a join against the registration map — the `Squad.*`
   table alone (18 bindings, 189 corpus call sites) was wrongly counted as entirely dead.
   See [00 §8.2](00-seam-overview.md).

**Two stale sections to be aware of.** The registration map post-dates the mechanics docs and silently
closes some of their open questions — [04 Q5](04-vm-lifecycle-and-script-objects.md#open-questions) says
"`GetSelf` is not VA-pinned" (it is: `Actor.GetSelf` @ `0x007126c0`), and [03
Q4](03-handle-and-object-model.md#open-questions) says the `GetHandleByName` family "could not be located"
(all four are now pinned). Those docs were not rewritten; where a mechanics doc and the map disagree on
whether something is *findable*, the map wins.
