# XMA2 decoder — `sab_formats::xma2`

**Scope:** decoding the 2008 Xbox 360 prototype's audio (every stream and bank media of
`sound/Saboteur.pck` is XMA2; [`../2008_audio_format.md`](../2008_audio_format.md)) to PCM in Rust,
so the workshop can play and export it. Retail audio is Wwise Vorbis and decodes elsewhere
([`audio_1kcp.md`](audio_1kcp.md)).

**Status:** a port of FFmpeg 9.0.1's XMA2 decoder. On all 5,740 media of the 2008 build it is
**bit-identical** to FFmpeg's C code path — 1,377,937,920 samples, every length equal. PROVEN by
`sab_probe xma2-oracle` (Method). Every media decodes in the game gate
`every_2008_sound_media_decodes_to_its_declared_length`.

## Why a port

The game decodes XMA in hardware: `CAkSrcBankXMA::InitXMA` @0x828ea0e0 hands the `data` bytes to
the console's `XMAPlaybackCreate` / `XMAPlaybackSubmitData` (WildStar_p), so the executable has no
decoder to follow. No Rust crate decodes XMA2 except `xma-rs` (git only, LGPL); the user chose a port
inside `sab_formats` (2026-10-18). XMA is WMA Pro inside 2048-byte packets, and FFmpeg's
`libavcodec/wmaprodec.c` is the reference implementation.

## What was ported

| Rust | FFmpeg 9.0.1 source |
|---|---|
| `xma2/mod.rs` | `libavcodec/wmaprodec.c`: `decode_init` (XMA2), `decode_packet`, `decode_frame`, `decode_tilehdr`, `decode_channel_transform`, `decode_coeffs`, `decode_scale_factors`, `inverse_channel_transform`, `wmapro_window`, `decode_subframe`, `save_bits`, `xma_decode_packet`; `libavcodec/wma.c`: `ff_wma_run_level_decode`, `ff_wma_get_large_val`; the 64-sample skip of `libavcodec/decode.c` |
| `xma2/bits.rs` | `get_bits.h` (`GET_VLC`), `put_bits.h`, `vlc.c` (`ff_vlc_init_from_lengths`, `build_table`) |
| `xma2/mdct.rs` | `libavutil/tx.c` + `tx_template.c`: `ff_tx_mdct_inv`, `ff_tx_mdct_gen_exp`, the split-radix `ff_tx_fft{2,4,8,16,N}_ns` codelets and `ff_tx_fft_sr_combine`, `split_radix_permutation`, the `ff_tx_tab_N` cosine tables |
| `xma2/tables.rs` | `libavcodec/wmaprodata.h`, converted mechanically (a scratch converter; the written file diffed equal to its output) |

Source tarball `ffmpeg-9.0.1.tar.xz`, SHA-256
`cf38e0e28c7e5605942c4a77755349b0145804a397af37eb1fb4c77cb237f635`. The ported files carry FFmpeg's
LGPL-2.1-or-later notice; `sab_formats` is therefore `MIT AND LGPL-2.1-or-later`.

**Scope of the port.** Only what an XMA2 stream reaches: decode flags fixed at `0x10d6` (512-sample
frames, a 15-bit length prefix on each frame, dynamic range compression, up to 4 subframes, so
128/256/512-point IMDCTs), one or two channels per stream, one stream per file (every 2008 media).
Multi-stream files, more than two channels, LFE, the decorrelation matrices for more than two
channels and the WMA Pro container path are not ported. Malformed input is an error; FFmpeg's
packet-loss recovery (resynchronising after a bad frame) is not reproduced.

## Bit-exactness: fused multiply-add

FFmpeg's `configure` does not set `-ffp-contract`, and clang's default (`on`) fuses a product into the
add or subtract of the same C expression. The Homebrew build's arm64 code therefore computes
`a*b - c*d` as `fma(a, b, -(c*d))`, `a*b + c*d` as `fma(a, b, c*d)` and `s += x*y` as `fma(x, y, s)`
— the left-hand product fused, the right-hand one rounded first (clang's `tryEmitFMulAdd`). The port
writes exactly those as `f32::mul_add`: the complex multiplies of the MDCT and FFT (`CMUL`),
`vector_fmul_window`, and the stereo matrix in `inverse_channel_transform`. Every other product is
plain. With that rule the output is bit-identical (PROVEN, below); without it, it would differ in the
last bits.

Tables computed at start-up use the platform's `libm` (`sinf` for the sine windows; `cos`, `sin`,
`sqrt` for the twiddles; `exp2` for the quantiser), as FFmpeg does. On macOS both use the system
`libm`. On another platform a `libm` that rounds one of these differently could change the last bit
of some samples — not checked.

## Output length

`decode` returns what FFmpeg returns: frames of 512 samples, the first frame dropped (`skip_frame`),
then 64 samples dropped (`avctx->internal->skip_samples`), a final half frame flushed at end of
stream, and `clamp(trim_end + trim_start − 192)` samples cut from the end (`xma_decode_packet`), the
trims being the last non-zero ones any frame header carried.

That equals the `XMA2` chunk's *samples in source* for 5,737 of the 5,740 media. Three end short by
less than a frame — streams `24A657A6` (384) and `2F3110BF` (384), bank `53B059FC` media `1FB94EE6`
(256) — and FFmpeg's output is the same length. Which length the console plays is not established.

## Results — PROVEN

`sab_probe xma2-oracle <Bin360> /opt/homebrew/bin/ffmpeg <work> c` (FFmpeg 9.0.1, Homebrew, arm64,
`-cpuflags 0`, `f32le`):

| | media | bit-identical | length differs | our failures | FFmpeg failures |
|---|---|---|---|---|---|
| package streams + whole bank media | 5,740 | 5,740 | 0 | 0 | 0 |

The same run with `default` (FFmpeg's NEON code) matches no media bit for bit: the largest
difference is 4.8e-7, about 1/64 of a 16-bit step — the SIMD FFT's different operation order.
FFmpeg's only messages were "Guessed Channel Layout" and, for 15 media, a muxer timestamp warning.

## Unknowns

* Whether the console plays the three short media to their samples in source.
* Bit-exactness on platforms other than macOS arm64 (`libm` differences, above).

## Method

* `sab_probe xma2-oracle <Bin360> <ffmpeg> <work dir> <c|default> [every-nth]`
  ([`tools/sab_probe/src/xma2_oracle.rs`](../../tools/sab_probe/src/xma2_oracle.rs)) — writes each
  media out, decodes it with FFmpeg and with the port, and compares every sample.
* Unit tests in `sab_formats::xma2` (bit reader and writer, multi-level VLC, and the IMDCT against
  FFmpeg's `ff_tx_mdct_naive_inv` reference in double precision).
* Game gate `every_2008_sound_media_decodes_to_its_declared_length`
  (`tools/sab_formats/tests/proto2008_retail.rs`, `cargo test --release --features retail`).
