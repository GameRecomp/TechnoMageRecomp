// SPDX-License-Identifier: MIT
//! The render target kept on the GPU (the native renderer): draws go on top of what it holds,
//! nothing is read back while the game does not look at its pixels, and the frame shown is made
//! from it on the GPU (with the overlays) and handed to the window as a texture.
//!
//! What guest memory would hold is reproduced exactly by passes over the target: the rounding to
//! the surface's 16-bit format after each batch, the black margins after a flip, and with native
//! resolution the large picture brought down to the surface's pixels and up again (what the next
//! frame's first draws start from). So the pictures stay those of the CPU path, pixel for pixel.

use super::Renderer;
use crate::gpu::{GpuPicture, OverlayImage};

/// A surface as the game sees it: its size, its colour bits per channel (red, green, blue,
/// alpha: 0 for none), its bytes per pixel, the widescreen margin on each side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Guest {
    pub width: u32,
    pub height: u32,
    pub bits: [u32; 4],
    pub bpp: u32,
    pub margin: u32,
}

impl Guest {
    /// Whether its pixels are RGB565 (the format the present table expands by its bits).
    fn rgb565(&self) -> bool {
        self.bpp == 16 && self.bits == [5, 6, 5, 0]
    }
}

const PASSES: &str = r#"
struct P { a: vec4<u32>, b: vec4<u32>, c: vec4<u32>, d: vec4<u32> };
@group(0) @binding(0) var<uniform> p: P;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var lut: texture_2d<f32>;
@group(0) @binding(3) var img: texture_2d<f32>;

// a: target width, height; guest width, height
// b: margin, edge strip x, edge strip y, flags (1 scaled, 2 sixteen bits, 4 gamma, 8 rgb565)
// c: bits of red, green, blue, alpha
// d: the guest's zero pixel as RGBA8

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let q = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(q * 2.0 - 1.0, 0.0, 1.0);
}

fn px(c: vec4<f32>) -> vec4<u32> {
    return vec4<u32>(round(clamp(c, vec4<f32>(0.0), vec4<f32>(1.0)) * 255.0));
}

fn out(v: vec4<u32>) -> vec4<f32> {
    return vec4<f32>(v) / 255.0;
}

fn fetch(t: texture_2d<f32>, x: u32, y: u32) -> vec4<u32> {
    return px(textureLoad(t, vec2<i32>(i32(x), i32(y)), 0));
}

// A channel into `n` bits as the HLE packs it (rounded), and back as it reads it (truncated).
fn quant1(v: u32, n: u32) -> u32 {
    if (n == 0u) { return 255u; }
    let m = (1u << n) - 1u;
    let c = (v * m + 127u) / 255u;
    return c * 255u / m;
}

fn quant(v: vec4<u32>) -> vec4<u32> {
    return vec4<u32>(quant1(v.x, p.c.x), quant1(v.y, p.c.y), quant1(v.z, p.c.z), quant1(v.w, p.c.w));
}

// The guest pixel a target column or row shows (scaled: nearest), and the target pixel at the
// centre of a guest pixel.
fn low(x: u32, size: u32, gsize: u32) -> u32 {
    if ((p.b.w & 1u) == 0u) { return x; }
    return min(x * gsize / size, gsize - 1u);
}

fn centre(l: u32, size: u32, gsize: u32) -> u32 {
    if ((p.b.w & 1u) == 0u) { return l; }
    return min((2u * l + 1u) * size / (2u * gsize), size - 1u);
}

fn in_margin(lx: u32) -> bool {
    return p.b.x > 0u && (lx < p.b.x || lx >= p.a.z - p.b.x);
}

// The target's edge strips (a picture drawn larger: the pixels before the first screen pixel's
// centre) as the first pixel after them.
@fragment
fn edges(@builtin(position) f: vec4<f32>) -> @location(0) vec4<f32> {
    let x = max(u32(f.x), p.b.y);
    let y = max(u32(f.y), p.b.z);
    return out(fetch(src, x, y));
}

// As written into the guest surface and read again.
@fragment
fn quantize(@builtin(position) f: vec4<f32>) -> @location(0) vec4<f32> {
    return out(quant(fetch(src, u32(f.x), u32(f.y))));
}

