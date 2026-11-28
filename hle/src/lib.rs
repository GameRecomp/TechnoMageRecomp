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
#[cfg(feature = "gpu")]
mod enlarging;
pub mod glyphs;
pub mod kernel32;
pub mod mixer;
pub mod mss32;
pub mod profile;
pub mod raster;
pub mod small;
pub mod upscale;
pub mod user32;

pub use rc_platform as platform;
pub use rc_runtime as runtime;

use rc_runtime::cpu::Cpu;

static CURSOR_MAP: std::sync::OnceLock<fn(i32) -> i32> = std::sync::OnceLock::new();

/// Where a point of the screen (its x, margins included) is in the game's pixels, when a game
/// patch moves parts of the interface on a wider screen (otherwise: one margin to the left).
pub fn set_cursor_map(f: fn(i32) -> i32) {
    let _ = CURSOR_MAP.set(f);
}

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
        Input::Pad { buttons, x, y } => dinput::pad(buttons, x, y),
        Input::TouchButtons { held } => dinput::touch_buttons(held),
        Input::Cursor { x, y } => {
            // Widescreen: the game's screen is the middle of the frame; on a margin the pointer
            // is left or right of it (the message's coordinates are signed). A game patch that
            // moves parts of its interface to the edges says where its pixels are.
            let x = match CURSOR_MAP.get() {
                Some(f) => f(x),
                None => x - ddraw::margin() as i32,
            };
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
    static LAST: Mutex<Option<rc_runtime::host::Stamp>> = Mutex::new(None);
    if !n.is_multiple_of(100)
        || !rc_runtime::host::env("RC_TRACE").is_some_and(|v| v.contains("fps"))
    {
        return;
    }
    let mut last = LAST.lock().expect("fps");
    if let Some(t) = *last {
        let secs = t.elapsed_secs();
        let raster = RASTER_NS.swap(0, Ordering::Relaxed) as f64 / 1e9;
        rc_runtime::log!(
            "[fps] frame {n}: {:.1} frames/s, rasterizer {:.0} %",
            100.0 / secs,
            100.0 * raster / secs
        );
    }
    *last = Some(rc_runtime::host::Stamp::now());
}

/// The frame rate for `rc_platform::fps`: frames shown over about half a second.
fn measure_fps() {
    use std::sync::Mutex;
    static SINCE: Mutex<Option<(rc_runtime::host::Stamp, u32)>> = Mutex::new(None);
    let mut s = SINCE.lock().expect("fps");
    match s.as_mut() {
        Some((t, frames)) => {
            *frames += 1;
            let secs = t.elapsed_secs();
            if secs >= 0.5 {
                rc_platform::fps::set((f64::from(*frames) / secs) as f32);
                *s = Some((rc_runtime::host::Stamp::now(), 0));
            }
        }
        None => *s = Some((rc_runtime::host::Stamp::now(), 0)),
    }
}

pub(crate) fn frame_done(c: &mut Cpu) {
    if rc_runtime::clock::is_virtual() {
        rc_runtime::clock::advance_ms(16);
    }
    let n = rc_platform::frame_number();
    measure_fps();
    fps_trace(n);
    profile::frame_end(n);
    scripted_input(n);
    if let Some(limit) = rc_runtime::host::env("RC_FRAMES").and_then(|v| v.parse::<u64>().ok()) {
        if n >= limit {
            rc_runtime::log!("[hle] {n} frames shown: the scripted run ends");
            rc_runtime::host::exit(0);
        }
    }
    #[cfg(feature = "gpu")]
    enlarging::pump();
    profile::time(profile::Section::Yield, rc_runtime::host::frame_yield);
    yield_point(c);
}

