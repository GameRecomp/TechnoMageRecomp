// SPDX-License-Identifier: MIT
//! The GPU renderer: Direct3D 7's fixed-function pixel pipeline on wgpu, offscreen.
//!
//! The HLE layer does the vertex work (transform and lighting) and hands a batch of screen-space
//! triangles per render target, each draw with its textures and state. This renders the batch on
//! top of the target's current pixels (uploaded from guest memory) and returns the result as
//! RGBA8, which the HLE layer writes back into guest memory: the guest's surfaces stay the one
//! truth, so the game's own 2D work on them (`Lock`, `Blt`) mixes with the 3D as on Windows.
//! The depth buffer lives on the GPU across batches of a frame.
//!
//! One shader serves every draw (the stages, alpha test and fog are uniforms); a pipeline per
//! blend and depth setting. Without a usable adapter [`Renderer::new`] returns `None` and the
//! HLE layer rasterizes in software.

use std::collections::HashMap;
use std::sync::Arc;
use wgpu::util::DeviceExt;

/// A vertex in screen space (Direct3D's pre-transformed form).
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    /// x, y (pixels), z (0..1), rhw.
    pub pos: [f32; 4],
    pub diffuse: [f32; 4],
    pub specular: [f32; 4],
    pub uv0: [f32; 2],
    pub uv1: [f32; 2],
    /// Distance from the eye (table fog).
    pub eye: f32,
    pub _pad: [f32; 3],
}

/// A texture's pixels (RGBA8), identified by `key` for the GPU-side cache.
#[derive(Debug, Clone)]
pub struct Texture {
    pub key: (u32, u32, bool),
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<Vec<[u8; 4]>>,
    pub bilinear: bool,
    pub clamp: (bool, bool),
}

/// A texture stage: colour op and arguments, alpha op and arguments, coordinate set.
#[derive(Debug, Clone, Copy, Default)]
pub struct Stage {
    pub cop: u32,
    pub ca1: u32,
    pub ca2: u32,
    pub aop: u32,
    pub aa1: u32,
    pub aa2: u32,
    pub coords: u32,
}

/// One draw: triangles (three vertices each) and the state they are drawn in.
#[derive(Debug, Clone)]
pub struct Draw {
    pub vertices: Vec<Vertex>,
    pub textures: [Option<Texture>; 2],
    pub stages: Vec<Stage>,
    pub factor: [f32; 4],
    pub specular_add: bool,
    /// Colour-keyed texels are discarded.
    pub keyed: bool,
    /// Alpha test: (function, reference 0..1), `None` when off.
    pub alpha_test: Option<(u32, f32)>,
    /// Blending: (source, destination) D3DBLEND factors, `None` when off.
    pub blend: Option<(u32, u32)>,
    /// Depth: (compare function, write), `None` when the test is off.
    pub depth: Option<(u32, bool)>,
    /// Fog: colour, and for linear table fog (start, end); vertex fog otherwise.
    pub fog: Option<Fog>,
}

/// Fog: the colour and, for linear table fog, (start, end); vertex fog without.
pub type Fog = ([f32; 4], Option<(f32, f32)>);

/// What a batch holds, in order.
#[derive(Debug, Clone)]
pub enum Command {
    Draw(Draw),
    /// Clear the depth buffer to a value (Direct3D's `Clear` with the z-buffer flag).
    ClearDepth(f32),
}

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
    _pad: [u32; 8],
}

