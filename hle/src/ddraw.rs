// SPDX-License-Identifier: MIT
//! ddraw: DirectDraw 7 as TechnoMage reaches it (`DirectDrawEnumerateExA`, `DirectDrawCreateEx`
//! through `LoadLibraryA`). Surfaces are pixel buffers in guest memory, so the game's `Lock`
//! hands out a real pointer; `Blt` and `BltFast` are software blits; `Flip` hands the back
//! buffer to the platform as a frame. Direct3D draws into the same surfaces (`d3d`, M5).

#![allow(non_snake_case)]

use crate::com::{self, Interface, E_NOINTERFACE, S_OK};
use rc_runtime::cpu::Cpu;
use rc_runtime::dispatch::call_guest;
use rc_runtime::imports::{self, ret_stdcall};
use rc_runtime::mem::Mem;
use rc_runtime::vm;
use std::collections::BTreeMap;
use std::sync::Mutex;

const DDERR_GENERIC: u32 = 0x8000_4005;
const DDERR_INVALIDPARAMS: u32 = 0x8007_0057;
const DDERR_NOTFOUND: u32 = 0x8876_00ff;
const DDERR_UNSUPPORTED: u32 = 0x8000_4001;

pub const IID_IDIRECTDRAW7: [u8; 16] = com::guid(
    0x15e6_5ec0,
    0x3b9c,
    0x11d2,
    [0xb9, 0x2f, 0x00, 0x60, 0x97, 0x97, 0xea, 0x5b],
);
pub const IID_IDIRECT3D7: [u8; 16] = com::guid(
    0xf504_9e77,
    0x4861,
    0x11d2,
    [0xa4, 0x07, 0x00, 0xa0, 0xc9, 0x06, 0x29, 0xa8],
);
const IID_IDIRECTDRAW: [u8; 16] = com::guid(
    0x6c14_db80,
    0xa733,
    0x11ce,
    [0xa5, 0x21, 0x00, 0x20, 0xaf, 0x0b, 0xe5, 0x60],
);
const IID_IDIRECTDRAW2: [u8; 16] = com::guid(
    0xb3a6_f3e0,
    0x2b43,
    0x11cf,
    [0xa2, 0xde, 0x00, 0xaa, 0x00, 0xb9, 0x33, 0x56],
);
const IID_IDIRECTDRAW4: [u8; 16] = com::guid(
    0x9c59_509a,
    0x39bd,
    0x11d1,
    [0x8c, 0x4a, 0x00, 0xc0, 0x4f, 0xd9, 0x30, 0xc5],
);
/// Every IDirectDrawSurface version: their vtables are prefixes of version 7's, in the same
/// order, so one object serves them all.
const IID_SURFACES: [[u8; 16]; 5] = [
    com::guid(
        0x6c14_db81,
        0xa733,
        0x11ce,
        [0xa5, 0x21, 0x00, 0x20, 0xaf, 0x0b, 0xe5, 0x60],
    ),
    com::guid(
        0x5780_5885,
        0x6eec,
        0x11cf,
        [0x94, 0x41, 0xa8, 0x23, 0x03, 0xc1, 0x0e, 0x27],
    ),
    com::guid(
        0xda04_4e00,
        0x69b2,
        0x11d0,
        [0xa1, 0xd5, 0x00, 0xaa, 0x00, 0xb8, 0xdf, 0xbb],
    ),
    com::guid(
        0x0b2b_8630,
        0xad35,
        0x11d0,
        [0x8e, 0xa6, 0x00, 0x60, 0x97, 0x97, 0xea, 0x5b],
    ),
    com::guid(
        0x0667_5a80,
        0x3b9b,
        0x11d2,
        [0xb9, 0x2f, 0x00, 0x60, 0x97, 0x97, 0xea, 0x5b],
    ),
];

// DDSCAPS
const DDSCAPS_BACKBUFFER: u32 = 0x4;
const DDSCAPS_COMPLEX: u32 = 0x8;
const DDSCAPS_FLIP: u32 = 0x10;
const DDSCAPS_FRONTBUFFER: u32 = 0x20;
const DDSCAPS_PRIMARYSURFACE: u32 = 0x200;
const DDSCAPS_TEXTURE: u32 = 0x1000;
const DDSCAPS_3DDEVICE: u32 = 0x2000;
const DDSCAPS_VIDEOMEMORY: u32 = 0x4000;
const DDSCAPS_LOCALVIDMEM: u32 = 0x1000_0000;
const DDSCAPS_ZBUFFER: u32 = 0x20000;

// DDSURFACEDESC2 flags
const DDSD_CAPS: u32 = 0x1;
const DDSD_HEIGHT: u32 = 0x2;
const DDSD_WIDTH: u32 = 0x4;
const DDSD_PITCH: u32 = 0x8;
const DDSD_BACKBUFFERCOUNT: u32 = 0x20;
const DDSD_ZBUFFERBITDEPTH: u32 = 0x40;
const DDSD_LPSURFACE: u32 = 0x800;
const DDSD_PIXELFORMAT: u32 = 0x1000;
const DDSD_REFRESHRATE: u32 = 0x40000;

/// One surface: its pixels in guest memory and how they are laid out.
#[derive(Debug, Clone, Default)]
struct Surface {
    width: u32,
    height: u32,
    bpp: u32,
    pitch: u32,
    buf: u32,
    caps: u32,
    /// The back buffer of a flipping chain, or the z-buffer attached.
    back: Option<u32>,
    zbuffer: Option<u32>,
    /// The source colour key (low, high).
    src_key: Option<(u32, u32)>,
    /// The guest object of this surface.
    object: u32,
    /// Bumped on every change, for Direct3D's texture cache.
    version: u32,
    /// Red, green, blue and alpha bit masks of the pixels (0 alpha: none).
    masks: [u32; 4],
    /// Widescreen: a surface of the screen is this many pixels wider on each side than the
    /// game knows it (see [`margin`]); 0 for every other surface.
    ox: u32,
}

/// What the renderer needs of a surface.
#[derive(Debug, Clone, Copy)]
pub struct SurfaceInfo {
    pub buf: u32,
    pub width: u32,
    pub height: u32,
    pub pitch: u32,
    pub bpp: u32,
    pub masks: [u32; 4],
    pub src_key: Option<(u32, u32)>,
}

/// The default masks of a depth: 565 for 16 bits, 8888 (no alpha) for 32.
fn default_masks(bpp: u32) -> [u32; 4] {
    match bpp {
        16 => [0xf800, 0x07e0, 0x001f, 0],
        _ => [0x00ff_0000, 0x0000_ff00, 0x0000_00ff, 0],
    }
}

/// A surface by its guest object, for the renderer.
pub fn info(m: Mem, obj: u32) -> Option<SurfaceInfo> {
    let s = surface(com::id_of(m, obj))?;
    Some(SurfaceInfo {
        buf: s.buf,
        width: s.width,
        height: s.height,
        pitch: s.pitch,
        bpp: s.bpp,
        masks: s.masks,
        src_key: s.src_key,
    })
}

