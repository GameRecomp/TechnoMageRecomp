# TechnoMageRecomp

**Play *TechnoMage: Return of Eternity* (*TechnoMage – Die Rückkehr der Ewigkeit*) natively on Windows and
Linux, or right in your browser.** The game by Sunflowers is abandonware and archived on archive.org; the
converter downloads the release in the language you choose (German or English) and builds the game from it, on
your machine.

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
| TechnoMage | German (SCREENFUN 03/2004 full version), English (European release) |

Each is pinned by its checksum ([docs/TARGETS.md](docs/TARGETS.md)).

## State

Early but playable on Linux: the intro, the menus and a new game in German or English run, Direct3D on the GPU.
The browser build and checked Windows builds are still to come. See [docs/PLAN.md](docs/PLAN.md).

## Playing

1. Download the converter for your system from the releases.
2. Run `recomp download --dest media technomage de` (or `en`): it fetches the release from archive.org and
   checks it.
3. Run `recomp convert media/2004-03.BIN --system linux` (English: `media/TECHNOMAGE.mdf`): it writes a zip with
   the game ready to run. Unpack it and start `technomage`.

## For developers

How it works and how to build it: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md),
[docs/DEVELOPMENT.md](docs/DEVELOPMENT.md).

## License

[MIT](LICENSE) applies to the code in this repository. The game is the property of its rights holders.
