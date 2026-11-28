// SPDX-License-Identifier: MIT
//! Smooth fonts: a game's bitmap font redrawn from an outline font (TrueType) on its texture
//! page enlarged as many times as the screen is drawn larger (`factor`), so text is drawn at
//! native resolution while the game lays it out, colours and clips it as ever.
//!
//! The game tells which rectangles of a texture page hold which character (`register`, a set per
//! font). The renderer finds the page a set lies on by the 2D quads it draws: one that shows
//! exactly a glyph's rectangle, one texel to one pixel, ties the texture to the set once three
//! of its glyphs were seen there (`note`). Such a page is then drawn enlarged (`refine`): each
//! glyph's rectangle filled with the character from the outline font, stretched to the
//! rectangle (the game's glyphs are cut to their ink), in the glyph's own colours: its fill, and
//! its outline where it has one. Texels around the glyphs stay the page's.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

type Px = [u8; 4];

/// A glyph: its rectangle on the texture page (x, y, width, height) and its character.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    pub rect: [u16; 4],
    pub ch: char,
}

/// A font's glyphs and the outline font (TrueType or OpenType data) to draw them with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlyphSet {
    pub cells: Vec<Cell>,
    pub face: &'static [u8],
}

#[derive(Default)]
struct State {
    /// The sets by the game's id for them.
    sets: HashMap<u32, Arc<GlyphSet>>,
    /// The sets a rectangle is a glyph of.
    by_rect: HashMap<[u16; 4], Vec<u32>>,
    /// Per texture and set: the glyphs seen drawn from it.
    seen: HashMap<(u64, u32), HashSet<[u16; 4]>>,
    /// The sets tied to a texture.
    tied: HashMap<u64, Vec<u32>>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

/// Glyphs of a texture seen drawn before it counts as the set's page.
const TIE_AFTER: usize = 3;

fn with<T>(f: impl FnOnce(&mut State) -> T) -> T {
    f(STATE
        .lock()
        .expect("glyphs")
        .get_or_insert_with(State::default))
}

/// The game's font `id` has these glyphs (a set already known as such is kept).
pub fn register(id: u32, set: GlyphSet) {
    with(|s| {
        if s.sets.get(&id).is_some_and(|old| **old == set) {
            return;
        }
        s.sets.insert(id, Arc::new(set));
        s.by_rect.clear();
        for (&id, set) in &s.sets {
            for c in &set.cells {
                s.by_rect.entry(c.rect).or_default().push(id);
            }
        }
        // Its page is found again.
        s.seen.retain(|k, _| k.1 != id);
        for t in s.tied.values_mut() {
            t.retain(|&k| k != id);
        }
    });
}

/// Whether any set is known (else the renderer need not look at its quads).
pub fn any() -> bool {
    with(|s| !s.sets.is_empty())
}

/// A 2D quad showed exactly `rect` of texture `tex` (the renderer's id for a texture's contents:
/// its surface and version, so a page loaded anew is found again).
pub fn note(tex: u64, rect: [u16; 4]) {
    with(|s| {
        let Some(ids) = s.by_rect.get(&rect).cloned() else {
            return;
        };
        for id in ids {
            let seen = s.seen.entry((tex, id)).or_default();
            seen.insert(rect);
            if seen.len() >= TIE_AFTER {
                let tied = s.tied.entry(tex).or_default();
                if !tied.contains(&id) {
                    tied.push(id);
                }
            }
        }
    });
}

/// The sets tied to texture `tex`, and a number that changes with them (for caches).
pub fn tied(tex: u64) -> Option<(Vec<Arc<GlyphSet>>, u64)> {
    with(|s| {
        let ids = s.tied.get(&tex)?;
        let sets: Vec<_> = ids.iter().filter_map(|i| s.sets.get(i).cloned()).collect();
        let mut stamp = sets.len() as u64;
        for set in &sets {
            stamp = stamp.wrapping_mul(31).wrapping_add(Arc::as_ptr(set) as u64);
        }
        (!sets.is_empty()).then_some((sets, stamp))
    })
}

/// The page `base` (already enlarged `factor` times from the `width × height` texels `src`)
/// with the glyphs of `sets` drawn from their outline fonts.
pub fn refine(
    sets: &[Arc<GlyphSet>],
    src: &[Px],
    width: usize,
    height: usize,
    factor: usize,
    base: &mut [Px],
) {
    for set in sets {
        refine_cells(set, 0..set.cells.len(), src, width, height, factor, base);
    }
}

/// How many times a font's page is enlarged when the screen (`screen` pixels) is drawn at
/// `render` pixels: at least as many texels as pixels (1 to 8).
pub fn factor(screen: (u32, u32), render: (u32, u32)) -> usize {
    let s =
        (render.0 as f32 / screen.0.max(1) as f32).max(render.1 as f32 / screen.1.max(1) as f32);
    (s - 0.01).ceil().clamp(1.0, 8.0) as usize
}

/// Part of `refine`: the glyphs `cells` of one set; so the work can be spread.
pub fn refine_cells(
    set: &GlyphSet,
    cells: std::ops::Range<usize>,
    src: &[Px],
    width: usize,
    height: usize,
    factor: usize,
    base: &mut [Px],
) {
    let Ok(font) = ab_glyph::FontRef::try_from_slice(set.face) else {
        return;
    };
    // Transparent around the glyphs when the font's texels are (alpha or a colour key's zero)
    // anywhere: a narrow glyph's rectangle may be ink all over.
    let clear = set.cells.iter().any(|c| {
        let [x, y, w, h] = c.rect.map(usize::from);
        x + w <= width
            && y + h <= height
            && (y..y + h).any(|yy| (x..x + w).any(|xx| src[yy * width + xx][3] < 128))
    });
    if clear && cells.start == 0 {
        clear_rings(set, (width, height), factor, base);
    }
    for cell in set.cells.get(cells).unwrap_or_default() {
        draw_cell(&font, cell, clear, src, (width, height), factor, base);
    }
}

/// A texel around each glyph made transparent (before the glyphs are drawn, so one beside
/// another's rectangle is drawn again with it): the game's quads for its letters reach up to a
/// texel past their rectangles, which on the enlarged page showed what the page held there
/// (the next glyph's edge, smeared by the scaler) as a thin line beside each letter.
fn clear_rings(set: &GlyphSet, (width, height): (usize, usize), factor: usize, base: &mut [Px]) {
    let k = factor.max(1);
    let stride = k * width;
    for cell in &set.cells {
        let [x, y, w, h] = cell.rect.map(usize::from);
        if w == 0 || h == 0 || x + w > width || y + h > height {
            continue;
        }
        let (x1, y1) = ((x + w + 1).min(width), (y + h + 1).min(height));
        for ty in y.saturating_sub(1)..y1 {
            for tx in x.saturating_sub(1)..x1 {
                if (x..x + w).contains(&tx) && (y..y + h).contains(&ty) {
                    continue;
                }
                for py in 0..k {
                    let at = (k * ty + py) * stride + k * tx;
                    base[at..at + k].fill([0; 4]);
                }
            }
        }
    }
}

/// How a glyph is drawn: what surrounds it, its fill, and whether it has a dark rim (an outline,
/// or the edge smoothed against black as the game's fonts are).
struct Look {
    background: Px,
    fill: Px,
    rim: bool,
}

fn luma(p: Px) -> u32 {
    (299 * u32::from(p[0]) + 587 * u32::from(p[1]) + 114 * u32::from(p[2])) / 1000
}

fn mean(ps: &[Px]) -> Px {
    let n = ps.len().max(1) as u32;
    let mut s = [0u32; 4];
    for p in ps {
        for i in 0..4 {
            s[i] += u32::from(p[i]);
        }
    }
    s.map(|v| (v / n) as u8)
}

/// How the glyph in `rect` of `src` looks, or `None` for an empty one (a space).
fn look(src: &[Px], width: usize, rect: [usize; 4], clear: bool) -> Option<Look> {
    let [x0, y0, w, h] = rect;
    let texels: Vec<Px> = (0..h)
        .flat_map(|y| (0..w).map(move |x| (x, y)))
        .map(|(x, y)| src[(y0 + y) * width + x0 + x])
        .collect();
    // Around the glyph: transparent (`clear`), else the commonest colour on the rectangle's
    // border (an opaque key colour).
    let background = if clear {
        [0, 0, 0, 0]
    } else {
        let mut count: HashMap<Px, usize> = HashMap::new();
        for (i, &p) in texels.iter().enumerate() {
            let (x, y) = (i % w, i / w);
            if x == 0 || y == 0 || x == w - 1 || y == h - 1 {
                *count.entry(p).or_default() += 1;
            }
        }
        count.into_iter().max_by_key(|e| e.1)?.0
    };
    let ink: Vec<Px> = texels
        .iter()
        .copied()
        .filter(|&p| p[3] >= 128 && (background[3] < 128 || p != background))
        .collect();
    let top = ink.iter().map(|&p| luma(p)).max()?;
    if top == 0 {
        return None;
    }
    // The fill: the brightest ink; a rim when much of the ink is far darker.
    let bright: Vec<Px> = ink
        .iter()
        .copied()
        .filter(|&p| 4 * luma(p) >= 3 * top)
        .collect();
    let dark = ink.iter().filter(|&&p| 3 * luma(p) < top).count();
    Some(Look {
        background,
        fill: mean(&bright),
        rim: dark * 5 >= ink.len(),
    })
}

/// Coverage of `ch` from `font` fitted to `w × h` texels: its outline's bounds as tall as the
/// box (the game's glyphs are cut to their ink), as wide as the outline's own proportions give
/// and centred (squeezed only where that is wider than the box: stretched, a narrow letter as
/// `i` became a block with half-covered columns down both its sides), rasterized with smoothed
/// edges.
fn coverage(font: &ab_glyph::FontRef<'_>, ch: char, w: usize, h: usize) -> Option<Vec<f32>> {
    use ab_glyph::{point, Font, OutlineCurve, Point};
    let id = font.glyph_id(ch);
    if id.0 == 0 || w == 0 || h == 0 {
        return None;
    }
    let outline = font.outline(id)?;
    let b = outline.bounds;
    // Font units, y up.
    let (left, right) = (b.min.x.min(b.max.x), b.min.x.max(b.max.x));
    let (bottom, top) = (b.min.y.min(b.max.y), b.min.y.max(b.max.y));
    let sy = h as f32 / (top - bottom).max(1.0);
    let sx = (w as f32 / (right - left).max(1.0)).min(sy);
    let ox = (w as f32 - (right - left) * sx) / 2.0;
    let to = |p: &Point| point(ox + (p.x - left) * sx, (top - p.y) * sy);
    let mut r = ab_glyph_rasterizer::Rasterizer::new(w, h);
    for c in &outline.curves {
        match c {
            OutlineCurve::Line(a, b) => r.draw_line(to(a), to(b)),
            OutlineCurve::Quad(a, b, c) => r.draw_quad(to(a), to(b), to(c)),
            OutlineCurve::Cubic(a, b, c, d) => r.draw_cubic(to(a), to(b), to(c), to(d)),
        }
    }
    let mut out = vec![0f32; w * h];
    r.for_each_pixel(|i, c| out[i] = c.min(1.0));
    Some(out)
}

fn draw_cell(
    font: &ab_glyph::FontRef<'_>,
    cell: &Cell,
    clear: bool,
    src: &[Px],
    (width, height): (usize, usize),
    factor: usize,
    base: &mut [Px],
) {
    let [x, y, w, h] = cell.rect.map(usize::from);
    if w == 0 || h == 0 || x + w > width || y + h > height || cell.ch == ' ' {
        return;
    }
    let Some(look) = look(src, width, [x, y, w, h], clear) else {
        return;
    };
    // `factor` texels a source texel, a rim a quarter of a source texel wide (the renderer
    // keeps a 2D quad's pixels within the texels of the game's, `Draw::enlarged`, so the glyph
    // may fill its rectangle). Below four texels a source texel the rim would be a whole texel,
    // a third or half of the game's pixel, black and stepped around each letter: there the
    // glyph is only smoothed against black, as the game's own are.
    let k = factor.max(1);
    let (bw, bh) = (k * w, k * h);
    let ring = if look.rim { k / 4 } else { 0 };
    let edge = ring;
    let (iw, ih) = (bw.saturating_sub(2 * ring), bh.saturating_sub(2 * ring));
    let Some(cov) = coverage(font, cell.ch, iw, ih) else {
        return;
    };
    let at = |px: isize, py: isize| -> f32 {
        let (px, py) = (px - edge as isize, py - edge as isize);
        if px < 0 || py < 0 || px as usize >= iw || py as usize >= ih {
            return 0.0;
        }
        cov[py as usize * iw + px as usize]
    };
    let r = ring as isize;
    let stride = k * width;
    for py in 0..bh as isize {
        for px in 0..bw as isize {
            let c = at(px, py);
            // Opaque where the glyph, grown by the rim, covers the texel.
            let grown = (-r..=r)
                .flat_map(|dy| (-r..=r).map(move |dx| (dx, dy)))
                .filter(|(dx, dy)| dx * dx + dy * dy <= r * r)
                .map(|(dx, dy)| at(px + dx, py + dy))
                .fold(0.0f32, f32::max);
            let texel = if grown < 0.5 {
                look.background
            } else if look.rim {
                // Smoothed against black, as the game's own glyphs are.
                let f = |v: u8| (f32::from(v) * c).round() as u8;
                [f(look.fill[0]), f(look.fill[1]), f(look.fill[2]), 255]
            } else {
                look.fill
            };
            base[(k * y + py as usize) * stride + k * x + px as usize] = texel;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FACE: &[u8] = include_bytes!("../../assets/fonts/DejaVuSans.ttf");

    #[test]
    fn a_page_is_tied_after_three_glyphs_and_redrawn() {
        let cells = vec![
            Cell {
                rect: [0, 0, 6, 8],
                ch: 'A',
            },
            Cell {
                rect: [6, 0, 6, 8],
                ch: 'B',
            },
            Cell {
                rect: [12, 0, 6, 8],
                ch: 'C',
            },
        ];
        register(
            7,
            GlyphSet {
                cells: cells.clone(),
                face: FACE,
            },
        );
        note(100, cells[0].rect);
        note(100, cells[1].rect);
        assert!(tied(100).is_none());
        note(100, cells[2].rect);
        let (sets, _) = tied(100).expect("tied");
        // A page of 18 × 8: white blocks on the key's zero.
        let (w, h) = (18usize, 8usize);
        let mut src = vec![[0u8; 4]; w * h];
        for y in 1..7 {
            for x in 1..17 {
                if x % 6 != 0 {
                    src[y * w + x] = [255, 255, 255, 255];
                }
            }
        }
        let mut big = vec![[9u8; 4]; 16 * w * h];
        refine(&sets, &src, w, h, 4, &mut big);
        // Only the fill and the background inside the glyphs: no outline (none in the source).
        assert!(big
            .iter()
            .all(|&p| p == [0, 0, 0, 0] || p == [255, 255, 255, 255]));
        assert!(big.contains(&[255, 255, 255, 255]));
        assert!(tied(101).is_none());
        // At other sizes too: three times, and as small as the game's own.
        for k in [1, 3] {
            let mut page = vec![[9u8; 4]; k * k * w * h];
            refine(&sets, &src, w, h, k, &mut page);
            assert!(page.contains(&[255, 255, 255, 255]), "{k}");
        }
    }

    #[test]
    fn the_factor_follows_the_render_size() {
        assert_eq!(factor((640, 480), (640, 480)), 1);
        assert_eq!(factor((640, 480), (1280, 960)), 2);
        assert_eq!(factor((854, 480), (1920, 1080)), 3);
        assert_eq!(factor((640, 480), (8000, 6000)), 8);
    }
}
