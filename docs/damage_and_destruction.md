# Damage & destruction states — how The Saboteur breaks things

**Date:** 2026-10-12. **Scope:** how a prop, vehicle, building piece or character shows a damaged or
destroyed state: the data that describes it (GameTemplates), the geometry that carries it (MESH), the
engine code that switches it, and the separately-modelled `_DAM` / `_Destroyed` meshes.
**Companions:** [`symbol_map/damage-physics.md`](symbol_map/damage-physics.md) (health, explosions,
ragdoll — the *damage* side), [`sab-engine-lua-seam/16-family-world-zone-interior.md`](sab-engine-lua-seam/16-family-world-zone-interior.md#the-damage-table--prop-damage-states)
(the `Damage.SetDamageState`/`GetDamageState` Lua bindings, read instruction by instruction),
[`formats/gametemplates.md`](formats/gametemplates.md), [`formats/mesh_geometry.md`](formats/mesh_geometry.md).

Evidence grades used on every claim:

| grade | meaning |
|---|---|
| **PROVEN** | verified this pass — a decomp/disassembly body read, or a count over the whole retail data set |
| **INFERRED** | reasoned from structure or names, not checked against the engine |
| **SPECULATIVE** | a lead, nothing more |

Ground truth: GOG `Saboteur.exe` (sha256 `628cdbb6…68e4ef`) decomp + objdump; the retail Steam install
(`France/loosefiles_BinPC.pack` GameTemplates — 10,761 templates; every `*.megapack` — 45,134 distinct
mesh names); the June 2008 Xbox 360 prototype's `WildStar_d.map` / PDB for names and 2008 layouts
(never for retail offsets).

---

## 0. Result in one line

**A destructible object is ONE mesh whose pieces hang off bones; the engine shows or hides those bones'
drawcalls by damage state.** GameTemplates lists the object's *damage groups* and their *parts*; each
part names a bone and says whether it is the **pristine** piece. In state 0 the pristine parts draw, in
state 1 the others do. Nothing is swapped in from another file. The separately-modelled `_DAM` /
`_Destroyed` meshes that also exist are a different, still-unexplained mechanism (§6).

---

## 1. The model: group → part → bone → drawcalls

```
GameTemplates object (Prop, CAR, TRUCK, …)
  Model            = pandemic_hash(mesh name)
  DamageGroups     : [ groupName, … ]                 a group has ONE state: 0 or 1
  PartList         : [ part, … ]
      part.boneName     = pandemic_hash(bone in the Model mesh)   e.g. VO_Rubble19, VO_Bridge
      part.DamageGroup  = which group it belongs to
      part.Pristine     = 1 → drawn in state 0 (intact) ; 0 → drawn in state 1 (damaged)

MESH (the Model)
  bones[]       name hashes + parent ids
  DrawCall[]    { …, u16 parentBone @+0xc, u16 lodFlags @+0xe }
                every drawcall hangs off one bone; the part that owns that bone's subtree owns it
```

**Data cross-check — PROVEN.** A Rust probe parsed every template's parts and every Model mesh's bones
and drawcalls itself, walked each part bone's subtree (stopping at bones another part owns — as the
engine does, §3), and classified the drawcalls:

| | DrawCall `lodFlags` bit 0x10 **clear** | bit 0x10 **set** |
|---|---:|---:|
| drawcalls under a **pristine** part | **22,051** | 0 |
| drawcalls under a **non-pristine** part | 0 | **17,551** |

835 templates carry parts (6,317 parts). 705 had their Model mesh found and parsed; 5,799 of those
templates' part bones exist in the mesh (20 do not); 17 parts own no drawcalls; 509 drawcalls sit under
no part (303 without the bit, 206 with). The 127 templates whose Model is not in any retail megapack, and
3 meshes whose tail did not end at the body end, were not checked.

So **DrawCall `lodFlags` bit 4 (0x10) marks geometry that belongs to a damaged-state part** — perfectly
separated over 39,602 drawcalls. No engine code reading the bit was found (§3); the engine selects by
bone, and the bit looks like an exporter-side marker. *(Correlation PROVEN; "exporter-side" INFERRED.)*

---

## 2. On disk — the GameTemplates keys

