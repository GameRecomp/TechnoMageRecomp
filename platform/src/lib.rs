// SPDX-License-Identifier: MIT
//! The host below the HLE layer, in pure Rust (no SDL):
//!
//! - window and input: `winit` (desktop and browser canvas)
//! - rendering: `wgpu` (Vulkan, Metal, Direct3D 12, WebGPU, WebGL2); the Direct3D 7 calls the
//!   HLE layer receives are translated into wgpu pipelines; DirectDraw surfaces are textures
//! - audio: `cpal` (desktop and Web Audio), a mixer for the Miles samples and DirectSound
//!   buffers
//! - `null`: no window, no audio, for tests, CI and scripted runs
//!
//! Nothing above this crate names winit, wgpu or cpal types.

/// Which host backend a program runs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Backend {
    /// winit + wgpu + cpal.
    #[default]
    Native,
    /// Headless: frames are kept for screenshots, audio is discarded.
    Null,
}
