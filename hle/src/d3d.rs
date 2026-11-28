// SPDX-License-Identifier: MIT
//! d3d: Direct3D 7 (immediate mode), reached through `IDirectDraw7::QueryInterface`.
//!
//! This layer keeps the device state the game sets (render states, texture stage states,
//! transforms, viewport, material, lights, textures) and records each draw with a copy of its
//! vertices and indices and the state it was made in, into the frame's command list
//! ([`take_frame`]). The platform's renderer replays that list with wgpu (docs/PLAN.md,
//! "Platform: Bevy"). `Clear` is done here, on the render target's and z-buffer's pixels in guest
//! memory, so 2D work on the same surfaces stays correct without the renderer.

#![allow(non_snake_case)]

use crate::com::{self, Interface, E_NOINTERFACE, S_OK};
use crate::ddraw;
use rc_runtime::cpu::Cpu;
use rc_runtime::dispatch::call_guest;
use rc_runtime::imports::ret_stdcall;
use rc_runtime::mem::Mem;
use rc_runtime::vm;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

const D3DERR_INVALIDCALL: u32 = 0x8876_086c;

const IID_RGB: [u8; 16] = com::guid(
    0xa466_5c60,
    0x2673,
    0x11cf,
    [0xa3, 0x1a, 0x00, 0xaa, 0x00, 0xb9, 0x33, 0x56],
);
const IID_HAL: [u8; 16] = com::guid(
    0x84e6_3de0,
    0x46aa,
    0x11cf,
    [0x81, 0x6f, 0x00, 0x00, 0xc0, 0x20, 0x15, 0x6e],
);
const IID_TNLHAL: [u8; 16] = com::guid(
    0xf504_9e78,
    0x4861,
    0x11d2,
    [0xa4, 0x07, 0x00, 0xa0, 0xc9, 0x06, 0x29, 0xa8],
);

/// A 4x4 matrix as Direct3D stores it (row-major, row vectors).
pub type Matrix = [f32; 16];

const IDENTITY: Matrix = [
    1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
];

/// The fixed-function state a draw is made in.
#[derive(Debug, Clone)]
pub struct State {
    pub render: BTreeMap<u32, u32>,
    /// Texture stage states, `[stage][type]`.
    pub stages: [BTreeMap<u32, u32>; 8],
    /// The textures bound to the stages: the surface object and its version.
    pub textures: [Option<(u32, u32)>; 8],
    pub world: Matrix,
    pub view: Matrix,
    pub projection: Matrix,
    /// x, y, width, height, min z, max z.
    pub viewport: [f32; 6],
    /// Diffuse, ambient, specular, emissive (RGBA each), power.
    pub material: [f32; 17],
    /// Light index → (the 104-byte D3DLIGHT7, enabled).
    pub lights: BTreeMap<u32, ([u8; 104], bool)>,
    /// The render target surface object.
    pub target: u32,
}

impl Default for State {
    fn default() -> Self {
        State {
            render: BTreeMap::new(),
            stages: Default::default(),
            textures: [None; 8],
            world: IDENTITY,
            view: IDENTITY,
            projection: IDENTITY,
            viewport: [0.0, 0.0, 640.0, 480.0, 0.0, 1.0],
            material: [0.0; 17],
            lights: BTreeMap::new(),
            target: 0,
        }
    }
}

/// One recorded command of a frame.
#[derive(Debug, Clone)]
pub enum Command {
    Clear {
        rects: Vec<[i32; 4]>,
        flags: u32,
        color: u32,
        z: f32,
    },
    Draw {
        primitive: u32,
        fvf: u32,
        vertices: Vec<u8>,
        indices: Vec<u16>,
        state: Arc<State>,
    },
}

struct Device {
    state: State,
    /// The state the last draw saw, shared by the draws until it changes.
    snapshot: Option<Arc<State>>,
}

struct VertexBuffer {
    buf: u32,
    fvf: u32,
    count: u32,
}

struct Global {
    devices: BTreeMap<u32, Device>,
    buffers: BTreeMap<u32, VertexBuffer>,
    next: u32,
    frame: Vec<Command>,
    /// Draws taken aside instead of drawn (see [`capture`]).
    captured: Option<Vec<Draw>>,
}

static G: Mutex<Global> = Mutex::new(Global {
    devices: BTreeMap::new(),
    buffers: BTreeMap::new(),
    next: 1,
    frame: Vec::new(),
    captured: None,
});

/// A draw as recorded: the primitive, its vertices and indices, and the state it was made in.
#[derive(Debug, Clone)]
pub struct Draw {
    pub primitive: u32,
    pub fvf: u32,
    pub vertices: Vec<u8>,
    pub indices: Vec<u16>,
    pub state: Arc<State>,
}

/// Run `f` with the draws it makes taken aside instead of drawn into their target, and return
/// them (for the project's patches: the game's mouse pointer becomes a hardware cursor).
pub fn capture(f: impl FnOnce()) -> Vec<Draw> {
    g().captured = Some(Vec::new());
    f();
    g().captured.take().unwrap_or_default()
}

fn g() -> std::sync::MutexGuard<'static, Global> {
    G.lock().expect("d3d")
}

fn next_id() -> u32 {
    let mut s = g();
    let id = s.next;
    s.next += 1;
    id
}

/// The GPU path: draws of one render target collected until the CPU needs its pixels.
#[cfg(feature = "gpu")]
mod batch {
    use rc_platform::gpu::{Command, Renderer};
    use rc_runtime::mem::Mem;
    use std::sync::Mutex;

    static BATCH: Mutex<(u32, Vec<Command>)> = Mutex::new((0, Vec::new()));
    static RENDERER: Mutex<Option<Option<Renderer>>> = Mutex::new(None);

    /// Whether a GPU renders (created on first use).
    pub fn available() -> bool {
        let mut r = RENDERER.lock().expect("renderer");
        if r.is_none() {
            let made =
                Renderer::new(rc_runtime::host::env("RC_RENDERER").is_some_and(|v| v == "soft"));
            rc_runtime::log!(
                "[d3d] renderer: {}",
                if made.is_some() { "GPU" } else { "software" }
            );
            *r = Some(made);
        }
        r.as_ref().is_some_and(Option::is_some)
    }

    pub fn push(m: Mem, target: u32, c: Command) {
        let other = {
            let b = BATCH.lock().expect("batch");
            b.0 != target && !b.1.is_empty()
        };
        if other {
            flush(m);
        }
        let mut b = BATCH.lock().expect("batch");
        b.0 = target;
        b.1.push(c);
    }

    /// Render what was collected into the target's pixels in guest memory.
    pub fn flush(m: Mem) {
        let (target, commands) = {
            let mut b = BATCH.lock().expect("batch");
            if !b.1.iter().any(|c| matches!(c, Command::Draw(_))) {
                // Only depth clears: they wait for the next draw.
                return;
            }
            (b.0, std::mem::take(&mut b.1))
        };
        let Some(info) = crate::ddraw::info(m, target) else {
            return;
        };
        use crate::profile::{time, Section as S};
        let (w, h) = super::shadow::render_size(&info);
        if resident_on() {
            draw_resident(m, target, &info, (w, h), &commands);
            return;
        }
        let mut r = RENDERER.lock().expect("renderer");
        let Some(Some(r)) = r.as_mut() else {
            return;
        };
        #[cfg(not(target_family = "wasm"))]
        if (w, h) != (info.width, info.height) {
            let background = time(S::FlushRead, || super::shadow::compose(m, &info, (w, h)));
            let out = time(S::FlushRender, || {
                r.render_view(w, h, (info.width, info.height), &background, &commands)
            });
            split_render();
            time(S::FlushWrite, || {
                super::shadow::store(m, &info, (w, h), out)
            });
            return;
        }
        super::shadow::drop(info.buf);
        let rgba = time(S::FlushRead, || crate::raster::to_rgba8(m, &info));
        let out = time(S::FlushRender, || {
            r.render(info.width, info.height, &rgba, &commands)
        });
        split_render();
        time(S::FlushWrite, || crate::raster::from_rgba8(m, &info, &out));
    }

