// SPDX-License-Identifier: MIT
//! The host below the HLE layer, in pure Rust (no SDL):
//!
//! - `bevy`: the game's backend. Window, input, gamepads, audio and the frame loop come from Bevy;
//!   the Direct3D 7 and DirectDraw draws the HLE layer records are replayed in a custom render
//!   node on Bevy's wgpu device (Vulkan, Direct3D 12, Metal, WebGPU, WebGL2)
//! - `null`: no window, no audio, for tests, CI and scripted runs
//!
//! Nothing above this crate names Bevy or wgpu types.

/// Which host backend a program runs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Backend {
    /// The Bevy app.
    #[default]
    Bevy,
    /// Headless: frames are kept for screenshots, audio is discarded.
    Null,
}
