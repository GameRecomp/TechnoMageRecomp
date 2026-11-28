// SPDX-License-Identifier: MIT
//! Textures enlarged before they are drawn ("smooth textures"): an edge-directed 2× scaler of our
//! own in the manner of xBR, applied twice (4×). Each source pixel becomes 2 × 2; a corner of it
//! takes on the colour across a diagonal edge where the neighbourhood shows one running through
//! that corner, so staircases become slopes while flat areas and straight edges stay as they
//! were. Opaque colours are blended at such corners; where a texel is transparent (a colour key's
//! zero, or little alpha) the corner takes the neighbour's texel whole, so outlines get smoother
//! without fringes and a keyed texel stays exactly zero.

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
    let (w, h) = (width as isize, height as isize);
    let at = |x: isize, y: isize| src[(y.clamp(0, h - 1) * w + x.clamp(0, w - 1)) as usize];
    let mut out = vec![[0u8; 4]; 4 * width * height];
    let ow = 2 * width;
    for y in 0..h {
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
    out
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
    let (b, c, d, f, g, h, i) = (
        p(0, -1),
        p(1, -1),
        p(-1, 0),
        p(1, 0),
        p(-1, 1),
        p(0, 1),
        p(1, 1),
    );
    let (f4, h5, i4, i5) = (p(2, 0), p(0, 2), p(2, 1), p(1, 2));
    // The corner is on an edge from F to H when E differs from both and the diagonal F–H is
    // more alike than E–I, weighed over the pixels around (xBR's first level).
    if dist(e, f) < SAME || dist(e, h) < SAME {
        return e;
    }
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

/// `width × height` texels enlarged four times in each direction (twice 2×).
pub fn scale4x(src: &[Px], width: usize, height: usize) -> Vec<Px> {
    let twice = scale2x(src, width, height);
    scale2x(&twice, 2 * width, 2 * height)
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
}
