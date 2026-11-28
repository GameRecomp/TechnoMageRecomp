// SPDX-License-Identifier: MIT
//! The GPU renderer: Direct3D 7's fixed-function pixel pipeline, offscreen: on wgpu natively
//! ([`wgpu`](self) backend), on WebGL2 through the browser player's JavaScript in a browser
//! (web/gl.js, which reads the batch as `web.rs` writes it).
//!
//! The HLE layer does the vertex work (transform and lighting) and hands a batch of screen-space
//! triangles per render target, each draw with its textures and state. Natively the target stays
//! on the GPU (`resident`): batches are drawn over what it holds, the frame shown is made there
//! and handed to the window as a texture, and only when the game touches the surface (`Lock`,
//! `Blt`) its pixels go into guest memory, exactly as a read-back would have written them, so
//! the game's own 2D work mixes with the 3D as on Windows. In a browser the same happens on
//! WebGL2, the frame drawn on the player's canvas. With `RC_GPU_RESIDENT=0` each batch is
//! rendered over the target's pixels uploaded from guest memory and read back. The depth buffer
//! lives on the GPU across batches of a frame.
//!
//! One shader serves every draw (the stages, alpha test and fog are uniforms); a pipeline per
//! blend and depth setting (natively). Without a usable GPU [`Renderer::new`] returns `None` and
//! the HLE layer rasterizes in software.

use std::sync::Arc;

/// The last render's stages (ns): prepared (upload, vertices, encoding), the GPU waited for
/// (drawing and the copy back), the rows unpacked, the edges filled (for profiling).
pub static LAST_RENDER: std::sync::Mutex<[u64; 4]> = std::sync::Mutex::new([0; 4]);

mod guest;
pub use guest::Guest;
#[cfg(not(target_family = "wasm"))]
mod native;
#[cfg(not(target_family = "wasm"))]
pub use native::{read, Renderer};
#[cfg(target_family = "wasm")]
mod web;
#[cfg(target_family = "wasm")]
pub use web::{read_present, Renderer};

/// The GPU the window draws with (Bevy's device), shared with the renderer so the frame it draws
/// is shown without leaving the GPU.
#[cfg(not(target_family = "wasm"))]
pub struct Shared {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub adapter: wgpu::Adapter,
}

#[cfg(not(target_family = "wasm"))]
static SHARED: std::sync::OnceLock<Shared> = std::sync::OnceLock::new();
#[cfg(not(target_family = "wasm"))]
static EXPECT_SHARED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// A window will share its GPU (`share`): the renderer waits for it.
#[cfg(not(target_family = "wasm"))]
pub fn expect_shared() {
    EXPECT_SHARED.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// The window's GPU, once it has one.
#[cfg(not(target_family = "wasm"))]
pub fn share(s: Shared) {
    let _ = SHARED.set(s);
}

/// The window's GPU: waited for (a few seconds at most) when a window is to give one.
#[cfg(not(target_family = "wasm"))]
pub(crate) fn shared() -> Option<&'static Shared> {
    if !EXPECT_SHARED.load(std::sync::atomic::Ordering::Relaxed) {
        return None;
    }
    for _ in 0..500 {
        if let Some(s) = SHARED.get() {
            return Some(s);
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    None
}

/// While the window reconfigures its surface (a new size: full screen and back), the renderer
/// submits nothing: wgpu waits for the GPU to be idle first and fails when another thread
/// submits meanwhile ("Failed to wait for GPU to come idle before reconfiguring the Surface"),
/// which ends the program. The window closes the gate (`close`, waiting for submits under way),
/// keeps it closed a few of its frames (resizes come in a row) and opens it again (`frame`).
#[cfg(not(target_family = "wasm"))]
pub mod gate {
    use std::sync::atomic::{AtomicU32, Ordering::SeqCst};

    /// Window frames the gate stays closed for.
    static CLOSED: AtomicU32 = AtomicU32::new(0);
    /// Submits under way.
    static ACTIVE: AtomicU32 = AtomicU32::new(0);

    /// Frames a closing lasts.
    const FRAMES: u32 = 3;

    /// Submit `f`'s work once the gate is open.
    pub fn submit<R>(f: impl FnOnce() -> R) -> R {
        loop {
            while CLOSED.load(SeqCst) > 0 {
                std::thread::sleep(std::time::Duration::from_micros(200));
            }
            ACTIVE.fetch_add(1, SeqCst);
            if CLOSED.load(SeqCst) == 0 {
                break;
            }
            ACTIVE.fetch_sub(1, SeqCst);
        }
        let r = f();
        ACTIVE.fetch_sub(1, SeqCst);
        r
    }

    /// The window is about to reconfigure: no new submits, and those under way done.
    pub fn close() {
        CLOSED.store(FRAMES, SeqCst);
        while ACTIVE.load(SeqCst) > 0 {
            std::thread::yield_now();
        }
    }

    /// A window frame prepared: a closing lasts one frame less.
    pub fn frame() {
        let n = CLOSED.load(SeqCst);
        if n > 0 {
            // Only the window closes it again, on this same thread: no other change to lose.
            let _ = CLOSED.compare_exchange(n, n - 1, SeqCst, SeqCst);
        }
    }
}

/// The device and queue frames on the GPU were drawn with (to read one back).
#[cfg(not(target_family = "wasm"))]
static FRAMES_DEVICE: std::sync::OnceLock<(wgpu::Device, wgpu::Queue)> = std::sync::OnceLock::new();

#[cfg(not(target_family = "wasm"))]
pub(crate) fn set_device_of_frames(device: &wgpu::Device, queue: &wgpu::Queue) {
    let _ = FRAMES_DEVICE.set((device.clone(), queue.clone()));
}

#[cfg(not(target_family = "wasm"))]
pub(crate) fn device_of_frames() -> Option<&'static (wgpu::Device, wgpu::Queue)> {
    FRAMES_DEVICE.get()
}

/// A frame on the GPU: the texture the window shows (RGBA8; copied into an sRGB one) and the
/// device that drew it, to read it back when asked (screenshots).
#[cfg(not(target_family = "wasm"))]
#[derive(Debug, Clone)]
pub struct GpuPicture {
    pub texture: wgpu::Texture,
    pub width: u32,
    pub height: u32,
    /// The game's screen it shows.
    pub game: (u32, u32),
}

/// A frame on the GPU in a browser: drawn on the player's WebGL canvas, which the page shows.
#[cfg(target_family = "wasm")]
#[derive(Debug, Clone)]
pub struct GpuPicture {
    pub width: u32,
    pub height: u32,
    /// The game's screen it shows.
    pub game: (u32, u32),
}

pub use crate::overlay::OverlayImage;

/// A vertex in screen space (Direct3D's pre-transformed form).
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    /// x, y (pixels), z (0..1), rhw.
    pub pos: [f32; 4],
    pub diffuse: [f32; 4],
    pub specular: [f32; 4],
    pub uv0: [f32; 2],
    pub uv1: [f32; 2],
    /// Distance from the eye (table fog).
    pub eye: f32,
    /// 1 when the triangle lies on the screen texel for pixel (see [`Draw::pixel_exact`]).
    pub exact: f32,
    pub _pad: [f32; 2],
}

