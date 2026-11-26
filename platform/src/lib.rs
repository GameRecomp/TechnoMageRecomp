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

#[cfg(not(target_family = "wasm"))]
use std::sync::Condvar;
use std::sync::{Mutex, OnceLock};

#[cfg(feature = "bevy")]
pub mod bevy_app;
#[cfg(feature = "gpu")]
pub mod gpu;

/// Input from the host, in the terms the HLE layer needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Input {
    /// A key: DirectInput scan code and Windows virtual key.
    Key { scan: u8, vk: u32, down: bool },
    /// Relative mouse motion.
    MouseMotion { dx: i32, dy: i32 },
    /// Mouse wheel (120 per notch).
    Wheel { dz: i32 },
    /// A mouse button: 0 left, 1 right, 2 middle.
    MouseButton { button: u8, down: bool },
    /// The pointer's position in the game's frame.
    Cursor { x: i32, y: i32 },
}

static INPUT_SINK: OnceLock<fn(Input)> = OnceLock::new();

/// Where input goes (the HLE layer registers it).
pub fn set_input_sink(f: fn(Input)) {
    let _ = INPUT_SINK.set(f);
}

/// Hand input to the HLE layer.
pub fn input(i: Input) {
    if let Some(f) = INPUT_SINK.get() {
        f(i);
    }
}

/// The audio mixer's connection: the HLE layer registers a function that fills interleaved
/// stereo samples at [`audio::RATE`]; the backend calls it from its audio thread.
pub mod audio {
    use std::sync::OnceLock;

    pub const RATE: u32 = 44_100;

    static MIXER: OnceLock<fn(&mut [f32])> = OnceLock::new();

    pub fn set_mixer(f: fn(&mut [f32])) {
        let _ = MIXER.set(f);
    }

    /// Fill `out` (interleaved stereo); silence without a mixer.
    pub fn mix(out: &mut [f32]) {
        match MIXER.get() {
            Some(f) => f(out),
            None => out.fill(0.0),
        }
    }
}

/// Which host backend a program runs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Backend {
    /// The Bevy app.
    #[default]
    Bevy,
    /// Headless: frames are kept for screenshots, audio is discarded.
    Null,
}

/// The game's mouse pointer as a hardware cursor: the project's patches render the pointer the
/// game draws into an image in each frame it draws one ([`cursor::show`]); a backend with a window
/// shows it in place of the system's, scaled as the frame is ([`cursor::current`]).
pub mod cursor {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    /// A cursor image: RGBA8 with straight alpha, `width * height * 4` bytes, and the point the
    /// pointer points with.
    #[derive(Debug, Clone, PartialEq, Eq, Hash)]
    pub struct Image {
        pub width: u32,
        pub height: u32,
        pub hotspot: (u32, u32),
        pub rgba: Vec<u8>,
    }

    struct Shown {
        image: Option<Arc<Image>>,
        /// The frame it was last drawn in.
        frame: u64,
        /// Changes with the image.
        generation: u64,
    }

    static SHOWN: Mutex<Shown> = Mutex::new(Shown {
        image: None,
        frame: 0,
        generation: 0,
    });
    static ENABLED: AtomicBool = AtomicBool::new(false);

    /// Frames a pointer stays shown without being drawn again (the game draws it every frame).
    const KEEP: u64 = 2;

    /// Whether a backend shows hardware cursors (the window); without one the game keeps drawing
    /// its own pointer.
    pub fn enabled() -> bool {
        ENABLED.load(Ordering::Relaxed)
    }

    pub fn set_enabled(on: bool) {
        ENABLED.store(on, Ordering::Relaxed);
    }

    /// The pointer drawn in the frame being made.
    pub fn show(image: Image) {
        let frame = crate::frame_number();
        let mut s = SHOWN.lock().expect("cursor");
        s.frame = frame;
        if s.image.as_deref() != Some(&image) {
            s.image = Some(Arc::new(image));
            s.generation += 1;
        }
    }

    /// The pointer to show (None: the game draws none now) and a number that changes with it.
    pub fn current() -> (u64, Option<Arc<Image>>) {
        current_at(crate::frame_number())
    }

    /// [`current`] at frame `frame` (for a present hook, which runs while the frame is locked).
    pub fn current_at(frame: u64) -> (u64, Option<Arc<Image>>) {
        let s = SHOWN.lock().expect("cursor");
        if frame > s.frame + KEEP {
            return (s.generation, None);
        }
        (s.generation, s.image.clone())
    }
}

