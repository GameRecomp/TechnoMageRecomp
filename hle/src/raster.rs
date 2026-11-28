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
    /// Clip-space position of a vertex the runtime transformed (none for pre-transformed ones).
    clip: Option<[f32; 4]>,
}

/// Screen position of a clip-space position through the viewport.
fn project(v: &mut Vert, c: [f32; 4], vp: &[f32; 6]) {
    let w = if c[3].abs() < 1e-7 { 1e-7 } else { c[3] };
    v.rhw = 1.0 / w;
    v.x = vp[0] + (c[0] / w + 1.0) * 0.5 * vp[2];
    v.y = vp[1] + (1.0 - c[1] / w) * 0.5 * vp[3];
    v.z = vp[4] + (c[2] / w) * (vp[5] - vp[4]);
}

fn lerp_vert(a: &Vert, b: &Vert, t: f32, vp: &[f32; 6]) -> Vert {
    let l = |x: f32, y: f32| x + (y - x) * t;
    let l4 =
        |x: [f32; 4], y: [f32; 4]| [l(x[0], y[0]), l(x[1], y[1]), l(x[2], y[2]), l(x[3], y[3])];
    let ca = a.clip.unwrap_or_default();
    let cb = b.clip.unwrap_or_default();
    let c = l4(ca, cb);
    let mut v = Vert {
        diffuse: l4(a.diffuse, b.diffuse),
        specular: l4(a.specular, b.specular),
        uv: [
            [l(a.uv[0][0], b.uv[0][0]), l(a.uv[0][1], b.uv[0][1])],
            [l(a.uv[1][0], b.uv[1][0]), l(a.uv[1][1], b.uv[1][1])],
        ],
        eye_z: l(a.eye_z, b.eye_z),
        clip: Some(c),
        ..Vert::default()
    };
    project(&mut v, c, vp);
    v
}

