# 2008 Xbox 360 prototype — `<hash>.materials` slot format

**Status:** solved for the record-level shape (name + texture-slot vector); post-texture
sub-blocks (aliases, render passes, constants, shader refs) are known-present but not needed
by the workshop yet and left unparsed.

**Evidence:** proven end-to-end against the two `Global/*.materials` slots on the current extract
whose packlog offset happens to land on raw WSAO bytes (12 out of ~717 slots today — see the
"scope" section below for why). Cross-referenced 20+ texture hashes against retail's
`France.materials` `XTSW` table; every one is present, confirming (a) the parser reads real
texture-name-hashes at the right offsets and (b) the 2008 hash namespace is identical to
retail's `pandemic_hash`.

**Impl:** `tools/sab_workshop/src/sources/prototype2008_material.rs`; wired into
`Prototype2008Source::textures_for_material` and the `hash_names` fallback.

## Where materials live

Retail keeps every material record in one loose `France.materials` (WSAO, LE) at the game root.
The 2008 build has no such consolidated file. Instead, each packlog block ships **one**
`<blockhash>.materials` slot inside its per-block `.Pack` container:

    File: D:\projects\wildstar\art\Cache\PackData\Xbox 360\Global\-1288447421.materials
    Start: 987255
    Size: 299

`-1288447421` is signed decimal for `pandemic_hash("Act1_IntKey")`. There are 863 such
`.materials` entries across the 1399 Global packlogs.

## On-disk layout (big-endian throughout)

```
WSAO file (one per .materials slot):
  char magic[4]   = "WSAO"       (retail is "OASW" LE — 2008 flips both magic and endianness)
  u32  num_records
  MTLD records[num_records]      (packed back-to-back, no per-record padding)

MTLD record:
  char magic[4]   = "MTLD"
  u32  uid                       (drawcall material hash — the key for
                                  textures_for_material)
  u32  body_size                 (bytes that follow, not counting these 12 header bytes)
  u8   body[body_size]:
    u32  name_len                (INCLUDES the trailing NUL — a 24-char name has name_len=25)
    char name[name_len]          (NOT 4-byte padded — subsequent u32s are unaligned reads)
    u32  num_textures
    u32  texture_hashes[num_textures]    (slot order [diffuse, spec, normal, wm])
    ... sub-blocks (render passes, constants, shader refs) — NOT parsed here ...
```

### Why this is not retail's WSAO with a different endianness

The magic is different (`WSAO` vs `OASW`), yes, but the interior is a wholly different
serialisation. Retail's OASW has **separate typed sub-blocks** — an `XTSW` array of every
texture hash in the file, an `AMSW` array of `WSMA` records that point INTO it by
`(tex_begin, num_tex)` slice indices, plus `APSW` / `TSSW` / `PCSW` / `PPSW` / `PVSW` for
passes/state/constants. The 2008 WSAO has **inline records** — each `MTLD` is
self-contained, texture hashes are stored right inside it, and there is no shared texture-hash
array. Retail's `MTLD` string does not appear anywhere in `France.materials` (checked); 2008's
`XTSW` / `AMSW` do not appear in any 2008 `.materials` (checked). Different formats that share
a family name.

### Slot ordering

The retail workshop's `wsao.rs` and the community engine-comparison doc both fix the slot
order as `[diffuse, specular, normal, wm]`. This impl assumes the same for 2008 — untested for
2008 specifically. Two independent props to verify once the SBL4 unwrap (probe #1) lets us see
enough materials to pick a known-good sample:

1. Take a material that has 3 textures and cross-reference each hash against a texture whose
   name suffix (`_D` / `_S` / `_N`) unambiguously identifies its slot.
2. Compare the 2008 record for a material name that also appears in retail's WSAO (mapping
   through material-name → aliases → retail WSMA record) — the retail slot vector is
   independently `[d,s,n,wm]` and should line up index-for-index.

### Aliases