Damage templates use the same flattened-list stream as body setups ([`formats/gametemplates.md`](formats/gametemplates.md#characters-body-setups-slots-and-lod)):
all group declarations first, then all parts. Every part's `DamageGroup` matches a declared group
(6,316/6,316, PROVEN). Names below are verified by `pandemic_hash(name) == key`; "dict" = the name came
from SabTool's `Hashes.txt` and hash-verifies.

**Root (`WSDamageableRootBlueprint::SetProperty` `0x006663b0`, PROVEN compare chain)**

| key | name | role |
|---|---|---|
| `0x1aac9531` | `List` (dict) | generic list marker; its value names the list that follows (`DamageGroups`, `PartList`) |
| `0x49d5c2ae` | `DamageGroups` | a group entry (the root allocates a 0x50-byte group per entry) |
| `0xc8d0f617` | `GroupName` | group name hash → group `+0x18` |
| `0x3c56b891` | `FxBone` | → group `+0x48` |
| `0xe3eb3242` | *(unnamed)* int | → group `+0x44`; code tests it against 2; retail values 0/1/2 (1771/697/110) |
| — | `PartList` | hashed at runtime; opens the part list; other keys are forwarded to the last part |

**Part (`WSDamageablePartBlueprint::SetProperty` `0x00664e20` — absent from the Ghidra export; read from
objdump, reached through thunk `0x668e10` from vtable `0x00fc6eb8`+4)**

| key | name | → field | retail |
|---|---|---|---|
| `0x207fba52` | `BoneName` | `+0x04` | the bone in the Model mesh |
| `0xdd2aa7ab` | `DamageGroup` | `+0x08` | |
| `0xc4c441bd` | `PartTimer` | `+0x0c` | |
| `0x6dd070b1` | *(unnamed)* f32 | `+0x10` | 1.0 ×3321, 0 ×51, 0.1 ×12 |
| `0x558c91d7` | *(unnamed)* string | `+0x14` | the part's effect |
| `0x30812467` | `DamageExplosion` | `+0x18` | `"None"` = no explosion |
| `0x7db13d1f` | `DamageSound` | `+0x1c` | |
| `0x187b88f6` | *(unnamed)* string | `+0x20` | |
| `0x03e19511` | *(unnamed)* f32 | `+0x24` | |
| `0x86de6639` | `Pristine` | `+0x28` bit 0 | 2,489 × 1, 3,827 × 0 |
| `0x3885f292` | `isWheel` (dict) | `+0x28` bit 1 | |
| `0x22650220` | *(unnamed)* bool | `+0x28` bit 2 | equals `Pristine` on all but 5 parts |
| `0x9465294b` / `0x8d117b98` | `DamageWeakness` / `DamageInitialHealth` | | per-part health |

**Damageable (`WSDamageableBlueprint::SetProperty` `0x00663d00`, slot +4 of vtable `0x00fc6864`)** —
object-level: `Weakness 0xdd7eae2c`→`+0xc`, `InitialHealth 0x8cbc442d`→`+0x10`, `AreaDamage 0x3121eecd`,
`ExplosionOnDeath 0x82e62211`, `PartImpulse 0x97e0db0f`, a 0x18-byte death-FX record list (count
`0x414631b3`, next `0x49c74990`, fields `0xa1b82ba7` ParticleEffect template, `0x55817592` attach node,
`0xc86877fd`, `0x496a25d2`, `0xfd4869cd`), plus `DamagedByFire/Bullets/Explosions/Collisions/Characters`,
`SmokeHealthThreshold`, `FireHealthThreshold`.

**Group names — PROVEN:** for 675 of 856 damage templates the single group is named after the Model mesh
(`groupName == Model` hash). Vehicles name groups by piece: `VH_OP_Aurora` has 17 (`Aurora_body`,
`hood`, `wheel_fl`, `headlight_*`, …). This is the name `Damage.SetDamageState(h, sDamageGroup, …)`
takes — the open question in the [seam doc](sab-engine-lua-seam/16-family-world-zone-interior.md).

**Who has damage groups — PROVEN (856 templates):** Prop 677, CAR 53, AnimatedObject 37, Weapon 33,
TRUCK 14, Searcher 10, Turret 9, TANK 6, APC 5, TrainCarriage 5, TrainEngine 4, SearchTurret 3.
`WSPropBlueprint` and `WSVehicleBlueprint` derive from `WSDamageableRootBlueprint`; `WSHumanBlueprint`
only from `WSDamageableBlueprint` (2008 PDB) — humans have health, not damage groups.

---

## 3. Runtime — how the state is applied

| step | function | evidence |
|---|---|---|
| decide a part's visibility | `FUN_006678e0` | PROVEN (decomp read): `on = ((Pristine && group.state==0) \|\| (!Pristine && group.state==1)) && enable`; part blueprint `+0x28 & 1`, group state at `+0x20` |
| on: fire the part's effect / `DamageExplosion` | `FUN_00666960` | PROVEN; skipped when the name is `"None"` (`0xfc65d0`/`0xfc65d8`) |
| queue the bone toggle | `FUN_006697c0` → `FUN_0066980f` | PROVEN: type-1 record (mesh, bone, enable, group) on the deferred queue at `0x01311a00` (0x20-byte records, cap 0x800, count `DAT_01321a04`) — the 2008 `ACTIVATEBONE` |
| drain it | `FUN_0066b840` | PROVEN: gathers `(bone, enable)` over the bone subtree (`FUN_00664700`), skipping bones other groups own |
| swap the drawcalls | `FUN_00e14610` | PROVEN (decomp read): for each pair, removes the instance's drawcalls whose u16 `+0xc` (parentBone) == bone; if enabling, re-adds the mesh's own drawcalls (table `+0x14`, count `+0x28`) for that bone; then `FUN_00e0d160` (`SetupLodSegmentCount`) |
| set a group's state | `FUN_00667d60` (writes 1), `FUN_00669c20` | PROVEN: the latter drains a 16-byte-record queue at `0x01302230` (= 2008 `WSDeferredDamagedGroup`: owner, state, group) and re-applies parts via `FUN_00667c40` |

`FUN_00e14610` is the same instance drawcall list the LOD code counts, which is why a damage swap ends in
`SetupLodSegmentCount` — LOD and damage are two filters over one per-instance drawcall table.

**Who sets states.** Health reaching zero (the damage side, [`damage-physics.md`](symbol_map/damage-physics.md)),
scripts and cinematics. Lua: `Damage.SetDamageState(h, sGroup, n)` hashes the group name, finds it in the
component's group list (`FUN_0066a320`, list head at component `+0xa8`) and calls component
`vfn[0x54]` — **it can only ever write state 1** (argument 3 is checked, never fetched; see the
[seam doc](sab-engine-lua-seam/16-family-world-zone-interior.md)). That `vfn[0x54]` feeds the
`0x01302230` queue is INFERRED (slot not traced). The `DestructionSequence` Lua library
([`saboteur-luacd/.../DestructionSequence.lua`](saboteur-luacd/src/Modules/Libraries/DestructionSequence.lua))
choreographs set-piece demolitions — explosions, FX, camera shake, sounds, `KILLOBJECT`,
`SETDAMAGESTATE` — though no shipped sequence uses `SETDAMAGESTATE`. 2008's `WSCinemaDamageState`
(`m_ObjName`, `m_Group`) lets cinematics set a group (INFERRED for retail).

**State domain.** Retail code only distinguishes 0 and 1. The 2008 enum `DamageStateTypes` has
`DMG_PRISTINE=1, LEVEL1=2, LEVEL2=3` (a different numbering); whether a second damaged level survives in
retail is unknown.

**Classes (RTTI + 2008 map, PROVEN names):** `WSDamageable`, `WSDamageableRoot(Blueprint)`,
`WSDamageablePart(Blueprint)`, `WSDamageableGroup` (`m_GroupName`, `m_GroupParent`, `m_DamageState`,
`m_maxDamageState`, `m_PartList`), `WSDestructable` (`SetGroupDamageState`, `ActivateBone`,
`CreateDynamicPart`, `DeferReInstance`), `WSDynamicPart`, `WSDynamicProp`, `WSCinemaDamageState`,
`WSDeferredDamagedGroup`. Struct offsets are 2008 Xbox; do not apply them to PC.

---

## 4. Vehicles

Same mechanism, many groups per object (PROVEN — data + shared root blueprint). A drivable car's mesh
already holds its breakable pieces: `VH_OP_Aurora` carries 115 bit-0x10 drawcalls. No CAR / TRUCK /
TANK / APC template references a separate wreck mesh; `VH_OP_Aurora_wreck` is used only by a static
prop (`PROP_VH_NO_CR_Aurora_wreck_01`) and differs from the car outright (15,855 vs 24,127 verts, 5 bones,
no shared materials).

---

## 5. Buildings and world objects

**In-mesh destruction (the common case) — PROVEN.** Destructible world props are damage-group props
like any other. `GHotel_Balcony_Destruction` holds one pristine part (`VO_Bridge`) and 20 rubble parts;
the bunker walls do the same (`Bunker_WallCrvC_A_DAM`: 28 bones, 43 drawcalls, 40 with bit 0x10). Many
breakables ship **only** as a `_DAM` mesh (crates `P_Global_A_Crate_*_DAM`, lamp posts, benches,
bread/tobacco/crêperie stands) — the `_DAM` mesh *is* the whole object, intact pieces included.

**Big set pieces — PROVEN as assets.** Self-contained Dynamic0 packs named after their mesh:
`CMN_NOR_Large_Train_Bridge_DAM` (108 bones, 450 drawcalls; template `InitialHealth` 10000, no damage
groups), `VH_OP_Zeppelin_Destruction` (179 bones, 215/272 drawcalls with bit 0x10; templates `ZeppDest` /
`ZeppDestruction`, 6 groups `Zep_Dest_Section1..6`, 42 parts — all non-pristine), the
`OccMed_CivTrain_*CarDam` wrecks. `VH_OP_Zeppelin_MAIN` is a separate prop; no template links the two.
The Zeppelin sequence is a cinematic (`France\EditNodes\ZeppelinDestruction.wsd`, community
`all_containers.txt`).

**Separate damaged twins — see §6.**

---

## 6. Separately-modelled damaged meshes (`_DAM`, `_Broken`, `_Destroyed`, `D1..D4`)

Name families across all retail packs (names are a heuristic; per-mesh structure PROVEN):

| family | meshes | with an intact twin | examples |
|---|---:|---:|---|
| `_DAM` / `_Dam` token | 98 | 22 | `Bunker_*_DAM`, `MN_Mars_BuildingSTR_Dam`, `P_TrainBridgeDestructible_A_End_Dam` |
| glued `…DAM` / `…Damaged` | 14 | 10 | `MN_LaVillette_clockDamaged`, `PRaceCPRS_Sign12DAM` |
| `Broken` | 39 | 6 | `MN_LaVillette_CNRF01A_Broken`, `MoriniBarn_Column_Broken[A–M]` |
| `Destroyed` / `Destruction` / `Dest` | 4 / 2 / 5 | 2 / 0 / 0 | `FBuilding_WShed_Destroyed_A`, `PF_Boat_01_Destroyed` |
| `Wreck` | 19 | 1 | `VH_OP_Aurora_wreck`, `VH_OP_Zep_Wreckage1-6`, `OccMed_BomberWreck_*` |
| stages `D1..D4` | 8 | 4 | `MN_Plantes_grounds_D1..D4`, `LeHavre_Church_Wall_D1/D2` |
| `Burnt` | 3 | 0 | `MoriniBarn_Burnt` |

Not damage: `PSBridge_HS_Dam_*` (next to `PSBridge_HS_Powerplant_*` — a water dam, INFERRED) and the
shared debris library `PGA_DynDebris_*` / `OccMed_Debris_*` (Dynamic0 entry hash `385E25B4`).

**Twins compared (PROVEN per mesh):**

| damaged / intact | same streamblock sub-pack? | verts | drawcalls | bones |
|---|---|---|---|---|
| `Bunker_WallCrvC_A_DAM` / base | yes (Mega0 #220) | 1122 / 34 | 43 / 5 | 28 / 1 |
| `MN_Mars_BuildingSTR_Dam` / base | 2 of 2 cells | 4314 / 4387 | 13 / 13 | 1 / 1 |
| `P_TrainBridgeDestructible_A_End_Dam` / base | 7 of 7 | 5213 / 2506 | 23 / 12 | 3 / 3 |
| `GHotel_GFloor_O_Main_Dam_A` / base | yes | 1463 / 4 | 8 / 3 | 1 / 1 |
| `MN_LaVillette_CNRF01A_Broken` / base | 2 of 2 | 5300 / 1031 | 81 / 10 | 28 / 1 |
| `OccLt_Poster_N1_Dam` / base | 40 of 40 | 33 / 4 | 1 / 1 | 1 / 1 |
| `FBuilding_WShed_Destroyed_A` / base | no | 6443 / 2836 | 7 / 3 | 1 / 1 |

A twin is a complete separate model (usually heavier, mostly new material hashes) that ships in the same
world cell as its base — **so the game most likely places both and swaps them (INFERRED; placements in
`France.map` not decoded).** Only 9 such pairs are referenced by any template, and the only direct
template link is the unnamed Prop key **`0xd6cfed23`** (present on all 1,828 Props, non-zero on 9):
`OccLt_Poster_N1..3` → `OccLt_Poster_N*_Dam` (with `0x6fae76bf` = the intact mesh again), and the kiosks /
postcard stand / pissoir → `*_HWF` (meaning unknown). Lua hints at layer swaps —
`UnloadStaticENTag("GRANDPRIX_Pristine")`, `"PristineBarn_Building2"` — so world-layer tags may be the
mechanism (SPECULATIVE).

In-mesh LODs: only the four `OccMed_CivTrain_*CarDam` meshes and `VH_OP_Aurora_wreck` have an LOD1;
every building damage mesh is LOD0 only (PROVEN).

---

## 7. Characters

No damage groups and no damaged meshes for generic humans (PROVEN for templates and meshes). Burning and
blood are runtime effects (2008 `WSHuman::m_bOnFire`, `WSDecalManager`, `FX_BloodSmallA` — INFERRED).
Story variants are separate setups, not states:

- **Luc** — `FBS_RS_Luc_Blood` → `CH_AL_Luc_Blood_HD`: identical geometry to `CH_AL_Luc_Gaudin_HD` (bbox,
  166 bones, 2025 verts), 3 of 8 materials differ — a material variant (PROVEN).
- **Kessler** — `FBS_NZ_Kessler_Disguised` uses `FBP_NZ_Kessler_Blood_GR`.
- **Dierker** — `RndHuman_NZ_Dierker` picks `Human_NZ_Dierker_Burnt` (weight 1.0) or `Human_NZ_Dierker`
  (weight 0), but `FBS_NZ_Dierker_Burnt` is pair-for-pair identical to `FBS_NZ_Dierker`; the burnt head
  template `FBP_NZ_Dierker_HD_Burnt` is orphaned and its mesh `CH_NZ_Dierker01_Burnt_HD` is not in retail.
  The 2008 prototype had `CH_NZ_Dierker_Burn`. A cut feature (INFERRED).

---

## 8. Mercenaries 2 — a sibling, not a parent

Mercs 2 solves the same problem with the same idea in a different container: a building's UCFX
skeleton has shard nodes with `<shard>_pristine` / `<shard>_ruin` (and `_interior` under pristine)
children, and the state machine drives a per-segment state mask
(`mercenaries-game/docs/reverse_engineer/dialect_destruction.md`, `state_machine_destruction_code_map.md`).
The Saboteur keeps "geometry hangs off named nodes; state picks which nodes draw", but moves the
pristine/damaged split out of node names into a per-part `Pristine` flag in GameTemplates, uses
`VO_*` bone names, and has no named-state orchestrator — a group is 0 or 1. Concept shared; names,
containers and offsets are not. Recorded in [`lineage_and_divergence.md`](lineage_and_divergence.md).

---

## 9. Tooling consequences

- `sab_workshop` cannot load these meshes today (unskinned, multi-bone). Loading one as-is would draw
  the intact and damaged pieces on top of each other — the damage analogue of stacked LODs. The faithful
  export is one object (or glTF scene) per damage state per group, built from the parts' `Pristine`
  flags; bit 0x10 gives the same split for meshes without a template.
- An edited damageable mesh must keep its bone names (the parts address them by hash) and its
  per-bone drawcall split.

---

## 10. Unknowns / next

1. **World twins:** decode `France.map` placements to confirm intact/`_DAM` twins are co-placed, and find
   what swaps them (layer tags?).
2. **`0xd6cfed23`** — name, reader (decomp ~line 104276, setter via `FUN_004511d0`), and what `_HWF` is.
3. **Unnamed keys:** `0xe3eb3242` (group, compared with 2), `0x22650220`, `0x6dd070b1`, `0x03e19511`,
   `0x558c91d7`, `0x187b88f6`.
4. **REINSTANCE** (deferred type 0, carries an `OdinMesh` handle) and **`CREATEDYNAMICPART`** — whether
   either swaps a mesh or spawns a physics debris piece on PC.
5. **A second damaged level** — does anything write a group state above 1?
6. Component `vfn[0x54]` → `0x01302230` queue link (INFERRED above).

## Method (to regenerate)

- **Template side:** load the largest `AULB` block of `France/loosefiles_BinPC.pack` with
  `sab_formats::gametemplates`; walk each template's pairs in order — a `BoneName` pair starts a part,
  later `Pristine`/`DamageGroup` pairs fill it.
- **Mesh side:** index every MSHA in every retail megapack (`sab_formats::mesh::scan_msha`) by
  `pandemic_hash(name)`; resolve `Model`; parse the MESH body (bones at `244+44+nb+unk0+64·nb`, then
  48-byte RTS, i16 parents, remaps, streams, primitives, 16-byte drawcalls — the same walk as
  `sab_workshop::meshload::parse_mesh`, without its skinned-only gate).
- **Cross-check:** subtree of each part bone, not entering other parts' bones; tally drawcall bit 0x10
  by the part's `Pristine`.
- **Code:** compare chains via `objdump -d --no-show-raw-insn` of the GOG exe where Ghidra dropped the
  function; 2008 names/layouts from the prototype's `WildStar_d.map` + PDB (a Rust `pdb`-crate dump).
