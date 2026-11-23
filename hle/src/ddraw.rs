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
}

impl Surface {
    fn bytes(&self) -> u32 {
        self.bpp / 8
    }
}

struct State {
    surfaces: BTreeMap<u32, Surface>,
    next: u32,
    mode: (u32, u32, u32),
}

static STATE: Mutex<State> = Mutex::new(State {
    surfaces: BTreeMap::new(),
    next: 1,
    mode: (640, 480, 16),
});

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

/// How often the surface object `obj` was locked: a texture's content version.
pub fn version_of(m: Mem, obj: u32) -> u32 {
    surface(com::id_of(m, obj)).map_or(0, |s| s.version)
}

/// The display mode the game set.
pub fn display_mode() -> (u32, u32, u32) {
    st().mode
}

fn new_surface(m: Mem, width: u32, height: u32, bpp: u32, caps: u32) -> u32 {
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
            ..Surface::default()
        },
    );
    obj
}

/// DDPIXELFORMAT of a surface depth at `p` (32 bytes).
fn put_pixel_format(m: Mem, p: u32, bpp: u32, zbuffer: bool) {
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
    m.st32(p + 4, 0x40); // DDPF_RGB
    m.st32(p + 12, bpp);
    let (r, g, b) = match bpp {
        16 => (0xf800, 0x07e0, 0x001f),
        _ => (0x00ff_0000, 0x0000_ff00, 0x0000_00ff),
    };
    m.st32(p + 16, r);
    m.st32(p + 20, g);
    m.st32(p + 24, b);
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
    m.st32(d + 12, s.width);
    m.st32(d + 16, s.pitch);
    m.st32(d + 36, surface_ptr);
    put_pixel_format(m, d + 72, s.bpp, z);
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
        eprintln!("[ddraw] IDirectDraw7::QueryInterface({g:02x?}): not provided");
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
    eprintln!("[ddraw] display mode {w}x{h}x{bpp}");
    ret_stdcall(c, S_OK, 4);
}

fn dd_SetDisplayMode(c: &mut Cpu) {
    let (w, h, bpp) = (c.arg(1), c.arg(2), c.arg(3));
    st().mode = (w, h, bpp);
    eprintln!("[ddraw] display mode {w}x{h}x{bpp}");
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
    let obj = new_surface(
        m,
        w,
        h,
        bpp,
        caps | if primary { DDSCAPS_FRONTBUFFER } else { 0 },
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
        eprintln!("[ddraw] IDirectDrawSurface7::QueryInterface({g:02x?}): not provided");
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
            put_desc(m, d, &s, s.buf);
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
            put_pixel_format(m, p, s.bpp, s.caps & DDSCAPS_ZBUFFER != 0);
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

fn read_rect(m: Mem, p: u32, s: &Surface) -> Rect {
    if p == 0 {
        return Rect {
            l: 0,
            t: 0,
            r: s.width as i32,
            b: s.height as i32,
        };
    }
    Rect {
        l: m.ld32(p) as i32,
        t: m.ld32(p + 4) as i32,
        r: m.ld32(p + 8) as i32,
        b: m.ld32(p + 12) as i32,
    }
}

fn s_Lock(c: &mut Cpu) {
    let m = c.mem();
    let (rect, d) = (c.arg(1), c.arg(2));
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

fn s_Unlock(c: &mut Cpu) {
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
    let Some(dst) = surface(sid(c)) else {
        return ret_stdcall(c, DDERR_GENERIC, 6);
    };
    let dr = read_rect(m, drect, &dst);
    if flags & (DDBLT_COLORFILL | DDBLT_DEPTHFILL) != 0 {
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
    let sr = read_rect(m, srect, &src);
    let key = if flags & DDBLT_KEYSRCOVERRIDE != 0 && fx != 0 {
        Some((m.ld32(fx + 0x68), m.ld32(fx + 0x6c)))
    } else if flags & DDBLT_KEYSRC != 0 {
        src.src_key
    } else {
        None
    };
    blit(m, &dst, dr, &src, sr, key);
    ret_stdcall(c, S_OK, 6);
}

fn s_BltFast(c: &mut Cpu) {
    let m = c.mem();
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

/// The surface as RGBA8.
fn to_rgba(m: Mem, s: &Surface) -> Vec<u8> {
    let mut out = Vec::with_capacity((s.width * s.height * 4) as usize);
    let gamma = crate::small::gamma_ramp();
    let g = |i: usize, v: u8| -> u8 {
        match &gamma {
            Some(r) => (r[i * 256 + usize::from(v)] >> 8) as u8,
            None => v,
        }
    };
    let mut row = vec![0u8; s.pitch as usize];
    for y in 0..s.height {
        m.read(s.buf + y * s.pitch, &mut row);
        for x in 0..s.width as usize {
            let (r, gr, b) = match s.bpp {
                16 => {
                    let v = u16::from_le_bytes([row[2 * x], row[2 * x + 1]]);
                    let r = ((v >> 11) & 31) as u8;
                    let gg = ((v >> 5) & 63) as u8;
                    let b = (v & 31) as u8;
                    (
                        (r << 3) | (r >> 2),
                        (gg << 2) | (gg >> 4),
                        (b << 3) | (b >> 2),
                    )
                }
                _ => (row[4 * x + 2], row[4 * x + 1], row[4 * x]),
            };
            out.extend_from_slice(&[g(0, r), g(1, gr), g(2, b), 255]);
        }
    }
    out
}

fn s_Flip(c: &mut Cpu) {
    let m = c.mem();
    let Some(front) = surface(sid(c)) else {
        return ret_stdcall(c, DDERR_GENERIC, 3);
    };
    let shown = front.back.and_then(surface).unwrap_or(front);
    let rgba = to_rgba(m, &shown);
    // The frame's Direct3D commands go to the renderer once it exists (M5); for now they are
    // counted and dropped.
    let commands = crate::d3d::take_frame();
    if std::env::var("RC_TRACE").is_ok_and(|v| v.contains("d3d")) {
        eprintln!("[d3d] frame: {} commands", commands.len());
    }
    rc_platform::submit_frame(shown.width, shown.height, rgba);
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

pub fn register() {
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
