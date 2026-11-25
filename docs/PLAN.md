# Plan

## Goals

1. **TechnoMage runs natively** from its original executable: *TechnoMage: Return of Eternity* /
   *TechnoMage – Die Rückkehr der Ewigkeit* (Sunflowers, 2000/2001), in **German and English**, the two releases
   archived on archive.org (see [TARGETS.md](TARGETS.md)), each at the **newest official patch level** of its
   language (see "Patches" below).
2. **Platforms: Windows, Linux and the browser (WebAssembly).** macOS is built in CI but not offered
   (`platforms.json`, as in the sister projects). No Android.
3. **Rust only.** The recompiler, the runtime, the HLE layer, the platform layer, the converter **and the lifted
   game code** are Rust. No C, no C++, no assembly sources, no C compiler in any build. `tools/audit.sh` refuses C
   sources.
4. **Rust-native platform, no SDL: Bevy.** The platform layer is a Bevy app: window, input, gamepads, audio
   and the frame loop on Windows, Linux and in the browser. Rendering goes through Bevy's **wgpu** device (the
   game uses Direct3D 7, so the platform needs a real 3D API, also in the browser through WebGPU/WebGL2).
5. **Same architecture** as TropicoRecomp, CulturesRecomp, HoMM4Recomp and SackedRecomp: the Rust recompiler crates
   (`recomp/`) taken over from SackedRecomp, the manifest, the converter (CLI and browser page), CI and the
   packaging. The runtime, HLE and platform layers follow the sister projects' C design module for module, so
   behaviour and fixes can travel both ways.
6. **Faithful.** The game logic is the original code, at the game's own resolutions. No widescreen patch in v0.1.
7. **Abandonware download built in, as in SackedRecomp.** The converter (CLI and browser page) fetches the medium of
   the chosen language from a **fixed archive.org URL** in the manifest (the page offers the languages as flags),
   checks it against the pinned SHA-256 and converts it; a local copy of the same file is accepted too. A file
   inside an archive on archive.org is linked directly (`<item>/Disc.rar/TECHNOMAGE.mdf`): nothing is unpacked.

Non-goals: shipping game code or data, remastering assets, widescreen patches, Android, other releases than the
pinned ones until their hashes are known, the options program `TM_Setup.exe` (the shell offers its settings).

## Starting point

Everything below "the game" exists in the sister projects, but in C for the runtime side:

| Piece | Source | Work here |
|-------|--------|-----------|
| `rc-pe`, `rc-manifest`, `rc-discover`, `rc-ir`, `rc-x86`, `rc-difftest` | SackedRecomp (`faf8fc7`) | relocations of the main executable used as exact pointer list; `_GMCODE` as second code section; difftest reference side in Rust (`asm!`, i686) instead of C |
| `rc-media` (raw CD, ISO 9660, InstallShield 5/6, Inno, NSIS, MSI, zip) | SackedRecomp | **InstallShield 7** (version check, the 20 `Sfx/*.SBK` that fail to inflate today), CD-resident folders copied beside the install; the English `.mdf` (raw MODE1) and InstallShield 5 are read already |
| – | – | **`rc-unpack`** (new): UPX 1.07 for Win32 PE |
| `rc-emit-c` | SackedRecomp | **replaced by `rc-emit-rust`** (new) |
| `rc-emit-native`, `rc-emit-wasm`, `rc-convert`, `rc-web`, `rc-cli` | SackedRecomp | one game, two languages, the Rust shell as template; the web runtime is Rust (wasm-bindgen), not Emscripten |
| `runtime/` (memory, CPU, x87, dispatch, loader, SEH, threads, clock, files) | C11 in the sister projects | **port to Rust** (`rc-runtime`) with the same `Cpu` layout and helper table |
| `hle/` kernel32, user32, gdi32, ole32, winmm, ddraw, dinput, dsound, mss32 | C11 in the sister projects | **port to Rust** (`rc-hle`), only what TechnoMage and `binkw32.dll` import; **new: Direct3D 7 on wgpu**, Miles 3D samples |
| `platform/` SDL3 + null | C11 in the sister projects | **new in Rust**: a Bevy app (window, input, audio, the frame; Direct3D on its wgpu device), null backend |
| `web/`, `tools/`, `.github/workflows` | SackedRecomp | rename, one game, two language flags, the wasm-bindgen player |