    /// Flush if `obj` is the surface the collected draws go to.
    /// The render's stages out of its section (`rc_platform::gpu::LAST_RENDER`).
    fn split_render() {
        use crate::profile::{shift, Section as S};
        let t = *rc_platform::gpu::LAST_RENDER.lock().expect("render times");
        shift(S::FlushRender, S::GpuPrepare, t[0]);
        shift(S::FlushRender, S::GpuWait, t[1]);
        shift(S::FlushRender, S::GpuUnpack, t[2] + t[3]);
    }

    pub fn flush_if(m: Mem, obj: u32) {
        let pending = {
            let b = BATCH.lock().expect("batch");
            b.0 == obj && !b.1.is_empty()
        };
        if pending {
            flush(m);
        }
        to_guest(m, obj);
    }

    // --- The target kept on the GPU -------------------------------------------------------

    /// A render target whose pixels are on the GPU (`rc_platform::gpu::Renderer`'s target), not
    /// yet in guest memory: its object and buffer, its render size, and with native resolution
    /// whether the frame drawn is still being drawn (`fresh`: not yet shown).
    struct Resident {
        obj: u32,
        buf: u32,
        size: (u32, u32),
        guest: rc_platform::gpu::Guest,
        fresh: bool,
    }

    /// The front surface's picture as flipped, kept on the GPU until the game reads it.
    struct Front {
        obj: u32,
        guest: rc_platform::gpu::Guest,
    }

    static RESIDENT: Mutex<Option<Resident>> = Mutex::new(None);
    static FRONT: Mutex<Option<Front>> = Mutex::new(None);

    /// Whether render targets stay on the GPU (`RC_GPU_RESIDENT=0` reads every batch back, as
    /// before, to compare).
    fn resident_on() -> bool {
        static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *ON.get_or_init(|| rc_runtime::host::env("RC_GPU_RESIDENT").is_none_or(|v| v != "0"))
    }

    fn guest_of(m: Mem, obj: u32, info: &crate::ddraw::SurfaceInfo) -> rc_platform::gpu::Guest {
        rc_platform::gpu::Guest {
            width: info.width,
            height: info.height,
            bits: info.masks.map(u32::count_ones),
            bpp: info.bpp,
            margin: crate::ddraw::margin_of(m, obj),
        }
    }

    /// Draw a batch on the target on the GPU: over what it holds when it holds this surface
    /// (with native resolution, a new frame first brought to what guest memory would hold),
    /// else over the surface's pixels uploaded.
    fn draw_resident(
        m: Mem,
        target: u32,
        info: &crate::ddraw::SurfaceInfo,
        size: (u32, u32),
        commands: &[Command],
    ) {
        use crate::profile::{time, Section as S};
        let guest = guest_of(m, target, info);
        let same = RESIDENT
            .lock()
            .expect("resident")
            .as_ref()
            .map(|r| r.buf == info.buf && r.size == size && r.guest == guest);
        if same == Some(false) {
            // Another surface's pixels are on the GPU: into guest memory first.
            let obj = RESIDENT
                .lock()
                .expect("resident")
                .as_ref()
                .map_or(0, |r| r.obj);
            to_guest(m, obj);
        }
        let scaled = size != (info.width, info.height);
        let mut guard = RENDERER.lock().expect("renderer");
        let Some(Some(r)) = guard.as_mut() else {
            return;
        };
        let kept = same == Some(true) && r.target_is(size);
        let background = if kept {
            None
        } else if scaled {
            Some(time(S::FlushRead, || super::shadow::compose(m, info, size)))
        } else {
            super::shadow::drop(info.buf);
            Some(time(S::FlushRead, || crate::raster::to_rgba8(m, info)))
        };
        if kept && scaled {
            let fresh = RESIDENT
                .lock()
                .expect("resident")
                .as_ref()
                .is_some_and(|r| r.fresh);
            if !fresh {
                time(S::FlushRender, || r.reduce(&guest));
            }
        }
        time(S::FlushRender, || {
            r.draw_resident(
                size,
                (info.width, info.height),
                &guest,
                background.as_deref(),
                commands,
            )
        });
        split_render();
        *RESIDENT.lock().expect("resident") = Some(Resident {
            obj: target,
            buf: info.buf,
            size,
            guest,
            fresh: true,
        });
    }

    /// The surface `obj`'s pixels into guest memory, if they are on the GPU (the CPU is about
    /// to read or write them): as the batch's read-back would have written them, with native
    /// resolution the large picture kept as its shadow.
    fn to_guest(m: Mem, obj: u32) {
        use crate::profile::{time, Section as S};
        let front = {
            let mut f = FRONT.lock().expect("front");
            if f.as_ref().is_some_and(|f| f.obj == obj) {
                f.take()
            } else {
                None
            }
        };
        if let Some(f) = front {
            let Some(info) = crate::ddraw::info(m, f.obj) else {
                return;
            };
            let mut guard = RENDERER.lock().expect("renderer");
            if let Some(Some(r)) = guard.as_mut() {
                if let Some(low) = time(S::FlushWrite, || r.read_guest(&f.guest, true)) {
                    crate::raster::from_rgba8(m, &info, &low);
                }
            }
            return;
        }
        let res = {
            let mut res = RESIDENT.lock().expect("resident");
            if res.as_ref().is_some_and(|r| r.obj == obj) {
                res.take()
            } else {
                None
            }
        };
        let Some(res) = res else {
            return;
        };
        let Some(info) = crate::ddraw::info(m, res.obj) else {
            return;
        };
        if info.buf != res.buf {
            return;
        }
        let mut guard = RENDERER.lock().expect("renderer");
        let Some(Some(r)) = guard.as_mut() else {
            return;
        };
        let Some(low) = time(S::FlushWrite, || r.read_guest(&res.guest, false)) else {
            return;
        };
        crate::raster::from_rgba8(m, &info, &low);
        if res.size != (info.width, info.height) {
            if let Some(big) = time(S::FlushWrite, || r.read_target()) {
                super::shadow::keep(m, &info, res.size, big, res.fresh);
            }
        }
    }

    /// A flip of `back` (on the GPU) to `front`: the frame shown made on the GPU with the
    /// pictures over it, the front's picture kept for the game to read, the margins black.
    /// `None` when the back surface's pixels are not on the GPU (shown as before).
    pub fn flip(
        m: Mem,
        back: u32,
        front: Option<u32>,
        overlays: impl FnOnce(u32, u32) -> Vec<rc_platform::overlay::OverlayImage>,
    ) -> bool {
        let pending = {
            let b = BATCH.lock().expect("batch");
            b.0 == back && !b.1.is_empty()
        };
        if pending {
            flush(m);
        }
        let mut res = RESIDENT.lock().expect("resident");
        let Some(state) = res.as_mut().filter(|r| r.obj == back) else {
            return false;
        };
        let mut guard = RENDERER.lock().expect("renderer");
        let Some(Some(r)) = guard.as_mut() else {
            return false;
        };
        if !r.target_is(state.size) {
            return false;
        }
        use crate::profile::{time, Section as S};
        if let Some(f) = front {
            r.keep_front();
            *FRONT.lock().expect("front") = Some(Front {
                obj: f,
                guest: state.guest,
            });
        }
        let (w, h) = state.size;
        let mut list = time(S::Overlay, || overlays(w, h));
        list.extend(rc_platform::fps::overlay_image(w, h));
        let gamma = crate::small::gamma_ramp();
        let game = (state.guest.width, state.guest.height);
        let Some(pic) = time(S::Picture, || {
            r.present(&state.guest, gamma.as_deref(), &list, game)
        }) else {
            return false;
        };
        r.clear_margins(&state.guest);
        state.fresh = false;
        if rc_runtime::host::env("RC_GPU_SYNC").is_some() {
            // Measured: the GPU's work of the frame waited for (else the next frame waits for it,
            // the CPU's work meanwhile overlapping).
            time(S::GpuWait, || r.wait());
        }
        drop(guard);
        drop(res);
        time(S::Submit, || rc_platform::submit_gpu_frame(pic));
        true
    }

