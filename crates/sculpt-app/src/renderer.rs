//! wgpu renderer: gradient background, ground grid, one pass per object,
//! optional wireframe overlay, optional multisampling.
//!
//! Per-object data lives in one uniform buffer addressed with dynamic offsets,
//! so drawing a scene costs one bind group rebind per object and no buffer
//! writes inside the pass.

use crate::gpu_vertex::{self, COLD_BYTES, COLD_WORDS, HOT_BYTES, HOT_WORDS};
use crate::matcap;
use glam::{Mat4, Vec3};
use sculpt_core::Scene;
use wgpu::util::DeviceExt;

pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const MATCAP_SIZE: u32 = 256;
/// Uniform slot stride; 256 is the worst-case alignment across backends.
const OBJECT_STRIDE: u64 = 256;
const MAX_OBJECTS: u64 = 64;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Shading {
    Matcap,
    Pbr,
    Unlit,
    Normals,
    Cavity,
    Clay,
}

impl Shading {
    pub const ALL: [Shading; 6] = [
        Shading::Matcap,
        Shading::Pbr,
        Shading::Unlit,
        Shading::Normals,
        Shading::Cavity,
        Shading::Clay,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Shading::Matcap => "Matcap",
            Shading::Pbr => "Lit",
            Shading::Unlit => "Unlit",
            Shading::Normals => "Normals",
            Shading::Cavity => "Cavity",
            Shading::Clay => "Clay",
        }
    }

    fn code(self) -> f32 {
        match self {
            Shading::Matcap => 0.0,
            Shading::Pbr => 1.0,
            Shading::Normals => 2.0,
            Shading::Cavity => 3.0,
            Shading::Clay => 4.0,
            Shading::Unlit => 5.0,
        }
    }

    /// Unlit shows the painted colour and nothing else, so the vertex colour
    /// switch has no say while it is on.
    pub fn forces_vertex_color(self) -> bool {
        self == Shading::Unlit
    }
}

/// Everything the renderer needs to know about how to draw a frame.
#[derive(Clone, Copy)]
pub struct FrameSettings {
    pub shading: Shading,
    pub flat: bool,
    pub wireframe: bool,
    pub show_mask: bool,
    pub vertex_color: bool,
    pub opacity: f32,
    pub grid: bool,
    pub grid_spacing: f32,
    pub cavity_strength: f32,
    /// The tone curve, and what it is fed.
    ///
    /// Only the lit view goes through it. A matcap is an image somebody already
    /// graded, the normals view is data rather than light, and the unlit view
    /// exists precisely to show a painted colour untouched. Putting a curve on
    /// any of those would be changing an answer, not shaping a picture.
    pub tone: bool,
    /// Stops of exposure, so one step is a doubling.
    pub exposure: f32,
    pub contrast: f32,
    pub saturation: f32,
    /// Occlusion in the creases. See `shaders/occlusion.wgsl`.
    pub occlusion: bool,
    /// How far a crease reaches to shade itself, in world units.
    pub occlusion_radius: f32,
    pub occlusion_strength: f32,
    pub background_top: [f32; 3],
    pub background_bottom: [f32; 3],
    /// Send only the vertices a stroke touched and let a compute pass scatter
    /// them, instead of re-sending the whole buffer every frame.
    pub gpu_scatter: bool,
}

impl Default for FrameSettings {
    fn default() -> Self {
        Self {
            shading: Shading::Matcap,
            flat: false,
            wireframe: false,
            show_mask: true,
            vertex_color: true,
            opacity: 1.0,
            grid: true,
            grid_spacing: 0.25,
            cavity_strength: 6.0,
            tone: true,
            exposure: 0.0,
            contrast: 1.0,
            saturation: 1.0,
            occlusion: true,
            occlusion_radius: 0.12,
            occlusion_strength: 1.0,
            // Held as sRGB so the colour pickers show what the viewport shows.
            background_top: [0.16, 0.17, 0.19],
            background_bottom: [0.075, 0.08, 0.09],
            gpu_scatter: true,
        }
    }
}

