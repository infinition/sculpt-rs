//! sculpt-rs: a dynamic-topology 3D sculpting tool.

mod camera;
mod gizmo;
mod icons;
mod input;
mod matcap;
mod navwidget;
mod renderer;
mod theme;
mod ui;
mod wheel;
mod widgets;

use camera::{Camera, Projection, ViewPreset};
use gizmo::Gizmo;
use glam::{Vec2, Vec3};
use input::{Gesture, Input, TouchOutcome};
use renderer::Renderer;
use sculpt_core::{io, primitives, BrushKind, Mesh, Object, Sculptor, StrokeInput};
use std::sync::Arc;
use std::time::Instant;
use ui::{Action, Overlay, Primitive, UiState};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

/// State that only lives for the duration of one stroke.
#[derive(Default)]
struct Stroke {
    active: bool,
    /// Last surface hit: position and normal, in world space.
    last_hit: Option<(Vec3, Vec3)>,
    /// Where the stroke started, for the brushes that lock their plane.
    anchor: Option<(Vec3, Vec3)>,
    /// Grab point for Move and Drag, tracked on a view-facing plane.
    grab: Option<Vec3>,
    /// Screen position the stroke started at, for Twist and Scale.
    screen_origin: Vec2,
    prev_screen: Vec2,
    /// Brush the stroke started with, so a modifier swap cannot change tools
    /// halfway through.
    kind: BrushKind,
    /// Tool the user actually picked, restored when the modifier is released.
    restore_kind: Option<BrushKind>,
}

struct State {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    max_samples: u32,

    renderer: Renderer,
    egui_ctx: egui::Context,
    egui_state: egui_winit::State,
    egui_renderer: egui_wgpu::Renderer,

    sculptor: Sculptor,
    camera: Camera,
    input: Input,
    ui: UiState,
    gizmo: Gizmo,
    stroke: Stroke,
    last_frame: Instant,
    /// Object whose buffers need re-uploading, or `None` for all of them.
    dirty_object: Option<usize>,
    full_resync: bool,
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

        // A sample count is only usable when the colour format and the depth
        // format both accept it. Asking for one the depth buffer cannot do is a
        // validation error, which on this path means the window simply closes.
        let colour_flags = adapter.get_texture_format_features(format).flags;
        let depth_flags = adapter
            .get_texture_format_features(renderer::DEPTH_FORMAT)
            .flags;
        let max_samples = [8u32, 4, 2]
            .into_iter()
            .find(|n| colour_flags.sample_count_supported(*n) && depth_flags.sample_count_supported(*n))
            .unwrap_or(1);
        let samples = max_samples.min(4);

        let renderer = Renderer::new(
            &device,
            &queue,
            format,
            config.width,
            config.height,
            wire_supported,
            samples,
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
        // egui draws in its own pass with no depth attachment and no MSAA.
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
        camera.settle();

        let ui = UiState {
            wireframe_available: wire_supported,
            msaa_available: max_samples > 1,
            max_samples,
            sample_count: samples,
            ..Default::default()
        };

        Self {
            window,
            surface,
            device,
            queue,
            config,
            max_samples,
            renderer,
            egui_ctx,
            egui_state,
            egui_renderer,
            sculptor,
            camera,
            input: Input::default(),
            ui,
            gizmo: Gizmo::default(),
            stroke: Stroke::default(),
            last_frame: Instant::now(),
            dirty_object: None,
            full_resync: true,
        }
    }

    fn resize(&mut self, w: u32, h: u32) {
        if w == 0 || h == 0 {
            return;
        }
        self.config.width = w;
        self.config.height = h;
        self.surface.configure(&self.device, &self.config);
        self.renderer.resize(&self.device, w, h);
    }

    fn aspect(&self) -> f32 {
        self.config.width as f32 / self.config.height.max(1) as f32
    }

    fn viewport(&self) -> (f32, f32) {
        (self.config.width as f32, self.config.height as f32)
    }

    // ---- picking ------------------------------------------------------------

    fn ray_at(&self, p: Vec2) -> (Vec3, Vec3) {
        let (w, h) = self.viewport();
        self.camera.ray(p.x, p.y, w, h)
    }

    #[allow(dead_code)]
    fn pick_at(&self, p: Vec2) -> Option<sculpt_core::SceneHit> {
        let (o, d) = self.ray_at(p);
        self.sculptor.pick(o, d)
    }

