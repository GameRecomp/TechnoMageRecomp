// SPDX-License-Identifier: MIT
//! High-level emulation of everything TechnoMage imports, one module per DLL.
//!
//! | Module      | Backed by                         | Imported by |
//! |-------------|-----------------------------------|-------------|
//! | `kernel32`  | runtime (files, heap, threads)    | game, binkw32, mss32 |
//! | `user32`    | platform window and input         | game, binkw32 |
//! | `gdi32`     | platform (gamma ramp only)        | game, binkw32 |
//! | `ole32`     | runtime COM registry              | game |
//! | `winmm`     | runtime clock                     | game, binkw32 |
//! | `ddraw`     | platform (Bevy, wgpu)             | game (`LoadLibrary`), binkw32 |
//! | `d3d`       | platform (Bevy, wgpu): Direct3D 7 | game, through `IDirectDraw7::QueryInterface` |
//! | `dinput`    | platform input                    | game (`LoadLibrary`) |
//! | `dsound`    | platform audio (Bevy)             | binkw32 (`BinkOpenDirectSound`) |
//! | `mss32`     | platform audio: a mixer with 3D   | game (45 `AIL_*` functions) |
//!
//! `binkw32.dll` is lifted, not emulated (as in HoMM4Recomp). Every HLE function has the
//! signature of lifted code, `fn(&mut Cpu)`, reads its arguments from the guest stack and pops
//! them (`stdcall`). See docs/ARCHITECTURE.md.

pub use rc_platform as platform;
pub use rc_runtime as runtime;
