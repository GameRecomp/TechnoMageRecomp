// SPDX-License-Identifier: MIT
//! The renderer in a browser: the batch goes to the player's WebGL2 renderer (web/gl.js) as
//! a list of 32-bit words, textures once per version by an id; WebGL reads the result back
//! synchronously, so the guest's surfaces stay in guest memory as natively.
//!
//! A command is `0` (a draw) or `1, depth` (clear the depth buffer). A draw:
//!
//! | words | what |
//! |---|---|
//! | 1 | vertex count `n` |
//! | 20 n | the vertices ([`Vertex`], f32) |
//! | 4 | texture 0 id (0: none), its flags (1 bilinear, 2 clamp u, 4 clamp v), texture 1 the same |
//! | 1 + 7 s | stage count `s`; each: colour op, arg 1, arg 2, alpha op, arg 1, arg 2, coordinate set |
//! | 4 | texture factor (f32) |
//! | 1 | flags: 1 specular added, 2 colour-keyed |
//! | 2 | alpha test function (0: off), reference (f32) |
//! | 2 | blend source and destination factors (0: off) |
//! | 2 | depth function (0: no test), write |
//! | 7 | fog mode (0 off, 1 vertex, 2 linear table), colour (4 f32), start, end (f32) |

use super::{Command, Draw, Texture, Vertex};
use std::collections::HashMap;

#[link(wasm_import_module = "rc_js")]
unsafe extern "C" {
    /// Whether the page can render with WebGL2 (1) or not (0).
    fn gl_init() -> i32;
    /// Texture `id`: `width * height` RGBA8 texels at `ptr`.
    fn gl_texture(id: u32, width: u32, height: u32, ptr: *const u8);
    fn gl_texture_drop(id: u32);
    /// Render the commands (`len` words at `cmd`) over the target's pixels at `rgba`
    /// (`width * height` RGBA8) and leave the result there.
    fn gl_render(cmd: *const u32, len: usize, rgba: *mut u8, width: u32, height: u32);
}

pub struct Renderer {
    textures: HashMap<(u32, u32, bool), (u32, u64)>,
    next: u32,
    frame: u64,
    words: Vec<u32>,
}

impl Renderer {
    /// The page's WebGL2 renderer, or `None` without one (`RC_RENDERER=soft` asks for none).
    pub fn new(soft: bool) -> Option<Renderer> {
        // SAFETY: a plain call.
        if soft || unsafe { gl_init() } != 1 {
            return None;
        }
        Some(Renderer {
            textures: HashMap::new(),
            next: 1,
            frame: 0,
            words: Vec::new(),
        })
    }

    fn texture(&mut self, t: &Texture) -> u32 {
        let frame = self.frame;
        if let Some(v) = self.textures.get_mut(&t.key) {
            v.1 = frame;
            return v.0;
        }
        if self.textures.len() > 2048 {
            // Drop what was not used for a while.
            self.textures.retain(|_, v| {
                let keep = frame.saturating_sub(v.1) < 600;
                if !keep {
                    // SAFETY: a plain call.
                    unsafe { gl_texture_drop(v.0) };
                }
                keep
            });
        }
        let id = self.next;
        self.next += 1;
        // SAFETY: the texels are valid for the call.
        unsafe {
            gl_texture(
                id,
                t.width.max(1),
                t.height.max(1),
                t.rgba.as_ptr().cast::<u8>(),
            )
        };
        self.textures.insert(t.key, (id, frame));
        id
    }

    fn draw(&mut self, d: &Draw) {
        let mut tex = [(0u32, 0u32); 2];
        for (k, t) in d.textures.iter().enumerate() {
            if let Some(t) = t {
                let flags = u32::from(t.bilinear)
                    | (u32::from(t.clamp.0) << 1)
                    | (u32::from(t.clamp.1) << 2);
                tex[k] = (self.texture(t), flags);
            }
        }
        let w = &mut self.words;
        let f = |v: f32| v.to_bits();
        w.push(0);
        w.push(d.vertices.len() as u32);
        w.extend_from_slice(bytemuck::cast_slice::<Vertex, u32>(&d.vertices));
        for (id, flags) in tex {
            w.push(id);
            w.push(flags);
        }
        w.push(d.stages.len().min(2) as u32);
        for s in d.stages.iter().take(2) {
            w.extend_from_slice(&[s.cop, s.ca1, s.ca2, s.aop, s.aa1, s.aa2, s.coords]);
        }
        w.extend(d.factor.iter().map(|&v| f(v)));
        w.push(u32::from(d.specular_add) | (u32::from(d.keyed) << 1));
        let (af, ar) = d.alpha_test.unwrap_or((0, 0.0));
        w.extend_from_slice(&[af, f(ar)]);
        let (bs, bd) = d.blend.unwrap_or((0, 0));
        w.extend_from_slice(&[bs, bd]);
        let (df, dw) = d.depth.unwrap_or((0, false));
        w.extend_from_slice(&[df, u32::from(dw)]);
        match d.fog {
            None => w.extend_from_slice(&[0; 7]),
            Some((c, table)) => {
                w.push(if table.is_some() { 2 } else { 1 });
                w.extend(c.iter().map(|&v| f(v)));
                let (s, e) = table.unwrap_or((0.0, 1.0));
                w.extend_from_slice(&[f(s), f(e)]);
            }
        }
    }

    /// Render `commands` over the target's pixels `rgba` (width × height RGBA8) and return the
    /// result.
    pub fn render(
        &mut self,
        width: u32,
        height: u32,
        rgba: &[u8],
        commands: &[Command],
    ) -> Vec<u8> {
        self.frame += 1;
        self.words.clear();
        for c in commands {
            match c {
                Command::Draw(d) => self.draw(d),
                Command::ClearDepth(z) => {
                    self.words.push(1);
                    self.words.push(z.to_bits());
                }
            }
        }
        let mut out = rgba.to_vec();
        // SAFETY: the words and the pixels are valid for the call; the page writes
        // `width * height * 4` bytes into `out`, which holds that many.
        unsafe {
            gl_render(
                self.words.as_ptr(),
                self.words.len(),
                out.as_mut_ptr(),
                width,
                height,
            )
        };
        out
    }
}
