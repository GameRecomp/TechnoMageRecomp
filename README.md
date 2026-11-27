# TechnoMageRecomp

<p align="center"><img src="tools/icons/technomage.svg" width="128" height="128" alt=""></p>

**Play *TechnoMage: Return of Eternity* (*TechnoMage – Die Rückkehr der Ewigkeit*) natively on Windows and
Linux (x86-64 and ARM64) and Android, or right in your browser.** The game by Sunflowers is abandonware and archived on archive.org; the
converter downloads the release in the language you choose (German, English, French, Italian or Spanish) and builds
the game from it, on your machine.

TechnoMageRecomp is an unofficial project that rebuilds the game as a native program for today's systems: no
emulator, no Wine. The game's own code is translated ahead of time ("static recompilation") into Rust and runs on a
small replacement for the parts of Windows, DirectX (including Direct3D 7, drawn with Bevy and wgpu) and Miles it uses. It is the
fifth project on the toolchain of [TropicoRecomp](https://github.com/GameRecomp/TropicoRecomp),
[CulturesRecomp](https://github.com/GameRecomp/CulturesRecomp),
[HoMM4Recomp](https://github.com/GameRecomp/HoMM4Recomp) and
[SackedRecomp](https://github.com/GameRecomp/SackedRecomp), and the first written entirely in Rust.

> Unofficial fan project. Not affiliated with Sunflowers or the game's publishers.
> This repository contains **no game code and no game data**.

## Supported releases

| Game | Languages |
|------|-----------|
| TechnoMage | German (SCREENFUN 03/2004 full version), English (European release), French, Italian, Spanish |

Each is pinned by its checksum ([docs/TARGETS.md](docs/TARGETS.md)).

## State

Early: the intro, the menus and the beginning of a new game in German, English, French, Italian and Spanish run on
Linux and in the browser,
Direct3D on the GPU (WebGL2 in the browser). Not played through yet; Windows builds are made but not played by
hand. See [docs/RELEASE-NOTES.md](docs/RELEASE-NOTES.md) and [docs/PLAN.md](docs/PLAN.md).

## Playing

1. Download the converter for your system from the releases.
2. Run `recomp download --dest media technomage de` (or `en`, `fr`, `it`, `es`): it fetches the release from
   archive.org and checks it.
3. Run `recomp convert media/2004-03.BIN --system linux` (English: `media/TECHNOMAGE.mdf`, French:
   `media/TECHNOMAGE.ISO`, Italian: `media/tecnomage.img`, Spanish: `media/TechnoMage_PC_CD_2000_ES.tar.001`): it
   writes a zip with the game ready to run. For an ARM64 machine: `--system linux-arm64` or `windows-arm64`. Unpack it and start `technomage`.

Or play in the browser: on the [converter page](https://gamerecomp.github.io/TechnoMageRecomp/), choose
"Browser" and a language; the game is stored in the browser and runs on its page (a current Chrome, Edge or
Firefox).

On Android: choose "Android" on the converter page (or `--system android`), copy the APK to the phone or tablet
and install it. The first start unpacks the game's files once. A finger is the mouse (tap to click, a second
finger is the right button), Back opens the menu; a keyboard, mouse or gamepad work too. Not yet tried on a
device (docs/PLAN.md).

## Improvements over the original

The game logic is the original's; around it, the program adds a few things. They are changes to how the game is
run and shown, made by the project's patches ([docs/DEVELOPMENT.md](docs/DEVELOPMENT.md)), not edits to the game's
files:

- **Pause menu:** "Configure graphics" shows the render resolution; "Show information" shows the program's
  version, build time and commit, the platform, the game's version and its language. Both speak the game's
  language (German, English, French, Italian, Spanish, Dutch).
- **Native resolution** (in "Configure graphics", kept): the 3D scene is drawn at the size of the window, or of
  the screen in full screen, keeping the game's 4:3 picture with black bars; menus, texts and the HUD stay as they
  were, only enlarged.
- **Widescreen** (in "Configure graphics", kept): the picture is as wide as the window (up to 32:9), the
  world drawn over the whole width, so more of the level shows at the sides. Nothing is stretched: menus,
  dialogues, the HUD, the map and the quest log stay as they were, in the middle (black at the sides where the game
  covers the whole screen); the cut scenes, made nearly 16:9 with black bars above and below, fill the width.
  Natively and in the browser; with native resolution too.
- **Antialiasing** and **Show frame rate** (in "Configure graphics", kept): the 3D scene drawn with four samples
  per pixel on the GPU (natively); the frames per second in the top right corner.
- **Hardware cursor:** the game's stone pointer is the system's mouse cursor, so it follows the mouse without
  delay, at the size it had in the picture. It keeps turning towards the hero as in the game, without the glow
  around it (a system cursor can cover, not add light). In the browser it is the page's cursor.
- **Updates in the browser:** a new version makes the stored game's code again from the files already in the
  browser when it starts; the CD is needed only once.
- **Window:** any size, the picture scaled with its proportions kept; Alt+Enter or F11 switches to full screen.

## For developers

How it works and how to build it: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md),
[docs/DEVELOPMENT.md](docs/DEVELOPMENT.md).

## License

[MIT](LICENSE) applies to the code in this repository. The game is the property of its rights holders.