    /// A surface's pixels moved or went away: what the GPU held of them is forgotten.
    pub fn forget(buf: u32) {
        let mut res = RESIDENT.lock().expect("resident");
        if res.as_ref().is_some_and(|r| r.buf == buf) {
            *res = None;
        }
    }
}

/// Native resolution (`rc_platform::render`): a render target the size of the screen is drawn
/// larger on the GPU. The large picture is kept as the surface's shadow; guest memory gets it
/// scaled down (what the game reads), and a copy of what was written. A pixel of the surface
/// still as written shows the shadow's pixels; one the game has written since (2D) shows its
/// own, enlarged. A colour clear drops the shadow.
#[cfg(feature = "gpu")]
#[cfg_attr(target_family = "wasm", allow(dead_code))]
mod shadow {
    use crate::ddraw::SurfaceInfo;
    use rc_runtime::mem::Mem;
    use std::collections::HashMap;
    use std::sync::Mutex;

    struct Shadow {
        size: (u32, u32),
        rgba: Vec<u8>,
        /// Drawn since the surface was last shown: the next draws of the frame build on it.
        /// Once shown, a new frame's first draws start from the surface itself (what the
        /// scene does not cover, a strip half a screen pixel wide along its edges, would keep
        /// old pictures).
        fresh: bool,
        /// The surface's bytes as last written from the shadow (rows without padding).
        written: Vec<u8>,
    }

    static SHADOWS: Mutex<Option<HashMap<u32, Shadow>>> = Mutex::new(None);

    /// The size to render `info` at: the screen's surfaces (the display mode's size) as the
    /// platform says, others as they are.
    pub fn render_size(info: &SurfaceInfo) -> (u32, u32) {
        let (w, h) = crate::ddraw::screen_size();
        if cfg!(target_family = "wasm") || (info.width, info.height) != (w, h) {
            return (info.width, info.height);
        }
        rc_platform::render::size_for(w, h)
    }

    pub fn drop(buf: u32) {
        if let Some(map) = SHADOWS.lock().expect("shadows").as_mut() {
            map.remove(&buf);
        }
    }

    fn raw(m: Mem, info: &SurfaceInfo) -> Vec<u8> {
        let row = (info.width * (info.bpp / 8)) as usize;
        let mut out = vec![0u8; row * info.height as usize];
        for y in 0..info.height as usize {
            m.read(
                info.buf + y as u32 * info.pitch,
                &mut out[y * row..(y + 1) * row],
            );
        }
        out
    }

    /// The `size` picture of the surface: its shadow where the surface is as written from it,
    /// else its own pixels (`low`, RGBA8) enlarged.
    pub fn compose(m: Mem, info: &SurfaceInfo, size: (u32, u32)) -> Vec<u8> {
        compose_with(m, info, size, true)
    }

    fn compose_with(m: Mem, info: &SurfaceInfo, size: (u32, u32), to_draw_on: bool) -> Vec<u8> {
        let (lw, lh) = (info.width as usize, info.height as usize);
        let (w, h) = (size.0 as usize, size.1 as usize);
        let bytes = (info.bpp / 8) as usize;
        let now = raw(m, info);
        let rows = crate::raster::RowsToRgba8::new(info);
        let shadows = SHADOWS.lock().expect("shadows");
        let shadow = shadows
            .as_ref()
            .and_then(|map| map.get(&info.buf))
            .filter(|s| s.size == size && s.written.len() == now.len())
            .filter(|s| s.fresh || !to_draw_on);
        let mut out = vec![0u8; w * h * 4];
        // Which of the surface's pixels a row and column of the large picture show.
        let xs: Vec<usize> = (0..w).map(|x| (x * lw / w).min(lw - 1)).collect();
        let row_bytes = lw * bytes;
        // The surface's row `ly` as RGBA8 and which of its pixels are as written from the
        // shadow, made once for all the large rows that show it.
        let mut low_row = vec![0u8; lw * 4];
        let mut same = vec![false; lw];
        let mut made: Option<usize> = None;
        // The last large row made from the surface's row alone (none of it as written): the
        // next large rows that show the same row copy it.
        let mut plain: Option<usize> = None;
        for y in 0..h {
            let ly = (y * lh / h).min(lh - 1);
            let now_row = &now[ly * row_bytes..(ly + 1) * row_bytes];
            if let Some(s) = shadow {
                if s.written[ly * row_bytes..(ly + 1) * row_bytes] == *now_row {
                    // The whole row as written: the shadow's row.
                    out[4 * y * w..4 * (y + 1) * w]
                        .copy_from_slice(&s.rgba[4 * y * w..4 * (y + 1) * w]);
                    plain = None;
                    continue;
                }
            }
            if plain == Some(ly) {
                // The row above showed the same row of the surface, none of it as written.
                let (above, here) = out.split_at_mut(4 * y * w);
                here[..4 * w].copy_from_slice(&above[4 * (y - 1) * w..]);
                continue;
            }
            if made != Some(ly) {
                rows.convert(now_row, &mut low_row);
                for (lx, v) in same.iter_mut().enumerate() {
                    *v = shadow.is_some_and(|s| {
                        let i = ly * row_bytes + lx * bytes;
                        s.written[i..i + bytes] == now[i..i + bytes]
                    });
                }
                made = Some(ly);
            }
            let line = &mut out[4 * y * w..4 * (y + 1) * w];
            if !same.iter().any(|&v| v) {
                let px = line.as_chunks_mut::<4>().0;
                let low = low_row.as_chunks::<4>().0;
                for (p, &lx) in px.iter_mut().zip(&xs) {
                    *p = low[lx];
                }
                plain = Some(ly);
                continue;
            }
            plain = None;
            for (x, &lx) in xs.iter().enumerate() {
                let o = 4 * x;
                match shadow {
                    Some(s) if same[lx] => {
                        let i = 4 * (y * w + x);
                        line[o..o + 4].copy_from_slice(&s.rgba[i..i + 4]);
                    }
                    _ => line[o..o + 4].copy_from_slice(&low_row[4 * lx..4 * lx + 4]),
                }
            }
        }
        out
    }

    /// Keep `rgba` (`size`) as the shadow of the surface's pixels as they are now.
    pub fn keep(m: Mem, info: &SurfaceInfo, size: (u32, u32), rgba: Vec<u8>, fresh: bool) {
        let written = raw(m, info);
        SHADOWS
            .lock()
            .expect("shadows")
            .get_or_insert_with(HashMap::new)
            .insert(
                info.buf,
                Shadow {
                    size,
                    rgba,
                    written,
                    fresh,
                },
            );
    }

    /// Keep `rgba` (`size`) as the surface's shadow and write it scaled down into the surface.
    pub fn store(m: Mem, info: &SurfaceInfo, size: (u32, u32), rgba: Vec<u8>) {
        let (lw, lh) = (info.width as usize, info.height as usize);
        let (w, h) = (size.0 as usize, size.1 as usize);
        let mut low = vec![0u8; lw * lh * 4];
        // The large picture's pixel at the centre of each of the surface's.
        let xs: Vec<usize> = (0..lw)
            .map(|lx| ((2 * lx + 1) * w / (2 * lw)).min(w - 1))
            .collect();
        let big = rgba.as_chunks::<4>().0;
        for (ly, line) in low.as_chunks_mut::<4>().0.chunks_exact_mut(lw).enumerate() {
            let y = ((2 * ly + 1) * h / (2 * lh)).min(h - 1);
            let row = &big[y * w..(y + 1) * w];
            for (p, &x) in line.iter_mut().zip(&xs) {
                *p = row[x];
            }
        }
        crate::raster::from_rgba8(m, info, &low);
        let written = raw(m, info);
        SHADOWS
            .lock()
            .expect("shadows")
            .get_or_insert_with(HashMap::new)
            .insert(
                info.buf,
                Shadow {
                    size,
                    rgba,
                    written,
                    fresh: true,
                },
            );
    }

