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
//! Done: `kernel32`, `user32`, `gdi32`, `ole32`, `winmm` (`small`), `mss32` (state, no mixer),
//! `ddraw` (software surfaces, frames to the platform). `dinput` (keyboard and mouse state). `dsound` (streaming buffers for Bink), `d3d` (state, recorded draws, the software rasterizer). The others follow in M4–M6
//! (docs/PLAN.md).

pub mod codepage;
pub mod com;
pub mod d3d;
pub mod ddraw;
pub mod dinput;
pub mod dsound;
pub mod kernel32;
pub mod mixer;
pub mod mss32;
pub mod raster;
pub mod small;
pub mod user32;

pub use rc_platform as platform;
pub use rc_runtime as runtime;

use rc_runtime::cpu::Cpu;

/// Host input into DirectInput's state and the window's messages.
fn on_input(i: rc_platform::Input) {
    use rc_platform::Input;
    const HWND: u32 = 0x10010;
    match i {
        Input::Key { scan, vk, down } => {
            dinput::key(scan, down);
            let msg = if down {
                user32::WM_KEYDOWN
            } else {
                user32::WM_KEYUP
            };
            let lp = (u32::from(scan) << 16) | 1 | if down { 0 } else { 0xc000_0000 };
            user32::post_input(user32::Msg {
                hwnd: HWND,
                msg,
                wparam: vk,
                lparam: lp,
            });
        }
        Input::MouseMotion { dx, dy } => dinput::mouse_motion(dx, dy, 0),
        Input::Wheel { dz } => dinput::mouse_motion(0, 0, dz),
        Input::MouseButton { button, down } => {
            dinput::mouse_button(usize::from(button), down);
            let msg = match (button, down) {
                (0, true) => user32::WM_LBUTTONDOWN,
                (0, false) => user32::WM_LBUTTONUP,
                (1, true) => user32::WM_RBUTTONDOWN,
                (1, false) => user32::WM_RBUTTONUP,
                _ => return,
            };
            let (x, y) = user32::cursor();
            let lp = ((y as u32) << 16) | (x as u32 & 0xffff);
            user32::post_input(user32::Msg {
                hwnd: HWND,
                msg,
                wparam: u32::from(down),
                lparam: lp,
            });
        }
        Input::Cursor { x, y } => {
            let lp = ((y as u32) << 16) | (x as u32 & 0xffff);
            user32::post_input(user32::Msg {
                hwnd: HWND,
                msg: user32::WM_MOUSEMOVE,
                wparam: 0,
                lparam: lp,
            });
        }
    }
}

/// Register every HLE module with the runtime's import table.
pub fn register_all() {
    rc_platform::set_input_sink(on_input);
    rc_platform::audio::set_mixer(mixer::mix);
    kernel32::register();
    user32::register();
    small::register();
    mss32::register();
    ddraw::register();
    dinput::register();
    dsound::register();
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

/// After each frame the game shows (`Flip`): the virtual clock moves on a frame, a scripted run
/// ends after `RC_FRAMES` frames, and the other guest threads get their turn.
/// Time spent in the software rasterizer, in nanoseconds (`RC_TRACE=fps`).
pub(crate) static RASTER_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn fps_trace(n: u64) {
    use std::sync::atomic::Ordering;
    use std::sync::Mutex;
    static LAST: Mutex<Option<std::time::Instant>> = Mutex::new(None);
    if !n.is_multiple_of(100) || !std::env::var("RC_TRACE").is_ok_and(|v| v.contains("fps")) {
        return;
    }
    let mut last = LAST.lock().expect("fps");
    if let Some(t) = *last {
        let secs = t.elapsed().as_secs_f64();
        let raster = RASTER_NS.swap(0, Ordering::Relaxed) as f64 / 1e9;
        eprintln!(
            "[fps] frame {n}: {:.1} frames/s, rasterizer {:.0} %",
            100.0 / secs,
            100.0 * raster / secs
        );
    }
    *last = Some(std::time::Instant::now());
}

pub(crate) fn frame_done(c: &mut Cpu) {
    if rc_runtime::clock::is_virtual() {
        rc_runtime::clock::advance_ms(16);
    }
    let n = rc_platform::latest_frame().number;
    fps_trace(n);
    scripted_input(n);
    if let Some(limit) = std::env::var("RC_FRAMES")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
    {
        if n >= limit {
            eprintln!("[hle] {n} frames shown: the scripted run ends");
            std::process::exit(0);
        }
    }
    yield_point(c);
}

/// `RC_INPUT="150:esc 400:enter 450:click@320,240"`: at the given frames, a key is pressed for
/// one frame (DirectInput and window messages) or the left button clicked at a position.
fn scripted_input(frame: u64) {
    use std::sync::Mutex;
    static RELEASE: Mutex<Vec<(u64, u8, u32)>> = Mutex::new(Vec::new());
    let Ok(script) = std::env::var("RC_INPUT") else {
        return;
    };
    let mut rel = RELEASE.lock().expect("input");
    rel.retain(|&(f, scan, vk)| {
        if f <= frame {
            if scan == 0 {
                dinput::mouse_button(0, false);
                user32::post_input(user32::Msg {
                    hwnd: 0x10010,
                    msg: user32::WM_LBUTTONUP,
                    wparam: 0,
                    lparam: vk,
                });
            } else {
                dinput::key(scan, false);
                user32::post_input(user32::Msg {
                    hwnd: 0x10010,
                    msg: user32::WM_KEYUP,
                    wparam: vk,
                    lparam: u32::from(scan) << 16,
                });
            }
            false
        } else {
            true
        }
    });
    for item in script.split_whitespace() {
        let Some((f, what)) = item.split_once(':') else {
            continue;
        };
        if f.parse::<u64>().ok() != Some(frame) {
            continue;
        }
        if let Some(pos) = what.strip_prefix("click@") {
            let (x, y) = pos.split_once(',').unwrap_or(("0", "0"));
            let (x, y): (u32, u32) = (x.parse().unwrap_or(0), y.parse().unwrap_or(0));
            let lp = (y << 16) | (x & 0xffff);
            user32::post_input(user32::Msg {
                hwnd: 0x10010,
                msg: user32::WM_MOUSEMOVE,
                wparam: 0,
                lparam: lp,
            });
            user32::post_input(user32::Msg {
                hwnd: 0x10010,
                msg: user32::WM_LBUTTONDOWN,
                wparam: 1,
                lparam: lp,
            });
            dinput::mouse_button(0, true);
            rel.push((frame + 2, 0, lp));
            continue;
        }
        let (scan, vk) = match what {
            "esc" => (0x01, 0x1b),
            "enter" => (0x1c, 0x0d),
            "space" => (0x39, 0x20),
            "ctrl" => (0x1d, 0x11),
            "z" => (0x2c, 0x5a),
            "x" => (0x2d, 0x58),
            "up" => (0xc8, 0x26),
            "down" => (0xd0, 0x28),
            "left" => (0xcb, 0x25),
            "right" => (0xcd, 0x27),
            _ => continue,
        };
        dinput::key(scan, true);
        user32::post_input(user32::Msg {
            hwnd: 0x10010,
            msg: user32::WM_KEYDOWN,
            wparam: vk,
            lparam: u32::from(scan) << 16 | 1,
        });
        rel.push((frame + 2, scan, vk));
    }
}

/// Where a guest thread may give way to another (`Sleep`, waits, the message loop).
pub(crate) fn yield_point(c: &mut Cpu) {
    let _ = c;
    rc_runtime::threads::yield_now();
}
