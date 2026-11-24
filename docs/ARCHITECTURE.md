# Architecture

## Overview

```
 your game files            recompiler (Rust, recomp/)                    build (cargo, Rust only)
 ─────────────────          ──────────────────────────────────            ─────────────────────────────
 CD image (.bin), ─┐
 .mdf (English)   ├─ extract ─► orig/technomage/<variant>/                generated/technomage/<bin>-<variant>/*.rs
 (archive.org)   ─┘      (InstallShield 7, CD files, patch)                         │
                                     │                                              ▼
                     unpack ─► load ─► discover ─► lift ─► emit Rust ───►  + runtime/ + hle/ + platform/
                  rc-unpack   rc-pe  rc-discover  rc-x86  rc-emit-rust     + games/technomage/ (shell, patches)
                                         ▲                                              │
                    games/technomage/game.toml (hashes, seeds, patches)                 ▼
                                                                          native program (cargo build)

 end users (no compiler):  medium ─► recomp convert ─► Cranelift (rc-emit-native) ─► shell template + code blob
                           medium ─► converter page ─► WebAssembly (rc-emit-wasm) ─► browser player
```

The recompiler runs **ahead of time**. It turns the pinned executable (after restoring it from its UPX packing)
and each DLL marked `lift` into code. The program links that code against the runtime, which provides guest
memory, the dispatcher and the HLE implementations of every imported API. At run time the game reads its own data
files from the user's install. We never convert or redistribute assets.

This is the fifth project on the toolchain of TropicoRecomp, CulturesRecomp, HoMM4Recomp and SackedRecomp, and the
first one **without C**: the sister projects' C11 runtime, HLE and platform layers and their C back end are
written in Rust here, and SDL3 is replaced by Bevy (decision log in [PLAN.md](PLAN.md)).

## What is different from the sister projects

| Sister projects | TechnoMage | Why |
|-----------------|------------|-----|
| `rc-emit-c` → C11, CMake, gcc/clang/MSVC | `rc-emit-rust` → Rust, cargo | Rust only |
| C11 runtime, HLE, platform | Rust crates `rc-runtime`, `rc-hle`, `rc-platform` | Rust only |
| SDL3, SDL3_ttf, SDL3_mixer | Bevy (on wgpu, winit, cpal) | Rust-native; wgpu because the game renders in 3D |
| 2D DirectDraw (software blits) | DirectDraw 7 in guest memory **and Direct3D 7**: transform and lighting on the CPU, the pixel pipeline on wgpu (WebGL2 in the browser), a software rasterizer as fallback | the game uses Direct3D |
| Emscripten web shell (C runtime) | the Rust runtime for `wasm32-unknown-unknown` with plain imports and JSPI (`technomage-web`, `web/player.js`) | no C compiler |
| one C file per 64 KiB of lifted code | lifted Rust in 16 shard crates on `rc-abi` | build time (decision log) |
| executables as shipped | **UPX 1.07** packed: `rc-unpack` restores the image first | the retail exe is packed |
| no relocations: heuristics find code pointers | the restored image **has `.reloc`**: every pointer is known | exact discovery |
| Inno, NSIS, InstallShield 5/6, MSI | InstallShield **7** and 5, data on the CD | the two pinned media |

## Pipeline (recomp/)

The crates are SackedRecomp's (the newest sister project, which contains HoMM4Recomp's shared code), copied at a
pinned commit in M0. New or changed crates are marked.

