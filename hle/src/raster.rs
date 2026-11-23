// SPDX-License-Identifier: MIT
//! A software rasterizer for Direct3D 7's fixed-function pipeline: the reference renderer and
//! the one the null backend uses. It draws straight into the render target and z-buffer in guest
//! memory, so the game's own 2D work on the same surfaces (`Lock`, `Blt`) stays consistent.
//!
//! Covered: pre-transformed (`XYZRHW`) and transformed (`XYZ`, world·view·projection, viewport)
//! vertices; vertex lighting with directional, point and spot lights (no spot cone falloff
//! beyond a hard cut) and material colour sources; triangle lists, strips and fans; flat and
//! Gouraud shading; perspective-correct texture coordinates; two texture stages with the common
//! colour and alpha operations; point and bilinear filtering, wrap and clamp addressing; colour
//! keys; alpha test; blending with every source and destination factor; z test and write; culling;
//! vertex and linear table fog; 565, 1555, 4444 and 8888 surfaces.

use crate::d3d::State;
use crate::ddraw::{self, SurfaceInfo};
use rc_runtime::mem::Mem;

// Render states.
const RS_ZENABLE: u32 = 7;
const RS_SHADEMODE: u32 = 9;
const RS_ZWRITEENABLE: u32 = 14;
const RS_ALPHATESTENABLE: u32 = 15;
const RS_SRCBLEND: u32 = 19;
const RS_DESTBLEND: u32 = 20;
const RS_CULLMODE: u32 = 22;
const RS_ZFUNC: u32 = 23;
const RS_ALPHAREF: u32 = 24;
const RS_ALPHAFUNC: u32 = 25;
const RS_ALPHABLENDENABLE: u32 = 27;
const RS_FOGENABLE: u32 = 28;
const RS_FOGCOLOR: u32 = 34;
const RS_FOGTABLEMODE: u32 = 35;
const RS_FOGSTART: u32 = 36;
const RS_FOGEND: u32 = 37;
const RS_COLORKEYENABLE: u32 = 41;
const RS_TEXTUREFACTOR: u32 = 60;
const RS_LIGHTING: u32 = 137;
const RS_AMBIENT: u32 = 139;
const RS_COLORVERTEX: u32 = 141;
const RS_DIFFUSEMATERIALSOURCE: u32 = 145;
const RS_AMBIENTMATERIALSOURCE: u32 = 147;

// Texture stage states.
const TSS_COLOROP: u32 = 1;
const TSS_COLORARG1: u32 = 2;
const TSS_COLORARG2: u32 = 3;
const TSS_ALPHAOP: u32 = 4;
const TSS_ALPHAARG1: u32 = 5;
const TSS_ALPHAARG2: u32 = 6;
const TSS_TEXCOORDINDEX: u32 = 11;
const TSS_ADDRESSU: u32 = 13;
const TSS_ADDRESSV: u32 = 14;
const TSS_MAGFILTER: u32 = 16;

type Rgba = [f32; 4];

fn rs(st: &State, k: u32, default: u32) -> u32 {
    st.render.get(&k).copied().unwrap_or(default)
}

fn rsf(st: &State, k: u32, default: f32) -> f32 {
    st.render.get(&k).map_or(default, |v| f32::from_bits(*v))
}

fn tss(st: &State, stage: usize, k: u32) -> u32 {
    if let Some(v) = st.stages[stage].get(&k) {
        return *v;
    }
    match (stage, k) {
        (0, TSS_COLOROP) => 4,                        // MODULATE
        (_, TSS_COLOROP) => 1,                        // DISABLE
        (0, TSS_ALPHAOP) => 2,                        // SELECTARG1
        (_, TSS_ALPHAOP) => 1,                        // DISABLE
        (_, TSS_COLORARG1) | (_, TSS_ALPHAARG1) => 2, // TEXTURE
        (_, TSS_COLORARG2) | (_, TSS_ALPHAARG2) => 1, // CURRENT
        (_, TSS_TEXCOORDINDEX) => stage as u32,
        (_, TSS_ADDRESSU) | (_, TSS_ADDRESSV) => 1, // WRAP
        (_, TSS_MAGFILTER) => 1,                    // POINT
        _ => 0,
    }
}