    /// The surface's picture at its render size, if it has a shadow (for showing it).
    pub fn picture(m: Mem, info: &SurfaceInfo) -> Option<(u32, u32, Vec<u8>)> {
        let size = {
            let shadows = SHADOWS.lock().expect("shadows");
            shadows.as_ref()?.get(&info.buf)?.size
        };
        let picture = compose_with(m, info, size, false);
        if let Some(s) = SHADOWS
            .lock()
            .expect("shadows")
            .as_mut()
            .and_then(|map| map.get_mut(&info.buf))
        {
            s.fresh = false;
        }
        Some((size.0, size.1, picture))
    }
}

/// The size the screen is rendered at: the display mode, or larger with native resolution when
/// a GPU renders (natively; the software renderer and the browser draw at the game's size).
pub fn screen_render_size() -> (u32, u32) {
    let (w, h) = ddraw::screen_size();
    #[cfg(all(feature = "gpu", not(target_family = "wasm")))]
    if batch::available() {
        return rc_platform::render::size_for(w, h);
    }
    (w, h)
}

/// The surface's picture at native resolution, if it is drawn at one (see `shadow`).
pub fn native_picture(m: Mem, info: &crate::ddraw::SurfaceInfo) -> Option<(u32, u32, Vec<u8>)> {
    #[cfg(feature = "gpu")]
    return shadow::picture(m, info);
    #[cfg(not(feature = "gpu"))]
    {
        let _ = (m, info);
        None
    }
}

/// Before the CPU touches surface `obj`: the GPU's pending draws into it are done.
/// A surface's pixels moved (`ddraw`'s margin changed): what was kept of the old ones goes.
pub fn forget_surface(buf: u32) {
    #[cfg(feature = "gpu")]
    shadow::drop(buf);
    #[cfg(feature = "gpu")]
    batch::forget(buf);
    #[cfg(not(feature = "gpu"))]
    let _ = buf;
}

/// A flip of `back` to `front` shown from the GPU (see `batch::flip`); false when the back
/// surface's pixels are not there.
pub fn flip_on_gpu(
    m: Mem,
    back: u32,
    front: Option<u32>,
    overlays: impl FnOnce(u32, u32) -> Vec<rc_platform::overlay::OverlayImage>,
) -> bool {
    #[cfg(feature = "gpu")]
    return batch::available() && batch::flip(m, back, front, overlays);
    #[cfg(not(feature = "gpu"))]
    {
        let _ = (m, back, front, overlays);
        false
    }
}

pub fn flush_if_target(m: Mem, obj: u32) {
    #[cfg(feature = "gpu")]
    batch::flush_if(m, obj);
    #[cfg(not(feature = "gpu"))]
    let _ = (m, obj);
}

/// The frame's commands so far, handed to the renderer (at `Flip`).
pub fn take_frame() -> Vec<Command> {
    std::mem::take(&mut g().frame)
}

/// The `IDirect3D7` object for the DirectDraw object `ddraw`.
pub fn create(m: Mem, ddraw: u32) -> Option<u32> {
    let _ = ddraw;
    Some(com::new_object(m, &D3D7, 0))
}

fn with_dev<R>(c: &Cpu, f: impl FnOnce(&mut Device) -> R) -> Option<R> {
    let id = com::id_of(c.mem(), c.arg(0));
    let mut s = g();
    let d = s.devices.get_mut(&id)?;
    d.snapshot = None;
    Some(f(d))
}

fn read_f32(m: Mem, a: u32) -> f32 {
    f32::from_bits(m.ld32(a))
}

fn read_matrix(m: Mem, a: u32) -> Matrix {
    let mut x = [0f32; 16];
    for (i, v) in x.iter_mut().enumerate() {
        *v = read_f32(m, a + 4 * i as u32);
    }
    x
}

fn write_matrix(m: Mem, a: u32, x: &Matrix) {
    for (i, v) in x.iter().enumerate() {
        m.st32(a + 4 * i as u32, v.to_bits());
    }
}

fn multiply(a: &Matrix, b: &Matrix) -> Matrix {
    let mut r = [0f32; 16];
    for i in 0..4 {
        for j in 0..4 {
            r[4 * i + j] = (0..4).map(|k| a[4 * i + k] * b[4 * k + j]).sum();
        }
    }
    r
}

fn ok1(c: &mut Cpu) {
    ret_stdcall(c, S_OK, 1);
}
fn ok3(c: &mut Cpu) {
    ret_stdcall(c, S_OK, 3);
}

// --- IDirect3D7 ----------------------------------------------------------------------------

/// D3DDEVICEDESC7 (236 bytes) of a hardware T&L device.
fn put_device_desc(m: Mem, p: u32, guid: &[u8; 16]) {
    for i in (0..236).step_by(4) {
        m.st32(p + i, 0);
    }
    // FLOATTLVERTEX, EXECUTESYSTEMMEMORY, TLVERTEXSYSTEMMEMORY, TEXTUREVIDEOMEMORY,
    // DRAWPRIMTLVERTEX, CANRENDERAFTERFLIP, TEXTURENONLOCALVIDMEM, DRAWPRIMITIVES2,
    // DRAWPRIMITIVES2EX, HWTRANSFORMANDLIGHT, HWRASTERIZATION
    m.st32(
        p,
        0x1 | 0x10 | 0x40 | 0x200 | 0x400 | 0x800 | 0x1000 | 0x2000 | 0x8000 | 0x1_0000 | 0x8_0000,
    );
    for prim in [p + 4, p + 60] {
        m.st32(prim, 56);
        m.st32(prim + 4, 0x2 | 0x10 | 0x20 | 0x40); // MASKZ, CULLNONE, CULLCW, CULLCCW
        m.st32(
            prim + 8,
            0x1 | 0x10 | 0x80 | 0x100 | 0x2000 | 0x4000 | 0x1_0000 | 0x10_0000,
        );
        m.st32(prim + 12, 0xff); // every z comparison
        m.st32(prim + 16, 0x1fff); // every source blend
        m.st32(prim + 20, 0x1fff); // every destination blend
        m.st32(prim + 24, 0xff); // every alpha comparison
        m.st32(prim + 28, 0x8 | 0x200 | 0x4000 | 0x8_0000);
        m.st32(prim + 32, 0x1 | 0x4 | 0x8); // PERSPECTIVE, ALPHA, TRANSPARENCY
        m.st32(prim + 36, 0x3f | 0x300 | 0x3_0000 | 0x300_0000);
        m.st32(prim + 40, 0xff);
        m.st32(prim + 44, 0xf); // WRAP, MIRROR, CLAMP, BORDER
    }
    m.st32(p + 116, 0x400 | 0x100); // 16 and 32-bit targets
    m.st32(p + 120, 0x400 | 0x200 | 0x100); // 16, 24, 32-bit z
    m.st32(p + 124, 1);
    m.st32(p + 128, 1);
    m.st32(p + 132, 2048);
    m.st32(p + 136, 2048);
    m.st32(p + 140, 2048);
    m.st32(p + 144, 2048);
    m.st32(p + 148, 1);
    m.st32(p + 176, 8); // FVF: up to 8 texture coordinate sets
    m.st32(p + 180, 0x3ff_ffff);
    m.st16(p + 184, 8);
    m.st16(p + 186, 8);
    m.st32(p + 188, 8);
    m.st32(p + 192, 1e10f32.to_bits());
    m.write(p + 196, guid);
    m.st32(p + 216, 0x3f);
}