/// A triangle clipped at the near plane (z = 0 in clip space, as Direct3D clips): none, itself,
/// or the pieces of the part in front, as a fan. Pre-transformed triangles are not clipped.
fn clip_near(t: [Vert; 3], vp: &[f32; 6]) -> Vec<[Vert; 3]> {
    let z = |v: &Vert| v.clip.map(|c| c[2]);
    let (Some(za), Some(zb), Some(zc)) = (z(&t[0]), z(&t[1]), z(&t[2])) else {
        return vec![t];
    };
    if za >= 0.0 && zb >= 0.0 && zc >= 0.0 {
        return vec![t];
    }
    if za < 0.0 && zb < 0.0 && zc < 0.0 {
        return Vec::new();
    }
    let zs = [za, zb, zc];
    let mut poly: Vec<Vert> = Vec::with_capacity(4);
    for i in 0..3 {
        let j = (i + 1) % 3;
        let (a, b) = (&t[i], &t[j]);
        if zs[i] >= 0.0 {
            poly.push(*a);
        }
        if (zs[i] >= 0.0) != (zs[j] >= 0.0) {
            let k = zs[i] / (zs[i] - zs[j]);
            poly.push(lerp_vert(a, b, k, vp));
        }
    }
    (1..poly.len().saturating_sub(1))
        .map(|i| [poly[0], poly[i], poly[i + 1]])
        .collect()
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
            project(&mut v, c, &vp);
            v.clip = Some(c);
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

fn channel(v: u32, mask: u32) -> f32 {
    if mask == 0 {
        return 1.0;
    }
    let shift = mask.trailing_zeros();
    let max = mask >> shift;
    ((v & mask) >> shift) as f32 / max as f32
}

/// A channel of a pixel `v` under `mask` as 0..=255 (255 without a mask).
#[inline(always)]
fn to8(v: u32, mask: u32) -> u8 {
    if mask == 0 {
        return 255;
    }
    let shift = mask.trailing_zeros();
    let max = mask >> shift;
    (((v & mask) >> shift) * 255 / max) as u8
}

type Table16 = std::sync::Arc<Vec<[u8; 4]>>;

/// Every 16-bit pixel under `masks` as RGBA8, made once per set of masks: a lookup instead of
/// four divisions a pixel.
fn table16(masks: [u32; 4]) -> Table16 {
    static TABLES: std::sync::Mutex<Vec<([u32; 4], Table16)>> = std::sync::Mutex::new(Vec::new());
    let mut t = TABLES.lock().expect("tables");
    if let Some((_, table)) = t.iter().find(|(k, _)| *k == masks) {
        return table.clone();
    }
    let table: Table16 = std::sync::Arc::new(
        (0..=u32::from(u16::MAX))
            .map(|v| masks.map(|k| to8(v, k)))
            .collect(),
    );
    t.push((masks, table.clone()));
    table
}

/// A texture decoded to RGBA8 once per content version (and colour-key setting), cached.
struct Decoded {
    w: u32,
    h: u32,
    texels: std::sync::Arc<Vec<[u8; 4]>>,
    /// Whether any texel is colour-keyed (alpha 0 from the key).
    keyed: bool,
}

fn decode(m: Mem, info: &SurfaceInfo, key: Option<(u32, u32)>) -> Decoded {
    let (w, h) = (info.width, info.height);
    let mut texels = Vec::with_capacity((w * h) as usize);
    let k = info.masks;
    let table = (info.bpp == 16).then(|| table16(k));
    let mut keyed = false;
    let bytes = info.bpp / 8;
    let mut row = vec![0u8; (w * bytes) as usize];
    for y in 0..h {
        m.read(info.buf + y * info.pitch, &mut row);
        for x in 0..w as usize {
            let v = match bytes {
                2 => u32::from(u16::from_le_bytes([row[2 * x], row[2 * x + 1]])),
                1 => u32::from(row[x]),
                _ => {
                    u32::from_le_bytes([row[4 * x], row[4 * x + 1], row[4 * x + 2], row[4 * x + 3]])
                }
            };
            if let Some((lo, hi)) = key {
                if v >= lo && v <= hi {
                    keyed = true;
                    texels.push([0, 0, 0, 0]);
                    continue;
                }
            }
            texels.push(match &table {
                Some(t) => t[v as usize],
                None => k.map(|mask| to8(v, mask)),
            });
        }
    }
    Decoded {
        w,
        h,
        texels: std::sync::Arc::new(texels),
        keyed,
    }
}

type CacheKey = (u32, u32, bool);
static TEXTURES: std::sync::Mutex<
    Option<std::collections::HashMap<CacheKey, std::sync::Arc<Decoded>>>,
> = std::sync::Mutex::new(None);

fn decoded(
    m: Mem,
    obj: u32,
    version: u32,
    info: &SurfaceInfo,
    key: Option<(u32, u32)>,
) -> std::sync::Arc<Decoded> {
    let k = (obj, version, key.is_some());
    let mut cache = TEXTURES.lock().expect("textures");
    let map = cache.get_or_insert_with(Default::default);
    if let Some(d) = map.get(&k) {
        return d.clone();
    }
    if map.len() > 1024 {
        map.clear();
    }
    let d = std::sync::Arc::new(decode(m, info, key));
    map.insert(k, d.clone());
    d
}

/// The texture enlarged for the GPU: smoothed four times (`smooth`, "smooth textures",
/// `upscale`) or not, and with the glyphs of the fonts tied to it redrawn (`fonts`, "smooth
/// fonts", `glyphs`) at as many texels as the screen is drawn larger (`glyphs::factor`, so a
/// font page follows the render resolution); made in the background (`enlarging`) and cached
/// here. With it the variant for the GPU's texture key (bit 1 smoothed, bit 2 fonts, bits 3–6
/// the factor, above them the fonts' stamp). `None` when nothing is asked, the texture is too
/// large to be enlarged or it is still being made: the plain texture is drawn meanwhile.
#[cfg(feature = "gpu")]
fn enlarge(
    m: Mem,
    (obj, version): (u32, u32),
    info: &SurfaceInfo,
    key: Option<(u32, u32)>,
    smooth: bool,
    fonts: Option<(Vec<std::sync::Arc<crate::glyphs::GlyphSet>>, u64)>,
) -> Option<(std::sync::Arc<Decoded>, u32)> {
    type Cache = std::collections::HashMap<crate::enlarging::Key, std::sync::Arc<Decoded>>;
    static ENLARGED: std::sync::Mutex<Option<Cache>> = std::sync::Mutex::new(None);
    /// The factor font pages were last enlarged by: when it changes, the old pages go.
    static FONT_FACTOR: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    if !smooth && fonts.is_none() {
        return None;
    }
    let side = info.width.max(info.height) as usize;
    let largest = (rc_platform::render::max_texture() as usize).min(4096);
    let factor = if fonts.is_some() {
        let screen = crate::ddraw::screen_size();
        let f = crate::glyphs::factor(screen, crate::d3d::screen_render_size());
        f.min(largest / side.max(1))
    } else if side <= 512 {
        4
    } else {
        0
    };
    if factor == 0 {
        return None;
    }
    let variant = u32::from(smooth) << 1
        | fonts
            .as_ref()
            .map_or(0, |f| 4 | (f.1 as u32 & 0x01ff_ffff) << 7)
        | (factor as u32 & 0xf) << 3;
    if fonts.is_some()
        && FONT_FACTOR.swap(factor as u32, std::sync::atomic::Ordering::Relaxed) != factor as u32
    {
        // The render resolution changed: font pages enlarged for the old one are not used again.
        if let Some(map) = ENLARGED.lock().expect("enlarged").as_mut() {
            map.retain(|k, _| k.3 & 4 == 0 || (k.3 >> 3) & 0xf == factor as u32);
        }
    }
    let k = (obj, version, key.is_some(), variant);
    let lookup = || {
        let mut cache = ENLARGED.lock().expect("enlarged");
        let map = cache.get_or_insert_with(Default::default);
        for done in crate::enlarging::take_done() {
            if map.len() > 1024 {
                map.clear();
            }
            map.insert(
                done.key,
                std::sync::Arc::new(Decoded {
                    w: done.w,
                    h: done.h,
                    texels: std::sync::Arc::new(done.texels),
                    keyed: done.keyed,
                }),
            );
        }
        map.get(&k).cloned()
    };
    if let Some(e) = lookup() {
        return Some((e, variant));
    }
    if crate::enlarging::pending(&k) {
        return None;
    }
    let d = decoded(m, obj, version, info, key);
    let job = crate::enlarging::Job::new(
        d.texels.clone(),
        (d.w as usize, d.h as usize),
        factor,
        d.keyed,
        smooth,
        fonts.map(|f| f.0).unwrap_or_default(),
    );
    crate::enlarging::submit(k, job);
    // Done at once with the virtual clock.
    lookup().map(|e| (e, variant))
}

/// The texel rectangles that 2D quads among `tris` show of a `width × height` texture, one texel
/// to one pixel (as `mark_texel_for_pixel` finds them), for `glyphs`.
#[cfg(feature = "gpu")]
fn exact_rects(tris: &[rc_platform::gpu::Vertex], width: f32, height: f32) -> Vec<[u16; 4]> {
    let mut out = Vec::new();
    for t in tris.as_chunks::<3>().0 {
        if !texel_for_pixel(t, width, height) {
            continue;
        }
        let us = t.map(|v| v.uv0[0] * width);
        let vs = t.map(|v| v.uv0[1] * height);
        let lo = |a: [f32; 3]| a.iter().copied().fold(f32::MAX, f32::min).round();
        let hi = |a: [f32; 3]| a.iter().copied().fold(f32::MIN, f32::max).round();
        let (x0, y0) = (lo(us), lo(vs));
        let (x1, y1) = (hi(us), hi(vs));
        if x0 < 0.0 || y0 < 0.0 || x1 <= x0 || y1 <= y0 || x1 > 65535.0 || y1 > 65535.0 {
            continue;
        }
        out.push([x0 as u16, y0 as u16, (x1 - x0) as u16, (y1 - y0) as u16]);
    }
    out
}

struct Texture {
    d: std::sync::Arc<Decoded>,
    bilinear: bool,
    clamp: (bool, bool),
}

impl Texture {
    #[inline(always)]
    fn texel(&self, x: i32, y: i32) -> Rgba {
        let (w, h) = (self.d.w as i32, self.d.h as i32);
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
        let t = self.d.texels[(y * w + x) as usize];
        const K: f32 = 1.0 / 255.0;
        [
            f32::from(t[0]) * K,
            f32::from(t[1]) * K,
            f32::from(t[2]) * K,
            f32::from(t[3]) * K,
        ]
    }

    #[inline(always)]
    fn sample(&self, u: f32, v: f32) -> Rgba {
        let (w, h) = (self.d.w as f32, self.d.h as f32);
        if !self.bilinear {
            return self.texel((u * w).floor() as i32, (v * h).floor() as i32);
        }
        let (fx, fy) = (u * w - 0.5, v * h - 0.5);
        let (x0, y0) = (fx.floor(), fy.floor());
        let (ax, ay) = (fx - x0, fy - y0);
        let (x0, y0) = (x0 as i32, y0 as i32);
        let t = [
            self.texel(x0, y0),
            self.texel(x0 + 1, y0),
            self.texel(x0, y0 + 1),
            self.texel(x0 + 1, y0 + 1),
        ];
        let mut r = [0f32; 4];
        for (i, o) in r.iter_mut().enumerate() {
            *o = (t[0][i] * (1.0 - ax) + t[1][i] * ax) * (1.0 - ay)
                + (t[2][i] * (1.0 - ax) + t[3][i] * ax) * ay;
        }
        r
    }
}

/// A texture stage's configuration, read once per draw.
#[derive(Clone, Copy)]
struct Stage {
    cop: u32,
    ca1: u32,
    ca2: u32,
    aop: u32,
    aa1: u32,
    aa2: u32,
    coords: usize,
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

    /// `v` at the depth buffer's precision, as the buffer would hold it: a draw at the depth
    /// already there compares equal (several passes over one object).
    fn zquant(&self, v: f32) -> f32 {
        match self.z {
            Some(z) => {
                let max = if z.bpp == 16 { 65535.0 } else { 16_777_215.0 };
                (v.clamp(0.0, 1.0) * max).round() / max
            }
            None => v,
        }
    }

    fn zwrite(&self, m: Mem, x: u32, y: u32, v: f32) {
        let Some(z) = self.z else {
            return;
        };
        let a = z.buf + y * z.pitch + x * (z.bpp / 8);
        let v = v.clamp(0.0, 1.0);
        match z.bpp {
            16 => m.st16(a, (v * 65535.0).round() as u16),
            24 => {
                let i = (v * 16_777_215.0).round() as u32;
                m.st16(a, i as u16);
                m.st8(a + 2, (i >> 16) as u8);
            }
            _ => m.st32(a, (v * 16_777_215.0).round() as u32),
        }
    }
}

/// Draw one primitive (D3DPRIMITIVETYPE: 1 points, 2 line list, 3 line strip, 4 triangle list,
/// 5 strip, 6 fan) into the state's render target.
pub fn draw(m: Mem, st: &State, primitive: u32, fvf: u32, data: &[u8], indices: &[u16]) {
    let Some(info) = ddraw::info(m, st.target) else {
        return;
    };
    let z = if rs(st, RS_ZENABLE, 1) != 0 {
        ddraw::zbuffer_info(m, st.target)
    } else {
        None
    };
    draw_into(m, st, info, z, primitive, fvf, data, indices);
}

/// [`draw`] into the pixels `info` (and depth `z`) instead of the state's target.
#[allow(clippy::too_many_arguments)]
pub fn draw_into(
    m: Mem,
    st: &State,
    info: SurfaceInfo,
    z: Option<SurfaceInfo>,
    primitive: u32,
    fvf: u32,
    data: &[u8],
    indices: &[u16],
) {
    let z = z.filter(|_| rs(st, RS_ZENABLE, 1) != 0);
    let target = Target { info, z };
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
        if let Some((obj, version)) = st.textures[s] {
            if let Some(info) = ddraw::info(m, obj) {
                let key = if rs(st, RS_COLORKEYENABLE, 0) != 0 {
                    info.src_key
                } else {
                    None
                };
                *slot = Some(Texture {
                    d: decoded(m, obj, ddraw::version_of(m, obj).max(version), &info, key),
                    bilinear: tss(st, s, TSS_MAGFILTER) >= 2,
                    clamp: (tss(st, s, TSS_ADDRESSU) == 3, tss(st, s, TSS_ADDRESSV) == 3),
                });
            }
        }
    }
    let mut stages = Vec::new();
    for s in 0..2 {
        let cop = tss(st, s, TSS_COLOROP);
        if cop == 1 {
            break;
        }
        stages.push(Stage {
            cop,
            ca1: tss(st, s, TSS_COLORARG1),
            ca2: tss(st, s, TSS_COLORARG2),
            aop: tss(st, s, TSS_ALPHAOP),
            aa1: tss(st, s, TSS_ALPHAARG1),
            aa2: tss(st, s, TSS_ALPHAARG2),
            coords: tss(st, s, TSS_TEXCOORDINDEX) as usize & 1,
        });
    }
    let cull = rs(st, RS_CULLMODE, 3);
    let flat = rs(st, RS_SHADEMODE, 2) == 1;
    let ctx = Ctx {
        m,
        target: &target,
        textures: &textures,
        stages: &stages,
        specular_add: rs(st, 29, 0) != 0,
        keyed: textures.iter().flatten().any(|t| t.d.keyed),
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
        let (va, mut vb, mut vc) = (verts[a], verts[b], verts[c]);
        if flat {
            vb.diffuse = va.diffuse;
            vc.diffuse = va.diffuse;
            vb.specular = va.specular;
            vc.specular = va.specular;
        }
        for [va, mut vb, mut vc] in clip_near([va, vb, vc], &st.viewport) {
            let area = (vb.x - va.x) * (vc.y - va.y) - (vc.x - va.x) * (vb.y - va.y);
            if (cull == 2 && area > 0.0) || (cull == 3 && area < 0.0) {
                continue;
            }
            if area < 0.0 {
                std::mem::swap(&mut vb, &mut vc);
            }
            if va.rhw <= 0.0 || vb.rhw <= 0.0 || vc.rhw <= 0.0 {
                continue;
            }
            triangle(&ctx, &va, &vb, &vc);
        }
    }
}