fn argb(v: u32) -> Rgba {
    [
        ((v >> 16) & 0xff) as f32 / 255.0,
        ((v >> 8) & 0xff) as f32 / 255.0,
        (v & 0xff) as f32 / 255.0,
        (v >> 24) as f32 / 255.0,
    ]
}

/// A vertex after transformation and lighting, in screen space.
#[derive(Debug, Clone, Copy, Default)]
struct Vert {
    x: f32,
    y: f32,
    z: f32,
    rhw: f32,
    diffuse: Rgba,
    specular: Rgba,
    uv: [[f32; 2]; 2],
    /// Distance from the eye, for table fog.
    eye_z: f32,
}

fn transform(v: [f32; 4], mx: &[f32; 16]) -> [f32; 4] {
    let mut r = [0f32; 4];
    for (j, out) in r.iter_mut().enumerate() {
        *out = (0..4).map(|k| v[k] * mx[4 * k + j]).sum();
    }
    r
}

fn mul(a: &[f32; 16], b: &[f32; 16]) -> [f32; 16] {
    let mut r = [0f32; 16];
    for i in 0..4 {
        for j in 0..4 {
            r[4 * i + j] = (0..4).map(|k| a[4 * i + k] * b[4 * k + j]).sum();
        }
    }
    r
}

struct Light {
    kind: u32,
    diffuse: Rgba,
    ambient: Rgba,
    position: [f32; 3],
    direction: [f32; 3],
    range: f32,
    attenuation: [f32; 3],
}

