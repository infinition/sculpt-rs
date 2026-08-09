//! wgpu renderer: gradient background, ground grid, one pass per object,
//! optional wireframe overlay, optional multisampling.
//!
//! Per-object data lives in one uniform buffer addressed with dynamic offsets,
//! so drawing a scene costs one bind group rebind per object and no buffer
//! writes inside the pass.

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
    pub background_top: [f32; 3],
    pub background_bottom: [f32; 3],
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
            // Held as sRGB so the colour pickers show what the viewport shows.
            background_top: [0.16, 0.17, 0.19],
            background_bottom: [0.075, 0.08, 0.09],
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
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct ObjectData {
    model: [[f32; 4]; 4],
    normal_mat: [[f32; 4]; 4],
    tint: [f32; 4],
    _pad: [f32; 12],
}

/// GPU mirror of one mesh.
struct MeshBuffers {
    vbuf: wgpu::Buffer,
    vcap: u64,
    ibuf: wgpu::Buffer,
    icap: u64,
    index_count: u32,
}

impl MeshBuffers {
    fn new(device: &wgpu::Device) -> Self {
        Self {
            vbuf: empty_buffer(device, "vertices", wgpu::BufferUsages::VERTEX),
            vcap: 1024,
            ibuf: empty_buffer(device, "indices", wgpu::BufferUsages::INDEX),
            icap: 1024,
            index_count: 0,
        }
    }

    /// Uploads the mesh. Buffers grow with 50% headroom so a dyntopo stroke does
    /// not reallocate on every step.
    fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, mesh: &sculpt_core::Mesh) {
        let vdata: &[u8] = bytemuck::cast_slice(&mesh.verts);
        if vdata.len() as u64 > self.vcap {
            self.vcap = (vdata.len() as u64 * 3 / 2).next_power_of_two();
            self.vbuf = sized_buffer(device, "vertices", self.vcap, wgpu::BufferUsages::VERTEX);
        }
        if !vdata.is_empty() {
            queue.write_buffer(&self.vbuf, 0, vdata);
        }

        let idata: &[u8] = bytemuck::cast_slice(&mesh.faces);
        if idata.len() as u64 > self.icap {
            self.icap = (idata.len() as u64 * 3 / 2).next_power_of_two();
            self.ibuf = sized_buffer(device, "indices", self.icap, wgpu::BufferUsages::INDEX);
        }
        if !idata.is_empty() {
            queue.write_buffer(&self.ibuf, 0, idata);
        }
        self.index_count = (mesh.faces.len() * 3) as u32;
    }
}

pub struct Renderer {
    mesh_pipeline: wgpu::RenderPipeline,
    wire_pipeline: Option<wgpu::RenderPipeline>,
    background_pipeline: wgpu::RenderPipeline,
    grid_pipeline: wgpu::RenderPipeline,

    global_layout: wgpu::BindGroupLayout,
    global_bind: wgpu::BindGroup,
    global_buf: wgpu::Buffer,
    object_layout: wgpu::BindGroupLayout,
    object_bind: wgpu::BindGroup,
    object_buf: wgpu::Buffer,

    sampler: wgpu::Sampler,
    matcap_view: wgpu::TextureView,

    meshes: Vec<MeshBuffers>,
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
        let matcap_view = upload_matcap(device, queue, matcap::Preset::Clay);

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

        let mut me = Self {
            // Placeholders replaced right below; building pipelines needs the
            // layouts that were just created.
            mesh_pipeline: dummy_pipeline(device, color_format),
            wire_pipeline: None,
            background_pipeline: dummy_pipeline(device, color_format),
            grid_pipeline: dummy_pipeline(device, color_format),
            global_layout,
            global_bind,
            global_buf,
            object_layout,
            object_bind,
            object_buf,
            sampler,
            matcap_view,
            meshes: Vec::new(),
            depth_view: make_depth(device, width, height, sample_count),
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

        let stride = std::mem::size_of::<sculpt_core::Vertex>() as u64;
        let attributes = [
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 0, shader_location: 0 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 12, shader_location: 1 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 24, shader_location: 2 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32, offset: 36, shader_location: 3 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32, offset: 40, shader_location: 4 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32, offset: 44, shader_location: 5 },
        ];
        let vertex_layout = wgpu::VertexBufferLayout {
            array_stride: stride,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &attributes,
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
                    buffers: &[Some(vertex_layout.clone())],
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
    }

    /// Rebuilds every pipeline for a new sample count.
    pub fn set_sample_count(&mut self, device: &wgpu::Device, samples: u32) {
        if samples == self.sample_count {
            return;
        }
        self.sample_count = samples;
        self.depth_view = make_depth(device, self.width, self.height, samples);
        self.msaa_view = make_msaa(device, self.color_format, self.width, self.height, samples);
        self.build_pipelines(device);
    }

    pub fn set_matcap(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, preset: matcap::Preset) {
        self.matcap_view = upload_matcap(device, queue, preset);
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
        for (i, obj) in scene.objects.iter().enumerate() {
            if dirty.is_none_or(|d| d == i) {
                self.meshes[i].upload(device, queue, &obj.mesh);
            }
        }
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

    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, scene: &Scene, s: &FrameSettings) {
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
            pass.set_vertex_buffer(0, buffers.vbuf.slice(..));
            pass.set_index_buffer(buffers.ibuf.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..buffers.index_count, 0, 0..1);
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
                    pass.set_vertex_buffer(0, buffers.vbuf.slice(..));
                    pass.set_index_buffer(buffers.ibuf.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..buffers.index_count, 0, 0..1);
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
    preset: matcap::Preset,
) -> wgpu::TextureView {
    let pixels = matcap::generate(preset, MATCAP_SIZE);
    let tex = device.create_texture_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: Some("matcap"),
            size: wgpu::Extent3d { width: MATCAP_SIZE, height: MATCAP_SIZE, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        &pixels,
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
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    tex.create_view(&wgpu::TextureViewDescriptor::default())
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