fn d3d_QueryInterface(c: &mut Cpu) {
    let m = c.mem();
    m.st32(c.arg(2), 0);
    ret_stdcall(c, E_NOINTERFACE, 3);
}

fn d3d_EnumDevices(c: &mut Cpu) {
    let m = c.mem();
    let (cb, ctx) = (c.arg(1), c.arg(2));
    let block = vm::heap_alloc(m, 512, true);
    let (desc, name, dd) = (block, block + 64, block + 128);
    let devices: [(&[u8], &[u8], [u8; 16]); 3] = [
        (
            b"Microsoft Direct3D RGB Software Emulation\0",
            b"RGB Emulation\0",
            IID_RGB,
        ),
        (
            b"Microsoft Direct3D Hardware acceleration through Direct3D HAL\0",
            b"Direct3D HAL\0",
            IID_HAL,
        ),
        (
            b"Microsoft Direct3D Hardware Transform and Lighting acceleration capable device\0",
            b"Direct3D T&L HAL\0",
            IID_TNLHAL,
        ),
    ];
    for (d, n, gd) in devices {
        m.write(desc, &d[..d.len().min(63)]);
        m.st8(desc + 63, 0);
        m.write(name, n);
        put_device_desc(m, dd, &gd);
        if call_guest(c, cb, &[desc, name, dd, ctx]) == 0 {
            break;
        }
    }
    vm::heap_free(block);
    ret_stdcall(c, S_OK, 3);
}

fn d3d_EnumZBufferFormats(c: &mut Cpu) {
    let m = c.mem();
    let (cb, ctx) = (c.arg(2), c.arg(3));
    let pf = vm::heap_alloc(m, 32, true);
    for bits in [16u32, 24, 32] {
        for i in (0..32).step_by(4) {
            m.st32(pf + i, 0);
        }
        m.st32(pf, 32);
        m.st32(pf + 4, 0x400); // DDPF_ZBUFFER
        m.st32(pf + 12, bits);
        m.st32(pf + 20, if bits == 16 { 0xffff } else { 0x00ff_ffff });
        if call_guest(c, cb, &[pf, ctx]) == 0 {
            break;
        }
    }
    vm::heap_free(pf);
    ret_stdcall(c, S_OK, 4);
}

fn d3d_CreateDevice(c: &mut Cpu) {
    let m = c.mem();
    let (target, out) = (c.arg(2), c.arg(3));
    let id = next_id();
    let (_, w, h, _, _) = ddraw::surface_of(m, target).unwrap_or((0, 640, 480, 0, 16));
    let mut state = State {
        target,
        ..State::default()
    };
    state.viewport = [0.0, 0.0, w as f32, h as f32, 0.0, 1.0];
    g().devices.insert(
        id,
        Device {
            state,
            snapshot: None,
        },
    );
    m.st32(out, com::new_object(m, &DEVICE7, id));
    ret_stdcall(c, S_OK, 4);
}

fn d3d_CreateVertexBuffer(c: &mut Cpu) {
    let m = c.mem();
    let (desc, out) = (c.arg(1), c.arg(2));
    // D3DVERTEXBUFFERDESC: size, caps, FVF, number of vertices.
    let (fvf, count) = (m.ld32(desc + 8), m.ld32(desc + 12));
    let stride = fvf_stride(fvf);
    let buf = vm::heap_alloc(m, (stride * count).max(4), true);
    let id = next_id();
    g().buffers.insert(id, VertexBuffer { buf, fvf, count });
    m.st32(out, com::new_object(m, &VB7, id));
    ret_stdcall(c, S_OK, 4);
}

// --- vertices ------------------------------------------------------------------------------

/// The size of one vertex of a flexible vertex format.
pub fn fvf_stride(fvf: u32) -> u32 {
    let mut n = match fvf & 0xe {
        0x2 => 12, // XYZ
        0x4 => 16, // XYZRHW
        0x6 => 16, // XYZB1
        0x8 => 20, // XYZB2
        0xa => 24, // XYZB3
        0xc => 28, // XYZB4
        0xe => 32, // XYZB5
        _ => 0,
    };
    if fvf & 0x10 != 0 {
        n += 12; // normal
    }
    if fvf & 0x20 != 0 {
        n += 4; // point size (reserved1 in DX7)
    }
    if fvf & 0x40 != 0 {
        n += 4; // diffuse
    }
    if fvf & 0x80 != 0 {
        n += 4; // specular
    }
    let sets = (fvf >> 8) & 0xf;
    for i in 0..sets {
        // D3DFVF_TEXCOORDSIZEn: 0 → 2 floats, 1 → 3, 2 → 4, 3 → 1.
        let size = match (fvf >> (16 + 2 * i)) & 3 {
            0 => 2,
            1 => 3,
            2 => 4,
            _ => 1,
        };
        n += 4 * size;
    }
    n
}

fn snapshot(d: &mut Device) -> Arc<State> {
    if let Some(s) = &d.snapshot {
        return s.clone();
    }
    let s = Arc::new(d.state.clone());
    d.snapshot = Some(s.clone());
    s
}

static WHOLE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Run `f` with its draws placed on a widescreen target's whole width (not moved into its
/// middle): for a game patched to draw its world over the margins (`ddraw::margin`).
pub fn on_whole_width<R>(f: impl FnOnce() -> R) -> R {
    use std::sync::atomic::Ordering;
    let before = WHOLE.swap(true, Ordering::Relaxed);
    let r = f();
    WHOLE.store(before, Ordering::Relaxed);
    r
}

/// Move a draw into a widescreen target's middle (`ddraw::margin`): pre-transformed vertices by
/// their screen position, others by the viewport.
fn widen(m: Mem, state: Arc<State>, fvf: u32, vertices: &mut [u8]) -> Arc<State> {
    let ox = ddraw::margin_of(m, state.target);
    if ox == 0 || WHOLE.load(std::sync::atomic::Ordering::Relaxed) {
        return state;
    }
    if fvf & 0xe == 0x4 {
        // D3DFVF_XYZRHW: x is the first float.
        let stride = fvf_stride(fvf) as usize;
        for v in vertices.chunks_exact_mut(stride) {
            let x = f32::from_le_bytes([v[0], v[1], v[2], v[3]]) + ox as f32;
            v[..4].copy_from_slice(&x.to_le_bytes());
        }
        return state;
    }
    let mut s = (*state).clone();
    s.viewport[0] += ox as f32;
    Arc::new(s)
}

fn record(c: &Cpu, primitive: u32, fvf: u32, vertices: Vec<u8>, indices: Vec<u16>) {
    crate::profile::time(crate::profile::Section::Record, || {
        record_draw(c, primitive, fvf, vertices, indices);
    });
}

fn record_draw(c: &Cpu, primitive: u32, fvf: u32, mut vertices: Vec<u8>, indices: Vec<u16>) {
    let m = c.mem();
    let id = com::id_of(m, c.arg(0));
    let mut s = g();
    let Some(d) = s.devices.get_mut(&id) else {
        return;
    };
    let mut state = snapshot(d);
    if s.captured.is_none() {
        state = widen(m, state, fvf, &mut vertices);
    }
    if let Some(list) = s.captured.as_mut() {
        list.push(Draw {
            primitive,
            fvf,
            vertices,
            indices,
            state,
        });
        return;
    }
    let t = rc_runtime::host::Stamp::now();
    #[cfg(feature = "gpu")]
    if batch::available() {
        let target = state.target;
        drop(s);
        if let Some(d) = crate::raster::gpu_draw(m, &state, primitive, fvf, &vertices, &indices) {
            batch::push(m, target, rc_platform::gpu::Command::Draw(d));
        }
        crate::RASTER_NS.fetch_add(t.elapsed_ns(), std::sync::atomic::Ordering::Relaxed);
        return;
    }
    // The software renderer draws now, into the target in guest memory.
    crate::raster::draw(m, &state, primitive, fvf, &vertices, &indices);
    crate::RASTER_NS.fetch_add(t.elapsed_ns(), std::sync::atomic::Ordering::Relaxed);
    s.frame.push(Command::Draw {
        primitive,
        fvf,
        vertices,
        indices,
        state,
    });
}