fn lights(st: &State) -> Vec<Light> {
    let f = |b: &[u8; 104], o: usize| f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
    st.lights
        .values()
        .filter(|(_, on)| *on)
        .map(|(b, _)| Light {
            kind: u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            diffuse: [f(b, 4), f(b, 8), f(b, 12), f(b, 16)],
            ambient: [f(b, 36), f(b, 40), f(b, 44), f(b, 48)],
            position: [f(b, 52), f(b, 56), f(b, 60)],
            direction: [f(b, 64), f(b, 68), f(b, 72)],
            range: f(b, 76),
            attenuation: [f(b, 84), f(b, 88), f(b, 92)],
        })
        .collect()
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if l > 0.0 {
        [v[0] / l, v[1] / l, v[2] / l]
    } else {
        v
    }
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Read and process the vertices of a draw.
fn vertices(m: Mem, st: &State, fvf: u32, data: &[u8]) -> Vec<Vert> {
    let stride = crate::d3d::fvf_stride(fvf) as usize;
    if stride == 0 {
        return Vec::new();
    }
    let rhw = fvf & 0xe == 0x4;
    let blend = match fvf & 0xe {
        0x6 => 1,
        0x8 => 2,
        0xa => 3,
        0xc => 4,
        0xe => 5,
        _ => 0,
    };
    let f = |o: usize, i: usize| -> f32 {
        let b = &data[o + 4 * i..o + 4 * i + 4];
        f32::from_le_bytes([b[0], b[1], b[2], b[3]])
    };
    let u =
        |o: usize| -> u32 { u32::from_le_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]]) };
    let wv = mul(&st.world, &st.view);
    let wvp = mul(&wv, &st.projection);
    let vp = st.viewport;
    let lighting = !rhw && rs(st, RS_LIGHTING, 1) != 0;
    let lights = if lighting { lights(st) } else { Vec::new() };
    let ambient = argb(rs(st, RS_AMBIENT, 0));
    let mat = &st.material;
    let colorvertex = rs(st, RS_COLORVERTEX, 1) != 0;
    let _ = m;
    let mut out = Vec::with_capacity(data.len() / stride);
    for o in (0..data.len() / stride).map(|i| i * stride) {
        let mut off = o;
        let pos = [f(off, 0), f(off, 1), f(off, 2)];
        off += if rhw { 16 } else { 12 + 4 * blend };
        let normal = if fvf & 0x10 != 0 {
            let n = [f(off, 0), f(off, 1), f(off, 2)];
            off += 12;
            Some(n)
        } else {
            None
        };
        if fvf & 0x20 != 0 {
            off += 4;
        }
        let diffuse = if fvf & 0x40 != 0 {
            let d = argb(u(off));
            off += 4;
            Some(d)
        } else {
            None
        };
        let specular = if fvf & 0x80 != 0 {
            let s = argb(u(off));
            off += 4;
            Some(s)
        } else {
            None
        };
        let mut uv = [[0f32; 2]; 2];
        let sets = (fvf >> 8) & 0xf;
        #[allow(clippy::needless_range_loop)]
        for i in 0..sets as usize {
            let size = match (fvf >> (16 + 2 * i)) & 3 {
                0 => 2,
                1 => 3,
                2 => 4,
                _ => 1,
            };
            if i < 2 {
                uv[i] = [f(off, 0), if size > 1 { f(off, 1) } else { 0.0 }];
            }
            off += 4 * size;
        }
        let mut v = Vert {
            uv,
            specular: specular.unwrap_or([0.0, 0.0, 0.0, 1.0]),
            ..Vert::default()
        };
        if rhw {
            v.x = pos[0];
            v.y = pos[1];
            v.z = pos[2];
            v.rhw = f(o, 3);
            v.eye_z = if v.rhw != 0.0 { 1.0 / v.rhw } else { 0.0 };
            v.diffuse = diffuse.unwrap_or([1.0; 4]);
        } else {
            let c = transform([pos[0], pos[1], pos[2], 1.0], &wvp);
            let w = if c[3].abs() < 1e-7 { 1e-7 } else { c[3] };
            v.rhw = 1.0 / w;
            v.x = vp[0] + (c[0] / w + 1.0) * 0.5 * vp[2];
            v.y = vp[1] + (1.0 - c[1] / w) * 0.5 * vp[3];
            v.z = vp[4] + (c[2] / w) * (vp[5] - vp[4]);
            v.eye_z = transform([pos[0], pos[1], pos[2], 1.0], &wv)[2];
            v.diffuse = if lighting {
                let mdiff = if colorvertex && rs(st, RS_DIFFUSEMATERIALSOURCE, 1) == 1 {
                    diffuse.unwrap_or([mat[0], mat[1], mat[2], mat[3]])
                } else {
                    [mat[0], mat[1], mat[2], mat[3]]
                };
                let mamb = if colorvertex && rs(st, RS_AMBIENTMATERIALSOURCE, 0) == 1 {
                    diffuse.unwrap_or([mat[4], mat[5], mat[6], mat[7]])
                } else {
                    [mat[4], mat[5], mat[6], mat[7]]
                };
                let world_pos = transform([pos[0], pos[1], pos[2], 1.0], &st.world);
                let n = normal.map(|n| {
                    let t = transform([n[0], n[1], n[2], 0.0], &st.world);
                    normalize([t[0], t[1], t[2]])
                });
                let mut col = [
                    ambient[0] * mamb[0] + mat[12],
                    ambient[1] * mamb[1] + mat[13],
                    ambient[2] * mamb[2] + mat[14],
                    mdiff[3],
                ];
                for l in &lights {
                    let (dir, att) = if l.kind == 3 {
                        (
                            normalize([-l.direction[0], -l.direction[1], -l.direction[2]]),
                            1.0,
                        )
                    } else {
                        let d = [
                            l.position[0] - world_pos[0],
                            l.position[1] - world_pos[1],
                            l.position[2] - world_pos[2],
                        ];
                        let dist = dot(d, d).sqrt();
                        if l.range > 0.0 && dist > l.range {
                            continue;
                        }
                        let a = l.attenuation[0]
                            + l.attenuation[1] * dist
                            + l.attenuation[2] * dist * dist;
                        (normalize(d), if a > 0.0 { 1.0 / a } else { 1.0 })
                    };
                    let ndl = n.map_or(0.0, |n| dot(n, dir).max(0.0));
                    for i in 0..3 {
                        col[i] += att * (l.ambient[i] * mamb[i] + ndl * l.diffuse[i] * mdiff[i]);
                    }
                }
                col.map(|x| x.clamp(0.0, 1.0))
            } else {
                diffuse.unwrap_or([1.0; 4])
            };
        }
        out.push(v);
    }
    out
}

