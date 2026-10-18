# Audio — the `1KCP` Wwise package

The Saboteur's voice/music/SFX ships as **Wwise file-packages** with a custom **`1KCP`** header (magic
`0x50434B31`), not stock Wwise AKPK. Confirmed genuine Wwise from the clean decomp
(`AK::SoundEngine::*` demangled; loader `FUN_0091aad0` @0x0091aad0 checks `!= 0x50434b31`).
No Mercs 2 analog — this is a from-scratch format for this project (Mercs 2 uses PWS/wavebanks).
The 2008 Xbox 360 prototype packs its audio differently (`PCK0`, big-endian, `BKHD` v29, XMA2
media): [`../2008_audio_format.md`](../2008_audio_format.md).

## `1KCP` layout (little-endian)

```
+0x00  u32  magic = 0x50434B31 ("1KCP")
+0x04  u32  block align (2048)
+0x08  u32  version (2)
+0x0C  u32  bankCount
+0x10  u32  bankTableOffset    = 0x1C on all 10 retail packs
+0x14  u32  streamCount
+0x18  u32  streamTableOffset  = bankTableOffset + bankCount*12 (sub-tables are contiguous)
+0x1C  record table: 12-byte entries { u32 id, u32 size, u32 offset(absolute) }
        - first  bankCount   records → .bnk soundbanks (magic BKHD; hold embedded SFX wems in DIDX/DATA)
        - next   streamCount records → loose .wem streams (magic RIFF) = the VO / music lines
```

**⚠️ The table starts at `+0x1C`, not `+0x28`.** Read the offset from `+0x10` rather than hardcoding it.
Verified across all 10 retail `.pck`: at `+0x1C` every one of the 82,575 records validates by magic
(`BKHD` for the first `bankCount`, `RIFF` for the rest) with **0** failures; reading from `+0x28`
skips the first record and reads one past the end, mis-reading **exactly 2 records per pack**. The
fields at `+0x10`/`+0x18` — previously documented as "unused" — are what prove it: `+0x10` literally
holds `28` (= `0x1C`), and `+0x18` holds `0x1C + bankCount*12` on every pack.

- Streamed `.wem` codec = **Wwise Vorbis** (`fmt` tag `0xFFFF`, 32 kHz, mono/stereo). ffmpeg can't
  decode this; **vgmstream** can (it rebuilds the Wwise codebooks). wem IDs are unique within a pack.
- Banks additionally contain embedded `.wem` (SFX) inside their `DATA` section, indexed by `DIDX`.

## The `.wem` codec, and decoding it in Rust (2026-10-12)

**Every retail `.wem` is the same variant — PROVEN.** A census of the 24,268 `.wem` carved from the
Steam install (English VO + `Saboteur.pck` streams and bank media; stage-2 goldens): all are RIFF/WAVE,
`fmt` tag `0xFFFF`, a **0x28-byte `fmt` chunk plus a separate `vorb` chunk** (the 2008-era Wwise
layout), mono or stereo, 8–44.1 kHz (32 kHz for 24,198). Each stream carries a **full Vorbis setup
header with inline codebooks** and 6-byte packet headers — not IDs into a packed codebook library.

**Rust decode path — PROVEN on retail data:** the `ww2ogg` crate (0.1.0, BSD-3-Clause, a port of hcs's
ww2ogg) converts Wwise Vorbis to standard Ogg Vorbis, and `lewton` (MIT/Apache-2.0, pure Rust) decodes
that to PCM. Use the builder with **`full_setup(true)` and `inline_codebooks(true)`**:

* 24,268 / 24,268 convert and decode: 24,219 in the full corpus run (3.59 G interleaved samples), and
  the other 49 in the 80-file comparison below. The packed-codebook modes fail on all of them
  (`invalid codebook id 0x342`).
* The crate's `validate()` rejects 49 SFX — its only check is "more than 10% of a packet's samples at
  ±32767", and these sounds genuinely clip. Do not gate on it.
