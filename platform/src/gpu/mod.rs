// SPDX-License-Identifier: MIT
//! The GPU renderer: Direct3D 7's fixed-function pixel pipeline, offscreen: on wgpu natively
//! ([`wgpu`](self) backend), on WebGL2 through the browser player's JavaScript in a browser
//! (web/gl.js, which reads the batch as `web.rs` writes it).
//!
//! The HLE layer does the vertex work (transform and lighting) and hands a batch of screen-space
//! triangles per render target, each draw with its textures and state. This renders the batch on
//! top of the target's current pixels (uploaded from guest memory) and returns the result as
//! RGBA8, which the HLE layer writes back into guest memory: the guest's surfaces stay the one
//! truth, so the game's own 2D work on them (`Lock`, `Blt`) mixes with the 3D as on Windows.
//! The depth buffer lives on the GPU across batches of a frame.
//!
//! One shader serves every draw (the stages, alpha test and fog are uniforms); a pipeline per
//! blend and depth setting (natively). Without a usable GPU [`Renderer::new`] returns `None` and
//! the HLE layer rasterizes in software.

use std::sync::Arc;

/// The last render's stages (ns): prepared (upload, vertices, encoding), the GPU waited for
/// (drawing and the copy back), the rows unpacked, the edges filled (for profiling).
pub static LAST_RENDER: std::sync::Mutex<[u64; 4]> = std::sync::Mutex::new([0; 4]);

#[cfg(not(target_family = "wasm"))]
mod native;
#[cfg(not(target_family = "wasm"))]
pub use native::Renderer;
#[cfg(target_family = "wasm")]
mod web;
#[cfg(target_family = "wasm")]
pub use web::Renderer;

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
    pub key: (u32, u32, bool),
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