The code is copied at pinned commits (decision log), not vendored as a submodule. Upstream heads checked on
2026-11-22: TropicoRecomp `90809b1`, CulturesRecomp `dfaf902`, HoMM4Recomp `74b80f2`, SackedRecomp `faf8fc7`.

The game, measured from the German CD (details in [RESEARCH.md](RESEARCH.md)):

- **Medium**: `2004-03.BIN`, a raw MODE2/2352 image (807 MiB) of the SCREENFUN 03/2004 cover CD: the magazine's
  menu and extras, an **InstallShield 7** setup (`data1.hdr`, `data1.cab`, `data2.cab`, 173 MB) and the folders
  the game reads from the CD at run time: `Mov/` (Bink videos, 200 MB), `Mus/` (WAV music, 230 MB) and `Spc/`
  (speech `.PCK`, 148 MB).
- **`TechnoMage.exe`**: 574 976 bytes, **UPX 1.07** packed (linked 2001-03-19). Restored: 1 863 680 bytes, PE32,
  MSVC 6 (linker 6.0), static CRT, base `0x400000`, `.text` 1.32 MiB, `_GMCODE` 6.8 KiB, `.data` 232 KiB (with a
  16 MiB virtual extent before `.idata`), `_GMDATA`, and **`.reloc`** (157 KiB).
- **Imports**: kernel32 (81), user32 (32), gdi32 (2: the gamma ramp), ole32 (3), winmm (1), `mss32.dll` (45:
  Miles 6 with 3D samples, providers, listener, room types), `binkw32.dll` (10). DirectDraw, Direct3D and
  DirectInput through `LoadLibraryA` ("You need DirectX Version >= 7.0"). No advapi32 (no registry), no network.
- **Shipped DLLs**: `binkw32.dll` (2000-05-04, base `0x10000000`), `Mss32.dll` (2000-10-23, base `0x21100000`)
  with nine `.m3d` 3D providers.
- **Data**: `Gfx/` (`.TPL` graphics, low and `Hi/` sets), `Lev/` (levels, maps), `Rul/`, `Sfx/` (`.SBK` sound
  banks), `Txt/Ger/` (dialogue `.dlg`), `DATA.OVL`, `resource.cmp`, `default.pal`, `Music.cfg`.

## Order of work

```
 M0 ─ M1 ─ M2 ─ M3 ─ M4 boots ─ M5 picture (DDraw + D3D on Bevy/wgpu) ─ M6 sound, video ─ M7 in game ─ M9 release
                └──────────── M3r runtime port (parallel from M1) ──┘                └─ M8 converter, web
```

The runtime port to Rust (M3r) is the big new piece; it starts in M1, in parallel to the media and the unpacker,
because the lifted code has nothing to run on without it. German leads; English follows from M7 (same engine;
the code differences are measured in M2).

## Milestones

Status: ✅ done · 🔶 in progress · ⬜ not started

| #   | Milestone | Status | Exit criterion |
|-----|-----------|--------|----------------|
| M0  | Foundation | ✅ | Plan and docs; workspace builds (stubs); shared recompiler crates imported from SackedRecomp at the pinned commit; `rc-emit-c` removed; CI and audit hook; `games/technomage/game.toml` pins both media and every binary; `recomp identify` recognises them. |
| M1  | Read the inputs | ✅ | `recomp extract` turns both media into `orig/technomage/<variant>/` without third-party tools: InstallShield 7 and 5 by their MD5s, the CD folders copied, every binary verified by hash; `recomp unpack` restores `TechnoMage.exe` byte-identical to `upx -d`. |
| M2  | Find the code | 🔶 | Coverage report for the restored `TechnoMage.exe` (both languages, patched) and `binkw32.dll`: 100 % classified, every leftover settled by analysis or a manifest hint. Direct3D interface level confirmed. |
| M3  | Lift and build | ✅ | Every function lifts; no trap in reachable code; `rc-emit-rust` output compiles warning-free and links into `technomage`; difftests pass with the Rust reference side. |
| M3r | Runtime in Rust | ✅ | `rc-runtime` passes the ported runtime tests (memory, dispatch, x87, SEH, threads); the Cranelift blob of a test program runs on it; a hand-written guest test program calls HLE kernel32. |
| M4  | Boots | ✅ | Static CRT start-up, files and the virtual CD drive, threads; the game creates its window and DirectDraw 7. |
| M5  | Picture and input | 🔶 | The main menu renders through DirectDraw in the Bevy app; Direct3D 7 draws the first level (geometry, textures, alpha, z, fog, lights); mouse and keyboard through DirectInput; windowed and full screen; gamma ramp. |
| M6  | Sound and video | 🔶 | Miles samples (2D and 3D), music (`Mus/*.WAV` by `Music.cfg`), speech (`Spc/*.PCK`); the Bink intro and cut scenes through the lifted `binkw32.dll` with DirectSound audio. |
| M7  | In game | 🔶 | All eight worlds of the German release can be played through; saves load; English starts and plays a level; scripted soak runs without trap. |
| M8  | Converter and platforms | 🔶 | `recomp convert` and the browser page download (or take) the medium of the chosen language from its fixed archive.org URL and produce Windows and Linux builds and a browser build; all three start and reach a level. |
| M9  | **Release v0.1** | ⬜ | Played through on Linux, Windows and in the browser; release notes with the known issues. |