/// The render target is sRGB, so a colour picked in the interface has to be
/// linearised before the shader writes it, otherwise every background reads
/// two stops brighter than the swatch beside it.
fn to_linear(c: [f32; 3]) -> [f32; 4] {
    let f = |v: f32| {
        let v = v.clamp(0.0, 1.0);
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    [f(c[0]), f(c[1]), f(c[2]), 1.0]
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Globals {
    view_proj: [[f32; 4]; 4],
    view: [[f32; 4]; 4],
    inv_view_proj: [[f32; 4]; 4],
    eye: [f32; 4],
    params: [f32; 4],
    extra: [f32; 4],
    bg_top: [f32; 4],
    bg_bottom: [f32; 4],
    /// x = exposure, y = contrast, z = saturation, w = 1 when the curve is on.
    tone: [f32; 4],
    /// x = world radius, y = strength, z = 1 when on, w unused.
    occlusion: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct ObjectData {
    model: [[f32; 4]; 4],
    normal_mat: [[f32; 4]; 4],
    tint: [f32; 4],
    _pad: [f32; 12],
}

/// Vertex and index buffers are also storage buffers so the scatter passes can
/// write them.
const VERTEX_USAGE: wgpu::BufferUsages =
    wgpu::BufferUsages::VERTEX.union(wgpu::BufferUsages::STORAGE);
const INDEX_USAGE: wgpu::BufferUsages =
    wgpu::BufferUsages::INDEX.union(wgpu::BufferUsages::STORAGE);

/// Indices per triangle, which is the third scatter channel's stride.
const FACE_WORDS: usize = 3;

/// GPU mirror of one mesh.
///
/// The vertices arrive in two streams rather than one: see `gpu_vertex`. The
/// split halves what the card reads to draw a triangle, and it means a paint
/// stroke never touches the positions nor a sculpt stroke the colours.
struct MeshBuffers {
    hot: wgpu::Buffer,
    hot_cap: u64,
    cold: wgpu::Buffer,
    cold_cap: u64,
    ibuf: wgpu::Buffer,
    icap: u64,
    index_count: u32,
    /// Set when a buffer was reallocated, which invalidates any bind group
    /// pointing at the old one.
    generation: u32,
}

impl MeshBuffers {
    fn new(device: &wgpu::Device) -> Self {
        Self {
            hot: empty_buffer(device, "vertices, hot", VERTEX_USAGE),
            hot_cap: 1024,
            cold: empty_buffer(device, "vertices, cold", VERTEX_USAGE),
            cold_cap: 1024,
            ibuf: empty_buffer(device, "indices", INDEX_USAGE),
            icap: 1024,
            index_count: 0,
            generation: 0,
        }
    }

    /// Uploads the whole mesh. Buffers grow with 50% headroom so a dyntopo
    /// stroke does not reallocate on every step.
    fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, mesh: &sculpt_core::Mesh) -> u64 {
        // Packing several million vertices is arithmetic on independent
        // elements, which is to say it runs on every core.
        let (hot, cold) = gpu_vertex::pack_all(mesh);
        let hot_data: &[u8] = bytemuck::cast_slice(&hot);
        let cold_data: &[u8] = bytemuck::cast_slice(&cold);

        if hot_data.len() as u64 > self.hot_cap {
            self.hot_cap = (hot_data.len() as u64 * 3 / 2).next_power_of_two();
            self.hot = sized_buffer(device, "vertices, hot", self.hot_cap, VERTEX_USAGE);
            self.generation = self.generation.wrapping_add(1);
        }
        if cold_data.len() as u64 > self.cold_cap {
            self.cold_cap = (cold_data.len() as u64 * 3 / 2).next_power_of_two();
            self.cold = sized_buffer(device, "vertices, cold", self.cold_cap, VERTEX_USAGE);
            self.generation = self.generation.wrapping_add(1);
        }
        if !hot_data.is_empty() {
            queue.write_buffer(&self.hot, 0, hot_data);
            queue.write_buffer(&self.cold, 0, cold_data);
        }

        let idata: &[u8] = bytemuck::cast_slice(&mesh.faces);
        if idata.len() as u64 > self.icap {
            self.icap = (idata.len() as u64 * 3 / 2).next_power_of_two();
            self.ibuf = sized_buffer(device, "indices", self.icap, INDEX_USAGE);
        }
        if !idata.is_empty() {
            queue.write_buffer(&self.ibuf, 0, idata);
        }
        self.index_count = (mesh.faces.len() * 3) as u32;
        (hot_data.len() + cold_data.len() + idata.len()) as u64
    }
}

/// One sparse upload channel: a list of destination slots and the data to put
/// in them, pushed to the GPU and scattered into place by a compute pass.
struct ScatterPass {
    pipeline: wgpu::ComputePipeline,
    slot_buf: wgpu::Buffer,
    data_buf: wgpu::Buffer,
    params_buf: wgpu::Buffer,
    /// Slots the buffers can hold.
    capacity: u32,
    /// Words per slot.
    stride: usize,
    /// Cached bind group, keyed by the object and buffer generation it was
    /// built against.
    bind: Option<(usize, u32, wgpu::BindGroup)>,
}

impl ScatterPass {
    fn new(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        source: wgpu::ShaderSource<'_>,
        label: &str,
        stride: usize,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(label),
            source,
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("scatter pipeline layout"),
            bind_group_layouts: &[Some(layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some(label),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let capacity = 4096;
        Self {
            pipeline,
            slot_buf: sized_buffer(device, "scatter slots", capacity as u64 * 4, wgpu::BufferUsages::STORAGE),
            data_buf: sized_buffer(
                device,
                "scatter data",
                capacity as u64 * stride as u64 * 4,
                wgpu::BufferUsages::STORAGE,
            ),
            params_buf: sized_buffer(device, "scatter params", 16, wgpu::BufferUsages::UNIFORM),
            capacity,
            stride,
            bind: None,
        }
    }

    fn reserve(&mut self, device: &wgpu::Device, count: u32) {
        if count <= self.capacity {
            return;
        }
        self.capacity = count.next_power_of_two();
        self.slot_buf = sized_buffer(
            device,
            "scatter slots",
            self.capacity as u64 * 4,
            wgpu::BufferUsages::STORAGE,
        );
        self.data_buf = sized_buffer(
            device,
            "scatter data",
            self.capacity as u64 * self.stride as u64 * 4,
            wgpu::BufferUsages::STORAGE,
        );
        self.bind = None;
    }

    /// Queues the scatter. `slots` and `data` must already be staged, and
    /// `destination` is the buffer being written into.
    fn dispatch(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        layout: &wgpu::BindGroupLayout,
        destination: &wgpu::Buffer,
        key: (usize, u32),
        slots: &[u32],
        data: &[u8],
    ) {
        let count = slots.len() as u32;
        if count == 0 {
            return;
        }
        self.reserve(device, count);
        queue.write_buffer(&self.slot_buf, 0, bytemuck::cast_slice(slots));
        queue.write_buffer(&self.data_buf, 0, data);
        queue.write_buffer(
            &self.params_buf,
            0,
            bytemuck::cast_slice(&[count, self.stride as u32, 0, 0]),
        );

        let stale = !matches!(&self.bind, Some((o, g, _)) if (*o, *g) == key);
        if stale {
            let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("scatter bind"),
                layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: destination.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: self.slot_buf.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 2, resource: self.data_buf.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 3, resource: self.params_buf.as_entire_binding() },
                ],
            });
            self.bind = Some((key.0, key.1, bind));
        }

        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("scatter"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.pipeline);
        if let Some((_, _, bind)) = &self.bind {
            pass.set_bind_group(0, bind, &[]);
        }
        pass.dispatch_workgroups(count.div_ceil(64), 1, 1);
    }
}

