// SPDX-License-Identifier: MIT
//! Textures enlarged before they are drawn ("smooth textures"): an edge-directed 2× scaler of our
//! own in the manner of xBR, applied twice (4×). Each source pixel becomes 2 × 2; a corner of it
//! takes on the colour across a diagonal edge where the neighbourhood shows one running through
//! that corner, so staircases become slopes while flat areas and straight edges stay as they
//! were. Opaque colours are blended at such corners; where a texel is transparent (a colour key's
//! zero, or little alpha) the corner takes the neighbour's texel whole, so outlines get smoother
//! without fringes and a keyed texel stays exactly zero.
//!
//! After the second 2× the picture is sharpened once (`sharpen`): the blends the scaler made
//! soften every slope, and a contrast adaptive pass in the manner of AMD's RCAS (FidelityFX FSR
//! 1, MIT) gives the edges back their bite without ringing past the colours around them.

type Px = [u8; 4];

/// Whether a texel counts as transparent.
fn clear(p: Px) -> bool {
    p[3] < 128
}

/// How different two texels look (luma weighted over chroma, as edge detectors do); a transparent
/// one is far from an opaque one.
fn dist(a: Px, b: Px) -> u32 {
    if clear(a) != clear(b) {
        return 4000;
    }
    let (r, g, b2) = (
        i32::from(a[0]) - i32::from(b[0]),
        i32::from(a[1]) - i32::from(b[1]),
        i32::from(a[2]) - i32::from(b[2]),
    );
    // Y, and the two colour differences (BT.601), in fixed point.
    let y = (299 * r + 587 * g + 114 * b2).abs() / 1000;
    let u = (-169 * r - 331 * g + 500 * b2).abs() / 1000;
    let v = (500 * r - 419 * g - 81 * b2).abs() / 1000;
    (48 * y + 7 * u + 6 * v) as u32 / 8
}

/// Below this two texels are one colour for the edge rules.
const SAME: u32 = 24;

/// `width × height` texels enlarged twice in each direction.
pub fn scale2x(src: &[Px], width: usize, height: usize) -> Vec<Px> {
    let mut out = vec![[0u8; 4]; 4 * width * height];
    scale2x_rows(src, width, height, 0..height, &mut out);
    out
}

/// Part of `scale2x`: the enlarged rows of source rows `rows` (output rows `2y` and `2y + 1`)
/// written into `out`, the whole `2 width × 2 height` picture; so the work can be spread.
pub fn scale2x_rows(
    src: &[Px],
    width: usize,
    height: usize,
    rows: std::ops::Range<usize>,
    out: &mut [Px],
) {
    let (w, h) = (width as isize, height as isize);
    let at = |x: isize, y: isize| src[(y.clamp(0, h - 1) * w + x.clamp(0, w - 1)) as usize];
    let ow = 2 * width;
    for y in rows.start as isize..rows.end.min(height) as isize {
        for x in 0..w {
            let e = at(x, y);
            for (sx, sy) in [(1isize, 1isize), (-1, 1), (1, -1), (-1, -1)] {
                // The neighbourhood mirrored so that the corner looked at is the lower right.
                let p = |dx: isize, dy: isize| at(x + dx * sx, y + dy * sy);
                let corner = corner(e, p);
                let ox = 2 * x as usize + usize::from(sx > 0);
                let oy = 2 * y as usize + usize::from(sy > 0);
                out[oy * ow + ox] = corner;
            }
        }
    }
}

/// The lower right quarter of `e`'s enlarged pixel, `p(dx, dy)` its neighbours:
///
/// ```text
///         B1 C1
///      A  B  C  C4
///      D  E  F  F4
///      G  H  I  I4
///         H5 I5
/// ```
fn corner(e: Px, p: impl Fn(isize, isize) -> Px) -> Px {
    let (f, h) = (p(1, 0), p(0, 1));
    // The corner is on an edge from F to H when E differs from both and the diagonal F–H is
    // more alike than E–I, weighed over the pixels around (xBR's first level). (The same texel
    // beside it, as often in an enlarged picture, before the distances.)
    if e == f || e == h || dist(e, f) < SAME || dist(e, h) < SAME {
        return e;
    }
    let (b, c, d, g, i) = (p(0, -1), p(1, -1), p(-1, 0), p(-1, 1), p(1, 1));
    let (f4, h5, i4, i5) = (p(2, 0), p(0, 2), p(2, 1), p(1, 2));
    let along = dist(e, c) + dist(e, g) + dist(i, f4) + dist(i, h5) + 4 * dist(h, f);
    let across = dist(h, d) + dist(h, i5) + dist(f, i4) + dist(f, b) + 4 * dist(e, i);
    if along >= across {
        return e;
    }
    let near = if dist(e, f) <= dist(e, h) { f } else { h };
    if clear(e) || clear(near) {
        // An outline: the corner becomes the other side's texel whole.
        return near;
    }
    [
        ((u16::from(e[0]) + u16::from(near[0])) / 2) as u8,
        ((u16::from(e[1]) + u16::from(near[1])) / 2) as u8,
        ((u16::from(e[2]) + u16::from(near[2])) / 2) as u8,
        ((u16::from(e[3]) + u16::from(near[3])) / 2) as u8,
    ]
}

/// `width × height` texels enlarged four times in each direction (twice 2×), then sharpened.
pub fn scale4x(src: &[Px], width: usize, height: usize) -> Vec<Px> {
    let twice = scale2x(src, width, height);
    let four = scale2x(&twice, 2 * width, 2 * height);
    sharpen(&four, 4 * width, 4 * height, SHARPNESS)
}

/// How strongly `scale4x` sharpens, as a share of the strongest the pass allows.
pub const SHARPNESS: f32 = 0.6;