    /// The pick a brush should use: the surface under the cursor, or the
    /// nearest surface just off the silhouette. Grabbing the edge of a form
    /// needs the latter.
    ///
    /// The reach is a screen distance, not a world one. A brush radius of reach
    /// sounds generous until you try to swing the camera: every click anywhere
    /// near the model would find it and start sculpting. A few points is enough
    /// to catch the edge and small enough to still be able to miss on purpose.
    fn pick_for_brush(&self, p: Vec2) -> Option<sculpt_core::SceneHit> {
        let (o, d) = self.ray_at(p);
        let reach = self.reach_in_world();
        if reach <= 0.0 {
            return self.sculptor.pick(o, d);
        }
        self.sculptor.pick_soft(o, d, reach)
    }

    /// Converts the configured screen reach into world units, measured at the
    /// pivot, which is as good a stand-in for the model's depth as we have.
    fn reach_in_world(&self) -> f32 {
        let points = self.ui.brush_reach;
        if points <= 0.0 {
            return 0.0;
        }
        let (_, h) = self.viewport();
        let per_world = self.camera.pixels_per_world(self.camera.target(), h).max(1e-6);
        points * self.egui_ctx.pixels_per_point() / per_world
    }

    /// Intersects the cursor ray with a view-facing plane through `anchor`.
    fn cursor_on_view_plane(&self, anchor: Vec3, p: Vec2) -> Option<Vec3> {
        let (o, d) = self.ray_at(p);
        let n = -self.camera.forward();
        let denom = d.dot(n);
        if denom.abs() < 1e-6 {
            return None;
        }
        let t = (anchor - o).dot(n) / denom;
        t.is_finite().then(|| o + d * t)
    }

    // ---- strokes ------------------------------------------------------------