// The margins black (the guest's zero pixel) after a flip.
@fragment
fn margins(@builtin(position) f: vec4<f32>) -> @location(0) vec4<f32> {
    let x = u32(f.x);
    if (in_margin(low(x, p.a.x, p.a.z))) { return out(p.d); }
    return out(fetch(src, x, u32(f.y)));
}

// Scaled: what the guest holds (the centre pixel of each of its pixels, rounded), enlarged.
@fragment
fn reduce(@builtin(position) f: vec4<f32>) -> @location(0) vec4<f32> {
    let lx = low(u32(f.x), p.a.x, p.a.z);
    let ly = low(u32(f.y), p.a.y, p.a.w);
    if (in_margin(lx)) { return out(p.d); }
    return out(quant(fetch(src, centre(lx, p.a.x, p.a.z), centre(ly, p.a.y, p.a.w))));
}

// The guest surface's pixels (drawn at its size): the target's at their centres.
@fragment
fn guest(@builtin(position) f: vec4<f32>) -> @location(0) vec4<f32> {
    return out(fetch(src, centre(u32(f.x), p.a.x, p.a.z), centre(u32(f.y), p.a.y, p.a.w)));
}

fn gamma(v: vec4<u32>) -> vec4<u32> {
    if ((p.b.w & 4u) == 0u) { return v; }
    return vec4<u32>(fetch(lut, v.x, 0u).x, fetch(lut, v.y, 0u).y, fetch(lut, v.z, 0u).z, v.w);
}

// A target pixel as the window shows it: a 16-bit surface's RGB565 expanded by repeating its
// bits, every other one as it is; through the gamma ramp.
fn shown(x: u32, y: u32) -> vec4<u32> {
    var v = fetch(src, x, y);
    if ((p.b.w & 1u) == 0u) {
        if ((p.b.w & 8u) != 0u) {
            let r = (v.x * 31u + 127u) / 255u;
            let g = (v.y * 63u + 127u) / 255u;
            let b = (v.z * 31u + 127u) / 255u;
            v = vec4<u32>((r << 3u) | (r >> 2u), (g << 2u) | (g >> 4u), (b << 3u) | (b >> 2u), 255u);
        } else {
            v = vec4<u32>(v.xyz, 255u);
        }
    }
    return gamma(v);
}

@fragment
fn present(@builtin(position) f: vec4<f32>) -> @location(0) vec4<f32> {
    return out(shown(u32(f.x), u32(f.y)));
}

// An overlay picture (premultiplied, its alpha the coverage) over the frame.
// a: frame width, height; picture x, y (as i32 bits)
// b: picture width, height; 1 for a gradient; the present flags (as for `shown`)
// c: gradient: region x0, x1, y0, y1 (frame pixels); d: gradient: column left, right
@vertex
fn vs_quad(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let q = vec2<f32>(f32(i & 1u), f32((i >> 1u) & 1u));
    let at = vec2<f32>(f32(bitcast<i32>(p.a.z)), f32(bitcast<i32>(p.a.w))) + q * vec2<f32>(p.b.xy);
    let ndc = at / vec2<f32>(p.a.xy) * 2.0 - 1.0;
    return vec4<f32>(ndc.x, -ndc.y, 0.0, 1.0);
}

@fragment
fn overlay(@builtin(position) f: vec4<f32>) -> @location(0) vec4<f32> {
    let x = u32(f.x);
    let y = u32(f.y);
    var base = vec4<f32>(shown(x, y));
    if (p.b.z == 1u && x >= p.c.x && x < p.c.y && y >= p.c.z && y < p.c.w) {
        let l = vec4<f32>(shown(p.d.x, y));
        let r = vec4<f32>(shown(p.d.y, y));
        let t = f32(x - p.c.x) / f32(max(p.c.y - p.c.x, 1u));
        base = l * (1.0 - t) + r * t;
    }
    let ix = i32(x) - bitcast<i32>(p.a.z);
    let iy = i32(y) - bitcast<i32>(p.a.w);
    let c = textureLoad(img, vec2<i32>(ix, iy), 0);
    let o = base / 255.0 * (1.0 - c.a) + vec4<f32>(c.rgb, 0.0);
    return vec4<f32>(o.rgb, 1.0);
}
"#;

