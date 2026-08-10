//! Orbit camera, screen-to-world rays and view presets.
//!
//! Every navigation action writes to a target, and the live values chase it
//! with a critically damped step. Nothing snaps, which is what makes fast
//! orbiting readable, and it costs one lerp per frame.

use glam::{Mat4, Vec3, Vec4Swizzles};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Projection {
    Perspective,
    Orthographic,
}

/// Named viewpoints, matching the numeric keypad convention.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ViewPreset {
    Front,
    Back,
    Left,
    Right,
    Top,
    Bottom,
}

impl ViewPreset {
    pub const ALL: [ViewPreset; 6] = [
        ViewPreset::Front,
        ViewPreset::Back,
        ViewPreset::Left,
        ViewPreset::Right,
        ViewPreset::Top,
        ViewPreset::Bottom,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ViewPreset::Front => "Front",
            ViewPreset::Back => "Back",
            ViewPreset::Left => "Left",
            ViewPreset::Right => "Right",
            ViewPreset::Top => "Top",
            ViewPreset::Bottom => "Bottom",
        }
    }

    /// Yaw and pitch, in radians.
    fn angles(self) -> (f32, f32) {
        use std::f32::consts::{FRAC_PI_2, PI};
        match self {
            ViewPreset::Front => (0.0, 0.0),
            ViewPreset::Back => (PI, 0.0),
            ViewPreset::Left => (-FRAC_PI_2, 0.0),
            ViewPreset::Right => (FRAC_PI_2, 0.0),
            ViewPreset::Top => (0.0, FRAC_PI_2 - 0.001),
            ViewPreset::Bottom => (0.0, -FRAC_PI_2 + 0.001),
        }
    }
}

#[derive(Clone, Copy)]
struct Pose {
    target: Vec3,
    distance: f32,
    yaw: f32,
    pitch: f32,
}

#[derive(Clone)]
pub struct Camera {
    live: Pose,
    goal: Pose,
    pub fov_y: f32,
    pub znear: f32,
    pub zfar: f32,
    pub projection: Projection,
    /// 0 disables smoothing, 1 is very soft.
    pub smoothing: f32,
    /// Radians of orbit per pixel of drag.
    pub orbit_speed: f32,
    pub invert_orbit_y: bool,
    /// Holds the viewing angle still. Panning and zooming keep working, and so
    /// do explicit commands like snapping to a named view.
    ///
    /// The lock is there for the moment you have found the angle you want and
    /// are painting or sculpting on it, where every stray drag costs you the
    /// view. In that moment you still need to slide the model across and get
    /// closer to it, so only the rotation is held.
    pub locked: bool,
}

impl Default for Camera {
    fn default() -> Self {
        let pose = Pose { target: Vec3::ZERO, distance: 3.5, yaw: 0.6, pitch: 0.35 };
        Self {
            live: pose,
            goal: pose,
            fov_y: 45f32.to_radians(),
            znear: 0.01,
            zfar: 100.0,
            projection: Projection::Perspective,
            smoothing: 0.55,
            orbit_speed: 0.008,
            invert_orbit_y: false,
            locked: false,
        }
    }
}

impl Camera {
    /// The point the camera orbits.
    pub fn target(&self) -> Vec3 {
        self.live.target
    }

    pub fn eye(&self) -> Vec3 {
        let (sy, cy) = self.live.yaw.sin_cos();
        let (sp, cp) = self.live.pitch.sin_cos();
        self.live.target + Vec3::new(cp * sy, sp, cp * cy) * self.live.distance
    }

    /// Camera forward, pointing into the scene.
    pub fn forward(&self) -> Vec3 {
        (self.live.target - self.eye()).normalize_or(-Vec3::Z)
    }

    /// Screen right, in world space. A brush alpha is printed along it, so the
    /// stamp keeps the orientation it has on screen.
    pub fn right(&self) -> Vec3 {
        let v = self.view();
        Vec3::new(v.x_axis.x, v.y_axis.x, v.z_axis.x).normalize_or(Vec3::X)
    }

    pub fn view(&self) -> Mat4 {
        glam::camera::rh::view::look_at_mat4(self.eye(), self.live.target, Vec3::Y)
    }

    /// Half-height of the view volume at the pivot distance.
    fn ortho_half_height(&self) -> f32 {
        (self.fov_y * 0.5).tan() * self.live.distance
    }

    pub fn proj(&self, aspect: f32) -> Mat4 {
        let aspect = aspect.max(1e-4);
        // The "directx" variants map z to 0..1, which is what wgpu expects.
        match self.projection {
            Projection::Perspective => glam::camera::rh::proj::directx::perspective(
                self.fov_y,
                aspect,
                self.znear,
                self.zfar,
            ),
            Projection::Orthographic => {
                let h = self.ortho_half_height();
                let w = h * aspect;
                // Pull the near plane behind the pivot so nothing in front of it
                // gets clipped when the model is bigger than the orbit radius.
                glam::camera::rh::proj::directx::orthographic(
                    -w,
                    w,
                    -h,
                    h,
                    -self.zfar,
                    self.zfar,
                )
            }
        }
    }