/// The most negative weight a neighbour may get (RCAS's limit: past it the curve is badly
/// conditioned).
const LOBE_LIMIT: f32 = 0.25 - 1.0 / 16.0;

/// `width × height` texels sharpened: each opaque texel moves away from the mean of its four
/// direct neighbours, `(e + w·(b + d + f + h)) / (1 + 4w)` with a negative `w`. The weight comes
/// from the neighbours' least and greatest value of each channel: as large as it can be without
/// taking any channel below 0 or above 1 if the texel itself were at that extreme, so flat areas
/// and soft gradients get the whole `strength` (0 to 1) and hard contrasts less. Transparent
/// texels stay exactly as they are; a transparent neighbour counts as the texel itself, so no
/// colour comes in from behind a colour key. Alpha is kept.
pub fn sharpen(src: &[Px], width: usize, height: usize, strength: f32) -> Vec<Px> {
    let mut out = src.to_vec();
    sharpen_rows(src, width, height, strength, 0..height, &mut out);
    out
}

/// Part of `sharpen`: rows `rows` of `out` (a copy of `src` beforehand) sharpened.
pub fn sharpen_rows(
    src: &[Px],
    width: usize,
    height: usize,
    strength: f32,
    rows: std::ops::Range<usize>,
    out: &mut [Px],
) {
    if width == 0 || height == 0 || strength <= 0.0 {
        return;
    }
    let rgb = |p: Px| [p[0], p[1], p[2]].map(|c| f32::from(c) / 255.0);
    for y in rows.start..rows.end.min(height) {
        for x in 0..width {
            let e = src[y * width + x];
            if clear(e) {
                continue;
            }
            let around = [
                (x, y.saturating_sub(1)),
                (x.saturating_sub(1), y),
                ((x + 1).min(width - 1), y),
                (x, (y + 1).min(height - 1)),
            ]
            .map(|(nx, ny)| src[ny * width + nx]);
            // Its neighbours its own colour (most of an enlarged picture, inside its texels):
            // nothing to sharpen.
            if around.iter().all(|&p| p == e || clear(p)) {
                continue;
            }
            let centre = rgb(e);
            let n = around.map(|p| if clear(p) { centre } else { rgb(p) });
            // The weight each channel allows; the gentlest of them is taken for all three so
            // that the hue stays.
            let mut weight = f32::NEG_INFINITY;
            for c in 0..3 {
                let lo = n.iter().map(|q| q[c]).fold(1.0f32, f32::min);
                let hi = n.iter().map(|q| q[c]).fold(0.0f32, f32::max);
                // Down to 0 from the darkest side, up to 1 from the brightest.
                let to_black = -lo / (4.0 * hi + 1e-6);
                let to_white = (1.0 - hi) / (4.0 * lo - 4.0 - 1e-6);
                weight = weight.max(to_black.max(to_white));
            }
            let w = weight.clamp(-LOBE_LIMIT, 0.0) * strength;
            if w == 0.0 {
                continue;
            }
            let p = &mut out[y * width + x];
            let r = 1.0 / (1.0 + 4.0 * w);
            for c in 0..3 {
                let sum: f32 = n.iter().map(|q| q[c]).sum();
                let v = (centre[c] + w * sum) * r;
                p[c] = (v.clamp(0.0, 1.0) * 255.0).round() as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: Px = [200, 40, 40, 255];
    const B: Px = [20, 20, 200, 255];
    const K: Px = [0, 0, 0, 0];

    #[test]
    fn flat_areas_and_straight_edges_stay() {
        // Left half A, right half B: a straight edge, nothing blended.
        let src: Vec<Px> = (0..16).map(|i| if i % 4 < 2 { A } else { B }).collect();
        let out = scale2x(&src, 4, 4);
        for (k, p) in out.iter().enumerate() {
            assert_eq!(*p, if k % 8 < 4 { A } else { B }, "texel {k}");
        }
    }

    #[test]
    fn a_staircase_gets_a_slope_and_keys_stay_exact() {
        // A diagonal of A over the transparent key.
        let src: Vec<Px> = (0..16)
            .map(|i| if i % 4 <= i / 4 { A } else { K })
            .collect();
        let out = scale4x(&src, 4, 4);
        assert_eq!(out.len(), 16 * 16);
        // Every texel is A or exactly the key: outlines are never blended.
        assert!(out.iter().all(|&p| p == A || p == K));
        // And the step's corners changed: more than the blocky picture's count of A.
        let blocky = src.iter().filter(|&&p| p == A).count() * 16;
        assert_ne!(out.iter().filter(|&&p| p == A).count(), blocky);
    }

    #[test]
    fn sharpening_steepens_a_ramp_and_leaves_flat_areas_and_keys() {
        // A soft ramp from dark to light, left to right, over four keyed rows.
        let (w, h) = (8, 8);
        let src: Vec<Px> = (0..w * h)
            .map(|i| {
                if i / w >= 4 {
                    return K;
                }
                let v = [40u8, 40, 40, 60, 140, 160, 160, 160][i % w];
                [v, v, v, 255]
            })
            .collect();
        let out = sharpen(&src, w, h, 1.0);
        // Keys stay exactly zero, the flat ends stay as they were.
        for (k, (a, b)) in src.iter().zip(&out).enumerate() {
            if *a == K || matches!(k % w, 0 | 7) {
                assert_eq!(a, b, "texel {k}");
            }
        }
        // The ramp's foot gets darker and its shoulder lighter, within black and white.
        assert!(out[3][0] < src[3][0]);
        assert!(out[4][0] > src[4][0]);
        // And alpha is kept.
        assert!(out.iter().zip(&src).all(|(a, b)| a[3] == b[3]));
    }
}
