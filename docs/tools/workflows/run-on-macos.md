# Run the game on macOS (Wine), and pull files out without Windows

How to install and launch the retail game on a Mac so a mod can be checked in-game, and how to get
the GOG files you need for decompiling or format work without a Windows machine. Verified on an
Apple Silicon Mac on 2026-10-11.

## What you need

- The **GOG** installer: `setup_the_saboteur_1.3_hotfix_(22140).exe` plus its `-1.bin` / `-2.bin`.
  The Steam build works for assets but its `Saboteur.exe` is DRM-wrapped (see
  [`tools/ghidra/README.md`](../../../tools/ghidra/README.md)).
- `innoextract` (`brew install innoextract`) — only to pull files out without running anything.
- A 64-bit Wine with experimental wow64 (32-bit apps in a 64-bit process). The build verified here is
  Heroic's [`wine-crossover`](https://github.com/Heroic-Games-Launcher/wine-crossover) `23.7.1-1` (the
  same one Mercenaries 2 Modkit installs). It is x86_64, so Apple Silicon needs Rosetta 2:
  `softwareupdate --install-rosetta --agree-to-license`.

In the commands below, `$WINE` is that build's `.../Contents/Resources/wine/bin` directory and
`$PREFIX` is the Wine prefix you dedicate to the game (e.g. `game-files/wine-prefix`; `game-files/` is
gitignored). Use its own prefix — don't share one with another game's saves.

## Pull files out without installing

`innoextract` reads the installer as an archive; none of the installer's code runs.

```
innoextract --info "setup_the_saboteur_1.3_hotfix_(22140).exe"            # metadata only
innoextract --list "setup_the_saboteur_1.3_hotfix_(22140).exe"            # every file it holds
innoextract --include Saboteur.exe --language en-US -d <out-dir> "setup_the_saboteur_1.3_hotfix_(22140).exe"
```

Drop `--include` to extract everything (~7.4 GB). The GOG `Saboteur.exe` should hash to
`628cdbb64a5317fef66d3c2fe87ab063b0c659dfc92f5882b85828f90568e4ef`.

## Install into a Wine prefix

```
WINEPREFIX="$PREFIX" "$WINE/wine64" wineboot --init
cd <folder with the GOG installer>
WINEPREFIX="$PREFIX" "$WINE/wine64" "setup_the_saboteur_1.3_hotfix_(22140).exe" \
  /VERYSILENT /SUPPRESSMSGBOXES /NORESTART /LANG=english \
  "/DIR=C:\GOG Games\The Saboteur" "/LOG=C:\gog_install.log"
WINEPREFIX="$PREFIX" "$WINE/wineserver" -w
```

This **does** run the installer's code. Check `$PREFIX/drive_c/gog_install.log` ends with
"Installation process succeeded." (the exit code alone proves nothing), and hash the installed
`Saboteur.exe` against the value above. The installer also runs the DirectX redist; every DLL the exe
imports (`d3dx9_39`, `XINPUT1_3`, `d3d9`, `binkw32`, …) ends up present.

## Launch

Run it from the install folder — the game loads its data relative to the working directory:

```
cd "$PREFIX/drive_c/GOG Games/The Saboteur"
WINEPREFIX="$PREFIX" "$WINE/wine64" Saboteur.exe > launch.log 2>&1
```

The single `err:environ:init_peb … experimental wow64 mode` line is expected.

## Video settings: `config.ini`

The in-game menus do not change resolution or texture quality; the PC game reads them from a text
file at startup (`Saboteur.exe` `FUN_007bcc20`):

```
<Documents>\My Games\The Saboteur™\SaveGames\config.ini
```

Wine links the prefix's `Documents` to your real `~/Documents` by default, so on a Mac this is
`~/Documents/My Games/The Saboteur™/SaveGames/config.ini`. The game does not create it for you.

- One `Name value` pair per line (tab or space). Lines starting `;`, `#` or `//` are comments.
- ⚠️ **Lines must end in CRLF (`\r\n`).** The game's line reader (`FUN_00dbb000`) ends a line only at
  `\r`. A file saved with Unix `\n` endings is read as one long line; if that line is a comment, the
  whole file is ignored with no error.
- Settings the file doesn't list keep their defaults: `ScreenWidth 1280`, `ScreenHeight 720`,
  `Windowed 0`, `TextureQuality 3`, `Shadows 1`, `PostProcessing 1`, `VSync 1`, `RefreshRate 60`,
  `ClipRange 3`, `ObjectQuality 2`, `RainDensity 100`, `SliceQuality 0`.

### Windowed mode

Without `Windowed 1` the game creates an **exclusive full-screen** Direct3D device (`FUN_00de0380` →
`FUN_00df5fb0`). Under Wine on a Mac that shows up as a 1280×720 image on one monitor which, once
clicked, takes over that display and blanks any others. For a normal window:

```
printf 'Windowed\t1\r\nScreenWidth\t1280\r\nScreenHeight\t720\r\n' \
  > ~/"Documents/My Games/The Saboteur™/SaveGames/config.ini"
```

The game reads `Windowed` (`FUN_00424a30`, call at `0x424c33`) and takes its windowed device path
(`FUN_00df6550`). Verified: no display-mode change for a whole session, click included.

## Known limits

- **Frame rate.** Wine's built-in D3D9 translates to OpenGL, which is slow on macOS — about 15 fps in
  the world. Lowering `config.ini` settings helps a little; a DXVK `d3d9.dll` (D3D9 → Vulkan →
  MoltenVK → Metal) is the real fix and is not set up here yet.
- **Wine virtual desktop doesn't work** with this build (`explorer /desktop=…` starts no window and
  never launches the game). Use `config.ini` for windowed mode instead.
- **Debug logging.** `WINEDEBUG=+timestamp,+macdrv,+d3d9` explains display problems, but `+d3d9` logs
  every API call — about 1.3 GB in two minutes. Keep sessions short.
