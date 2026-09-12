//! Completed offscreen frames on the real GPU, at 1080p, with rotating camera.
//! cargo run --release -p sculpt-app --example viewport -- 9
//! Includes CPU submission and a GPU completion wait; excludes picking, UI,
//! vsync, loading and strokes. This is not the application's displayed FPS.
#![allow(dead_code)]
#[path = "../src/gpu_vertex.rs"]
mod gpu_vertex;
#[path = "../src/matcap.rs"]
mod matcap;
#[path = "../src/renderer.rs"]
mod renderer;

use glam::{Mat4, Vec3};
use renderer::{FrameSettings, Renderer};
use sculpt_core::{Object, Scene, cluster, primitives};
use std::time::Instant;

fn planes(m: Mat4) -> [[f32; 4]; 6] {
    let r = m.transpose();
    [
        r.w_axis + r.x_axis,
        r.w_axis - r.x_axis,
        r.w_axis + r.y_axis,
        r.w_axis - r.y_axis,
        r.z_axis,
        r.w_axis - r.z_axis,
    ]
    .map(|p| p.to_array())
}

fn main() {
    let level = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(9);
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
        apply_limit_buckets: false,
    }))
    .unwrap();
    println!("GPU: {:?}", adapter.get_info());
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("viewport benchmark"),
        required_features: wgpu::Features::empty(),
        required_limits: adapter.limits(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }))
    .unwrap();
    let mut mesh = if std::env::args().nth(1).as_deref() == Some("40m") {
        primitives::uv_sphere(4096, 5121)
    } else {
        primitives::icosphere(level)
    };
    let partition = cluster::build(&mut mesh, cluster::TARGET_FACES);
    println!("{} triangles; 1920 x 1080; Matcap", mesh.face_count());
    let scene = Scene::with_object(Object::new("sphere", mesh));
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("viewport"),
        size: wgpu::Extent3d {
            width: 1920,
            height: 1080,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let target_view = target.create_view(&Default::default());
    let mut renderer = Renderer::new(&device, &queue, format, 1920, 1080, false, 1);
    renderer.sync(&device, &queue, &scene, None);
    if std::env::args().any(|a| a == "--sculpt") {
        return sculpt_frames(&device, &queue, &target_view, renderer, scene, partition);
    }

    for samples in [1, 4] {
        if samples == 4
            && !adapter
                .get_texture_format_features(format)
                .flags
                .contains(wgpu::TextureFormatFeatureFlags::MULTISAMPLE_X4)
        {
            continue;
        }
        renderer.set_sample_count(&device, samples);
        for occlusion in [false, true] {
            let settings = FrameSettings {
                occlusion,
                ..Default::default()
            };
            let mut times = Vec::new();
            let mut drawn = 0u64;
            for i in 0..70 {
                let start = Instant::now();
                let angle = i as f32 * 0.04;
                let eye = Vec3::new(angle.sin() * 3.0, 0.4, angle.cos() * 3.0);
                let view = glam::camera::rh::view::look_at_mat4(eye, Vec3::ZERO, Vec3::Y);
                let proj =
                    glam::camera::rh::proj::directx::perspective(0.9, 1920.0 / 1080.0, 0.05, 100.0);
                let visible = partition.visible_ranges(&planes(proj * view), Some(eye));
                let count = sculpt_core::Partition::faces_in(&visible);
                renderer.set_uniforms(&queue, &scene, view, proj, eye, &settings);
                let mut encoder = device.create_command_encoder(&Default::default());
                {
                    let (view, resolve_target) = match renderer.msaa_view() {
                        Some(msaa) => (msaa, Some(&target_view)),
                        None => (&target_view, None),
                    };
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("viewport"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view,
                            resolve_target,
                            depth_slice: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                            view: renderer.depth_view(),
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
                    renderer.draw(&mut pass, &scene, &settings, &[Some(visible)]);
                }
                renderer.draw_occlusion(&mut encoder, &target_view, &settings);
                queue.submit([encoder.finish()]);
                device
                    .poll(wgpu::PollType::Wait {
                        submission_index: None,
                        timeout: None,
                    })
                    .unwrap();
                if i >= 10 {
                    times.push(start.elapsed().as_secs_f64() * 1000.0);
                    drawn += count as u64;
                }
            }
            times.sort_by(f64::total_cmp);
            println!(
                "MSAA {samples}, AO {occlusion}: complete frame p50 {:.3} ms, p95 {:.3} ms, max {:.3} ms; mean drawn {} tris",
                times[30],
                times[57],
                times[59],
                drawn / 60
            );
        }
    }
}

/// Includes picking, CPU sculpture, partition refit, sparse uploads, raster and
/// AO completion. Excludes egui, input-device latency, presentation and vsync.
fn sculpt_frames(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    target: &wgpu::TextureView,
    mut renderer: Renderer,
    scene: Scene,
    mut partition: sculpt_core::Partition,
) {
    use sculpt_core::{Sculptor, StrokeInput, query};
    let mut s = Sculptor::with_scene(scene);
    s.symmetry = false;
    s.brush.radius = std::env::args()
        .find_map(|arg| arg.strip_prefix("--radius=").and_then(|v| v.parse().ok()))
        .unwrap_or(0.15);
    s.dyntopo.max_verts = 40_000_000;
    let radius = s.brush.radius;
    s.mesh_mut().unwrap().ensure_accel(radius);
    s.take_dirty();
    s.topology_dirty = false;
    let settings = FrameSettings {
        grid: false,
        ..Default::default()
    };
    let eye = Vec3::new(0.0, 0.0, 3.0);
    let view = glam::camera::rh::view::look_at_mat4(eye, Vec3::ZERO, Vec3::Y);
    let proj = glam::camera::rh::proj::directx::perspective(0.9, 1920.0 / 1080.0, 0.05, 100.0);
    for dynamic in [false, true] {
        s.dyntopo_enabled = dynamic;
        s.begin_stroke();
        let mut timings = Vec::new();
        let mut upload = 0;
        for i in 0..70 {
            let start = Instant::now();
            let origin = Vec3::new(i as f32 * 0.004, 0.0, 3.0);
            if let Some(hit) = query::raycast(s.mesh(), origin, -Vec3::Z) {
                s.stroke(&StrokeInput {
                    point: hit.point,
                    normal: hit.normal,
                    ..Default::default()
                });
            }
            let (mut vertices, mut faces, full) = s.take_dirty();
            assert!(!full);
            cluster::follow(s.mesh(), &mut partition, &faces, cluster::TARGET_FACES);
            let ranges = partition.visible_ranges(&planes(proj * view), Some(eye));
            if !s.topology_dirty {
                faces.clear();
            }
            let mut encoder = device.create_command_encoder(&Default::default());
            assert!(renderer.sync_sparse(
                device,
                queue,
                &mut encoder,
                &s.scene,
                0,
                &mut vertices,
                &mut faces
            ));
            s.topology_dirty = false;
            renderer.set_uniforms(queue, &s.scene, view, proj, eye, &settings);
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("sculpt frame"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: target,
                        resolve_target: None,
                        depth_slice: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: renderer.depth_view(),
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
                renderer.draw(&mut pass, &s.scene, &settings, &[Some(ranges)]);
            }
            renderer.draw_occlusion(&mut encoder, target, &settings);
            queue.submit([encoder.finish()]);
            device
                .poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: None,
                })
                .unwrap();
            if i >= 10 {
                timings.push(start.elapsed().as_secs_f64() * 1000.0);
                upload += renderer.last_upload_bytes;
            }
        }
        s.end_stroke();
        timings.sort_by(f64::total_cmp);
        println!(
            "SCULPT dynamic={dynamic}, radius={}: p50 {:.3} ms, p95 {:.3} ms, max {:.3} ms; mean upload {} bytes",
            s.brush.radius,
            timings[30],
            timings[57],
            timings[59],
            upload / 60
        );
    }
}
