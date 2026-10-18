# 2008 Xbox 360 prototype — audio (`sound/Saboteur.pck`, `WWiseIDTable.bin`)

**Scope:** the sound data of the 2008-05-20 Xbox 360 prototype (`Bin360/sound/`): the Wwise package,
the banks and streams inside it, the `HIRC` objects a voice line passes through, and the game's
key→Wwise-id table. Written so the workshop's audio (decode, play, export) can treat both builds alike.
**Library:**
[`sab_formats::proto2008::sound_pack`](../tools/sab_formats/src/proto2008/sound_pack.rs) (the
package, chunk streams, `RIFX` media, the `XMA2` format),
[`sound_bank`](../tools/sab_formats/src/proto2008/sound_bank.rs) (`HIRC` objects: event, action,
sound, random/sequence container, node base parameters) and
[`sound_ids`](../tools/sab_formats/src/proto2008/sound_ids.rs) (`WWiseIDTable.bin`).
**Engine oracle:** `WildStar_p.exe`, whose decomp carries full Wwise symbols from `WildStar_p.map`
(`output/_ghidra_wildstar_p/wildstar_p_all_functions_decomp.txt`); VAs below are WildStar_p.

**Status:** every record of the package parses to its last byte: 114 banks, 2,976 streams, all
16,691 `HIRC` objects walked; every event, action, sound and random/sequence container read to its
exact end; every bank media and stream a `RIFX` XMA2 file. PROVEN by the game gate
`every_2008_sound_package_record_parses` (`tools/sab_formats/tests/proto2008_retail.rs`) and the
probe `sab_probe proto2008-audio` (Method).

## Retail vs. 2008

| | Retail PC ([`formats/audio_1kcp.md`](formats/audio_1kcp.md)) | 2008 Xbox 360 |
|---|---|---|
| Package | `1KCP`, little-endian, 12-byte records | `PCK0`, big-endian, 16-byte records |
| Packages | `Saboteur.pck` + one per language | one `Saboteur.pck`, voice included |
| Bank version (`BKHD`) | 44 | 29 |
| Media container | `RIFF` `WAVE` | `RIFX` `WAVE` (size word little-endian, chunks big-endian) |
| Codec | Wwise Vorbis (`fmt` 0xFFFF + `vorb`) | XMA2 (`XMA2` chunk, plugin id `0x00030001`) |
| Decode | software: `ww2ogg` + `lewton`, PROVEN on all 24,268; `sab_formats::wwise::decode_wem` | console hardware (`XMAPlayback*`); here `sab_formats::xma2`, a port of FFmpeg's decoder bit-identical to it ([`formats/xma2_decoder.md`](formats/xma2_decoder.md)) |
| `WWiseIDTable.bin` | described as a string table (audio_1kcp.md; not re-checked here) | binary key → id tables (below) |

**What carries over between the builds:** a voice line is still the event `Play_<key>`, hashed with
Wwise FNV-1 (`sab_formats::wwise::wwise_fnv1` of `wwise::event_name`), and banks still hold `HIRC`,
`DIDX`/`DATA` and `STID`. Everything below the event — byte order, record sizes, object layouts and
the codec — has to be read per build.

## What the game loads — PROVEN

`WSSoundManager::InitSoundData` @0x825f6550:

* `WWiseIDTable::Load("d:\sound\WWiseIDTable.bin")` (string @0x820b7f2c).
* `CAkDefaultLowLevelIO::SetBasePath("game:\sound\")` (@0x820b7ec4),
  `SetLangSpecificDirName("English(US)\")` (@0x820b7ea8),
  `WSStreamingIO::LoadSoundPack("Saboteur.pck")` (@0x820b7e8c).

`Bin360/sound/` holds exactly `Saboteur.pck` (359,708,672 bytes), `WWiseIDTable.bin` (53,064) and
`X360/`. There is no `English(US)` package; all 1,849 voiced lines resolve inside `Saboteur.pck`
(below). What the language folder name is used for in this build is not established.

`sound/X360/` (3,033 `.xma`, 65 `.bnk`, `StreamPack.spk`, its own `WWiseIDTable.bin`,
`English(US)/`) is **not what the game reads** and is not the same generation as the package:
none of its `WWiseIDTable.bin`'s 2,358 play ids is an event in the package (the loaded table's are,
4,307 of 4,307); no loose `.bnk` carries the id `wwise_fnv1(file stem)` in its `BKHD` although 61
stems name a package bank, and none is byte-identical to that bank; 1,172 `.xma` stems are package
stream ids and 1 of those files is byte-identical. PROVEN (probe). That it is authoring output from
another build of the project is INFERRED.