fn read_bytes(m: Mem, a: u32, n: u32) -> Vec<u8> {
    let mut v = vec![0u8; n as usize];
    m.read(a, &mut v);
    v
}

fn read_indices(m: Mem, a: u32, n: u32) -> Vec<u16> {
    (0..n).map(|i| m.ld16(a + 2 * i)).collect()
}

// --- IDirect3DDevice7 ----------------------------------------------------------------------

fn dev_GetCaps(c: &mut Cpu) {
    put_device_desc(c.mem(), c.arg(1), &IID_TNLHAL);
    ret_stdcall(c, S_OK, 2);
}

/// The texture formats offered: 16-bit 565, 1555, 4444 and 32-bit 8888.
fn dev_EnumTextureFormats(c: &mut Cpu) {
    let m = c.mem();
    let (cb, ctx) = (c.arg(1), c.arg(2));
    let pf = vm::heap_alloc(m, 32, true);
    let formats: [(u32, u32, [u32; 4]); 4] = [
        (0x40, 16, [0xf800, 0x07e0, 0x001f, 0]),
        (0x41, 16, [0x7c00, 0x03e0, 0x001f, 0x8000]),
        (0x41, 16, [0x0f00, 0x00f0, 0x000f, 0xf000]),
        (
            0x41,
            32,
            [0x00ff_0000, 0x0000_ff00, 0x0000_00ff, 0xff00_0000],
        ),
    ];
    for (flags, bits, masks) in formats {
        m.st32(pf, 32);
        m.st32(pf + 4, flags); // DDPF_RGB (| DDPF_ALPHAPIXELS)
        m.st32(pf + 8, 0);
        m.st32(pf + 12, bits);
        for (i, v) in masks.iter().enumerate() {
            m.st32(pf + 16 + 4 * i as u32, *v);
        }
        if call_guest(c, cb, &[pf, ctx]) == 0 {
            break;
        }
    }
    vm::heap_free(pf);
    ret_stdcall(c, S_OK, 3);
}

fn dev_GetDirect3D(c: &mut Cpu) {
    let m = c.mem();
    m.st32(c.arg(1), com::new_object(m, &D3D7, 0));
    ret_stdcall(c, S_OK, 2);
}

fn dev_SetRenderTarget(c: &mut Cpu) {
    let t = c.arg(1);
    let old = with_dev(c, |d| d.state.target).unwrap_or(0);
    flush_if_target(c.mem(), old);
    with_dev(c, |d| d.state.target = t);
    ret_stdcall(c, S_OK, 3);
}

fn dev_GetRenderTarget(c: &mut Cpu) {
    let m = c.mem();
    let t = with_dev(c, |d| d.state.target).unwrap_or(0);
    if t != 0 {
        com::add_ref_raw(m, t);
    }
    m.st32(c.arg(1), t);
    ret_stdcall(c, S_OK, 2);
}

/// `Clear`: the render target's (D3DCLEAR_TARGET 1) and its z-buffer's (D3DCLEAR_ZBUFFER 2)
/// pixels in guest memory, within the viewport or the given rectangles; and recorded.
fn dev_Clear(c: &mut Cpu) {
    let m = c.mem();
    let (n, rects_p, flags, color, z) = (
        c.arg(1),
        c.arg(2),
        c.arg(3),
        c.arg(4),
        f32::from_bits(c.arg(5)),
    );
    let Some((target, vp)) = with_dev(c, |d| (d.state.target, d.state.viewport)) else {
        return ret_stdcall(c, D3DERR_INVALIDCALL, 7);
    };
    let mut rects: Vec<[i32; 4]> = (0..n)
        .map(|i| {
            let p = rects_p + 16 * i;
            [
                m.ld32(p) as i32,
                m.ld32(p + 4) as i32,
                m.ld32(p + 8) as i32,
                m.ld32(p + 12) as i32,
            ]
        })
        .collect();
    let ox = ddraw::margin_of(m, target) as i32;
    if rects.is_empty() {
        rects.push([
            vp[0] as i32,
            vp[1] as i32,
            (vp[0] + vp[2]) as i32,
            (vp[1] + vp[3]) as i32,
        ]);
    }
    if ox > 0 {
        // Widescreen: moved into the middle; a clear of the whole screen clears the margins too.
        let (gw, gh, _) = ddraw::display_mode();
        for r in &mut rects {
            if r[0] <= 0 && r[1] <= 0 && r[2] >= gw as i32 && r[3] >= gh as i32 {
                r[0] = -ox;
                r[2] += ox;
            }
            r[0] += ox;
            r[2] += ox;
        }
    }
    #[cfg(feature = "gpu")]
    let gpu = batch::available();
    #[cfg(not(feature = "gpu"))]
    let gpu = false;
    if flags & 1 != 0 {
        flush_if_target(m, target);
        if let Some((buf, w, h, pitch, bpp)) = ddraw::surface_of(m, target) {
            let v = match bpp {
                16 => {
                    let (r, gg, b) = ((color >> 16) & 0xff, (color >> 8) & 0xff, color & 0xff);
                    ((r >> 3) << 11) | ((gg >> 2) << 5) | (b >> 3)
                }
                _ => color,
            };
            fill(m, buf, w, h, pitch, bpp, &rects, v);
            #[cfg(feature = "gpu")]
            shadow::drop(buf);
        }
    }
    #[cfg(feature = "gpu")]
    if gpu && flags & 2 != 0 {
        batch::push(
            m,
            target,
            rc_platform::gpu::Command::ClearDepth(z.clamp(0.0, 1.0)),
        );
    }
    if flags & 2 != 0 && !gpu {
        if let Some((buf, w, h, pitch, bpp)) = ddraw::zbuffer_of(m, target) {
            let max = if bpp == 16 { 0xffff } else { 0x00ff_ffff };
            let v = (z.clamp(0.0, 1.0) * max as f32) as u32;
            fill(m, buf, w, h, pitch, bpp, &rects, v);
        }
    }
    g().frame.push(Command::Clear {
        rects,
        flags,
        color,
        z,
    });
    ret_stdcall(c, S_OK, 7);
}

#[allow(clippy::too_many_arguments)]
fn fill(m: Mem, buf: u32, w: u32, h: u32, pitch: u32, bpp: u32, rects: &[[i32; 4]], v: u32) {
    for r in rects {
        for y in r[1].max(0)..r[3].min(h as i32) {
            for x in r[0].max(0)..r[2].min(w as i32) {
                let a = buf + y as u32 * pitch + x as u32 * (bpp / 8);
                match bpp {
                    16 => m.st16(a, v as u16),
                    24 => {
                        m.st16(a, v as u16);
                        m.st8(a + 2, (v >> 16) as u8);
                    }
                    _ => m.st32(a, v),
                }
            }
        }
    }
}

fn transform_slot(d: &mut Device, kind: u32) -> Option<&mut Matrix> {
    match kind {
        1 => Some(&mut d.state.world), // D3DTRANSFORMSTATE_WORLD (DX7 also 256)
        256 => Some(&mut d.state.world),
        2 => Some(&mut d.state.view),
        3 => Some(&mut d.state.projection),
        _ => None,
    }
}