/// A texture's pixels (RGBA8), identified by `key` for the GPU-side cache.
#[derive(Debug, Clone)]
pub struct Texture {
    /// The surface, its version, and how it was made (bit 0 colour-keyed, others the HLE's
    /// variants: smoothed, fonts redrawn).
    pub key: (u32, u32, u32),
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<Vec<[u8; 4]>>,
    pub bilinear: bool,
    pub clamp: (bool, bool),
}

/// A texture stage: colour op and arguments, alpha op and arguments, coordinate set.
#[derive(Debug, Clone, Copy, Default)]
pub struct Stage {
    pub cop: u32,
    pub ca1: u32,
    pub ca2: u32,
    pub aop: u32,
    pub aa1: u32,
    pub aa2: u32,
    pub coords: u32,
}

/// One draw: triangles (three vertices each) and the state they are drawn in.
#[derive(Debug, Clone)]
pub struct Draw {
    pub vertices: Vec<Vertex>,
    pub textures: [Option<Texture>; 2],
    pub stages: Vec<Stage>,
    pub factor: [f32; 4],
    pub specular_add: bool,
    /// Colour-keyed texels are discarded.
    pub keyed: bool,
    /// Alpha test: (function, reference 0..1), `None` when off.
    pub alpha_test: Option<(u32, f32)>,
    /// Blending: (source, destination) D3DBLEND factors, `None` when off.
    pub blend: Option<(u32, u32)>,
    /// Depth: (compare function, write), `None` when the test is off.
    pub depth: Option<(u32, bool)>,
    /// Fog: colour, and for linear table fog (start, end); vertex fog otherwise.
    pub fog: Option<Fog>,
    /// Some of its triangles lie on the screen texel for pixel (2D drawn with Direct3D:
    /// panels, text; [`Vertex::exact`]): drawn larger than the screen, they are sampled at the
    /// centre of the screen's pixel each drawn pixel belongs to, so they look as at the game's
    /// size, only enlarged.
    pub pixel_exact: bool,
    /// How many times larger texture 0 is than the game's (1, or 4 for one the HLE enlarged:
    /// smoothed, fonts redrawn). Its pixel-exact triangles then take the texel the game's pixel
    /// shows, sampled within at where the screen's pixel lies in the game's: four times the
    /// detail, never a texel past the game's.
    pub enlarged: u32,
}

/// Fog: the colour and, for linear table fog, (start, end); vertex fog without.
pub type Fog = ([f32; 4], Option<(f32, f32)>);

/// What a batch holds, in order.
#[derive(Debug, Clone)]
pub enum Command {
    Draw(Draw),
    /// Clear the depth buffer to a value (Direct3D's `Clear` with the z-buffer flag).
    ClearDepth(f32),
}

