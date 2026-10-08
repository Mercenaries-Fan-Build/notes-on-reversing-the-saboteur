# Task: developer-chooseable model export formats + reassembly detail

**Status**: Design-only. Not started. Discussed 2026-09-29.

**One-line goal**: Give the Workshop's model exporter a chooseable output format
(`.glb` today; want optional `.obj`, `.smd`, `.dae`, FBX) and a chooseable
"reassembly detail" level that widens what the `raw/` preservation slice contains.

## Confidence key used below
- **PROVEN** — read from the repo or a doc this session; cite is verifiable.
- **INFERRED** — reasoned from what was read, not directly stated.
- **SPECULATIVE** — general knowledge or opinion, no cite behind it.

Do not promote INFERRED/SPECULATIVE to fact without checking. Effort estimates
are all SPECULATIVE unless a crate is named.

---

## 1. What the exporter does today (PROVEN)

Single hardcoded exporter in [tools/sab_workshop/src/bundle.rs](../../tools/sab_workshop/src/bundle.rs).
Emits a directory shaped like:

```
<label>/
  raw/*.msha         # byte-exact source records
  textures/*.png     # decoded bound skins
  model.glb          # editable bind-pose glTF 2.0 (one primitive per submesh)
  manifest.json      # reassembly map (bindings, provenance, bone table, full pool)
```

Key facts:

- **No writer trait, no format enum.** `write_glb` at
  [bundle.rs:305-582](../../tools/sab_workshop/src/bundle.rs#L305-L582) builds
  glTF JSON + BIN inline from `LoadedMesh`.
- **No `gltf` crate dep** — Cargo.toml lists only png/serde/flate2/etc. The GLB
  is hand-rolled.
- **`raw/*.msha` is a NARROW slice**: [meshload.rs:197-200](../../tools/sab_workshop/src/meshload.rs#L197-L200)
  returns `buf[e.file_off .. e.file_off + 276 + comp0 + comp1]`. That is
  ONE part's MSHA (276-byte header + zlib blob0 + zlib blob1). Bind skeleton
  survives because it lives inside blob0 ([meshload.rs:273](../../tools/sab_workshop/src/meshload.rs#L273)).
  It does NOT include ANM0 clips, WSAO/WSMA records, or the SBLA sibling slots.
- **No animation is written to disk anywhere in the crate.** Grep confirmed;
  only `fs::write` calls are the four files above.
- **WSAO slots are variable-length** ([wsao.rs:52](../../tools/sab_workshop/src/wsao.rs#L52)
  reads `num_tex` per record). The bundle exporter truncates to 4 via
  `.take(SubmeshBinding::NUM_SLOTS)` at
  [app.rs:1373](../../tools/sab_workshop/src/app.rs#L1373). No evidence in-crate
  of `num_tex > 4` in real records — the layout permits it, unclear if it happens.

## 2. Prior art in the Mercs 2 ancestor (PROVEN)

The bundle docstring says the sab exporter was ported from
`mercs2_workshop`'s `bundle.rs`. That ancestor lives at
`C:/Users/Shadow/Desktop/notes-on-the-released-game/tools/wad_simulator/crates/mercs2_workshop/`
and has **more** than one format path:

- **`bundle.rs::export_bundle`** — writes SEPARATE `model.gltf` + `model.bin`
  (not a single `.glb`), and **includes animation clips**
  (`bundle.rs:952`: `if !animations.is_empty() { gltf["animations"] = … }`;
  signature takes `clips: &[ClipAnim]`). Also emits `raw/*.ucfx` per LOD rung.
- **`app.rs::export_model_data`** — a separate **OBJ + MTL** exporter
  (`app.rs:8143-8219`; writes `model.mtl` + `model.obj` + PNGs).

Neither uses a writer trait or format enum. They're two hand-rolled functions
selected by caller. The sab port dropped BOTH the animation path and the OBJ path.

**Take-home**: OBJ and animation-in-glTF are not novel work — there is a
reference in the family we can lift structure from.

## 3. What MESH fields are unmapped (PROVEN, from our own docs)

The bundle's docstring at [bundle.rs:6-10](../../tools/sab_workshop/src/bundle.rs#L6-L10)
names four things we don't fully reverse. Cross-checked:

1. **SMSH stream/format table beyond decoded vertex semantics** — PROVEN unmapped.
   `mesh_geometry.md:140` marks `Stream.unk0 @136`, `Primitive.numFaces`,
   `DrawCall.unk` as INFERRED — "present & read, exact meaning unused."
2. **`parentBone` on pre-v2 files** — **the docstring is misleading.**
   Source MESH `DrawCall.parentBone` IS documented ([mesh_geometry.md:80](../../docs/formats/mesh_geometry.md#L80)).
   The loss is our OWN `SMSH v1` intermediate ([tools/sab_workshop/src/formats.rs:38-39](../../tools/sab_workshop/src/formats.rs#L38-L39))
   discarding it. Fixable by keeping v2 always.
3. **Per-drawcall LOD/damage material passes** — PROVEN unmapped.
   `archive_and_models.md:157` says "LOD/damage-state handling not yet
   observed in the toolset (TBD)". The "per-drawcall" phrasing in the
   docstring is a GUESS — docs don't pin it there.
4. **Material hashes we can't resolve to names** — PROVEN.
   `mesh_geometry.md:78` DrawCall `material` = StringHash → external
   .materials / WSAO, "not resolved in v1".

**Additional unmapped fields the docstring doesn't mention** (all from
`docs/formats/skeleton.md` and `mesh_geometry.md`):

- MESH header `unk0 @184` (u32)
- Skeleton block `numUnkBones0` / `numUnkBones1` (function unknown, controls padding)
- `Bone.unk0 @28` (always 0 across all 168 Sean bones, still unnamed)
- `BoneRemap.ibm` (engine recomputes; write-side authority unclear)
- ~100 bytes of `N × u32 = 0` padding in the header with no semantic claim

## 4. Havok collision is NOT in the mesh (PROVEN)

Grep of `docs/`, `docs/formats/`, and the workshop crate turned up ZERO hits
tying Havok collision shapes to MESH bytes. Physics is a **peer SBLA slot**,
not embedded:

- [sbla_subpack.md:5](../../docs/formats/sbla_subpack.md#L5): "one or more
  compressed blocks (an `MSHA`/`AHSM` mesh, `DTEX` textures, **physics**,
  layout…)"
- 2008 slot enum: `physics` hash is a peer of `meshes` (`docs/2008_subpack_format.md:48`).

**Implication**: preserving collision means widening the bundle's `raw/` from
"the mesh MSHA" to "the whole SBLA". That's a scope change, not a writer.

## 5. Rust crates already in-tree (PROVEN)

Surveyed every `Cargo.toml` and `Cargo.lock` under `tools/` in both this repo
and the mercs2 sibling.

- **No crate anywhere writes OBJ, SMD, DAE, or FBX.**
- `gltf` crate — direct dep of `mercs2_workshop` and `mercs2_formats`, READ-only
  usages. `sab_workshop` doesn't even depend on it.
- Transitive-only (would need promoting to direct deps): `quick-xml` and
  `xml-rs` (could underpin a DAE writer), `byteorder` (helpful for FBX binary).
  All pulled in by egui/naga in `wad_simulator`, not present in sabetour lockfiles.

**Bottom line**: any new format = new dep OR new hand-rolled code. No freebie.

## 6. Format landscape (PROVEN from crates.io/docs.rs Sept 2026)

| Format | Skinning | Rust writer | Effort if hand-rolled |
|---|---|---|---|
| OBJ + MTL | **no** (hard) | **`wavefront_rs` v1.0.4 (Jul 2026)** writes both | trivial, ~200 LOC |
| SMD (Valve) | yes | **none** on crates.io | ~1 day, plain text |
| Collada .dae | yes + IBMs | **`dae-parser` v0.11.0 (Sep 2026)** writes it, actively maintained | N/A |
| FBX ASCII | yes | **none** — `fbxcel` refuses ASCII explicitly | 3–7 days, undocumented grammar |
| FBX binary | yes | **`fbxcel` v0.9.0** writes the container, NOT the scene DOM | 1–2 weeks on top |

Fact-checks worth locking in:
- "FBX ASCII is unofficial" — the ENTIRE FBX format is undocumented by
  Autodesk, not just ASCII. The Blender RE post
  (`code.blender.org/2013/08/fbx-binary-file-format-specification/`) is the
  practical reference for binary; ASCII has no equivalent.
- "OBJ can't skin" — confirmed hard. Community "extended OBJ with skeletal"
  proposals exist; no DCC or engine consumes them.

Sources cited by the survey subagent:
- crates.io: `fbxcel`, `fbxcel-dom`, `dae-parser`, `collada`, `wavefront_rs`, `wobj`
- Wikipedia: FBX, Wavefront .obj
- Khronos: COLLADA 1.4.1 / 1.5.0
- Valve Developer Community wiki: Studiomdl_Data (SMD grammar)

## 7. Design sketch (SPECULATIVE — this is the plan, not a decision)

**Step 1: writer trait + format enum (behavior-neutral refactor).**

```rust
enum ExportFormat { Glb, ObjMtl, Smd, ColladaDae, FbxAscii, FbxBinary }

trait ModelWriter {
    fn extension(&self) -> &'static str;
    fn write(
        &self,
        lm: &LoadedMesh,
        submeshes: &[SubMesh],
        submesh_tex: &[SubmeshBinding],
        written_tex: &[WrittenTex],
        label: &str,
    ) -> Result<Vec<u8>, String>;
}
```

`Job::formats: Vec<ExportFormat>`. `bundle::run` iterates, writes each output
next to the shared `raw/`, `textures/`, `manifest.json`, and adds a
`model_files: [...]` array to the manifest so downstream tools discover what
was actually written. Default is `[Glb]` — zero user-visible change.

**Step 2: add DAE via `dae-parser`.** Cheapest rigged option — one new dep,
one Writer impl, done. Actively maintained, ships write support, known
skinning shape.

**Step 3: port OBJ+MTL from `mercs2_workshop::export_model_data`.**
Structure exists; adapt to the new trait. Even without skinning it's useful
for artists doing texture reference work.

**Step 4: add animation channels to the GLB writer.** Prior art in the
ancestor's `bundle.rs` (`animations` node, `ClipAnim` param). Clip decode is
already in-crate via [clip.rs](../../tools/sab_workshop/src/clip.rs) and
[havok.rs](../../tools/sab_workshop/src/havok.rs). This is orthogonal to the
new writer trait but does need the trait to expose `clips` too — probably
extend the `Job` first.

**Step 5: SMD if anyone asks.** ~1 day hand-roll, text format. No crate.

**Step 6: FBX ASCII last if at all.** Real cost. Most people wanting FBX
just want it to open in Blender — glTF already does that. Escalate only on
a specific request.

**Step 7 (independent): `DetailLevel::Full`.** Not a writer change — a
preservation-scope change. Widen `raw/` from "one part's MSHA" to:

- The whole SBLA subpack the mesh came from (includes physics/layout peers)
- The WSAO/WSMA row(s) the mesh's materials bind against (record bytes, not
  just hashes)
- The referenced ANM0 clips from `Animations.pack`

Bump `bundle_version` in `manifest.json`. This is the actual "carry
reassembly details" answer — the writer format is a separate axis from
preservation scope, and conflating them was a mistake in the first framing.

## 8. Things worth verifying before starting

- Does any observed WSAO record carry `num_tex > 4`? Grep the actual
  `France.materials` histogram. If yes, widen `NUM_SLOTS`.
- Does `dae-parser` v0.11 actually round-trip skinning against Blender import?
  Write one test with Sean's bind pose before committing to the choice.
- Is the ancestor's animation-in-glTF path clean, or did it have known bugs?
  Check `mercs2_workshop` git log / issues before porting.

## 9. Standing-rule callouts (from repo AGENTS.md)

- Bundle preservation rule: **nothing is discarded**. Any new format the user
  picks is IN ADDITION TO `raw/`, never a replacement.
- No silent fallback if a format writer fails — hard error on that format, keep
  the others.
- The confidence labels above should stay: it took two rounds of the user
  challenging me to actually verify claims. Don't drop them.

## 10. Files touched in the eventual PR (SPECULATIVE)

- `tools/sab_workshop/src/bundle.rs` — split into `bundle.rs` (orchestration) +
  `writers/mod.rs` + `writers/glb.rs` (the existing writer, unchanged behavior).
- `tools/sab_workshop/src/writers/{dae,obj_mtl,smd}.rs` — new writers per step.
- `tools/sab_workshop/Cargo.toml` — add `dae-parser`, `wavefront_rs` when
  those steps land.
- `tools/sab_workshop/src/main.rs` / `src/gui.rs` — Export button gains a
  format multi-select. Detail-level dropdown on top of that.
- Manifest schema: add `model_files: [{path, format}]` and bump `bundle_version`.

## 11. Session evidence trail

This dump summarizes work from 2026-09-29. Four investigation subagents were
spawned in parallel; their reports (full text) are in the session transcript
under agent IDs `a72167b8b9dedf787`, `a6f414f9f870dcc6f`, `a61544993ab382593`,
`ac61afe08c4a0c71e`. Everything labelled PROVEN above traces back to their
cited file:line evidence, which they read directly.