/// The z-buffer attached to the surface object `obj`.
pub fn zbuffer_info(m: Mem, obj: u32) -> Option<SurfaceInfo> {
    let z = surface(com::id_of(m, obj))?.zbuffer?;
    let s = surface(z)?;
    Some(SurfaceInfo {
        buf: s.buf,
        width: s.width,
        height: s.height,
        pitch: s.pitch,
        bpp: s.bpp,
        masks: s.masks,
        src_key: None,
    })
}

impl Surface {
    fn bytes(&self) -> u32 {
        self.bpp / 8
    }

    /// The width the game knows.
    fn game_width(&self) -> u32 {
        self.width - 2 * self.ox
    }
}

struct State {
    surfaces: BTreeMap<u32, Surface>,
    next: u32,
    mode: (u32, u32, u32),
    /// The screen surfaces' margin now (see [`margin`]).
    margin: u32,
}

static STATE: Mutex<State> = Mutex::new(State {
    surfaces: BTreeMap::new(),
    next: 1,
    mode: (640, 480, 16),
    margin: 0,
});

/// Widescreen (`rc_platform::render::widescreen`): the surfaces of the screen (the primary
/// surface, its back buffer, z-buffers and render targets of the display mode's size) are wider
/// than the display mode by this many pixels on each side. The game keeps its display mode: it
/// sees them at that size, as the middle of what they are. A pointer from `Lock`, the
/// rectangles of blits and clears, and the screen positions of Direct3D's pre-transformed
/// vertices are moved by the margin, so what it draws lands in the middle and the frame shows
/// the margins beside it; a draw beyond the display mode's edges (a game patched to draw its
/// world wider) fills them. The margin follows the output's proportions; it changes between
/// frames (at `Flip`), the surfaces made again at the new width.
pub fn margin() -> u32 {
    st().margin
}

/// The size of the screen's surfaces: the display mode's with the margins.
pub fn screen_size() -> (u32, u32) {
    let s = st();
    (s.mode.0 + 2 * s.margin, s.mode.1)
}

/// The margin of the surface object `obj` (0 for one not of the screen).
pub fn margin_of(m: Mem, obj: u32) -> u32 {
    surface(com::id_of(m, obj)).map_or(0, |s| s.ox)
}

/// Whether a surface made with `caps` at `width` × `height` is one of the screen's.
fn of_screen(caps: u32, width: u32, height: u32, mode: (u32, u32, u32)) -> bool {
    caps & DDSCAPS_TEXTURE == 0
        && (width, height) == (mode.0, mode.1)
        && caps & (DDSCAPS_PRIMARYSURFACE | DDSCAPS_BACKBUFFER | DDSCAPS_ZBUFFER | DDSCAPS_3DDEVICE)
            != 0
}

/// Black margins on a surface just shown: what the game draws over them in the next frame (its
/// world, patched to) shows there, nothing else does (a picture of a screen the game covers
/// whole, a map, is not left beside it).
fn clear_margins(m: Mem, s: &Surface) {
    if s.ox == 0 {
        return;
    }
    let zero = vec![0u8; (s.ox * s.bytes()) as usize];
    let right = (s.width - s.ox) * s.bytes();
    for y in 0..s.height {
        m.write(s.buf + y * s.pitch, &zero);
        m.write(s.buf + y * s.pitch + right, &zero);
    }
    crate::d3d::forget_surface(s.buf);
}

/// Make the screen's surfaces again at the margin the output asks for now, their pictures kept
/// in the middle.
fn apply_margin(m: Mem) {
    let (gw, gh, _) = display_mode();
    let want = rc_platform::render::margin_for(gw, gh);
    let mut s = st();
    if want == s.margin {
        return;
    }
    s.margin = want;
    rc_runtime::log!("[ddraw] widescreen: a margin of {want} pixels on each side");
    let mut moved = Vec::new();
    for x in s.surfaces.values_mut().filter(|x| x.ox != want) {
        if !of_screen(x.caps, x.game_width(), x.height, (gw, gh, 0)) {
            continue;
        }
        let width = x.game_width() + 2 * want;
        let pitch = (width * x.bpp / 8 + 3) & !3;
        let buf = vm::heap_alloc(m, (pitch * x.height).max(4), true);
        let row = (x.game_width() * x.bytes()) as usize;
        let mut line = vec![0u8; row];
        for y in 0..x.height {
            m.read(x.buf + y * x.pitch + x.ox * x.bytes(), &mut line);
            m.write(buf + y * pitch + want * x.bytes(), &line);
        }
        moved.push(x.buf);
        vm::heap_free(x.buf);
        (x.width, x.pitch, x.buf, x.ox) = (width, pitch, buf, want);
        x.version = x.version.wrapping_add(1);
    }
    drop(s);
    for buf in moved {
        crate::d3d::forget_surface(buf);
    }
}

fn st() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().expect("ddraw")
}

fn surface(id: u32) -> Option<Surface> {
    st().surfaces.get(&id).cloned()
}

/// The pixel buffer and layout of the surface object `obj` (for `d3d`).
pub fn surface_of(m: Mem, obj: u32) -> Option<(u32, u32, u32, u32, u32)> {
    let s = surface(com::id_of(m, obj))?;
    Some((s.buf, s.width, s.height, s.pitch, s.bpp))
}

/// The z-buffer attached to the surface object `obj`, as [`surface_of`].
pub fn zbuffer_of(m: Mem, obj: u32) -> Option<(u32, u32, u32, u32, u32)> {
    let z = surface(com::id_of(m, obj))?.zbuffer?;
    let s = surface(z)?;
    Some((s.buf, s.width, s.height, s.pitch, s.bpp))
}

/// `IDirect3DDevice7::Load`: `src`'s `rect` (its whole area if 0) copied into `dst` at `point`
/// (the origin if 0). Returns false for an unknown surface.
pub fn load(m: Mem, dst: u32, point: u32, src: u32, rect: u32) -> bool {
    let (Some(d), Some(sr)) = (surface(com::id_of(m, dst)), surface(com::id_of(m, src))) else {
        return false;
    };
    let r = read_rect(m, rect, &sr);
    let (x, y) = if point != 0 {
        (m.ld32(point) as i32, m.ld32(point + 4) as i32)
    } else {
        (0, 0)
    };
    let x = x + d.ox as i32;
    let dr = Rect {
        l: x,
        t: y,
        r: x + r.r - r.l,
        b: y + r.b - r.t,
    };
    blit(m, &d, dr, &sr, r, None);
    let id = com::id_of(m, dst);
    if let Some(s) = st().surfaces.get_mut(&id) {
        s.version = s.version.wrapping_add(1);
    }
    true
}

/// How often the surface object `obj` was locked: a texture's content version.
pub fn version_of(m: Mem, obj: u32) -> u32 {
    surface(com::id_of(m, obj)).map_or(0, |s| s.version)
}

/// The display mode the game set.
pub fn display_mode() -> (u32, u32, u32) {
    st().mode
}

fn new_surface(m: Mem, width: u32, height: u32, bpp: u32, caps: u32, masks: [u32; 4]) -> u32 {
    let ox = if of_screen(caps, width, height, display_mode()) {
        margin()
    } else {
        0
    };
    let width = width + 2 * ox;
    let pitch = (width * bpp / 8 + 3) & !3;
    let buf = vm::heap_alloc(m, (pitch * height).max(4), true);
    let id = {
        let mut s = st();
        let id = s.next;
        s.next += 1;
        id
    };
    let obj = com::new_object(m, &SURFACE7, id);
    st().surfaces.insert(
        id,
        Surface {
            width,
            height,
            bpp,
            pitch,
            buf,
            caps,
            object: obj,
            masks,
            ox,
            ..Surface::default()
        },
    );
    obj
}