| Stage    | Crate | Job |
|----------|-------|-----|
| identify | `rc-manifest` | Load `games/technomage/game.toml`, match files by SHA-256 against all variants. |
| extract  | `rc-media` (+ `iscab` v7) | Read the raw CD images (German `.bin` MODE2/2352, English Alcohol `.mdf` MODE1/2352, which archive.org serves straight out of its `Disc.rar`) as ISO 9660, the InstallShield cabinets on them (German: version 7, `data1.hdr` + `data1.cab`, `data2.cab`; English: version 5, `DATA1.HDR` + `DATA1.CAB`), and copy the CD-resident folders (`Mov/`, `Mus/`, `Spc/`) beside the install. Every file is checked (MD5 from the cabinet, SHA-256 from the manifest). The release's name goes to `recomp-variant.txt`. `recomp download` fetches a release from archive.org first. |
| unpack   | **`rc-unpack`** (new) | Restore `TechnoMage.exe` from UPX 1.07: NRV decompression, the `call`/`jmp` filter, imports and relocations. The result is byte-identical to `upx -d` (the reference in tests) and pinned by its own hash. |
| load     | `rc-pe` | Parse the PE: sections, imports, exports, relocations, resources. Refuse any binary whose hash is not in a manifest. |
| discover | `rc-discover` | The code map. Recursive descent from the entry point, exports and manifest seeds; `switch` tables; **the relocation table gives every absolute pointer** in code and data, so candidates are exact (as the sister projects already do for DLLs). Then gap filling, no-return functions, the coverage report. `_GMCODE` is a second code section. |
| lift     | `rc-x86` → `rc-ir` | Decode with `iced-x86`; integer, string, x87 (fast mode) and MMX into IR; unmodelled instructions become named traps. Shared unchanged. |
| emit     | **`rc-emit-rust`** (new) | One Rust function per guest function (`technomage_exe_00401000(c: &mut Cpu)`), grouped as `code_XXXX.rs` per 64 KiB, a sorted dispatch table and the binary record. Blocks as `loop { match }`. Deterministic. Replaces `rc-emit-c`. |
| verify   | `rc-difftest` (dev only) | Each instruction form run on the real CPU and as lifted code from random states. The sister projects' reference side is freestanding C built with `clang -m32`; here it is Rust for `i686-unknown-linux-gnu` with `core::arch::asm!`. |
| convert  | `rc-convert`, `rc-emit-native`, `rc-emit-wasm`, `rc-web` | Without a compiler: medium → identify → unpack → lift → native code (Cranelift) into a shell template (Windows, Linux), or WebAssembly into the browser's storage. Shared; the templates are the Rust shell. |
| CLI      | `rc-cli` (`recomp`) | `identify`, `info`, `extract`, `download`, `unpack`, `discover`, `codediff`, `lift`, `convert`, `template`. |

## Runtime model (runtime/, crate `rc-runtime`)

A port of the sister projects' runtime, module for module, with the same guest-visible behaviour and the same
`Cpu` layout, so the Cranelift and WebAssembly back ends work unchanged.

- **Start-up**: the shell finds the install (`--game-dir`, `$RC_GAME_DIR`), hashes the executable, restores and
  checks the image, maps the lifted DLL (`binkw32.dll`), then the executable, binds imports to the DLL's exports
  or to HLE, sets up stack, TIB and PEB, runs `DllMain` and calls the entry point.
- **Guest memory**: a 4 GiB reservation on 64-bit hosts (commit on demand), a 768 MiB region of linear memory in
  the browser (`GUEST_TOP`: heaps, stacks and information blocks stay below it). Guest address `a` is host `base + a`. The image's base `0x400000` is used as is
  (the image could be relocated, but nothing needs it). Accessors are `#[inline]` unaligned little-endian loads and
  stores; the `unsafe` is confined to the `mem` module.
- **CPU state**: `#[repr(C)] struct Cpu` (eight GPRs, one byte per flag plus `df`, x87 fast mode, `fs` base,
  `mem`, `helpers`), the sister projects' `rc_cpu`. Lifted functions keep registers and flags in locals and write
  them back at calls, returns and indirect transfers. `cpuid` reports a P6-class CPU without SSE.
- **Calls**: direct calls are Rust calls; indirect ones go through `dispatch(cpu, target)`: patches, then the sorted
  tables of the lifted binaries, then the reserved import addresses. An unknown target stops with its address.
- **HLE**: every import has the signature of lifted code (`fn(&mut Cpu)`), reads its arguments from the guest
  stack and pops them. Each import gets a reserved guest address. `LoadLibraryA`/`GetProcAddress` answer for HLE
  modules too: the game loads `ddraw.dll` and `dinput.dll` that way.