/// The graphics page's settings. The render resolution: with "native resolution" on, Direct3D
/// draws at the largest size of the game's proportions that fits the window (the screen in full
/// screen); the backend tells its size. Antialiasing, and the frame rate shown.
pub mod render {
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    static NATIVE: AtomicBool = AtomicBool::new(false);
    static OUTPUT: AtomicU64 = AtomicU64::new(0);

    pub fn set_native(on: bool) {
        NATIVE.store(on, Ordering::Relaxed);
    }

    pub fn native() -> bool {
        NATIVE.load(Ordering::Relaxed)
    }

    static ANTIALIAS: AtomicBool = AtomicBool::new(false);
    static SHOW_FPS: AtomicBool = AtomicBool::new(false);

    /// Antialiasing: the GPU renderer draws with several samples per pixel (multisampling).
    pub fn set_antialias(on: bool) {
        ANTIALIAS.store(on, Ordering::Relaxed);
    }

    pub fn antialias() -> bool {
        ANTIALIAS.load(Ordering::Relaxed)
    }

    /// The frame rate shown in a corner of the picture.
    pub fn set_show_fps(on: bool) {
        SHOW_FPS.store(on, Ordering::Relaxed);
    }

    pub fn show_fps() -> bool {
        SHOW_FPS.load(Ordering::Relaxed)
    }

    /// The size the picture is shown at (window pixels; 0 × 0 without a window).
    pub fn set_output(width: u32, height: u32) {
        OUTPUT.store(
            u64::from(width) << 32 | u64::from(height),
            Ordering::Relaxed,
        );
    }

    /// The output size; without a window `RC_RENDER_OUTPUT=WxH` stands in (scripted runs).
    pub fn output() -> (u32, u32) {
        let v = OUTPUT.load(Ordering::Relaxed);
        if v == 0 {
            if let Some((w, h)) = std::env::var("RC_RENDER_OUTPUT")
                .ok()
                .and_then(|s| s.split_once('x').map(|(a, b)| (a.parse(), b.parse())))
                .and_then(|(a, b)| Some((a.ok()?, b.ok()?)))
            {
                return (w, h);
            }
        }
        ((v >> 32) as u32, v as u32)
    }

    /// The size to render a `width` × `height` screen at: itself, or with native resolution the
    /// largest multiple of it that fits the output, proportions kept.
    pub fn size_for(width: u32, height: u32) -> (u32, u32) {
        let (ow, oh) = output();
        if !native() || width == 0 || height == 0 || ow == 0 || oh == 0 {
            return (width, height);
        }
        let s = (ow as f32 / width as f32).min(oh as f32 / height as f32);
        if s <= 1.0 {
            return (width, height);
        }
        (
            (width as f32 * s).round() as u32,
            (height as f32 * s).round() as u32,
        )
    }
}

/// The frame rate shown with `render::show_fps` (the HLE measures it, frames are drawn with it).
pub mod fps {
    use std::sync::atomic::{AtomicU32, Ordering};

    static FPS: AtomicU32 = AtomicU32::new(0);

    /// The latest measurement, in frames per second.
    pub fn set(fps: f32) {
        FPS.store(fps.to_bits(), Ordering::Relaxed);
    }

    pub fn get() -> f32 {
        f32::from_bits(FPS.load(Ordering::Relaxed))
    }

    /// 3 × 5 glyphs, a row per byte, the high of three bits left.
    fn glyph(ch: u8) -> [u8; 5] {
        match ch {
            b'0' => [7, 5, 5, 5, 7],
            b'1' => [2, 6, 2, 2, 7],
            b'2' => [7, 1, 7, 4, 7],
            b'3' => [7, 1, 3, 1, 7],
            b'4' => [5, 5, 7, 1, 1],
            b'5' => [7, 4, 7, 1, 7],
            b'6' => [7, 4, 7, 5, 7],
            b'7' => [7, 1, 2, 2, 2],
            b'8' => [7, 5, 7, 5, 7],
            b'9' => [7, 5, 7, 1, 7],
            b'F' => [7, 4, 6, 4, 4],
            b'P' => [7, 5, 7, 4, 4],
            b'S' => [7, 4, 7, 1, 7],
            _ => [0; 5],
        }
    }