/// DDPIXELFORMAT of a surface depth at `p` (32 bytes).
fn put_pixel_format(m: Mem, p: u32, bpp: u32, zbuffer: bool, masks: [u32; 4]) {
    for i in (0..32).step_by(4) {
        m.st32(p + i, 0);
    }
    m.st32(p, 32);
    if zbuffer {
        m.st32(p + 4, 0x400); // DDPF_ZBUFFER
        m.st32(p + 12, bpp);
        m.st32(p + 20, if bpp == 16 { 0xffff } else { 0xffff_ffff });
        return;
    }
    let alpha = if masks[3] != 0 { 0x1 } else { 0 }; // DDPF_ALPHAPIXELS
    m.st32(p + 4, 0x40 | alpha); // DDPF_RGB
    m.st32(p + 12, bpp);
    m.st32(p + 16, masks[0]);
    m.st32(p + 20, masks[1]);
    m.st32(p + 24, masks[2]);
    m.st32(p + 28, masks[3]);
}

/// DDSURFACEDESC2 (124 bytes) or DDSURFACEDESC (108 bytes, the older interfaces; the same
/// layout without the extended caps and the texture stage) of `s` at `d`, by its `dwSize`.
fn put_desc(m: Mem, d: u32, s: &Surface, surface_ptr: u32) {
    let size = if m.ld32(d) == 108 { 108 } else { 124 };
    for i in (4..size).step_by(4) {
        m.st32(d + i, 0);
    }
    m.st32(d, size);
    let z = s.caps & DDSCAPS_ZBUFFER != 0;
    m.st32(
        d + 4,
        DDSD_CAPS | DDSD_HEIGHT | DDSD_WIDTH | DDSD_PITCH | DDSD_PIXELFORMAT | DDSD_LPSURFACE,
    );
    m.st32(d + 8, s.height);
    m.st32(d + 12, s.game_width());
    m.st32(d + 16, s.pitch);
    m.st32(d + 36, surface_ptr);
    let masks = if s.masks == [0; 4] {
        default_masks(s.bpp)
    } else {
        s.masks
    };
    put_pixel_format(m, d + 72, s.bpp, z, masks);
    m.st32(d + 104, s.caps);
}

// --- entry points --------------------------------------------------------------------------

fn DirectDrawEnumerateExA(c: &mut Cpu) {
    let m = c.mem();
    let (cb, ctx) = (c.arg(0), c.arg(1));
    let desc = vm::heap_alloc(m, 64, true);
    m.write(desc, b"Primary Display Driver\0");
    m.write(desc + 32, b"display\0");
    call_guest(c, cb, &[0, desc, desc + 32, ctx, 0]);
    ret_stdcall(c, S_OK, 3);
}

fn DirectDrawEnumerateA(c: &mut Cpu) {
    let m = c.mem();
    let (cb, ctx) = (c.arg(0), c.arg(1));
    let desc = vm::heap_alloc(m, 64, true);
    m.write(desc, b"Primary Display Driver\0");
    m.write(desc + 32, b"display\0");
    call_guest(c, cb, &[0, desc, desc + 32, ctx]);
    ret_stdcall(c, S_OK, 2);
}

fn new_ddraw(m: Mem) -> u32 {
    com::new_object(m, &DDRAW7, 0)
}

fn DirectDrawCreateEx(c: &mut Cpu) {
    let m = c.mem();
    let (out, iid) = (c.arg(1), c.arg(2));
    if com::read_guid(m, iid) != IID_IDIRECTDRAW7 {
        return ret_stdcall(c, E_NOINTERFACE, 4);
    }
    m.st32(out, new_ddraw(m));
    ret_stdcall(c, S_OK, 4);
}

fn DirectDrawCreate(c: &mut Cpu) {
    let m = c.mem();
    let out = c.arg(1);
    m.st32(out, com::new_object(m, &DDRAW1, 0));
    ret_stdcall(c, S_OK, 3);
}

// --- IDirectDraw7 --------------------------------------------------------------------------

fn dd_QueryInterface(c: &mut Cpu) {
    let m = c.mem();
    let (this, iid, out) = (c.arg(0), c.arg(1), c.arg(2));
    let g = com::read_guid(m, iid);
    let r = if g == IID_IDIRECTDRAW7 || g == IID_IDIRECTDRAW4 || g == IID_IDIRECTDRAW2 {
        // Versions 2, 4 and 7 share one vtable layout (each a prefix of the next).
        m.st32(out, com::new_object(m, &DDRAW7, 0));
        S_OK
    } else if g == IID_IDIRECTDRAW {
        m.st32(out, com::new_object(m, &DDRAW1, 0));
        S_OK
    } else if g == IID_IDIRECT3D7 {
        match crate::d3d::create(m, this) {
            Some(o) => {
                m.st32(out, o);
                S_OK
            }
            None => E_NOINTERFACE,
        }
    } else {
        rc_runtime::log!("[ddraw] IDirectDraw7::QueryInterface({g:02x?}): not provided");
        m.st32(out, 0);
        E_NOINTERFACE
    };
    ret_stdcall(c, r, 3);
}

fn ok1(c: &mut Cpu) {
    ret_stdcall(c, S_OK, 1);
}
fn ok2(c: &mut Cpu) {
    ret_stdcall(c, S_OK, 2);
}
fn ok3(c: &mut Cpu) {
    ret_stdcall(c, S_OK, 3);
}

/// IDirectDraw (version 1): width, height, depth; no refresh rate or flags.
fn dd_SetDisplayMode1(c: &mut Cpu) {
    let (w, h, bpp) = (c.arg(1), c.arg(2), c.arg(3));
    st().mode = (w, h, bpp);
    rc_runtime::log!("[ddraw] display mode {w}x{h}x{bpp}");
    ret_stdcall(c, S_OK, 4);
}

fn dd_SetDisplayMode(c: &mut Cpu) {
    let (w, h, bpp) = (c.arg(1), c.arg(2), c.arg(3));
    st().mode = (w, h, bpp);
    rc_runtime::log!("[ddraw] display mode {w}x{h}x{bpp}");
    ret_stdcall(c, S_OK, 6);
}

const MODES: &[(u32, u32)] = &[(640, 480), (800, 600), (1024, 768)];

fn dd_EnumDisplayModes(c: &mut Cpu) {
    let m = c.mem();
    let (ctx, cb) = (c.arg(3), c.arg(4));
    let d = vm::heap_alloc(m, 124, true);
    'modes: for &bpp in &[16u32, 32] {
        for &(w, h) in MODES {
            let s = Surface {
                width: w,
                height: h,
                bpp,
                pitch: w * bpp / 8,
                ..Surface::default()
            };
            put_desc(m, d, &s, 0);
            m.st32(
                d + 4,
                DDSD_WIDTH | DDSD_HEIGHT | DDSD_PITCH | DDSD_PIXELFORMAT | DDSD_REFRESHRATE,
            );
            m.st32(d + 24, 60);
            if call_guest(c, cb, &[d, ctx]) == 0 {
                break 'modes;
            }
        }
    }
    vm::heap_free(d);
    ret_stdcall(c, S_OK, 5);
}