const SHADER: &str = r#"
struct U {
    stage: array<vec4<u32>, 4>,
    factor: vec4<f32>,
    fog_color: vec4<f32>,
    view: vec4<f32>,
    misc: vec4<u32>,
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
};
struct Out {
    @builtin(position) clip: vec4<f32>,
    @location(0) diffuse: vec4<f32>,
    @location(1) specular: vec4<f32>,
    @location(2) uv0: vec2<f32>,
    @location(3) uv1: vec2<f32>,
    @location(4) eye: f32,
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
    // Both textures are sampled in uniform control flow, then used as the stages say.
    let c0 = textureSample(t0, s0, select(i.uv0, i.uv1, u.stage[1].z == 1u));
    let c1 = textureSample(t1, s1, select(i.uv0, i.uv1, u.stage[3].z == 1u));
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

type PipelineKey = (Option<(u32, u32)>, Option<(u32, bool)>);

/// The renderer: a wgpu device of its own and the caches.
pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    shader: wgpu::ShaderModule,
    layout: wgpu::PipelineLayout,
    uniform_layout: wgpu::BindGroupLayout,
    texture_layout: wgpu::BindGroupLayout,
    pipelines: HashMap<PipelineKey, wgpu::RenderPipeline>,
    textures: HashMap<(u32, u32, bool), (wgpu::TextureView, u64)>,
    samplers: HashMap<(bool, bool, bool), wgpu::Sampler>,
    white: wgpu::TextureView,
    target: Option<(u32, u32, wgpu::Texture, wgpu::Texture)>,
    frame: u64,
}

impl Renderer {
    /// A renderer on the system's GPU, or `None` without one (`RC_RENDERER=soft` asks for none).
    pub fn new() -> Option<Renderer> {
        if std::env::var("RC_RENDERER").is_ok_and(|v| v == "soft") {
            return None;
        }
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
            ..Default::default()
        }))
        .ok()?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("technomage renderer"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::downlevel_defaults(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .ok()?;
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
        Some(Renderer {
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
        })
    }

    fn pipeline(&mut self, key: PipelineKey) -> &wgpu::RenderPipeline {
        let (device, shader, layout) = (&self.device, &self.shader, &self.layout);
        self.pipelines.entry(key).or_insert_with(|| {
            let attrs = wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x2, 4 => Float32x2, 5 => Float32];
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
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState::default(),
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
        self.frame += 1;
        if self
            .target
            .as_ref()
            .is_none_or(|t| (t.0, t.1) != (width, height))
        {
            let mk = |format, usage| {
                self.device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("target"),
                    size: wgpu::Extent3d {
                        width,
                        height,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage,
                    view_formats: &[],
                })
            };
            let color = mk(
                wgpu::TextureFormat::Rgba8Unorm,
                wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::COPY_DST
                    | wgpu::TextureUsages::COPY_SRC,
            );
            let depth = mk(
                wgpu::TextureFormat::Depth32Float,
                wgpu::TextureUsages::RENDER_ATTACHMENT,
            );
            self.target = Some((width, height, color, depth));
        }
        let (color, depth) = {
            let t = self.target.as_ref().expect("target");
            (t.2.clone(), t.3.clone())
        };
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
        let color_view = color.create_view(&wgpu::TextureViewDescriptor::default());
        let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());

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
                view: [width as f32, height as f32, 0.0, 0.0],
                misc: [0; 4],
                _pad: [0; 8],
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
            ranges.push(start..start + d.vertices.len() as u32);
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
            let key: PipelineKey = (d.blend, d.depth);
            self.pipeline(key);
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
                        view: &color_view,
                        depth_slice: None,
                        resolve_target: None,
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
                    if !ranges[k].is_empty() {
                        pass.draw(ranges[k].clone(), 0..1);
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
        // Read the result back, rows padded to 256 bytes.
        let row = (4 * width).div_ceil(256) * 256;
        let out = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: u64::from(row * height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
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
        self.queue.submit([enc.finish()]);
        let slice = out.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        let _ = self.device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        });
        let Ok(data) = slice.get_mapped_range() else {
            return rgba.to_vec();
        };
        let mut result = Vec::with_capacity((4 * width * height) as usize);
        for y in 0..height as usize {
            result
                .extend_from_slice(&data[y * row as usize..y * row as usize + 4 * width as usize]);
        }
        drop(data);
        out.unmap();
        result
    }
}
