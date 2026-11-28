// SPDX-License-Identifier: MIT
//! The renderer on wgpu: a device of its own, one WGSL shader for every draw.

use super::{Command, Draw, Texture, Vertex};

#[path = "resident.rs"]
mod resident;
pub use resident::read;
use std::collections::HashMap;
use wgpu::util::DeviceExt;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    /// Per stage: cop, ca1, ca2, aop / aa1, aa2, coords, enabled.
    stage: [[u32; 4]; 4],
    factor: [f32; 4],
    fog_color: [f32; 4],
    /// viewport width, height, fog start, fog end
    view: [f32; 4],
    /// alpha func (0 off), alpha ref bits, fog mode (0 off, 1 vertex, 2 table), flags
    /// (1 specular add, 2 keyed, 4 texture 0, 8 texture 1)
    misc: [u32; 4],
    /// Target pixels per screen pixel (x, y), 1 to sample a pixel-exact draw at the screen's
    /// pixel centres, and how much larger texture 0 is (`Draw::enlarged`, 0 when it is not).
    snap: [f32; 4],
    _pad: [u32; 4],
}

const SHADER: &str = r#"
struct U {
    stage: array<vec4<u32>, 4>,
    factor: vec4<f32>,
    fog_color: vec4<f32>,
    view: vec4<f32>,
    misc: vec4<u32>,
    snap: vec4<f32>,
};
@group(0) @binding(0) var<uniform> u: U;
@group(1) @binding(0) var t0: texture_2d<f32>;
@group(1) @binding(1) var s0: sampler;
@group(1) @binding(2) var t1: texture_2d<f32>;
@group(1) @binding(3) var s1: sampler;

struct In {
    @location(0) pos: vec4<f32>,
    @location(1) diffuse: vec4<f32>,
    @location(2) specular: vec4<f32>,
    @location(3) uv0: vec2<f32>,
    @location(4) uv1: vec2<f32>,
    @location(5) eye: f32,
    @location(6) exact: f32,
    @location(7) box0: vec4<f32>,
};
struct Out {
    @builtin(position) clip: vec4<f32>,
    @location(0) diffuse: vec4<f32>,
    @location(1) specular: vec4<f32>,
    @location(2) uv0: vec2<f32>,
    @location(3) uv1: vec2<f32>,
    @location(4) eye: f32,
    @location(5) @interpolate(flat) exact: f32,
    @location(6) @interpolate(flat) box0: vec4<f32>,
};

@vertex
fn vs(i: In) -> Out {
    var o: Out;
    // Direct3D 7 has pixel centres on integer coordinates: half a pixel to the GPU's.
    let x = (i.pos.x + 0.5) / u.view.x * 2.0 - 1.0;
    let y = 1.0 - (i.pos.y + 0.5) / u.view.y * 2.0;
    var w = 1.0;
    if (i.pos.w > 0.0) { w = 1.0 / i.pos.w; }
    o.clip = vec4<f32>(x * w, y * w, clamp(i.pos.z, 0.0, 1.0) * w, w);
    o.diffuse = i.diffuse;
    o.specular = i.specular;
    o.uv0 = i.uv0;
    o.uv1 = i.uv1;
    o.eye = i.eye;
    o.exact = i.exact;
    o.box0 = i.box0;
    return o;
}

fn arg(a: u32, diffuse: vec4<f32>, current: vec4<f32>, tex: vec4<f32>, spec: vec4<f32>) -> vec4<f32> {
    var v = current;
    switch (a & 15u) {
        case 0u: { v = diffuse; }
        case 1u: { v = current; }
        case 2u: { v = tex; }
        case 3u: { v = u.factor; }
        case 4u: { v = spec; }
        default: { v = current; }
    }
    if ((a & 32u) != 0u) { v = vec4<f32>(v.a); }
    if ((a & 16u) != 0u) { v = vec4<f32>(1.0) - v; }
    return v;
}