Retail's `WSMA` record carries an `ids[]` array — mesh drawcalls can key on either the
primary uid or any alias, and both resolve to the same slot vector. The 2008 MTLD body
carries additional fields after the texture list that MAY be aliases (a bunch of
hash-shaped u32s in the same-position that retail keeps aliases). Not decoded here because
the workshop's picker doesn't need them yet — mesh drawcalls in the 2008 build come from
`.meshes` slots we can't unwrap either (again, probe #1). When that changes and drawcalls
start driving the lookup, a mesh whose material hash matches an alias will miss this index
and we'll need to add alias fold-ins. The parser's `MaterialRecord` shape leaves room for
this — just extend it and update the ingest to key every alias to the same slot vector.

## Sample record (`DefaultLib.GR_C2_metal:0`, from `Common.materials`)

```
0x008  MTLD magic
0x00C  uid          0x0373DF9E
0x010  body_size    0x0000011D  (285)
0x014  body:
0x014    name_len   0x00000019  (25 = 24 chars + NUL)
0x018    name       "DefaultLib.GR_C2_metal:0\0"
0x031    num_tex    0x00000002
0x035    tex[0]     0xE74F9B28   (retail-WSTX match: DIFFUSE)
0x039    tex[1]     0x183EA998   (retail-WSTX match)
0x03D    (sub-blocks follow: 3 render passes tagged 0x0ABCCFE8 / 0x236F5A8D / 0xB20202F9;
0x03D     each pass carries a per-slot texture-index + per-pass floats; skipped by this parser)
0x131  ← next MTLD record
```

(The parser reads `MaterialRecord { uid: 0x0373DF9E, textures: [Some(0xE74F9B28), Some(0x183EA998), None, None], name: "DefaultLib.GR_C2_metal:0" }` and moves on.)

## Distribution of slot sizes across the 863 material slots

The smallest is 258 bytes (`CollectionItems`, `NaziOfficerTag`, `SabotageItem`, `SniperScope` —
each an item slot with a minimal material set), the largest 14999 bytes (`Common` — 42 records)
followed by 7746 (`Saboteur` — the main character bundle) and 7351 (a repeated
skeleton-scale/infantry bundle referenced by 16 different packlogs). The 258-byte slots are
truncated on this extract; the 14999-byte one holds 42 fully-formed MTLD records.

## Scope of this probe vs the whole build

At extract time the packlog `Start:` offsets were written for the pre-SBL4-container layout of
each `.Pack`. When the container was later applied, some byte ranges were preserved in place
(giving us the 12 slots we can read raw today) and most were reshuffled or wrapped, giving us
gibberish at the packlog offset. The `MaterialIndex` counter split (`slots_ok` /
`slots_sbl4` / `slots_bad_magic`) tells the user exactly how much of the total probe #1
unlocks.

Empirically on the current extract:

| bucket                     | slots | note                                             |
|----------------------------|-------|--------------------------------------------------|
| `slots_ok` (WSAO parsed)   | 12    | 78 materials indexed today                       |
| `slots_bad_magic`          | 705   | packlog offset lands on high-entropy SBL4 payload |
| `slots_sbl4` (SBL4 magic)  | 0     | none literally begin with `SBL4` on this extract  |
| pack missing / truncated   | 146   | 0-byte or short `.Pack` files in the extract      |
| total per-slot manifests   | 863   |                                                  |

The parser itself is complete for the WSAO/MTLD shape — the number-of-materials-indexed
counter jumps from 78 to (thousands) automatically the moment probe #1 hands us un-wrapped
slot bytes.

## Not solved here (deliberately out of scope)

- Full MTLD sub-block schema (render passes, constants, shader refs). Needed for exact shader
  reproduction, not for the workshop's picker + Blinn-Phong preview + bundle export.
- Alias fold-ins into the material-uid index. Needed once mesh drawcalls start driving the
  lookup — parked pending probe #1 (mesh reads).
- SBL4 sub-pack unwrap. Owned by probe #1 in this batch.
- 2008 texture decode. Owned by probe #2 in this batch.