fn dev_SetTransform(c: &mut Cpu) {
    let m = c.mem();
    let (kind, p) = (c.arg(1), c.arg(2));
    let x = read_matrix(m, p);
    with_dev(c, |d| {
        if let Some(t) = transform_slot(d, kind) {
            *t = x;
        }
    });
    ret_stdcall(c, S_OK, 3);
}

fn dev_GetTransform(c: &mut Cpu) {
    let m = c.mem();
    let (kind, p) = (c.arg(1), c.arg(2));
    let x = with_dev(c, |d| transform_slot(d, kind).map(|t| *t))
        .flatten()
        .unwrap_or(IDENTITY);
    write_matrix(m, p, &x);
    ret_stdcall(c, S_OK, 3);
}

fn dev_MultiplyTransform(c: &mut Cpu) {
    let m = c.mem();
    let (kind, p) = (c.arg(1), c.arg(2));
    let x = read_matrix(m, p);
    with_dev(c, |d| {
        if let Some(t) = transform_slot(d, kind) {
            *t = multiply(&x, t);
        }
    });
    ret_stdcall(c, S_OK, 3);
}

fn dev_SetViewport(c: &mut Cpu) {
    let m = c.mem();
    let p = c.arg(1);
    let v = [
        m.ld32(p) as f32,
        m.ld32(p + 4) as f32,
        m.ld32(p + 8) as f32,
        m.ld32(p + 12) as f32,
        read_f32(m, p + 16),
        read_f32(m, p + 20),
    ];
    with_dev(c, |d| d.state.viewport = v);
    ret_stdcall(c, S_OK, 2);
}

fn dev_GetViewport(c: &mut Cpu) {
    let m = c.mem();
    let p = c.arg(1);
    let v = with_dev(c, |d| d.state.viewport).unwrap_or([0.0, 0.0, 640.0, 480.0, 0.0, 1.0]);
    for (i, x) in v[..4].iter().enumerate() {
        m.st32(p + 4 * i as u32, *x as u32);
    }
    m.st32(p + 16, v[4].to_bits());
    m.st32(p + 20, v[5].to_bits());
    ret_stdcall(c, S_OK, 2);
}

fn dev_SetMaterial(c: &mut Cpu) {
    let m = c.mem();
    let p = c.arg(1);
    let mut x = [0f32; 17];
    for (i, v) in x.iter_mut().enumerate() {
        *v = read_f32(m, p + 4 * i as u32);
    }
    with_dev(c, |d| d.state.material = x);
    ret_stdcall(c, S_OK, 2);
}

fn dev_GetMaterial(c: &mut Cpu) {
    let m = c.mem();
    let p = c.arg(1);
    let x = with_dev(c, |d| d.state.material).unwrap_or([0.0; 17]);
    for (i, v) in x.iter().enumerate() {
        m.st32(p + 4 * i as u32, v.to_bits());
    }
    ret_stdcall(c, S_OK, 2);
}

fn dev_SetLight(c: &mut Cpu) {
    let m = c.mem();
    let (i, p) = (c.arg(1), c.arg(2));
    let mut l = [0u8; 104];
    m.read(p, &mut l);
    with_dev(c, |d| {
        let on = d.state.lights.get(&i).is_some_and(|x| x.1);
        d.state.lights.insert(i, (l, on));
    });
    ret_stdcall(c, S_OK, 3);
}

fn dev_GetLight(c: &mut Cpu) {
    let m = c.mem();
    let (i, p) = (c.arg(1), c.arg(2));
    let l = with_dev(c, |d| d.state.lights.get(&i).map(|x| x.0)).flatten();
    match l {
        Some(l) => {
            m.write(p, &l);
            ret_stdcall(c, S_OK, 3);
        }
        None => ret_stdcall(c, D3DERR_INVALIDCALL, 3),
    }
}

fn dev_LightEnable(c: &mut Cpu) {
    let (i, on) = (c.arg(1), c.arg(2) != 0);
    with_dev(c, |d| {
        d.state.lights.entry(i).or_insert(([0; 104], false)).1 = on;
    });
    ret_stdcall(c, S_OK, 3);
}

fn dev_GetLightEnable(c: &mut Cpu) {
    let m = c.mem();
    let (i, p) = (c.arg(1), c.arg(2));
    let on = with_dev(c, |d| d.state.lights.get(&i).is_some_and(|x| x.1)).unwrap_or(false);
    m.st32(p, u32::from(on));
    ret_stdcall(c, S_OK, 3);
}

fn dev_SetRenderState(c: &mut Cpu) {
    let (k, v) = (c.arg(1), c.arg(2));
    with_dev(c, |d| {
        d.state.render.insert(k, v);
    });
    ret_stdcall(c, S_OK, 3);
}

fn dev_GetRenderState(c: &mut Cpu) {
    let m = c.mem();
    let (k, p) = (c.arg(1), c.arg(2));
    let v = with_dev(c, |d| d.state.render.get(&k).copied())
        .flatten()
        .unwrap_or(0);
    m.st32(p, v);
    ret_stdcall(c, S_OK, 3);
}

fn dev_SetTextureStageState(c: &mut Cpu) {
    let (stage, k, v) = (c.arg(1) as usize, c.arg(2), c.arg(3));
    with_dev(c, |d| {
        if let Some(s) = d.state.stages.get_mut(stage) {
            s.insert(k, v);
        }
    });
    ret_stdcall(c, S_OK, 4);
}

fn dev_GetTextureStageState(c: &mut Cpu) {
    let m = c.mem();
    let (stage, k, p) = (c.arg(1) as usize, c.arg(2), c.arg(3));
    let v = with_dev(c, |d| {
        d.state.stages.get(stage).and_then(|s| s.get(&k).copied())
    })
    .flatten()
    .unwrap_or(0);
    m.st32(p, v);
    ret_stdcall(c, S_OK, 4);
}

fn dev_SetTexture(c: &mut Cpu) {
    let m = c.mem();
    let (stage, tex) = (c.arg(1) as usize, c.arg(2));
    let version = if tex != 0 {
        ddraw::version_of(m, tex)
    } else {
        0
    };
    with_dev(c, |d| {
        if let Some(t) = d.state.textures.get_mut(stage) {
            *t = (tex != 0).then_some((tex, version));
        }
    });
    ret_stdcall(c, S_OK, 3);
}

fn dev_GetTexture(c: &mut Cpu) {
    let m = c.mem();
    let (stage, p) = (c.arg(1) as usize, c.arg(2));
    let t = with_dev(c, |d| d.state.textures.get(stage).copied().flatten())
        .flatten()
        .map_or(0, |t| t.0);
    if t != 0 {
        com::add_ref_raw(m, t);
    }
    m.st32(p, t);
    ret_stdcall(c, S_OK, 3);
}

fn dev_DrawPrimitive(c: &mut Cpu) {
    let m = c.mem();
    let (prim, fvf, verts, n) = (c.arg(1), c.arg(2), c.arg(3), c.arg(4));
    let data = read_bytes(m, verts, fvf_stride(fvf) * n);
    record(c, prim, fvf, data, Vec::new());
    ret_stdcall(c, S_OK, 6);
}

fn dev_DrawIndexedPrimitive(c: &mut Cpu) {
    let m = c.mem();
    let (prim, fvf, verts, nv, idx, ni) =
        (c.arg(1), c.arg(2), c.arg(3), c.arg(4), c.arg(5), c.arg(6));
    let data = read_bytes(m, verts, fvf_stride(fvf) * nv);
    let indices = read_indices(m, idx, ni);
    record(c, prim, fvf, data, indices);
    ret_stdcall(c, S_OK, 8);
}

fn vb(m: Mem, obj: u32) -> Option<(u32, u32, u32)> {
    let id = com::id_of(m, obj);
    g().buffers.get(&id).map(|b| (b.buf, b.fvf, b.count))
}

