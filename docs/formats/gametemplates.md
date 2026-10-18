# The Saboteur (2009) — `GameTemplates.wsd` (magic `AULB`) format

The GameTemplates file is the game's **object-definition layer**: a flat table of
named "templates" (a disguise use-point, a car, a weapon, a prop, a light setting,
a particle effect, …), each carrying a bag of `{property-hash → data}` pairs.
`global.map` / the world streamer reference these templates by name-hash.

Two files were used as ground truth:

| file | size | entries | note |
|------|------|---------|------|
| `DLC/01/GameTemplates.wsd` (loose) | 3969 B | 5 templates, 0 markers | small, no markers |
| main game, embedded in `France/loosefiles_BinPC.pack` @ **0x2733ED0** (dec 41107152) *(hex corrected 2026-07-24 — the old `0x2732C50` disagreed with the decimal; `AULB` occurs exactly once in the pack, at `0x2733ED0`)* | 8 204 431 B | 11072 entries = 10761 templates + 311 markers | full object DB |

Both parse with **exact byte consumption** and re-emit **byte-identical** (SHA-256 match).

The 2008 Xbox 360 prototype's `GameTemplates.wsd` is the same layout big-endian, magic `BLU2`:
[2008_templates_and_text.md](../2008_templates_and_text.md).

---

## Loader (decomp oracle)

Boot calls `thunk_FUN_0162bfa0("%s\\GameTemplates.wsd", 0)` at decomp line 823978/823979.
The real loader is **`FUN_0162bfa0` @ `0x0162bfa0`** (decomp line 1756841). It:

1. opens the file (`FUN_00dc1060`), gets its size (`FUN_00dc13a0`), allocates
   (`FUN_00db3a30`) and reads the whole thing into memory (`FUN_00dc14e0`);
2. reads the header via `FUN_00463b00(&count)` — **`FUN_00463b00` @ `0x00463b00`** is a
   generic *"read little-endian u32, advance cursor 4"* helper (line 56856:
   `*param_2 = *(u32*)(base+cursor); cursor += 4;`);
3. loops per entry, and per pair calls `FUN_0045f440` — **`FUN_0045f440` @ `0x0045f440`**
   — a *"copy NUL-terminated string, advance cursor past the NUL"* helper (line 54222:
   `cursor += len + 1;`). This is why strings are stored with a trailing NUL.

All integer reads are native x86 `*(u32*)` → **little-endian**.

---

## Byte layout

```
Header
  char   magic[4]     = "AULB"            @0x00
  u32    entry_count                       @0x04   (LE)  -- includes markers
  entry_count × Entry                      @0x08 …         (contiguous, no padding)
```

### Entry = Marker | Template

Peek the entry: if the next 12 bytes are `08 00 00 00 00 00 00 00 00 00 00 00`
it is a **Marker**; otherwise it is a **Template**. A Marker is a degenerate record
(`total_size == 8`, body = two zero u32s); no real template can have `total_size == 8`,
so the discriminator is unambiguous. A Marker **still consumes one `entry_count` slot**
(10761 + 311 = 11072). Markers act as group separators; the DLC file has none.
This is the `peek(12)=={8,0,…}` case the community "GameTemplates-Helper" special-cases.

### Template record

| off (rel) | field | type | meaning |
|-----------|-------|------|---------|
| +0 | `total_size` | u32 LE | bytes of this record **after** this field. `4 + total_size` = full record. |
| +4 | `unk1` | u32 LE | always `0` observed |
| +8 | `unk2` | u32 LE | always `1` observed (`0` inside markers) |
| +12 | `name_len` | u32 LE | `strlen(name)+1` (includes NUL) |
| +16 | `name` | u8[name_len] | ASCII + trailing `\0` |
| … | `type_len` | u32 LE | `strlen(type)+1` |
| … | `type` | u8[type_len] | ASCII + trailing `\0` |
| … | `pair_count` | u32 LE | number of property pairs |
| … | `pair_count × Pair` | | |

Verified on template 0 of the DLC file: `name="UsePt_BrothelHat"` (@0x18),
`type="AIAttractionPt"` (@0x2d), `pair_count=81` (@0x3c), `total_size=1042` which
equals exactly the bytes from offset 0x0c to the end of the record.

### Pair

| field | type | meaning |
|-------|------|---------|
| `hash` | u32 **LE** | `pandemic_hash(property_name)` |
| `data_size` | u32 LE | length of `data` |
| `data` | u8[data_size] | value (see below) |