## `PCK0` package — PROVEN

Read whole by `WSSoundPack::LoadPack` @0x825f7eb0; records looked up by
`SoundEntryList::FindID` @0x825f7be8 (a binary search); `WSStreamingIO::Open` @0x825fa010 looks a
bank (codec 0) up in the bank table and every other file in the stream table.

```text
+0x00 "PCK0"   +0x04 u32 block size (0x800)   +0x08 u32 not read (0)
+0x0C u32 bank count (114)    +0x10 u32 bank table offset (0x1C)
+0x14 u32 stream count (2976) +0x18 u32 stream table offset (0x73C)
record (16 bytes, each table sorted by id): u32 id, u32 size, u32 offset (absolute), u32 (0 on all)
```

All 3,090 records start on a block boundary. A bank record's id is its `BKHD` id (114 of 114).

## Banks

A bank is big-endian chunks `{tag, u32 size, body}` that end exactly at the record's end (114 of
114). Chunks seen: `BKHD` 114 (version 29 on all), `HIRC` 90, `STID` 88, `DIDX` 83, `DATA` 83,
`ENVS`, `FXPR`, `STMG` 1 each.

### `DIDX` / `DATA` — PROVEN

`CAkBankMgr::LoadMedia` @0x828a0f90: `DIDX` is 12-byte entries `{u32 media id, u32 offset into
DATA, u32 size}`; `CAkBankMgr::ProcessDataChunk` @0x8289efc8 loads `DATA` whole. Of the 2,771
entries, 2,764 are a whole `RIFX` XMA2 file and 7 are the **head of a package stream**: the first
bytes of the stream with the same id, carrying that stream's full-size `RIFX` header (prefetch data).

### `HIRC` — PROVEN

`CAkBankMgr::ProcessHircChunk` @0x8289fee8: `u32 count`, then `count × {u32 type, u32 size,
payload}`; every payload starts with its u32 id. Types 1–9 are State, Sound, Action, Event,
RanSeqCntr, SwitchCntr, ActorMixer, Bus, LayerCntr; 14 Attenuation, 15 DialogueEvent; 10–13 go to
the music engine's handler ("Music engine not initialized : Content can not be loaded from bank").

| type | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9 | 10 | 11 | 12 | 13 | 14 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| objects | 6,036 | 5,329 | 4,813 | 334 | 55 | 221 | 23 | 27 | 109 | 108 | 4 | 54 | 94 |

16,691 distinct ids; 357 ids are defined in more than one bank.

### Object layouts — PROVEN (decomp, and every object of the type reads to its exact end)

**Event (4)** — `CAkEvent::SetInitialValues` @0x828b8ba8: `id, u32 count, action id × count`.

**Action (3)** — `CAkAction::SetInitialValues` @0x828b17b8: `id, u32 action type, u32 target id,
3 × i32 milliseconds (scaled to 48 kHz samples), u32 params flag`, then the class's own parameters
when the flag is set. `CAkAction::Create` @0x828b1580 picks the class from `type & 0xFF000`;
`0x4000` is Play. Every Play action in the package has type `0x00004011` and 37 parameter bytes
(`CAkActionPlay::SetActionParams` @0x828cd920). Classes present: `0x1000` 449, `0x2000` 1,
`0x3000` 1, `0x4000` 4,868, `0xE000` 4, `0x60000` 4, `0x70000` 2.

**Sound (2)** — `CAkSound::SetInitialValues` @0x828b9310 → `CAkBankMgr::LoadSource` @0x8289e6b8:

```text
u32 id, u32 plugin id, u32 stream type, u32 × 2 audio format, u32 source id, u32 file id
stream type != 1: u32 in-bank offset, u32 in-bank size
u8 flag
plugin id & 0xF == 2 or 5: u32 size, plugin parameters
node base parameters, u16 × 3
```

| plugin id | stream type | sounds | media |
|---|---|---|---|
| `0x00030001` | 0 | 3,093 | in its own bank: source id is a `DIDX` entry of that bank whose offset/size equal the sound's; file id = the bank's id |
| `0x00030001` | 1 | 2,938 | streamed: file id is a package stream id |
| `0x00030001` | 2 | 2 | prefetch: a `DIDX` head in the bank **and** a package stream (file id) |
| `0x00650002` | 0 | 3 | none — a source plugin (`& 0xF` = 2); no `DIDX` entry |

That `0x00030001` is the XMA codec is INFERRED from every one of its media being `XMA2`.

