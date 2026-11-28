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

#[cfg(all(feature = "bevy", target_os = "android"))]
pub mod android;
#[cfg(feature = "bevy")]
pub mod bevy_app;
#[cfg(feature = "gpu")]
pub mod gpu;
pub mod overlay;

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
    /// The touch screen's on-screen buttons held (bit `k`: button `k`, [`touch`]).
    TouchButtons { held: u32 },
    /// The gamepads, as one PlayStation pad: the buttons held ([`pad`]'s bits) and the
    /// direction (the left stick or the directional buttons, each -32768 to 32767, y down).
    Pad { buttons: u16, x: i16, y: i16 },
}

/// The touch screen's on-screen buttons (a game patch's, drawn by it): where they are (the
/// game's screen, margins included) and whether touch is what the player uses.
pub mod touch {
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::sync::Mutex;

    static BUTTONS: Mutex<Vec<[f32; 4]>> = Mutex::new(Vec::new());
    static IN_USE: AtomicBool = AtomicBool::new(false);
    static HELD: AtomicU32 = AtomicU32::new(0);

    /// The buttons shown now (x, y, width, height); none when none are.
    pub fn set_buttons(b: Vec<[f32; 4]>) {
        *BUTTONS.lock().expect("touch buttons") = b;
    }

    /// The button at a point, if any.
    pub fn hit(x: f32, y: f32) -> Option<u8> {
        BUTTONS
            .lock()
            .expect("touch buttons")
            .iter()
            .position(|[bx, by, w, h]| (*bx..bx + w).contains(&x) && (*by..by + h).contains(&y))
            .map(|i| i as u8)
    }

    /// Touch used last (a finger) rather than the keyboard, the mouse or a gamepad.
    pub fn set_in_use(on: bool) {
        IN_USE.store(on, Ordering::Relaxed);
    }

    /// Whether touch is used (`RC_TOUCH=1` says so from the start, for scripted runs).
    pub fn in_use() -> bool {
        static FORCED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if *FORCED.get_or_init(|| std::env::var_os("RC_TOUCH").is_some()) {
            IN_USE.store(true, Ordering::Relaxed);
        }
        IN_USE.load(Ordering::Relaxed)
    }

    /// The buttons held (bit `k`: button `k`), as the backend last said.
    pub fn set_held(bits: u32) {
        HELD.store(bits, Ordering::Relaxed);
        crate::input(crate::Input::TouchButtons { held: bits });
    }

    pub fn held() -> u32 {
        HELD.load(Ordering::Relaxed)
    }
}

/// The gamepads' vibration, as the game asks for it (its force feedback): a strength from 0 to 1
/// that the backends follow (Bevy's gamepads, Android's through JNI, the browser's). It lapses
/// a quarter of a second after the game last set it, so a game that stops asking stops it.
pub mod rumble {
    use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

    static STRENGTH: AtomicU32 = AtomicU32::new(0);
    static SET_AT: AtomicU64 = AtomicU64::new(0);
    const LAPSE_MS: u64 = 250;

    #[cfg(not(target_family = "wasm"))]
    fn now_ms() -> u64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64)
    }

    /// The page's clock (`std` has none in a browser).
    #[cfg(target_family = "wasm")]
    fn now_ms() -> u64 {
        #[link(wasm_import_module = "rc_js")]
        unsafe extern "C" {
            fn now_ms() -> f64;
        }
        // SAFETY: a plain call.
        unsafe { now_ms() as u64 }
    }

    /// The game's force (0 to 1).
    pub fn set(strength: f32) {
        STRENGTH.store(strength.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
        SET_AT.store(now_ms(), Ordering::Relaxed);
    }

    /// The strength to vibrate with now.
    pub fn strength() -> f32 {
        if now_ms().saturating_sub(SET_AT.load(Ordering::Relaxed)) > LAPSE_MS {
            return 0.0;
        }
        f32::from_bits(STRENGTH.load(Ordering::Relaxed))
    }
}

/// A gamepad's buttons as the PlayStation pad has them, in the order the game's DirectInput
/// joystick reports them (bit `n` is the joystick's button `n`; the game's own default table
/// numbers the first eight so).
pub mod pad {
    pub const SQUARE: u16 = 1 << 0;
    pub const TRIANGLE: u16 = 1 << 1;
    pub const CROSS: u16 = 1 << 2;
    pub const CIRCLE: u16 = 1 << 3;
    pub const L2: u16 = 1 << 4;
    pub const L1: u16 = 1 << 5;
    pub const R2: u16 = 1 << 6;
    pub const R1: u16 = 1 << 7;
    pub const SELECT: u16 = 1 << 8;
    pub const START: u16 = 1 << 9;
    pub const L3: u16 = 1 << 10;
    pub const R3: u16 = 1 << 11;
    /// How many buttons the joystick has.
    pub const BUTTONS: u32 = 12;

    /// Whose buttons the gamepad has (what its face buttons are labelled).
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub enum Style {
        PlayStation,
        Xbox,
        Nintendo,
    }

    static STYLE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(1);