fn op(o: u32, a1: vec4<f32>, a2: vec4<f32>, ad: f32, at: f32, ac: f32) -> vec4<f32> {
    switch (o) {
        case 2u: { return a1; }
        case 3u: { return a2; }
        case 4u: { return a1 * a2; }
        case 5u: { return min(a1 * a2 * 2.0, vec4<f32>(1.0)); }
        case 6u: { return min(a1 * a2 * 4.0, vec4<f32>(1.0)); }
        case 7u: { return min(a1 + a2, vec4<f32>(1.0)); }
        case 8u: { return clamp(a1 + a2 - 0.5, vec4<f32>(0.0), vec4<f32>(1.0)); }
        case 9u: { return clamp((a1 + a2 - 0.5) * 2.0, vec4<f32>(0.0), vec4<f32>(1.0)); }
        case 10u: { return a1 - a2; }
        case 11u: { return a1 + a2 - a1 * a2; }
        case 12u: { return a1 * ad + a2 * (1.0 - ad); }
        case 13u: { return a1 * at + a2 * (1.0 - at); }
        case 15u: { return a1 * ac + a2 * (1.0 - ac); }
        default: { return a1 * a2; }
    }
}

fn compare(f: u32, a: f32, b: f32) -> bool {
    switch (f) {
        case 1u: { return false; }
        case 2u: { return a < b; }
        case 3u: { return a == b; }
        case 4u: { return a <= b; }
        case 5u: { return a > b; }
        case 6u: { return a != b; }
        case 7u: { return a >= b; }
        default: { return true; }
    }
}

@fragment
fn fs(i: Out) -> @location(0) vec4<f32> {
    // A pixel-exact draw on a larger target: its coordinates moved to the centre of the screen
    // pixel this pixel belongs to (along their gradients).
    let centre = (floor(i.clip.xy / u.snap.xy) + 0.5) * u.snap.xy;
    let d = select(vec2<f32>(0.0), centre - i.clip.xy, u.snap.z != 0.0 && i.exact > 0.5);
    var uv0 = i.uv0 + dpdx(i.uv0) * d.x + dpdy(i.uv0) * d.y;
    // An enlarged texture on a pixel-exact triangle: the texel the game's pixel shows (at its
    // centre), sampled within at where this pixel lies in the game's pixel.
    let game = i.clip.xy / u.snap.xy;
    let dc = (floor(game) + 0.5) * u.snap.xy - i.clip.xy;
    let gx = dpdx(i.uv0);
    let gy = dpdy(i.uv0);
    let at_centre = i.uv0 + gx * dc.x + gy * dc.y;
    let size = vec2<f32>(textureDimensions(t0)) / max(u.snap.w, 1.0);
    let f = fract(game);
    // Half an enlarged texel inside the game's texel at most: filtered, a sample nearer its edge
    // takes in the next texel on the page (beside a glyph a transparent black one, a dark line;
    // beside a piece of a window's frame the next piece).
    let inset = 0.5 / max(u.snap.w, 1.0);
    let within = clamp(
        vec2<f32>(select(f.x, 1.0 - f.x, gx.x < 0.0), select(f.y, 1.0 - f.y, gy.y < 0.0)),
        vec2<f32>(inset),
        vec2<f32>(1.0 - inset),
    );
    let sub = (floor(at_centre * size) + within) / size;
    uv0 = select(uv0, sub, u.snap.w > 1.5 && i.exact > 0.5);
    // A piece's texels (`Vertex::box0`): filtering takes nothing from beside them on the page.
    let texels = vec2<f32>(textureDimensions(t0));
    let mid = (i.box0.xy + i.box0.zw) * 0.5;
    let within_box = clamp(
        uv0,
        min(i.box0.xy + 0.5 / texels, mid),
        max(i.box0.zw - 0.5 / texels, mid),
    );
    uv0 = select(within_box, uv0, i.box0.z < i.box0.x);
    let uv1 = i.uv1 + dpdx(i.uv1) * d.x + dpdy(i.uv1) * d.y;
    // Both textures are sampled in uniform control flow, then used as the stages say.
    let c0 = textureSample(t0, s0, select(uv0, uv1, u.stage[1].z == 1u));
    let c1 = textureSample(t1, s1, select(uv0, uv1, u.stage[3].z == 1u));
    var current = i.diffuse;
    var keyed = false;
    for (var s = 0u; s < 2u; s = s + 1u) {
        let a = u.stage[2u * s];
        let b = u.stage[2u * s + 1u];
        if (b.w == 0u) { break; }
        var tex = vec4<f32>(1.0);
        if (s == 0u && (u.misc.w & 4u) != 0u) { tex = c0; }
        if (s == 1u && (u.misc.w & 8u) != 0u) { tex = c1; }
        if ((u.misc.w & 2u) != 0u && all(tex == vec4<f32>(0.0))) { keyed = true; }
        var next = current;
        let c = op(a.x, arg(a.y, i.diffuse, current, tex, i.specular), arg(a.z, i.diffuse, current, tex, i.specular), i.diffuse.a, tex.a, current.a);
        next = vec4<f32>(clamp(c.rgb, vec3<f32>(0.0), vec3<f32>(1.0)), current.a);
        if (a.w != 1u) {
            let al = op(a.w, arg(b.x, i.diffuse, current, tex, i.specular), arg(b.y, i.diffuse, current, tex, i.specular), i.diffuse.a, tex.a, current.a);
            next.a = clamp(al.a, 0.0, 1.0);
        }
        current = next;
    }
    if ((u.misc.w & 1u) != 0u) { current = vec4<f32>(min(current.rgb + i.specular.rgb, vec3<f32>(1.0)), current.a); }
    if (keyed) { discard; }
    if (u.misc.x != 0u && !compare(u.misc.x, current.a, bitcast<f32>(u.misc.y))) { discard; }
    if (u.misc.z == 1u) {
        current = vec4<f32>(mix(u.fog_color.rgb, current.rgb, clamp(i.specular.a, 0.0, 1.0)), current.a);
    } else if (u.misc.z == 2u) {
        var f = 1.0;
        if (u.view.w > u.view.z) { f = clamp((u.view.w - i.eye) / (u.view.w - u.view.z), 0.0, 1.0); }
        current = vec4<f32>(mix(u.fog_color.rgb, current.rgb, f), current.a);
    }
    return current;
}
"#;