/// `RC_INPUT="150:esc 400:enter 450:click@320,240 500:pad-cross"`: at the given frames, a key
/// is pressed for one frame (DirectInput and window messages), the left button clicked at a
/// position, or a button of the gamepad (`pad-square` … `pad-start`, `pad-l1` …, a direction
/// `pad-left` …) held for four frames, or the touch screen's on-screen button at `tap@x,y`;
/// `fullscreen` switches the window to full screen and back (F11); `point@x,y` moves the pointer
/// to a point of the screen, margins included (`move@x,y` takes the game's coordinates).
fn scripted_input(frame: u64) {
    use std::sync::Mutex;
    static RELEASE: Mutex<Vec<(u64, u8, u32)>> = Mutex::new(Vec::new());
    let Some(script) = rc_runtime::host::env("RC_INPUT") else {
        return;
    };
    static PAD_RELEASE: Mutex<Option<u64>> = Mutex::new(None);
    {
        let mut p = PAD_RELEASE.lock().expect("input");
        if p.is_some_and(|f| f <= frame) {
            *p = None;
            dinput::pad(0, 0, 0);
            rc_platform::touch::set_held(0);
        }
    }
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
        // `a-b:key`: the key held from frame a to frame b.
        if let Some((a, b)) = f.split_once('-') {
            let (a, b) = (a.parse::<u64>().unwrap_or(0), b.parse::<u64>().unwrap_or(0));
            if frame == a || frame == b {
                if let Some((scan, vk)) = scripted_key(what) {
                    let down = frame == a;
                    dinput::key(scan, down);
                    user32::post_input(user32::Msg {
                        hwnd: 0x10010,
                        msg: if down {
                            user32::WM_KEYDOWN
                        } else {
                            user32::WM_KEYUP
                        },
                        wparam: vk,
                        lparam: u32::from(scan) << 16 | if down { 1 } else { 0xc000_0001 },
                    });
                }
            }
            continue;
        }
        if f.parse::<u64>().ok() != Some(frame) {
            continue;
        }
        if let Some(b) = what.strip_prefix("pad-") {
            use rc_platform::pad;
            let (bits, x, y) = match b {
                "square" => (pad::SQUARE, 0, 0),
                "triangle" => (pad::TRIANGLE, 0, 0),
                "cross" => (pad::CROSS, 0, 0),
                "circle" => (pad::CIRCLE, 0, 0),
                "l1" => (pad::L1, 0, 0),
                "l2" => (pad::L2, 0, 0),
                "r1" => (pad::R1, 0, 0),
                "r2" => (pad::R2, 0, 0),
                "select" => (pad::SELECT, 0, 0),
                "start" => (pad::START, 0, 0),
                "left" => (0, -32768, 0),
                "right" => (0, 32767, 0),
                "up" => (0, 0, -32768),
                "down" => (0, 0, 32767),
                _ => continue,
            };
            dinput::pad(bits, x, y);
            *PAD_RELEASE.lock().expect("input") = Some(frame + 4);
            continue;
        }
        if let Some(pos) = what.strip_prefix("point@") {
            // A point of the screen (margins included), as a window or a finger gives it.
            let (x, y) = pos.split_once(',').unwrap_or(("0", "0"));
            let (x, y): (i32, i32) = (x.parse().unwrap_or(0), y.parse().unwrap_or(0));
            // And moved there (DirectInput), as a finger or a mouse does.
            static AT: Mutex<(i32, i32)> = Mutex::new((0, 0));
            let (px, py) = std::mem::replace(&mut *AT.lock().expect("point"), (x, y));
            on_input(rc_platform::Input::MouseMotion {
                dx: x - px,
                dy: y - py,
            });
            on_input(rc_platform::Input::Cursor { x, y });
            continue;
        }
        if what == "fullscreen" {
            rc_platform::render::toggle_fullscreen();
            continue;
        }
        if let Some(pos) = what.strip_prefix("tap@") {
            // A finger on the on-screen button there (the game's screen, margins included).
            let (x, y) = pos.split_once(',').unwrap_or(("0", "0"));
            let (x, y): (f32, f32) = (x.parse().unwrap_or(0.0), y.parse().unwrap_or(0.0));
            if let Some(b) = rc_platform::touch::hit(x, y) {
                rc_platform::touch::set_held(1 << b);
                *PAD_RELEASE.lock().expect("input") = Some(frame + 4);
            }
            continue;
        }
        if let Some(pos) = what.strip_prefix("move@") {
            let (x, y) = pos.split_once(',').unwrap_or(("0", "0"));
            let (x, y): (u32, u32) = (x.parse().unwrap_or(0), y.parse().unwrap_or(0));
            user32::post_input(user32::Msg {
                hwnd: 0x10010,
                msg: user32::WM_MOUSEMOVE,
                wparam: 0,
                lparam: (y << 16) | (x & 0xffff),
            });
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
        let Some((scan, vk)) = scripted_key(what) else {
            continue;
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

/// A scripted key's DirectInput scan code and virtual key.
fn scripted_key(what: &str) -> Option<(u8, u32)> {
    Some(match what {
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
        "m" => (0x32, 0x4d),
        "i" => (0x17, 0x49),
        "tab" => (0x0f, 0x09),
        "f11" => (0x57, 0x7a),
        "f12" => (0x58, 0x7b),
        "alt" => (0x38, 0x12),
        "s" => (0x1f, 0x53),
        "q" => (0x10, 0x51),
        _ => return None,
    })
}

/// Where a guest thread may give way to another (`Sleep`, waits, the message loop).
pub(crate) fn yield_point(c: &mut Cpu) {
    let _ = c;
    profile::time(profile::Section::Yield, rc_runtime::threads::yield_now);
}