    /// The gamepad's style, by its maker's USB vendor id (Sony, Microsoft, Nintendo); others
    /// are taken for Xbox-like (most are).
    pub fn style_of_vendor(vendor: u16) -> Style {
        match vendor {
            0x054c => Style::PlayStation,
            0x057e => Style::Nintendo,
            _ => Style::Xbox,
        }
    }

    pub fn set_style(s: Style) {
        STYLE.store(s as u8, std::sync::atomic::Ordering::Relaxed);
    }

    /// The style of the gamepad used last (Xbox until one says otherwise; `RC_PAD_STYLE`
    /// `playstation`, `xbox` or `nintendo` for scripted runs).
    pub fn style() -> Style {
        static FORCED: std::sync::OnceLock<Option<Style>> = std::sync::OnceLock::new();
        let forced = FORCED.get_or_init(|| match std::env::var("RC_PAD_STYLE").ok().as_deref() {
            Some("playstation") => Some(Style::PlayStation),
            Some("xbox") => Some(Style::Xbox),
            Some("nintendo") => Some(Style::Nintendo),
            _ => None,
        });
        if let Some(s) = forced {
            return *s;
        }
        match STYLE.load(std::sync::atomic::Ordering::Relaxed) {
            0 => Style::PlayStation,
            2 => Style::Nintendo,
            _ => Style::Xbox,
        }
    }
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
    use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

    static NATIVE: AtomicBool = AtomicBool::new(false);
    static OUTPUT: AtomicU64 = AtomicU64::new(0);

    pub fn set_native(on: bool) {
        NATIVE.store(on, Ordering::Relaxed);
    }

    pub fn native() -> bool {
        NATIVE.load(Ordering::Relaxed)
    }

    static WIDESCREEN: AtomicBool = AtomicBool::new(false);

    /// Widescreen: the screen is as wide as the output's proportions ask, the game's picture in
    /// its middle (the HLE's `ddraw::margin`); a game patched for it draws its world into the
    /// sides. Never wider than 32:9, never narrower than the game's screen.
    pub fn set_widescreen(on: bool) {
        WIDESCREEN.store(on, Ordering::Relaxed);
    }

    pub fn widescreen() -> bool {
        WIDESCREEN.load(Ordering::Relaxed)
    }

    /// The margin on each side of a `width` × `height` screen with widescreen on (0 off, or
    /// without an output): even, so the picture stays on whole pixels when it is halved.
    pub fn margin_for(width: u32, height: u32) -> u32 {
        let (ow, oh) = output();
        if !widescreen() || width == 0 || height == 0 || ow == 0 || oh == 0 {
            return 0;
        }
        let wide = (u64::from(height) * u64::from(ow) / u64::from(oh))
            .min(u64::from(height) * 32 / 9) as u32;
        (wide.saturating_sub(width) / 2) & !1
    }

    static ANTIALIAS: AtomicBool = AtomicBool::new(false);
    static SHOW_FPS: AtomicBool = AtomicBool::new(false);
    static SMOOTH_TEXTURES: AtomicBool = AtomicBool::new(false);

    /// Textures enlarged and smoothed before the GPU draws them (an edge-directed scaler).
    pub fn set_smooth_textures(on: bool) {
        SMOOTH_TEXTURES.store(on, Ordering::Relaxed);
    }

    pub fn smooth_textures() -> bool {
        SMOOTH_TEXTURES.load(Ordering::Relaxed)
    }

    static SMOOTH_FONTS: AtomicBool = AtomicBool::new(false);

    /// The game's bitmap fonts redrawn from outline fonts, sharp at any size.
    pub fn set_smooth_fonts(on: bool) {
        SMOOTH_FONTS.store(on, Ordering::Relaxed);
    }

    pub fn smooth_fonts() -> bool {
        SMOOTH_FONTS.load(Ordering::Relaxed)
    }

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

    /// The GPU's largest texture side; until a renderer tells it, the least every GPU offers.
    static MAX_TEXTURE: AtomicU32 = AtomicU32::new(2048);

    pub fn set_max_texture(side: u32) {
        MAX_TEXTURE.store(side.max(1), Ordering::Relaxed);
    }

    pub fn max_texture() -> u32 {
        MAX_TEXTURE.load(Ordering::Relaxed)
    }

