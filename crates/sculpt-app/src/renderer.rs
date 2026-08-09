//! wgpu mesh renderer: matcap pipeline, growable vertex/index buffers, depth.

use crate::matcap;
use glam::Mat4;
use sculpt_core::Mesh;
use wgpu::util::DeviceExt;

pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const MATCAP_SIZE: u32 = 256;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    view_proj: [[f32; 4]; 4],
    view: [[f32; 4]; 4],
    params: [f32; 4],
}

pub struct MeshRenderer {
    pipeline: wgpu::RenderPipeline,
    wire_pipeline: Option<wgpu::RenderPipeline>,
    bind_layout: wgpu::BindGroupLayout,
    bind_group: wgpu::BindGroup,
    uniform_buf: wgpu::Buffer,
    sampler: wgpu::Sampler,
    matcap_view: wgpu::TextureView,
    vbuf: wgpu::Buffer,
    vcap: u64,
    ibuf: wgpu::Buffer,
    icap: u64,
    index_count: u32,
    depth_view: wgpu::TextureView,
}

impl MeshRenderer {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        color_format: wgpu::TextureFormat,
        width: u32,
        height: u32,
        wire_supported: bool,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("matcap"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });

        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mesh bind layout"),
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

        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("uniforms"),
            size: std::mem::size_of::<Uniforms>() as u64,
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

        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mesh pipeline layout"),
            bind_group_layouts: &[Some(&bind_layout)],
            immediate_size: 0,
        });

        let vertex_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<sculpt_core::Vertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 0, shader_location: 0 },
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 12, shader_location: 1 },
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 24, shader_location: 2 },
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32, offset: 36, shader_location: 3 },
            ],
        };

        let make_pipeline = |fs_entry: &str, polygon: wgpu::PolygonMode, bias: i32| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("mesh pipeline"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[Some(vertex_layout.clone())],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs_entry),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: color_format,
                        blend: Some(wgpu::BlendState::REPLACE),
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
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };

        let pipeline = make_pipeline("fs_main", wgpu::PolygonMode::Fill, 0);
        let wire_pipeline = wire_supported
            .then(|| make_pipeline("fs_wire", wgpu::PolygonMode::Line, -2));

        let vbuf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("vertices"),
            size: 1024,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let ibuf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("indices"),
            size: 1024,
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bind_group = make_bind_group(device, &bind_layout, &uniform_buf, &matcap_view, &sampler);
        let depth_view = make_depth(device, width, height);

        Self {
            pipeline,
            wire_pipeline,
            bind_layout,
            bind_group,
            uniform_buf,
            sampler,
            matcap_view,
            vbuf,
            vcap: 1024,
            ibuf,
            icap: 1024,
            index_count: 0,
            depth_view,
        }
    }

    pub fn set_matcap(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, preset: matcap::Preset) {
        self.matcap_view = upload_matcap(device, queue, preset);
        self.bind_group = make_bind_group(
            device,
            &self.bind_layout,
            &self.uniform_buf,
            &self.matcap_view,
            &self.sampler,
        );
    }

    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        self.depth_view = make_depth(device, width, height);
    }

    /// Uploads whatever the sculptor marked dirty.
    ///
    /// Buffers grow with 50% headroom so a dyntopo stroke does not reallocate
    /// on every step. Uploads are whole-buffer: the touched vertices are
    /// scattered, so a partial range would usually span everything anyway.
    /// Chunked dirty tracking is the next optimisation if this shows up in a
    /// profile.
    pub fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        mesh: &Mesh,
        verts_dirty: bool,
        topo_dirty: bool,
    ) {
        if verts_dirty || topo_dirty {
            let data: &[u8] = bytemuck::cast_slice(&mesh.verts);
            if data.len() as u64 > self.vcap {
                self.vcap = (data.len() as u64 * 3 / 2).next_power_of_two();
                self.vbuf = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("vertices"),
                    size: self.vcap,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
            }
            if !data.is_empty() {
                queue.write_buffer(&self.vbuf, 0, data);
            }
        }

        if topo_dirty {
            let data: &[u8] = bytemuck::cast_slice(&mesh.faces);
            if data.len() as u64 > self.icap {
                self.icap = (data.len() as u64 * 3 / 2).next_power_of_two();
                self.ibuf = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("indices"),
                    size: self.icap,
                    usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
            }
            if !data.is_empty() {
                queue.write_buffer(&self.ibuf, 0, data);
            }
            self.index_count = (mesh.faces.len() * 3) as u32;
        }
    }

    pub fn set_uniforms(
        &self,
        queue: &wgpu::Queue,
        view: Mat4,
        proj: Mat4,
        show_mask: bool,
        vertex_color: bool,
    ) {
        let u = Uniforms {
            view_proj: (proj * view).to_cols_array_2d(),
            view: view.to_cols_array_2d(),
            params: [
                if show_mask { 1.0 } else { 0.0 },
                if vertex_color { 1.0 } else { 0.0 },
                0.0,
                0.0,
            ],
        };
        queue.write_buffer(&self.uniform_buf, 0, bytemuck::bytes_of(&u));
    }

    pub fn depth_view(&self) -> &wgpu::TextureView {
        &self.depth_view
    }

    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, wireframe: bool) {
        if self.index_count == 0 {
            return;
        }
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, self.vbuf.slice(..));
        pass.set_index_buffer(self.ibuf.slice(..), wgpu::IndexFormat::Uint32);

        pass.set_pipeline(&self.pipeline);
        pass.draw_indexed(0..self.index_count, 0, 0..1);

        if wireframe {
            if let Some(wp) = &self.wire_pipeline {
                pass.set_pipeline(wp);
                pass.draw_indexed(0..self.index_count, 0, 0..1);
            }
        }
    }
}

fn make_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    uniform: &wgpu::Buffer,
    tex: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("mesh bind group"),
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

fn make_depth(device: &wgpu::Device, width: u32, height: u32) -> wgpu::TextureView {
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("depth"),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: DEPTH_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    tex.create_view(&wgpu::TextureViewDescriptor::default())
}