/// The three sparse channels, sharing one bind group layout and one shader.
struct Scatter {
    layout: wgpu::BindGroupLayout,
    hot: ScatterPass,
    cold: ScatterPass,
    faces: ScatterPass,
    hot_data: Vec<u32>,
    cold_data: Vec<u32>,
    face_data: Vec<u32>,
}

impl Scatter {
    fn new(device: &wgpu::Device) -> Self {
        let storage = |read_only: bool| wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        };
        let entry = |binding: u32, ty: wgpu::BindingType| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty,
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("scatter layout"),
            entries: &[
                entry(0, storage(false)),
                entry(1, storage(true)),
                entry(2, storage(true)),
                entry(
                    3,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                ),
            ],
        });
        // One shader for all three: it copies words and takes the stride from
        // its parameters, so a triangle, a position with a normal and a colour
        // with its material are the same job at three different sizes.
        let source = || wgpu::ShaderSource::Wgsl(include_str!("shaders/scatter.wgsl").into());
        let hot = ScatterPass::new(device, &layout, source(), "scatter, hot", HOT_WORDS);
        let cold = ScatterPass::new(device, &layout, source(), "scatter, cold", COLD_WORDS);
        let faces = ScatterPass::new(device, &layout, source(), "scatter, indices", FACE_WORDS);
        Self {
            layout,
            hot,
            cold,
            faces,
            hot_data: Vec::new(),
            cold_data: Vec::new(),
            face_data: Vec::new(),
        }
    }
}

pub struct Renderer {
    mesh_pipeline: wgpu::RenderPipeline,
    wire_pipeline: Option<wgpu::RenderPipeline>,
    background_pipeline: wgpu::RenderPipeline,
    grid_pipeline: wgpu::RenderPipeline,
    occlusion_pipeline: wgpu::RenderPipeline,
    occlusion_layout: wgpu::BindGroupLayout,
    occlusion_bind: wgpu::BindGroup,

    global_layout: wgpu::BindGroupLayout,
    global_bind: wgpu::BindGroup,
    global_buf: wgpu::Buffer,
    object_layout: wgpu::BindGroupLayout,
    object_bind: wgpu::BindGroup,
    object_buf: wgpu::Buffer,

    sampler: wgpu::Sampler,
    matcap_view: wgpu::TextureView,