fn blend_factor(f: u32) -> wgpu::BlendFactor {
    use wgpu::BlendFactor as B;
    match f {
        1 => B::Zero,
        2 => B::One,
        3 => B::Src,
        4 => B::OneMinusSrc,
        5 => B::SrcAlpha,
        6 => B::OneMinusSrcAlpha,
        7 => B::DstAlpha,
        8 => B::OneMinusDstAlpha,
        9 => B::Dst,
        10 => B::OneMinusDst,
        11 => B::SrcAlphaSaturated,
        _ => B::One,
    }
}

fn compare_fn(f: u32) -> wgpu::CompareFunction {
    use wgpu::CompareFunction as C;
    match f {
        1 => C::Never,
        2 => C::Less,
        3 => C::Equal,
        4 => C::LessEqual,
        5 => C::Greater,
        6 => C::NotEqual,
        7 => C::GreaterEqual,
        _ => C::Always,
    }
}

/// Blend factors, depth test and write, the target's samples per pixel, and whether it draws
/// rims (see [`rims`]: depth pushed back).
type PipelineKey = (Option<(u32, u32)>, Option<(u32, bool)>, u32, bool);

/// Samples per pixel with antialiasing on (`render::antialias`).
const MSAA: u32 = 4;

/// The target's pixels into a multisampled target: every sample of a pixel gets its colour.
const BLIT: &str = r#"
@group(0) @binding(0) var src: texture_2d<f32>;

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    return textureLoad(src, vec2<i32>(p.xy), 0);
}
"#;

/// The render target: its size and samples, the single-sampled colour the CPU's pixels go in
/// and out through, the depth, and with several samples the multisampled colour.
struct Target {
    width: u32,
    height: u32,
    samples: u32,
    color: wgpu::Texture,
    depth: wgpu::Texture,
    msaa: Option<wgpu::Texture>,
}