/// A texture: its pixels decoded on demand from guest memory.
struct Texture {
    info: SurfaceInfo,
    key: Option<(u32, u32)>,
    bilinear: bool,
    clamp: (bool, bool),
}

fn channel(v: u32, mask: u32) -> f32 {
    if mask == 0 {
        return 1.0;
    }
    let shift = mask.trailing_zeros();
    let max = mask >> shift;
    ((v & mask) >> shift) as f32 / max as f32
}

impl Texture {
    fn raw(&self, m: Mem, x: u32, y: u32) -> u32 {
        let a = self.info.buf + y * self.info.pitch + x * (self.info.bpp / 8);
        match self.info.bpp {
            16 => u32::from(m.ld16(a)),
            8 => u32::from(m.ld8(a)),
            _ => m.ld32(a),
        }
    }

    fn texel(&self, m: Mem, x: i32, y: i32) -> Rgba {
        let (w, h) = (self.info.width as i32, self.info.height as i32);
        let x = if self.clamp.0 {
            x.clamp(0, w - 1)
        } else {
            x.rem_euclid(w)
        };
        let y = if self.clamp.1 {
            y.clamp(0, h - 1)
        } else {
            y.rem_euclid(h)
        };
        let v = self.raw(m, x as u32, y as u32);
        if let Some((lo, hi)) = self.key {
            if v >= lo && v <= hi {
                return [0.0; 4];
            }
        }
        let k = self.info.masks;
        [
            channel(v, k[0]),
            channel(v, k[1]),
            channel(v, k[2]),
            channel(v, k[3]),
        ]
    }

    fn sample(&self, m: Mem, u: f32, v: f32) -> Rgba {
        let (w, h) = (self.info.width as f32, self.info.height as f32);
        if !self.bilinear {
            return self.texel(m, (u * w).floor() as i32, (v * h).floor() as i32);
        }
        let (fx, fy) = (u * w - 0.5, v * h - 0.5);
        let (x0, y0) = (fx.floor(), fy.floor());
        let (ax, ay) = (fx - x0, fy - y0);
        let (x0, y0) = (x0 as i32, y0 as i32);
        let t = [
            self.texel(m, x0, y0),
            self.texel(m, x0 + 1, y0),
            self.texel(m, x0, y0 + 1),
            self.texel(m, x0 + 1, y0 + 1),
        ];
        let mut r = [0f32; 4];
        for (i, o) in r.iter_mut().enumerate() {
            *o = (t[0][i] * (1.0 - ax) + t[1][i] * ax) * (1.0 - ay)
                + (t[2][i] * (1.0 - ax) + t[3][i] * ax) * ay;
        }
        r
    }
}

fn arg(a: u32, diffuse: Rgba, current: Rgba, tex: Rgba, factor: Rgba, specular: Rgba) -> Rgba {
    let mut v = match a & 0xf {
        0 => diffuse,
        1 => current,
        2 => tex,
        3 => factor,
        4 => specular,
        _ => current,
    };
    if a & 0x20 != 0 {
        v = [v[3]; 4];
    }
    if a & 0x10 != 0 {
        v = v.map(|x| 1.0 - x);
    }
    v
}

