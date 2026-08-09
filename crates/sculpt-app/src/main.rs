//! sculpt-rs: a dynamic-topology 3D sculpting tool.

mod camera;
mod matcap;
mod renderer;
mod ui;

use camera::Camera;
use glam::Vec3;
use renderer::MeshRenderer;
use sculpt_core::{io, primitives, BrushKind, Sculptor, StrokeInput};
use std::sync::Arc;
use std::time::Instant;
use ui::{Action, Primitive, UiState};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

#[derive(Default)]
struct Input {
    mouse: (f32, f32),
    prev_mouse: (f32, f32),
    lmb: bool,
    mmb: bool,
    rmb: bool,
    shift: bool,
    ctrl: bool,
    alt: bool,
    /// Last surface hit of the active stroke: position and normal.
    last_hit: Option<(Vec3, Vec3)>,
    /// Grab point for the Move brush, tracked on a view-facing plane.
    move_anchor: Option<Vec3>,
}

struct State {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,

    mesh_renderer: MeshRenderer,
    egui_ctx: egui::Context,
    egui_state: egui_winit::State,
    egui_renderer: egui_wgpu::Renderer,

    sculptor: Sculptor,
    camera: Camera,
    input: Input,
    ui: UiState,
    last_frame: Instant,
}

impl State {
    fn new(window: Arc<Window>) -> Self {
        let size = window.inner_size();
        // `*_from_env` honours WGPU_BACKEND and friends, handy for debugging a
        // driver-specific problem without a rebuild.
        let instance = wgpu::Instance::new(
            wgpu::InstanceDescriptor::new_without_display_handle_from_env(),
        );
        let surface = instance
            .create_surface(window.clone())
            .expect("failed to create surface");

        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        }))
        .expect("no suitable GPU adapter");

        // Wireframe needs an optional feature; ask for it, carry on without it.
        let wire_supported = adapter.features().contains(wgpu::Features::POLYGON_MODE_LINE);
        let features = if wire_supported {
            wgpu::Features::POLYGON_MODE_LINE
        } else {
            wgpu::Features::empty()
        };

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("sculpt-rs device"),
            required_features: features,
            required_limits: wgpu::Limits::default(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .expect("failed to create device");

        // Start from the driver's own defaults, then override only what we care
        // about. Less brittle than spelling out every field.
        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .expect("surface is not supported by this adapter");
        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(config.format);
        config.format = format;
        config.usage = wgpu::TextureUsages::RENDER_ATTACHMENT;
        config.present_mode = wgpu::PresentMode::AutoVsync;
        config.desired_maximum_frame_latency = 2;
        surface.configure(&device, &config);

        let mesh_renderer = MeshRenderer::new(
            &device,
            &queue,
            format,
            config.width,
            config.height,
            wire_supported,
        );

        let egui_ctx = egui::Context::default();
        let egui_state = egui_winit::State::new(
            egui_ctx.clone(),
            egui::ViewportId::ROOT,
            &window,
            None,
            None,
            None,
        );
        // egui draws in its own pass with no depth attachment.
        let egui_renderer = egui_wgpu::Renderer::new(
            &device,
            format,
            egui_wgpu::RendererOptions {
                msaa_samples: 1,
                depth_stencil_format: None,
                ..Default::default()
            },
        );

        let sculptor = Sculptor::new(primitives::icosphere(5));
        let mut camera = Camera::default();
        camera.frame(Vec3::ZERO, 1.0);

        let mut ui = UiState::default();
        ui.wireframe_available = wire_supported;

        Self {
            window,
            surface,
            device,
            queue,
            config,
            mesh_renderer,
            egui_ctx,
            egui_state,
            egui_renderer,
            sculptor,
            camera,
            input: Input::default(),
            ui,
            last_frame: Instant::now(),
        }
    }

    fn resize(&mut self, w: u32, h: u32) {
        if w == 0 || h == 0 {
            return;
        }
        self.config.width = w;
        self.config.height = h;
        self.surface.configure(&self.device, &self.config);
        self.mesh_renderer.resize(&self.device, w, h);
    }

    fn aspect(&self) -> f32 {
        self.config.width as f32 / self.config.height.max(1) as f32
    }

    fn pick_at_cursor(&self) -> Option<sculpt_core::Hit> {
        // The scene is single-object for now; unwrap the world-space hit.
        let (o, d) = self.camera.ray(
            self.input.mouse.0,
            self.input.mouse.1,
            self.config.width as f32,
            self.config.height as f32,
        );
        self.sculptor.pick(o, d).map(|h| h.world)
    }

    /// Intersects the cursor ray with a view-facing plane through `anchor`.
    fn cursor_on_view_plane(&self, anchor: Vec3) -> Option<Vec3> {
        let (o, d) = self.camera.ray(
            self.input.mouse.0,
            self.input.mouse.1,
            self.config.width as f32,
            self.config.height as f32,
        );
        let n = (self.camera.eye() - self.camera.target).normalize_or(Vec3::Z);
        let denom = d.dot(n);
        if denom.abs() < 1e-6 {
            return None;
        }
        let t = (anchor - o).dot(n) / denom;
        (t > 0.0).then(|| o + d * t)
    }

    fn begin_stroke(&mut self) {
        let Some(hit) = self.pick_at_cursor() else {
            return;
        };
        self.sculptor.begin_stroke();
        self.sculptor.brush.negative = self.input.ctrl;
        self.input.last_hit = Some((hit.point, hit.normal));
        self.input.move_anchor = Some(hit.point);

        if self.sculptor.brush.kind != BrushKind::Move {
            self.sculptor.stroke(&StrokeInput {
                point: hit.point,
                normal: hit.normal,
                drag: Vec3::ZERO, ..Default::default()
            });
        }
    }

    fn continue_stroke(&mut self) {
        if self.sculptor.brush.kind == BrushKind::Move {
            let Some(anchor) = self.input.move_anchor else {
                return;
            };
            let Some(now) = self.cursor_on_view_plane(anchor) else {
                return;
            };
            let drag = now - anchor;
            if drag.length_squared() < 1e-12 {
                return;
            }
            let normal = self.input.last_hit.map(|h| h.1).unwrap_or(Vec3::Y);
            self.sculptor.stroke(&StrokeInput { point: anchor, normal, drag, ..Default::default() });
            self.input.move_anchor = Some(now);
            return;
        }

        let Some(hit) = self.pick_at_cursor() else {
            return;
        };
        let to = (hit.point, hit.normal);
        let from = self.input.last_hit.unwrap_or(to);

        // Space the dabs so a fast drag does not leave a dotted trail.
        let spacing = (self.sculptor.brush.radius * 0.25).max(1e-4);
        let steps = ((from.0.distance(to.0) / spacing).ceil() as usize).clamp(1, 32);
        for i in 1..=steps {
            let t = i as f32 / steps as f32;
            self.sculptor.stroke(&StrokeInput {
                point: from.0.lerp(to.0, t),
                normal: from.1.lerp(to.1, t).normalize_or(to.1),
                drag: Vec3::ZERO, ..Default::default()
            });
        }
        self.input.last_hit = Some(to);
    }

    fn end_stroke(&mut self) {
        self.sculptor.end_stroke();
        self.sculptor.brush.negative = false;
        self.input.last_hit = None;
        self.input.move_anchor = None;
    }

    fn frame_view(&mut self) {
        let (lo, hi) = self.sculptor.mesh().bounds();
        let c = (lo + hi) * 0.5;
        let r = ((hi - lo).length() * 0.5).max(0.05);
        self.camera.frame(c, r);
    }

    fn handle_actions(&mut self, actions: Vec<Action>) {
        for a in actions {
            match a {
                Action::New(p) => {
                    let mesh = match p {
                        Primitive::Sphere => primitives::icosphere(5),
                        Primitive::Cube => primitives::cube(24),
                        Primitive::Cylinder => primitives::cylinder(48, 24),
                        Primitive::Torus => primitives::torus(64, 32, 0.35),
                        Primitive::Plane => primitives::plane(48),
                    };
                    self.sculptor.replace_mesh(mesh);
                    self.frame_view();
                    self.ui.status.clear();
                }
                Action::Import => {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("Mesh", &["obj", "stl"])
                        .pick_file()
                    {
                        match io::load(&path) {
                            Ok(mut m) => {
                                m.normalize_scale();
                                m.recompute_normals();
                                self.sculptor.replace_mesh(m);
                                self.frame_view();
                                self.ui.status = format!(
                                    "loaded {}",
                                    path.file_name().unwrap_or_default().to_string_lossy()
                                );
                            }
                            Err(e) => self.ui.status = format!("import failed: {e}"),
                        }
                    }
                }
                Action::Export => {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("Wavefront OBJ", &["obj"])
                        .add_filter("Binary STL", &["stl"])
                        .set_file_name("sculpt.obj")
                        .save_file()
                    {
                        match io::save(self.sculptor.mesh(), &path) {
                            Ok(()) => {
                                self.ui.status = format!(
                                    "saved {}",
                                    path.file_name().unwrap_or_default().to_string_lossy()
                                )
                            }
                            Err(e) => self.ui.status = format!("export failed: {e}"),
                        }
                    }
                }
                Action::Undo => self.sculptor.undo(),
                Action::Redo => self.sculptor.redo(),
                Action::ClearMask => self.sculptor.clear_mask(),
                Action::InvertMask => self.sculptor.invert_mask(),
                Action::FrameView => self.frame_view(),
                Action::MatcapChanged => {
                    self.mesh_renderer
                        .set_matcap(&self.device, &self.queue, self.ui.matcap);
                }
            }
        }
    }

    fn render(&mut self) {
        let now = Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f32();
        self.last_frame = now;
        if dt > 0.0 {
            // Exponential moving average, otherwise the readout is unreadable.
            self.ui.fps = self.ui.fps * 0.9 + (1.0 / dt) * 0.1;
        }

        use wgpu::CurrentSurfaceTexture as Cst;
        let frame = match self.surface.get_current_texture() {
            Cst::Success(f) | Cst::Suboptimal(f) => f,
            Cst::Outdated | Cst::Lost => {
                self.surface.configure(&self.device, &self.config);
                return;
            }
            _ => return,
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        self.mesh_renderer.upload(
            &self.device,
            &self.queue,
            self.sculptor.mesh(),
            self.sculptor.verts_dirty,
            self.sculptor.topology_dirty,
        );
        self.sculptor.verts_dirty = false;
        self.sculptor.topology_dirty = false;

        self.mesh_renderer.set_uniforms(
            &self.queue,
            self.camera.view(),
            self.camera.proj(self.aspect()),
            self.ui.show_mask,
            self.ui.vertex_color,
        );

        // Brush cursor radius in pixels, measured at the surface under it.
        let cursor = (!self.egui_ctx.egui_wants_pointer_input())
            .then(|| self.pick_at_cursor())
            .flatten()
            .map(|hit| {
                let ppp = self.egui_ctx.pixels_per_point();
                let r = self.camera.world_radius_to_pixels(
                    hit.point,
                    self.sculptor.brush.radius,
                    self.config.height as f32,
                );
                (
                    egui::pos2(self.input.mouse.0 / ppp, self.input.mouse.1 / ppp),
                    r / ppp,
                )
            });

        let raw_input = self.egui_state.take_egui_input(&self.window);
        let ctx = self.egui_ctx.clone();
        let mut actions = Vec::new();
        let full_output = {
            let sculptor = &mut self.sculptor;
            let ui_state = &mut self.ui;
            ctx.run_ui(raw_input, |root| {
                actions = ui::draw(root, sculptor, ui_state, cursor);
            })
        };
        self.egui_state
            .handle_platform_output(&self.window, full_output.platform_output);
        let paint_jobs = ctx.tessellate(full_output.shapes, full_output.pixels_per_point);
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [self.config.width, self.config.height],
            pixels_per_point: full_output.pixels_per_point,
        };

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });

        for (id, deltas) in &full_output.textures_delta.set {
            for delta in deltas {
                self.egui_renderer
                    .update_texture(&self.device, &self.queue, *id, delta);
            }
        }
        self.egui_renderer.update_buffers(
            &self.device,
            &self.queue,
            &mut encoder,
            &paint_jobs,
            &screen,
        );

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("mesh pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.055,
                            g: 0.058,
                            b: 0.066,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: self.mesh_renderer.depth_view(),
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
            self.mesh_renderer.draw(&mut pass, self.ui.wireframe);
        }

        {
            let mut pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("egui pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
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
            self.egui_renderer.render(&mut pass, &paint_jobs, &screen);
        }

        self.queue.submit(Some(encoder.finish()));
        self.queue.present(frame);

        for id in &full_output.textures_delta.free {
            self.egui_renderer.free_texture(id);
        }

        self.handle_actions(actions);
    }
}