    meshes: Vec<MeshBuffers>,
    scatter: Scatter,
    /// Bytes sent to the GPU by the last sync, for the statistics readout.
    pub last_upload_bytes: u64,
    depth_view: wgpu::TextureView,
    msaa_view: Option<wgpu::TextureView>,
    sample_count: u32,
    color_format: wgpu::TextureFormat,
    pub wire_supported: bool,
    width: u32,
    height: u32,
}

impl Renderer {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        color_format: wgpu::TextureFormat,
        width: u32,
        height: u32,
        wire_supported: bool,
        sample_count: u32,
    ) -> Self {
        let global_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("globals"),
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
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let object_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("object"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(
                        std::mem::size_of::<ObjectData>() as u64
                    ),
                },
                count: None,
            }],
        });

        let global_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: std::mem::size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let object_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("objects"),
            size: OBJECT_STRIDE * MAX_OBJECTS,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("matcap sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let matcap_view = upload_matcap(
            device,
            queue,
            &matcap::generate(matcap::Preset::Clay, MATCAP_SIZE),
            MATCAP_SIZE,
        );

        let global_bind =
            make_global_bind(device, &global_layout, &global_buf, &matcap_view, &sampler);
        let object_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("object bind"),
            layout: &object_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &object_buf,
                    offset: 0,
                    size: wgpu::BufferSize::new(std::mem::size_of::<ObjectData>() as u64),
                }),
            }],
        });

        let depth_view = make_depth(device, width, height, sample_count);
        let occl_layout = occlusion_layout(device, sample_count);
        let occl_bind = make_occlusion_bind(device, &occl_layout, &global_buf, &depth_view);

        let mut me = Self {
            // Placeholders replaced right below; building pipelines needs the
            // layouts that were just created.
            mesh_pipeline: dummy_pipeline(device, color_format),
            wire_pipeline: None,
            background_pipeline: dummy_pipeline(device, color_format),
            grid_pipeline: dummy_pipeline(device, color_format),
            occlusion_pipeline: dummy_pipeline(device, color_format),
            occlusion_layout: occl_layout,
            occlusion_bind: occl_bind,
            global_layout,
            global_bind,
            global_buf,
            object_layout,
            object_bind,
            object_buf,
            sampler,
            matcap_view,
            meshes: Vec::new(),
            scatter: Scatter::new(device),
            last_upload_bytes: 0,
            depth_view,
            msaa_view: make_msaa(device, color_format, width, height, sample_count),
            sample_count,
            color_format,
            wire_supported,
            width,
            height,
        };
        me.build_pipelines(device);
        me
    }

    fn build_pipelines(&mut self, device: &wgpu::Device) {
        let mesh_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("mesh"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/mesh.wgsl").into()),
        });
        let bg_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("background"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/background.wgsl").into()),
        });
        let grid_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("grid"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/grid.wgsl").into()),
        });

        let mesh_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mesh layout"),
            bind_group_layouts: &[Some(&self.global_layout), Some(&self.object_layout)],
            immediate_size: 0,
        });
        let screen_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("screen layout"),
            bind_group_layouts: &[Some(&self.global_layout)],
            immediate_size: 0,
        });

        // Sixteen bytes of what shading needs, eight of what only painting
        // writes. The formats have to match `gpu_vertex` word for word.
        let hot_attrs = [
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 0, shader_location: 0 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Unorm16x2, offset: 12, shader_location: 1 },
        ];
        let cold_attrs = [
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Unorm8x4, offset: 0, shader_location: 2 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Unorm8x4, offset: 4, shader_location: 3 },
        ];
        let hot_layout = wgpu::VertexBufferLayout {
            array_stride: HOT_BYTES as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &hot_attrs,
        };
        let cold_layout = wgpu::VertexBufferLayout {
            array_stride: COLD_BYTES as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &cold_attrs,
        };

        let samples = self.sample_count;
        let color_format = self.color_format;
        let blend_over = Some(wgpu::BlendState::ALPHA_BLENDING);

        let mesh_pipeline = |entry: &str, polygon, bias: i32, blend| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("mesh pipeline"),
                layout: Some(&mesh_layout),
                vertex: wgpu::VertexState {
                    module: &mesh_shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[Some(hot_layout.clone()), Some(cold_layout.clone())],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &mesh_shader,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: color_format,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    strip_index_format: None,
                    front_face: wgpu::FrontFace::Ccw,
                    // Sculpted surfaces get inverted locally all the time, and
                    // culling them would punch holes in the model.
                    cull_mode: None,
                    unclipped_depth: false,
                    polygon_mode: polygon,
                    conservative: false,
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::LessEqual),
                    stencil: Default::default(),
                    bias: wgpu::DepthBiasState { constant: bias, slope_scale: 0.0, clamp: 0.0 },
                }),
                multisample: wgpu::MultisampleState {
                    count: samples,
                    ..Default::default()
                },
                multiview_mask: None,
                cache: None,
            })
        };

        self.mesh_pipeline = mesh_pipeline("fs_main", wgpu::PolygonMode::Fill, 0, blend_over);
        self.wire_pipeline = self
            .wire_supported
            .then(|| mesh_pipeline("fs_wire", wgpu::PolygonMode::Line, -2, blend_over));

        let screen_pipeline = |shader: &wgpu::ShaderModule, label, depth_write, blend| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&screen_layout),
                vertex: wgpu::VertexState {
                    module: shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: shader,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: color_format,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(depth_write),
                    depth_compare: Some(if depth_write {
                        wgpu::CompareFunction::Always
                    } else {
                        wgpu::CompareFunction::LessEqual
                    }),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: wgpu::MultisampleState { count: samples, ..Default::default() },
                multiview_mask: None,
                cache: None,
            })
        };

        self.background_pipeline =
            screen_pipeline(&bg_shader, "background", false, Some(wgpu::BlendState::REPLACE));
        self.grid_pipeline = screen_pipeline(&grid_shader, "grid", false, blend_over);

        // The occlusion pass. Its source is written out for the sample count in
        // use, because a multisampled depth is a different type in the shading
        // language and there is no preprocessor here yet to say so.
        let occl_src = include_str!("shaders/occlusion.wgsl")
            .replace(
                "DEPTH_TEXTURE_TYPE",
                if samples > 1 { "texture_depth_multisampled_2d" } else { "texture_depth_2d" },
            )
            .replace("LOAD_DEPTH", "textureLoad(depth_tex, c, 0)");
        let occl_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("occlusion"),
            source: wgpu::ShaderSource::Wgsl(occl_src.into()),
        });
        let occl_pipe_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("occlusion pipeline layout"),
            bind_group_layouts: &[Some(&self.occlusion_layout)],
            immediate_size: 0,
        });
        self.occlusion_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("occlusion"),
            layout: Some(&occl_pipe_layout),
            vertex: wgpu::VertexState {
                module: &occl_shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &occl_shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: color_format,
                    // Multiply. What the pass writes is how much light gets
                    // through, so the frame keeps its colour and loses its
                    // brightness where the surface closes in on itself.
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::Dst,
                            dst_factor: wgpu::BlendFactor::Zero,
                            operation: wgpu::BlendOperation::Add,
                        },
                        alpha: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::Zero,
                            dst_factor: wgpu::BlendFactor::One,
                            operation: wgpu::BlendOperation::Add,
                        },
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            // It runs after the scene pass has ended and resolved, so there is
            // nothing attached to test against.
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
    }

    /// Rebuilds every pipeline for a new sample count.
    pub fn sample_count(&self) -> u32 {
        self.sample_count
    }

    pub fn set_sample_count(&mut self, device: &wgpu::Device, samples: u32) {
        if samples == self.sample_count {
            return;
        }
        self.sample_count = samples;
        self.depth_view = make_depth(device, self.width, self.height, samples);
        self.msaa_view = make_msaa(device, self.color_format, self.width, self.height, samples);
        // A multisampled depth is a different binding, so the layout, the bind
        // group and the shader all follow the sample count.
        self.occlusion_layout = occlusion_layout(device, samples);
        self.occlusion_bind = make_occlusion_bind(
            device,
            &self.occlusion_layout,
            &self.global_buf,
            &self.depth_view,
        );
        self.build_pipelines(device);
    }

    /// Replaces the matcap with a square RGBA8 image.
    ///
    /// Pixels rather than a preset: a generated lightcap, an edited one and a
    /// file loaded off disk are all the same thing by the time they get here.
    pub fn set_matcap(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pixels: &[u8],
        size: u32,
    ) {
        self.matcap_view = upload_matcap(device, queue, pixels, size);
        self.global_bind = make_global_bind(
            device,
            &self.global_layout,
            &self.global_buf,
            &self.matcap_view,
            &self.sampler,
        );
    }

    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        self.width = width;
        self.height = height;
        self.depth_view = make_depth(device, width, height, self.sample_count);
        self.msaa_view = make_msaa(device, self.color_format, width, height, self.sample_count);
        self.occlusion_bind = make_occlusion_bind(
            device,
            &self.occlusion_layout,
            &self.global_buf,
            &self.depth_view,
        );
    }

    pub fn depth_view(&self) -> &wgpu::TextureView {
        &self.depth_view
    }

    pub fn msaa_view(&self) -> Option<&wgpu::TextureView> {
        self.msaa_view.as_ref()
    }

    /// Uploads whatever changed. `dirty` selects the object whose geometry moved;
    /// `None` re-uploads everything, which is what a scene change needs.
    pub fn sync(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scene: &Scene,
        dirty: Option<usize>,
    ) {
        while self.meshes.len() < scene.objects.len() {
            self.meshes.push(MeshBuffers::new(device));
        }
        self.meshes.truncate(scene.objects.len().max(1));
        let mut bytes = 0;
        for (i, obj) in scene.objects.iter().enumerate() {
            if dirty.is_none_or(|d| d == i) {
                bytes += self.meshes[i].upload(device, queue, &obj.mesh);
            }
        }
        self.last_upload_bytes = bytes;
    }

    /// Sends only the vertices and triangles that changed, and scatters them
    /// into place with a pair of compute passes.
    ///
    /// Returns false when the change is too broad or the buffers would have to
    /// grow, in which case the caller should fall back to a whole upload.
    pub fn sync_sparse(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        scene: &Scene,
        object: usize,
        dirty_verts: &mut Vec<u32>,
        dirty_faces: &mut Vec<u32>,
    ) -> bool {
        let Some(obj) = scene.objects.get(object) else {
            return true;
        };
        while self.meshes.len() < scene.objects.len() {
            self.meshes.push(MeshBuffers::new(device));
        }
        let mesh = &obj.mesh;
        let (vcount, fcount) = (mesh.pos.len(), mesh.faces.len());
        if vcount == 0 {
            self.last_upload_bytes = 0;
            return true;
        }

        // A buffer that has to grow cannot be patched in place.
        let buffers = &self.meshes[object];
        let needed_i = (fcount * std::mem::size_of::<[u32; 3]>()) as u64;
        if (vcount * HOT_BYTES) as u64 > buffers.hot_cap
            || (vcount * COLD_BYTES) as u64 > buffers.cold_cap
            || needed_i > buffers.icap
        {
            return false;
        }

        prune(dirty_verts, vcount);
        prune(dirty_faces, fcount);
        // Past a quarter of the mesh the slot lists stop being a saving.
        if dirty_verts.len() * 4 >= vcount || dirty_faces.len() * 4 >= fcount.max(1) {
            return false;
        }

        let generation = self.meshes[object].generation;
        let key = (object, generation);
        let mut sent = 0u64;

        if !dirty_verts.is_empty() {
            let s = &mut self.scatter;
            s.hot_data.clear();
            s.cold_data.clear();
            s.hot_data.reserve(dirty_verts.len() * HOT_WORDS);
            s.cold_data.reserve(dirty_verts.len() * COLD_WORDS);
            for &v in dirty_verts.iter() {
                let i = v as usize;
                s.hot_data
                    .extend_from_slice(&gpu_vertex::hot_from(mesh.pos[i], mesh.nrm[i]));
                s.cold_data.extend_from_slice(&gpu_vertex::cold_from(
                    mesh.col(v),
                    mesh.mask(v),
                    mesh.rough(v),
                    mesh.metal(v),
                ));
            }
            let hot: &[u8] = bytemuck::cast_slice(&s.hot_data);
            let cold: &[u8] = bytemuck::cast_slice(&s.cold_data);
            sent += (dirty_verts.len() * 8 + hot.len() + cold.len()) as u64;
            let layout = &s.layout;
            s.hot.dispatch(
                device,
                queue,
                encoder,
                layout,
                &self.meshes[object].hot,
                key,
                dirty_verts,
                hot,
            );
            let s = &mut self.scatter;
            let cold: &[u8] = bytemuck::cast_slice(&s.cold_data);
            let layout = &s.layout;
            s.cold.dispatch(
                device,
                queue,
                encoder,
                layout,
                &self.meshes[object].cold,
                key,
                dirty_verts,
                cold,
            );
        }

        if !dirty_faces.is_empty() {
            let s = &mut self.scatter;
            s.face_data.clear();
            s.face_data.reserve(dirty_faces.len() * FACE_WORDS);
            for &f in dirty_faces.iter() {
                s.face_data.extend_from_slice(&mesh.faces[f as usize]);
            }
            let data: &[u8] = bytemuck::cast_slice(&s.face_data);
            sent += (dirty_faces.len() * 4 + data.len()) as u64;
            let layout = &s.layout;
            s.faces.dispatch(
                device,
                queue,
                encoder,
                layout,
                &self.meshes[object].ibuf,
                key,
                dirty_faces,
                data,
            );
        }

        // The draw range follows the face count even when no triangle moved.
        self.meshes[object].index_count = (fcount * 3) as u32;
        self.last_upload_bytes = sent;
        true
    }

    /// Writes the per-frame and per-object uniforms.
    pub fn set_uniforms(
        &self,
        queue: &wgpu::Queue,
        scene: &Scene,
        view: Mat4,
        proj: Mat4,
        eye: Vec3,
        s: &FrameSettings,
    ) {
        let view_proj = proj * view;
        let g = Globals {
            view_proj: view_proj.to_cols_array_2d(),
            view: view.to_cols_array_2d(),
            inv_view_proj: view_proj.inverse().to_cols_array_2d(),
            eye: [eye.x, eye.y, eye.z, 1.0],
            params: [
                if s.show_mask { 1.0 } else { 0.0 },
                if s.vertex_color || s.shading.forces_vertex_color() {
                    1.0
                } else {
                    0.0
                },
                s.shading.code(),
                s.opacity.clamp(0.05, 1.0),
            ],
            extra: [
                s.cavity_strength,
                if s.flat { 1.0 } else { 0.0 },
                s.grid_spacing.max(1e-3),
                // Fade the grid out well before the far plane.
                (self.grid_fade(eye)).max(1.0),
            ],
            bg_top: to_linear(s.background_top),
            bg_bottom: to_linear(s.background_bottom),
            tone: [
                // Stops, which is how anyone holding a camera thinks about it.
                (2.0f32).powf(s.exposure),
                s.contrast.max(0.0),
                s.saturation.max(0.0),
                if s.tone { 1.0 } else { 0.0 },
            ],
            occlusion: [
                s.occlusion_radius.max(1e-4),
                s.occlusion_strength.max(0.0),
                if s.occlusion { 1.0 } else { 0.0 },
                0.0,
            ],
        };
        queue.write_buffer(&self.global_buf, 0, bytemuck::bytes_of(&g));

        for (i, obj) in scene.objects.iter().enumerate().take(MAX_OBJECTS as usize) {
            let model = obj.transform.matrix();
            let normal_mat = Mat4::from_mat3(glam::Mat3::from_mat4(model).inverse().transpose());
            let active = i == scene.active;
            let data = ObjectData {
                model: model.to_cols_array_2d(),
                normal_mat: normal_mat.to_cols_array_2d(),
                tint: [1.0, 1.0, 1.0, if active { 1.0 } else { 0.0 }],
                _pad: [0.0; 12],
            };
            queue.write_buffer(
                &self.object_buf,
                i as u64 * OBJECT_STRIDE,
                bytemuck::bytes_of(&data),
            );
        }
    }

    fn grid_fade(&self, eye: Vec3) -> f32 {
        // Keep the grid legible whatever the zoom level.
        (eye.length() * 2.5).clamp(4.0, 60.0)
    }

    /// Draws one object, whole or by the ranges the view left standing.
    fn draw_object(
        pass: &mut wgpu::RenderPass<'_>,
        buffers: &MeshBuffers,
        ranges: Option<&[(u32, u32)]>,
    ) {
        match ranges {
            // Ranges are in faces; the index buffer counts in indices.
            Some(ranges) => {
                for (first, count) in ranges {
                    let start = first * 3;
                    pass.draw_indexed(start..start + count * 3, 0, 0..1);
                }
            }
            None => pass.draw_indexed(0..buffers.index_count, 0, 0..1),
        }
    }

    /// Darkens the creases of the frame that has just been drawn.
    ///
    /// A pass of its own, after the scene one has ended, because it reads the
    /// depth buffer and a texture cannot be sampled while it is still attached.
    pub fn draw_occlusion(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        s: &FrameSettings,
    ) {
        if !s.occlusion {
            return;
        }
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("occlusion"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.occlusion_pipeline);
        pass.set_bind_group(0, &self.occlusion_bind, &[]);
        pass.draw(0..3, 0..1);
    }

    /// `visible` holds, per object, the face ranges worth drawing, or `None` to
    /// draw the object whole.
    pub fn draw(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        scene: &Scene,
        s: &FrameSettings,
        visible: &[Option<Vec<(u32, u32)>>],
    ) {
        pass.set_bind_group(0, &self.global_bind, &[]);
        pass.set_pipeline(&self.background_pipeline);
        pass.draw(0..3, 0..1);

        pass.set_pipeline(&self.mesh_pipeline);
        for (i, obj) in scene.objects.iter().enumerate().take(MAX_OBJECTS as usize) {
            if !obj.visible {
                continue;
            }
            let Some(buffers) = self.meshes.get(i) else { continue };
            if buffers.index_count == 0 {
                continue;
            }
            let offset = (i as u64 * OBJECT_STRIDE) as u32;
            pass.set_bind_group(1, &self.object_bind, &[offset]);
            pass.set_vertex_buffer(0, buffers.hot.slice(..));
            pass.set_vertex_buffer(1, buffers.cold.slice(..));
            pass.set_index_buffer(buffers.ibuf.slice(..), wgpu::IndexFormat::Uint32);
            Self::draw_object(pass, buffers, visible.get(i).and_then(|r| r.as_deref()));
        }

        if s.wireframe {
            if let Some(wp) = &self.wire_pipeline {
                pass.set_pipeline(wp);
                for (i, obj) in scene.objects.iter().enumerate().take(MAX_OBJECTS as usize) {
                    if !obj.visible {
                        continue;
                    }
                    let Some(buffers) = self.meshes.get(i) else { continue };
                    if buffers.index_count == 0 {
                        continue;
                    }
                    let offset = (i as u64 * OBJECT_STRIDE) as u32;
                    pass.set_bind_group(1, &self.object_bind, &[offset]);
                    pass.set_vertex_buffer(0, buffers.hot.slice(..));
                    pass.set_vertex_buffer(1, buffers.cold.slice(..));
                    pass.set_index_buffer(buffers.ibuf.slice(..), wgpu::IndexFormat::Uint32);
                    Self::draw_object(pass, buffers, visible.get(i).and_then(|r| r.as_deref()));
                }
            }
        }

        if s.grid {
            pass.set_pipeline(&self.grid_pipeline);
            pass.draw(0..3, 0..1);
        }
    }
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