fn op(o: u32, a1: f32, a2: f32, alpha_diffuse: f32, alpha_tex: f32, alpha_cur: f32) -> f32 {
    match o {
        2 => a1,
        3 => a2,
        4 => a1 * a2,
        5 => (a1 * a2 * 2.0).min(1.0),
        6 => (a1 * a2 * 4.0).min(1.0),
        7 => (a1 + a2).min(1.0),
        8 => (a1 + a2 - 0.5).clamp(0.0, 1.0),
        9 => ((a1 + a2 - 0.5) * 2.0).clamp(0.0, 1.0),
        10 => a1 - a2,
        11 => a1 + a2 - a1 * a2,
        12 => a1 * alpha_diffuse + a2 * (1.0 - alpha_diffuse),
        13 => a1 * alpha_tex + a2 * (1.0 - alpha_tex),
        15 => a1 * alpha_cur + a2 * (1.0 - alpha_cur),
        _ => a1 * a2,
    }
}

fn compare(func: u32, a: f32, b: f32) -> bool {
    match func {
        1 => false,
        2 => a < b,
        3 => a == b,
        4 => a <= b,
        5 => a > b,
        6 => a != b,
        7 => a >= b,
        _ => true,
    }
}

fn blend_factor(f: u32, src: Rgba, dst: Rgba) -> Rgba {
    match f {
        1 => [0.0; 4],
        2 => [1.0; 4],
        3 => src,
        4 => src.map(|x| 1.0 - x),
        5 => [src[3]; 4],
        6 => [1.0 - src[3]; 4],
        7 => [dst[3]; 4],
        8 => [1.0 - dst[3]; 4],
        9 => dst,
        10 => dst.map(|x| 1.0 - x),
        11 => {
            let f = src[3].min(1.0 - dst[3]);
            [f, f, f, 1.0]
        }
        _ => [1.0; 4],
    }
}

struct Target {
    info: SurfaceInfo,
    z: Option<SurfaceInfo>,
}

impl Target {
    fn read(&self, m: Mem, x: u32, y: u32) -> Rgba {
        let a = self.info.buf + y * self.info.pitch + x * (self.info.bpp / 8);
        let v = if self.info.bpp == 16 {
            u32::from(m.ld16(a))
        } else {
            m.ld32(a)
        };
        let k = self.info.masks;
        [
            channel(v, k[0]),
            channel(v, k[1]),
            channel(v, k[2]),
            if k[3] != 0 { channel(v, k[3]) } else { 1.0 },
        ]
    }

    fn write(&self, m: Mem, x: u32, y: u32, c: Rgba) {
        let a = self.info.buf + y * self.info.pitch + x * (self.info.bpp / 8);
        let pack = |v: f32, mask: u32| -> u32 {
            if mask == 0 {
                return 0;
            }
            let shift = mask.trailing_zeros();
            let max = mask >> shift;
            (((v.clamp(0.0, 1.0) * max as f32) + 0.5) as u32) << shift & mask
        };
        let k = self.info.masks;
        let v = pack(c[0], k[0]) | pack(c[1], k[1]) | pack(c[2], k[2]) | pack(c[3], k[3]);
        if self.info.bpp == 16 {
            m.st16(a, v as u16);
        } else {
            m.st32(a, v);
        }
    }

    fn zread(&self, m: Mem, x: u32, y: u32) -> Option<f32> {
        let z = self.z?;
        let a = z.buf + y * z.pitch + x * (z.bpp / 8);
        Some(match z.bpp {
            16 => f32::from(m.ld16(a)) / 65535.0,
            24 => (u32::from(m.ld16(a)) | u32::from(m.ld8(a + 2)) << 16) as f32 / 16_777_215.0,
            _ => (m.ld32(a) & 0x00ff_ffff) as f32 / 16_777_215.0,
        })
    }