fn dd_GetDisplayMode(c: &mut Cpu) {
    let m = c.mem();
    let d = c.arg(1);
    let (w, h, bpp) = display_mode();
    let s = Surface {
        width: w,
        height: h,
        bpp,
        pitch: w * bpp / 8,
        ..Surface::default()
    };
    put_desc(m, d, &s, 0);
    m.st32(d + 24, 60);
    ret_stdcall(c, S_OK, 2);
}

fn put_caps(m: Mem, p: u32) {
    if p == 0 {
        return;
    }
    let size = m.ld32(p).clamp(4, 380);
    for i in (4..size).step_by(4) {
        m.st32(p + i, 0);
    }
    // DDCAPS_3D | BLT | BLTSTRETCH | COLORKEY | BLTCOLORFILL
    m.st32(p + 4, 0x1 | 0x40 | 0x200 | 0x0040_0000 | 0x0400_0000);
    m.st32(p + 0x0c, 0x200); // DDCKEYCAPS_SRCBLT
    m.st32(p + 0x3c, 64 << 20); // video memory
    m.st32(p + 0x40, 60 << 20);
    m.st32(
        p + 0x68,
        DDSCAPS_3DDEVICE | DDSCAPS_TEXTURE | DDSCAPS_ZBUFFER | DDSCAPS_VIDEOMEMORY | DDSCAPS_FLIP,
    );
}

fn dd_GetCaps(c: &mut Cpu) {
    let m = c.mem();
    put_caps(m, c.arg(1));
    put_caps(m, c.arg(2));
    ret_stdcall(c, S_OK, 3);
}

fn dd_GetAvailableVidMem(c: &mut Cpu) {
    let m = c.mem();
    let (total, free) = (c.arg(2), c.arg(3));
    if total != 0 {
        m.st32(total, 64 << 20);
    }
    if free != 0 {
        m.st32(free, 60 << 20);
    }
    ret_stdcall(c, S_OK, 4);
}

fn dd_GetFourCCCodes(c: &mut Cpu) {
    let m = c.mem();
    let n = c.arg(1);
    if n != 0 {
        m.st32(n, 0);
    }
    ret_stdcall(c, S_OK, 3);
}

fn dd_GetMonitorFrequency(c: &mut Cpu) {
    let p = c.arg(1);
    c.mem().st32(p, 60);
    ret_stdcall(c, S_OK, 2);
}

fn dd_GetVerticalBlankStatus(c: &mut Cpu) {
    let p = c.arg(1);
    c.mem().st32(p, 1);
    ret_stdcall(c, S_OK, 2);
}

fn dd_GetScanLine(c: &mut Cpu) {
    let p = c.arg(1);
    c.mem().st32(p, 0);
    ret_stdcall(c, S_OK, 2);
}

fn dd_GetDeviceIdentifier(c: &mut Cpu) {
    let m = c.mem();
    let p = c.arg(1);
    for i in (0..1072).step_by(4) {
        m.st32(p + i, 0);
    }
    m.write(p, b"display\0");
    m.write(p + 512, b"TechnoMageRecomp (wgpu)\0");
    ret_stdcall(c, S_OK, 3);
}

fn dd_CreateClipper(c: &mut Cpu) {
    let m = c.mem();
    let out = c.arg(2);
    m.st32(out, com::new_object(m, &CLIPPER, 0));
    ret_stdcall(c, S_OK, 4);
}

fn dd_CreateSurface(c: &mut Cpu) {
    let m = c.mem();
    let (d, out) = (c.arg(1), c.arg(2));
    let flags = m.ld32(d + 4);
    let caps = if flags & DDSD_CAPS != 0 {
        m.ld32(d + 104)
    } else {
        0
    };
    let (mw, mh, mbpp) = display_mode();
    let primary = caps & DDSCAPS_PRIMARYSURFACE != 0;
    let (w, h) = if primary || flags & (DDSD_WIDTH | DDSD_HEIGHT) == 0 {
        (mw, mh)
    } else {
        (m.ld32(d + 12), m.ld32(d + 8))
    };
    let z = caps & DDSCAPS_ZBUFFER != 0;
    let bpp = if z {
        if flags & DDSD_ZBUFFERBITDEPTH != 0 {
            m.ld32(d + 24) // dwZBufferBitDepth shares the union of the refresh rate (DX6+: in the pixel format)
        } else {
            m.ld32(d + 72 + 12)
        }
        .clamp(16, 32)
    } else if flags & DDSD_PIXELFORMAT != 0 && m.ld32(d + 72 + 12) != 0 {
        m.ld32(d + 72 + 12)
    } else {
        mbpp
    };
    if w == 0 || h == 0 || w > 4096 || h > 4096 {
        m.st32(out, 0);
        return ret_stdcall(c, DDERR_INVALIDPARAMS, 4);
    }
    let caps = caps | DDSCAPS_VIDEOMEMORY | DDSCAPS_LOCALVIDMEM;
    let masks = if !z && flags & DDSD_PIXELFORMAT != 0 && m.ld32(d + 72 + 12) != 0 {
        [
            m.ld32(d + 88),
            m.ld32(d + 92),
            m.ld32(d + 96),
            if m.ld32(d + 76) & 1 != 0 {
                m.ld32(d + 100)
            } else {
                0
            },
        ]
    } else {
        default_masks(bpp)
    };
    let obj = new_surface(
        m,
        w,
        h,
        bpp,
        caps | if primary { DDSCAPS_FRONTBUFFER } else { 0 },
        masks,
    );
    if primary && caps & DDSCAPS_FLIP != 0 {
        let backs = if flags & DDSD_BACKBUFFERCOUNT != 0 {
            m.ld32(d + 20).max(1)
        } else {
            1
        };
        let _ = backs; // one back buffer serves any count: Flip copies it out
        let back = new_surface(
            m,
            w,
            h,
            bpp,
            (caps & !DDSCAPS_PRIMARYSURFACE) | DDSCAPS_BACKBUFFER | DDSCAPS_COMPLEX,
            masks,
        );
        let (pid, bid) = (com::id_of(m, obj), com::id_of(m, back));
        if let Some(s) = st().surfaces.get_mut(&pid) {
            s.back = Some(bid);
        }
    }
    m.st32(out, obj);
    ret_stdcall(c, S_OK, 4);
}

// --- IDirectDrawSurface7 -------------------------------------------------------------------

fn sid(c: &Cpu) -> u32 {
    com::id_of(c.mem(), c.arg(0))
}

fn s_QueryInterface(c: &mut Cpu) {
    let m = c.mem();
    let (this, iid, out) = (c.arg(0), c.arg(1), c.arg(2));
    let g = com::read_guid(m, iid);
    let r = if IID_SURFACES.contains(&g) {
        com::add_ref_raw(m, this);
        m.st32(out, this);
        S_OK
    } else {
        rc_runtime::log!("[ddraw] IDirectDrawSurface7::QueryInterface({g:02x?}): not provided");
        m.st32(out, 0);
        E_NOINTERFACE
    };
    ret_stdcall(c, r, 3);
}

