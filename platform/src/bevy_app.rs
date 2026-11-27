// SPDX-License-Identifier: MIT
//! The Bevy backend: a window that shows the game's latest frame, scaled to fit with its aspect
//! kept; keyboard, mouse and window events handed to the input sink; the audio mixer played as
//! one endless Bevy audio source. The guest runs on its own thread; this owns the main thread.

use crate::{audio, input, Input};
use bevy::asset::RenderAssetUsages;
use bevy::audio::{AddAudioSource, AudioPlayer, Decodable};
use bevy::image::ImageSampler;
use bevy::input::gamepad::{GamepadRumbleIntensity, GamepadRumbleRequest};
use bevy::input::keyboard::KeyboardInput;
use bevy::input::mouse::{MouseButtonInput, MouseMotion, MouseWheel};
use bevy::input::touch::{TouchInput, TouchPhase};
use bevy::input::ButtonState;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::render::settings::{Backends, WgpuSettings};
use bevy::render::RenderPlugin;
use bevy::window::{
    CursorIcon, CursorMoved, CursorOptions, CustomCursor, CustomCursorImage, MonitorSelection,
    WindowCloseRequested, WindowMode,
};
use std::collections::HashMap;
use std::num::NonZero;
use std::sync::Arc;

/// The game's frame on screen.
#[derive(Resource)]
struct Screen {
    image: Handle<Image>,
    sprite: Entity,
    shown: u64,
    /// The frame's pixels, and the game's screen it shows (input and cursor coordinates).
    size: (u32, u32),
    game: (u32, u32),
}

/// The mixer as a Bevy audio source.
#[derive(Asset, TypePath, Clone)]
struct GameAudio;

/// Pulls stereo samples from the platform's mixer.
struct GameAudioDecoder {
    buf: Vec<f32>,
    pos: usize,
}

impl Iterator for GameAudioDecoder {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if self.pos >= self.buf.len() {
            self.buf.resize(1024, 0.0);
            audio::mix(&mut self.buf);
            self.pos = 0;
        }
        let v = self.buf[self.pos];
        self.pos += 1;
        Some(v)
    }
}

impl bevy::audio::Source for GameAudioDecoder {
    fn current_span_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> NonZero<u16> {
        NonZero::new(2).expect("two")
    }
    fn sample_rate(&self) -> NonZero<u32> {
        NonZero::new(audio::RATE).expect("rate")
    }
    fn total_duration(&self) -> Option<std::time::Duration> {
        None
    }
}

impl Decodable for GameAudio {
    type Decoder = GameAudioDecoder;
    fn decoder(&self) -> Self::Decoder {
        GameAudioDecoder {
            buf: Vec::new(),
            pos: 0,
        }
    }
}

/// Run the window on this thread and `guest` on another; returns never (the window closing
/// ends the process).
pub fn run(title: &str, guest: impl FnOnce() + Send + 'static) -> ! {
    std::thread::Builder::new()
        .name("guest-main".into())
        .stack_size(64 << 20)
        .spawn(move || {
            guest();
            std::process::exit(0);
        })
        .expect("the guest thread");
    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(WindowPlugin {
                primary_window: Some(Window {
                    title: title.to_string(),
                    resolution: (1280u32, 960u32).into(),
                    // A phone's screen is the window.
                    mode: if cfg!(target_os = "android") {
                        WindowMode::BorderlessFullscreen(MonitorSelection::Current)
                    } else {
                        WindowMode::Windowed
                    },
                    ..default()
                }),
                ..default()
            })
            .set(ImagePlugin::default_nearest())
            .set(RenderPlugin {
                render_creation: WgpuSettings {
                    backends: Some(window_backends()),
                    ..default()
                }
                .into(),
                ..default()
            }),
    )
    .add_audio_source::<GameAudio>()
    .insert_resource(ClearColor(Color::BLACK))
    .init_resource::<HardwareCursor>()
    .init_resource::<Fingers>()
    .add_systems(Startup, setup)
    .add_systems(
        Update,
        (
            show_frame,
            keyboard,
            mouse,
            gamepad,
            vibrate,
            touch,
            close,
            hardware_cursor,
        ),
    );
    // Without a mouse (Android) the game draws its pointer itself, where the finger is.
    crate::cursor::set_enabled(!cfg!(target_os = "android"));
    app.run();
    std::process::exit(0);
}