    fn zwrite(&self, m: Mem, x: u32, y: u32, v: f32) {
        let Some(z) = self.z else {
            return;
        };
        let a = z.buf + y * z.pitch + x * (z.bpp / 8);
        let v = v.clamp(0.0, 1.0);
        match z.bpp {
            16 => m.st16(a, (v * 65535.0) as u16),
            24 => {
                let i = (v * 16_777_215.0) as u32;
                m.st16(a, i as u16);
                m.st8(a + 2, (i >> 16) as u8);
            }
            _ => m.st32(a, (v * 16_777_215.0) as u32),
        }
    }
}

/// Draw one primitive (D3DPRIMITIVETYPE: 1 points, 2 line list, 3 line strip, 4 triangle list,
/// 5 strip, 6 fan) into the state's render target.
pub fn draw(m: Mem, st: &State, primitive: u32, fvf: u32, data: &[u8], indices: &[u16]) {
    let Some(info) = ddraw::info(m, st.target) else {
        return;
    };
    let target = Target {
        info,
        z: if rs(st, RS_ZENABLE, 1) != 0 {
            ddraw::zbuffer_info(m, st.target)
        } else {
            None
        },
    };
    let verts = vertices(m, st, fvf, data);
    if verts.is_empty() {
        return;
    }
    let idx: Vec<usize> = if indices.is_empty() {
        (0..verts.len()).collect()
    } else {
        indices
            .iter()
            .map(|&i| usize::from(i))
            .filter(|&i| i < verts.len())
            .collect()
    };
    let mut tris = Vec::new();
    match primitive {
        4 => {
            for t in idx.as_chunks::<3>().0 {
                tris.push((t[0], t[1], t[2]));
            }
        }
        5 => {
            for i in 2..idx.len() {
                if i % 2 == 0 {
                    tris.push((idx[i - 2], idx[i - 1], idx[i]));
                } else {
                    tris.push((idx[i - 1], idx[i - 2], idx[i]));
                }
            }
        }
        6 => {
            for i in 2..idx.len() {
                tris.push((idx[0], idx[i - 1], idx[i]));
            }
        }
        _ => return, // points and lines: not used for the game's look
    }
    let mut textures: [Option<Texture>; 2] = [None, None];
    for (s, slot) in textures.iter_mut().enumerate() {
        if let Some((obj, _)) = st.textures[s] {
            if let Some(info) = ddraw::info(m, obj) {
                *slot = Some(Texture {
                    info,
                    key: if rs(st, RS_COLORKEYENABLE, 0) != 0 {
                        info.src_key
                    } else {
                        None
                    },
                    bilinear: tss(st, s, TSS_MAGFILTER) >= 2,
                    clamp: (tss(st, s, TSS_ADDRESSU) == 3, tss(st, s, TSS_ADDRESSV) == 3),
                });
            }
        }
    }
    let cull = rs(st, RS_CULLMODE, 3);
    let flat = rs(st, RS_SHADEMODE, 2) == 1;
    let ctx = Ctx {
        m,
        st,
        target: &target,
        textures: &textures,
        zfunc: rs(st, RS_ZFUNC, 4),
        zwrite: rs(st, RS_ZWRITEENABLE, 1) != 0,
        alphatest: rs(st, RS_ALPHATESTENABLE, 0) != 0,
        alpharef: rs(st, RS_ALPHAREF, 0) as f32 / 255.0,
        alphafunc: rs(st, RS_ALPHAFUNC, 8),
        blend: rs(st, RS_ALPHABLENDENABLE, 0) != 0,
        srcblend: rs(st, RS_SRCBLEND, 2),
        dstblend: rs(st, RS_DESTBLEND, 1),
        factor: argb(rs(st, RS_TEXTUREFACTOR, 0xffff_ffff)),
        fog: rs(st, RS_FOGENABLE, 0) != 0,
        fogcolor: argb(rs(st, RS_FOGCOLOR, 0)),
        fogtable: rs(st, RS_FOGTABLEMODE, 0),
        fogrange: (rsf(st, RS_FOGSTART, 0.0), rsf(st, RS_FOGEND, 1.0)),
    };
    for (a, b, c) in tris {
        let (mut va, mut vb, mut vc) = (verts[a], verts[b], verts[c]);
        let area = (vb.x - va.x) * (vc.y - va.y) - (vc.x - va.x) * (vb.y - va.y);
        if (cull == 2 && area > 0.0) || (cull == 3 && area < 0.0) {
            continue;
        }
        if flat {
            vb.diffuse = va.diffuse;
            vc.diffuse = va.diffuse;
            vb.specular = va.specular;
            vc.specular = va.specular;
        }
        if area < 0.0 {
            std::mem::swap(&mut vb, &mut vc);
        }
        if va.rhw <= 0.0 || vb.rhw <= 0.0 || vc.rhw <= 0.0 {
            continue; // behind the eye: no near-plane clipping yet
        }
        triangle(&ctx, &va, &vb, &vc);
        let _ = &mut va;
    }
}

