// SPDX-License-Identifier: MIT
//! Pictures over the frame (on-screen buttons, gamepad buttons, the frame rate): made once on
//! the CPU, laid over the frame on the GPU or, for a frame made on the CPU, here.

use std::sync::Arc;

/// A picture over the frame (frame pixels): premultiplied RGBA8, alpha its coverage, made once
/// per `key`. A gradient picture covers its region with the colours left and right of it first
/// (the region x0, x1, y0, y1 and the columns left and right), as a gamepad button over the
/// game's key picture.
#[derive(Debug, Clone)]
pub struct OverlayImage {
    pub key: u64,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub premul: Arc<Vec<u8>>,
    pub gradient: Option<[u32; 6]>,
}

impl OverlayImage {
    /// A picture of `width` × `height` at (`x`, `y`) made by `draw`, which paints over a frame
    /// (its RGBA8, width, height) as it would over the game's: it paints twice, over black and
    /// over white, and the difference is its coverage. Made once per `key` (kept a while).
    pub fn make(
        key: u64,
        (x, y): (i32, i32),
        (width, height): (u32, u32),
        gradient: Option<[u32; 6]>,
        draw: impl Fn(&mut [u8], u32, u32),
    ) -> OverlayImage {
        use std::collections::HashMap;
        use std::sync::Mutex;
        static MADE: Mutex<Option<HashMap<u64, Arc<Vec<u8>>>>> = Mutex::new(None);
        let premul = {
            let mut made = MADE.lock().expect("overlay pictures");
            let map = made.get_or_insert_with(HashMap::new);
            if map.len() > 4096 {
                map.clear();
            }
            map.entry(key)
                .or_insert_with(|| {
                    let n = (width * height) as usize;
                    let mut black = vec![0u8; 4 * n];
                    let mut white = vec![255u8; 4 * n];
                    for p in black.as_chunks_mut::<4>().0 {
                        p[3] = 255;
                    }
                    draw(&mut black, width, height);
                    draw(&mut white, width, height);
                    let mut out = vec![0u8; 4 * n];
                    for ((o, b), w) in out
                        .as_chunks_mut::<4>()
                        .0
                        .iter_mut()
                        .zip(black.as_chunks::<4>().0)
                        .zip(white.as_chunks::<4>().0)
                    {
                        // Over black the colour times its coverage; over white that plus the
                        // white left showing.
                        let shows: u32 = (0..3)
                            .map(|i| u32::from(w[i]).saturating_sub(u32::from(b[i])))
                            .sum();
                        let a = 255 - (shows / 3).min(255);
                        *o = [b[0], b[1], b[2], a as u8];
                    }
                    Arc::new(out)
                })
                .clone()
        };
        OverlayImage {
            key,
            x,
            y,
            width,
            height,
            premul,
            gradient,
        }
    }

    /// Over an RGBA8 frame on the CPU (frames that are not the GPU's: videos, software).
    pub fn apply(&self, frame: &mut [u8], fw: u32, fh: u32) {
        for py in 0..self.height as i32 {
            let fy = self.y + py;
            if fy < 0 || fy >= fh as i32 {
                continue;
            }
            for px in 0..self.width as i32 {
                let fx = self.x + px;
                if fx < 0 || fx >= fw as i32 {
                    continue;
                }
                let at = 4 * (fy as usize * fw as usize + fx as usize);
                let mut base = [frame[at], frame[at + 1], frame[at + 2]].map(f32::from);
                if let Some([x0, x1, y0, y1, l, r]) = self.gradient {
                    let (ux, uy) = (fx as u32, fy as u32);
                    if ux >= x0 && ux < x1 && uy >= y0 && uy < y1 {
                        let t = (ux - x0) as f32 / (x1 - x0).max(1) as f32;
                        let row = 4 * fy as usize * fw as usize;
                        for (i, v) in base.iter_mut().enumerate() {
                            let a = f32::from(frame[row + 4 * l as usize + i]);
                            let b = f32::from(frame[row + 4 * r as usize + i]);
                            *v = a * (1.0 - t) + b * t;
                        }
                    }
                }
                let o = 4 * (py as usize * self.width as usize + px as usize);
                let a = f32::from(self.premul[o + 3]) / 255.0;
                for (i, b) in base.iter().enumerate() {
                    let v = b * (1.0 - a) + f32::from(self.premul[o + i]);
                    frame[at + i] = v.round().clamp(0.0, 255.0) as u8;
                }
            }
        }
    }
}