fn s_GetAttachedSurface(c: &mut Cpu) {
    let m = c.mem();
    let (caps_p, out) = (c.arg(1), c.arg(2));
    let want = m.ld32(caps_p);
    let s = surface(sid(c));
    let found = s.and_then(|s| {
        if want & DDSCAPS_ZBUFFER != 0 {
            s.zbuffer
        } else {
            s.back
        }
    });
    match found.and_then(surface) {
        Some(b) => {
            com::add_ref_raw(m, b.object);
            m.st32(out, b.object);
            ret_stdcall(c, S_OK, 3);
        }
        None => {
            m.st32(out, 0);
            ret_stdcall(c, DDERR_NOTFOUND, 3);
        }
    }
}

fn s_AddAttachedSurface(c: &mut Cpu) {
    let m = c.mem();
    let other = com::id_of(m, c.arg(1));
    let id = sid(c);
    let is_z = surface(other).is_some_and(|s| s.caps & DDSCAPS_ZBUFFER != 0);
    if let Some(s) = st().surfaces.get_mut(&id) {
        if is_z {
            s.zbuffer = Some(other);
        } else {
            s.back = Some(other);
        }
    }
    ret_stdcall(c, S_OK, 2);
}

fn s_GetSurfaceDesc(c: &mut Cpu) {
    let m = c.mem();
    let d = c.arg(1);
    match surface(sid(c)) {
        Some(s) => {
            put_desc(m, d, &s, s.buf + s.ox * s.bytes());
            ret_stdcall(c, S_OK, 2);
        }
        None => ret_stdcall(c, DDERR_GENERIC, 2),
    }
}

fn s_GetPixelFormat(c: &mut Cpu) {
    let m = c.mem();
    let p = c.arg(1);
    match surface(sid(c)) {
        Some(s) => {
            put_pixel_format(m, p, s.bpp, s.caps & DDSCAPS_ZBUFFER != 0, s.masks);
            ret_stdcall(c, S_OK, 2);
        }
        None => ret_stdcall(c, DDERR_GENERIC, 2),
    }
}

fn s_GetCaps(c: &mut Cpu) {
    let m = c.mem();
    let p = c.arg(1);
    let caps = surface(sid(c)).map_or(0, |s| s.caps);
    m.st32(p, caps);
    for i in 1..4 {
        m.st32(p + 4 * i, 0);
    }
    ret_stdcall(c, S_OK, 2);
}

#[derive(Debug, Clone, Copy)]
struct Rect {
    l: i32,
    t: i32,
    r: i32,
    b: i32,
}

/// A rectangle the game gives (`p`; 0: the whole surface as the game knows it), on the
/// surface's pixels (moved by its margin).
fn read_rect(m: Mem, p: u32, s: &Surface) -> Rect {
    let ox = s.ox as i32;
    if p == 0 {
        return Rect {
            l: ox,
            t: 0,
            r: ox + s.game_width() as i32,
            b: s.height as i32,
        };
    }
    Rect {
        l: m.ld32(p) as i32 + ox,
        t: m.ld32(p + 4) as i32,
        r: m.ld32(p + 8) as i32 + ox,
        b: m.ld32(p + 12) as i32,
    }
}

/// The whole of a surface, margins included.
fn whole(s: &Surface) -> Rect {
    Rect {
        l: 0,
        t: 0,
        r: s.width as i32,
        b: s.height as i32,
    }
}

fn s_Lock(c: &mut Cpu) {
    let m = c.mem();
    let (rect, d) = (c.arg(1), c.arg(2));
    crate::d3d::flush_if_target(m, c.arg(0));
    let id = sid(c);
    let Some(s) = surface(id) else {
        return ret_stdcall(c, DDERR_GENERIC, 5);
    };
    let r = read_rect(m, rect, &s);
    let ptr = s.buf + r.t.max(0) as u32 * s.pitch + r.l.max(0) as u32 * s.bytes();
    put_desc(m, d, &s, ptr);
    if let Some(x) = st().surfaces.get_mut(&id) {
        x.version = x.version.wrapping_add(1);
    }
    ret_stdcall(c, S_OK, 5);
}

/// A picture on the primary surface (x, y, width, height as the game knows the surface).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Picture {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    /// Letterboxed: the game shows it lower than the screen, with black above and below. Only
    /// such a picture is cut to fill the window (`fill_screen`); a full-frame one is shown
    /// whole, boxed if it must be.
    pub letterboxed: bool,
}

/// Where a picture on the primary surface is, while one is shown: a video. A game patch
/// tells it.
pub type PictureFn = fn(Mem) -> Option<Picture>;

static PICTURE: std::sync::OnceLock<PictureFn> = std::sync::OnceLock::new();

/// With widescreen, a frame of the primary surface shows only the picture `f` says is on it
/// (the platform enlarges it to the window, proportions kept): a video made wider than 4:3
/// fills a wide window instead of being boxed in black on all sides.
pub fn set_picture_fn(f: PictureFn) {
    let _ = PICTURE.set(f);
}

/// Draws over a frame before it is shown (a game patch's): the frame's RGBA, its size, and how
/// many of its pixels one of the screen surface's is (more with native resolution).
pub type OverlayFn = fn(Mem, &mut [u8], u32, u32, f32);

static OVERLAY: std::sync::OnceLock<OverlayFn> = std::sync::OnceLock::new();

/// Pictures of a game patch's over each frame of the screen (in the screen surface's pixels,
/// margins included, scaled with the frame).
pub fn set_overlay_fn(f: OverlayFn) {
    let _ = OVERLAY.set(f);
}

fn overlay(m: Mem, rgba: &mut [u8], w: u32, h: u32, surface_w: u32) {
    if let Some(f) = OVERLAY.get() {
        f(m, rgba, w, h, w as f32 / surface_w.max(1) as f32);
    }
}

/// A letterboxed picture (`x`, `y`, `w`, `h`) cut to the screen's proportions (`sw` × `sh`,
/// margins included) by at most an eighth of its width or height, so a video a little wider or
/// narrower than the window fills it instead of leaving black bars (a 16:10 window and a
/// 640 × 368 video: 8 % off its sides).
fn fill_screen(x: u32, y: u32, w: u32, h: u32, sw: u32, sh: u32) -> (u32, u32, u32, u32) {
    let (wide, tall) = (u64::from(w) * u64::from(sh), u64::from(h) * u64::from(sw));
    if wide > tall {
        let cw = ((u64::from(h) * u64::from(sw) / u64::from(sh.max(1))) as u32).max(w - w / 8);
        (x + (w - cw) / 2, y, cw, h)
    } else if tall > wide {
        let ch = ((u64::from(w) * u64::from(sh) / u64::from(sw.max(1))) as u32).max(h - h / 8);
        (x, y + (h - ch) / 2, w, ch)
    } else {
        (x, y, w, h)
    }
}