    /// The frame data the gizmo needs, in the units it expects.
    fn gizmo_view(&self) -> gizmo::View<'_> {
        gizmo::View {
            camera: &self.camera,
            size: self.viewport(),
            pixels_per_point: self.egui_ctx.pixels_per_point(),
        }
    }

    fn cursor_points(&self) -> egui::Pos2 {
        let ppp = self.egui_ctx.pixels_per_point().max(1e-3);
        egui::pos2(self.input.cursor.x / ppp, self.input.cursor.y / ppp)
    }

    /// Offers the click to the gizmo. Returns true when it took it.
    fn gizmo_press(&mut self) -> bool {
        let Some(t) = self.sculptor.scene.active().map(|o| o.transform) else {
            return false;
        };
        let cursor = self.cursor_points();
        let view = gizmo::View {
            camera: &self.camera,
            size: self.viewport(),
            pixels_per_point: self.egui_ctx.pixels_per_point(),
        };
        let took = self.gizmo.press(cursor, &view, &t);
        if took {
            // One undo step per drag, recorded before anything moves.
            let index = self.sculptor.scene.active;
            self.sculptor.history.push_placement(&self.sculptor.scene, index);
        }
        took
    }

    fn gizmo_drag(&mut self) {
        if !self.gizmo.is_dragging() {
            return;
        }
        let Some(t) = self.sculptor.scene.active().map(|o| o.transform) else {
            return;
        };
        let cursor = self.cursor_points();
        let view = gizmo::View {
            camera: &self.camera,
            size: self.viewport(),
            pixels_per_point: self.egui_ctx.pixels_per_point(),
        };
        if let Some(next) = self.gizmo.drag(cursor, &view, &t) {
            if let Some(o) = self.sculptor.scene.active_mut() {
                o.transform = next;
            }
        }
    }

    fn begin_stroke(&mut self, pressure: f32) {
        let cursor = self.input.cursor;
        let Some(hit) = self.pick_for_brush(cursor) else {
            return;
        };

        if self.ui.picking_color {
            if let Some(c) = self.sculptor.sample_color(&hit) {
                self.sculptor.brush.paint_color = c;
                self.ui.picking_color = false;
                self.ui.say("colour picked");
            }
            return;
        }

        // Clicking a different object selects it rather than sculpting through
        // the one you meant to leave alone.
        if hit.object != self.sculptor.scene.active {
            self.sculptor.select(hit.object);
            self.full_resync = true;
        }

        // Fill acts on the click, not on the drag that follows.
        if self.sculptor.brush.kind == BrushKind::Fill {
            let n = self.sculptor.fill_at(&hit);
            self.dirty_object = Some(self.sculptor.scene.active);
            self.ui.say(format!("filled {n} vertices"));
            return;
        }

        self.sculptor.begin_stroke();
        self.sculptor.brush.negative = self.input.ctrl;
        self.stroke = Stroke {
            active: true,
            last_hit: Some((hit.world.point, hit.world.normal)),
            anchor: Some((hit.world.point, hit.world.normal)),
            grab: Some(hit.world.point),
            screen_origin: cursor,
            prev_screen: cursor,
            kind: self.sculptor.brush.kind,
            restore_kind: self.stroke.restore_kind,
        };

        if !self.sculptor.brush.kind.is_grab() {
            let input = self.stroke_input(hit.world.point, hit.world.normal, Vec3::ZERO, pressure);
            self.sculptor.stroke(&input);
            self.dirty_object = Some(self.sculptor.scene.active);
        }
    }

    fn stroke_input(&self, point: Vec3, normal: Vec3, drag: Vec3, pressure: f32) -> StrokeInput {
        StrokeInput {
            point,
            normal,
            drag,
            view_dir: self.camera.forward(),
            pressure,
            ..Default::default()
        }
    }

    fn continue_stroke(&mut self, pressure: f32) {
        if !self.stroke.active {
            return;
        }
        let cursor = self.input.cursor;

        if self.stroke.kind.is_grab() {
            self.continue_grab_stroke(cursor, pressure);
            self.stroke.prev_screen = cursor;
            self.dirty_object = Some(self.sculptor.scene.active);
            return;
        }

        let Some(hit) = self.pick_for_brush(cursor) else {
            return;
        };
        let mut to = (hit.world.point, hit.world.normal);
        if self.sculptor.brush.lock_plane {
            // Keep pushing against the plane the stroke started on.
            if let Some((_, n)) = self.stroke.anchor {
                to.1 = n;
            }
        }
        let from = self.stroke.last_hit.unwrap_or(to);

        // Space the dabs so a fast drag does not leave a dotted trail.
        let spacing = (self.sculptor.brush.radius * 0.25).max(1e-4);
        let steps = ((from.0.distance(to.0) / spacing).ceil() as usize).clamp(1, 32);
        for i in 1..=steps {
            let t = i as f32 / steps as f32;
            let input = self.stroke_input(
                from.0.lerp(to.0, t),
                from.1.lerp(to.1, t).normalize_or(to.1),
                Vec3::ZERO,
                pressure,
            );
            self.sculptor.stroke(&input);
        }
        self.stroke.last_hit = Some(to);
        self.stroke.prev_screen = cursor;
        self.dirty_object = Some(self.sculptor.scene.active);
    }

    /// Move, Drag, Twist and Scale all key off cursor motion instead of the
    /// surface under it, so they share this path.
    fn continue_grab_stroke(&mut self, cursor: Vec2, pressure: f32) {
        let kind = self.stroke.kind;
        let Some(grab) = self.stroke.grab else { return };
        let normal = self.stroke.anchor.map(|a| a.1).unwrap_or(Vec3::Y);

        match kind {
            BrushKind::Move | BrushKind::Drag => {
                let Some(now) = self.cursor_on_view_plane(grab, cursor) else {
                    return;
                };
                let drag = now - grab;
                if drag.length_squared() < 1e-12 {
                    return;
                }
                let input = self.stroke_input(grab, normal, drag, pressure);
                self.sculptor.stroke(&input);
                // Move keeps stretching from where it grabbed; Drag lets the
                // bulge travel with the cursor.
                self.stroke.grab = Some(if kind == BrushKind::Drag { now } else { grab + drag });
            }
            BrushKind::Twist => {
                let a = self.stroke.prev_screen - self.stroke.screen_origin;
                let b = cursor - self.stroke.screen_origin;
                // Too close to the pivot and the angle is all noise.
                if a.length() < 12.0 || b.length() < 12.0 {
                    return;
                }
                let angle = sculpt_core::brush::signed_angle_2d(a, b);
                if angle.abs() < 1e-5 {
                    return;
                }
                let mut input = self.stroke_input(grab, normal, Vec3::ZERO, pressure);
                input.twist = angle;
                self.sculptor.stroke(&input);
            }
            BrushKind::Scale => {
                let delta = (cursor.x - self.stroke.prev_screen.x) * 0.01;
                if delta.abs() < 1e-5 {
                    return;
                }
                let mut input = self.stroke_input(grab, normal, Vec3::ZERO, pressure);
                input.pinch = delta;
                self.sculptor.stroke(&input);
            }
            _ => {}
        }
    }

    fn end_stroke(&mut self) {
        if !self.stroke.active {
            return;
        }
        self.sculptor.end_stroke();
        self.sculptor.brush.negative = false;
        self.stroke = Stroke { restore_kind: self.stroke.restore_kind, ..Default::default() };
    }

    /// Holding shift swaps to Smooth for as long as it is down.
    fn sync_modifier_tool(&mut self) {
        if self.stroke.active {
            return;
        }
        match (self.input.shift, self.stroke.restore_kind) {
            (true, None) if self.sculptor.brush.kind != BrushKind::Smooth => {
                self.stroke.restore_kind = Some(self.sculptor.brush.kind);
                self.sculptor.set_brush_kind(BrushKind::Smooth);
            }
            (false, Some(previous)) => {
                self.stroke.restore_kind = None;
                self.sculptor.set_brush_kind(previous);
            }
            _ => {}
        }
    }

    // ---- camera -------------------------------------------------------------

    fn frame_view(&mut self) {
        let (center, radius) = renderer::scene_bounds(&self.sculptor.scene);
        self.camera.frame(center, radius);
    }

    fn apply_gesture(&mut self, g: Gesture) {
        let (_, h) = self.viewport();
        match g {
            Gesture::Orbit(d) => self.camera.orbit(d.x, d.y),
            Gesture::Pan(d) => self.camera.pan(d.x, d.y, h),
            Gesture::Zoom(f) => self.camera.zoom_by(f),
            Gesture::Wheel(amount) => self.camera.zoom(amount),
        }
    }

    // ---- actions ------------------------------------------------------------

    fn build_primitive(p: Primitive) -> Mesh {
        match p {
            Primitive::Sphere => primitives::icosphere(5),
            Primitive::UvSphere => primitives::uv_sphere(64, 32),
            Primitive::Cube => primitives::cube(24),
            Primitive::Cylinder => primitives::cylinder(48, 24),
            Primitive::Torus => primitives::torus(64, 32, 0.35),
            Primitive::Plane => primitives::plane(48),
        }
    }

    fn handle_actions(&mut self, actions: Vec<Action>) {
        for a in actions {
            match a {
                Action::New(p) => {
                    self.sculptor.replace_mesh(Self::build_primitive(p));
                    self.frame_view();
                    self.full_resync = true;
                    self.ui.say(format!("new {}", p.label().to_lowercase()));
                }
                Action::AddObject(p) => {
                    self.sculptor
                        .add_object(&p.label().to_lowercase(), Self::build_primitive(p));
                    self.full_resync = true;
                    self.ui.say(format!("added a {}", p.label().to_lowercase()));
                }
                Action::Import => self.import(),
                Action::Export => self.export(),
                Action::OpenScene => self.open_scene(),
                Action::SaveScene => self.save_scene(),
                Action::Undo => {
                    self.sculptor.undo();
                    self.full_resync = true;
                }
                Action::Redo => {
                    self.sculptor.redo();
                    self.full_resync = true;
                }
                Action::ClearMask => self.sculptor.clear_mask(),
                Action::InvertMask => self.sculptor.invert_mask(),
                Action::FilterMask(amount) => self.sculptor.filter_mask(amount),
                Action::ExtractMask => {
                    let t = self.ui.extract_thickness;
                    if self.sculptor.extract_masked(t) {
                        self.full_resync = true;
                        self.ui.say("extracted the masked region");
                    } else {
                        self.ui.say("nothing is masked");
                    }
                }
                Action::FrameView => self.frame_view(),
                Action::MatcapChanged => {
                    self.renderer
                        .set_matcap(&self.device, &self.queue, self.ui.matcap);
                }
                Action::SampleCountChanged(n) => {
                    let n = n.min(self.max_samples).max(1);
                    self.ui.sample_count = n;
                    self.renderer.set_sample_count(&self.device, n);
                }
                Action::Subdivide(smooth) => {
                    let before = self.sculptor.mesh().face_count();
                    match self.sculptor.subdivide(smooth) {
                        Ok(after) => {
                            self.full_resync = true;
                            self.ui.say(format!("{before} to {after} triangles"));
                        }
                        Err(why) => self.ui.say(why),
                    }
                }
                Action::Decimate => {
                    let ratio = self.ui.decimate_ratio;
                    let before = self.sculptor.mesh().face_count();
                    self.sculptor.decimate(ratio);
                    let after = self.sculptor.mesh().face_count();
                    self.full_resync = true;
                    self.ui.say(format!("{before} to {after} triangles"));
                }
                Action::Remesh => {
                    let opts = self.ui.remesh;
                    self.sculptor.voxel_remesh(&opts);
                    self.full_resync = true;
                    let n = self.sculptor.mesh().face_count();
                    self.ui.say(format!("remeshed to {n} triangles"));
                }
                Action::CloseHoles => {
                    let n = self.sculptor.close_holes();
                    self.full_resync = true;
                    self.ui.say(match n {
                        0 => "no holes to close".to_string(),
                        1 => "closed one hole".to_string(),
                        n => format!("closed {n} holes"),
                    });
                }
                Action::SmoothAll => {
                    self.sculptor.smooth_all(0.5);
                    self.dirty_object = Some(self.sculptor.scene.active);
                }
                Action::Mirror(axis) => {
                    self.sculptor.mirror(axis);
                    self.full_resync = true;
                }
                Action::Symmetrize(axis, keep_positive) => {
                    self.sculptor.symmetrize(axis, keep_positive);
                    self.full_resync = true;
                    self.ui.say("symmetrized");
                }
                Action::SelectObject(i) => {
                    self.sculptor.select(i);
                    self.full_resync = true;
                }
                Action::DeleteObject(i) => {
                    self.sculptor.delete_object(i);
                    self.full_resync = true;
                }
                Action::DuplicateObject(i) => {
                    self.sculptor.duplicate_object(i);
                    self.full_resync = true;
                }
                Action::ToggleVisible(i) => {
                    if let Some(o) = self.sculptor.scene.objects.get_mut(i) {
                        o.visible = !o.visible;
                    }
                }
                Action::MergeVisible => {
                    self.sculptor.merge_visible();
                    self.full_resync = true;
                    self.ui.say("merged");
                }
                Action::ApplyTransform => {
                    self.sculptor.apply_transform();
                    self.full_resync = true;
                }
                Action::SetView(p) => self.camera.set_preset(p),
                Action::PickColorMode => self.ui.picking_color = !self.ui.picking_color,
                Action::ResetBrushes => {
                    self.sculptor.reset_brush_presets();
                    self.ui.say("brushes reset");
                }
                Action::ResetTheme => {
                    self.ui.theme = theme::UiTheme::default();
                    self.ui.say("theme reset");
                }
            }
        }
    }

    fn import(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Meshes and scenes", io::IMPORT_EXTENSIONS)
            .pick_file()
        else {
            return;
        };
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        if path.extension().and_then(|e| e.to_str()) == Some("sculpt") {
            match io::read_scene(&path) {
                Ok(scene) => {
                    self.sculptor = Sculptor::with_scene(scene);
                    self.frame_view();
                    self.full_resync = true;
                    self.ui.say(format!("opened {name}"));
                }
                Err(e) => self.ui.say(format!("could not open: {e}")),
            }
            return;
        }

        match io::load(&path) {
            Ok(mut m) => {
                m.normalize_scale();
                m.recompute_normals();
                self.sculptor.replace_mesh(m);
                self.frame_view();
                self.full_resync = true;
                self.ui.say(format!("loaded {name}"));
            }
            Err(e) => self.ui.say(format!("import failed: {e}")),
        }
    }

    fn export(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Wavefront OBJ", &["obj"])
            .add_filter("Stanford PLY", &["ply"])
            .add_filter("Binary STL", &["stl"])
            .set_file_name("sculpt.obj")
            .save_file()
        else {
            return;
        };
        // Export what the viewer sees, transform included.
        let mut baked = self
            .sculptor
            .scene
            .active()
            .cloned()
            .unwrap_or_else(|| Object::new("mesh", Mesh::new()));
        baked.apply_transform();
        match io::save(&baked.mesh, &path) {
            Ok(()) => self.ui.say(format!(
                "saved {}",
                path.file_name().unwrap_or_default().to_string_lossy()
            )),
            Err(e) => self.ui.say(format!("export failed: {e}")),
        }
    }

    fn open_scene(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("sculpt-rs scene", &["sculpt"])
            .pick_file()
        else {
            return;
        };
        match io::read_scene(&path) {
            Ok(scene) => {
                self.sculptor = Sculptor::with_scene(scene);
                self.frame_view();
                self.full_resync = true;
                self.ui.say("scene opened");
            }
            Err(e) => self.ui.say(format!("could not open: {e}")),
        }
    }

    fn save_scene(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("sculpt-rs scene", &["sculpt"])
            .set_file_name("scene.sculpt")
            .save_file()
        else {
            return;
        };
        match io::write_scene(&self.sculptor.scene, &path) {
            Ok(()) => self.ui.say("scene saved"),
            Err(e) => self.ui.say(format!("save failed: {e}")),
        }
    }

    // ---- keyboard -----------------------------------------------------------

    fn on_key(&mut self, code: KeyCode) {
        let ctrl = self.input.ctrl;
        let shift = self.input.shift;
        let mut actions = Vec::new();
        match code {
            KeyCode::KeyZ if ctrl && shift => actions.push(Action::Redo),
            KeyCode::KeyZ if ctrl => actions.push(Action::Undo),
            KeyCode::KeyY if ctrl => actions.push(Action::Redo),
            KeyCode::KeyX => self.sculptor.symmetry = !self.sculptor.symmetry,
            KeyCode::KeyW if self.ui.wireframe_available => {
                self.ui.settings.wireframe = !self.ui.settings.wireframe
            }
            KeyCode::KeyG => self.ui.settings.grid = !self.ui.settings.grid,
            KeyCode::KeyF => actions.push(Action::FrameView),
            KeyCode::KeyH => self.ui.show_help = !self.ui.show_help,
            KeyCode::KeyT => self.gizmo.mode = ui::next_gizmo_mode(self.gizmo.mode),
            KeyCode::Tab => self.ui.show_panel = !self.ui.show_panel,
            KeyCode::BracketLeft => {
                self.sculptor.brush.radius = (self.sculptor.brush.radius * 0.85).max(0.003)
            }
            KeyCode::BracketRight => {
                self.sculptor.brush.radius = (self.sculptor.brush.radius * 1.18).min(1.5)
            }
            KeyCode::Minus => {
                self.sculptor.brush.strength = (self.sculptor.brush.strength - 0.05).max(0.0)
            }
            KeyCode::Equal => {
                self.sculptor.brush.strength = (self.sculptor.brush.strength + 0.05).min(1.0)
            }
            KeyCode::Numpad1 => actions.push(Action::SetView(ViewPreset::Front)),
            KeyCode::Numpad3 => actions.push(Action::SetView(ViewPreset::Right)),
            KeyCode::Numpad7 => actions.push(Action::SetView(ViewPreset::Top)),
            KeyCode::Numpad5 => {
                self.camera.projection = match self.camera.projection {
                    Projection::Perspective => Projection::Orthographic,
                    Projection::Orthographic => Projection::Perspective,
                }
            }
            _ => {
                if let Some(i) = tool_index(code, shift) {
                    if let Some(&kind) = BrushKind::ALL.get(i) {
                        self.sculptor.set_brush_kind(kind);
                        // A tool picked by hand outranks the shift-to-smooth
                        // swap, so drop the pending restore.
                        self.stroke.restore_kind = None;
                        self.ui.say(kind.label());
                    }
                }
            }
        }
        self.handle_actions(actions);
    }

    // ---- frame --------------------------------------------------------------

    fn render(&mut self) {
        let now = Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f32();
        self.last_frame = now;
        if dt > 0.0 {
            // Exponential moving average, otherwise the readout is unreadable.
            self.ui.fps = self.ui.fps * 0.9 + (1.0 / dt) * 0.1;
        }
        self.ui.status_age += dt;
        self.camera.update(dt);

        theme::apply(&self.egui_ctx, &self.ui.theme);
        widgets::set_icon_scale(&self.egui_ctx, self.ui.theme.icon_scale);

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

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });

        // Upload whatever the sculptor touched. The mesh records which slots it
        // wrote, so a stroke sends a few kilobytes instead of the whole model;
        // a scene change, or a mesh that outgrew its buffers, still goes in
        // full.
        let (mut dirty_verts, mut dirty_faces, fully_dirty) = self.sculptor.take_dirty();
        let object = self.sculptor.scene.active;
        let changed = self.sculptor.verts_dirty || self.sculptor.topology_dirty;
        let sparse_ok = self.ui.settings.gpu_scatter
            && !self.full_resync
            && !fully_dirty
            && changed
            && self.dirty_object == Some(object);

        let mut done = false;
        if sparse_ok {
            done = self.renderer.sync_sparse(
                &self.device,
                &self.queue,
                &mut encoder,
                &self.sculptor.scene,
                object,
                &mut dirty_verts,
                &mut dirty_faces,
            );
        }
        if !done && (self.full_resync || changed) {
            let target = if self.full_resync { None } else { self.dirty_object };
            self.renderer
                .sync(&self.device, &self.queue, &self.sculptor.scene, target);
        }
        self.ui.upload_bytes = self.renderer.last_upload_bytes;
        self.sculptor.verts_dirty = false;
        self.sculptor.topology_dirty = false;
        self.full_resync = false;
        self.dirty_object = None;

        self.renderer.set_uniforms(
            &self.queue,
            &self.sculptor.scene,
            self.camera.view(),
            self.camera.proj(self.aspect()),
            self.camera.eye(),
            &self.ui.settings,
        );

        let overlay = self.build_overlay();
        let raw_input = self.egui_state.take_egui_input(&self.window);
        let ctx = self.egui_ctx.clone();
        let mut actions = Vec::new();
        // The gizmo draws after the interface, from a snapshot of the camera:
        // the panels hold the live one mutably, and a frame of lag on a set of
        // handles is invisible.
        let camera_snapshot = self.camera.clone();
        let viewport = self.viewport();
        let full_output = {
            let sculptor = &mut self.sculptor;
            let ui_state = &mut self.ui;
            let cam = &mut self.camera;
            let giz = &mut self.gizmo;
            ctx.run_ui(raw_input, |root| {
                actions = ui::draw(root, sculptor, ui_state, cam, giz, &overlay);

                if let Some(t) = sculptor.scene.active().map(|o| o.transform) {
                    let view = gizmo::View {
                        camera: &camera_snapshot,
                        size: viewport,
                        pixels_per_point: root.ctx().pixels_per_point(),
                    };
                    let painter = root.ctx().layer_painter(egui::LayerId::new(
                        egui::Order::Foreground,
                        egui::Id::new("gizmo"),
                    ));
                    giz.draw(root, &painter, &view, &t, &theme::Palette::of(root.ctx()));
                }
            })
        };
        self.egui_state
            .handle_platform_output(&self.window, full_output.platform_output);
        let paint_jobs = ctx.tessellate(full_output.shapes, full_output.pixels_per_point);
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [self.config.width, self.config.height],
            pixels_per_point: full_output.pixels_per_point,
        };

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
            // With multisampling the scene renders into the MSAA target and
            // resolves into the swapchain image; egui then draws on top of the
            // resolved result in its own single-sampled pass.
            let (target, resolve) = match self.renderer.msaa_view() {
                Some(msaa) => (msaa, Some(&view)),
                None => (&view, None),
            };
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scene pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: resolve,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.02,
                            g: 0.02,
                            b: 0.025,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: self.renderer.depth_view(),
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
            self.renderer
                .draw(&mut pass, &self.sculptor.scene, &self.ui.settings);
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

    /// Brush cursor and other viewport decorations for this frame.
    fn build_overlay(&self) -> Overlay {
        let ppp = self.egui_ctx.pixels_per_point();
        let over_ui = self.egui_ctx.egui_wants_pointer_input();
        // A handle under the cursor means the next click moves the object, so
        // showing a brush ring there would be a lie.
        let over_gizmo = self
            .sculptor
            .scene
            .active()
            .map(|o| o.transform)
            .and_then(|t| {
                self.gizmo
                    .handle_at(self.cursor_points(), &self.gizmo_view(), &t)
            })
            .is_some()
            || self.gizmo.is_dragging();
        let hit = (!over_ui && !over_gizmo && !self.input.touch_active())
            .then(|| self.pick_for_brush(self.input.cursor))
            .flatten();

        let cursor = hit.as_ref().map(|h| {
            let (_, height) = self.viewport();
            let r = self
                .camera
                .world_radius_to_pixels(h.world.point, self.sculptor.brush.radius, height);
            (
                egui::pos2(self.input.cursor.x / ppp, self.input.cursor.y / ppp),
                r / ppp,
            )
        });
        let tilt = hit.as_ref().map(|h| {
            let v = self.camera.view();
            let n = v.transform_vector3(h.world.normal).normalize_or(Vec3::Z);
            (n.x, -n.y)
        });

        // When the pick reached past the cursor to find the surface, say so.
        let cursor_point = egui::pos2(self.input.cursor.x / ppp, self.input.cursor.y / ppp);
        let anchor = hit.as_ref().and_then(|h| {
            let at = self.project_to_points(h.world.point)?;
            ((at - cursor_point).length() > 6.0).then_some(at)
        });

        Overlay {
            cursor,
            cursor_tilt: tilt,
            anchor,
            stroking: self.stroke.active,
        }
    }

    /// Projects a world point into interface points.
    fn project_to_points(&self, p: Vec3) -> Option<egui::Pos2> {
        let (w, h) = self.viewport();
        let clip = self.camera.view_proj(self.aspect()) * glam::Vec4::new(p.x, p.y, p.z, 1.0);
        if clip.w <= 1e-6 {
            return None;
        }
        let ndc = clip.truncate() / clip.w;
        let ppp = self.egui_ctx.pixels_per_point().max(1e-3);
        Some(egui::pos2(
            (ndc.x * 0.5 + 0.5) * w / ppp,
            (0.5 - ndc.y * 0.5) * h / ppp,
        ))
    }
}