    /// The size to render a `width` × `height` screen at: itself, or with native resolution the
    /// largest multiple of it that fits the output (and the GPU's textures), proportions kept.
    pub fn size_for(width: u32, height: u32) -> (u32, u32) {
        let (ow, oh) = output();
        if !native() || width == 0 || height == 0 || ow == 0 || oh == 0 {
            return (width, height);
        }
        let max = max_texture() as f32;
        let s = (ow as f32 / width as f32)
            .min(oh as f32 / height as f32)
            .min(max / width as f32)
            .min(max / height as f32);
        if s <= 1.0 {
            return (width, height);
        }
        (
            ((width as f32 * s).round() as u32).min(max as u32),
            ((height as f32 * s).round() as u32).min(max as u32),
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
        draw_text_px(rgba, width, height, text, (height / 240).max(1) as usize);
    }

    /// [`draw_text`] with glyph pixels `px` large.
    fn draw_text_px(rgba: &mut [u8], width: u32, height: u32, text: &str, px: usize) {
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

    /// The frame rate as a picture over a `width` × `height` frame (the GPU's), if shown.
    pub fn overlay_image(width: u32, height: u32) -> Option<crate::overlay::OverlayImage> {
        if !crate::render::show_fps() {
            return None;
        }
        let text = format!("{:.0} FPS", get());
        let px = (height / 240).max(1) as usize;
        let (bw, bh) = ((4 * text.len() + 3) * px, 9 * px);
        let x = width as i32 - bw as i32;
        let key = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            ("fps", &text, px).hash(&mut h);
            h.finish()
        };
        Some(crate::overlay::OverlayImage::make(
            key,
            (x, 0),
            (bw as u32, bh as u32),
            None,
            |rgba, w, h| draw_text_px(rgba, w, h, &text, px),
        ))
    }
}

/// A finished frame: RGBA8, `width * height * 4` bytes; or on the GPU (`gpu`, `rgba` empty).
#[derive(Debug, Clone, Default)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    #[cfg(feature = "gpu")]
    pub gpu: Option<gpu::GpuPicture>,
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
    #[cfg(feature = "gpu")]
    gpu: None,
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
    #[cfg(feature = "gpu")]
    {
        f.gpu = None;
    }
    f.number += 1;
    let n = f.number;
    if let Some(p) = PRESENT.get() {
        p(&f);
    }
    drop(f);
    #[cfg(not(target_family = "wasm"))]
    NEW_FRAME.notify_all();
    screenshot_if_asked(n);
    hash_if_asked(n);
}

/// `RC_FRAME_HASHES=file`: each frame's size and a hash of its pixels, a line a frame (to compare
/// whole scripted runs before and after a change).
fn hash_if_asked(n: u64) {
    use std::io::Write;
    static OUT: Mutex<Option<std::fs::File>> = Mutex::new(None);
    let Ok(path) = std::env::var("RC_FRAME_HASHES") else {
        return;
    };
    let f = latest_frame();
    let rgba = pixels(&f);
    // FNV-1a over the pixels, 8 bytes at a time.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for c in rgba.as_chunks::<8>().0 {
        h ^= u64::from_le_bytes(*c);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    let mut out = OUT.lock().expect("hashes");
    if out.is_none() {
        *out = std::fs::File::create(&path).ok();
    }
    if let Some(o) = out.as_mut() {
        let _ = writeln!(o, "{n} {}x{} {h:016x}", f.width, f.height);
    }
}

/// Hand over a finished frame drawn on the GPU (the renderer's, on the window's device; in a
/// browser on the player's canvas): shown without its pixels leaving the GPU (read back only
/// for a screenshot).
#[cfg(feature = "gpu")]
pub fn submit_gpu_frame(picture: gpu::GpuPicture) {
    let mut f = FRAME.lock().expect("frame");
    f.width = picture.width;
    f.height = picture.height;
    f.game = picture.game;
    f.rgba = Vec::new();
    f.gpu = Some(picture);
    f.number += 1;
    let n = f.number;
    if let Some(p) = PRESENT.get() {
        p(&f);
    }
    drop(f);
    #[cfg(not(target_family = "wasm"))]
    NEW_FRAME.notify_all();
    screenshot_if_asked(n);
    hash_if_asked(n);
}

/// The pixels of a frame (RGBA8): read back from the GPU if it is there.
fn pixels(f: &Frame) -> std::borrow::Cow<'_, [u8]> {
    #[cfg(all(feature = "gpu", not(target_family = "wasm")))]
    if let Some(p) = &f.gpu {
        if let Some(s) = gpu::device_of_frames() {
            return std::borrow::Cow::Owned(gpu::read(&s.0, &s.1, &p.texture));
        }
    }
    #[cfg(all(feature = "gpu", target_family = "wasm"))]
    if let Some(p) = &f.gpu {
        return std::borrow::Cow::Owned(gpu::read_present(p.width, p.height));
    }
    std::borrow::Cow::Borrowed(&f.rgba)
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
    let rgba = pixels(&f);
    let mut out = format!("P6\n{} {}\n255\n", f.width, f.height).into_bytes();
    for px in rgba.as_chunks::<4>().0 {
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
    fn widescreen_margins() {
        render::set_output(1920, 1080);
        assert_eq!(render::margin_for(640, 480), 0, "off");
        render::set_widescreen(true);
        assert_eq!(render::margin_for(640, 480), 106); // 853 wide
        render::set_output(1280, 1024);
        assert_eq!(render::margin_for(640, 480), 0, "narrower than 4:3");
        render::set_output(5120, 1080);
        assert_eq!(render::margin_for(640, 480), 532, "at most 32:9");
        render::set_widescreen(false);
        render::set_output(0, 0);
    }

    #[test]
    fn frames_are_counted() {
        submit_frame(2, 1, vec![0; 8]);
        let f = latest_frame();
        assert_eq!((f.width, f.height), (2, 1));
        assert!(f.number >= 1);
    }
}
