// SPDX-License-Identifier: MIT
//! The gamepad's buttons as pictures of our own, drawn into a frame where the game shows a key
//! (its dialogues' key pictures, `controls.rs`): smooth shapes at any size, in the colours of the
//! gamepad's maker (PlayStation's symbols, Xbox's and Nintendo's letters).

use rc_platform::pad::Style;

/// A colour, components 0 to 1.
type Rgb = [f32; 3];

const fn rgb(v: u32) -> Rgb {
    [
        ((v >> 16) & 0xff) as f32 / 255.0,
        ((v >> 8) & 0xff) as f32 / 255.0,
        (v & 0xff) as f32 / 255.0,
    ]
}

const BODY: Rgb = rgb(0x26_2628);
const RIM: Rgb = rgb(0x6a_6a70);
const LABEL: Rgb = rgb(0xf0_f0f0);

/// The buttons in `rc_platform::pad`'s order.
pub const SQUARE: usize = 0;
pub const TRIANGLE: usize = 1;
pub const CROSS: usize = 2;
pub const CIRCLE: usize = 3;
pub const L2: usize = 4;
pub const L1: usize = 5;
pub const R2: usize = 6;
pub const R1: usize = 7;
pub const SELECT: usize = 8;
pub const START: usize = 9;
pub const L3: usize = 10;
pub const R3: usize = 11;