/// Sorts, deduplicates and drops slots past the end of the array.
fn prune(slots: &mut Vec<u32>, len: usize) {
    slots.sort_unstable();
    slots.dedup();
    slots.retain(|&s| (s as usize) < len);
}

fn empty_buffer(device: &wgpu::Device, label: &str, usage: wgpu::BufferUsages) -> wgpu::Buffer {
    sized_buffer(device, label, 1024, usage)
}

fn sized_buffer(
    device: &wgpu::Device,
    label: &str,
    size: u64,
    usage: wgpu::BufferUsages,
) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size,
        usage: usage | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn make_global_bind(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    uniform: &wgpu::Buffer,
    tex: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("global bind"),
        layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: uniform.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(tex) },
            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(sampler) },
        ],
    })
}

fn upload_matcap(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pixels: &[u8],
    size: u32,
) -> wgpu::TextureView {
    // A short buffer would be a validation error and take the window with it,
    // so fall back to a generated one rather than trusting the caller.
    let generated;
    let (pixels, size) = if pixels.len() == (size as usize) * (size as usize) * 4 && size > 0 {
        (pixels, size)
    } else {
        generated = matcap::generate(matcap::Preset::Clay, MATCAP_SIZE);
        (generated.as_slice(), MATCAP_SIZE)
    };
    let tex = device.create_texture_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: Some("matcap"),
            size: wgpu::Extent3d { width: size, height: size, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        pixels,
    );
    tex.create_view(&wgpu::TextureViewDescriptor::default())
}