/// The renderer: a wgpu device of its own and the caches.
pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    shader: wgpu::ShaderModule,
    layout: wgpu::PipelineLayout,
    uniform_layout: wgpu::BindGroupLayout,
    texture_layout: wgpu::BindGroupLayout,
    pipelines: HashMap<PipelineKey, wgpu::RenderPipeline>,
    textures: HashMap<(u32, u32, u32), (wgpu::TextureView, u64)>,
    samplers: HashMap<(bool, bool, bool), wgpu::Sampler>,
    white: wgpu::TextureView,
    target: Option<Target>,
    frame: u64,
    /// Whether the adapter renders the target's formats with `MSAA` samples.
    msaa: bool,
    blit_layout: wgpu::BindGroupLayout,
    blit: Option<wgpu::RenderPipeline>,
    /// The buffer results are read back through, kept while its size fits.
    readback: Option<(u64, wgpu::Buffer)>,
    /// The target kept on the GPU (see `resident`).
    resident: Option<resident::Resident>,
}

impl Renderer {
    /// A renderer on the system's GPU, or `None` without one (or when `soft` asks for none).
    pub fn new(soft: bool) -> Option<Renderer> {
        if soft {
            return None;
        }
        if let Some(s) = super::shared() {
            eprintln!("[gpu] the renderer draws on the window's GPU");
            return Some(Self::with_device(
                &s.adapter,
                s.device.clone(),
                s.queue.clone(),
            ));
        }
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
            ..Default::default()
        }))
        .ok()?;
        // The least limits every GPU offers, but textures as large as this one takes: a render
        // target at native resolution is as wide as the window (wider than 2048 on a large or
        // widescreen display).
        let limits = wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits());
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("technomage renderer"),
            required_features: wgpu::Features::empty(),
            required_limits: limits,
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .ok()?;
        Some(Self::with_device(&adapter, device, queue))
    }

    /// The renderer on `device` (its own, or the window's).
    fn with_device(adapter: &wgpu::Adapter, device: wgpu::Device, queue: wgpu::Queue) -> Renderer {
        let msaa = [
            wgpu::TextureFormat::Rgba8Unorm,
            wgpu::TextureFormat::Depth32Float,
        ]
        .iter()
        .all(|&f| {
            adapter
                .get_texture_format_features(f)
                .flags
                .sample_count_supported(MSAA)
        });
        crate::render::set_max_texture(device.limits().max_texture_dimension_2d);
        super::set_device_of_frames(&device, &queue);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("fixed function"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let uniform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("uniforms"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let tex = |b: u32| wgpu::BindGroupLayoutEntry {
            binding: b,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let smp = |b: u32| wgpu::BindGroupLayoutEntry {
            binding: b,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        };
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("textures"),
            entries: &[tex(0), smp(1), tex(2), smp(3)],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("fixed function"),
            bind_group_layouts: &[Some(&uniform_layout), Some(&texture_layout)],
            immediate_size: 0,
        });
        let white = device
            .create_texture_with_data(
                &queue,
                &wgpu::TextureDescriptor {
                    label: Some("white"),
                    size: wgpu::Extent3d {
                        width: 1,
                        height: 1,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                },
                wgpu::util::TextureDataOrder::LayerMajor,
                &[255, 255, 255, 255],
            )
            .create_view(&wgpu::TextureViewDescriptor::default());
        let blit_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("blit"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });
        Renderer {
            msaa,
            blit_layout,
            blit: None,
            readback: None,
            resident: None,
            device,
            queue,
            shader,
            layout,
            uniform_layout,
            texture_layout,
            pipelines: HashMap::new(),
            textures: HashMap::new(),
            samplers: HashMap::new(),
            white,
            target: None,
            frame: 0,
        }
    }

    fn pipeline(&mut self, key: PipelineKey) -> &wgpu::RenderPipeline {
        let (device, shader, layout) = (&self.device, &self.shader, &self.layout);
        self.pipelines.entry(key).or_insert_with(|| {
            let attrs = wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x2, 4 => Float32x2, 5 => Float32, 6 => Float32, 7 => Float32x4];
            let blend = key.0.map(|(s, d)| wgpu::BlendState {
                color: wgpu::BlendComponent {
                    src_factor: blend_factor(s),
                    dst_factor: blend_factor(d),
                    operation: wgpu::BlendOperation::Add,
                },
                alpha: wgpu::BlendComponent {
                    src_factor: blend_factor(s),
                    dst_factor: blend_factor(d),
                    operation: wgpu::BlendOperation::Add,
                },
            });
            let (compare, write) = match key.1 {
                Some((f, w)) => (compare_fn(f), w),
                None => (wgpu::CompareFunction::Always, false),
            };
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("draw"),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module: shader,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<Vertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &attrs,
                    })],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: Some(write),
                    depth_compare: Some(compare),
                    stencil: wgpu::StencilState::default(),
                    bias: if key.3 {
                        wgpu::DepthBiasState {
                            constant: 16,
                            slope_scale: 2.0,
                            clamp: 0.0,
                        }
                    } else {
                        wgpu::DepthBiasState::default()
                    },
                }),
                multisample: wgpu::MultisampleState {
                    count: key.2,
                    ..Default::default()
                },
                fragment: Some(wgpu::FragmentState {
                    module: shader,
                    entry_point: Some("fs"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        })
    }

    /// The pipeline that copies the target's pixels into its multisampled colour.
    fn blit_pipeline(&mut self) -> &wgpu::RenderPipeline {
        let (device, blit_layout) = (&self.device, &self.blit_layout);
        self.blit.get_or_insert_with(|| {
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("blit"),
                source: wgpu::ShaderSource::Wgsl(BLIT.into()),
            });
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("blit"),
                bind_group_layouts: &[Some(blit_layout)],
                immediate_size: 0,
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("blit"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState {
                    count: MSAA,
                    ..Default::default()
                },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some("fs"),
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
    }

    fn texture_view(&mut self, t: &Texture) -> wgpu::TextureView {
        let frame = self.frame;
        if let Some(v) = self.textures.get_mut(&t.key) {
            v.1 = frame;
            return v.0.clone();
        }
        let data: &[u8] = bytemuck::cast_slice(t.rgba.as_slice());
        let view = self
            .device
            .create_texture_with_data(
                &self.queue,
                &wgpu::TextureDescriptor {
                    label: Some("game texture"),
                    size: wgpu::Extent3d {
                        width: t.width.max(1),
                        height: t.height.max(1),
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                },
                wgpu::util::TextureDataOrder::LayerMajor,
                data,
            )
            .create_view(&wgpu::TextureViewDescriptor::default());
        if self.textures.len() > 2048 {
            // Drop what was not used for a while.
            self.textures.retain(|_, v| frame.saturating_sub(v.1) < 600);
        }
        self.textures.insert(t.key, (view.clone(), frame));
        view
    }

    fn sampler(&mut self, bilinear: bool, clamp: (bool, bool)) -> wgpu::Sampler {
        let device = &self.device;
        self.samplers
            .entry((bilinear, clamp.0, clamp.1))
            .or_insert_with(|| {
                let mode = |c: bool| {
                    if c {
                        wgpu::AddressMode::ClampToEdge
                    } else {
                        wgpu::AddressMode::Repeat
                    }
                };
                let filter = if bilinear {
                    wgpu::FilterMode::Linear
                } else {
                    wgpu::FilterMode::Nearest
                };
                device.create_sampler(&wgpu::SamplerDescriptor {
                    label: Some("game sampler"),
                    address_mode_u: mode(clamp.0),
                    address_mode_v: mode(clamp.1),
                    mag_filter: filter,
                    min_filter: filter,
                    ..Default::default()
                })
            })
            .clone()
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
        self.render_view(width, height, (width, height), rgba, commands)
    }

    /// [`render`](Self::render) into a `width` × `height` target that shows a screen of `view`
    /// pixels (the draws' coordinates): larger than it, the scene is drawn at that resolution.
    pub fn render_view(
        &mut self,
        width: u32,
        height: u32,
        view: (u32, u32),
        rgba: &[u8],
        commands: &[Command],
    ) -> Vec<u8> {
        let mut out = self
            .render_inner(width, height, view, Some(rgba), commands, true)
            .unwrap_or_else(|| rgba.to_vec());
        let started = std::time::Instant::now();
        fill_edges(&mut out, (width, height), view);
        super::LAST_RENDER.lock().expect("render times")[3] = started.elapsed().as_nanos() as u64;
        out
    }

    /// Draw `commands` into the target (`width` × `height`, showing a screen of `view`): over
    /// `background` uploaded first, or over what the target holds; the result read back when
    /// `read` asks (else it stays on the GPU: `None`).
    pub(crate) fn render_inner(
        &mut self,
        width: u32,
        height: u32,
        view: (u32, u32),
        background: Option<&[u8]>,
        commands: &[Command],
        read: bool,
    ) -> Option<Vec<u8>> {
        let started = std::time::Instant::now();
        self.frame += 1;
        let samples = if self.msaa && crate::render::antialias() {
            MSAA
        } else {
            1
        };
        if self
            .target
            .as_ref()
            .is_none_or(|t| (t.width, t.height, t.samples) != (width, height, samples))
        {
            let mk = |label, format, usage, samples| {
                self.device.create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width,
                        height,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: samples,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage,
                    view_formats: &[],
                })
            };
            let color = mk(
                "target",
                wgpu::TextureFormat::Rgba8Unorm,
                wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_DST
                    | wgpu::TextureUsages::COPY_SRC,
                1,
            );
            let depth = mk(
                "depth",
                wgpu::TextureFormat::Depth32Float,
                wgpu::TextureUsages::RENDER_ATTACHMENT,
                samples,
            );
            let msaa = (samples > 1).then(|| {
                mk(
                    "multisampled target",
                    wgpu::TextureFormat::Rgba8Unorm,
                    wgpu::TextureUsages::RENDER_ATTACHMENT,
                    samples,
                )
            });
            self.target = Some(Target {
                width,
                height,
                samples,
                color,
                depth,
                msaa,
            });
        }
        let (color, depth, msaa) = {
            let t = self.target.as_ref().expect("target");
            (t.color.clone(), t.depth.clone(), t.msaa.clone())
        };
        if let Some(rgba) = background {
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &color,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                rgba,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(4 * width),
                    rows_per_image: Some(height),
                },
                wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
            );
        }
        let color_view = color.create_view(&wgpu::TextureViewDescriptor::default());
        let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());
        let msaa_view = msaa
            .as_ref()
            .map(|t| t.create_view(&wgpu::TextureViewDescriptor::default()));

        // Uniforms of every draw in one buffer, 256 bytes apart.
        let draws: Vec<&Draw> = commands
            .iter()
            .filter_map(|c| {
                if let Command::Draw(d) = c {
                    Some(d)
                } else {
                    None
                }
            })
            .collect();
        let mut ubytes = vec![0u8; 256 * draws.len().max(1)];
        let mut vbytes: Vec<u8> = Vec::new();
        let mut ranges = Vec::new();
        for (k, d) in draws.iter().enumerate() {
            let mut u = Uniforms {
                stage: [[0; 4]; 4],
                factor: d.factor,
                fog_color: d.fog.map_or([0.0; 4], |f| f.0),
                view: [view.0 as f32, view.1 as f32, 0.0, 0.0],
                misc: [0; 4],
                snap: [
                    width as f32 / view.0 as f32,
                    height as f32 / view.1 as f32,
                    if d.pixel_exact && (width, height) != view {
                        1.0
                    } else {
                        0.0
                    },
                    if d.pixel_exact && d.enlarged > 1 {
                        d.enlarged as f32
                    } else {
                        0.0
                    },
                ],
                _pad: [0; 4],
            };
            for (s, st) in d.stages.iter().take(2).enumerate() {
                u.stage[2 * s] = [st.cop, st.ca1, st.ca2, st.aop];
                u.stage[2 * s + 1] = [st.aa1, st.aa2, st.coords, 1];
            }
            if let Some((f, r)) = d.alpha_test {
                u.misc[0] = f;
                u.misc[1] = r.to_bits();
            }
            if let Some((_, table)) = d.fog {
                match table {
                    Some((s, e)) => {
                        u.misc[2] = 2;
                        u.view[2] = s;
                        u.view[3] = e;
                    }
                    None => u.misc[2] = 1,
                }
            }
            u.misc[3] = u32::from(d.specular_add)
                | if d.keyed { 2 } else { 0 }
                | if d.textures[0].is_some() { 4 } else { 0 }
                | if d.textures[1].is_some() { 8 } else { 0 };
            ubytes[256 * k..256 * k + std::mem::size_of::<Uniforms>()]
                .copy_from_slice(bytemuck::bytes_of(&u));
            let start = (vbytes.len() / std::mem::size_of::<Vertex>()) as u32;
            vbytes.extend_from_slice(bytemuck::cast_slice(&d.vertices));
            let end = start + d.vertices.len() as u32;
            // Opaque 3D gets rims that close the gaps between meshes (`super::rims`).
            let rim = if d.blend.is_none() && d.depth.is_some_and(|(_, write)| write) {
                super::rims(&d.vertices, 0.5)
            } else {
                Vec::new()
            };
            vbytes.extend_from_slice(bytemuck::cast_slice(&rim));
            ranges.push((start..end, end..end + rim.len() as u32));
        }
        if vbytes.is_empty() {
            vbytes.resize(std::mem::size_of::<Vertex>(), 0);
        }
        let ubuf = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("uniforms"),
                contents: &ubytes,
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let vbuf = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("vertices"),
                contents: &vbytes,
                usage: wgpu::BufferUsages::VERTEX,
            });
        let ubind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("uniforms"),
            layout: &self.uniform_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &ubuf,
                    offset: 0,
                    size: std::num::NonZeroU64::new(std::mem::size_of::<Uniforms>() as u64),
                }),
            }],
        });
        // Per draw: its pipeline and texture bind group (made before the pass borrows them).
        let mut per_draw = Vec::new();
        for d in &draws {
            let key: PipelineKey = (d.blend, d.depth, samples, false);
            self.pipeline(key);
            self.pipeline((d.blend, d.depth, samples, true));
            let mut views = Vec::new();
            let mut samplers = Vec::new();
            for t in &d.textures {
                match t {
                    Some(t) => {
                        views.push(self.texture_view(t));
                        samplers.push(self.sampler(t.bilinear, t.clamp));
                    }
                    None => {
                        views.push(self.white.clone());
                        samplers.push(self.sampler(false, (false, false)));
                    }
                }
            }
            let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("textures"),
                layout: &self.texture_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&views[0]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&samplers[0]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(&views[1]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::Sampler(&samplers[1]),
                    },
                ],
            });
            per_draw.push((key, bind));
        }
        let mut enc = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame"),
            });
        // Multisampled: the target's pixels into every sample first; each pass below resolves
        // into the target again.
        if let Some(view) = &msaa_view {
            let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("blit"),
                layout: &self.blit_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&color_view),
                }],
            });
            let pipeline = self.blit_pipeline().clone();
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("blit"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bind, &[]);
            pass.draw(0..3, 0..1);
        }
        // A render pass per run of draws between depth clears.
        let mut k = 0;
        let mut i = 0;
        let mut depth_load = wgpu::LoadOp::Load;
        while i <= commands.len() {
            let run_end = commands[i..]
                .iter()
                .position(|c| matches!(c, Command::ClearDepth(_)))
                .map_or(commands.len(), |p| i + p);
            let n = commands[i..run_end].len();
            if n > 0 || matches!(depth_load, wgpu::LoadOp::Clear(_)) {
                let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("draws"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: msaa_view.as_ref().unwrap_or(&color_view),
                        depth_slice: None,
                        resolve_target: msaa_view.as_ref().map(|_| &color_view),
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &depth_view,
                        depth_ops: Some(wgpu::Operations {
                            load: depth_load,
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                pass.set_vertex_buffer(0, vbuf.slice(..));
                for _ in 0..n {
                    let (key, bind) = &per_draw[k];
                    pass.set_pipeline(&self.pipelines[key]);
                    pass.set_bind_group(0, &ubind, &[256 * k as u32]);
                    pass.set_bind_group(1, bind, &[]);
                    let (own, rim) = &ranges[k];
                    if !own.is_empty() {
                        pass.draw(own.clone(), 0..1);
                    }
                    if !rim.is_empty() {
                        pass.set_pipeline(&self.pipelines[&(key.0, key.1, key.2, true)]);
                        pass.draw(rim.clone(), 0..1);
                    }
                    k += 1;
                }
            }
            match commands.get(run_end) {
                Some(Command::ClearDepth(z)) => depth_load = wgpu::LoadOp::Clear(*z),
                _ => break,
            }
            i = run_end + 1;
        }
        if !read {
            let prepared = started.elapsed();
            crate::gpu::gate::submit(|| self.queue.submit([enc.finish()]));
            *super::LAST_RENDER.lock().expect("render times") =
                [prepared.as_nanos() as u64, 0, 0, 0];
            return None;
        }
        // Read the result back, rows padded to 256 bytes.
        let row = (4 * width).div_ceil(256) * 256;
        let size = u64::from(row * height);
        if self.readback.as_ref().is_none_or(|(s, _)| *s != size) {
            let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("readback"),
                size,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            self.readback = Some((size, buffer));
        }
        let out = self.readback.as_ref().expect("readback").1.clone();
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &color,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &out,
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
        let prepared = started.elapsed();
        crate::gpu::gate::submit(|| self.queue.submit([enc.finish()]));
        let slice = out.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        let _ = self.device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        });
        let waited = started.elapsed();
        let Ok(data) = slice.get_mapped_range() else {
            return None;
        };
        let result = if row == 4 * width {
            data[..(row * height) as usize].to_vec()
        } else {
            let mut result = Vec::with_capacity((4 * width * height) as usize);
            for y in 0..height as usize {
                result.extend_from_slice(
                    &data[y * row as usize..y * row as usize + 4 * width as usize],
                );
            }
            result
        };
        drop(data);
        out.unmap();
        let unpacked = started.elapsed();
        *super::LAST_RENDER.lock().expect("render times") = [
            prepared.as_nanos() as u64,
            (waited - prepared).as_nanos() as u64,
            (unpacked - waited).as_nanos() as u64,
            0,
        ];
        Some(result)
    }
}

