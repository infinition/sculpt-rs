//! Render the real UI and studio shader to PNG without a window.
//! cargo run --release -p sculpt-app --example studio_preview -- target/studio.png
#![allow(dead_code)]
#[path = "../src/camera.rs"]
mod camera;
#[path = "../src/gizmo.rs"]
mod gizmo;
#[path = "../src/gpu_vertex.rs"]
mod gpu_vertex;
#[path = "../src/hud.rs"]
mod hud;
#[path = "../src/icons.rs"]
mod icons;
#[path = "../src/input.rs"]
mod input;
#[path = "../src/matcap.rs"]
mod matcap;
#[path = "../src/materials.rs"]
mod materials;
#[path = "../src/navwidget.rs"]
mod navwidget;
#[path = "../src/renderer.rs"]
mod renderer;
#[path = "../src/theme.rs"]
mod theme;
#[path = "../src/ui.rs"]
mod ui;
#[path = "../src/wheel.rs"]
mod wheel;
#[path = "../src/widgets.rs"]
mod widgets;

use glam::Vec3;
use sculpt_core::{BrushKind, Sculptor, StrokeInput, primitives};

fn main() {
    let output = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "target/studio-preview.png".into());
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
        apply_limit_buckets: false,
    }))
    .unwrap();
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_limits: adapter.limits(),
        ..Default::default()
    }))
    .unwrap();
    const W: u32 = 1280;
    const H: u32 = 800;
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("studio preview"),
        size: wgpu::Extent3d {
            width: W,
            height: H,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let target_view = target.create_view(&Default::default());
    let mut r = renderer::Renderer::new(&device, &queue, format, W, H, false, 1);
    let mut sculptor = Sculptor::new(primitives::icosphere(6));
    sculptor.symmetry = false;
    sculptor.dyntopo_enabled = false;
    sculptor.brush.radius = 0.16;
    sculptor.brush.kind = BrushKind::Clay;
    sculptor.begin_stroke();
    for i in 0..42 {
        let a = i as f32 * 0.06;
        let point = Vec3::new(a.cos() * 0.38, a.sin() * 0.4, 1.0).normalize();
        sculptor.stroke(&StrokeInput {
            point,
            normal: point,
            ..Default::default()
        });
    }
    sculptor.end_stroke();
    sculptor.brush.alpha = Some(2);
    r.sync(&device, &queue, &sculptor.scene, None);
    let mut camera = camera::Camera::default();
    camera.frame(Vec3::ZERO, 1.0);
    camera.settle();
    let ctx = egui::Context::default();
    let mut st = ui::UiState::default();
    st.settings.shading = renderer::Shading::Clay;
    st.settings.grid = false;
    st.tab = if std::env::args().any(|a| a == "materials") {
        ui::Tab::Materials
    } else {
        ui::Tab::Brush
    };
    st.brushes.push(sculpt_core::io::NamedBrush {
        name: "Sculpting clay".into(),
        brush: sculptor.brush,
        icon: Some("Clay".into()),
    });
    let mut gizmo = gizmo::Gizmo::default();
    let mut egui_renderer = egui_wgpu::Renderer::new(&device, format, Default::default());
    let overlay = ui::Overlay {
        cursor: Some((egui::pos2(610.0, 400.0), 54.0)),
        cursor_tilt: Some((0.2, -0.15)),
        anchor: None,
        stroking: false,
        footprint: Some([
            egui::pos2(558.0, 350.0),
            egui::pos2(656.0, 345.0),
            egui::pos2(664.0, 447.0),
            egui::pos2(562.0, 454.0),
        ]),
    };
    for frame in 0..3 {
        theme::apply(&ctx, &st.theme);
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(W as f32, H as f32),
            )),
            time: Some(frame as f64 / 60.0),
            ..Default::default()
        };
        let full = ctx.run_ui(input, |root| {
            ui::draw(
                root,
                &mut sculptor,
                &mut st,
                &mut camera,
                &mut gizmo,
                &overlay,
            );
        });
        let jobs = ctx.tessellate(full.shapes, full.pixels_per_point);
        let center = st.viewport.center();
        camera.set_canvas_center(glam::Vec2::new(center.x, center.y), glam::Vec2::new(W as f32, H as f32));
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [W, H],
            pixels_per_point: full.pixels_per_point,
        };
        for (id, deltas) in &full.textures_delta.set {
            for delta in deltas {
                egui_renderer.update_texture(&device, &queue, *id, delta);
            }
        }
        let mut encoder = device.create_command_encoder(&Default::default());
        let callbacks = egui_renderer.update_buffers(&device, &queue, &mut encoder, &jobs, &screen);
        r.set_uniforms(
            &queue,
            &sculptor.scene,
            camera.view(),
            camera.proj(W as f32 / H as f32),
            camera.eye(),
            &st.settings,
        );
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("model"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target_view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: r.depth_view(),
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            r.draw(&mut pass, &sculptor.scene, &st.settings, &[None]);
        }
        r.draw_occlusion(&mut encoder, &target_view, &st.settings);
        {
            let mut pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("UI"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &target_view,
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
                })
                .forget_lifetime();
            egui_renderer.render(&mut pass, &jobs, &screen);
        }
        queue.submit(callbacks.into_iter().chain([encoder.finish()]));
        device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .unwrap();
        for id in &full.textures_delta.free {
            egui_renderer.free_texture(id);
        }
    }
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("pixels"),
        size: (W * H * 4) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &target,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(W * 4),
                rows_per_image: Some(H),
            },
        },
        wgpu::Extent3d {
            width: W,
            height: H,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);
    buffer.slice(..).map_async(wgpu::MapMode::Read, |_| {});
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        })
        .unwrap();
    let pixels = buffer.slice(..).get_mapped_range().unwrap();
    image::save_buffer(&output, &pixels, W, H, image::ColorType::Rgba8).unwrap();
    println!("{output}");
}