/// What a button looks like.
enum Face {
    /// A round button with a PlayStation symbol in its colour.
    Symbol(Symbol, Rgb),
    /// A round button of a colour with a white letter.
    Letter(&'static str, Rgb),
    /// A shoulder button or trigger: a grey bar with a label.
    Shoulder(&'static str),
    /// Select and start, as each maker draws them.
    Small(Small),
    /// A stick pressed: a ring with a label.
    Stick(&'static str),
}

#[derive(Clone, Copy)]
enum Symbol {
    Triangle,
    Circle,
    Cross,
    Square,
}

#[derive(Clone, Copy)]
enum Small {
    /// The PlayStation's select (a bar) and start (a triangle to the right).
    PsSelect,
    PsStart,
    /// Xbox's view (two windows) and menu (three lines).
    View,
    Menu,
    /// Nintendo's minus and plus.
    Minus,
    Plus,
}

fn face(button: usize, style: Style) -> Face {
    use Style::{Nintendo, PlayStation, Xbox};
    match (style, button) {
        (PlayStation, SQUARE) => Face::Symbol(Symbol::Square, rgb(0xe0_8ad0)),
        (PlayStation, TRIANGLE) => Face::Symbol(Symbol::Triangle, rgb(0x3c_d0a0)),
        (PlayStation, CROSS) => Face::Symbol(Symbol::Cross, rgb(0x7c_a8f0)),
        (PlayStation, CIRCLE) => Face::Symbol(Symbol::Circle, rgb(0xf0_5a64)),
        (PlayStation, L1) => Face::Shoulder("L1"),
        (PlayStation, L2) => Face::Shoulder("L2"),
        (PlayStation, R1) => Face::Shoulder("R1"),
        (PlayStation, R2) => Face::Shoulder("R2"),
        (PlayStation, SELECT) => Face::Small(Small::PsSelect),
        (PlayStation, START) => Face::Small(Small::PsStart),
        // Xbox: A green (below), B red (right), X blue (left), Y yellow (top).
        (Xbox, SQUARE) => Face::Letter("X", rgb(0x2f_6fe0)),
        (Xbox, TRIANGLE) => Face::Letter("Y", rgb(0xe8_b818)),
        (Xbox, CROSS) => Face::Letter("A", rgb(0x3c_a83c)),
        (Xbox, CIRCLE) => Face::Letter("B", rgb(0xd8_3438)),
        (Xbox, L1) => Face::Shoulder("LB"),
        (Xbox, L2) => Face::Shoulder("LT"),
        (Xbox, R1) => Face::Shoulder("RB"),
        (Xbox, R2) => Face::Shoulder("RT"),
        (Xbox, SELECT) => Face::Small(Small::View),
        (Xbox, START) => Face::Small(Small::Menu),
        // Nintendo: B below, A right, Y left, X top, in the Super Nintendo's colours.
        (Nintendo, SQUARE) => Face::Letter("Y", rgb(0x2c_9a4c)),
        (Nintendo, TRIANGLE) => Face::Letter("X", rgb(0x3a_5ad0)),
        (Nintendo, CROSS) => Face::Letter("B", rgb(0xe0_b020)),
        (Nintendo, CIRCLE) => Face::Letter("A", rgb(0xd0_3030)),
        (Nintendo, L1) => Face::Shoulder("L"),
        (Nintendo, L2) => Face::Shoulder("ZL"),
        (Nintendo, R1) => Face::Shoulder("R"),
        (Nintendo, R2) => Face::Shoulder("ZR"),
        (Nintendo, SELECT) => Face::Small(Small::Minus),
        (Nintendo, START) => Face::Small(Small::Plus),
        (_, L3) => Face::Stick("L"),
        (_, R3) => Face::Stick("R"),
        _ => Face::Stick(""),
    }
}

// --- Shapes as signed distances (negative inside), in the picture's units: -1 to 1 ------------

fn length(x: f32, y: f32) -> f32 {
    (x * x + y * y).sqrt()
}

fn segment(px: f32, py: f32, (ax, ay): (f32, f32), (bx, by): (f32, f32)) -> f32 {
    let (dx, dy) = (bx - ax, by - ay);
    let t = (((px - ax) * dx + (py - ay) * dy) / (dx * dx + dy * dy).max(1e-6)).clamp(0.0, 1.0);
    length(px - ax - t * dx, py - ay - t * dy)
}

fn rounded_box(px: f32, py: f32, hw: f32, hh: f32, r: f32) -> f32 {
    let qx = px.abs() - hw + r;
    let qy = py.abs() - hh + r;
    length(qx.max(0.0), qy.max(0.0)) + qx.max(qy).min(0.0) - r
}

/// An equilateral triangle pointing up (or right), centred, of circumradius `r`.
fn triangle(px: f32, py: f32, r: f32) -> f32 {
    let k = 3f32.sqrt();
    let (x, y) = (px.abs() / r, (py + r * 0.25) / r);
    let d = (k * x + y - 1.0).max(-y - 0.5) * 0.5;
    d * r * 1.2
}

/// A glyph's strokes in a box 0.6 wide and 1 high (y down).
/// A stroke: from one point to another.
type Stroke = ((f32, f32), (f32, f32));

fn glyph(c: char) -> &'static [Stroke] {
    match c {
        'A' => &[
            ((0.0, 1.0), (0.3, 0.0)),
            ((0.3, 0.0), (0.6, 1.0)),
            ((0.12, 0.64), (0.48, 0.64)),
        ],
        'B' => &[
            ((0.0, 0.0), (0.0, 1.0)),
            ((0.0, 0.0), (0.4, 0.0)),
            ((0.4, 0.0), (0.54, 0.12)),
            ((0.54, 0.12), (0.54, 0.35)),
            ((0.54, 0.35), (0.4, 0.47)),
            ((0.0, 0.47), (0.42, 0.47)),
            ((0.42, 0.47), (0.6, 0.6)),
            ((0.6, 0.6), (0.6, 0.86)),
            ((0.6, 0.86), (0.44, 1.0)),
            ((0.44, 1.0), (0.0, 1.0)),
        ],
        'X' => &[((0.0, 0.0), (0.6, 1.0)), ((0.6, 0.0), (0.0, 1.0))],
        'Y' => &[
            ((0.0, 0.0), (0.3, 0.5)),
            ((0.6, 0.0), (0.3, 0.5)),
            ((0.3, 0.5), (0.3, 1.0)),
        ],
        'L' => &[((0.0, 0.0), (0.0, 1.0)), ((0.0, 1.0), (0.55, 1.0))],
        'R' => &[
            ((0.0, 1.0), (0.0, 0.0)),
            ((0.0, 0.0), (0.42, 0.0)),
            ((0.42, 0.0), (0.57, 0.13)),
            ((0.57, 0.13), (0.57, 0.34)),
            ((0.57, 0.34), (0.42, 0.47)),
            ((0.42, 0.47), (0.0, 0.47)),
            ((0.28, 0.47), (0.6, 1.0)),
        ],
        'T' => &[((0.0, 0.0), (0.6, 0.0)), ((0.3, 0.0), (0.3, 1.0))],
        'Z' => &[
            ((0.0, 0.0), (0.6, 0.0)),
            ((0.6, 0.0), (0.0, 1.0)),
            ((0.0, 1.0), (0.6, 1.0)),
        ],
        '1' => &[((0.12, 0.22), (0.36, 0.0)), ((0.36, 0.0), (0.36, 1.0))],
        '2' => &[
            ((0.02, 0.2), (0.16, 0.03)),
            ((0.16, 0.03), (0.44, 0.03)),
            ((0.44, 0.03), (0.58, 0.18)),
            ((0.58, 0.18), (0.58, 0.36)),
            ((0.58, 0.36), (0.0, 1.0)),
            ((0.0, 1.0), (0.6, 1.0)),
        ],
        _ => &[],
    }
}

/// The distance to a label's strokes, set in a box of half width `hw` and half height `hh`
/// around the centre.
fn text(px: f32, py: f32, label: &str, hw: f32, hh: f32) -> f32 {
    const GAP: f32 = 0.28;
    let n = label.chars().count() as f32;
    let width = 0.6 * n + GAP * (n - 1.0).max(0.0);
    let scale = (2.0 * hh).min(2.0 * hw / width);
    let x0 = -width * scale / 2.0;
    let y0 = -scale / 2.0;
    let mut d = f32::MAX;
    for (i, c) in label.chars().enumerate() {
        let ox = x0 + i as f32 * (0.6 + GAP) * scale;
        for &((ax, ay), (bx, by)) in glyph(c) {
            let a = (ox + ax * scale, y0 + ay * scale);
            let b = (ox + bx * scale, y0 + by * scale);
            d = d.min(segment(px, py, a, b));
        }
    }
    d
}

/// The picture at a point: its colour and coverage (0 to 1), `aa` the size of a pixel.
fn shade(face: &Face, x: f32, y: f32, aa: f32) -> (Rgb, f32) {
    let cover = |d: f32| (0.5 - d / aa).clamp(0.0, 1.0);
    let mut colour = [0.0; 3];
    let mut alpha = 0.0f32;
    let mut over = |c: Rgb, a: f32| {
        for i in 0..3 {
            colour[i] = colour[i] * (1.0 - a) + c[i] * a;
        }
        alpha = a + alpha * (1.0 - a);
    };
    let stroke = 0.075;
    match face {
        Face::Symbol(s, c) => {
            let disc = length(x, y) - 0.95;
            over(RIM, cover(disc));
            over(BODY, cover(disc + 0.1));
            let d = match s {
                Symbol::Triangle => triangle(x, y, 0.5).abs() - stroke,
                Symbol::Circle => (length(x, y) - 0.42).abs() - stroke,
                Symbol::Cross => {
                    segment(x, y, (-0.38, -0.38), (0.38, 0.38)).min(segment(
                        x,
                        y,
                        (-0.38, 0.38),
                        (0.38, -0.38),
                    )) - stroke
                }
                Symbol::Square => rounded_box(x, y, 0.38, 0.38, 0.02).abs() - stroke,
            };
            over(*c, cover(d));
        }
        Face::Letter(l, c) => {
            let disc = length(x, y) - 0.95;
            over(RIM, cover(disc));
            over(*c, cover(disc + 0.1));
            over(LABEL, cover(text(x, y, l, 0.32, 0.42) - stroke));
        }
        Face::Shoulder(l) => {
            let bar = rounded_box(x, y, 0.97, 0.8, 0.3);
            over(RIM, cover(bar));
            over(BODY, cover(bar + 0.1));
            over(LABEL, cover(text(x, y, l, 0.72, 0.48) - stroke));
        }
        Face::Small(k) => {
            let pill = rounded_box(x, y, 0.95, 0.55, 0.5);
            over(RIM, cover(pill));
            over(BODY, cover(pill + 0.1));
            let d = match k {
                Small::PsSelect => rounded_box(x, y, 0.38, 0.14, 0.05),
                Small::PsStart => triangle(-y, x, 0.36),
                Small::View => {
                    let a = rounded_box(x + 0.12, y + 0.1, 0.24, 0.18, 0.03).abs() - 0.05;
                    let b = rounded_box(x - 0.12, y - 0.1, 0.24, 0.18, 0.03).abs() - 0.05;
                    a.min(b)
                }
                Small::Menu => [-0.22, 0.0, 0.22]
                    .iter()
                    .map(|&yy| segment(x, y, (-0.32, yy), (0.32, yy)) - 0.05)
                    .fold(f32::MAX, f32::min),
                Small::Minus => segment(x, y, (-0.32, 0.0), (0.32, 0.0)) - 0.07,
                Small::Plus => {
                    segment(x, y, (-0.32, 0.0), (0.32, 0.0)).min(segment(
                        x,
                        y,
                        (0.0, -0.32),
                        (0.0, 0.32),
                    )) - 0.07
                }
            };
            over(LABEL, cover(d));
        }
        Face::Stick(l) => {
            let disc = length(x, y) - 0.95;
            over(RIM, cover(disc));
            over(BODY, cover(disc + 0.1));
            over(RIM, cover((length(x, y) - 0.7).abs() - 0.05));
            over(LABEL, cover(text(x, y, l, 0.3, 0.36) - stroke));
        }
    }
    (colour, alpha)
}

/// Draw `button` (in `rc_platform::pad`'s order) as `style` has it into the RGBA frame (`fw` by
/// `fh`) over the place `(x, y, w, h)` (frame pixels): the place filled first with the colours
/// left and right of it (so nothing of what was there shows), the button centred in it.
pub fn draw(
    frame: &mut [u8],
    fw: u32,
    fh: u32,
    (x, y, w, h): (f32, f32, f32, f32),
    button: usize,
    style: Style,
) {
    let f = face(button, style);
    let x0 = x.floor().max(0.0) as i32;
    let y0 = y.floor().max(0.0) as i32;
    let x1 = ((x + w).ceil() as i32).min(fw as i32);
    let y1 = ((y + h).ceil() as i32).min(fh as i32);
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    let size = w.min(h) * 0.98;
    let (cx, cy) = (x + w / 2.0, y + h / 2.0);
    let (half_x, half_y) = match f {
        Face::Shoulder(_) | Face::Small(_) => ((size * 0.68).min(w / 2.0), size / 2.0),
        _ => (size / 2.0, size / 2.0),
    };
    let aa = 1.0 / half_y.max(1.0);
    let px = |xx: i32, yy: i32| ((yy as u32 * fw + xx as u32) * 4) as usize;
    let left = (x0 - 1).max(0);
    let right = x1.min(fw as i32 - 1);
    for yy in y0..y1 {
        let (l, r) = (px(left, yy), px(right, yy));
        let (cl, cr) = (
            [frame[l], frame[l + 1], frame[l + 2]],
            [frame[r], frame[r + 1], frame[r + 2]],
        );
        for xx in x0..x1 {
            let t = (xx - x0) as f32 / (x1 - x0).max(1) as f32;
            let at = px(xx, yy);
            let mut out = [0f32; 3];
            for i in 0..3 {
                out[i] = (f32::from(cl[i]) * (1.0 - t) + f32::from(cr[i]) * t) / 255.0;
            }
            let u = (xx as f32 + 0.5 - cx) / half_y;
            let v = (yy as f32 + 0.5 - cy) / half_y;
            // Shoulders and pills are wider than high: their x in their own half width.
            let u = if half_x > half_y {
                u * half_y / half_x
            } else {
                u
            };
            let (c, a) = shade(&f, u, v, aa);
            for i in 0..3 {
                out[i] = out[i] * (1.0 - a) + c[i] * a;
                frame[at + i] = (out[i] * 255.0).round() as u8;
            }
        }
    }
}

// --- The touch screen's on-screen buttons ---------------------------------------------------------

/// What an on-screen button of ours shows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Glyph {
    Jump,
    CameraLeft,
    CameraRight,
    Spell,
    Tool,
    Rucksack,
}

const GOLD: Rgb = rgb(0xd8_b060);
const GOLD_BRIGHT: Rgb = rgb(0xff_e090);

/// A polyline's distance.
fn polyline(x: f32, y: f32, pts: &[(f32, f32)]) -> f32 {
    pts.windows(2)
        .map(|w| segment(x, y, w[0], w[1]))
        .fold(f32::MAX, f32::min)
}

/// An arc of radius `r` from angle `a0` to `a1` (radians, y down), as a polyline.
fn arc(x: f32, y: f32, r: f32, a0: f32, a1: f32) -> f32 {
    let n = 16;
    let pts: Vec<(f32, f32)> = (0..=n)
        .map(|i| {
            let a = a0 + (a1 - a0) * i as f32 / n as f32;
            (r * a.cos(), r * a.sin())
        })
        .collect();
    polyline(x, y, &pts)
}

/// A turning arrow: an arc over the top, its head at the left (`left`) or the right end.
fn turn(x: f32, y: f32, left: bool) -> f32 {
    use std::f32::consts::PI;
    let x = if left { -x } else { x };
    let body = arc(x, y + 0.1, 0.45, -PI * 0.95, -PI * 0.05);
    let tip = (
        0.45 * (-PI * 0.05f32).cos(),
        0.1 + 0.45 * (-PI * 0.05f32).sin(),
    );
    let head = segment(x, y, tip, (tip.0 - 0.22, tip.1 - 0.02)).min(segment(
        x,
        y,
        tip,
        (tip.0 + 0.04, tip.1 - 0.24),
    ));
    body.min(head)
}

fn glyph_distance(g: Glyph, x: f32, y: f32) -> f32 {
    match g {
        Glyph::Jump => polyline(x, y, &[(0.0, 0.45), (0.0, -0.45)])
            .min(polyline(x, y, &[(-0.3, -0.15), (0.0, -0.45), (0.3, -0.15)]))
            .min(segment(x, y, (-0.45, 0.6), (0.45, 0.6))),
        Glyph::CameraLeft => turn(x, y, true),
        Glyph::CameraRight => turn(x, y, false),
        Glyph::Spell => {
            let pts: Vec<(f32, f32)> = (0..=10)
                .map(|i| {
                    let a = -std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::PI / 5.0;
                    let r = if i % 2 == 0 { 0.55 } else { 0.24 };
                    (r * a.cos(), r * a.sin() + 0.04)
                })
                .collect();
            polyline(x, y, &pts)
        }
        Glyph::Tool => {
            // A wrench: the handle, the open jaw.
            use std::f32::consts::PI;
            let jaw = arc(x - 0.22, y + 0.22, 0.2, -PI * 0.15, PI * 1.35);
            segment(x, y, (-0.45, 0.45), (0.12, -0.12)).min(jaw)
        }
        Glyph::Rucksack => {
            let body = rounded_box(x, y - 0.12, 0.38, 0.4, 0.12).abs();
            let flap = polyline(x, y, &[(-0.38, -0.02), (0.0, 0.12), (0.38, -0.02)]);
            let handle = arc(
                x,
                y + 0.32,
                0.2,
                std::f32::consts::PI,
                std::f32::consts::TAU,
            );
            body.min(flap).min(handle)
        }
    }
}

/// Draw an on-screen button of ours into the RGBA frame over `(x, y, w, h)` (frame pixels): a
/// dark, half transparent square with a gold rim and the glyph; brighter while held.
pub fn draw_touch(
    frame: &mut [u8],
    fw: u32,
    fh: u32,
    (x, y, w, h): (f32, f32, f32, f32),
    g: Glyph,
    held: bool,
) {
    let x0 = x.floor().max(0.0) as i32;
    let y0 = y.floor().max(0.0) as i32;
    let x1 = ((x + w).ceil() as i32).min(fw as i32);
    let y1 = ((y + h).ceil() as i32).min(fh as i32);
    let half = w.min(h) / 2.0;
    let (cx, cy) = (x + w / 2.0, y + h / 2.0);
    let aa = 1.0 / half.max(1.0);
    let cover = |d: f32| (0.5 - d / aa).clamp(0.0, 1.0);
    let (fill, fill_a, line) = if held {
        (rgb(0x50_4020), 0.75, GOLD_BRIGHT)
    } else {
        (rgb(0x10_0c08), 0.55, GOLD)
    };
    for yy in y0..y1 {
        for xx in x0..x1 {
            let u = (xx as f32 + 0.5 - cx) / half;
            let v = (yy as f32 + 0.5 - cy) / half;
            let at = ((yy as u32 * fw + xx as u32) * 4) as usize;
            let mut out = [0f32; 3];
            for i in 0..3 {
                out[i] = f32::from(frame[at + i]) / 255.0;
            }
            let mut over = |c: Rgb, a: f32| {
                for i in 0..3 {
                    out[i] = out[i] * (1.0 - a) + c[i] * a;
                }
            };
            let square = rounded_box(u, v, 0.95, 0.95, 0.25);
            over(fill, cover(square) * fill_a);
            over(line, cover(square.abs() - 0.05));
            over(
                line,
                cover(glyph_distance(g, u / 0.8, v / 0.8) * 0.8 - 0.07),
            );
            for i in 0..3 {
                frame[at + i] = (out[i] * 255.0).round() as u8;
            }
        }
    }
}

/// Only the gold rim of an on-screen button (around a picture of the game's).
pub fn draw_rim(
    frame: &mut [u8],
    fw: u32,
    fh: u32,
    (x, y, w, h): (f32, f32, f32, f32),
    held: bool,
) {
    let x0 = x.floor().max(0.0) as i32;
    let y0 = y.floor().max(0.0) as i32;
    let x1 = ((x + w).ceil() as i32).min(fw as i32);
    let y1 = ((y + h).ceil() as i32).min(fh as i32);
    let half = w.min(h) / 2.0;
    let (cx, cy) = (x + w / 2.0, y + h / 2.0);
    let aa = 1.0 / half.max(1.0);
    let line = if held { GOLD_BRIGHT } else { GOLD };
    for yy in y0..y1 {
        for xx in x0..x1 {
            let u = (xx as f32 + 0.5 - cx) / half;
            let v = (yy as f32 + 0.5 - cy) / half;
            let a = (0.5 - (rounded_box(u, v, 0.95, 0.95, 0.25).abs() - 0.05) / aa).clamp(0.0, 1.0);
            let at = ((yy as u32 * fw + xx as u32) * 4) as usize;
            for i in 0..3 {
                let o = f32::from(frame[at + i]) / 255.0 * (1.0 - a) + line[i] * a;
                frame[at + i] = (o * 255.0).round() as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_button_covers_its_place_and_keeps_the_rest() {
        let (w, h) = (40u32, 30u32);
        let mut frame = vec![0u8; (w * h * 4) as usize];
        for p in frame.as_chunks_mut::<4>().0 {
            p.copy_from_slice(&[10, 20, 30, 255]);
        }
        draw(
            &mut frame,
            w,
            h,
            (10.0, 5.0, 19.0, 21.0),
            CROSS,
            Style::PlayStation,
        );
        // Outside untouched, the centre the button's dark body or its blue cross.
        assert_eq!(&frame[0..4], &[10, 20, 30, 255]);
        let centre = ((15 * w + 19) * 4) as usize;
        assert_ne!(&frame[centre..centre + 3], &[10, 20, 30]);
        for g in [
            Glyph::Jump,
            Glyph::CameraLeft,
            Glyph::CameraRight,
            Glyph::Spell,
            Glyph::Tool,
            Glyph::Rucksack,
        ] {
            draw_touch(&mut frame, w, h, (4.0, 4.0, 24.0, 24.0), g, false);
        }
        for style in [Style::PlayStation, Style::Xbox, Style::Nintendo] {
            for b in 0..12 {
                draw(&mut frame, w, h, (2.0, 2.0, 29.0, 21.0), b, style);
            }
        }
    }
}