**Node base parameters** — `CAkParameterNodeBase::SetNodeBaseParams` @0x828c6e40. Its four virtual
readers are called through vtable slots +0x120, +0x11C, +0x124, +0x128; the `CAkSound` vtable
(`??_7CAkSound@@6BCAkAudioNode@@@` @0x820e6600) and the `CAkRanSeqCntr` one (@0x820e6750) both hold
`SetInitialFxParams` @0x828c1a70, `SetInitialParams` @0x828c3468, `SetPositioningParams` @0x828c1d50,
`SetAdvSettingsParams` @0x828c2068 there, so that is the byte order:

```text
FX         u8 override, u8 count; count != 0: u8 bypass bits,
           count × { u8 index, u32 fx id, u8 rendered, u32 size, bytes[size] }
u32 bus id      non-zero: must already be loaded ("Master bus structure not loaded: ")
u32 parent id   non-zero and loaded: the parent adopts this node
u8 × 4
initial    u32 × 13 (volume, LFE, pitch, LPF and their random ranges; the last is the state group)
position   u8 has; has: i32, u32, u32, u8 is3d; is3d == 0: u8;
           is3d: u32 kind, u32, u8; kind 3: u8;
           kind 2: u32, u8, u32, u32 nv, 16 × nv, u32 ni, 8 × ni
advanced   u32, u8, u16, u32, u8, u8
u32 state sync, u32 count, count × 9 bytes
RTPC       u32 count, count × { u32 × 5, u32 points, 12 × points }   (SetInitialRTPC @0x828c65f0)
```

**Random/sequence container (5)** — `CAkRanSeqCntr::SetInitialValues` @0x828bb4b0: `id`, node base
parameters, `u16`, 22 bytes of playback settings (byte +0x10 is the mode), `u32 count, child id ×
count` (`CAkParentNode::SetChildren` @0x828bba38), `u32 count, count × {u32 id, u8}` (playlist,
`AddPlaylistItem`). Children: 1,688 sounds and 35 containers.

Switch (6) and layer (9) containers' layouts are in the decomp (`CAkSwitchCntr::SetInitialValues`
@0x828be8c8, `CAkLayerCntr::SetInitialValues` @0x828bf818) but are not read by the library; no voice
line goes through one.

## Streams and media: `RIFX` XMA2 — PROVEN

Every stream record (2,976) and whole bank media (2,764) is `"RIFX"`, a size word, `"WAVE"`, then
big-endian chunks to the record's end. **The size word is little-endian** and equals the record's
whole length; the engine never reads it (`ValidateRIFX` @0x828edbb0 checks only `RIFX` and `WAVE`).
`CAkFileParser::Parse` @0x828edc10 walks the chunks:

| chunk | engine use | in the data |
|---|---|---|
| `XMA2` (or `fmt `) | the first one is the wave format | 44 bytes on all: `XMA2WAVEFORMAT` (xma2defs.h layout), version 4, loop count 0, 2,817 one-channel and 159 two-channel, 44.1 kHz (one stream 24 kHz) |
| `seek` | its offset is kept; `CAkSrcFileXMA::ParseHeader` @0x828eb2e8 copies `block count × 4` bytes | one u32 per XMA2 block, strictly rising, the last equal to samples encoded (2,976 of 2,976) |
| `XMAc` | 12 bytes copied into `AkXMA2CustomData` | 12 bytes on all; its words equal no `XMA2` field — meaning unknown |
| `cue ` / `LIST` (`adtl`, `labl`) | markers and their labels | 1,157 / 457 streams |
| `smpl` | loop points | none |
| `vorb` | a Vorbis header path | none |
| `data` | end of the walk; size and offset kept | length within the last block of block size × block count (2,976 of 2,976) |

Decode: `CAkSrcBankXMA::InitXMA` @0x828ea0e0 hands the `data` bytes to `XMAPlaybackCreate` /
`XMAPlaybackSubmitData` — the console's XMA hardware. The executable contains no software XMA decoder.

## `WWiseIDTable.bin` — PROVEN

`WWiseIDTable::Load` @0x825fc488; tables by lookup: `GetEventPlayID` @0x825fbf70 / `GetEventStopID`,
`GetStateID` @0x825fc010, `GetSwitchID` @0x825fc090, `GetTriggerID` @0x825fc110, `GetRPTCID`
@0x825fc160.

```text
u32 version (1; anything else fails the load)
u32 events, u32 state groups, u32 switch groups, u32 triggers, u32 RTPCs
events        × { u32 key, u32 play id, u32 stop id }    (0xFFFFFFFF = none)
state groups  × { u32 key, u32 group id, u32 n, n × { u32 key, u32 id } }
switch groups × (as state groups)
triggers, RTPCs × { u32 key, u32 id }
```