/// A screen drawn larger: the screen's pixel centres are at whole coordinates (Direct3D), so
/// what it draws from 0 on begins half a screen pixel into the target, and the target's first
/// columns and rows (the left and upper half of the screen's first pixels) stay as they were.
/// They show the column and row after them, which belong to the same screen pixels.
fn fill_edges(rgba: &mut [u8], (width, height): (u32, u32), view: (u32, u32)) {
    let (w, h) = (width as usize, height as usize);
    // The target's pixels whose centre lies before the first screen pixel's centre.
    let strip = |size: u32, view: u32| {
        let s = size as f32 / view as f32;
        (((s - 1.0) / 2.0).ceil().max(0.0) as usize).min(size as usize - 1)
    };
    let (sx, sy) = (strip(width, view.0), strip(height, view.1));
    for y in 0..h {
        let row = &mut rgba[4 * y * w..4 * (y + 1) * w];
        let from: [u8; 4] = row[4 * sx..4 * sx + 4].try_into().expect("pixel");
        for x in 0..sx {
            row[4 * x..4 * x + 4].copy_from_slice(&from);
        }
    }
    if sy > 0 {
        let from = rgba[4 * sy * w..4 * (sy + 1) * w].to_vec();
        for y in 0..sy {
            rgba[4 * y * w..4 * (y + 1) * w].copy_from_slice(&from);
        }
    }
}
