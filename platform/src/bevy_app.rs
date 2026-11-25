// SPDX-License-Identifier: MIT
//! The Bevy backend: a window that shows the game's latest frame, scaled to fit with its aspect
//! kept; keyboard, mouse and window events handed to the input sink; the audio mixer played as
//! one endless Bevy audio source. The guest runs on its own thread; this owns the main thread.

use crate::{audio, input, Input};
use bevy::asset::RenderAssetUsages;
use bevy::audio::{AddAudioSource, AudioPlayer, Decodable};
use bevy::image::ImageSampler;
use bevy::input::keyboard::KeyboardInput;
use bevy::input::mouse::{MouseButtonInput, MouseMotion, MouseWheel};
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
    size: (u32, u32),
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
    .add_systems(Startup, setup)
    .add_systems(
        Update,
        (show_frame, keyboard, mouse, close, hardware_cursor),
    );
    crate::cursor::set_enabled(true);
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
        if (f.width, f.height) != screen.size {
            screen.size = (f.width, f.height);
            if let Some(mut img) = images.get_mut(&screen.image) {
                *img = new_image(f.width, f.height);
            }
        }
        if let Some(mut img) = images.get_mut(&screen.image) {
            img.data = Some(f.rgba);
        }
    }
    let Ok(win) = windows.single() else {
        return;
    };
    let (w, h) = (win.width(), win.height());
    let (gw, gh) = (screen.size.0 as f32, screen.size.1 as f32);
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
    /// reuse its image, and there are at most `CURSOR_IMAGES` of them.
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
/// Cursor images kept at most; then the most similar one stands in (the pointer turns with the
/// mouse and sways: in a long game it takes thousands of slightly different forms).
const CURSOR_IMAGES: usize = 256;

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
    let (gw, gh) = (screen.size.0 as f32, screen.size.1 as f32);
    let fit = (win.width() / gw).min(win.height() / gh) * win.scale_factor();
    let largest = (CURSOR_MAX as f32 / p.width.max(p.height).max(1) as f32).max(1.0);
    let scale = fit.clamp(1.0, largest);
    let key = (generation, scale.to_bits());
    if hw.shown == Some(key) && options.visible {
        return;
    }
    let scale_bits = scale.to_bits();
    let image = (Arc::unwrap_or_clone(p), scale_bits);
    let found = hw.index.get(&image).copied();
    let at = match found {
        Some(i) => i,
        None if hw.images.len() < CURSOR_IMAGES => {
            let scaled = scale_cursor(&image.0, scale);
            let hotspot = (scaled.hotspot.0 as u16, scaled.hotspot.1 as u16);
            let img = Image::new(
                Extent3d {
                    width: scaled.width,
                    height: scaled.height,
                    depth_or_array_layers: 1,
                },
                TextureDimension::D2,
                scaled.rgba,
                TextureFormat::Rgba8UnormSrgb,
                RenderAssetUsages::MAIN_WORLD,
            );
            let h = images.add(img);
            hw.images.push(CursorEntry {
                pointer: image.0.clone(),
                scale: scale_bits,
                image: h,
                hotspot,
            });
            let n = hw.images.len() - 1;
            hw.index.insert(image, n);
            if std::env::var("RC_TRACE").is_ok_and(|v| v.contains("cursor")) {
                eprintln!("[cursor] {} cursor images", hw.images.len());
            }
            hw.images.len() - 1
        }
        None => {
            let near = nearest_cursor(&hw.images, &image.0, scale_bits);
            hw.index.insert(image, near);
            near
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

/// The image of `images` (at scale `scale`) most like `c`: the least difference of their pixels,
/// aligned at the hotspot.
fn nearest_cursor(images: &[CursorEntry], c: &crate::cursor::Image, scale: u32) -> usize {
    let px = |i: &crate::cursor::Image, x: i32, y: i32| -> [u8; 4] {
        let (x, y) = (x + i.hotspot.0 as i32, y + i.hotspot.1 as i32);
        if x < 0 || y < 0 || x >= i.width as i32 || y >= i.height as i32 {
            return [0; 4];
        }
        let k = 4 * (y as u32 * i.width + x as u32) as usize;
        [i.rgba[k], i.rgba[k + 1], i.rgba[k + 2], i.rgba[k + 3]]
    };
    let (x0, y0) = (-(c.hotspot.0 as i32), -(c.hotspot.1 as i32));
    let (x1, y1) = (x0 + c.width as i32, y0 + c.height as i32);
    images
        .iter()
        .enumerate()
        .filter(|(_, e)| e.scale == scale)
        .min_by_key(|(_, e)| {
            let mut d = 0u64;
            for y in y0..y1 {
                for x in x0..x1 {
                    let (a, b) = (px(c, x, y), px(&e.pointer, x, y));
                    d += (0..4).map(|k| u64::from(a[k].abs_diff(b[k]))).sum::<u64>();
                }
            }
            d
        })
        .map_or(0, |(i, _)| i)
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
        if let Some((scan, vk)) = key_codes(k.key_code) {
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
        // Window coordinates to the game's, through the letterboxing.
        let (w, h) = (win.width(), win.height());
        let (gw, gh) = (screen.size.0 as f32, screen.size.1 as f32);
        let scale = (w / gw).min(h / gh);
        let (ox, oy) = ((w - gw * scale) / 2.0, (h - gh * scale) / 2.0);
        let x = ((c.position.x - ox) / scale).clamp(0.0, gw - 1.0);
        let y = ((c.position.y - oy) / scale).clamp(0.0, gh - 1.0);
        input(Input::Cursor {
            x: x as i32,
            y: y as i32,
        });
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