- **COM**: DirectX interfaces are objects in guest memory whose vtables hold reserved addresses named after the
  methods (`IDirect3DDevice7::DrawIndexedPrimitive`), so the game's own vtable calls work unchanged.
- **SEH**: the MSVC 6 static CRT's `_except_handler3` frames, dispatched and unwound over the guest stack.
- **Threads**: guest threads are cooperative, exactly one runs at a time. Native: one OS thread per guest thread,
  handing a baton (`Mutex` + `Condvar`), the sister projects' pthread backend. Browser: each guest thread is a
  JSPI stack with a shadow stack of its own; `web/player.js` switches between them and restores Rust's
  `__stack_pointer` around every wait.
- **Host threading**: natively Bevy owns the main thread and the frame loop; the guest runs on its own thread and
  hands over frames (`rc_platform::submit_frame`) and takes input and audio requests. In the browser there is no
  Bevy: the guest runs on the page's thread as a JSPI coroutine that gives way at every frame (`frame_js`), and
  the page shows frames on a canvas, feeds input and pulls audio (`web/play.js`).
- **Host services** (`host`): files, clocks, settings (`RC_*`), logging and sleeping, from `std` natively and from
  the player's imports (`rc_js`) in the browser; nothing else in the runtime or the HLE layer touches `std::fs`,
  `std::time` or `std::env`.
- **Clock**: real time, or a virtual clock for scripted runs (`RC_CLOCK=virtual`), `timeGetTime`,
  `QueryPerformanceCounter`, `rdtsc`.