### M0 – Foundation

- [x] Plan, architecture, research and target docs; `CLAUDE.md`, `CONTRIBUTING.md`, `README.md`
- [x] Workspace skeleton: `rc-runtime`, `rc-hle`, `rc-platform`, `technomage` shell, `rc-unpack`, `rc-emit-rust`
      (stubs that build, also for `wasm32-unknown-unknown`)
- [x] `tools/audit.sh` (refuses game files, C sources, missing SPDX, home paths), `tools/hooks/` (pre-commit,
      commit-msg), `.gitignore` covers `orig/`, `generated/`, the media and data extensions
- [x] CI skeleton (audit, fmt/clippy/test on Linux, Windows, macOS, wasm build)
- [x] `games/technomage/game.toml`: the German CD and its binaries by hash
- [ ] Copy the recompiler crates from SackedRecomp at `faf8fc7` into `recomp/crates/`: `rc-pe`, `rc-manifest`,
      `rc-media`, `rc-discover`, `rc-ir`, `rc-x86`, `rc-emit-native`, `rc-emit-wasm`, `rc-convert`, `rc-web`,
      `rc-difftest`, `rc-cli`; **not** `rc-emit-c`; the sister games' manifests and fixtures not taken over;
      tests that read other games' manifests adapted
- [ ] `rc-difftest`: the reference side (`tools/difftest/ref32.c`) rewritten as a Rust `i686` program
- [x] The archive.org URLs fixed: `SF2004-03/2004-03.BIN`, and `TechnoMage_Return_of_Eternity_Europe/Disc.rar/TECHNOMAGE.mdf`
      (archive.org serves the member of the RAR directly: no RAR reader)
- [x] The English release measured and pinned (the `.mdf`, its executable and tools)
- [x] Patches: 1.02 and 1.04 (German only) identified; the cover CD is 1.04 already, no English patch (below)
- [ ] `recomp identify` recognises the media, the patch and every binary
- [ ] Icon (`tools/icons/technomage.svg`, our own drawing)
- [ ] CI green on GitHub (nothing pushed yet)

Binary treatment:

| File | Role | Treatment | Why |
|------|------|-----------|-----|
| `TechnoMage.exe` | main | unpack, lift | the game; UPX 1.07 packed |
| `binkw32.dll` | library | lift | shipped; Bink 1 decoder; narrow API below it (kernel32, user32, gdi32, winmm, DirectSound) |
| `Mss32.dll`, `Miles/redist/win32/*.m3d` | library | **hle** | 45 `AIL_*` calls are less work than Miles, nine 3D providers and DirectSound 3D |
| `TM_Setup.exe` | tool | skip | options program; its settings move into the shell (it writes `TechnoMage.ini`) |
| `TM_Diagnostic.exe`, InstallShield engine files, CD menu (`SCREENFUN-CD.exe`, extras) | – | skip | not the game |

### M1 – Read the inputs

