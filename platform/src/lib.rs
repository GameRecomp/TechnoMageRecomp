// SPDX-License-Identifier: MIT
//! The host below the HLE layer, in pure Rust (no SDL):
//!
//! - `bevy`: the game's backend. Window, input, gamepads, audio and the frame loop come from Bevy;
//!   the Direct3D 7 and DirectDraw draws the HLE layer records are replayed in a custom render
//!   node on Bevy's wgpu device (Vulkan, Direct3D 12, Metal, WebGPU, WebGL2)
//! - `null`: no window, no audio, for tests, CI and scripted runs
//!
//! Nothing above this crate names Bevy or wgpu types. What exists so far is the frame sink both
//! backends read: the HLE layer submits each finished frame here.

use std::sync::{Condvar, Mutex};

/// Which host backend a program runs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Backend {
    /// The Bevy app.
    #[default]
    Bevy,
    /// Headless: frames are kept for screenshots, audio is discarded.
    Null,
}

/// A finished frame: RGBA8, `width * height * 4` bytes.
#[derive(Debug, Clone, Default)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// Frames submitted so far, this one included.
    pub number: u64,
}

static FRAME: Mutex<Frame> = Mutex::new(Frame {
    width: 0,
    height: 0,
    rgba: Vec::new(),
    number: 0,
});
static NEW_FRAME: Condvar = Condvar::new();

/// Hand over a finished frame (the game's `Flip`).
pub fn submit_frame(width: u32, height: u32, rgba: Vec<u8>) {
    let mut f = FRAME.lock().expect("frame");
    f.width = width;
    f.height = height;
    f.rgba = rgba;
    f.number += 1;
    let n = f.number;
    drop(f);
    NEW_FRAME.notify_all();
    screenshot_if_asked(n);
}

/// The latest frame.
pub fn latest_frame() -> Frame {
    FRAME.lock().expect("frame").clone()
}

/// `RC_SCREENSHOT=path` with `RC_SCREENSHOT_FRAME=n` (default 1): frame `n` as a PPM file.
fn screenshot_if_asked(n: u64) {
    let Ok(path) = std::env::var("RC_SCREENSHOT") else {
        return;
    };
    let want: u64 = std::env::var("RC_SCREENSHOT_FRAME")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    if n != want {
        return;
    }
    let f = latest_frame();
    let mut out = format!("P6\n{} {}\n255\n", f.width, f.height).into_bytes();
    for px in f.rgba.as_chunks::<4>().0 {
        out.extend_from_slice(&px[..3]);
    }
    if let Err(e) = std::fs::write(&path, out) {
        eprintln!("screenshot {path}: {e}");
    } else {
        eprintln!("screenshot of frame {n} in {path}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_are_counted() {
        submit_frame(2, 1, vec![0; 8]);
        let f = latest_frame();
        assert_eq!((f.width, f.height), (2, 1));
        assert!(f.number >= 1);
    }
}