The loaded file: 4,312 events, 7 state groups, 13 switch groups (117 members across both group
kinds), 4 triggers, 11 RTPCs, ending exactly. Its 4,307 play ids and 338 stop ids are all events in
the package. `WSSoundManager::PlayEvent` @0x825f6bf8 takes a key, looks up the play id and posts it.

## A voice line, end to end — PROVEN

For each of the 3,903 keyed (non-UI) lines of `Cinematics/Dialog/English/GameText.dlg`:

* **1,849** have an event `wwise_fnv1("Play_<key>")` in the package. Every one has exactly one
  action, a Play, whose target is a sound (1,843) or a random/sequence container whose children are
  all sounds (6). The 1,843 direct sounds' media: 1,059 streamed (a package stream), 784 in their
  bank's `DATA`.
* **1,784** have a table event whose key is `pandemic_hash(key)` and whose play id is
  `wwise_fnv1("Play_<key>")` (87 of them also have a stop id). That is the path the game itself
  takes.
* **65** have a `Play_<key>` event in the package but no table row, so `PlayEvent(key)` cannot
  reach them.
* **2,054** have neither an event nor a table row; that these lines are simply not recorded in this
  build is INFERRED.

## Playing a line: `sound_voice` and the workshop — PROVEN

[`sab_formats::proto2008::sound_voice`](../tools/sab_formats/src/proto2008/sound_voice.rs) holds the
banks in memory and resolves a key down the chain above. Its rules, from the data:

* **Objects are read inside one bank.** 357 ids are defined by more than one bank, and some
  definitions differ: 139 sounds only in their source fields (an in-bank media offset is relative to
  that bank's `DATA`), 68 actions only in their parameters (same type and target), 3
  random/sequence containers and 58 actor-mixers in their children; no event differs (probe). So
  an object the event's bank defines is taken from that bank, and one it does not define must be
  defined alike everywhere (else an error).
* **An event defined by several banks:** 37 voice lines. Each bank's chain reaches **byte-identical
  media**, so which bank is read does not change the audio (game gate).
* **A random/sequence container** plays its first playlist entry (the six crowd lines); the source
  says "entry 1 of N".
* **A prefetch sound** (stream type 2) plays the whole package stream, not the `DIDX` head.

The workshop reaches this through `GameSource::voice_line` (`tools/sab_workshop/src/source.rs`):
retail resolves through `wwise::SoundPack` and decodes with `wwise::decode_wem`, the 2008 build
through `sound_voice` and `xma2`; both return `sab_formats::pcm::Pcm`, which the Strings page plays on
the default output device (`rodio`) or writes as a 16-bit WAV. vgmstream is no longer used there.

## Unknowns

* `XMAc`'s three words (copied into `AkXMA2CustomData`; their consumer is not in the two functions
  read).
* The `PCK0` header word +0x08 (0; `LoadPack` does not read it) and the record word +0x0C (0 on all
  3,090 records; its use not established).
* Plugin `0x00650002`'s identity (three sounds).
* What `SetLangSpecificDirName("English(US)\")` affects when the only package is `Saboteur.pck`.
* The `STID`, `ENVS`, `FXPR` and `STMG` chunk contents and the music objects (types 10–13) — not
  read.

## Method

* `sab_probe proto2008-audio <Bin360>` ([`tools/sab_probe/src/proto2008_audio.rs`](../tools/sab_probe/src/proto2008_audio.rs))
  — every count above: package, chunk tags, `HIRC` histogram, object parse totals, action classes,
  sound plugin/stream-type table, `DIDX` media, stream chunk sets and `XMA2` fields, `seek`/`data`
  checks, `X360/` comparison, both `WWiseIDTable.bin` files, and the voice-line resolution.
* Game gates in `tools/sab_formats/tests/proto2008_retail.rs` (`cargo test --release --features
  retail`): `every_2008_sound_package_record_parses`, `every_2008_sound_media_decodes_to_its_declared_length`,
  `every_2008_voice_line_resolves_and_decodes` (1,849 voiced, 6 through a container, 37 with the
  event in several banks, 2,054 without an event).
* The probe's "ids defined differently by two banks" lines — the counts in "Playing a line".
* Workshop gate `voice_lines_play_from_both_builds` (`tools/sab_workshop/src/source.rs`,
  `cargo test --release --features retail`).
* Decomp: grep the function names above in
  `output/_ghidra_wildstar_p/wildstar_p_all_functions_decomp.txt`; vtable slots read from
  `WildStar_p.exe` at the `??_7…` addresses in `Bin360/WildStar_p.map`; strings read from the exe at
  the VAs given.
