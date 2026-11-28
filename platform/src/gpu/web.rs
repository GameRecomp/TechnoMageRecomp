// SPDX-License-Identifier: MIT
//! The renderer in a browser: the batch goes to the player's WebGL2 renderer (web/gl.js) as
//! a list of 32-bit words, textures once per version by an id. As natively the render target
//! stays on the GPU (`draw_resident`): the passes that make it what guest memory would hold, the
//! frame shown with its overlays (drawn on the player's WebGL canvas, which the page shows) and
//! the read-backs when the game touches the surface are web/gl.js's, the same as the native
//! WGSL ones (`resident.rs`). `render` (`RC_GPU_RESIDENT=0`) draws over the pixels uploaded and
//! reads the result back.
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
//! | 1 | flags: 1 specular added, 2 colour-keyed, 4 pixel-exact; above bit 8 how much larger texture 0 is (`Draw::enlarged`) when the draw is pixel-exact, else 0 |
//! | 2 | alpha test function (0: off), reference (f32) |
//! | 2 | blend source and destination factors (0: off) |
//! | 2 | depth function (0: no test), write |
//! | 7 | fog mode (0 off, 1 vertex, 2 linear table), colour (4 f32), start, end (f32) |
//! | 1 + 20 m | rim vertex count `m` and the rims ([`super::rims`], opaque 3D: drawn after the draw with its depth pushed back) |

use super::guest::{overlay_uniforms, strip, uniforms};
use super::{Command, Draw, GpuPicture, Guest, OverlayImage, Texture, Vertex};
use std::collections::HashMap;

#[link(wasm_import_module = "rc_js")]
unsafe extern "C" {
    /// Whether the page can render with WebGL2 (1) or not (0).
    fn gl_init() -> i32;
    /// WebGL's largest texture side.
    fn gl_max_texture() -> u32;
    /// Texture `id`: `width * height` RGBA8 texels at `ptr`.
    fn gl_texture(id: u32, width: u32, height: u32, ptr: *const u8);
    fn gl_texture_drop(id: u32);
    /// Render the commands (`len` words at `cmd`) over the target's pixels at `rgba`
    /// (`width * height` RGBA8) and leave the result there.
    fn gl_render(cmd: *const u32, len: usize, rgba: *mut u8, width: u32, height: u32);
    /// Render the commands on the target kept (`width × height`; the positions in the game's
    /// screen, `view_w × view_h`), over the pixels at `rgba` if it is not null; nothing is read
    /// back.
    fn gl_draw(
        cmd: *const u32,
        len: usize,
        rgba: *const u8,
        width: u32,
        height: u32,
        view_w: u32,
        view_h: u32,
    );
    /// Pass `pass` (0 edges, 1 quantize, 2 margins, 3 reduce) over the target in place, with
    /// the 16 uniforms at `u`.
    fn gl_pass(pass: u32, u: *const u32);
    /// The target (or the picture kept of the front, `front` 1) at the guest's size
    /// (`width × height`, uniforms at `u`) into `out`.
    fn gl_read_guest(u: *const u32, front: u32, out: *mut u8, width: u32, height: u32);
    /// The target whole into `out`.
    fn gl_read_target(out: *mut u8);
    /// The target kept as the picture of the front surface.
    fn gl_keep_front();
    /// The frame shown made from the target (uniforms at `u`; the gamma ramp texture `lut`, 0
    /// for none) with `n` overlays (each its texture id and 16 uniforms at `list`), drawn on the
    /// player's canvas.
    fn gl_present(u: *const u32, lut: u32, list: *const u32, n: u32);
    /// The frame last shown into `out` (`width × height` RGBA8).
    fn gl_read_present(out: *mut u8, width: u32, height: u32);
    /// Wait for the GPU.
    fn gl_wait();
}

/// The frame last shown on the player's canvas into `width × height` RGBA8 (screenshots).
pub fn read_present(width: u32, height: u32) -> Vec<u8> {
    let mut out = vec![0u8; (4 * width * height) as usize];
    // SAFETY: `out` holds `width * height * 4` bytes.
    unsafe { gl_read_present(out.as_mut_ptr(), width, height) };
    out
}