**Endianness of the hash — resolved.** The engine reads the hash with the same
LE u32 reader as everything else. Interpreting the 4 hash bytes **little-endian**
matches `pandemic_hash` of real property names; big-endian matches nothing:

| stored bytes | LE value | pandemic_hash of |
|--------------|----------|------------------|
| `24 c8 e5 1d` | `0x1de5c824` | `"Name"` |
| `50 42 72 5b` | `0x5b724250` | `"Model"` |
| `19 90 51 87` | `0x87519019` | `"Priority"` |
| `a0 89 25 98` | `0x982589a0` | `"Offset"` |
| `24 3a 47 92` | `0x92473a24` | `"AIAttractionPt"` |

(0 big-endian matches over all 325 DLC pairs.) The Helper's "big-endian" is a
*display* choice for the reversed bytes, **not** the value the game uses.

### `pandemic_hash`

FNV-1a variant, case-folded via `|0x20`, with a `^0x2A` finalize:

```
h = 0x811C9DC5
for c in bytes(s):  h = ((c | 0x20) ^ h) * 0x01000193   (mod 2^32)
h = (h ^ 0x2A) * 0x01000193                              (mod 2^32)
```

Self-test: `hash("ANY") == 0xED057225`, `hash("none") == 0x4FF9F863`.

### Pair `data` payloads

`data` is untyped bytes; the meaning depends on the property. Observed in the DLC file
(data-size histogram: `1B×111, 4B×198, 5B×3, 8B×6, 12B×3, 21/24/25/28B×1`):

* **1 byte** — bool/enum (`0`/`1`).
* **4 bytes** — one of: `u32`/`int`, `float32`, or **another `pandemic_hash`**.
  Confirmed hash-valued data: `0x4FF9F863` = `"none"` (28×, an explicit null-ref),
  and cross-references to sibling templates by name-hash —
  `0xFB5E3070` = `"KnifeThrow"`, `0x8F4F6379` = `"CP_Burlesque_Stool"` (both are
  templates in the same file). Floats look normal, e.g. `0x3C23D70A` = `0.01`,
  `0x42480000` = `50.0`.
* **larger** — packed structs (vectors, colors, curves) — property-specific, opaque here.

---

## What a template looks like per `type`

`type` is a free ASCII class name. 108 distinct types in the main file. Top types and
some gameplay-relevant ones:

```
1828 Prop          914 ParticleEffect   757 ParticleEffectSpawner  736 LightSettings
 690 FxHumanBodyPart 527 AIAttractionPt  459 Spore     402 Sound     357 Human
  92 Weapon          82 Explosion        63 Ammo        60 CAR        …
```

* **Vehicle** definitions: `type=CAR` (e.g. `SmallCar`, `LimoCar`, `MilitaryCar`, 345
  pairs each) plus the `VirVehicle*` family (`VirVehicleEngine`, `VirVehicleChassis`,
  `VirVehicleWheel`, `VirVehicleTransmission`, `VirVehicleSetup`), `VehicleCollision`,
  `VehicleWheelFx`.
* **Disguise / use-point** definitions: `type=AIAttractionPt` (e.g. `UsePt_BrothelHat`,
  `UsePt_Brothel`), `PairedAttrPt`.
* **Weapons**: `type=Weapon` (`Artillery`, `RocketLauncher`), `MeleeWeapon`, `Ammo`.
* **Characters**: `Human`, `FxHumanBodyPart`, `FxHumanHead`, `Player`.

A modder edits a definition by locating its template (by name) and rewriting the
`data` of the pair whose key is `pandemic_hash("<PropertyName>")`.

---

## Characters: body setups, slots, and LOD

A character is an `FxHumanBodySetup`. Its pairs are a **stream** that
`WSFxHumanBodySetupBlueprint::SetProperty` (`0x515c40`, GOG exe; objdump — Ghidra did not recover
it) replays in order:

| key | name | effect |
|---|---|---|
| `0x79691A2F` / `0xDB9C23DC` / `0x9E288655` / `0xFC20CB8A` / `0x1E233FA7` / `0x37A9525D` | `UpperBody` / `LowerBody` / `GearA` / `GearB` / `GearC` / *(unnamed)* | open slot 0–5 with this many variations |
| `0x922CC098` | `FxHumanBodyPartVariation` | start the next variation in the current slot |
| `0xA1757F0A` | `BodyPart` | the variation's `FxHumanBodyPart` template |
| `0xD0541403` | `BodyPartProbability` | the variation's weight (float) |
| `0x44C27FCB` | `Head` | head variation count |
| `0xC57D2B43` | `FxHumanHeadVariation` | start the next head variation |
| `0x622DD842` | `Face` | the variation's `FxHumanHead` template |
| `0xD594DB5B` | `FaceProbability` | the head variation's weight (float) |