struct Ctx<'a> {
    m: Mem,
    target: &'a Target,
    textures: &'a [Option<Texture>; 2],
    stages: &'a [Stage],
    specular_add: bool,
    keyed: bool,
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
    let inv_area = 1.0 / area;
    // Direct3D's top-left rule: a pixel on an edge two triangles share is drawn by one of them.
    // With the area positive, an edge is a left edge when it runs up, a top edge when it runs
    // right along a row; only those take the pixels exactly on them.
    let top_left = |p: &Vert, q: &Vert| q.y < p.y || (q.y == p.y && q.x > p.x);
    let takes = [top_left(b, c), top_left(c, a), top_left(a, b)];
    let outside = |w: f32, takes: bool| w < 0.0 || (w == 0.0 && !takes);
    let row = |y: i32| {
        // Direct3D before version 10 puts pixel centres on integer coordinates.
        let py = y as f32;
        for x in min_x..=max_x {
            let px = x as f32;
            let w0 = edge(b, c, px, py);
            let w1 = edge(c, a, px, py);
            let w2 = edge(a, b, px, py);
            if outside(w0, takes[0]) || outside(w1, takes[1]) || outside(w2, takes[2]) {
                continue;
            }
            let (l0, l1, l2) = (w0 * inv_area, w1 * inv_area, w2 * inv_area);
            let z = l0 * a.z + l1 * b.z + l2 * c.z;
            if let Some(old) = cx.target.zread(cx.m, x as u32, y as u32) {
                if !compare(cx.zfunc, cx.target.zquant(z), old) {
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
            let mut keyed_out = false;
            for (s, stg) in cx.stages.iter().enumerate() {
                let ci = stg.coords;
                let tex = match cx.textures[s].as_ref() {
                    Some(t) => {
                        let u = p0 * a.uv[ci][0] + p1 * b.uv[ci][0] + p2 * c.uv[ci][0];
                        let v = p0 * a.uv[ci][1] + p1 * b.uv[ci][1] + p2 * c.uv[ci][1];
                        let x = t.sample(u, v);
                        if t.d.keyed && x == [0.0; 4] {
                            keyed_out = true;
                        }
                        x
                    }
                    None => [1.0; 4],
                };
                let ca1 = arg(stg.ca1, diffuse, current, tex, cx.factor, specular);
                let ca2 = arg(stg.ca2, diffuse, current, tex, cx.factor, specular);
                let mut next = current;
                for i in 0..3 {
                    next[i] =
                        op(stg.cop, ca1[i], ca2[i], diffuse[3], tex[3], current[3]).clamp(0.0, 1.0);
                }
                if stg.aop != 1 {
                    let aa1 = arg(stg.aa1, diffuse, current, tex, cx.factor, specular)[3];
                    let aa2 = arg(stg.aa2, diffuse, current, tex, cx.factor, specular)[3];
                    next[3] = op(stg.aop, aa1, aa2, diffuse[3], tex[3], current[3]).clamp(0.0, 1.0);
                }
                current = next;
            }
            if cx.specular_add {
                for i in 0..3 {
                    current[i] = (current[i] + specular[i]).min(1.0);
                }
            }
            if cx.keyed && keyed_out {
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
    };
    // Large triangles: rows in parallel (each pixel is written by one row only).
    if (max_x - min_x + 1) * (max_y - min_y + 1) >= 8192 {
        use rayon::prelude::*;
        (min_y..=max_y).into_par_iter().for_each(row);
    } else {
        (min_y..=max_y).for_each(row);
    }
}

/// Whether a triangle shows its texture one texel to one screen pixel, axis-aligned.
#[cfg(feature = "gpu")]
fn texel_for_pixel(t: &[rc_platform::gpu::Vertex; 3], width: f32, height: f32) -> bool {
    let near = |a: f32, b: f32| (a - b).abs() < 0.02;
    let [a, b, c] = t;
    let (x1, y1) = (b.pos[0] - a.pos[0], b.pos[1] - a.pos[1]);
    let (x2, y2) = (c.pos[0] - a.pos[0], c.pos[1] - a.pos[1]);
    let det = x1 * y2 - x2 * y1;
    if det.abs() < 1e-3 {
        return false;
    }
    // The texture coordinates' gradients over the screen (an affine map per triangle).
    let grad = |d1: f32, d2: f32| ((d1 * y2 - d2 * y1) / det, (x1 * d2 - x2 * d1) / det);
    let (ux, uy) = grad(b.uv0[0] - a.uv0[0], c.uv0[0] - a.uv0[0]);
    let (vx, vy) = grad(b.uv0[1] - a.uv0[1], c.uv0[1] - a.uv0[1]);
    near((ux * width).abs(), 1.0)
        && near((vy * height).abs(), 1.0)
        && near(uy * width, 0.0)
        && near(vx * height, 0.0)
}

/// Mark the triangles that map their texture (`width` × `height` texels, coordinates of set 0)
/// one texel to one screen pixel along the axes, 2D drawn with Direct3D (`Vertex::exact`);
/// whether there are any.
#[cfg(feature = "gpu")]
fn mark_texel_for_pixel(tris: &mut [rc_platform::gpu::Vertex], width: f32, height: f32) -> bool {
    let mut any = false;
    for t in tris.as_chunks_mut::<3>().0 {
        if texel_for_pixel(t, width, height) {
            any = true;
            for v in t.iter_mut() {
                v.exact = 1.0;
            }
        }
    }
    any
}

/// The draw as the GPU renderer takes it: culled screen-space triangles, the textures decoded
/// (shared with the software path's cache), the stage setup and the pixel state. `None` when
/// it draws nothing.
#[cfg(feature = "gpu")]
pub fn gpu_draw(
    m: Mem,
    st: &State,
    primitive: u32,
    fvf: u32,
    data: &[u8],
    indices: &[u16],
) -> Option<rc_platform::gpu::Draw> {
    use rc_platform::gpu;
    let verts = vertices(m, st, fvf, data);
    if verts.is_empty() {
        return None;
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
        _ => return None,
    }
    let cull = rs(st, RS_CULLMODE, 3);
    let flat = rs(st, RS_SHADEMODE, 2) == 1;
    let gv = |v: &Vert| gpu::Vertex {
        pos: [v.x, v.y, v.z, v.rhw],
        diffuse: v.diffuse,
        specular: v.specular,
        uv0: v.uv[0],
        uv1: v.uv[1],
        eye: v.eye_z,
        exact: 0.0,
        _pad: [0.0; 2],
    };
    let mut out = Vec::with_capacity(tris.len() * 3);
    for (a, b, c) in tris {
        let (va, mut vb, mut vc) = (verts[a], verts[b], verts[c]);
        if flat {
            vb.diffuse = va.diffuse;
            vc.diffuse = va.diffuse;
            vb.specular = va.specular;
            vc.specular = va.specular;
        }
        for [va, vb, vc] in clip_near([va, vb, vc], &st.viewport) {
            let area = (vb.x - va.x) * (vc.y - va.y) - (vc.x - va.x) * (vb.y - va.y);
            if (cull == 2 && area > 0.0) || (cull == 3 && area < 0.0) {
                continue;
            }
            if va.rhw <= 0.0 || vb.rhw <= 0.0 || vc.rhw <= 0.0 {
                continue;
            }
            out.extend([gv(&va), gv(&vb), gv(&vc)]);
        }
    }
    if out.is_empty() {
        return None;
    }
    let mut textures: [Option<gpu::Texture>; 2] = [None, None];
    let mut enlarged = 1;
    for (s, slot) in textures.iter_mut().enumerate() {
        if let Some((obj, version)) = st.textures[s] {
            if let Some(info) = ddraw::info(m, obj) {
                let key = if rs(st, RS_COLORKEYENABLE, 0) != 0 {
                    info.src_key
                } else {
                    None
                };
                let version = ddraw::version_of(m, obj).max(version);
                // Smooth fonts: the glyphs this texture shows (stage 0's 2D quads) found out,
                // then, if it is a font's page, the page with them redrawn.
                let fonts = rc_platform::render::smooth_fonts() && crate::glyphs::any();
                let tex = u64::from(obj) << 32 | u64::from(version);
                if fonts && s == 0 {
                    for r in exact_rects(&out, info.width as f32, info.height as f32) {
                        crate::glyphs::note(tex, r);
                    }
                }
                let tied = fonts.then(|| crate::glyphs::tied(tex)).flatten();
                let smooth = rc_platform::render::smooth_textures();
                let (d, variant) = enlarge(m, (obj, version), &info, key, smooth, tied)
                    .unwrap_or_else(|| (decoded(m, obj, version, &info, key), 0));
                if s == 0 && variant != 0 {
                    enlarged = d.w / info.width.max(1);
                }
                // Enlarged, a 2D quad (one texel to a pixel of the original) is sampled at its
                // nearest texel: filtered, its edges would take in the packed neighbours.
                let quad_2d = variant != 0
                    && out
                        .as_chunks::<3>()
                        .0
                        .iter()
                        .any(|t| texel_for_pixel(t, info.width as f32, info.height as f32));
                *slot = Some(gpu::Texture {
                    key: (obj, version, u32::from(key.is_some()) | variant),
                    width: d.w,
                    height: d.h,
                    rgba: d.texels.clone(),
                    bilinear: tss(st, s, TSS_MAGFILTER) >= 2 && !quad_2d,
                    clamp: (tss(st, s, TSS_ADDRESSU) == 3, tss(st, s, TSS_ADDRESSV) == 3),
                });
            }
        }
    }
    let mut stages = Vec::new();
    for s in 0..2 {
        let cop = tss(st, s, TSS_COLOROP);
        if cop == 1 {
            break;
        }
        stages.push(gpu::Stage {
            cop,
            ca1: tss(st, s, TSS_COLORARG1),
            ca2: tss(st, s, TSS_COLORARG2),
            aop: tss(st, s, TSS_ALPHAOP),
            aa1: tss(st, s, TSS_ALPHAARG1),
            aa2: tss(st, s, TSS_ALPHAARG2),
            coords: tss(st, s, TSS_TEXCOORDINDEX) & 1,
        });
    }
    let keyed = rs(st, RS_COLORKEYENABLE, 0) != 0
        && st
            .textures
            .iter()
            .take(2)
            .flatten()
            .any(|(o, _)| ddraw::info(m, *o).is_some_and(|i| i.src_key.is_some()));
    let zbuffer = rs(st, RS_ZENABLE, 1) != 0 && ddraw::zbuffer_info(m, st.target).is_some();
    let pixel_exact = textures[0].as_ref().is_some_and(|t| {
        let n = enlarged as f32;
        mark_texel_for_pixel(&mut out, t.width as f32 / n, t.height as f32 / n)
    });
    Some(gpu::Draw {
        pixel_exact,
        enlarged,
        vertices: out,
        textures,
        stages,
        factor: argb(rs(st, RS_TEXTUREFACTOR, 0xffff_ffff)),
        specular_add: rs(st, 29, 0) != 0,
        keyed,
        alpha_test: (rs(st, RS_ALPHATESTENABLE, 0) != 0).then(|| {
            (
                rs(st, RS_ALPHAFUNC, 8),
                rs(st, RS_ALPHAREF, 0) as f32 / 255.0,
            )
        }),
        blend: (rs(st, RS_ALPHABLENDENABLE, 0) != 0)
            .then(|| (rs(st, RS_SRCBLEND, 2), rs(st, RS_DESTBLEND, 1))),
        depth: zbuffer.then(|| (rs(st, RS_ZFUNC, 4), rs(st, RS_ZWRITEENABLE, 1) != 0)),
        fog: (rs(st, RS_FOGENABLE, 0) != 0).then(|| {
            (
                argb(rs(st, RS_FOGCOLOR, 0)),
                (rs(st, RS_FOGTABLEMODE, 0) == 3)
                    .then(|| (rsf(st, RS_FOGSTART, 0.0), rsf(st, RS_FOGEND, 1.0))),
            )
        }),
    })
}

/// Pixels of a surface as RGBA8, and back.
pub fn to_rgba8(m: Mem, s: &SurfaceInfo) -> Vec<u8> {
    let mut out = vec![0u8; (s.width * s.height * 4) as usize];
    if s.width == 0 {
        return out;
    }
    let k = s.masks;
    let bytes = s.bpp / 8;
    let mut row = vec![0u8; (s.width * bytes) as usize];
    let table = (bytes == 2).then(|| table16(k));
    for (y, line) in out.chunks_exact_mut(4 * s.width as usize).enumerate() {
        m.read(s.buf + y as u32 * s.pitch, &mut row);
        let px = line.as_chunks_mut::<4>().0;
        match &table {
            Some(t) => {
                for (p, v) in px.iter_mut().zip(row.as_chunks::<2>().0) {
                    *p = t[usize::from(u16::from_le_bytes(*v))];
                }
            }
            None => {
                for (p, v) in px.iter_mut().zip(row.as_chunks::<4>().0) {
                    let v = u32::from_le_bytes(*v);
                    *p = k.map(|mask| to8(v, mask));
                }
            }
        }
    }
    out
}

/// Converts a surface's rows to RGBA8 as `to_rgba8` does, one row of its bytes at a time.
pub struct RowsToRgba8 {
    masks: [u32; 4],
    bytes: usize,
    table: Option<Table16>,
}

impl RowsToRgba8 {
    pub fn new(s: &SurfaceInfo) -> Self {
        let bytes = (s.bpp / 8) as usize;
        RowsToRgba8 {
            masks: s.masks,
            bytes,
            table: (bytes == 2).then(|| table16(s.masks)),
        }
    }

    /// `row` (the surface's bytes, no padding) into `out` (4 bytes a pixel).
    pub fn convert(&self, row: &[u8], out: &mut [u8]) {
        let px = out.as_chunks_mut::<4>().0;
        match &self.table {
            Some(t) => {
                for (p, v) in px.iter_mut().zip(row.as_chunks::<2>().0) {
                    *p = t[usize::from(u16::from_le_bytes(*v))];
                }
            }
            None => {
                let k = self.masks;
                for (p, v) in px.iter_mut().zip(row.as_chunks::<4>().0) {
                    let v = u32::from_le_bytes(*v);
                    *p = k.map(|mask| to8(v, mask));
                }
            }
        }
        let _ = self.bytes;
    }
}

pub fn from_rgba8(m: Mem, s: &SurfaceInfo, rgba: &[u8]) {
    let k = s.masks;
    // Per channel: each 8-bit value packed into its mask.
    let pack: [[u32; 256]; 4] = k.map(|mask| {
        std::array::from_fn(|v| {
            if mask == 0 {
                return 0;
            }
            let shift = mask.trailing_zeros();
            let max = mask >> shift;
            ((v as u32 * max + 127) / 255) << shift
        })
    });
    let bytes = s.bpp / 8;
    let mut row = vec![0u8; (s.width * bytes) as usize];
    if s.width == 0 {
        return;
    }
    for (y, line) in rgba
        .chunks_exact(4 * s.width as usize)
        .take(s.height as usize)
        .enumerate()
    {
        let px = line.as_chunks::<4>().0;
        let v = |p: &[u8; 4]| {
            pack[0][usize::from(p[0])]
                | pack[1][usize::from(p[1])]
                | pack[2][usize::from(p[2])]
                | pack[3][usize::from(p[3])]
        };
        match bytes {
            2 => {
                for (o, p) in row.as_chunks_mut::<2>().0.iter_mut().zip(px) {
                    *o = (v(p) as u16).to_le_bytes();
                }
            }
            _ => {
                for (o, p) in row.as_chunks_mut::<4>().0.iter_mut().zip(px) {
                    *o = v(p).to_le_bytes();
                }
            }
        }
        m.write(s.buf + y as u32 * s.pitch, &row);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sixteen_bit_table_round_trips() {
        for masks in [
            [0xf800, 0x07e0, 0x001f, 0],
            [0x7c00, 0x03e0, 0x001f, 0x8000],
        ] {
            let t = table16(masks);
            for v in 0..=u32::from(u16::MAX) {
                let p = t[v as usize];
                assert_eq!(p, masks.map(|k| to8(v, k)));
                // Packing it again (as `from_rgba8` does) gives the pixel back.
                let back = masks.iter().zip(p).fold(0, |acc, (&k, c)| {
                    if k == 0 {
                        return acc;
                    }
                    let shift = k.trailing_zeros();
                    acc | ((u32::from(c) * (k >> shift) + 127) / 255) << shift
                });
                let used = masks.iter().fold(0, |a, k| a | k);
                assert_eq!(back, v & used);
            }
        }
    }

    fn vert(c: [f32; 4]) -> Vert {
        let vp = [0.0, 0.0, 640.0, 480.0, 0.0, 1.0];
        let mut v = Vert {
            clip: Some(c),
            ..Vert::default()
        };
        project(&mut v, c, &vp);
        v
    }

    #[test]
    fn near_plane_clipping() {
        let vp = [0.0, 0.0, 640.0, 480.0, 0.0, 1.0];
        let front = [
            vert([0.0, 0.0, 0.5, 1.0]),
            vert([1.0, 0.0, 0.5, 1.0]),
            vert([0.0, 1.0, 0.5, 1.0]),
        ];
        assert_eq!(clip_near(front, &vp).len(), 1);
        let behind = [
            vert([0.0, 0.0, -0.5, 1.0]),
            vert([1.0, 0.0, -0.5, 1.0]),
            vert([0.0, 1.0, -0.1, 1.0]),
        ];
        assert!(clip_near(behind, &vp).is_empty());
        // One vertex behind: a quad in front, two triangles, all on the plane or in front of it.
        let one = [
            vert([0.0, 0.0, -1.0, 1.0]),
            vert([1.0, 0.0, 1.0, 1.0]),
            vert([0.0, 1.0, 1.0, 1.0]),
        ];
        let pieces = clip_near(one, &vp);
        assert_eq!(pieces.len(), 2);
        for t in &pieces {
            for v in t {
                assert!(v.clip.unwrap()[2] >= -1e-6);
            }
        }
        // Two behind: one smaller triangle.
        let two = [
            vert([0.0, 0.0, 1.0, 1.0]),
            vert([1.0, 0.0, -1.0, 1.0]),
            vert([0.0, 1.0, -1.0, 1.0]),
        ];
        assert_eq!(clip_near(two, &vp).len(), 1);
        // Pre-transformed vertices are left alone.
        let raw = [Vert::default(); 3];
        assert_eq!(clip_near(raw, &vp).len(), 1);
    }

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
