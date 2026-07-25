# sab_probe

Ask The Saboteur's assets questions. **Read-only**: it writes no files and changes nothing — it
reports what is actually in the game data.

This exists so investigations don't get bolted onto the extraction tools as env vars and one-off
flags. Questions live here; [`sab_mesh`](../sab_mesh/README.md) and
[`sab_skeleton`](../sab_skeleton/README.md) stay about extraction. It is a **diagnostic tool for
people debugging a broken mod or a format**, not part of a normal modding workflow.

## Commands

```
cargo build --release

sab_probe parts <megapack> <name_substr>   cross-part bone identity; stored inverse-bind vs chain
sab_probe bones <megapack> <name_substr>   per-bone truth vs chain, and the local it implies
sab_probe names <skeleton.json> -          recover bone names by hashing candidates
sab_probe anim  <anim_bone_map.json> <skeleton.json>
                                           does each track drive the bone it claims?
sab_probe editnodes <info|list|tree> <EditNodes.pack>   dynamic-object DB (NPCs/vehicles/…)
sab_probe gametext  <info|list|get|hash|roundtrip> <GameText.dlg>   UI text + VO subtitles

e.g. sab_probe parts "C:/GOG Games/The Saboteur/Global/Dynamic0.megapack" CH_AL_SeanDevlin
```

## What `parts` found

The command that motivated the tool, and the reason character rigs are handled the way they are:

- A character's parts each carry their **own** skeleton with their own bone count (HD=168, UB=182,
  LB=182, GR=191, HAT=105), and bone **index N is not the same bone in every part**. The pipeline
  extracts one skeleton — from whichever part has the most bones, i.e. the glove — and poses the
  merged mesh against it.
- Every skinned bone ships its own inverse-bind matrix in the mesh's `BoneRemaps`. Where two parts
  skin the same bone, those stored matrices **agree with each other** (trustworthy ground truth)
  and **disagree** with the `world[i] = world[parent] · localTMS[i]` chain the tools derive.
- The chain cannot be repaired from what it reads: a character's face bones have local translations
  of `(0,0,0)` on disk, so the chain collapses that subtree onto a single point. The stored
  inverse-bind is the only place those offsets exist.

**None of this is visible at bind pose.** `jointMatrix = world · inv_bind` comes out identity for
any self-consistent-but-wrong chain, so the mesh renders perfectly until a clip plays. If your
ported character looks right standing still and explodes on animation, start here.

## Note

Its container/MESH/skeleton parsing is **copied** from `sab_skeleton` rather than shared with it, on
purpose: the probe reads the **game**, not the tools, so it must not depend on code that could hide
the very bug it is being used to find.
