# Release notes

## v0.1.0 (draft, not tagged yet)

The first build of TechnoMageRecomp: *TechnoMage: Return of Eternity* (*TechnoMage – Die Rückkehr der Ewigkeit*)
recompiled into Rust, without an emulator or Wine, for Linux, Windows and the browser.

### What you need

Nothing but the converter: it downloads the release in your language from archive.org (abandonware) and checks it
by its SHA-256. You can use copies you already have instead.

| Language | Medium | Also downloaded |
|----------|--------|-----------------|
| German | SCREENFUN 03/2004 cover CD (`2004-03.BIN`, 806 MiB), version 1.04 | 20 sound banks from the retail CD 1.00 (`data1.hdr`/`data1.cab`, 160 MiB): the archived cover CD has unreadable sectors in them |
| English | European release (`TECHNOMAGE.mdf`, 802 MiB) | the German cover CD (806 MiB): its executable runs the English data, as the English one is wrapped in SecuROM |

### How to play

- **Linux, Windows**: `recomp download --dest media technomage de` (or `en`), then
  `recomp convert media/2004-03.BIN --system linux` (English: `media/TECHNOMAGE.mdf`; `--system windows`). On
  Linux the result is one AppImage holding the whole game: start it. On Windows unzip it and start
  `technomage.exe`. Alt+Enter or F11 switches to full screen.
- **Browser**: on the converter page choose "Browser" and a language. The game is stored in the browser and
  plays on its page. It needs WebAssembly stack switching (JSPI): a current Chrome, Edge or Firefox.

Saved games and settings go to your user directory (`~/.local/share/technomagerecomp`,
`%APPDATA%\technomagerecomp`), or stay in the browser.

### What works

- The intro and cut scenes (Bink, through the game's own `binkw32.dll`, recompiled too), the menus, a new game in
  German and English, the first world's first rooms, dialogue and tutorial.
- Direct3D 7 on the GPU (wgpu natively, WebGL2 in the browser) at the game's 640×480, with a software renderer as
  fallback (`RC_RENDERER=soft`).
- Miles sound (samples, 3D samples, music) and Bink's sound through one mixer; speech and music from the
  virtual CD drive.
- Keyboard and mouse (DirectInput and window messages).
- Leaving through the main menu; the game's settings file.
- Scripted runs (`RC_INPUT`, `RC_FRAMES`, `RC_SCREENSHOT`): 20,000 frames in the first world without a fault.

### Known issues

- **Not played through yet.** Only the beginning has been played; later worlds may stop on something the
  runtime does not implement yet. Please report where (the console shows the address).
- **Saving and loading a game** have not been tried in a real playthrough.
- **Sound** has been checked by tests and traces, not by ear on every system.
- **Windows** builds are made and checked by CI but have not been played on Windows by hand.
- **macOS** is not offered.
- No gamepad support yet.
- The browser player keeps the game in the browser's storage (about 900 MiB); a browser may clear it when
  space runs low unless the page was allowed to keep it.

### Thanks

To the sister projects TropicoRecomp, CulturesRecomp, HoMM4Recomp and SackedRecomp, whose recompiler this one
builds on; and to archive.org for keeping the game.

The game is the property of its rights holders. This project contains no game code and no game data.