    pub fn view_proj(&self, aspect: f32) -> Mat4 {
        self.proj(aspect) * self.view()
    }

    // ---- navigation ---------------------------------------------------------

    pub fn orbit(&mut self, dx: f32, dy: f32) {
        if self.locked {
            return;
        }
        const LIMIT: f32 = std::f32::consts::FRAC_PI_2 - 0.01;
        let sign = if self.invert_orbit_y { -1.0 } else { 1.0 };
        self.goal.yaw -= dx * self.orbit_speed;
        self.goal.pitch = (self.goal.pitch + dy * self.orbit_speed * sign).clamp(-LIMIT, LIMIT);
    }

    pub fn pan(&mut self, dx: f32, dy: f32, viewport_h: f32) {
        // Scale so a pixel of drag moves the same amount of surface regardless
        // of zoom level.
        let half = match self.projection {
            Projection::Perspective => (self.fov_y * 0.5).tan() * self.goal.distance,
            Projection::Orthographic => self.ortho_half_height(),
        };
        let world_per_px = 2.0 * half / viewport_h.max(1.0);
        let v = self.view();
        let right = Vec3::new(v.x_axis.x, v.y_axis.x, v.z_axis.x);
        let up = Vec3::new(v.x_axis.y, v.y_axis.y, v.z_axis.y);
        self.goal.target += (-right * dx + up * dy) * world_per_px;
    }

    pub fn zoom(&mut self, scroll: f32) {
        self.goal.distance = (self.goal.distance * (1.0 - scroll * 0.12)).clamp(0.02, 200.0);
    }

    /// Multiplicative zoom, for pinch gestures.
    pub fn zoom_by(&mut self, factor: f32) {
        self.goal.distance = (self.goal.distance / factor.max(1e-3)).clamp(0.02, 200.0);
    }

    pub fn frame(&mut self, center: Vec3, radius: f32) {
        self.goal.target = center;
        self.goal.distance = (radius / (self.fov_y * 0.5).sin()).max(0.1) * 1.15;
    }

    pub fn set_preset(&mut self, preset: ViewPreset) {
        let (yaw, pitch) = preset.angles();
        // Take the shortest way round rather than unwinding the long way.
        let mut delta = yaw - self.goal.yaw;
        while delta > std::f32::consts::PI {
            delta -= std::f32::consts::TAU;
        }
        while delta < -std::f32::consts::PI {
            delta += std::f32::consts::TAU;
        }
        self.goal.yaw += delta;
        self.goal.pitch = pitch;
    }

    /// Snaps the live pose to its goal, skipping the animation.
    pub fn settle(&mut self) {
        self.live = self.goal;
    }

    /// Advances the smoothing. `dt` is in seconds.
    pub fn update(&mut self, dt: f32) {
        if self.smoothing <= 0.001 {
            self.live = self.goal;
            return;
        }
        // Frame-rate independent exponential approach.
        let rate = 30.0 * (1.05 - self.smoothing.clamp(0.0, 1.0));
        let k = 1.0 - (-rate * dt.clamp(0.0, 0.1)).exp();
        self.live.target += (self.goal.target - self.live.target) * k;
        self.live.distance += (self.goal.distance - self.live.distance) * k;
        self.live.yaw += (self.goal.yaw - self.live.yaw) * k;
        self.live.pitch += (self.goal.pitch - self.live.pitch) * k;
    }

    // ---- projection helpers -------------------------------------------------

    /// Builds a world-space ray through a physical pixel.
    pub fn ray(&self, px: f32, py: f32, w: f32, h: f32) -> (Vec3, Vec3) {
        let ndc_x = 2.0 * px / w.max(1.0) - 1.0;
        let ndc_y = 1.0 - 2.0 * py / h.max(1.0);
        let inv = self.view_proj(w / h.max(1.0)).inverse();
        let near = inv * glam::Vec4::new(ndc_x, ndc_y, 0.0, 1.0);
        let far = inv * glam::Vec4::new(ndc_x, ndc_y, 1.0, 1.0);
        let p0 = near.xyz() / near.w;
        let p1 = far.xyz() / far.w;
        (p0, (p1 - p0).normalize_or(-Vec3::Z))
    }

    /// How many pixels one world unit covers at `world_pos`.
    pub fn pixels_per_world(&self, world_pos: Vec3, h: f32) -> f32 {
        self.world_radius_to_pixels(world_pos, 1.0, h)
    }

    /// Pixel radius of a world-space sphere at `world_pos`, for drawing the
    /// brush cursor in 2D.
    pub fn world_radius_to_pixels(&self, world_pos: Vec3, r: f32, h: f32) -> f32 {
        let half = match self.projection {
            Projection::Perspective => {
                let d = (world_pos - self.eye()).length().max(1e-4);
                (self.fov_y * 0.5).tan() * d
            }
            Projection::Orthographic => self.ortho_half_height(),
        };
        r / half.max(1e-6) * (h * 0.5)
    }
}