    /// The text into the top right corner of a `width` × `height` RGBA8 picture: white on a
    /// dark box, a glyph pixel as large as the picture is tall in multiples of 240.
    pub fn draw_text(rgba: &mut [u8], width: u32, height: u32, text: &str) {
        let px = (height / 240).max(1) as usize;
        let (w, h) = (width as usize, height as usize);
        if rgba.len() < 4 * w * h {
            return;
        }
        let n = text.len();
        let box_w = (4 * n + 1) * px;
        let box_h = 7 * px;
        let (x0, y0) = (w.saturating_sub(box_w + 2 * px), 2 * px);
        let mut put = |x: usize, y: usize, c: [u8; 3], keep: u32| {
            if x < w && y < h {
                let i = 4 * (y * w + x);
                for k in 0..3 {
                    let v = u32::from(rgba[i + k]) * keep / 256 + u32::from(c[k]);
                    rgba[i + k] = v.min(255) as u8;
                }
            }
        };
        for y in 0..box_h {
            for x in 0..box_w {
                put(x0 + x, y0 + y, [0, 0, 0], 80);
            }
        }
        for (k, ch) in text.bytes().enumerate() {
            let g = glyph(ch);
            for (row, bits) in g.iter().enumerate() {
                for col in 0..3 {
                    if bits >> (2 - col) & 1 == 0 {
                        continue;
                    }
                    for dy in 0..px {
                        for dx in 0..px {
                            let x = x0 + (1 + 4 * k + col) * px + dx;
                            let y = y0 + (1 + row) * px + dy;
                            put(x, y, [255, 255, 255], 0);
                        }
                    }
                }
            }
        }
    }

    /// The frame rate into a frame, if it is to be shown.
    pub(crate) fn overlay(rgba: &mut [u8], width: u32, height: u32) {
        if crate::render::show_fps() {
            draw_text(rgba, width, height, &format!("{:.0} FPS", get()));
        }
    }
}

/// A finished frame: RGBA8, `width * height * 4` bytes.
#[derive(Debug, Clone, Default)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// Frames submitted so far, this one included.
    pub number: u64,
    /// The game's screen size (the frame may be rendered larger: [`render`]); input and the
    /// cursor are in its coordinates.
    pub game: (u32, u32),
}

static FRAME: Mutex<Frame> = Mutex::new(Frame {
    width: 0,
    height: 0,
    rgba: Vec::new(),
    number: 0,
    game: (0, 0),
});
#[cfg(not(target_family = "wasm"))]
static NEW_FRAME: Condvar = Condvar::new();

static PRESENT: OnceLock<fn(&Frame)> = OnceLock::new();

/// A backend that shows frames as they come (the browser player) instead of reading the latest.
pub fn set_present_hook(f: fn(&Frame)) {
    let _ = PRESENT.set(f);
}

/// Hand over a finished frame (the game's `Flip`).
pub fn submit_frame(width: u32, height: u32, rgba: Vec<u8>) {
    submit_frame_scaled((width, height), width, height, rgba);
}

/// Hand over a finished frame of the `game`-sized screen, rendered at `width` × `height`.
pub fn submit_frame_scaled(game: (u32, u32), width: u32, height: u32, mut rgba: Vec<u8>) {
    fps::overlay(&mut rgba, width, height);
    let mut f = FRAME.lock().expect("frame");
    f.width = width;
    f.height = height;
    f.game = game;
    f.rgba = rgba;
    f.number += 1;
    let n = f.number;
    if let Some(p) = PRESENT.get() {
        p(&f);
    }
    drop(f);
    #[cfg(not(target_family = "wasm"))]
    NEW_FRAME.notify_all();
    screenshot_if_asked(n);
}

/// How many frames were submitted.
pub fn frame_number() -> u64 {
    FRAME.lock().expect("frame").number
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
    // The hardware cursor shown then, beside it (RGBA as a PAM file).
    if let (_, Some(c)) = cursor::current() {
        let cpath = format!("{path}.cursor.pam");
        let mut out = format!(
            "P7\nWIDTH {}\nHEIGHT {}\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n",
            c.width, c.height
        )
        .into_bytes();
        out.extend_from_slice(&c.rgba);
        if std::fs::write(&cpath, out).is_ok() {
            eprintln!(
                "cursor {}x{} (hotspot {:?}) in {cpath}",
                c.width, c.height, c.hotspot
            );
        }
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