#[derive(Default)]
struct App {
    state: Option<State>,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("sculpt-rs")
            .with_inner_size(winit::dpi::LogicalSize::new(1440.0, 900.0));
        let window = Arc::new(el.create_window(attrs).expect("failed to create window"));
        self.state = Some(State::new(window));
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(st) = self.state.as_mut() else {
            return;
        };

        // egui gets first refusal on every event.
        let response = st.egui_state.on_window_event(&st.window, &event);
        if response.repaint {
            st.window.request_redraw();
        }
        let egui_captured = response.consumed || st.egui_ctx.egui_wants_pointer_input();

        match event {
            WindowEvent::CloseRequested => el.exit(),
            WindowEvent::Resized(size) => st.resize(size.width, size.height),
            WindowEvent::RedrawRequested => {
                st.render();
                st.window.request_redraw();
            }
            WindowEvent::ModifiersChanged(m) => {
                let s = m.state();
                st.input.shift = s.shift_key();
                st.input.ctrl = s.control_key();
                st.input.alt = s.alt_key();
            }
            WindowEvent::CursorMoved { position, .. } => {
                st.input.prev_mouse = st.input.mouse;
                st.input.mouse = (position.x as f32, position.y as f32);
                let dx = st.input.mouse.0 - st.input.prev_mouse.0;
                let dy = st.input.mouse.1 - st.input.prev_mouse.1;

                if st.input.mmb || st.input.rmb || (st.input.lmb && st.input.alt) {
                    if st.input.shift && st.input.mmb {
                        st.camera.pan(dx, dy, st.config.height as f32);
                    } else {
                        st.camera.orbit(dx, dy);
                    }
                } else if st.input.lmb && st.input.last_hit.is_some() {
                    st.continue_stroke();
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                if !egui_captured {
                    let amount = match delta {
                        MouseScrollDelta::LineDelta(_, y) => y,
                        MouseScrollDelta::PixelDelta(p) => p.y as f32 / 60.0,
                    };
                    st.camera.zoom(amount);
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let down = state == ElementState::Pressed;
                match button {
                    MouseButton::Left => {
                        st.input.lmb = down;
                        if down {
                            if !egui_captured && !st.input.alt {
                                st.begin_stroke();
                            }
                        } else {
                            st.end_stroke();
                        }
                    }
                    MouseButton::Middle => st.input.mmb = down,
                    MouseButton::Right => st.input.rmb = down,
                    _ => {}
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if egui_captured || event.state != ElementState::Pressed {
                    return;
                }
                let PhysicalKey::Code(code) = event.physical_key else {
                    return;
                };
                match code {
                    KeyCode::KeyZ if st.input.ctrl && st.input.shift => st.sculptor.redo(),
                    KeyCode::KeyZ if st.input.ctrl => st.sculptor.undo(),
                    KeyCode::KeyY if st.input.ctrl => st.sculptor.redo(),
                    KeyCode::KeyX => st.sculptor.symmetry = !st.sculptor.symmetry,
                    KeyCode::KeyW if st.ui.wireframe_available => {
                        st.ui.wireframe = !st.ui.wireframe
                    }
                    KeyCode::KeyF => st.frame_view(),
                    KeyCode::BracketLeft => {
                        st.sculptor.brush.radius = (st.sculptor.brush.radius * 0.85).max(0.005)
                    }
                    KeyCode::BracketRight => {
                        st.sculptor.brush.radius = (st.sculptor.brush.radius * 1.18).min(1.0)
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

fn main() {
    let el = EventLoop::new().expect("failed to create event loop");
    el.set_control_flow(ControlFlow::Poll);
    let mut app = App::default();
    el.run_app(&mut app).expect("event loop failed");
}
