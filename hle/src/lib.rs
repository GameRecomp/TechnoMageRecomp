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
//!
//! Done: `kernel32`, `user32`, `gdi32`, `ole32`, `winmm` (`small`), `mss32` (state, no mixer). The others follow in M4–M6
//! (docs/PLAN.md).

pub mod codepage;
pub mod kernel32;
pub mod mss32;
pub mod small;
pub mod user32;

pub use rc_platform as platform;
pub use rc_runtime as runtime;

use rc_runtime::cpu::Cpu;

/// Register every HLE module with the runtime's import table.
pub fn register_all() {
    kernel32::register();
    user32::register();
    small::register();
    mss32::register();
    rc_runtime::dispatch::set_import_resolver(rc_runtime::imports::resolve);
}

/// The base of a lifted DLL the process mapped (`binkw32.dll`).
pub(crate) fn lifted_module_base(name: &str) -> Option<u32> {
    rc_runtime::process::module_base(name)
}

/// An export of a lifted DLL, by its module handle (its base).
pub(crate) fn lifted_export(base: u32, name: &str) -> Option<u32> {
    rc_runtime::process::module_export(base, name)
}

/// Where a guest thread may give way to another (`Sleep`, waits). Guest threads come in M4.
pub(crate) fn yield_point(c: &mut Cpu) {
    let _ = c;
}
