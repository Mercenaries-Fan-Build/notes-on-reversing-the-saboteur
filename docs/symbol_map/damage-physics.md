# Damage, Physics & Explosion

*WildStar/Odin engine subsystem — The Saboteur (2009). Slug: `damage-physics`.*

The combat-physics layer of the engine: explosion spawning + area impulse/damage, per-human Havok physics (fall / impact / human-on-human collision damage), the shared `WSDamageable` health-subtraction core, ragdoll death paths, destructibles, and the bridges from all of these into AI reactions and HUD feedback.

This is the **MERCS2 wall** — the highest cross-repo-value cluster. `WSDamageable` / `WSExplosion` / apply-impulse concepts recur across Pandemic titles, and `Util.CreateExplosion` is the canonical Lua entry.

## How it fits together

```
Lua: Util.CreateExplosion(bp, x,y,z)          Lua: SetHealth / Kill / SetFallDamage*
        │                                             │
        ▼                                             ▼
FUN_004886b0  WSExplosion::Spawn            (binding thunks — need vtable map)
   alloc + set pos/blueprint                          │
        │                                             ▼
        ▼                                   FUN_00506140  WSHuman::ApplyDamage
FUN_00487510  WSExplosion::SetupExplosion   (resistance mask this[0x99..0x9c])
   radius/damage from blueprint[0xd]                  │
   world query + impulse + FX                         ▼
                                            FUN_00666b20  WSDamageable::ApplyDamage
Havok per-tick:                              (health this[3] subtraction core)
 FUN_00586fe0 DoUpdatePhysics
 FUN_00584fa0 objectInteractionCallback     Collision-driven damage:
        │                                    FUN_004ffc10  WSHuman::Collide
        ├─► FUN_005897e0 TakeFallDamage ─┐    FUN_00588100  DamageHumanOnHuman
        └─► FUN_00589bc0 TakeImpactDamage┼──► FUN_0099bc00  DispatchCollisionDamage
                                         │        (build WSDamageEvent + dispatch)
Death / ragdoll:                         ▼
 FUN_00503160 DieImmediateDropAndRagdoll        FUN_004fad70 NotifyVehicleCollision
 FUN_0050c010 DieImmediateNoDropOrRagdoll       FUN_0060c210 GhostOfDeath::ApplyRagdoll
 FUN_00577490 RagdollState::TestCollision       FUN_005ceed0 Vehicle::ApplyDamageAIReactions
 FUN_006c4030/4140 HavokUtil ragdoll pivots         └─► FUN_00898440 HiveMind gawk event
```

## Pinned functions (string-anchored) — ⚠️ 17 of 22, not all 22

**17** of the 22 VAs below carry an inline assert/log string (`file.cpp` + `Class::Method`) that the
decompiler preserved; for those the class/method names are **ground truth**, not guesses.

**The other 5 are inferred and are proposals, not ground truth:**

| VA | Claimed name | What the body actually carries |
|----|--------------|-------------------------------|
| `FUN_004886b0` | `WSExplosion::Spawn` | no `Class::Method` string, no `.cpp` path. (`"WSExplosion::Spawn"` occurs nowhere in the decomp; only `"WSExplosion::SetupExplosion"` does.) |
| `FUN_00666b20` | `WSDamageable::ApplyDamage` | no anchor string at all — and this doc already says so at its own line 83 ("its exact method name is inferred"). No `"WSDamageable::…"` string exists anywhere in the decomp. |
| `FUN_0099bc00` | `WSHumanPhysics::DispatchCollisionDamage` | no anchor string, no `.cpp` path |
| `FUN_006c4030` | `…GetRagdollConstraintPivotA` | only `WSHavokUtil.cpp` + the message `"Unsupported type of constraint in rag doll"` — the *method* name is invented |
| `FUN_006c4140` | `…GetRagdollConstraintPivotB` | as above |