fn dev_DrawPrimitiveVB(c: &mut Cpu) {
    let m = c.mem();
    let (prim, vbo, start, n) = (c.arg(1), c.arg(2), c.arg(3), c.arg(4));
    if let Some((buf, fvf, _)) = vb(m, vbo) {
        let s = fvf_stride(fvf);
        let data = read_bytes(m, buf + s * start, s * n);
        record(c, prim, fvf, data, Vec::new());
    }
    ret_stdcall(c, S_OK, 6);
}

fn dev_DrawIndexedPrimitiveVB(c: &mut Cpu) {
    let m = c.mem();
    let (prim, vbo, start, nv, idx, ni) =
        (c.arg(1), c.arg(2), c.arg(3), c.arg(4), c.arg(5), c.arg(6));
    if let Some((buf, fvf, _)) = vb(m, vbo) {
        let s = fvf_stride(fvf);
        let data = read_bytes(m, buf + s * start, s * nv);
        let indices = read_indices(m, idx, ni);
        record(c, prim, fvf, data, indices);
    }
    ret_stdcall(c, S_OK, 8);
}

fn dev_Load(c: &mut Cpu) {
    let m = c.mem();
    let ok = ddraw::load(m, c.arg(1), c.arg(2), c.arg(3), c.arg(4));
    ret_stdcall(c, if ok { S_OK } else { D3DERR_INVALIDCALL }, 6);
}

fn dev_ValidateDevice(c: &mut Cpu) {
    let p = c.arg(1);
    if p != 0 {
        c.mem().st32(p, 1);
    }
    ret_stdcall(c, S_OK, 2);
}

fn dev_PreLoad(c: &mut Cpu) {
    ret_stdcall(c, S_OK, 2);
}

fn dev_SetClipStatus(c: &mut Cpu) {
    ret_stdcall(c, S_OK, 2);
}

fn dev_GetClipStatus(c: &mut Cpu) {
    let m = c.mem();
    let p = c.arg(1);
    for i in (0..24).step_by(4) {
        m.st32(p + i, 0);
    }
    ret_stdcall(c, S_OK, 2);
}

fn dev_GetInfo(c: &mut Cpu) {
    ret_stdcall(c, 1, 4); // S_FALSE: no statistics
}

// --- IDirect3DVertexBuffer7 ----------------------------------------------------------------

fn vb_Lock(c: &mut Cpu) {
    let m = c.mem();
    let (out, size) = (c.arg(2), c.arg(3));
    match vb(m, c.arg(0)) {
        Some((buf, fvf, count)) => {
            m.st32(out, buf);
            if size != 0 {
                m.st32(size, fvf_stride(fvf) * count);
            }
            ret_stdcall(c, S_OK, 4);
        }
        None => ret_stdcall(c, D3DERR_INVALIDCALL, 4),
    }
}

fn vb_GetVertexBufferDesc(c: &mut Cpu) {
    let m = c.mem();
    let p = c.arg(1);
    if let Some((_, fvf, count)) = vb(m, c.arg(0)) {
        m.st32(p, 16);
        m.st32(p + 4, 0);
        m.st32(p + 8, fvf);
        m.st32(p + 12, count);
    }
    ret_stdcall(c, S_OK, 2);
}

// --- interfaces ----------------------------------------------------------------------------

static D3D7: Interface = Interface {
    dll: "ddraw.dll",
    name: "IDirect3D7",
    methods: &[
        ("QueryInterface", Some(d3d_QueryInterface)),
        ("AddRef", Some(com::add_ref)),
        ("Release", Some(com::release)),
        ("EnumDevices", Some(d3d_EnumDevices)),
        ("CreateDevice", Some(d3d_CreateDevice)),
        ("CreateVertexBuffer", Some(d3d_CreateVertexBuffer)),
        ("EnumZBufferFormats", Some(d3d_EnumZBufferFormats)),
        ("EvictManagedTextures", Some(ok1)),
    ],
};

static DEVICE7: Interface = Interface {
    dll: "ddraw.dll",
    name: "IDirect3DDevice7",
    methods: &[
        ("QueryInterface", None),
        ("AddRef", Some(com::add_ref)),
        ("Release", Some(com::release)),
        ("GetCaps", Some(dev_GetCaps)),
        ("EnumTextureFormats", Some(dev_EnumTextureFormats)),
        ("BeginScene", Some(ok1)),
        ("EndScene", Some(ok1)),
        ("GetDirect3D", Some(dev_GetDirect3D)),
        ("SetRenderTarget", Some(dev_SetRenderTarget)),
        ("GetRenderTarget", Some(dev_GetRenderTarget)),
        ("Clear", Some(dev_Clear)),
        ("SetTransform", Some(dev_SetTransform)),
        ("GetTransform", Some(dev_GetTransform)),
        ("SetViewport", Some(dev_SetViewport)),
        ("MultiplyTransform", Some(dev_MultiplyTransform)),
        ("GetViewport", Some(dev_GetViewport)),
        ("SetMaterial", Some(dev_SetMaterial)),
        ("GetMaterial", Some(dev_GetMaterial)),
        ("SetLight", Some(dev_SetLight)),
        ("GetLight", Some(dev_GetLight)),
        ("SetRenderState", Some(dev_SetRenderState)),
        ("GetRenderState", Some(dev_GetRenderState)),
        ("BeginStateBlock", None),
        ("EndStateBlock", None),
        ("PreLoad", Some(dev_PreLoad)),
        ("DrawPrimitive", Some(dev_DrawPrimitive)),
        ("DrawIndexedPrimitive", Some(dev_DrawIndexedPrimitive)),
        ("SetClipStatus", Some(dev_SetClipStatus)),
        ("GetClipStatus", Some(dev_GetClipStatus)),
        ("DrawPrimitiveStrided", None),
        ("DrawIndexedPrimitiveStrided", None),
        ("DrawPrimitiveVB", Some(dev_DrawPrimitiveVB)),
        ("DrawIndexedPrimitiveVB", Some(dev_DrawIndexedPrimitiveVB)),
        ("ComputeSphereVisibility", None),
        ("GetTexture", Some(dev_GetTexture)),
        ("SetTexture", Some(dev_SetTexture)),
        ("GetTextureStageState", Some(dev_GetTextureStageState)),
        ("SetTextureStageState", Some(dev_SetTextureStageState)),
        ("ValidateDevice", Some(dev_ValidateDevice)),
        ("ApplyStateBlock", None),
        ("CaptureStateBlock", None),
        ("DeleteStateBlock", None),
        ("CreateStateBlock", None),
        ("Load", Some(dev_Load)),
        ("LightEnable", Some(dev_LightEnable)),
        ("GetLightEnable", Some(dev_GetLightEnable)),
        ("SetClipPlane", Some(ok3)),
        ("GetClipPlane", None),
        ("GetInfo", Some(dev_GetInfo)),
    ],
};

static VB7: Interface = Interface {
    dll: "ddraw.dll",
    name: "IDirect3DVertexBuffer7",
    methods: &[
        ("QueryInterface", None),
        ("AddRef", Some(com::add_ref)),
        ("Release", Some(com::release)),
        ("Lock", Some(vb_Lock)),
        ("Unlock", Some(ok1)),
        ("ProcessVertices", None),
        ("GetVertexBufferDesc", Some(vb_GetVertexBufferDesc)),
        ("Optimize", Some(ok3)),
        ("ProcessVerticesStrided", None),
    ],
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vertex_sizes() {
        assert_eq!(fvf_stride(0x1c4), 32); // XYZRHW | DIFFUSE | SPECULAR | TEX1
        assert_eq!(fvf_stride(0x112), 32); // XYZ | NORMAL | TEX1
        assert_eq!(fvf_stride(0x142), 24); // XYZ | DIFFUSE | TEX1
    }
}