pub struct Renderer {
    textures: HashMap<(u32, u32, u32), (u32, u64)>,
    next: u32,
    frame: u64,
    words: Vec<u32>,
    /// The size of the target kept, and of the front surface's picture.
    target: Option<(u32, u32)>,
    front: Option<(u32, u32)>,
    /// Overlay pictures by key (texture id, the frame shown it was last used in), the frames
    /// shown so far, the gamma ramp's texture and the ramp it holds.
    images: HashMap<u64, (u32, u64)>,
    shown: u64,
    lut: Option<(u32, Vec<u16>)>,
}

impl Renderer {
    /// The page's WebGL2 renderer, or `None` without one (`RC_RENDERER=soft` asks for none).
    pub fn new(soft: bool) -> Option<Renderer> {
        // SAFETY: a plain call.
        if soft || unsafe { gl_init() } != 1 {
            return None;
        }
        // SAFETY: a plain call.
        crate::render::set_max_texture(unsafe { gl_max_texture() });
        Some(Renderer {
            textures: HashMap::new(),
            next: 1,
            frame: 0,
            words: Vec::new(),
            target: None,
            front: None,
            images: HashMap::new(),
            shown: 0,
            lut: None,
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
        let enlarged = if d.pixel_exact && d.enlarged > 1 {
            d.enlarged
        } else {
            0
        };
        w.push(
            u32::from(d.specular_add)
                | (u32::from(d.keyed) << 1)
                | (u32::from(d.pixel_exact) << 2)
                | enlarged << 8,
        );
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
        // Opaque 3D gets rims that close the gaps between meshes, as natively.
        let rims = if d.blend.is_none() && d.depth.is_some_and(|(_, write)| write) {
            super::rims(&d.vertices, 0.5)
        } else {
            Vec::new()
        };
        w.push(rims.len() as u32);
        w.extend_from_slice(bytemuck::cast_slice::<Vertex, u32>(&rims));
    }

    /// The commands as words.
    fn encode(&mut self, commands: &[Command]) {
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
    }

    /// A new texture's id, its texels uploaded.
    fn upload(&mut self, width: u32, height: u32, rgba: &[u8]) -> u32 {
        let id = self.next;
        self.next += 1;
        // SAFETY: the texels (`width * height * 4` bytes) are valid for the call.
        unsafe { gl_texture(id, width, height, rgba.as_ptr()) };
        id
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
        self.encode(commands);
        // What is drawn here is not kept.
        self.target = None;
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

    /// Whether the target is `size` (it holds a picture to draw on).
    pub fn target_is(&self, size: (u32, u32)) -> bool {
        self.target == Some(size)
    }

    fn pass(&mut self, pass: u32, u: [u32; 16]) {
        // SAFETY: the uniforms are valid for the call.
        unsafe { gl_pass(pass, u.as_ptr()) }
    }

    /// Draw `commands` on the target (uploading `background` first if given) and make it what
    /// guest memory would hold (as natively). Nothing is read back.
    pub fn draw_resident(
        &mut self,
        size: (u32, u32),
        view: (u32, u32),
        guest: &Guest,
        background: Option<&[u8]>,
        commands: &[Command],
    ) {
        self.encode(commands);
        let bg = background.map_or(std::ptr::null(), <[u8]>::as_ptr);
        // SAFETY: the words and the pixels (`size` RGBA8 if given) are valid for the call.
        unsafe {
            gl_draw(
                self.words.as_ptr(),
                self.words.len(),
                bg,
                size.0,
                size.1,
                view.0,
                view.1,
            )
        };
        self.target = Some(size);
        if size == (guest.width, guest.height) {
            if guest.bits.iter().any(|&b| b != 8 && b != 0) {
                self.pass(1, uniforms(size, guest, 0, (0, 0)));
            }
        } else {
            let st = (strip(size.0, view.0), strip(size.1, view.1));
            if st != (0, 0) {
                self.pass(0, uniforms(size, guest, 0, st));
            }
        }
    }

    /// After a flip: the margins black, as the HLE clears them in guest memory.
    pub fn clear_margins(&mut self, guest: &Guest) {
        if let Some(size) = self.target.filter(|_| guest.margin != 0) {
            self.pass(2, uniforms(size, guest, 0, (0, 0)));
        }
    }

    /// Drawn larger, before a new frame's first draws: the target as guest memory holds it.
    pub fn reduce(&mut self, guest: &Guest) {
        if let Some(size) = self.target {
            self.pass(3, uniforms(size, guest, 0, (0, 0)));
        }
    }

    /// Wait until the GPU has done all that was handed to it (for measuring).
    pub fn wait(&self) {
        // SAFETY: a plain call.
        unsafe { gl_wait() }
    }

    /// The target read back whole (RGBA8 at its size).
    pub fn read_target(&mut self) -> Option<Vec<u8>> {
        let (w, h) = self.target?;
        let mut out = vec![0u8; (4 * w * h) as usize];
        // SAFETY: `out` holds the target's `w * h * 4` bytes.
        unsafe { gl_read_target(out.as_mut_ptr()) };
        Some(out)
    }

    /// What guest memory holds of the target (or of the picture flipped to the front, `front`):
    /// RGBA8 at the guest's size.
    pub fn read_guest(&mut self, guest: &Guest, front: bool) -> Option<Vec<u8>> {
        let size = if front { self.front? } else { self.target? };
        let u = uniforms(size, guest, 0, (0, 0));
        let mut out = vec![0u8; (4 * guest.width * guest.height) as usize];
        // SAFETY: the uniforms and `out` (the guest's `width * height * 4` bytes) are valid.
        unsafe {
            gl_read_guest(
                u.as_ptr(),
                u32::from(front),
                out.as_mut_ptr(),
                guest.width,
                guest.height,
            )
        };
        Some(out)
    }

    /// At a flip: the target kept as the picture of the front surface (read on demand).
    pub fn keep_front(&mut self) {
        if let Some(size) = self.target {
            // SAFETY: a plain call.
            unsafe { gl_keep_front() };
            self.front = Some(size);
        }
    }

    /// The frame shown: the target as the window shows it (through `gamma`, a 16-bit surface
    /// expanded as the HLE does) with the `overlays` over it, drawn on the player's canvas.
    pub fn present(
        &mut self,
        guest: &Guest,
        gamma: Option<&[u16]>,
        overlays: &[OverlayImage],
        game: (u32, u32),
    ) -> Option<GpuPicture> {
        let size = self.target?;
        self.shown += 1;
        let shown = self.shown;
        if self.lut.as_ref().map(|l| l.1.as_slice()) != gamma {
            if let Some((id, _)) = self.lut.take() {
                // SAFETY: a plain call.
                unsafe { gl_texture_drop(id) };
            }
            if let Some(g) = gamma {
                let mut bytes = vec![255u8; 256 * 4];
                for v in 0..256 {
                    for c in 0..3 {
                        bytes[4 * v + c] = (g[c * 256 + v] >> 8) as u8;
                    }
                }
                let id = self.upload(256, 1, &bytes);
                self.lut = Some((id, g.to_vec()));
            }
        }
        let flags = if gamma.is_some() { 4 } else { 0 } | if guest.rgb565() { 8 } else { 0 };
        let u = uniforms(size, guest, flags, (0, 0));
        let mut list = Vec::with_capacity(17 * overlays.len());
        for o in overlays {
            let id = match self.images.get_mut(&o.key) {
                Some(e) => {
                    e.1 = shown;
                    e.0
                }
                None => {
                    let id = self.upload(o.width.max(1), o.height.max(1), &o.premul);
                    self.images.insert(o.key, (id, shown));
                    id
                }
            };
            list.push(id);
            list.extend_from_slice(&overlay_uniforms(size, o, u[7]));
        }
        self.images.retain(|_, e| {
            let keep = shown - e.1 < 120;
            if !keep {
                // SAFETY: a plain call.
                unsafe { gl_texture_drop(e.0) };
            }
            keep
        });
        let lut = self.lut.as_ref().map_or(0, |l| l.0);
        // SAFETY: the uniforms and the list (17 words an overlay) are valid for the call.
        unsafe { gl_present(u.as_ptr(), lut, list.as_ptr(), overlays.len() as u32) };
        Some(GpuPicture {
            width: size.0,
            height: size.1,
            game,
        })
    }
}