All 293 retail setups replay with no out-of-order entry. A slot's variations are **alternatives**:
every retail weight is 1.0, and the lists hold things no body wears together — `FBS_CV_Elderly_F`'s
UpperBody is a trench coat, student top, sweater or flower dress; its GearB is one of four scarves
or `FBP_A_CF_NONE` (`CH_CM_None_GR`). So one figure is one variation per slot. (INFERRED from that
structure and content; the spawn-time draw itself was not traced.)

Each `FxHumanBodyPart` / `FxHumanHead` then names its meshes by level of detail —
`WSFxHumanBodyPartBlueprint::SetProperty` (`0x514bd0`) stores `HighDetailModel 0x62404569`,
`LowDetailModel 0xD9725C55` and `CombinedLODModel 0xCBA8D62C`, and its resolve step (`0x5192f0`)
looks each up as an asset. Retail: `LowDetailModel` is the High mesh's `_LOD` twin for 456 parts and
the same mesh as `HighDetailModel` for 106 (Sean's parts, for one); `CombinedLODModel` is almost
always 0. A few `LowDetailModel`s name a `_LOD` mesh that is not shipped (e.g.
`CH_NZ_Helmet_WM_GR_LOD`). `sab_workshop` assembles characters exactly this way —
[`assets.rs`](../../tools/sab_workshop/src/assets.rs).

---

## Damage groups (props, vehicles, weapons, …)

856 templates (Prop 677, CAR 53, …) carry a damage stream in the same flattened-list style: a
`List 0x1AAC9531` marker naming `DamageGroups`, one `DamageGroups 0x49D5C2AE` entry per group
(`GroupName 0xC8D0F617`, …), then a `PartList`, one part per `BoneName 0x207FBA52` with
`DamageGroup 0xDD2AA7AB`, `Pristine 0x86DE6639`, health and effect keys. Parts name bones in the
template's `Model` mesh; the group's state (0/1) shows the pristine or the damaged parts' drawcalls.
Full key table, engine path and data counts: [damage_and_destruction.md](../damage_and_destruction.md).

---

## Texture / icon references (RE-2 — how a template names a texture)

A pair whose 4-byte `data` is a `pandemic_hash` can reference a **texture**: the value is
`pandemic_hash(textureName)`, and that is the *same* hash that keys the texture inside its `ALBS`
bundle directory and (for world materials) the `WSTX` table in `France.materials`. So a template
parameter → texture resolves straight through:

```
template pair { property_hash, data=pandemic_hash(textureName) }
   → ALBS sub-pack directory hash  (== pandemic_hash(textureName))   in Dynamic0/Palettes0.megapack
   → DTEX record  (plaintext name; name-hash == the value)
```

**Proven end-to-end.** Foliage template's `Texture` value `0xB52BF31A` == `pandemic_hash("FO_PT_Oak01_AB")`,
a real DTEX in `Global/Palettes0.megapack` sub-pack #92; `0x26CDA9C7` == `pandemic_hash("Foliage_City_Grass_AB")`
in the same bundle. Method: score every 4-byte property value against the set of ALBS directory hashes
(harvested uncompressed from all sub-packs) + the `WSTX` array; texture properties light up.

Reversed texture-carrying properties (from the 10 761-template main DB):

| property hash | name | uses | texture-hash hits | notes |
|---|---|---|---|---|
| `0xF011157A` | **`Texture`** | 332 | 182 (Foliage/LightHalo/Decal) | generic texture ref; values in WSTX (world) + Palettes0 (shared) |
| `0x7172B7AE` | *DecalTexture?* | 39 | 39/39 (WSTX) | only on `type=Decal`; not yet name-reversed |
| `0x62404569` | **`HighDetailModel`** | 889 | 705 (Dynamic0) | a MESH name-hash, not a texture — see [Characters](#characters-body-setups-slots-and-lod) |
| `0xD9725C55` | **`LowDetailModel`** | 889 | 689 (Dynamic0) | a MESH name-hash, not a texture — see [Characters](#characters-body-setups-slots-and-lod) |
| `0xEE7D0BA3` / `0x1DCCDB2F` | *?* | 199 | 129 / 127 (Dynamic0) | second object-texture pair |

Also reversed (non-texture) from the histogram: `Type 0x28E10525`, `LOD 0xA5FBBC36`,
`Color 0x3FF4F976`, `Face 0x622DD842`, `Head 0x44C27FCB`, `Skin 0x8410E4D6`,
`Description 0xCE1A4A79`, `Image 0x18D7B670`.

### The property table — 835 of 2 723 keys named ★

The hand-listed names above have been superseded by a derived table:
[`data/gametemplates_properties.tsv`](../../data/gametemplates_properties.tsv), generated by
[`tools/derive/derive_blueprint_properties.py`](../../tools/derive/derive_blueprint_properties.py),
whose types are compiled into `sab_formats::gametemplates::property_type`. Names
(`known_property_name`) now come from the shared table [`data/hash_names.tsv`](../../data/hash_names.tsv)
([hash_names.md](../hash_names.md)), which takes this file's names as one of its sources and names
1,474 of the 2,724 property keys.

Retail carries **2 723 distinct property keys over 644 154 pairs**. **835 now have a name, covering
41.3% of all property uses** — up from 13, which covered about 1%.

**Acceptance rule: `pandemic_hash(name) == key`, strictly little-endian.** Nothing else is carried.
There is no "probable" tier: a name either reproduces its key or it is discarded to the harvester's
reject list. The test `every_property_name_is_a_preimage` re-hashes all 835 rows, so a bad row
cannot survive `cargo test`. This is deliberately stricter than
[`editnodes.md`](editnodes.md)'s two-byte-order rule, which existed to *rescue* a mangled
third-party dictionary — here we generate our own preimages, so we can demand the exact match.

> **Correction (2026-10-12).** A preimage is not proof of the name. `pandemic_hash` is 32 bits, so a
> wrong string reproduces a key with probability ≈ keys / 2³², and the harvest above tried millions of
> fragments. Two rows are visibly chance hits — `0x4D165B66` `72b20` and `0x430217A0`
> `8bcc10 f   AkAudioEngine` (a linker-map line) — where SabTool has `Back Light Falloff` and
> `Spotlight Intensity`; and `0xE655DC80` has two candidates, `_MinAirResistanceSpeed` (here) and
> SabTool's `Fwd Fric HandBrake Scalar`. [hash_names.md](../hash_names.md) estimates each name's
> chance of being a collision and ranks the alternatives.

Sources, in descending preference for *spelling* (all hash-verify, so all are correct up to case —
`pandemic_hash` case-folds):

| source | keys | share of pairs |
|---|---|---|
| the community `GameTemplates-inspector` dictionary, hash-filtered | 146 | 15.5% |
| `Saboteur.exe` printable strings, suffix-aware | 206 | 8.2% |
| the 2008 prototype symbols (`WildStar_d.pdb`, `.map`, `Saboteur.pdb`) | 483 | 17.6% |

**★ Property names can contain SPACES.** `0x246DB06C` is `"Game Event Name"`. Identifier-shaped
harvesting (`[A-Za-z_][A-Za-z0-9_]*`) structurally cannot produce it, which is why the community
dictionary reaches keys the much larger symbol sweep cannot. That dictionary is *not* trustworthy as
a dictionary — 125 of its 298 rows (42%) verify in neither byte order, and its byte-order convention
is internally inconsistent — but hashing recovers the genuine rows from it. It even holds this one
twice, both broken: once as a spaceless guess `"GameEventName"` on the correct key, once with the
correct name on the byteswapped key `6cb06d24`.

**Corroboration.** The engine's universal attachment mixin `FUN_00662530` decodes a 12-byte record
array purely structurally; the recovered names describe the same thing independently:

| key | decomp | name |
|---|---|---|
| `0x42C6DA9D` | count → allocates `count × 12` | `Sound Events` |
| `0x02D3EB9F` | advances the record index | `AudibleEvent` |
| `0x246DB06C` | record `+0` : hash | `Game Event Name` |
| `0xD76D7747` | record `+4` : hash | `Sound Event Name` |
| `0x1E59E605` | record `+8` : u8 | `Looping` |
| `0x85B86BD7` / `0x212A9DB0` | bool bitfields | `Send Doppler` / `Send Distance` |

Two independent methods reconstructing one 26 000-pair audio block.

### Value types — derived, display-only

`property_type(key) -> PropType` covers all 2 723 keys: `Bool` 341, `Scalar4` 1 922, `Ref` 244,
`Vec3` 69, `Str` 4, `Bytes` 143. It comes from the ~130 per-class virtual
`SetProperty(u32 key, Cursor*)` dispatchers, whose reader helper names the type —
`FUN_00450f20` plain 4 bytes, `FUN_004511d0` 4 bytes byte-reversing (a CRC/ref),
`FUN_00451290` 12 bytes, `FUN_0045f440` NUL-terminated string, inline single-byte reads for flags.

A type is only marked corroborated (`type_src=decomp+size`, **755 keys**) when that reader agrees
with the size histogram measured over all 10 761 templates; the rest rest on the corpus shape alone.
**This is display and authoring assistance only, never framing** — the pair header's `size` stays
authoritative, per [`editnodes.md`](editnodes.md)'s account of a value-based classifier that became a
live parse bug.

**`size` is a skip length, which makes width load-bearing for editors.** The pair loop
`FUN_004614a0` calls the class's virtual `SetProperty(key, cursor)`; when it returns false — an
unknown key — it advances the cursor by `size` and carries on. So the engine tolerates keys it does
not know, which is why leaving an unrecognised pair untouched is safe. The corollary is the
authoring rule: **an edit must preserve the pair's byte width**, because a one-byte flag written back
four bytes wide shifts every later pair in that record. Only `Str` may legitimately resize.
`sab_workshop`'s `encode_value` enforces this and refuses the edit otherwise; 177,413 of the 644,154
retail pairs are not 4 bytes wide.

**Known limit.** `Scalar4` vs `Ref` is not always separable. Ghidra emits inverted guards —
`if (param_2 != 0xKEY) { …another key's body… }` with the key's own reader *after* the closing
brace — so attributing readers by a forward window from a `!=` site mis-types. Attribution is
therefore restricted to `== 0xKEY` sites, which costs coverage (893 → 755 corroborated) but removes
false claims. The size histogram cannot substitute here: both helpers consume 4 bytes.
`Game Event Name` is the worked example — genuinely a `Ref`, conservatively recorded as `Scalar4`.
Brace-aware extraction would recover these.

**To wire a custom icon/texture from a template:** pack the DTEX under a name (`sab_dtex`/`sab_poc`),
then set the relevant texture property's 4-byte value to `pandemic_hash(thatName)` (via
`sab_gametemplates set-pair … --data <hash LE>`). The engine finds it by that hash. The exact property
depends on the template `type`; the editor surfaces any hash-valued pair whose value resolves to a
known texture as a texture reference.

## Confidence

| claim | status |
|-------|--------|
| magic `AULB`, `u32 entry_count`, LE throughout | **CONFIRMED** (bytes + `FUN_00463b00`) |
| record = total_size, unk1, unk2, len-prefixed+NUL name & type, pair_count, pairs | **CONFIRMED** (exact-consume both files; `FUN_0045f440` string reader) |
| `total_size` excludes its own 4 bytes | **CONFIRMED** (measured == stored, all 10761 template records) *(corrected 2026-07-24: was "10766")* |
| pairs = `{u32 hash LE, u32 size, bytes}` | **CONFIRMED** (bytes) |
| hash is `pandemic_hash(name)`, read **little-endian** | **CONFIRMED** (5 exact name matches, 0 BE matches) |
| 4-byte data may itself be a `pandemic_hash` ref (`"none"`, sibling templates) | **CONFIRMED** (matches) |
| Marker = `08 00 00 00`+8 zero bytes, counts as one entry, group separator | **CONFIRMED** (311 markers, 10761+311==11072==count, exact-consume) |
| `unk1` always 0; `unk2` always 1 (0 in markers) | **INFERRED** (constant across all records; role unknown) |
| ~~per-`type` payload schemas (which hash = which field beyond the 5 named)~~ | **SUPERSEDED 2026-07-29** — the count was already stale (13 names, not 5). See the two rows below |
| **835 of 2 723 property keys named, 41.3% of all pair uses** | every row satisfies `pandemic_hash(name) == key` LE — but a preimage can be a chance hit (Correction above); 1,474 keys are named in [hash_names.md](../hash_names.md)'s table, each with its estimated chance |
| property names may contain SPACES (`0x246DB06C` = `"Game Event Name"`) | **CONFIRMED** (preimage) — and it explains why identifier-only harvesting plateaus |
| value type per key (`Bool`/`Scalar4`/`Ref`/`Vec3`/`Str`/`Bytes`) | **PARTIAL, display-only** — 755 of 2 723 corroborated by the engine's reader helper agreeing with the observed size histogram; the rest from corpus shape alone. `Scalar4` vs `Ref` is not always separable (inverted-guard limit, above) |
| per-`type` (per-class) property *schemas* — which keys belong to which `WS*Blueprint` | **OPEN** — the ~130 `SetProperty` dispatchers hold this and the key→function attribution is already recorded in the TSV's `dispatchers` column; turning it into an ordered per-class field list is not done |