/// Rims around `tris`, `d` screen pixels wide. The games place their vertices on whole screen
/// pixels, so meshes that meet do not quite: at the screen's size the gaps between them hit a
/// pixel centre now and then (black lines between a level's tiles where the background is black,
/// as widescreen draws it), drawn larger they are lines through which what lies behind shows. A rim continues its triangle (depth, texture coordinates and colours
/// along their planes, within the triangle's own ranges) and is drawn with its depth pushed
/// back: it fills a gap, but loses against any surface drawn there itself.
pub(crate) fn rims(tris: &[Vertex], d: f32) -> Vec<Vertex> {
    let cross = |a: [f32; 2], b: [f32; 2]| a[0] * b[1] - a[1] * b[0];
    let sub = |a: [f32; 2], b: [f32; 2]| [a[0] - b[0], a[1] - b[1]];
    let mut out = Vec::new();
    'tri: for t in tris.as_chunks::<3>().0 {
        if t.iter().any(|v| v.exact != 0.0) {
            continue;
        }
        let p = t.map(|v| [v.pos[0], v.pos[1]]);
        let area = cross(sub(p[1], p[0]), sub(p[2], p[0]));
        if area.abs() < 1e-3 {
            continue;
        }
        // The edge's normal facing away from the third corner.
        let normal = |from: [f32; 2], to: [f32; 2], other: [f32; 2]| {
            let e = sub(to, from);
            let len = (e[0] * e[0] + e[1] * e[1]).sqrt();
            let n = [e[1] / len, -e[0] / len];
            let o = sub(other, from);
            if n[0] * o[0] + n[1] * o[1] > 0.0 {
                [-n[0], -n[1]]
            } else {
                n
            }
        };
        let mut grown = [t[0]; 3];
        for i in 0..3 {
            let (j, k) = ((i + 1) % 3, (i + 2) % 3);
            let n1 = normal(p[i], p[j], p[k]);
            let n2 = normal(p[k], p[i], p[j]);
            let dot = n1[0] * n2[0] + n1[1] * n2[1];
            // Where the two moved edges meet, at most two rim widths out.
            let mut o = if dot > -0.99 {
                let f = d / (1.0 + dot);
                [(n1[0] + n2[0]) * f, (n1[1] + n2[1]) * f]
            } else {
                [0.0, 0.0]
            };
            let len = (o[0] * o[0] + o[1] * o[1]).sqrt();
            if len > 2.0 * d || len == 0.0 {
                let a = sub(p[i], p[j]);
                let b = sub(p[i], p[k]);
                let la = (a[0] * a[0] + a[1] * a[1]).sqrt();
                let lb = (b[0] * b[0] + b[1] * b[1]).sqrt();
                let dir = [a[0] / la + b[0] / lb, a[1] / la + b[1] / lb];
                let ld = (dir[0] * dir[0] + dir[1] * dir[1]).sqrt();
                if ld < 1e-6 {
                    continue 'tri;
                }
                o = [dir[0] / ld * 2.0 * d, dir[1] / ld * 2.0 * d];
            }
            let q = [p[i][0] + o[0], p[i][1] + o[1]];
            // The point's weights of the three corners (screen space, may be negative).
            let la = cross(sub(p[1], q), sub(p[2], q)) / area;
            let lb = cross(sub(p[2], q), sub(p[0], q)) / area;
            let l = [la, lb, 1.0 - la - lb];
            let rhw: f32 = (0..3).map(|c| l[c] * t[c].pos[3]).sum();
            let least = t.iter().map(|v| v.pos[3]).fold(f32::INFINITY, f32::min);
            if rhw.is_nan() || rhw <= 0.5 * least {
                continue 'tri;
            }
            // An attribute along the triangle (perspective-correct), within its corners' range.
            let along = |get: &dyn Fn(&Vertex) -> f32| {
                let v: f32 = (0..3).map(|c| l[c] * t[c].pos[3] * get(&t[c])).sum::<f32>() / rhw;
                let lo = t.iter().map(get).fold(f32::INFINITY, f32::min);
                let hi = t.iter().map(get).fold(f32::NEG_INFINITY, f32::max);
                v.clamp(lo, hi)
            };
            let mut v = t[i];
            v.pos = [
                q[0],
                q[1],
                (0..3)
                    .map(|c| l[c] * t[c].pos[2])
                    .sum::<f32>()
                    .clamp(0.0, 1.0),
                rhw,
            ];
            for c in 0..4 {
                v.diffuse[c] = along(&|v: &Vertex| v.diffuse[c]);
                v.specular[c] = along(&|v: &Vertex| v.specular[c]);
            }
            for c in 0..2 {
                v.uv0[c] = along(&|v: &Vertex| v.uv0[c]);
                v.uv1[c] = along(&|v: &Vertex| v.uv1[c]);
            }
            v.eye = along(&|v: &Vertex| v.eye);
            grown[i] = v;
        }
        // A band along each edge, from the triangle to its grown edge.
        for i in 0..3 {
            let j = (i + 1) % 3;
            out.extend([t[i], t[j], grown[j], t[i], grown[j], grown[i]]);
        }
    }
    out
}
