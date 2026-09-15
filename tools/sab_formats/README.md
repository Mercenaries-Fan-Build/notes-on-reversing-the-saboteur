# sab_formats

Format-parsing library + authoring CLIs for *The Saboteur* (2009, GOG PC retail). Two halves:

- **Library** (`src/*.rs`) — byte-verified parsers/writers for shipped file formats: `AP0L`
  (`Animations.pack`), `EditNodes.pack`, `GameText.dlg`, plus shared plumbing.
- **Authoring binaries** (`src/bin/*.rs`) — command-line tools that use the library to edit
  live game files. Two binaries today; more land in `src/bin/` as new format work becomes
  authorable.

Read-only inspection of the same formats lives in the sibling [`sab_probe`](../sab_probe/)
crate; the workshop UI ([`sab_workshop`](../sab_workshop/)) consumes both.

## anim_patch_saboteur — retail animation modding

The first working end-to-end animation-mod pipeline for retail Saboteur. Drives Havok Content
Tools 6.5 (period-correct oracle at `C:\Program Files (x86)\Havok\BatchProcess\AssetCc\AssetCc2.exe`
— "Built with Havok-6.5.0-r1", exact match with the game's shipped `Animations.pack`) as a black
box: extract → decompile to XML → edit → recompile to a Win32-LE packfile → re-splice into the
AP0L container.

Sab's `Animations.pack` is a single AP0L container holding the ANIM metadata block + one
concatenated Havok packfile carrying all 2,214 `hkaSplineCompressedAnimation` clips. The tool
preserves every non-Havok byte on splice; only the Havok blob is modified.

### Build

```
cargo build --release -p sab_formats --bin anim_patch_saboteur
```

### Subcommands

```
anim_patch_saboteur perturb-clip <pack> --clip N [--out <path>] [--assetcc <AssetCc2.exe>] <edit-flags>
anim_patch_saboteur verify-splice <baseline.pack> <modified.pack>
anim_patch_saboteur probe-clip    <pack> --clip N
```

**Edit flags** (combine any; at least one required — `--set-duration` needs `--assetcc`, the
static-track edits do not):

| flag | effect |
|---|---|
| `--set-duration <secs>` | Change the clip's playback duration. Uses AssetCc2 XML round-trip. |
| `--set-static-translation <trk> <x> <y> <z>` | Overwrite a track's static translation (12 bytes). |
| `--set-static-rotation    <trk> <qx> <qy> <qz> <qw>` | Overwrite the static packed quaternion (auto-normalised, THREECOMP40-encoded). |
| `--set-static-scale       <trk> <sx> <sy> <sz>` | Overwrite the static scale. |
| `--offset-static-translation <trk> <dx> <dy> <dz>` | Additive delta on the static translation. |

Static edits bypass AssetCc2 entirely and splice bytes in-place (a translation edit produces
exactly 12 bytes of Havok-blob delta on a 8.4 MB packfile). Duration edits go through AssetCc2
because the ANIM-header offsets shift when the encoded blob resizes.

### Example — set clip 0's duration to 5 seconds

```
anim_patch_saboteur perturb-clip "C:/GOG Games/The Saboteur/Animations.pack" \
    --clip 0 --set-duration 5.0 \
    --out out/Animations.pack \
    --assetcc "C:/Program Files (x86)/Havok/BatchProcess/AssetCc/AssetCc2.exe"

anim_patch_saboteur verify-splice \
    "C:/GOG Games/The Saboteur/Animations.pack" \
    out/Animations.pack
# head + tail byte-identical; havok blob same size; drift within HCT's re-quant budget.
```

### Example — move track 3's static translation

```
anim_patch_saboteur perturb-clip "C:/GOG Games/The Saboteur/Animations.pack" \
    --clip 0 --set-static-translation 3 1.0 2.0 3.0 \
    --out out/Animations.pack
# 12 bytes of delta, 0.000% drift.
```

### Deploy

`anim_patch_saboteur` writes to `--out`; it never touches the game install. To deploy, replace
`Animations.pack` in your GOG install after backing up the original:

```
cp "C:/GOG Games/The Saboteur/Animations.pack" \
   "C:/GOG Games/The Saboteur/Animations.pack.sabbak"
cp out/Animations.pack "C:/GOG Games/The Saboteur/Animations.pack"
# … launch the game, verify, then restore …
cp "C:/GOG Games/The Saboteur/Animations.pack.sabbak" \
   "C:/GOG Games/The Saboteur/Animations.pack"
```

### Requirements

- HCT 6.5 installed at `C:\Program Files (x86)\Havok\` (only needed for `--set-duration`;
  static-track edits are pure Rust).
- Rust nightly-or-stable; std-only.

## sab_gametext — GameText.dlg editor

CLI companion to the workshop's GameText page. See `docs/formats/gametext.md` for the format
and the memory index for the round-trip proof.

## Library API

- `sab_formats::ap0l` — AP0L parser + splice, XML find/edit, spline-track byte walker for the
  compressed data buffer, Hk↔XML index mapping (fingerprint + FNV-1a data-payload hash,
  bijective on the 2,214-clip retail corpus).
- `sab_formats::editnodes` — dynamic-object DB read/write for `EditNodes.pack`.
- `sab_formats::gametext` — UI-string dictionary keyed by `pandemic_hash`, byte-exact round-trip.

## References

- [`docs/formats/animation_havok65.md`](../../docs/formats/animation_havok65.md) — Havok 6.5
  `hkaSplineCompressedAnimation` format spec.
- [`sab_havok65`](../sab_havok65/) — the reference decoder + HCT-XML and per-frame TRS
  validation harnesses (`validate-hct`, `validate-trs`).