/// The passes' entry points.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Pass {
    Edges,
    Quantize,
    Margins,
    Reduce,
    Guest,
    Present,
    Overlay,
}

impl Pass {
    fn entry(self) -> &'static str {
        match self {
            Pass::Edges => "edges",
            Pass::Quantize => "quantize",
            Pass::Margins => "margins",
            Pass::Reduce => "reduce",
            Pass::Guest => "guest",
            Pass::Present => "present",
            Pass::Overlay => "overlay",
        }
    }
}

/// The resident state's GPU objects (made on first use).
pub(crate) struct Resident {
    module: wgpu::ShaderModule,
    layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    pipelines: std::collections::HashMap<Pass, wgpu::RenderPipeline>,
    /// Scratch the passes draw into (then swapped with the target's colour), by size.
    scratch: Option<wgpu::Texture>,
    /// The gamma ramp as a 256 × 1 texture, and the ramp it holds.
    lut: wgpu::Texture,
    lut_of: Option<Vec<u16>>,
    /// The frame shown, by size; and the picture of the surface flipped to the front (guest
    /// view made on demand).
    present: Option<wgpu::Texture>,
    front: Option<wgpu::Texture>,
    /// Overlay pictures by key, and the frame each was last used in.
    images: std::collections::HashMap<u64, (wgpu::Texture, u64)>,
    frame: u64,
    /// The last frame shown's work: the next waits for it, so the game is at most a frame
    /// ahead of the GPU.
    last: Option<wgpu::SubmissionIndex>,
}

fn texture(
    device: &wgpu::Device,
    label: &str,
    (width, height): (u32, u32),
    usage: wgpu::TextureUsages,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage,
        view_formats: &[],
    })
}

const ALL: wgpu::TextureUsages = wgpu::TextureUsages::RENDER_ATTACHMENT
    .union(wgpu::TextureUsages::TEXTURE_BINDING)
    .union(wgpu::TextureUsages::COPY_SRC)
    .union(wgpu::TextureUsages::COPY_DST);

fn size_of(t: &wgpu::Texture) -> (u32, u32) {
    (t.width(), t.height())
}