* **Oracle:** on 80 files (the 49 + 31 spread over the rest) the Rust port's Ogg output is
  **byte-identical** to the original C++ `ww2ogg` 0.24 (SabTool's `tools/ww2ogg/ww2ogg.exe`, run under
  Wine), and `lewton` decodes **exactly** the sample count each `.wem` header declares.
* Not cross-checked: `lewton`'s PCM against libvorbis/vgmstream sample-for-sample — the
  `tools/vgmstream/` copy lacks its DLLs (`libvorbis.dll`, ffmpeg, …) and cannot start.

Method: scratch probe `wem_decode_test`. The decode now lives in `sab_formats::wwise::decode_wem`
(builder with `full_setup(true)`, `inline_codebooks(true)`); the game gate
`retail_wem_streams_decode_to_their_declared_length` (`tools/sab_formats/tests/audio_retail.rs`)
decodes every 25th stream of every `Sound/*.pck` to exactly the sample count its `vorb` chunk
declares — 852 streams of `English(US).pck` and `Saboteur.pck` in the Steam install (2026-10-18).
The workshop plays and exports voice lines through it (no vgmstream).

## Pack inventory (retail install)

| Pack | Streamed lines | Banks | Notes |
|---|---|---|---|
| `Sound\English(US).pck` | 20,741 | 205 | VO |
| `Sound\French(Canada).pck` | 20,082 | 205 | VO |
| `Sound\German.pck` | 19,914 | 205 | VO |
| `Sound\Italian.pck` | 20,063 | 205 | VO |
| `Sound\Saboteur.pck` | 529 | 342 | SFX / music / ambience (language-neutral) |
| `DLC\01\sound\*.pck` | 20 each | 1 | DLC VO |

`Sound\WWiseIDTable.bin` is a **7,340-string name table** (bank names + `vo_cht_chr_*` per-character
banks + `wp_*`/`vh_*` wildcards) — the key to human-readable naming (see "Open" below).

## Extraction pipeline (built)

`tools/saboteur_audio` (Rust, std-only) parses the `1KCP` index, carves the streamed `.wem`, and
batch-decodes via vgmstream to WAV, then tidies names and writes a `manifest.csv`.

```
cargo run --release -- \
  --game "C:/GOG Games/The Saboteur" \
  --out output/saboteur_audio \
  --vgmstream tools/vgmstream/vgmstream-cli.exe \
  --langs eng,fra,deu,ita --batch 150
```
Flags: `--keep-wem`, `--no-decode`, `--limit N` (test subset), `--batch N`.

**Result:** all 4 languages VO extracted → **80,880 WAV**, ~12 GB. Named by Wwise ID
(`eng_main_<id>.wav`). Output is gitignored; regenerate from your own install.

> **Corrected 2026-07-24.** This previously read "80,872 WAV (100% of decodable streams; only 8
> non-RIFF placeholders skipped)". There are no placeholder records: those 8 "non-RIFF" entries were
> the `+0x28` off-by-one above, one per pack across the 4 languages × (main + DLC). The extractor
> was silently losing 1 real stream per pack. Both the tool and the layout above are now fixed; the
> true total is 80,880 (20,741 + 20,082 + 19,914 + 20,063 main, + 20 each DLC).

### ⚠️ vgmstream batch gotcha
vgmstream-cli overflows a static argv buffer at ~400 file args (starts reading garbage as options,
`unknown option -a/-4`, and silently drops output). **Keep `--batch ≤ 200`** (150 used). A first run at
400 lost ~92% of files with no error surfaced by the exit code.

## Open / follow-up
- **Human names.** wem IDs are Wwise short-IDs. Map to character/mission/line by parsing each bank's
  `HIRC` event graph and cross-referencing `WWiseIDTable.bin`. This turns `eng_main_03845311.wav` into
  something like `Sean/mission3/line12.wav`.
- `Sound\Saboteur.pck` (SFX/music/ambience) and the bank-embedded SFX wems are not extracted yet.