/// The primary surface is what the screen shows: a change to it (a video written by `Lock`,
/// a `Blt` onto it) is a frame.
fn present_if_primary(c: &mut Cpu, id: u32) {
    let Some(s) = surface(id) else {
        return;
    };
    if s.caps & DDSCAPS_PRIMARYSURFACE == 0 {
        return;
    }
    let m = c.mem();
    let rgba = to_rgba(m, &s);
    let picture = PICTURE
        .get()
        .filter(|_| rc_platform::render::widescreen())
        .and_then(|f| f(m))
        .filter(|p| p.w > 0 && p.h > 0 && p.x + p.w <= s.game_width() && p.y + p.h <= s.height);
    if let Some(p) = picture {
        let (x, y, w, h) = if p.letterboxed {
            fill_screen(p.x, p.y, p.w, p.h, s.width, s.height)
        } else {
            (p.x, p.y, p.w, p.h)
        };
        let mut out = Vec::with_capacity((w * h * 4) as usize);
        for r in y..y + h {
            let at = ((r * s.width + s.ox + x) * 4) as usize;
            out.extend_from_slice(&rgba[at..at + (w * 4) as usize]);
        }
        rc_platform::submit_frame(w, h, out);
        apply_margin(m);
        crate::frame_done(c);
        return;
    }
    rc_platform::submit_frame(s.width, s.height, rgba);
    apply_margin(c.mem());
    crate::frame_done(c);
}

fn s_Unlock(c: &mut Cpu) {
    let id = sid(c);
    present_if_primary(c, id);
    ret_stdcall(c, S_OK, 2);
}

fn read_px(m: Mem, s: &Surface, x: u32, y: u32) -> u32 {
    let a = s.buf + y * s.pitch + x * s.bytes();
    match s.bpp {
        16 => u32::from(m.ld16(a)),
        8 => u32::from(m.ld8(a)),
        _ => m.ld32(a),
    }
}

fn write_px(m: Mem, s: &Surface, x: u32, y: u32, v: u32) {
    let a = s.buf + y * s.pitch + x * s.bytes();
    match s.bpp {
        16 => m.st16(a, v as u16),
        8 => m.st8(a, v as u8),
        _ => m.st32(a, v),
    }
}

/// Copy `src`'s `sr` into `dst`'s `dr`, scaled by nearest pixel, skipping the source key.
fn blit(m: Mem, dst: &Surface, dr: Rect, src: &Surface, sr: Rect, key: Option<(u32, u32)>) {
    let (dw, dh) = ((dr.r - dr.l).max(0), (dr.b - dr.t).max(0));
    let (sw, sh) = ((sr.r - sr.l).max(0), (sr.b - sr.t).max(0));
    if dw == 0 || dh == 0 || sw == 0 || sh == 0 {
        return;
    }
    // Same size and format, no key: whole rows at once.
    let plain = key.is_none() && dw == sw && dh == sh && dst.bpp == src.bpp;
    for y in 0..dh {
        let dy = dr.t + y;
        if dy < 0 || dy >= dst.height as i32 {
            continue;
        }
        let sy = sr.t + y * sh / dh;
        if sy < 0 || sy >= src.height as i32 {
            continue;
        }
        if plain && dr.l >= 0 && dr.r <= dst.width as i32 && sr.l >= 0 && sr.r <= src.width as i32 {
            let n = (dw as u32 * dst.bytes()) as usize;
            let mut row = vec![0u8; n];
            m.read(
                src.buf + sy as u32 * src.pitch + sr.l as u32 * src.bytes(),
                &mut row,
            );
            m.write(
                dst.buf + dy as u32 * dst.pitch + dr.l as u32 * dst.bytes(),
                &row,
            );
            continue;
        }
        for x in 0..dw {
            let dx = dr.l + x;
            let sx = sr.l + x * sw / dw;
            if dx < 0 || dx >= dst.width as i32 || sx < 0 || sx >= src.width as i32 {
                continue;
            }
            let v = read_px(m, src, sx as u32, sy as u32);
            if let Some((lo, hi)) = key {
                if v >= lo && v <= hi {
                    continue;
                }
            }
            write_px(m, dst, dx as u32, dy as u32, v);
        }
    }
}

fn fill(m: Mem, dst: &Surface, r: Rect, v: u32) {
    for y in r.t.max(0)..r.b.min(dst.height as i32) {
        for x in r.l.max(0)..r.r.min(dst.width as i32) {
            write_px(m, dst, x as u32, y as u32, v);
        }
    }
}

const DDBLT_COLORFILL: u32 = 0x400;
const DDBLT_KEYSRC: u32 = 0x8000;
const DDBLT_KEYSRCOVERRIDE: u32 = 0x10000;
const DDBLT_DEPTHFILL: u32 = 0x0200_0000;

fn s_Blt(c: &mut Cpu) {
    let m = c.mem();
    let (drect, src_obj, srect, flags, fx) = (c.arg(1), c.arg(2), c.arg(3), c.arg(4), c.arg(5));
    crate::d3d::flush_if_target(m, c.arg(0));
    crate::d3d::flush_if_target(m, src_obj);
    let Some(dst) = surface(sid(c)) else {
        return ret_stdcall(c, DDERR_GENERIC, 6);
    };
    let mut dr = read_rect(m, drect, &dst);
    if flags & (DDBLT_COLORFILL | DDBLT_DEPTHFILL) != 0 {
        if drect == 0 {
            dr = whole(&dst); // the margins too
        }
        let v = if fx != 0 { m.ld32(fx + 0x50) } else { 0 };
        fill(m, &dst, dr, v);
        return ret_stdcall(c, S_OK, 6);
    }
    let Some(src) = (src_obj != 0)
        .then(|| surface(com::id_of(m, src_obj)))
        .flatten()
    else {
        return ret_stdcall(c, DDERR_INVALIDPARAMS, 6);
    };
    let mut sr = read_rect(m, srect, &src);
    if drect == 0 && srect == 0 && dst.ox == src.ox && dst.width == src.width {
        (dr, sr) = (whole(&dst), whole(&src)); // one screen surface onto another, margins too
    }
    let key = if flags & DDBLT_KEYSRCOVERRIDE != 0 && fx != 0 {
        Some((m.ld32(fx + 0x68), m.ld32(fx + 0x6c)))
    } else if flags & DDBLT_KEYSRC != 0 {
        src.src_key
    } else {
        None
    };
    blit(m, &dst, dr, &src, sr, key);
    let id = sid(c);
    present_if_primary(c, id);
    ret_stdcall(c, S_OK, 6);
}

fn s_BltFast(c: &mut Cpu) {
    let m = c.mem();
    crate::d3d::flush_if_target(m, c.arg(0));
    crate::d3d::flush_if_target(m, c.arg(3));
    let (x, y, src_obj, srect, trans) = (
        c.arg(1) as i32,
        c.arg(2) as i32,
        c.arg(3),
        c.arg(4),
        c.arg(5),
    );
    let (Some(dst), Some(src)) = (surface(sid(c)), surface(com::id_of(m, src_obj))) else {
        return ret_stdcall(c, DDERR_GENERIC, 6);
    };
    let sr = read_rect(m, srect, &src);
    let x = x + dst.ox as i32;
    let dr = Rect {
        l: x,
        t: y,
        r: x + sr.r - sr.l,
        b: y + sr.b - sr.t,
    };
    let key = if trans & 1 != 0 { src.src_key } else { None };
    blit(m, &dst, dr, &src, sr, key);
    ret_stdcall(c, S_OK, 6);
}