impl Resident {
    pub(crate) fn new(device: &wgpu::Device) -> Resident {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("resident passes"),
            source: wgpu::ShaderSource::Wgsl(PASSES.into()),
        });
        let tex = |b: u32| wgpu::BindGroupLayoutEntry {
            binding: b,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("resident passes"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                tex(1),
                tex(2),
                tex(3),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("resident passes"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        Resident {
            module,
            layout,
            pipeline_layout,
            pipelines: std::collections::HashMap::new(),
            scratch: None,
            lut: texture(device, "gamma", (256, 1), ALL),
            lut_of: None,
            present: None,
            front: None,
            images: std::collections::HashMap::new(),
            frame: 0,
            last: None,
        }
    }

    fn pipeline(&mut self, device: &wgpu::Device, pass: Pass) -> wgpu::RenderPipeline {
        let (module, layout) = (&self.module, &self.pipeline_layout);
        self.pipelines
            .entry(pass)
            .or_insert_with(|| {
                device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some(pass.entry()),
                    layout: Some(layout),
                    vertex: wgpu::VertexState {
                        module,
                        entry_point: Some(if pass == Pass::Overlay {
                            "vs_quad"
                        } else {
                            "vs"
                        }),
                        compilation_options: Default::default(),
                        buffers: &[],
                    },
                    primitive: wgpu::PrimitiveState {
                        topology: if pass == Pass::Overlay {
                            wgpu::PrimitiveTopology::TriangleStrip
                        } else {
                            wgpu::PrimitiveTopology::TriangleList
                        },
                        ..Default::default()
                    },
                    depth_stencil: None,
                    multisample: wgpu::MultisampleState::default(),
                    fragment: Some(wgpu::FragmentState {
                        module,
                        entry_point: Some(pass.entry()),
                        compilation_options: Default::default(),
                        targets: &[Some(wgpu::ColorTargetState {
                            format: wgpu::TextureFormat::Rgba8Unorm,
                            blend: None,
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
                    }),
                    multiview_mask: None,
                    cache: None,
                })
            })
            .clone()
    }
}

/// The uniforms of a pass for `guest` on a target of `size`.
fn uniforms(size: (u32, u32), guest: &Guest, flags: u32, strip: (u32, u32)) -> [u32; 16] {
    let zero = |n: u32| if n == 0 { 255 } else { 0 };
    let scaled = u32::from(size != (guest.width, guest.height));
    [
        size.0,
        size.1,
        guest.width,
        guest.height,
        guest.margin,
        strip.0,
        strip.1,
        flags | scaled | if guest.bpp == 16 { 2 } else { 0 },
        guest.bits[0],
        guest.bits[1],
        guest.bits[2],
        guest.bits[3],
        zero(guest.bits[0]),
        zero(guest.bits[1]),
        zero(guest.bits[2]),
        zero(guest.bits[3]),
    ]
}

/// The pixels of the target before the first screen pixel's centre (as `fill_edges` has them).
fn strip(size: u32, view: u32) -> u32 {
    let s = size as f32 / view as f32;
    (((s - 1.0) / 2.0).ceil().max(0.0) as u32).min(size - 1)
}

impl Renderer {
    fn resident(&mut self) -> &mut Resident {
        if self.resident.is_none() {
            self.resident = Some(Resident::new(&self.device));
        }
        self.resident.as_mut().expect("resident")
    }

    /// Run `pass` from `src` into `dst` (at `dst`'s size).
    #[allow(clippy::too_many_arguments)]
    fn run_pass(
        &mut self,
        enc: &mut wgpu::CommandEncoder,
        pass: Pass,
        u: [u32; 16],
        src: &wgpu::Texture,
        dst: &wgpu::Texture,
        load: bool,
        quad: Option<&wgpu::Texture>,
    ) {
        use wgpu::util::DeviceExt;
        let device = self.device.clone();
        let pipeline = self.resident().pipeline(&device, pass);
        let r = self.resident.as_ref().expect("resident");
        let ubuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("pass"),
            contents: bytemuck::cast_slice(&u),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let view = |t: &wgpu::Texture| t.create_view(&wgpu::TextureViewDescriptor::default());
        let (sv, lv) = (view(src), view(&r.lut));
        let iv = view(quad.unwrap_or(&r.lut));
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("pass"),
            layout: &r.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: ubuf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&sv),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&lv),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&iv),
                },
            ],
        });
        let dv = view(dst);
        let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(pass.entry()),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &dv,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: if load {
                        wgpu::LoadOp::Load
                    } else {
                        wgpu::LoadOp::Clear(wgpu::Color::BLACK)
                    },
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        rp.set_pipeline(&pipeline);
        rp.set_bind_group(0, &bind, &[]);
        rp.draw(0..if quad.is_some() { 4 } else { 3 }, 0..1);
    }

    /// A pass over the target in place (into the scratch, which then becomes the target).
    fn target_pass(&mut self, enc: &mut wgpu::CommandEncoder, pass: Pass, u: [u32; 16]) {
        let Some(color) = self.target.as_ref().map(|t| t.color.clone()) else {
            return;
        };
        let size = size_of(&color);
        let scratch = match self.resident().scratch.take() {
            Some(s) if size_of(&s) == size => s,
            _ => texture(&self.device, "scratch", size, ALL),
        };
        self.run_pass(enc, pass, u, &color, &scratch, false, None);
        if let Some(t) = self.target.as_mut() {
            t.color = scratch;
        }
        self.resident().scratch = Some(color);
    }

    /// Whether the target is `size` (it holds a picture to draw on).
    pub fn target_is(&self, size: (u32, u32)) -> bool {
        self.target
            .as_ref()
            .is_some_and(|t| (t.width, t.height) == size)
    }

    /// Draw `commands` on the target (uploading `background` first if given) and make it what
    /// guest memory would hold: rounded to the surface's format, or (drawn larger) its edge
    /// strips filled. Nothing is read back.
    pub fn draw_resident(
        &mut self,
        size: (u32, u32),
        view: (u32, u32),
        guest: &Guest,
        background: Option<&[u8]>,
        commands: &[crate::gpu::Command],
    ) {
        let _ = self.render_inner(size.0, size.1, view, background, commands, false);
        let mut enc = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        if size == (guest.width, guest.height) {
            if guest.bits.iter().any(|&b| b != 8 && b != 0) {
                self.target_pass(&mut enc, Pass::Quantize, uniforms(size, guest, 0, (0, 0)));
            }
        } else {
            let st = (strip(size.0, view.0), strip(size.1, view.1));
            if st != (0, 0) {
                self.target_pass(&mut enc, Pass::Edges, uniforms(size, guest, 0, st));
            }
        }
        self.queue.submit([enc.finish()]);
    }

    /// After a flip: the margins black, as the HLE clears them in guest memory.
    pub fn clear_margins(&mut self, guest: &Guest) {
        let Some(size) = self.target.as_ref().map(|t| (t.width, t.height)) else {
            return;
        };
        if guest.margin == 0 {
            return;
        }
        let mut enc = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        self.target_pass(&mut enc, Pass::Margins, uniforms(size, guest, 0, (0, 0)));
        self.queue.submit([enc.finish()]);
    }

    /// Drawn larger, before a new frame's first draws: the target as guest memory holds it
    /// (each pixel's centre, rounded), enlarged again.
    pub fn reduce(&mut self, guest: &Guest) {
        let Some(size) = self.target.as_ref().map(|t| (t.width, t.height)) else {
            return;
        };
        let mut enc = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        self.target_pass(&mut enc, Pass::Reduce, uniforms(size, guest, 0, (0, 0)));
        self.queue.submit([enc.finish()]);
    }

    /// Wait until the GPU has done all that was handed to it (for measuring).
    pub fn wait(&self) {
        let _ = self.device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        });
    }

    /// The target read back whole (RGBA8 at its size).
    pub fn read_target(&mut self) -> Option<Vec<u8>> {
        let color = self.target.as_ref()?.color.clone();
        Some(self.read_texture(&color))
    }

    /// What guest memory holds of the target (or of the picture flipped to the front, `front`):
    /// RGBA8 at the guest's size, each pixel the target's at its centre.
    pub fn read_guest(&mut self, guest: &Guest, front: bool) -> Option<Vec<u8>> {
        let src = if front {
            self.resident.as_ref()?.front.clone()?
        } else {
            self.target.as_ref()?.color.clone()
        };
        let size = size_of(&src);
        if size == (guest.width, guest.height) {
            return Some(self.read_texture(&src));
        }
        let low = texture(&self.device, "guest", (guest.width, guest.height), ALL);
        let mut enc = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        self.run_pass(
            &mut enc,
            Pass::Guest,
            uniforms(size, guest, 0, (0, 0)),
            &src,
            &low,
            false,
            None,
        );
        self.queue.submit([enc.finish()]);
        Some(self.read_texture(&low))
    }

    /// At a flip: the target kept as the picture of the front surface (read on demand).
    pub fn keep_front(&mut self) {
        let Some(color) = self.target.as_ref().map(|t| t.color.clone()) else {
            return;
        };
        let size = size_of(&color);
        let front = match self.resident().front.take() {
            Some(f) if size_of(&f) == size => f,
            _ => texture(&self.device, "front", size, ALL),
        };
        let mut enc = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        copy(&mut enc, &color, &front, size);
        self.queue.submit([enc.finish()]);
        self.resident().front = Some(front);
    }

    /// The frame shown: the target as the window shows it (through `gamma`, a 16-bit surface
    /// expanded as the HLE does) with the `overlays` over it, on the GPU.
    pub fn present(
        &mut self,
        guest: &Guest,
        gamma: Option<&[u16]>,
        overlays: &[OverlayImage],
        game: (u32, u32),
    ) -> Option<GpuPicture> {
        let color = self.target.as_ref()?.color.clone();
        let size = size_of(&color);
        let device = self.device.clone();
        let queue = self.queue.clone();
        let r = self.resident();
        if let Some(last) = r.last.take() {
            let _ = device.poll(wgpu::PollType::Wait {
                submission_index: Some(last),
                timeout: None,
            });
        }
        r.frame += 1;
        if r.lut_of.as_deref() != gamma {
            if let Some(g) = gamma {
                let mut bytes = vec![0u8; 256 * 4];
                for v in 0..256 {
                    for c in 0..3 {
                        bytes[4 * v + c] = (g[c * 256 + v] >> 8) as u8;
                    }
                }
                write(&queue, &r.lut, (256, 1), &bytes);
            }
            r.lut_of = gamma.map(<[u16]>::to_vec);
        }
        let present = match r.present.take() {
            Some(p) if size_of(&p) == size => p,
            _ => texture(&device, "present", size, ALL),
        };
        // The overlay pictures, made once each.
        let frame = r.frame;
        for o in overlays {
            r.images
                .entry(o.key)
                .and_modify(|e| e.1 = frame)
                .or_insert_with(|| {
                    let t = texture(&device, "overlay", (o.width.max(1), o.height.max(1)), ALL);
                    write(&queue, &t, (o.width.max(1), o.height.max(1)), &o.premul);
                    (t, frame)
                });
        }
        r.images.retain(|_, e| frame - e.1 < 120);
        let flags = if gamma.is_some() { 4 } else { 0 } | if guest.rgb565() { 8 } else { 0 };
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        let u = uniforms(size, guest, flags, (0, 0));
        self.run_pass(&mut enc, Pass::Present, u, &color, &present, false, None);
        for o in overlays {
            let img = self.resident.as_ref()?.images.get(&o.key)?.0.clone();
            let g = o.gradient.unwrap_or_default();
            let q = [
                size.0,
                size.1,
                o.x as u32,
                o.y as u32,
                o.width,
                o.height,
                u32::from(o.gradient.is_some()),
                u[7],
                g[0],
                g[1],
                g[2],
                g[3],
                g[4],
                g[5],
                0,
                0,
            ];
            self.run_pass(
                &mut enc,
                Pass::Overlay,
                q,
                &color,
                &present,
                true,
                Some(&img),
            );
        }
        let index = queue.submit([enc.finish()]);
        let r = self.resident();
        r.present = Some(present.clone());
        r.last = Some(index);
        Some(GpuPicture {
            texture: present,
            width: size.0,
            height: size.1,
            game,
        })
    }

    /// A texture read back (RGBA8, rows unpadded).
    pub(crate) fn read_texture(&mut self, t: &wgpu::Texture) -> Vec<u8> {
        read(&self.device, &self.queue, t)
    }
}