- **Files**: a case-insensitive VFS with `\` → `/`. The CD's drive is virtual: `GetLogicalDrives`,
  `GetDriveTypeA` (`DRIVE_CDROM`) and `GetVolumeInformationA` (the disc's label) describe a drive whose root is the
  folder `recomp extract` copied the CD files to. Writes (saves, `TechnoMage.ini`) go to the user directory.

## HLE layer (hle/, crate `rc-hle`)

| Module | Backed by | Used by | Notes |
|--------|-----------|---------|-------|
| `kernel32` | runtime | game, binkw32 | 81 imports by the game: static CRT start-up, heap, files, `FindFirstFile`, semaphores and mutexes, the CD drive queries, `LoadLibraryA`/`GetProcAddress` |
| `user32` | platform window/input | game, binkw32 | one window, message queue from host input, cursor, `SetCursorPos`, `MessageBoxA` |
| `gdi32` | platform | game | only `GetDeviceGammaRamp`/`SetDeviceGammaRamp`: a gamma ramp applied in the final blit shader |
| `ole32` | runtime | game | `CoInitialize`, `CoCreateInstance` → HLE classes (`CLSID_DirectDraw`) |
| `winmm` | runtime clock | game, binkw32 | `timeGetTime` |
| `ddraw` | guest memory | game, binkw32 | DirectDraw 7: display modes, surfaces in guest memory (565, 1555, 4444, 8888), primary/back buffer and flip, colour keys, `Blt`/`BltFast` with stretching and fills, `Lock`; the primary's flips become frames |
| `d3d`, `raster` | platform `gpu` (wgpu, WebGL2) or software | game | Direct3D 7 immediate mode: one HAL device, vertex buffers, `DrawPrimitive`/`DrawIndexedPrimitive` with FVF vertices, render and texture-stage states, transforms, lights, materials, fog, z-buffer. Transform and lighting on the CPU; the triangles of a render target are collected until the CPU needs its pixels (`Flip`, `Lock`, `Blt`, another target) and drawn by the GPU renderer over the target's pixels, which are then written back to guest memory: 2D and 3D mix as on Windows. `raster` is the software rasterizer of the same pipeline (`RC_RENDERER=soft`, and without a GPU) |
| `dinput` | platform input | game | DirectInput 7: keyboard and mouse (loaded with `LoadLibraryA`) |
| `dsound` | platform audio | binkw32 | the buffers Bink's DirectSound output writes |
| `mss32` | platform audio | game | Miles Sound System 6: `AIL_startup`, 2D samples and 3D samples with position, velocity, distances, a listener and room types, providers enumerated as one; a mixer in Rust |

`binkw32.dll` (Bink 1, 2000) is **lifted**, as in HoMM4Recomp: shipped with the game, a narrow API below it, and
faithful. `Mss32.dll` and its `.m3d` providers are **replaced**: the game calls 45 `AIL_*` functions, and those
are less work than lifting Miles, its nine 3D providers and DirectSound 3D beneath them.

## Platform layer (platform/, crate `rc-platform`)

A thin Rust API between the HLE layer and the host; nothing above it names Bevy or wgpu types.

- The **frame sink** (`submit_frame`, `latest_frame`, a present hook), **input** (`Input` to the HLE layer's sink)
  and the **audio mixer** connection (`audio::mix`, 44.1 kHz stereo).
- **`bevy`** (the game natively): one window, the frame as a sprite scaled with its aspect kept, keyboard, mouse
  (relative motion for DirectInput, the pointer mapped through the letterboxing), the mixer as one endless
  `bevy_audio` source. The guest runs on its own thread.
- **`gpu`**: Direct3D's pixel pipeline offscreen. Natively a wgpu device of its own with one WGSL shader (two
  texture stages, colour keys, alpha test, blending, depth, vertex and table fog) and pipelines per blend and
  depth setting; in the browser the same batches go to `web/gl.js` (WebGL2, a GLSL port of the shader). Both read
  the result back synchronously.
- Without a window (`--headless`, `RC_HEADLESS`) frames are kept for screenshots (`RC_SCREENSHOT`); scripted input
  (`RC_INPUT`) and the virtual clock come from the HLE layer and the runtime.

## Games (games/technomage/)

| File | Purpose |
|------|---------|
| `game.toml` | Manifest: variants and media (URL, size, SHA-256), binaries with role, treatment, packing and hashes; per binary a `[binary.discover]` table with seeds and hints; patches. |
| `Cargo.toml`, `src/main.rs` | The shell: arguments, locating the install, the hash check, boot. Includes the lifted code when `generated/` holds it (a `build.rs` finds it; without it the shell still builds). |
| `src/patches.rs` *(none yet)* | Hooks at guest addresses (for example the CD check, if the virtual drive is not enough). Every patch is opt-in or documented. |
| `lifted/` | The 16 shard crates the lifted Rust is built in (`shard-build.rs` includes their part of `generated/`). |
| `../technomage-web/` | The player's runtime module for the browser: the same start-up as `main.rs` for `/game` and `/user`, the exports `web/player.js` calls. |

## Generated code (generated/, git-ignored)

`generated/technomage/<binary>-<variant>/`: `codemap.json` from discovery, `code_XXXX.rs`, `mod.rs`. It is derived
from copyrighted code, so it is **never committed**, never uploaded and never put into issues. Builds regenerate
it locally from the user's own files.

## Repository layout

```
.github/workflows/   CI: audit, fmt/clippy/test, builds for Linux, Windows (macOS built, not offered), web
Cargo.toml           the workspace
docs/                PLAN, ARCHITECTURE, DEVELOPMENT, TARGETS, RESEARCH
games/technomage/    game.toml, the shell crate, the lifted-code shards
games/technomage-web/ the browser player's runtime module
hle/                 Win32/DirectX/Miles reimplementations (crate rc-hle, one module per DLL)
platform/            the Bevy app, the GPU renderer, the frame sink (crate rc-platform)
recomp/crates/       the recompiler and the `recomp` CLI
runtime/             guest memory, CPU, dispatch, loader, SEH, threads, clock, files (crate rc-runtime)
tools/               audit.sh, git hooks, build-web, play-node.mjs and web-test/ (the player headless)
web/                 the converter page (index, app, worker) and the player (play, player, gl)
orig/                YOUR game files (git-ignored)
generated/           lifted Rust (git-ignored), built in games/technomage/lifted/shard*
```