fn s_SetColorKey(c: &mut Cpu) {
    let m = c.mem();
    let (flags, ck) = (c.arg(1), c.arg(2));
    let id = sid(c);
    if flags & 0x8 != 0 {
        // DDCKEY_SRCBLT
        let key = (ck != 0).then(|| (m.ld32(ck), m.ld32(ck + 4)));
        if let Some(s) = st().surfaces.get_mut(&id) {
            s.src_key = key;
        }
    }
    ret_stdcall(c, S_OK, 3);
}

fn s_GetColorKey(c: &mut Cpu) {
    let m = c.mem();
    let ck = c.arg(2);
    match surface(sid(c)).and_then(|s| s.src_key) {
        Some((lo, hi)) => {
            m.st32(ck, lo);
            m.st32(ck + 4, hi);
            ret_stdcall(c, S_OK, 3);
        }
        None => ret_stdcall(c, 0x8876_0186, 3), // DDERR_NOCOLORKEY
    }
}

/// The surface as RGBA8, through the gamma ramp.
fn to_rgba(m: Mem, s: &Surface) -> Vec<u8> {
    let mut out = vec![0u8; (s.width * s.height * 4) as usize];
    if s.width == 0 {
        return out;
    }
    let gamma = crate::small::gamma_ramp();
    let g = |i: usize, v: u8| -> u8 {
        match &gamma {
            Some(r) => (r[i * 256 + usize::from(v)] >> 8) as u8,
            None => v,
        }
    };
    let mut row = vec![0u8; s.pitch as usize];
    if s.bpp == 16 {
        let table = rgb565_table(gamma.as_deref());
        for (y, line) in out.chunks_exact_mut(4 * s.width as usize).enumerate() {
            m.read(s.buf + y as u32 * s.pitch, &mut row);
            for (p, v) in line
                .as_chunks_mut::<4>()
                .0
                .iter_mut()
                .zip(row.as_chunks::<2>().0)
            {
                *p = table[usize::from(u16::from_le_bytes(*v))];
            }
        }
        return out;
    }
    for (y, line) in out.chunks_exact_mut(4 * s.width as usize).enumerate() {
        m.read(s.buf + y as u32 * s.pitch, &mut row);
        for (p, v) in line
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(row.as_chunks::<4>().0)
        {
            *p = [g(0, v[2]), g(1, v[1]), g(2, v[0]), 255];
        }
    }
    out
}

/// Every RGB565 pixel as RGBA8 through the gamma ramp, made again only when the ramp changes.
fn rgb565_table(gamma: Option<&[u16]>) -> std::sync::Arc<Vec<[u8; 4]>> {
    type Cached = Option<(Option<Vec<u16>>, std::sync::Arc<Vec<[u8; 4]>>)>;
    static TABLE: std::sync::Mutex<Cached> = std::sync::Mutex::new(None);
    let mut t = TABLE.lock().expect("rgb565");
    if let Some((g, table)) = t.as_ref() {
        if g.as_deref() == gamma {
            return table.clone();
        }
    }
    let g = |i: usize, v: u8| -> u8 {
        match gamma {
            Some(r) => (r[i * 256 + usize::from(v)] >> 8) as u8,
            None => v,
        }
    };
    let table = std::sync::Arc::new(
        (0..=u16::MAX)
            .map(|v| {
                let r = ((v >> 11) & 31) as u8;
                let gg = ((v >> 5) & 63) as u8;
                let b = (v & 31) as u8;
                [
                    g(0, (r << 3) | (r >> 2)),
                    g(1, (gg << 2) | (gg >> 4)),
                    g(2, (b << 3) | (b >> 2)),
                    255,
                ]
            })
            .collect::<Vec<_>>(),
    );
    *t = Some((gamma.map(<[u16]>::to_vec), table.clone()));
    table
}

fn s_Flip(c: &mut Cpu) {
    let m = c.mem();
    let Some(front) = surface(sid(c)) else {
        return ret_stdcall(c, DDERR_GENERIC, 3);
    };
    if let Some(back) = front.back.and_then(surface) {
        crate::d3d::flush_if_target(m, back.object);
    }
    let front_buf = (front.buf, front.pitch, front.height);
    let shown = front.back.and_then(surface).unwrap_or(front);
    // The frame's Direct3D commands go to the renderer once it exists (M5); for now they are
    // counted and dropped.
    let commands = crate::d3d::take_frame();
    if rc_runtime::host::env("RC_TRACE").is_some_and(|v| v.contains("d3d")) {
        rc_runtime::log!("[d3d] frame: {} commands", commands.len());
    }
    match info(m, shown.object).and_then(|i| crate::d3d::native_picture(m, &i)) {
        Some((w, h, mut big)) => {
            if let Some(r) = crate::small::gamma_ramp() {
                for px in big.as_chunks_mut::<4>().0 {
                    for (i, v) in px.iter_mut().take(3).enumerate() {
                        *v = (r[i * 256 + usize::from(*v)] >> 8) as u8;
                    }
                }
            }
            overlay(m, &mut big, w, h, shown.width);
            rc_platform::submit_frame_scaled((shown.width, shown.height), w, h, big);
        }
        None => {
            let mut rgba = to_rgba(m, &shown);
            overlay(m, &mut rgba, shown.width, shown.height, shown.width);
            rc_platform::submit_frame(shown.width, shown.height, rgba);
        }
    }
    // The front surface now holds what is shown, as after a real flip: a game that reads it
    // (TechnoMage's saved games take their picture from it) finds the frame there.
    if shown.buf != front_buf.0 && (shown.pitch, shown.height) == (front_buf.1, front_buf.2) {
        let mut frame = vec![0u8; (shown.pitch * shown.height) as usize];
        m.read(shown.buf, &mut frame);
        m.write(front_buf.0, &frame);
    }
    clear_margins(m, &shown);
    apply_margin(m);
    crate::frame_done(c);
    ret_stdcall(c, S_OK, 3);
}

fn s_IsLost(c: &mut Cpu) {
    ret_stdcall(c, S_OK, 1);
}

fn s_GetDDInterface(c: &mut Cpu) {
    let m = c.mem();
    let out = c.arg(1);
    m.st32(out, new_ddraw(m));
    ret_stdcall(c, S_OK, 2);
}

fn s_GetUniquenessValue(c: &mut Cpu) {
    let m = c.mem();
    let out = c.arg(1);
    let v = surface(sid(c)).map_or(0, |s| s.version);
    m.st32(out, v);
    ret_stdcall(c, S_OK, 2);
}

fn s_GetPriority(c: &mut Cpu) {
    c.mem().st32(c.arg(1), 0);
    ret_stdcall(c, S_OK, 2);
}

fn unsupported3(c: &mut Cpu) {
    ret_stdcall(c, DDERR_UNSUPPORTED, 3);
}

// --- interfaces ----------------------------------------------------------------------------