/// The graphics backends Bevy may draw the window with: `WGPU_BACKEND` if set, else the primary
/// ones (Vulkan, Direct3D 12, Metal) when any of them has an adapter, else all (OpenGL).
///
/// Bevy's default, all backends at high performance, can choose an OpenGL adapter over a
/// software Vulkan one (a virtual machine's SVGA3D driver beside lavapipe). Its OpenGL surface for
/// an X11 window then offers no present modes and Bevy panics creating it ("Fallback system
/// failed to choose present mode", docs/RESEARCH.md).
fn window_backends() -> Backends {
    if let Some(b) = Backends::from_env() {
        return b;
    }
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: Backends::PRIMARY,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    if pollster::block_on(instance.enumerate_adapters(Backends::PRIMARY)).is_empty() {
        Backends::all()
    } else {
        Backends::PRIMARY
    }
}

fn new_image(w: u32, h: u32) -> Image {
    let mut img = Image::new_fill(
        Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        &[0, 0, 0, 255],
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    img.sampler = ImageSampler::nearest();
    img
}

fn setup(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mut sources: ResMut<Assets<GameAudio>>,
) {
    commands.spawn(Camera2d);
    let image = images.add(new_image(640, 480));
    let sprite = commands
        .spawn(Sprite {
            image: image.clone(),
            custom_size: Some(Vec2::new(640.0, 480.0)),
            ..default()
        })
        .id();
    commands.insert_resource(Screen {
        image,
        sprite,
        shown: 0,
        size: (640, 480),
        game: (640, 480),
    });
    let src = sources.add(GameAudio);
    commands.spawn(AudioPlayer(src));
}

fn show_frame(
    mut screen: ResMut<Screen>,
    mut images: ResMut<Assets<Image>>,
    mut sprites: Query<&mut Sprite>,
    windows: Query<&Window>,
) {
    let f = crate::latest_frame();
    if f.number != screen.shown && f.width > 0 {
        screen.shown = f.number;
        screen.game = f.game;
        if (f.width, f.height) != screen.size {
            // A new image for a new size (the sprite does not show one resized in place).
            screen.size = (f.width, f.height);
            screen.image = images.add(new_image(f.width, f.height));
            if let Ok(mut s) = sprites.get_mut(screen.sprite) {
                s.image = screen.image.clone();
            }
        }
        if let Some(mut img) = images.get_mut(&screen.image) {
            img.data = Some(f.rgba);
        }
    }
    let Ok(win) = windows.single() else {
        return;
    };
    crate::render::set_output(win.physical_width(), win.physical_height());
    let (w, h) = (win.width(), win.height());
    let (gw, gh) = (screen.game.0 as f32, screen.game.1 as f32);
    let scale = (w / gw).min(h / gh);
    if let Ok(mut s) = sprites.get_mut(screen.sprite) {
        s.custom_size = Some(Vec2::new(gw * scale, gh * scale));
    }
}

/// The game's pointer as the window's cursor.
#[derive(Resource, Default)]
struct HardwareCursor {
    /// What is shown: the pointer's generation and the scale (None: hidden).
    shown: Option<(u64, u32)>,
    /// The cursor images made so far (the game's pointer and the scale they were made at), by
    /// content: Bevy keeps a system cursor for each image to the end, so the same picture must
    /// reuse its image, and there are about `CURSOR_IMAGES` of them at most.
    images: Vec<CursorEntry>,
    index: HashMap<(crate::cursor::Image, u32), usize>,
}

/// A cursor image made: the game's pointer and the scale it was made at, the image and its
/// hotspot.
struct CursorEntry {
    pointer: crate::cursor::Image,
    scale: u32,
    image: Handle<Image>,
    hotspot: (u16, u16),
}

/// The largest cursor the systems take well.
const CURSOR_MAX: u32 = 256;
/// Cursor images made at most; then the most similar one stands in (the pointer turns with the
/// mouse, sways, and is seen from a slightly different side at each place: in a long game it
/// takes thousands of slightly different forms).
const CURSOR_IMAGES: usize = 1024;
/// A pointer that differs from an image made by at most this (`cursor_difference`) shows that
/// image: the turn's smallest steps (about 0.05 to 0.1 apart) and the sway's (0.01) do not each
/// make an image of their own.
const CURSOR_SAME: f32 = 0.05;
/// A pointer that differs from every image made by more than this (one facing another way:
/// 0.2 to 0.3) gets its own even past `CURSOR_IMAGES`, so that no direction is shown by a
/// pointer that points elsewhere.
const CURSOR_FAR: f32 = 0.12;
/// Pointers remembered with the image they show; then forgotten (they are found again).
const CURSOR_INDEX: usize = 8192;

fn hardware_cursor(
    mut commands: Commands,
    mut windows: Query<(Entity, &Window, &mut CursorOptions)>,
    mut images: ResMut<Assets<Image>>,
    screen: Res<Screen>,
    mut hw: ResMut<HardwareCursor>,
) {
    let Ok((entity, win, mut options)) = windows.single_mut() else {
        return;
    };
    let (generation, pointer) = crate::cursor::current();
    let Some(p) = pointer else {
        if hw.shown.take().is_some() || options.visible {
            options.visible = false;
        }
        return;
    };
    // As big as the game's pointer in the scaled frame (window pixels, nearest neighbour).
    let (gw, gh) = (screen.game.0 as f32, screen.game.1 as f32);
    let fit = (win.width() / gw).min(win.height() / gh) * win.scale_factor();
    let largest = (CURSOR_MAX as f32 / p.width.max(p.height).max(1) as f32).max(1.0);
    let scale = fit.clamp(1.0, largest);
    let key = (generation, scale.to_bits());
    if hw.shown == Some(key) && options.visible {
        return;
    }
    let scale_bits = scale.to_bits();
    let image = (Arc::unwrap_or_clone(p), scale_bits);
    let at = match hw.index.get(&image).copied() {
        Some(i) => i,
        None => {
            // An image made at this scale that looks the same, else a new one; when there are
            // as many as can be, the most similar one unless it looks quite different.
            let near = nearest_cursor(&hw.images, &image.0, scale_bits);
            let at = match near {
                Some((i, d))
                    if d <= CURSOR_SAME
                        || (hw.images.len() >= CURSOR_IMAGES && d <= CURSOR_FAR) =>
                {
                    i
                }
                _ => {
                    let h = images.add(cursor_image(&image.0, scale));
                    let hotspot = scale_cursor(&image.0, scale).hotspot;
                    hw.images.push(CursorEntry {
                        pointer: image.0.clone(),
                        scale: scale_bits,
                        image: h,
                        hotspot: (hotspot.0 as u16, hotspot.1 as u16),
                    });
                    if std::env::var("RC_TRACE").is_ok_and(|v| v.contains("cursor")) {
                        eprintln!(
                            "[cursor] {} cursor images (the nearest differed by {:.4})",
                            hw.images.len(),
                            near.map_or(1.0, |n| n.1)
                        );
                    }
                    hw.images.len() - 1
                }
            };
            if hw.index.len() >= CURSOR_INDEX {
                hw.index.clear();
            }
            hw.index.insert(image, at);
            at
        }
    };
    let (handle, hotspot) = (hw.images[at].image.clone(), hw.images[at].hotspot);
    commands
        .entity(entity)
        .insert(CursorIcon::Custom(CustomCursor::Image(CustomCursorImage {
            handle,
            texture_atlas: None,
            flip_x: false,
            flip_y: false,
            rect: None,
            hotspot,
        })));
    options.visible = true;
    hw.shown = Some(key);
}

/// `c` as a cursor image at `scale`.
fn cursor_image(c: &crate::cursor::Image, scale: f32) -> Image {
    let scaled = scale_cursor(c, scale);
    Image::new(
        Extent3d {
            width: scaled.width,
            height: scaled.height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        scaled.rgba,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::MAIN_WORLD,
    )
}

/// How different two pointers look, aligned at their hotspots: the mean difference of their
/// pixels' channels over both their areas, 0 (the same) to 1.
fn cursor_difference(a: &crate::cursor::Image, b: &crate::cursor::Image) -> f32 {
    let px = |i: &crate::cursor::Image, x: i32, y: i32| -> [u8; 4] {
        let (x, y) = (x + i.hotspot.0 as i32, y + i.hotspot.1 as i32);
        if x < 0 || y < 0 || x >= i.width as i32 || y >= i.height as i32 {
            return [0; 4];
        }
        let k = 4 * (y as u32 * i.width + x as u32) as usize;
        [i.rgba[k], i.rgba[k + 1], i.rgba[k + 2], i.rgba[k + 3]]
    };
    let x0 = -(a.hotspot.0.max(b.hotspot.0) as i32);
    let y0 = -(a.hotspot.1.max(b.hotspot.1) as i32);
    let x1 = (a.width - a.hotspot.0).max(b.width - b.hotspot.0) as i32;
    let y1 = (a.height - a.hotspot.1).max(b.height - b.hotspot.1) as i32;
    let mut d = 0u64;
    for y in y0..y1 {
        for x in x0..x1 {
            let (p, q) = (px(a, x, y), px(b, x, y));
            // Colour counts as far as it is seen.
            let (pa, qa) = (u32::from(p[3]), u32::from(q[3]));
            for k in 0..3 {
                d += u64::from((u32::from(p[k]) * pa / 255).abs_diff(u32::from(q[k]) * qa / 255));
            }
            d += u64::from(pa.abs_diff(qa));
        }
    }
    let n = ((x1 - x0) * (y1 - y0)).max(1) as f32;
    d as f32 / (n * 4.0 * 255.0)
}

/// The image of `images` made at scale `scale` most like `c`, and how different it is.
fn nearest_cursor(
    images: &[CursorEntry],
    c: &crate::cursor::Image,
    scale: u32,
) -> Option<(usize, f32)> {
    images
        .iter()
        .enumerate()
        .filter(|(_, e)| e.scale == scale)
        .map(|(i, e)| (i, cursor_difference(c, &e.pointer)))
        .min_by(|a, b| a.1.total_cmp(&b.1))
}

/// `c` enlarged by `scale` with nearest neighbours.
fn scale_cursor(c: &crate::cursor::Image, scale: f32) -> crate::cursor::Image {
    let w = ((c.width as f32 * scale).round() as u32).max(1);
    let h = ((c.height as f32 * scale).round() as u32).max(1);
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        let sy = ((y as f32 / scale) as u32).min(c.height - 1);
        for x in 0..w {
            let sx = ((x as f32 / scale) as u32).min(c.width - 1);
            let i = 4 * (sy * c.width + sx) as usize;
            rgba.extend_from_slice(&c.rgba[i..i + 4]);
        }
    }
    let hot = |v: u32, n: u32| ((v as f32 * scale) as u32).min(n - 1);
    crate::cursor::Image {
        width: w,
        height: h,
        hotspot: (hot(c.hotspot.0, w), hot(c.hotspot.1, h)),
        rgba,
    }
}

fn keyboard(
    mut keys: MessageReader<KeyboardInput>,
    held: Res<ButtonInput<KeyCode>>,
    mut windows: Query<&mut Window>,
) {
    for k in keys.read() {
        // Alt+Enter and F11 switch between the window and full screen (not passed to the game).
        let alt = held.pressed(KeyCode::AltLeft) || held.pressed(KeyCode::AltRight);
        if (k.key_code == KeyCode::F11 || (alt && k.key_code == KeyCode::Enter))
            && k.state == ButtonState::Pressed
        {
            if !k.repeat {
                if let Ok(mut w) = windows.single_mut() {
                    w.mode = match w.mode {
                        WindowMode::Windowed => {
                            WindowMode::BorderlessFullscreen(MonitorSelection::Current)
                        }
                        _ => WindowMode::Windowed,
                    };
                }
            }
            continue;
        }
        // Android's back button is Escape (the pause menu); any other key means the keyboard is
        // used (the on-screen buttons go).
        let back = k.logical_key == bevy::input::keyboard::Key::BrowserBack;
        if !back && k.state == ButtonState::Pressed {
            crate::touch::set_in_use(false);
        }
        let code = if back { KeyCode::Escape } else { k.key_code };
        if let Some((scan, vk)) = key_codes(code) {
            input(Input::Key {
                scan,
                vk,
                down: k.state == ButtonState::Pressed,
            });
        }
    }
}

fn mouse(
    mut motion: MessageReader<MouseMotion>,
    mut buttons: MessageReader<MouseButtonInput>,
    mut wheel: MessageReader<MouseWheel>,
    mut moved: MessageReader<CursorMoved>,
    windows: Query<&Window>,
    screen: Res<Screen>,
) {
    for m in motion.read() {
        input(Input::MouseMotion {
            dx: m.delta.x.round() as i32,
            dy: m.delta.y.round() as i32,
        });
    }
    for w in wheel.read() {
        input(Input::Wheel {
            dz: (w.y * 120.0) as i32,
        });
    }
    for b in buttons.read() {
        if b.state == ButtonState::Pressed {
            crate::touch::set_in_use(false);
        }
        let n = match b.button {
            MouseButton::Left => 0,
            MouseButton::Right => 1,
            MouseButton::Middle => 2,
            _ => continue,
        };
        input(Input::MouseButton {
            button: n,
            down: b.state == ButtonState::Pressed,
        });
    }
    let Ok(win) = windows.single() else {
        return;
    };
    for c in moved.read() {
        let (x, y) = to_game(win, &screen, c.position);
        input(Input::Cursor { x, y });
    }
}

/// The gamepads' vibration follows the game's force feedback (`rumble`): when it changes, and
/// renewed while it lasts (Bevy's rumbles have a duration). On Android through its own API.
fn vibrate(
    pads: Query<Entity, With<Gamepad>>,
    mut requests: MessageWriter<GamepadRumbleRequest>,
    mut last: Local<(f32, Option<std::time::Instant>)>,
) {
    use std::time::{Duration, Instant};
    const RENEW: Duration = Duration::from_millis(100);
    let s = crate::rumble::strength();
    let now = Instant::now();
    let due = last.1.is_none_or(|t| now.duration_since(t) >= RENEW);
    if (s - last.0).abs() < 0.02 && !(s > 0.0 && due) {
        return;
    }
    *last = (s, Some(now));
    #[cfg(target_os = "android")]
    crate::android::vibrate(s, RENEW + RENEW / 2);
    for gamepad in &pads {
        requests.write(GamepadRumbleRequest::Stop { gamepad });
        if s > 0.0 {
            requests.write(GamepadRumbleRequest::Add {
                gamepad,
                duration: RENEW + RENEW / 2,
                intensity: GamepadRumbleIntensity {
                    strong_motor: s,
                    weak_motor: s,
                },
            });
        }
    }
}

/// The gamepads as one PlayStation pad (their buttons together, the first one's direction that
/// is pushed), sent when it changes.
fn gamepad(pads: Query<&Gamepad>, mut last: Local<Option<(u16, i16, i16)>>) {
    use crate::pad;
    const BUTTONS: [(GamepadButton, u16); 12] = [
        (GamepadButton::West, pad::SQUARE),
        (GamepadButton::North, pad::TRIANGLE),
        (GamepadButton::South, pad::CROSS),
        (GamepadButton::East, pad::CIRCLE),
        (GamepadButton::LeftTrigger2, pad::L2),
        (GamepadButton::LeftTrigger, pad::L1),
        (GamepadButton::RightTrigger2, pad::R2),
        (GamepadButton::RightTrigger, pad::R1),
        (GamepadButton::Select, pad::SELECT),
        (GamepadButton::Start, pad::START),
        (GamepadButton::LeftThumb, pad::L3),
        (GamepadButton::RightThumb, pad::R3),
    ];
    let mut buttons = 0u16;
    let mut dir = Vec2::ZERO;
    for g in &pads {
        let before = buttons;
        for (b, bit) in BUTTONS {
            if g.pressed(b) {
                buttons |= bit;
            }
        }
        // Whose buttons: the maker of the gamepad pressed.
        if buttons != before {
            if let Some(v) = g.vendor_id() {
                pad::set_style(pad::style_of_vendor(v));
            }
        }
        // The directional buttons push all the way; the stick as far as it is pushed.
        let d = g.dpad();
        let v = if d != Vec2::ZERO { d } else { g.left_stick() };
        if dir == Vec2::ZERO {
            dir = v;
        }
    }
    let axis = |v: f32| (v.clamp(-1.0, 1.0) * 32767.0) as i16;
    // Bevy's y points up, DirectInput's down.
    let now = (buttons, axis(dir.x), axis(-dir.y));
    if *last != Some(now) {
        *last = Some(now);
        if now != (0, 0, 0) {
            crate::touch::set_in_use(false);
        }
        input(Input::Pad {
            buttons: now.0,
            x: now.1,
            y: now.2,
        });
    }
}

/// Window coordinates to the game's, through the letterboxing.
fn to_game(win: &Window, screen: &Screen, p: Vec2) -> (i32, i32) {
    let (w, h) = (win.width(), win.height());
    let (gw, gh) = (screen.game.0 as f32, screen.game.1 as f32);
    let scale = (w / gw).min(h / gh);
    let (ox, oy) = ((w - gw * scale) / 2.0, (h - gh * scale) / 2.0);
    let x = ((p.x - ox) / scale).clamp(0.0, gw - 1.0);
    let y = ((p.y - oy) / scale).clamp(0.0, gh - 1.0);
    (x as i32, y as i32)
}

/// The fingers on a touch screen, as the mouse: the first one moves the pointer and holds the
/// left button, a second one meanwhile holds the right button.
#[derive(Resource, Default)]
struct Fingers {
    /// The first finger, where it is in the game, and whether its button is down yet.
    first: Option<(u64, (i32, i32), bool)>,
    second: Option<u64>,
    /// Fingers on the game's on-screen buttons: which finger, which button.
    on_buttons: Vec<(u64, u8)>,
}

fn touch(
    mut touches: MessageReader<TouchInput>,
    mut f: ResMut<Fingers>,
    windows: Query<&Window>,
    screen: Res<Screen>,
) {
    let Ok(win) = windows.single() else {
        return;
    };
    // The button goes down a frame after the pointer moved there: the game takes the position
    // from its window messages, the button from DirectInput (docs/RESEARCH.md).
    if let Some((_, _, down @ false)) = &mut f.first {
        *down = true;
        input(Input::MouseButton {
            button: 0,
            down: true,
        });
    }
    for t in touches.read() {
        crate::touch::set_in_use(true);
        let pos = to_game(win, &screen, t.position);
        let first = f.first.map(|(id, ..)| id);
        // A finger that starts on an on-screen button holds it until it is lifted.
        if t.phase == TouchPhase::Started {
            if let Some(b) = crate::touch::hit(pos.0 as f32, pos.1 as f32) {
                f.on_buttons.push((t.id, b));
                crate::touch::set_held(f.on_buttons.iter().fold(0, |m, (_, b)| m | 1 << b));
                continue;
            }
        }
        if let Some(i) = f.on_buttons.iter().position(|(id, _)| *id == t.id) {
            if matches!(t.phase, TouchPhase::Ended | TouchPhase::Canceled) {
                f.on_buttons.remove(i);
                crate::touch::set_held(f.on_buttons.iter().fold(0, |m, (_, b)| m | 1 << b));
            }
            continue;
        }
        match t.phase {
            TouchPhase::Started if first.is_none() => {
                input(Input::Cursor { x: pos.0, y: pos.1 });
                f.first = Some((t.id, pos, false));
            }
            TouchPhase::Started if f.second.is_none() => {
                f.second = Some(t.id);
                input(Input::MouseButton {
                    button: 1,
                    down: true,
                });
            }
            TouchPhase::Moved if first == Some(t.id) => {
                if let Some((_, at, _)) = &mut f.first {
                    input(Input::MouseMotion {
                        dx: pos.0 - at.0,
                        dy: pos.1 - at.1,
                    });
                    *at = pos;
                }
                input(Input::Cursor { x: pos.0, y: pos.1 });
            }
            TouchPhase::Ended | TouchPhase::Canceled if first == Some(t.id) => {
                if let Some((_, _, down)) = f.first.take() {
                    input(Input::Cursor { x: pos.0, y: pos.1 });
                    if down {
                        input(Input::MouseButton {
                            button: 0,
                            down: false,
                        });
                    } else {
                        // Lifted within the frame: a click all the same.
                        input(Input::MouseButton {
                            button: 0,
                            down: true,
                        });
                        input(Input::MouseButton {
                            button: 0,
                            down: false,
                        });
                    }
                }
            }
            TouchPhase::Ended | TouchPhase::Canceled if f.second == Some(t.id) => {
                f.second = None;
                input(Input::MouseButton {
                    button: 1,
                    down: false,
                });
            }
            _ => {}
        }
    }
}

fn close(mut closed: MessageReader<WindowCloseRequested>) {
    if closed.read().next().is_some() {
        std::process::exit(0);
    }
}

/// A Bevy key as a DirectInput scan code and a Windows virtual key.
fn key_codes(k: KeyCode) -> Option<(u8, u32)> {
    use KeyCode::*;
    Some(match k {
        Escape => (0x01, 0x1b),
        Digit1 => (0x02, 0x31),
        Digit2 => (0x03, 0x32),
        Digit3 => (0x04, 0x33),
        Digit4 => (0x05, 0x34),
        Digit5 => (0x06, 0x35),
        Digit6 => (0x07, 0x36),
        Digit7 => (0x08, 0x37),
        Digit8 => (0x09, 0x38),
        Digit9 => (0x0a, 0x39),
        Digit0 => (0x0b, 0x30),
        Minus => (0x0c, 0xbd),
        Equal => (0x0d, 0xbb),
        Backspace => (0x0e, 0x08),
        Tab => (0x0f, 0x09),
        KeyQ => (0x10, 0x51),
        KeyW => (0x11, 0x57),
        KeyE => (0x12, 0x45),
        KeyR => (0x13, 0x52),
        KeyT => (0x14, 0x54),
        KeyY => (0x15, 0x59),
        KeyU => (0x16, 0x55),
        KeyI => (0x17, 0x49),
        KeyO => (0x18, 0x4f),
        KeyP => (0x19, 0x50),
        BracketLeft => (0x1a, 0xdb),
        BracketRight => (0x1b, 0xdd),
        Enter => (0x1c, 0x0d),
        ControlLeft => (0x1d, 0x11),
        KeyA => (0x1e, 0x41),
        KeyS => (0x1f, 0x53),
        KeyD => (0x20, 0x44),
        KeyF => (0x21, 0x46),
        KeyG => (0x22, 0x47),
        KeyH => (0x23, 0x48),
        KeyJ => (0x24, 0x4a),
        KeyK => (0x25, 0x4b),
        KeyL => (0x26, 0x4c),
        Semicolon => (0x27, 0xba),
        Quote => (0x28, 0xde),
        Backquote => (0x29, 0xc0),
        ShiftLeft => (0x2a, 0x10),
        Backslash => (0x2b, 0xdc),
        KeyZ => (0x2c, 0x5a),
        KeyX => (0x2d, 0x58),
        KeyC => (0x2e, 0x43),
        KeyV => (0x2f, 0x56),
        KeyB => (0x30, 0x42),
        KeyN => (0x31, 0x4e),
        KeyM => (0x32, 0x4d),
        Comma => (0x33, 0xbc),
        Period => (0x34, 0xbe),
        Slash => (0x35, 0xbf),
        ShiftRight => (0x36, 0x10),
        NumpadMultiply => (0x37, 0x6a),
        AltLeft => (0x38, 0x12),
        Space => (0x39, 0x20),
        CapsLock => (0x3a, 0x14),
        F1 => (0x3b, 0x70),
        F2 => (0x3c, 0x71),
        F3 => (0x3d, 0x72),
        F4 => (0x3e, 0x73),
        F5 => (0x3f, 0x74),
        F6 => (0x40, 0x75),
        F7 => (0x41, 0x76),
        F8 => (0x42, 0x77),
        F9 => (0x43, 0x78),
        F10 => (0x44, 0x79),
        F11 => (0x57, 0x7a),
        F12 => (0x58, 0x7b),
        Numpad7 => (0x47, 0x67),
        Numpad8 => (0x48, 0x68),
        Numpad9 => (0x49, 0x69),
        NumpadSubtract => (0x4a, 0x6d),
        Numpad4 => (0x4b, 0x64),
        Numpad5 => (0x4c, 0x65),
        Numpad6 => (0x4d, 0x66),
        NumpadAdd => (0x4e, 0x6b),
        Numpad1 => (0x4f, 0x61),
        Numpad2 => (0x50, 0x62),
        Numpad3 => (0x51, 0x63),
        Numpad0 => (0x52, 0x60),
        NumpadEnter => (0x9c, 0x0d),
        ControlRight => (0x9d, 0x11),
        AltRight => (0xb8, 0x12),
        Home => (0xc7, 0x24),
        ArrowUp => (0xc8, 0x26),
        PageUp => (0xc9, 0x21),
        ArrowLeft => (0xcb, 0x25),
        ArrowRight => (0xcd, 0x27),
        End => (0xcf, 0x23),
        ArrowDown => (0xd0, 0x28),
        PageDown => (0xd1, 0x22),
        Insert => (0xd2, 0x2d),
        Delete => (0xd3, 0x2e),
        IntlBackslash => (0x56, 0xe2),
        _ => return None,
    })
}
