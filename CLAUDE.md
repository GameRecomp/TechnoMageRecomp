# TechnoMageRecomp – orientation for a new working session

Static recompilation of Sunflowers' action RPG *TechnoMage: Return of Eternity* (German: *TechnoMage – Die
Rückkehr der Ewigkeit*, 2000/2001) into **Rust**, on a Rust runtime with a Bevy platform layer (Direct3D on
wgpu). Fifth project on the toolchain of `../TropicoRecomp`, `../CulturesRecomp`, `../HoMM4Recomp` and `../SackedRecomp`
(github.com/GameRecomp); SackedRecomp is the template for the shared recompiler crates. Unlike the sister
projects there is **no C anywhere** (not even for lifted code) and **no SDL**. Start here, then read
`docs/PLAN.md` (milestones, checklists, decision log), `docs/RESEARCH.md` and `docs/TARGETS.md`.

## State (2026-11-24)

M0–M4 done; M5–M8 nearly (docs/PLAN.md). The game boots natively (Bevy window, Direct3D on wgpu) and in the
browser (`technomage-web` + `web/player.js`, JSPI, WebGL2), German, English, French, Italian and Spanish, intro to
the first rooms; the
converter (CLI and page) builds packages from the archive.org media. Open: playing through by hand, saving and
loading, sound by ear, Windows by hand, the v0.1 tag (draft notes in `docs/RELEASE-NOTES.md`).

```sh
cargo run -p rc-cli --release -- lift orig/technomage/cd-de # generated/ (16 shards), then:
cargo build --release -p technomage                         # ~7 min
target/release/technomage --game-dir orig/technomage/cd-de  # RC_* options: docs/DEVELOPMENT.md
tools/audit.sh                                              # what the pre-commit hook runs
```

VS Code: `CMakeLists.txt` + `.vscode/` wrap the same cargo commands (lift, build, debug; docs/DEVELOPMENT.md).
Scripted runs (`RC_HEADLESS`, `RC_INPUT`, `RC_FRAMES`, `RC_SCREENSHOT`) and the headless browser tests
(`tools/play-node.mjs`, `tools/web-test/`) are in docs/DEVELOPMENT.md. `/tmp` is a RAM disk: large artifacts go
under `target/`.

## Facts that matter

- Inputs (git-ignored, in `orig/`): the archive.org media listed in `docs/TARGETS.md`, abandonware; the converter
  downloads them itself. Five languages: German, English, French, Italian, Spanish; all but German run on the
  German cover CD's executable (a supplement) with their own `techmage.cfg`. German: SCREENFUN 03/2004 cover CD (`2004-03.BIN`, raw MODE2/2352, InstallShield 7).
  English: `TECHNOMAGE.mdf` (raw MODE1/2352, InstallShield 5), served by archive.org straight out of its
  `Disc.rar` (no RAR reader). German is version 1.04 (the last patch) without SecuROM: no patch applied.
- `TechnoMage.exe` is **UPX 1.07** packed; restored: MSVC 6, static CRT, base `0x400000`, `.text` 1.32 MiB plus a
  second code section `_GMCODE`, **with relocations**. Imports: kernel32, user32, gdi32 (gamma ramp only), ole32,
  winmm, `mss32.dll` (Miles 6, 45 functions, 3D samples), `binkw32.dll` (10). DirectDraw/Direct3D 7 and
  DirectInput are loaded with `LoadLibraryA`. No registry, no network. Settings in `TechnoMage.ini` (binary).
- Videos, music and speech stay on the CD (`Mov/`, `Mus/`, `Spc/`); the game looks for its CD drive.

## Conventions (same as the sister projects)

- **Rust only**: no C/C++/assembly sources, no SDL; `tools/audit.sh` refuses C sources.
- Shared recompiler code stays general; game-specific behaviour goes into `games/technomage/`.
- Never commit, upload or paste game files, generated code, disassembly or decompiled code. Addresses and
  behaviour described in our own words are fine.
- Every source file starts with `SPDX-License-Identifier: MIT`; no absolute home paths.
- Docs, code and comments in English (the maintainer writes in German).
- Commits in logical steps; author **and** committer `TerminShock <357108427+TerminShock@users.noreply.github.com>`
  (as in the initial commit). Never the local git identity. Messages describe the change only: no AI tools, models
  or attribution trailers (`tools/hooks/commit-msg` enforces it).
- No personal data anywhere in the repository.
- Keep the docs current: tick `docs/PLAN.md`, findings to `docs/RESEARCH.md` (unknowns **(open)**), decisions to
  the decision log, new builds to `docs/TARGETS.md` and the manifest first.
