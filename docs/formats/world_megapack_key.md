# World / startup megapack key (`France\Mega*`, `Start0.kiloPack`)

**Question:** what is the `path_crc` (megapack bsearch key) for the world/startup packs, so a modder can
register a *new* world asset the way `sab_megapack_key` does for `Global\Dynamic0`
(`pandemic_hash("global\<name>.dynpack")`)?

**Answer (2026-10-12):** for every `.megapack` it is `pandemic_hash` of the object's **pack path**, a
string the map loaders build at load time and the pack loader extends with the pack type. Checked on
every ALBS entry of every retail megapack (`sab_formats` `tests/sbla_retail.rs`; `sab_probe
albs-keys`). The three `.kiloPack`s are the exception: their `crc` matches no path form tried (see
[Open](#open)). This supersedes the earlier answer on this page ("a stored key baked by the offline
cooker"), see [Correction](#correction-2026-10-12).

## The pack paths — PROVEN

The map loaders store a path on each object with `FUN_006586c0(obj+0x18, fmt, …)` (the string pointer
lands at `obj+0x18+0x20`); the pack loader appends the extension (`FUN_00658870`:
`sprintf("%s.%s", *(this+0x20), "pack")`, the literals `"%s\%s"` at VA `0x0104bcc4` and `"pack"` at
`0x00fc4594` read from the exe) and the resolver `FUN_009ef620` hashes the result (`FUN_00db7e10`) and
bsearches the mounted megapacks on `crc`.

| object | path set by | path | extension appended by | retail entries |
|---|---|---|---|---:|
| world streamblock (terrain cell, `X ≥ 0xB4`; or `X == 0x3C`) | `FUN_009f5a10` `"%s\\%d"` (id < 10) / `"%s\\%s\\%d"` | `France\<id>` or `France\<first two digits of id>\<id>` | `.pack` (`FUN_00658870`) | 7,914 (Mega0/1/2, `dlc01mega0`) |
| `France.map` list 2 / list 3 record | `FUN_009f3bf0` / `FUN_009f3fa0` `"%s\\%s"` | `<region>\<name>` | `.pack` (`FUN_00658870`, `"%s.%s"` with `"pack"`) | 72 + 5 DLC |
| `global.map` dynamic | `FUN_009f2a00` `"%s\\%s"` | `Global\<name>` | `.dynpack` (`"%s%s"` with `".dynpack"` in `FUN_009f2370`, `FUN_009f2530`; the general dynamic load is not traced) | 761 |
| `global.map` list 2 record | `FUN_009f2df0` `"%s\\%s"` of `"%s%d"` | `Global\<name>1` | `.palettepack` (`FUN_009f1520`) | 319 |

* The streamblock `id` is the megapack entry's `index`. For `id ≥ 10` the middle component is the
  decimal id truncated to two characters (`sprintf(local_110,"%d")`, then `local_110[2] = 0`):
  id 1177344 → `France\11\1177344.pack`.
* `<region>` is the region name stored in the region map (`France` in both retail region maps).
* The `global.map` list records' names get the list number appended (`"%s%d"`): the stored record hash
  is `pandemic_hash(name + "0")` for list 1 (325/325) and `pandemic_hash(name + "1")` for list 2
  (325/325). Dynamics and region records hash the bare name.
* `pandemic_hash` folds ASCII case, so `Global\` and `global\` hash alike.

The library forms: `sab_formats::map6::streamblock_path(region, id)` and `map6::pack_counts` (the
object paths, keyed by hash, with each object's ALBS type counts).

## The `index` field

* streamblocks: the streamblock id (`France_Streamblock_1177344` → `index = 1177344`).
* dynamics and region records: `pandemic_hash(name)` = the record hash.
* `global.map` list 2 (Palettes0): `pandemic_hash(name + "1")` = the record hash.
* The 39 cinematic packs of `France\Mega0` (`102_cinb_farmintro` …): the leading number of the name
  (`0x66` = 102). No map record carries that value; the engine path above never reads `index`.
* kiloPack entries: the record hash of the object (105/105).

## Open

* **kiloPack `crc`.** `Start0`, `Start_German0` and `BelleStart0.kiloPack` (105 object entries, 14
  streamblocks): none of the path forms above, nor `<pack stem>\…`, `France\Global\…` prefixes,
  `name+digit`, or CRC-32 of the same strings reproduce the `crc`. The packs are mounted from
  `FUN_009ccb50` (`FUN_009f0ca0("France\\BelleStart0.KiloPack")`); how their entries are looked up is
  not traced.
* **Two `Global\Palettes0` entries with `X == 0x3C`**: their `crc` has no known path.

## Correction (2026-10-12)

This page used to answer "the preimage string is NOT in `Saboteur.exe` and is NOT reproducible from
pack data … a stored key baked by the offline cooker", reading the `"%s\\%s"`/`"%s%d"` calls near
`FUN_009f2df0` as entity *display names*. They are the pack paths: `FUN_00658870` formats
`"%s.%s"` from that same string and `"pack"` and passes it to the resolver. The streamblock test
`ph("France\1024.pack") ≠ 0x98D576ED` was right to fail — the path for id 1024 is
`France\10\1024.pack`. One name with several `crc`s (the "Aurora triple") fits the same name in several
map lists, each with its own path — `VH_OP_Coot` is in `global.map` list 1, list 2 and the dynamics
(the Aurora names themselves were not re-checked). `index` is not "a small ordinal for assets": it is the record hash for
dynamics, region records and palettes. `ALBS+0x08` is the hash of the sub-pack's first record
([sbla_subpack.md](sbla_subpack.md)), not a pack-level name hash.

## Confirmed earlier, still holds (decomp VAs)

- **`pandemic_hash` = `FUN_00dc1e20`** @0x00dc1e20 (null-term) / **`FUN_00dc1e60`** @0x00dc1e60 (len-limited).
  `hash("ANY")==0xED057225`.
- **Mount** = `FUN_00e34f70` @0x00e34f70; `France\Mega0.megapack` built by `sprintf("%s\%s0.megapack",
  worldDir, "France")` in `FUN_009f4320`. Index entry `{u32 crc, u32 index, u32 size, u64 offset}`,
  qsort'd + **bsearch on `crc` only** (`FUN_00e42740` @0x00e42740).