struct Ctx<'a> {
    m: Mem,
    st: &'a State,
    target: &'a Target,
    textures: &'a [Option<Texture>; 2],
    zfunc: u32,
    zwrite: bool,
    alphatest: bool,
    alpharef: f32,
    alphafunc: u32,
    blend: bool,
    srcblend: u32,
    dstblend: u32,
    factor: Rgba,
    fog: bool,
    fogcolor: Rgba,
    fogtable: u32,
    fogrange: (f32, f32),
}

fn triangle(cx: &Ctx, a: &Vert, b: &Vert, c: &Vert) {
    let (w, h) = (cx.target.info.width as f32, cx.target.info.height as f32);
    let min_x = a.x.min(b.x).min(c.x).floor().max(0.0) as i32;
    let max_x = a.x.max(b.x).max(c.x).ceil().min(w - 1.0) as i32;
    let min_y = a.y.min(b.y).min(c.y).floor().max(0.0) as i32;
    let max_y = a.y.max(b.y).max(c.y).ceil().min(h - 1.0) as i32;
    if min_x > max_x || min_y > max_y {
        return;
    }
    let area = (b.x - a.x) * (c.y - a.y) - (c.x - a.x) * (b.y - a.y);
    if area.abs() < 1e-9 {
        return;
    }
    let edge =
        |p: &Vert, q: &Vert, x: f32, y: f32| (q.x - p.x) * (y - p.y) - (q.y - p.y) * (x - p.x);
    for y in min_y..=max_y {
        // Direct3D before version 10 puts pixel centres on integer coordinates.
        let py = y as f32;
        for x in min_x..=max_x {
            let px = x as f32;
            let w0 = edge(b, c, px, py);
            let w1 = edge(c, a, px, py);
            let w2 = edge(a, b, px, py);
            if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                continue;
            }
            let (l0, l1, l2) = (w0 / area, w1 / area, w2 / area);
            let z = l0 * a.z + l1 * b.z + l2 * c.z;
            if let Some(old) = cx.target.zread(cx.m, x as u32, y as u32) {
                if !compare(cx.zfunc, z, old) {
                    continue;
                }
            }
            // Perspective-correct weights.
            let r = l0 * a.rhw + l1 * b.rhw + l2 * c.rhw;
            let (p0, p1, p2) = (l0 * a.rhw / r, l1 * b.rhw / r, l2 * c.rhw / r);
            let lerp4 = |s: Rgba, t: Rgba, u: Rgba| -> Rgba {
                [0, 1, 2, 3].map(|i| p0 * s[i] + p1 * t[i] + p2 * u[i])
            };
            let diffuse = lerp4(a.diffuse, b.diffuse, c.diffuse);
            let specular = lerp4(a.specular, b.specular, c.specular);
            let mut current = diffuse;
            for s in 0..2 {
                let cop = tss(cx.st, s, TSS_COLOROP);
                if cop == 1 {
                    break;
                }
                let ci = tss(cx.st, s, TSS_TEXCOORDINDEX) as usize & 1;
                let u = p0 * a.uv[ci][0] + p1 * b.uv[ci][0] + p2 * c.uv[ci][0];
                let v = p0 * a.uv[ci][1] + p1 * b.uv[ci][1] + p2 * c.uv[ci][1];
                let tex = cx.textures[s]
                    .as_ref()
                    .map_or([1.0; 4], |t| t.sample(cx.m, u, v));
                let ca1 = arg(
                    tss(cx.st, s, TSS_COLORARG1),
                    diffuse,
                    current,
                    tex,
                    cx.factor,
                    specular,
                );
                let ca2 = arg(
                    tss(cx.st, s, TSS_COLORARG2),
                    diffuse,
                    current,
                    tex,
                    cx.factor,
                    specular,
                );
                let aop = tss(cx.st, s, TSS_ALPHAOP);
                let aa1 = arg(
                    tss(cx.st, s, TSS_ALPHAARG1),
                    diffuse,
                    current,
                    tex,
                    cx.factor,
                    specular,
                )[3];
                let aa2 = arg(
                    tss(cx.st, s, TSS_ALPHAARG2),
                    diffuse,
                    current,
                    tex,
                    cx.factor,
                    specular,
                )[3];
                let mut next = current;
                for i in 0..3 {
                    next[i] =
                        op(cop, ca1[i], ca2[i], diffuse[3], tex[3], current[3]).clamp(0.0, 1.0);
                }
                if aop != 1 {
                    next[3] = op(aop, aa1, aa2, diffuse[3], tex[3], current[3]).clamp(0.0, 1.0);
                }
                // A colour-keyed texel is transparent whatever the operations.
                if cx.textures[s].as_ref().is_some_and(|t| t.key.is_some()) && tex == [0.0; 4] {
                    next[3] = 0.0;
                }
                current = next;
            }
            if rs(cx.st, 29, 0) != 0 {
                // SPECULARENABLE: added after the stages.
                for i in 0..3 {
                    current[i] = (current[i] + specular[i]).min(1.0);
                }
            }
            if cx.textures.iter().flatten().any(|t| t.key.is_some()) && current[3] == 0.0 {
                continue;
            }
            if cx.alphatest && !compare(cx.alphafunc, current[3], cx.alpharef) {
                continue;
            }
            if cx.fog {
                let f = if cx.fogtable == 3 {
                    let d = 1.0 / r;
                    let (s, e) = cx.fogrange;
                    if e > s {
                        ((e - d) / (e - s)).clamp(0.0, 1.0)
                    } else {
                        1.0
                    }
                } else {
                    specular[3]
                };
                for i in 0..3 {
                    current[i] = current[i] * f + cx.fogcolor[i] * (1.0 - f);
                }
            }
            let out = if cx.blend {
                let dst = cx.target.read(cx.m, x as u32, y as u32);
                let sf = blend_factor(cx.srcblend, current, dst);
                let df = blend_factor(cx.dstblend, current, dst);
                [0, 1, 2, 3].map(|i| (current[i] * sf[i] + dst[i] * df[i]).clamp(0.0, 1.0))
            } else {
                current
            };
            cx.target.write(cx.m, x as u32, y as u32, out);
            if cx.zwrite {
                cx.target.zwrite(cx.m, x as u32, y as u32, z);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colour_operations() {
        assert_eq!(op(4, 0.5, 0.5, 0.0, 0.0, 0.0), 0.25);
        assert_eq!(op(2, 0.3, 0.9, 0.0, 0.0, 0.0), 0.3);
        assert!(compare(4, 0.5, 0.5));
        assert!(!compare(2, 0.5, 0.5));
        assert_eq!(channel(0xf800, 0xf800), 1.0);
        assert_eq!(blend_factor(6, [0.0, 0.0, 0.0, 0.25], [0.0; 4]), [0.75; 4]);
    }
}
