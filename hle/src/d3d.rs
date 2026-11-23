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
}

static G: Mutex<Global> = Mutex::new(Global {
    devices: BTreeMap::new(),
    buffers: BTreeMap::new(),
    next: 1,
    frame: Vec::new(),
});

fn g() -> std::sync::MutexGuard<'static, Global> {
    G.lock().expect("d3d")
}

fn next_id() -> u32 {
    let mut s = g();
    let id = s.next;
    s.next += 1;
    id
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

fn record(c: &Cpu, primitive: u32, fvf: u32, vertices: Vec<u8>, indices: Vec<u16>) {
    let id = com::id_of(c.mem(), c.arg(0));
    let mut s = g();
    let Some(d) = s.devices.get_mut(&id) else {
        return;
    };
    let state = snapshot(d);
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
    if rects.is_empty() {
        rects.push([
            vp[0] as i32,
            vp[1] as i32,
            (vp[0] + vp[2]) as i32,
            (vp[1] + vp[3]) as i32,
        ]);
    }
    if flags & 1 != 0 {
        if let Some((buf, w, h, pitch, bpp)) = ddraw::surface_of(m, target) {
            let v = match bpp {
                16 => {
                    let (r, gg, b) = ((color >> 16) & 0xff, (color >> 8) & 0xff, color & 0xff);
                    ((r >> 3) << 11) | ((gg >> 2) << 5) | (b >> 3)
                }
                _ => color,
            };
            fill(m, buf, w, h, pitch, bpp, &rects, v);
        }
    }
    if flags & 2 != 0 {
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
        ("Load", None),
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