- [x] `rc-media` InstallShield: major version 7 accepted (the cabinet layout is version 6's); all files by MD5
- [x] Why 20 `Sfx/*.SBK` fail: the archived German image has 1 608 unreadable sectors inside `DATA2.CAB`
      (RESEARCH); everything else is intact
- [x] A source for the 20 damaged sound banks: the German retail CD 1.00's `data1.hdr`/`data1.cab`, served out of
      its ISO by archive.org (167 MB), pinned as the variant's supplement; `recomp download` fetches it,
      `recomp extract` takes the 20 files from it, each checked by SHA-256
- [x] `recomp extract orig/2004-03.BIN` unpacks the German release completely (164 + 20 files, the CD folders,
      every binary verified)
- [x] The English executable: the English CD's is inside SecuROM; the cover CD's plain 1.04 executable runs the
      English data (`techmage.cfg`: `LANGUAGE = 1`) and is the English variant's supplement
- [ ] The English `TECHNOMAGE.mdf` (raw MODE1/2352) with its InstallShield 5 cabinet, read by the existing
      readers; nested: the cabinets inside the CD image without unpacking to disk
- [x] The CD-resident folders (manifest `cd_dirs`: `Mov/`, `Mus/`, `Spc/`) copied to
      `orig/technomage/<variant>/cd/` with the disc's label in `cd/volume-label.txt`, for the virtual CD drive
- [x] `rc-unpack`: UPX 1.07 Win32/PE (NRV2D, the call-trick filter `0x26`, imports rebuilt from the packer's
      table, relocations restored), `recomp unpack`; byte-identical to `upx -d` 4.2.4 for the German executable
- [ ] `recomp download technomage de|en` from the fixed archive.org URLs (curl), size and SHA-256 checked
- [ ] Ignored tests `orig_extract` for every medium in `orig/`

### M2 – Find the code

- [ ] Discovery on the restored `TechnoMage.exe` with its relocations as the pointer list; `_GMCODE` (what is it:
      **open**); vtables; SEH scope tables of the static CRT
- [ ] `binkw32.dll` (with its relocations; base `0x10000000` free, Miles is HLE)
- [ ] `recomp codediff` between the German and the English executable (and the patch levels)
- [ ] The Direct3D interface level and the calls made (`IDirect3D7`/`IDirect3DDevice7` expected; the executable
      carries the GUIDs of every DirectX 7 interface, so confirm by the `QueryInterface` calls)
- [ ] CPU features: x87 precision control, MMX or 3DNow! paths (Sunflowers' engines of the time had some)

### M3 – Lift and build

- [ ] `rc-emit-rust`: functions, blocks as `loop { match }`, the dispatch table, the binary record; rustfmt-clean,
      warning-free output; module split so `rustc` stays within memory (per 64 KiB, `codegen-units`)
- [x] `games/technomage` includes `generated/` through `build.rs` when present; the shell maps the executable
      and calls its entry point (`--check` lists missing imports)
- [x] First measurements: 2 936 functions, 363 297 instructions, 82 MB of Rust (109 MB before the flag
      liveness and the grouping by body). One crate builds in more than 20 minutes on 8 cores with `opt-level
      = 1`: `rustc`'s front end runs on one core, and only code generation uses more than two
- [ ] **Shards**: the functions spread over 16 fixed shard crates (`games/technomage/lifted/shardNN`, each
      including its part of `generated/` through `build.rs`), balanced by size, so cargo builds them in parallel.
      A call into another shard goes through an `unsafe extern "Rust"` block of `safe fn` declarations with
      `#[no_mangle]` names, generated from the same table as the definitions (the one place generated code says
      `unsafe`). Target: a full build under 10 minutes on 8 cores, an incremental one in a minute
- [ ] For quick turns while developing: `recomp convert --system linux` (Cranelift, no `rustc`) with the Rust
      shell as the template (M8's path, brought forward)
- [ ] Difftests pass (Rust reference side)

### M3r – Runtime in Rust

- [x] `mem`: 4 GiB reservation (native, pages committed on first touch), a checked region elsewhere; `vm`:
      page regions and heaps (Windows still commits the whole block: a reserve/commit scheme is to do)
- [x] `cpu` (the `rc_cpu` layout, checked by a test), `x87` (fast mode), `mmx`, `cpuid`, `rdtsc`
- [ ] the helper table for native and wasm code
- [x] `dispatch` (patches, lifted tables, import resolver, per-thread cache), traps, unwind claim/begin,
      callbacks into guest code; `process` (PE mapping, packed executables restored, relocations, import
      binding, TIB/PEB, the module list); `imports` (reserved addresses, the HLE registry, `RC_TRACE=imports`)
- [ ] `seh` (`_except_handler3`, unwinding), `threads` (baton over OS threads; JSPI in the browser)
- [x] `clock` (real/virtual), `vfs` (case-insensitive, the virtual CD drive, user directory for writes), `handles`
- [ ] `native`: the Cranelift blob loaded into executable memory (the converter's path)
- [ ] The sister projects' runtime tests ported

### M4 – Boots

- [x] kernel32: all 81 imports of the game and those of `binkw32.dll` (CRT start-up, heap, files,
      `FindFirstFile`, code pages, synchronisation objects, threads, `LoadLibraryA`/`GetProcAddress` for HLE
      DLLs), SEH (`RaiseException`, `RtlUnwind` over the guest's `fs:[0]` chain)
- [x] user32 (the window, message queue, cursor), gdi32 (gamma ramp), ole32 (COM classes), winmm
- [x] The DirectX version check (DirectDraw 1, 2, 4, 7 and every surface interface); DirectDraw and
      DirectInput through `CoCreateInstance` as well as their DLLs
- [x] The CD check passes on the virtual drive (`D:`, the disc's label), no patch needed
- [x] `binkw32.dll` lifted, mapped at its base, `DllMain` run, the executable's imports bound to its exports
- [x] Guest threads (Bink's), cooperative with a baton

### M5 – Picture and input

- [x] The intro (Bink into the primary surface), the main menu, a new game in Dreamertown (German and English)
- [x] DirectDraw 7 in software: surfaces in guest memory, `Blt`/`BltFast` with stretch, keys and fills,
      `Flip`; pixel formats 565, 1555, 4444, 8888
- [x] Direct3D 7 object layer: device and z-buffer enumeration, render and stage states, transforms, viewport,
      material, lights, textures, vertex buffers, `Load`, `Clear`
- [x] Software rasterizer of the fixed-function pipeline (reference, null backend, fallback)
- [x] **GPU renderer** with wgpu (`rc-platform` feature `gpu`): about six times the software path
- [x] DirectInput 7 keyboard and mouse, state and buffered events, object names; window messages
- [x] The Bevy window: frames scaled with the aspect kept, keyboard, mouse, wheel, pointer mapping
- [x] Gamma ramp in the frame conversion
- [x] Full screen toggle in the Bevy window (Alt+Enter, F11); the page's full screen button in the browser
- [ ] The `TECHMAGE.CFG` options (`WINDOWED`, `LO-RES`) checked by hand
- [x] Near-plane clipping of transformed triangles (z = 0 in clip space), for both renderers

### M6 – Sound and video

- [x] Mixer (PCM, IMA ADPCM, volume, pan, loops, resampling) as one Bevy audio source
- [x] Miles 2D and 3D samples (distance attenuation and pan by the listener), music by `AIL_set_sample_file`
- [x] DirectSound with streaming buffers for Bink's sound (through `AIL_get_DirectSound_info`)
- [ ] Heard on a machine with an audio device (this development machine has none in its session)
- [x] Speech (`Spc/*.PCK`): the game falls back to `D:\SPC\` and reads it from the virtual drive (RESEARCH.md)

### M7 – In game

- [x] A new game starts in German and English; dialogue, tutorial, the first room
- [x] Scripted runs: `RC_INPUT` (keys and clicks at frames), `RC_FRAMES`, `RC_SCREENSHOT`, `RC_TRACE=fps`
- [x] Scripted soak: 20,000 frames of clicks and keys in the first room without a trap (RESEARCH.md)
- [x] Leaving through the main menu writes `TechnoMage.ini`; the next start reads it; the save book opens
- [x] English through the converter (the German CD's executable as supplement) runs to the first room in the
      browser player
- [ ] All eight worlds played through by hand; saving and loading a game; a one-hour soak

### M8 – Converter and platforms

- [x] `recomp download`/`extract` with supplements; `recomp convert` builds with cargo into `out/technomage/`
- [x] Native code from the converter: the runtime loads Cranelift blobs (format 2); the shell runs a package's
      `data/code-<target>-technomage.exe.bin` with `data/game`; `RC_NO_GENERATED` builds the template shell
- [x] CI: audit, fmt, clippy, tests on Linux/Windows/macOS, wasm crates; release workflow for the converter
      (Linux, Windows); Pages workflow
- [x] `recomp convert --system linux` end to end: a template built here, the German CD converted (supplements,
      CD folders), the package runs to the first room on its Cranelift code
- [x] Converter page: languages downloaded from archive.org with their supplements, or local copies; the wasm
      converter in a worker writes the same package as `recomp convert` (checked in Node)
- [x] Browser player: the Rust runtime for `wasm32-unknown-unknown`, guest code from `rc-emit-wasm`, guest
      threads on JSPI, frames on a canvas, Direct3D on WebGL2, input, Web Audio, the game's writes kept in the
      browser's storage, reads in blocks; runs to the first room in Node (software) and headless Firefox (WebGL2);
      the page's whole flow (worker → OPFS → `play.html`) scripted in headless Firefox (`tools/web-test/flow.html`)
- [x] Windows: the shell and `recomp` check for `x86_64-pc-windows-msvc` (cross, without linking)
- [ ] The page's flow checked in a real browser with a person at the keyboard (sound, input, saving)
- [ ] Windows build checked on Windows

### M9 – Release v0.1

- [ ] Played through on Linux, Windows and in the browser, in both languages (by hand)
- [x] Release notes with the known issues (`docs/RELEASE-NOTES.md`, a draft until the tag); README for players
- [ ] Tag `v0.1.0` and publish the release (the release workflow attaches the converter)

## Patches

Sunflowers released two official patches, both German only: 1.02 (by January 2001) and 1.04 (2001-03-23, on
archive.org as `TechnoMageRoE/technomage_patch104_ger.exe`). There is no English patch. Details in
[RESEARCH.md](RESEARCH.md) and [TARGETS.md](TARGETS.md).

- **German**: the SCREENFUN 03/2004 cover CD **is 1.04**: its executable has the same link time as the patch's
  (2001-03-19 18:43:50) and every level and data file the patch carries is identical. The patch's executable is
  the same build inside a SecuROM wrapper; the cover CD's is without. Nothing is applied.
- **English**: mastered 2001-05-03, after 1.04; its version is read from `DATA.TAG` and the executable in M0. No
  patch exists for it either way, so it is played as shipped.
- The rule stays: each language at its newest official patch level. Should a newer build turn up, it is pinned like
  a medium (fixed archive.org URL, size, SHA-256) and applied by `recomp extract`.

## Platform: Bevy

The platform layer is a **Bevy** app. Bevy brings the window, keyboard, mouse and gamepads, audio, full screen,
screenshots, diagnostics, an egui overlay for the settings `TM_Setup.exe` used to make (`bevy_egui`), and the
same app in the browser. Underneath it is wgpu, winit and cpal, the crates this layer would otherwise use
directly.

How the recompiled game fits into it:

- **Who owns the loop.** Natively Bevy does: the guest (the game's own message loop and frame) runs on its own
  thread, hands over finished frames and takes input through the HLE layer's queues. In the browser there is no
  Bevy (decision log): the guest is a JSPI coroutine on the page's thread that gives way at every frame.
- **Drawing.** As built (decision log): DirectDraw surfaces live in guest memory; Direct3D's transform and
  lighting run on the CPU and the triangles of a render target are drawn by a renderer of their own offscreen
  (wgpu natively, WebGL2 in the browser) over the target's pixels, which are read back into guest memory when
  the CPU needs them. The Bevy window shows the finished frame as a sprite scaled with its aspect kept; the gamma
  ramp is applied in the frame conversion.
- **Audio.** The HLE mixer (Miles samples, DirectSound buffers, the music and speech streams) is one custom
  `bevy_audio` source that pulls PCM from the guest side's ring buffer.
- **Input.** Bevy's keyboard, mouse-motion and button events become window messages and DirectInput state for the
  guest.
- **Scripted runs and CI.** The null backend (no Bevy): frames kept for screenshots, scripted input, virtual clock,
  as in the sister projects. Bevy's own time is not used for game time.
- **Costs and how they are handled.** Bevy is a large dependency with slow clean builds and a breaking release
  every few months: the version is pinned in the workspace, only the needed Bevy features are enabled
  (`default-features = false`), and upgrades are their own commits. Bevy pins its wgpu version, so our Direct3D
  code follows it.

## Definition of done (per release)

- The game can be played to the end on Linux, Windows and in the browser, in German and English.
- No trap and no unimplemented import in a one-hour soak.
- The converter produces the builds from the archive.org media and from local copies.
- Docs current: PLAN ticked, RESEARCH answered, TARGETS and the manifest in sync.

## Risks

| Risk | Impact | Plan |
|------|--------|------|
| Runtime, HLE and platform rewritten in Rust (first in the family) | months of work before the first picture; fixes no longer copy 1:1 between the repositories | port module for module with the same names and tests; keep the `Cpu` layout and helper table identical, so the shared back ends stay shared |
| Lifted code as Rust: `rustc` on ~1.3 MiB of x86 | measured: more than 20 minutes for one crate, 4 GB of memory | dead flag settings dropped, functions grouped by body, shards built in parallel (M3), Cranelift for quick turns; the end-user path uses Cranelift and needs no `rustc` |
| Direct3D 7 fixed function on wgpu | wrong blending, fog, texture combiners; z-fighting | state-keyed WGSL generator, unit tests per state, screenshot comparisons with a reference capture from Windows (by hand) |
| Mixed 2D/3D (locks of the back buffer, blits onto 3D) | GPU read-backs per frame, slow in the browser | track which surfaces are CPU- or GPU-resident; read back only when the game locks |
| UPX unpacking | wrong image | byte-identical to `upx -d`; both hashes pinned |
| Data on the CD (Mov, Mus, Spc: ~580 MB) | big downloads, browser storage limits | copy only what the game reads; stream from the medium; ask for persistent storage |
| Miles 3D audio in HLE | wrong spatial sound | simple model first (attenuation, pan); the game's own distances |
| JSPI and wgpu in browsers | Firefox/Safari support | JSPI is in Chrome and Firefox; WebGL2 fallback for wgpu; Safari not a target for v0.1 |
| archive.org items change or vanish | download fails | pinned SHA-256 refuses changed files; local media keep working |

## Research questions

Tracked with answers in [RESEARCH.md](RESEARCH.md):

1. Which patch versions exist per language, which version does each archived medium carry?
2. ~~What does the English `Disc.rar` hold?~~ An Alcohol image, served directly by archive.org (RESEARCH). Its version and copy protection?
3. Direct3D 7 or an older interface (`IDirect3D3`)? Which render and texture-stage states are used?
4. What is `_GMCODE`/`_GMDATA`?
5. How does the CD check work (`GetDriveTypeA`, `GetVolumeInformationA`, a file on the disc)?
6. Formats: `.TPL`, `.cmp`, `.PCK`, `.SBK`, `DATA.OVL`, `TechnoMage.ini` (only as far as the HLE needs it).
7. Which threads does the game start (loading, music)?
8. Do the German and English executables differ beyond text?

## Decision log

| Date | Decision |
|------|----------|
| 2026-11-22 | The project follows TropicoRecomp, CulturesRecomp, HoMM4Recomp and SackedRecomp: MIT license, the Rust recompiler crates, the manifest, the converter with CLI and browser page. Platforms: Windows, Linux, browser; macOS built in CI, not offered; no Android. |
| 2026-11-22 | **Rust only**, also for the lifted code: `rc-emit-rust` replaces `rc-emit-c`; the runtime, HLE and platform layers are written in Rust after the sister projects' C design. No C compiler in any build; the audit refuses C sources. |
| 2026-11-22 | **No SDL**. wgpu for rendering, because the game renders with Direct3D 7 and the browser build needs WebGPU/WebGL2. |
| 2026-11-22 | SackedRecomp (`faf8fc7`) is the template for the shared recompiler crates (it contains HoMM4Recomp's shared code plus raw CD images, InstallShield 6, the archive.org download and the language flags). |
| 2026-11-22 | Targets: German (SCREENFUN 03/2004 cover CD, archive.org `SF2004-03`) and English (European release, archive.org `TechnoMage_Return_of_Eternity_Europe`, `TECHNOMAGE.mdf` inside `Disc.rar`), pinned by hash with fixed download URLs, as SackedRecomp does. Each at its newest official patch level. |
| 2026-11-22 | No RAR reader: archive.org serves the members of an archive directly (`<item>/Disc.rar/TECHNOMAGE.mdf`, with CORS through `/cors/`), so the manifest links the image itself. |
| 2026-11-23 | The German cover CD's 20 damaged sound banks come from the German retail CD 1.00's cabinet (archive.org serves `TechnoMage.iso/data1.cab` on its own, 167 MB), not from the English CD (840 MB) or the RAR patch. `HBoss.SBK` is therefore version 1.00's: patch 1.04 changed it, but taking it from the patch would need a RAR 2.0 reader. Accepted as a documented deviation. |
| 2026-11-23 | **Direct3D on its own wgpu device, offscreen, read back into guest memory**, not in a Bevy render-graph node: the guest's surfaces stay the one truth (2D `Lock`/`Blt` work mixes with 3D as on Windows), the null backend and CI use the same path, and the Bevy window only shows finished frames. Transform and lighting stay on the CPU (a few thousand vertices a frame). The software rasterizer remains as reference and fallback (`RC_RENDERER=soft`). |
| 2026-11-23 | Lifted Rust in **16 shard crates** that depend only on **`rc-abi`** (CPU state, memory, x87, MMX; the runtime's entry points as `extern` declarations defined by `rc-runtime`): a full build takes about 5 minutes (dev, unoptimised shards) or 7 (release) instead of more than 40, and runtime changes do not rebuild it. |
| 2026-11-24 | **The browser player has no Bevy**: the runtime and the HLE layer build for `wasm32-unknown-unknown` with plain imports (`rc_js`) from a small JavaScript core (`web/player.js`) instead of wasm-bindgen and Bevy's web backend. JSPI needs the imports wrapped (`WebAssembly.Suspending`) and the entries made `promising`, which wasm-bindgen's glue does not do; the page needs only a canvas, input events and Web Audio. Each guest thread gets a JSPI stack and its own shadow stack (`__stack_pointer` is exported and swapped around every wait). |
| 2026-11-25 | **CMake as an optional IDE front end** (`CMakeLists.txt`, `CMakePresets.json`, `.vscode/`): custom targets that call cargo and `recomp` (lift, build, run, checks), with no language enabled, so VS Code's CMake Tools can lift, build and debug as in the sister projects. Cargo remains the build system; nothing requires CMake. |
| 2026-11-25 | **Changes to the game as hooks in Rust** (`games/technomage/src/patches.rs`, `[binary.patch] hooks` in `game.toml`), not as edited data files: the pause menu gets "Configure graphics", built from a copy of its table with the callbacks' entry indices mapped back, and opens the game's own options panel with its entries swapped for one line (the render resolution) while it is shown. New texts come from our table in the game's language through the game's string lookup, so no file of the install is changed and every language works. |
| 2026-11-25 | **The mouse pointer as a hardware cursor** (with a window): the game's pointer primitives are taken out of the frame's ordering table, drawn on their own through the game's renderer into a captured list, rendered by the software rasterizer over black and over white (alpha from the difference) and shown as the system cursor at the frame's scale; animated (it sways and turns), without the additive glow, at most 256 distinct images (Bevy keeps a system cursor per image), then the most similar one. Headless and in the browser the game draws its own. |
| 2026-11-24 | **Direct3D in the browser on WebGL2 from JavaScript** (`web/gl.js`), fed the same batches as the wgpu renderer: WebGL2's `readPixels` reads back synchronously, so the guest's surfaces stay in guest memory as natively; wgpu on WebGPU would need an asynchronous map per batch. 35 frames a second in the first room in headless Firefox, 4.5 in software. |
| 2026-11-24 | **Guest memory tops out at 768 MiB in the browser** (`GUEST_TOP`): the runtime's own memory and the game's 4 GiB address space do not both fit WebAssembly's 4 GiB; the heaps, stacks and information blocks go below the top instead of below 2 GiB. |
| 2026-11-23 | **English runs on the German cover CD's executable.** The English CD's executable is wrapped in SecuROM with part of it encrypted; undoing copy protection is not done here. The cover CD's 1.04 executable has none and reads its language from `techmage.cfg`, so the English variant downloads the cover CD image as a supplement for its executable. |
| 2026-11-23 | From SackedRecomp `0dbd1df`: `rc-emit-wasm`'s flag liveness (`flags.rs`, now public and also used by `rc-emit-rust`) and its MMX in WebAssembly SIMD. The browser lessons (frames paced by `requestAnimationFrame`, read-ahead of the game's files, buffered small reads) are requirements for M8. |
| 2026-11-22 | German needs no patch: the cover CD is 1.04, the last official patch, without SecuROM. There is no English patch. Patch 1.04 is kept in TARGETS as a reference only. |
| 2026-11-22 | `binkw32.dll` is lifted (as in HoMM4Recomp); `Mss32.dll` and its `.m3d` providers are HLE. |
| 2026-11-22 | `TechnoMage.exe` is restored from UPX by our own `rc-unpack` (no external `upx` in the pipeline); `upx -d` is only the test reference. |
| 2026-11-22 | **Bevy is the platform** (section "Platform: Bevy"): window, input, gamepads, audio, the frame loop and the browser build come from it; Direct3D and DirectDraw are drawn with our own wgpu code on Bevy's render device. The `rc-platform` API keeps Bevy out of the HLE layer, and a null backend stays for tests. |
| 2026-11-22 | Cooperative guest threads: OS threads with a baton natively, JSPI in the browser (no ucontext/fibers in Rust). |