static DDRAW7: Interface = Interface {
    dll: "ddraw.dll",
    name: "IDirectDraw7",
    methods: &[
        ("QueryInterface", Some(dd_QueryInterface)),
        ("AddRef", Some(com::add_ref)),
        ("Release", Some(com::release)),
        ("Compact", Some(ok1)),
        ("CreateClipper", Some(dd_CreateClipper)),
        ("CreatePalette", None),
        ("CreateSurface", Some(dd_CreateSurface)),
        ("DuplicateSurface", None),
        ("EnumDisplayModes", Some(dd_EnumDisplayModes)),
        ("EnumSurfaces", None),
        ("FlipToGDISurface", Some(ok1)),
        ("GetCaps", Some(dd_GetCaps)),
        ("GetDisplayMode", Some(dd_GetDisplayMode)),
        ("GetFourCCCodes", Some(dd_GetFourCCCodes)),
        ("GetGDISurface", None),
        ("GetMonitorFrequency", Some(dd_GetMonitorFrequency)),
        ("GetScanLine", Some(dd_GetScanLine)),
        ("GetVerticalBlankStatus", Some(dd_GetVerticalBlankStatus)),
        ("Initialize", Some(ok2)),
        ("RestoreDisplayMode", Some(ok1)),
        ("SetCooperativeLevel", Some(ok3)),
        ("SetDisplayMode", Some(dd_SetDisplayMode)),
        ("WaitForVerticalBlank", Some(ok3)),
        ("GetAvailableVidMem", Some(dd_GetAvailableVidMem)),
        ("GetSurfaceFromDC", None),
        ("RestoreAllSurfaces", Some(ok1)),
        ("TestCooperativeLevel", Some(ok1)),
        ("GetDeviceIdentifier", Some(dd_GetDeviceIdentifier)),
        ("StartModeTest", Some(unsupported3)),
        ("EvaluateMode", None),
    ],
};

/// IDirectDraw (version 1): version 7's first 23 methods, `SetDisplayMode` with three
/// arguments.
static DDRAW1: Interface = Interface {
    dll: "ddraw.dll",
    name: "IDirectDraw",
    methods: &[
        ("QueryInterface", Some(dd_QueryInterface)),
        ("AddRef", Some(com::add_ref)),
        ("Release", Some(com::release)),
        ("Compact", Some(ok1)),
        ("CreateClipper", Some(dd_CreateClipper)),
        ("CreatePalette", None),
        ("CreateSurface", Some(dd_CreateSurface)),
        ("DuplicateSurface", None),
        ("EnumDisplayModes", Some(dd_EnumDisplayModes)),
        ("EnumSurfaces", None),
        ("FlipToGDISurface", Some(ok1)),
        ("GetCaps", Some(dd_GetCaps)),
        ("GetDisplayMode", Some(dd_GetDisplayMode)),
        ("GetFourCCCodes", Some(dd_GetFourCCCodes)),
        ("GetGDISurface", None),
        ("GetMonitorFrequency", Some(dd_GetMonitorFrequency)),
        ("GetScanLine", Some(dd_GetScanLine)),
        ("GetVerticalBlankStatus", Some(dd_GetVerticalBlankStatus)),
        ("Initialize", Some(ok2)),
        ("RestoreDisplayMode", Some(ok1)),
        ("SetCooperativeLevel", Some(ok3)),
        ("SetDisplayMode", Some(dd_SetDisplayMode1)),
        ("WaitForVerticalBlank", Some(ok3)),
    ],
};

static SURFACE7: Interface = Interface {
    dll: "ddraw.dll",
    name: "IDirectDrawSurface7",
    methods: &[
        ("QueryInterface", Some(s_QueryInterface)),
        ("AddRef", Some(com::add_ref)),
        ("Release", Some(com::release)),
        ("AddAttachedSurface", Some(s_AddAttachedSurface)),
        ("AddOverlayDirtyRect", None),
        ("Blt", Some(s_Blt)),
        ("BltBatch", None),
        ("BltFast", Some(s_BltFast)),
        ("DeleteAttachedSurface", Some(ok3)),
        ("EnumAttachedSurfaces", None),
        ("EnumOverlayZOrders", None),
        ("Flip", Some(s_Flip)),
        ("GetAttachedSurface", Some(s_GetAttachedSurface)),
        ("GetBltStatus", Some(ok2)),
        ("GetCaps", Some(s_GetCaps)),
        ("GetClipper", None),
        ("GetColorKey", Some(s_GetColorKey)),
        ("GetDC", None),
        ("GetFlipStatus", Some(ok2)),
        ("GetOverlayPosition", None),
        ("GetPalette", None),
        ("GetPixelFormat", Some(s_GetPixelFormat)),
        ("GetSurfaceDesc", Some(s_GetSurfaceDesc)),
        ("Initialize", None),
        ("IsLost", Some(s_IsLost)),
        ("Lock", Some(s_Lock)),
        ("ReleaseDC", None),
        ("Restore", Some(ok1)),
        ("SetClipper", Some(ok2)),
        ("SetColorKey", Some(s_SetColorKey)),
        ("SetOverlayPosition", None),
        ("SetPalette", Some(ok2)),
        ("Unlock", Some(s_Unlock)),
        ("UpdateOverlay", None),
        ("UpdateOverlayDisplay", None),
        ("UpdateOverlayZOrder", None),
        ("GetDDInterface", Some(s_GetDDInterface)),
        ("PageLock", Some(ok2)),
        ("PageUnlock", Some(ok2)),
        ("SetSurfaceDesc", None),
        ("SetPrivateData", None),
        ("GetPrivateData", None),
        ("FreePrivateData", None),
        ("GetUniquenessValue", Some(s_GetUniquenessValue)),
        ("ChangeUniquenessValue", Some(ok1)),
        ("SetPriority", Some(ok2)),
        ("GetPriority", Some(s_GetPriority)),
        ("SetLOD", Some(ok2)),
        ("GetLOD", None),
    ],
};

static CLIPPER: Interface = Interface {
    dll: "ddraw.dll",
    name: "IDirectDrawClipper",
    methods: &[
        ("QueryInterface", None),
        ("AddRef", Some(com::add_ref)),
        ("Release", Some(com::release)),
        ("GetClipList", None),
        ("GetHWnd", None),
        ("Initialize", Some(ok3)),
        ("IsClipListChanged", None),
        ("SetClipList", Some(ok3)),
        ("SetHWnd", Some(ok3)),
    ],
};

const CLSID_DIRECTDRAW: [u8; 16] = com::guid(
    0xd7b7_0ee0,
    0x4340,
    0x11cf,
    [0xb0, 0x63, 0x00, 0x20, 0xaf, 0xc2, 0xcd, 0x35],
);

/// `CoCreateInstance(CLSID_DirectDraw)`: the object the game then initialises.
fn create(c: &mut Cpu, iid: [u8; 16]) -> u32 {
    let m = c.mem();
    if iid == IID_IDIRECTDRAW {
        com::new_object(m, &DDRAW1, 0)
    } else {
        com::new_object(m, &DDRAW7, 0)
    }
}

pub fn register() {
    crate::small::register_class(CLSID_DIRECTDRAW, create);
    imports::register(
        "ddraw.dll",
        &[
            (
                "DirectDrawEnumerateExA",
                DirectDrawEnumerateExA as rc_runtime::dispatch::GuestFn,
            ),
            ("DirectDrawEnumerateA", DirectDrawEnumerateA),
            ("DirectDrawCreateEx", DirectDrawCreateEx),
            ("DirectDrawCreate", DirectDrawCreate),
        ],
    );
}