fn make_depth(
    device: &wgpu::Device,
    width: u32,
    height: u32,
    samples: u32,
) -> wgpu::TextureView {
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("depth"),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: samples,
        dimension: wgpu::TextureDimension::D2,
        format: DEPTH_FORMAT,
        // Read as well as written: the occlusion pass is the only thing that
        // knows a crease is a crease, and depth is all it has to go on.
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    tex.create_view(&wgpu::TextureViewDescriptor::default())
}

/// What the occlusion pass reads: the globals, and the depth of the frame that
/// has just been drawn.
fn occlusion_layout(device: &wgpu::Device, samples: u32) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("occlusion layout"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: samples > 1,
                },
                count: None,
            },
        ],
    })
}

fn make_occlusion_bind(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    globals: &wgpu::Buffer,
    depth: &wgpu::TextureView,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("occlusion bind"),
        layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: globals.as_entire_binding() },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(depth),
            },
        ],
    })
}

fn make_msaa(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    width: u32,
    height: u32,
    samples: u32,
) -> Option<wgpu::TextureView> {
    if samples <= 1 {
        return None;
    }
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("msaa colour"),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: samples,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    Some(tex.create_view(&wgpu::TextureViewDescriptor::default()))
}

/// A minimal pipeline used only to initialise the struct before the real ones
/// are built; it is never drawn with.
fn dummy_pipeline(device: &wgpu::Device, format: wgpu::TextureFormat) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("placeholder"),
        source: wgpu::ShaderSource::Wgsl(
            r#"
            @vertex fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
                return vec4<f32>(0.0, 0.0, 0.0, 1.0);
            }
            @fragment fn fs_main() -> @location(0) vec4<f32> {
                return vec4<f32>(0.0, 0.0, 0.0, 1.0);
            }
            "#
            .into(),
        ),
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("placeholder"),
        layout: None,
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: None,
                write_mask: wgpu::ColorWrites::empty(),
            })],
        }),
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        multiview_mask: None,
        cache: None,
    })
}

/// Bounding sphere of everything visible, for framing the camera.
pub fn scene_bounds(scene: &Scene) -> (Vec3, f32) {
    let (lo, hi) = scene.bounds();
    ((lo + hi) * 0.5, ((hi - lo).length() * 0.5).max(0.05))
}