/// Copy `src` into `dst` (both `size`).
fn copy(
    enc: &mut wgpu::CommandEncoder,
    src: &wgpu::Texture,
    dst: &wgpu::Texture,
    size: (u32, u32),
) {
    enc.copy_texture_to_texture(
        src.as_image_copy(),
        dst.as_image_copy(),
        wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        },
    );
}

fn write(queue: &wgpu::Queue, t: &wgpu::Texture, size: (u32, u32), rgba: &[u8]) {
    queue.write_texture(
        t.as_image_copy(),
        rgba,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4 * size.0),
            rows_per_image: Some(size.1),
        },
        wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        },
    );
}

/// A texture's pixels read back: RGBA8, rows without padding.
pub fn read(device: &wgpu::Device, queue: &wgpu::Queue, t: &wgpu::Texture) -> Vec<u8> {
    let (width, height) = size_of(t);
    let row = (4 * width).div_ceil(256) * 256;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("read"),
        size: u64::from(row * height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    enc.copy_texture_to_buffer(
        t.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([enc.finish()]);
    let slice = buffer.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    let _ = device.poll(wgpu::PollType::Wait {
        submission_index: None,
        timeout: None,
    });
    let Ok(data) = slice.get_mapped_range() else {
        return vec![0; (4 * width * height) as usize];
    };
    let mut out = Vec::with_capacity((4 * width * height) as usize);
    for y in 0..height as usize {
        out.extend_from_slice(&data[y * row as usize..y * row as usize + 4 * width as usize]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The passes' shader compiles (where a GPU is at hand).
    #[test]
    fn passes_compile() {
        let Some(r) = Renderer::new(false) else {
            return;
        };
        let scope = r.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut res = Resident::new(&r.device);
        for p in [
            Pass::Edges,
            Pass::Quantize,
            Pass::Margins,
            Pass::Reduce,
            Pass::Guest,
            Pass::Present,
            Pass::Overlay,
        ] {
            res.pipeline(&r.device, p);
        }
        let error = pollster::block_on(scope.pop());
        assert!(error.is_none(), "{error:?}");
    }
}
