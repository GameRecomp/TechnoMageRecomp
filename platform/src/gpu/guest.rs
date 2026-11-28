// SPDX-License-Identifier: MIT
//! What the passes over a render target kept on the GPU need to know of the surface the game
//! sees (natively `resident`, in a browser web/gl.js), and their uniforms.

/// A surface as the game sees it: its size, its colour bits per channel (red, green, blue,
/// alpha: 0 for none), its bytes per pixel, the widescreen margin on each side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Guest {
    pub width: u32,
    pub height: u32,
    pub bits: [u32; 4],
    pub bpp: u32,
    pub margin: u32,
}

impl Guest {
    /// Whether its pixels are RGB565 (the format the present table expands by its bits).
    pub(crate) fn rgb565(&self) -> bool {
        self.bpp == 16 && self.bits == [5, 6, 5, 0]
    }
}

/// The uniforms of a pass for `guest` on a target of `size`.
pub(crate) fn uniforms(
    size: (u32, u32),
    guest: &Guest,
    flags: u32,
    strip: (u32, u32),
) -> [u32; 16] {
    let zero = |n: u32| if n == 0 { 255 } else { 0 };
    let scaled = u32::from(size != (guest.width, guest.height));
    [
        size.0,
        size.1,
        guest.width,
        guest.height,
        guest.margin,
        strip.0,
        strip.1,
        flags | scaled | if guest.bpp == 16 { 2 } else { 0 },
        guest.bits[0],
        guest.bits[1],
        guest.bits[2],
        guest.bits[3],
        zero(guest.bits[0]),
        zero(guest.bits[1]),
        zero(guest.bits[2]),
        zero(guest.bits[3]),
    ]
}

/// The pixels of the target before the first screen pixel's centre (as `fill_edges` has them).
pub(crate) fn strip(size: u32, view: u32) -> u32 {
    let s = size as f32 / view as f32;
    (((s - 1.0) / 2.0).ceil().max(0.0) as u32).min(size - 1)
}

/// The uniforms of an overlay picture's pass over a frame of `size` (`flags` the present's).
pub(crate) fn overlay_uniforms(size: (u32, u32), o: &super::OverlayImage, flags: u32) -> [u32; 16] {
    let g = o.gradient.unwrap_or_default();
    [
        size.0,
        size.1,
        o.x as u32,
        o.y as u32,
        o.width,
        o.height,
        u32::from(o.gradient.is_some()),
        flags,
        g[0],
        g[1],
        g[2],
        g[3],
        g[4],
        g[5],
        0,
        0,
    ]
}