/// Maps a number key to a tool index; shift reaches the last three.
fn tool_index(code: KeyCode, shift: bool) -> Option<usize> {
    let base = match code {
        KeyCode::Digit1 => 0,
        KeyCode::Digit2 => 1,
        KeyCode::Digit3 => 2,
        KeyCode::Digit4 => 3,
        KeyCode::Digit5 => 4,
        KeyCode::Digit6 => 5,
        KeyCode::Digit7 => 6,
        KeyCode::Digit8 => 7,
        KeyCode::Digit9 => 8,
        KeyCode::Digit0 => 9,
        _ => return None,
    };
    // Shift reaches the tools past the tenth.
    Some(if shift && base < 6 { base + 10 } else { base })
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
            .with_inner_size(winit::dpi::LogicalSize::new(1600.0, 980.0));
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
                st.sync_modifier_tool();
            }
            WindowEvent::CursorMoved { position, .. } => {
                let delta = st.input.move_cursor(position.x as f32, position.y as f32);
                if let Some(g) = st.input.mouse_navigation(delta) {
                    st.apply_gesture(g);
                } else if st.gizmo.is_dragging() {
                    st.gizmo_drag();
                } else if st.stroke.active {
                    st.continue_stroke(1.0);
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                if !egui_captured {
                    let amount = match delta {
                        MouseScrollDelta::LineDelta(_, y) => y,
                        MouseScrollDelta::PixelDelta(p) => p.y as f32 / 60.0,
                    };
                    st.apply_gesture(Gesture::Wheel(amount));
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let down = state == ElementState::Pressed;
                match button {
                    MouseButton::Left => {
                        st.input.lmb = down;
                        if down {
                            // The radial menu owns the pointer while it is up,
                            // then the gizmo gets first refusal, then the brush.
                            if !st.ui.wheel.open
                                && !egui_captured
                                && !st.input.alt
                                && !st.gizmo_press()
                            {
                                st.begin_stroke(1.0);
                            }
                        } else {
                            st.gizmo.release();
                            st.end_stroke();
                        }
                    }
                    MouseButton::Middle => st.input.mmb = down,
                    MouseButton::Right => st.input.rmb = down,
                    _ => {}
                }
            }
            WindowEvent::Touch(touch) => {
                if let Some(outcome) = st.input.on_touch(&touch, egui_captured) {
                    match outcome {
                        TouchOutcome::StrokeStart { at, pressure } => {
                            st.input.cursor = at;
                            st.begin_stroke(pressure);
                        }
                        TouchOutcome::StrokeMove { at, pressure } => {
                            st.input.cursor = at;
                            st.continue_stroke(pressure);
                        }
                        TouchOutcome::StrokeEnd => st.end_stroke(),
                        TouchOutcome::Navigate(gestures) => {
                            // A second finger cancels the stroke it interrupted.
                            st.end_stroke();
                            for g in gestures {
                                st.apply_gesture(g);
                            }
                        }
                    }
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if st.egui_ctx.egui_wants_keyboard_input() {
                    return;
                }
                let PhysicalKey::Code(code) = event.physical_key else {
                    return;
                };
                let pressed = event.state == ElementState::Pressed;

                // Rebinding swallows the next key, whatever it is.
                if st.ui.rebinding_wheel && pressed {
                    st.ui.wheel_key = code;
                    st.ui.rebinding_wheel = false;
                    st.ui.say(format!("radial menu on {}", ui::key_name(code)));
                    return;
                }

                // The radial menu lives for exactly as long as its key is held.
                if code == st.ui.wheel_key {
                    if pressed {
                        if !st.ui.wheel.open && !event.repeat {
                            let at = st.cursor_points();
                            st.ui.wheel.open_at(at, &st.sculptor);
                            // A stroke and a menu at the same time helps nobody.
                            st.end_stroke();
                        }
                    } else {
                        st.ui.wheel.close();
                    }
                    return;
                }

                if pressed && !st.ui.wheel.open {
                    st.on_key(code);
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