Everything else in this section is genuinely string-anchored. (Corrected 2026-07-24: this paragraph
previously claimed *every* VA was anchored, which contradicted the doc's own line 83.)

| VA | Name | Evidence |
|----|------|----------|
| `FUN_00487510` | `WSExplosion::SetupExplosion` | `"WSExplosion::SetupExplosion"` @L76712; reads radius/damage off blueprint `this[0xd]` |
| `FUN_004886b0` | `WSExplosion::Spawn` (CreateExplosion backend) | alloc + write x/y/z, tail-call SetupExplosion; matches `Util.CreateExplosion` |
| `FUN_00506140` | `WSHuman::ApplyDamage` | `"WSHuman::ApplyDamage"` @L149697; damage-type mask vs resistances, calls `FUN_00666b20` |
| `FUN_00666b20` | `WSDamageable::ApplyDamage` (health core) | called by ApplyDamage @0x00506213 + weapon/vehicle sites; mutates health `this[3]` |
| `FUN_0099bc00` | `WSHumanPhysics::DispatchCollisionDamage` | shared helper called by Fall/Impact/HumanOnHuman with `(desc, target, 0, dmg)` |
| `FUN_00589bc0` | `WSHumanPhysics::TakeImpactDamage` | `"WSHumanPhysics::TakeImpactDamage"` @L224583; speed-over-threshold → ApplyDamage |
| `FUN_005897e0` | `WSHumanPhysics::TakeFallDamage` | `"WSHumanPhysics::TakeFallDamage"` @L224290; fronts `Set/ResetFallDamage*` Lua |
| `FUN_00586fe0` | `WSHumanPhysics::DoUpdatePhysics` | `"WSHumanPhysics::DoUpdatePhysics",0x894` @L222716 |
| `FUN_00584fa0` | `WSHumanPhysics::objectInteractionCallback` | `"...objectInteractionCallback"` @L221767 |
| `FUN_00588100` | `WSAllCdPointCollector::DamageHumanOnHuman` | asserts @L223363/223371; both collision-type==0x10, rel-velocity crush damage |
| `FUN_00577490` | `WSHumanStateRagdoll::TestCollisionWithOthers` | `"...TestCollisionWithOthers"` @L215070 |
| `FUN_00503160` | `WSHuman::DieImmediateDropAndRagdoll` | `"...DieImmediateDropAndRagdoll"` @L147880 |
| `FUN_0050c010` | `WSHuman::DieImmediateNoDropOrRagdoll` | `"...DieImmediateNoDropOrRagdoll"` @L153002 |
| `FUN_004ffc10` | `WSHuman::Collide` | `"WSHuman::Collide"` @L146453 |
| `FUN_004fad70` | `WSHuman::NotifyVehicleCollision` | `"...NotifyVehicleCollision"` @L143624 |
| `FUN_0069a640` | `WSGrenade::Update` | `"WSGrenade::Update"` @L369444 |
| `FUN_005ceed0` | `WSVehicle::ApplyDamageAIReactions` | `"...ApplyDamageAIReactions"` @L258330; calls hive-mind gawk |
| `FUN_00898440` | `WSAICombatHiveMind::BroadcastVehicleDamageGawkEvent` | `"...BroadcastVehicleDamageGawkEvent"` @L662991 |
| `FUN_0060c210` | `WSGhostOfDeath::ApplyRagdollIfRelevant` | `"...ApplyRagdollIfRelevant"` @L290538 (WSPhysicsVehicleUtils.cpp) |
| `FUN_0066b410` | `WSDynamicPart::DeleteSynched` | `"WSDynamicPart::DeleteSynched",0x577` (WSDestructable.cpp) |
| `FUN_006c4030` | `WSHavokUtil::GetRagdollConstraintPivotA` | `"Unsupported type of constraint in rag doll"` WSHavokUtil.cpp:0x6b |
| `FUN_006c4140` | `WSHavokUtil::GetRagdollConstraintPivotB` | same message @WSHavokUtil.cpp:0xa5 |

## Data-flow notes

- **Explosion**: `Util.CreateExplosion(sBlueprint, x, y, z)` (see `ScriptControllers/ExplosionController.lua:46/74`) → `FUN_004886b0` allocates the `WSExplosion`, stores the position vector at `obj+0x24/0x28/0x2c` and the blueprint handles at `obj+0xe80..`, then calls `FUN_00487510` (`SetupExplosion`). SetupExplosion reads `radius = blueprint[0xd]+0xa0`, `damage = blueprint[0xd]+0x94`, runs a world overlap query and issues the area impulse + FX. The `WSExplosionApplyImpulseFunction` per-body callback itself is not yet pinned (see gaps).
- **Human damage funnel**: all collision damage converges on `FUN_0099bc00`, which takes a `(file, func, line)` descriptor plus `(target, 0, damageFloat)` and builds/dispatches a `WSDamageEvent`. `WSHuman::ApplyDamage` (`FUN_00506140`) then gates by damage-type bitmask (`WSDamageEvent+0x54`, bits 1/3/4/10) against the human's per-type resistance floats `this[0x99..0x9c]` before decrementing health via `FUN_00666b20`.
- **Impact vs fall**: `TakeImpactDamage` computes `clamp((speed − threshold) · scale)` and calls the human's ApplyDamage vtable slot (`+0x174`). The dispatcher that chooses fall vs impact sits in an un-named decomp gap (see gaps).
- **Ragdoll**: driven from `WSHumanStateRagdoll`; constraint pivots resolved through the two `WSHavokUtil` accessors, which switch on Havok constraint type and assert on unsupported types.
- **AI/HUD bridges**: `WSVehicle::ApplyDamageAIReactions` → `WSAICombatHiveMind::BroadcastVehicleDamageGawkEvent`; HUD side is `WSHUDDamageIndicator` + `WSDamageBlurFilter` + `CameraShakeExplosion` (Lua).

## Lua API surface

`CreateExplosion`, `CameraShakeExplosion`, `GetHealth`/`SetHealth`/`GetMaxHealth`/`SetHealthRecoveryPct`, `GetDamageState`/`SetDamageState`, `AllowHumanDamage`, `SetRespondToDamage`/`SetRespondToFriendlyDamage`, `SetTakeDamageInCinematic`, `SetLethalForce`, `Kill`/`KillPlane`/`SetPlaneHealth`, the `Set/ResetFallDamage{Death,MinSpeed,Mult,Ragdoll}` tuning family, `ActorRagdoll`, `RegisterRagdollCallback`/`ClearRagdollCallback`, `IsRagdollInWater`, `SetDropWeaponWhenRagdolled`, `SetDistantRagdollSound`, `VehicleForceKeyframing`, `WSTrain::TrainReleaseToPhysics`.

## Gaps / caveats

- ✅ **Resolved 2026-07-24:** the Lua binding C-thunks (`CreateExplosion`, `GetHealth`/`SetHealth`, `GetDamageState`, `ActorRagdoll`, fall-damage setters) **can** now be pinned exactly — [`lua_registration_map.tsv`](../../data/lua_registration_map.tsv) carries `impl_va`/`thunk_va` for all 898. This doc predates it and pins only the CreateExplosion path, via the `FUN_004886b0 → FUN_00487510` chain + Lua-corpus behavior match; the rest are unconverted.
- `WSExplosionApplyImpulseFunction` (the per-body impulse callback) is **unpinned**; SetupExplosion's world/impulse/FX calls (`FUN_00898970`, `FUN_007f85e0`, `FUN_009db910`, `thunk_FUN_00492f81`) are unlabeled.
- The **fall-vs-impact dispatcher** at ~`0x0058ace3`/`0x0058acef` lives in an un-emitted region between `FUN_0058a480` and `FUN_0058ad00` — no header was produced, so it could not be named.
- `WSDamageable` / `WSDamageablePart` / `WSDamageSphere` / `WSDamageRegion` carry no inline strings; their methods aren't individually pinned. `FUN_00666b20` is the best-evidenced shared health-subtraction core but its exact method name is inferred.
- `WSDestructable` is represented only by the `WSDynamicPart::DeleteSynched` stub; the destruction-trigger / mesh-swap logic is unpinned.
  - ✅ **Resolved 2026-10-12:** there is no mesh swap for damage groups — the state shows/hides the drawcalls hanging off each part's bone inside one mesh (`FUN_006678e0` → deferred queue → `FUN_0066b840` → `FUN_00e14610`). Full write-up: [`../damage_and_destruction.md`](../damage_and_destruction.md).
- `FUN_004f69f0` (`"WSHuman::Die"`) is a 54-byte **profiling/log marker only** (calls `FUN_009997a0` with a `(file,func,line)` tag), **not** the real death logic — use `DieImmediateDropAndRagdoll` / `DieImmediateNoDropOrRagdoll`.

---

## Verification (adversarial pass)

**Verdict: solid** — 22/22 key functions confirmed against the decomp.

**Seams (cross-subsystem):**

- FUN_0099bc00 (the 'human-physics' damage dispatch) is actually a GENERAL WSDamageEvent build+dispatch shared across subsystems. Beyond TakeImpactDamage/TakeFallDamage/DamageHumanOnHuman it is also called from FUN_00714230 (Script/Interface Actor.cpp @0x0071430b — a Lua actor-damage binding), FUN_008ed1e0 (WSAIAttractionPt.cpp @0x008ed2af — AI subsystem), and FUN_006ab030 (@0x006ab2ee — a damage-type-gated trigger). Doc's 'used by all human-physics damage sources' framing is too narrow.
- FUN_009db910 (explosion FX/broadcast fired by SetupExplosion @0x004884c6) is a widely-shared FX dispatcher: also invoked from a Lua thunk FUN_0161a3a0 (@0x0161a42a), grenade FX FUN_006a0fb0/FUN_006a53b6, FUN_0051c760, and the FUN_009d1fa0/009d21a0/009d24e0/009d2650 family — a cross-subsystem FX seam the explosion doc doesn't surface.
- FUN_005ceed0 (WSVehicle::ApplyDamageAIReactions) has an upstream caller FUN_00665990 (@0x006659eb) in addition to 0x005d3799; this ties the vehicle-damage path into the AI hive-mind (FUN_00898440) from the vehicle side, which the doc only describes downstream.
- FUN_00666b20 (WSDamageable core) is reached from a weapon/impulse site FUN_004b3960 (@0x004b3a26) and vehicle sites (0x005d3ec1/0x005e5510) plus FUN_0082adf0 — confirming a shared weapon+vehicle+human damage funnel into the single health-subtraction core, worth stating explicitly.

**Additional gaps / suspected decomp corruption:**

- FUN_0069a640 WSGrenade::Update: identity CONFIRMED via assert 'WSGrenade::Update' @369444 (WSGrenade.cpp), but the doc's causal '(arming/fuse -> explosion spawn)' is NOT evidenced — the body contains no direct call to the explosion spawn/setup functions (FUN_004886b0 / FUN_00487510) nor to FUN_009db910. Spawn presumably occurs via a vtable slot or an uninlined helper; treat the arrow as inference, not fact.
- FUN_00666b20 has a 5th/6th caller at 0x0044c159 (doc omits it) that has NO owning function header — 0x0044c159 falls past the end of FUN_0044c070 (0x0044c070+218=0x0044c14a) and before FUN_0044c230, i.e. an orphan xref in a padding/thunk gap. Possible decomp artifact worth a second look.
- FUN_00506140 (WSHuman::ApplyDamage) and FUN_004ffc10 (WSHuman::Collide) both show callers=[] in the decomp — they are vtable-dispatched (ApplyDamage is the +0x174 slot invoked by TakeImpact/TakeFall/DamageHumanOnHuman; Collide via the human vtable). The doc asserts these roles but provides no vtable-slot evidence; the empty caller list is expected but should be flagged as indirect-only.
- FUN_0060c210 assert text confirmed as 'WSGhostOfDeath::ApplyRagdollIfRelevant' @290538, but I did not independently confirm the doc's file attribution 'WSPhysicsVehicleUtils.cpp' (the class prefix is WSGhostOfDeath); low-risk but unverified.

**Verifier corrections:**

Verified against decomp; all 22 key VAs confirmed (headers + bodies). Notable body confirmations to fold in:

- FUN_00487510 SetupExplosion: assert @76711-76712 (WSExplosion.cpp); radius read `*(float*)(this[0xd]+0xa0) * _DAT_01114d18` @76766, damage `*(this[0xd]+0x94)` @76767; FX calls FUN_00898970 (@76699/0x00488307), FUN_007f85e0 (@76768/0x00488517), FUN_009db910 (@76757/0x004884c6). CONFIRMED.
- FUN_004886b0 Spawn: alloc via thunk_FUN_0160f480 @76846, pos writes obj+0x24/0x28/0x2c @76848-76850, blueprint handles obj+0xe80/0xe84/0xe88/0xe8c @76853-76856, tail-call FUN_00487510 @76857. Note its own caller is FUN_0160f160 (Lua thunk region), consistent with a CreateExplosion backend.
- FUN_00506140 WSHuman::ApplyDamage: resistance/type gate is EXACT — bits 1/3/4/10 of *(param_2+0x54) vs floats this[0x99]/[0x9a]/[0x9b]/[0x9c] @149491-149494; health snapshot fVar1=this[3] @149495, FUN_00666b20 @149496 (=0x00506213), post-check this[3]<=fVar1 @149497. CONFIRMED.
- FUN_00666b20: health core confirmed — writes `param_1[3]=(int)param_4` and `param_1[3]=0` inside body. CONFIRMED.
- FUN_00589bc0 TakeImpactDamage: FUN_00422b50 speed, minus _DAT_011174b8/_DAT_011174b0, *_DAT_01117490, *_DAT_011174cc, clamp FUN_004372d0, ApplyDamage slot +0x174, then FUN_0099bc00(&desc, target, uVar12, dmg). CONFIRMED.
- FUN_005897e0 TakeFallDamage: FUN_0099bc00(&pcStack_6c, uVar2, uVar7, fVar8) after +0x174 slot. CONFIRMED.
- FUN_00588100 DamageHumanOnHuman: two FUN_0057b4e0()==0x10 gates, velocity vs _DAT_010a45c0, TWO FUN_0099bc00 calls (param_1 and param_2 targets, arg3=0, dmg=_DAT_00f7df54). CONFIRMED.
- FUN_006c4030 PivotA: switch on constraint vtable +0x20 → return piVar1+8 (case 0,8) / piVar1+0x14 (case 1,2,6,7); assert WSHavokUtil.cpp:0x6b. CONFIRMED. Twin FUN_006c4140 assert @391610 (0xa5). CONFIRMED.
- FUN_0066b410 DeleteSynched: FUN_00453070(WSDestructable.cpp, "WSDynamicPart::DeleteSynched", 0x577) @343396-343397. CONFIRMED.

Recommend rewording the FUN_0099bc00 entry from 'the common WSDamageEvent build-and-dispatch used by all human-physics damage sources' to 'general WSDamageEvent build-and-dispatch; used by the human-physics damage sources AND by Script/Interface (Actor.cpp) and AI (WSAIAttractionPt) damage paths.' Recommend softening FUN_0069a640's '-> explosion spawn' to '(fuse/arming; explosion spawn via vtable, not directly evidenced in body)'.
