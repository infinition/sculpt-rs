//! Orbit camera and screen-to-world ray casting.

use glam::{Mat4, Vec3, Vec4Swizzles};

pub struct Camera {
    pub target: Vec3,
    pub distance: f32,
    pub yaw: f32,
    pub pitch: f32,
    pub fov_y: f32,
    pub znear: f32,
    pub zfar: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            target: Vec3::ZERO,
            distance: 3.5,
            yaw: 0.6,
            pitch: 0.35,
            fov_y: 45f32.to_radians(),
            znear: 0.01,
            zfar: 100.0,
        }
    }
}

impl Camera {
    pub fn eye(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        self.target + Vec3::new(cp * sy, sp, cp * cy) * self.distance
    }

    pub fn view(&self) -> Mat4 {
        glam::camera::rh::view::look_at_mat4(self.eye(), self.target, Vec3::Y)
    }

    pub fn proj(&self, aspect: f32) -> Mat4 {
        // The "directx" variant maps z to 0..1, which is what wgpu expects.
        glam::camera::rh::proj::directx::perspective(
            self.fov_y,
            aspect.max(1e-4),
            self.znear,
            self.zfar,
        )
    }

    pub fn view_proj(&self, aspect: f32) -> Mat4 {
        self.proj(aspect) * self.view()
    }

    pub fn orbit(&mut self, dx: f32, dy: f32) {
        const LIMIT: f32 = std::f32::consts::FRAC_PI_2 - 0.01;
        self.yaw -= dx * 0.008;
        self.pitch = (self.pitch + dy * 0.008).clamp(-LIMIT, LIMIT);
    }

    pub fn pan(&mut self, dx: f32, dy: f32, viewport_h: f32) {
        // Scale so a pixel of drag moves the same amount of surface regardless
        // of zoom level.
        let world_per_px = 2.0 * self.distance * (self.fov_y * 0.5).tan() / viewport_h.max(1.0);
        let v = self.view();
        let right = Vec3::new(v.x_axis.x, v.y_axis.x, v.z_axis.x);
        let up = Vec3::new(v.x_axis.y, v.y_axis.y, v.z_axis.y);
        self.target += (-right * dx + up * dy) * world_per_px;
    }

    pub fn zoom(&mut self, scroll: f32) {
        self.distance = (self.distance * (1.0 - scroll * 0.12)).clamp(0.05, 60.0);
    }

    pub fn frame(&mut self, center: Vec3, radius: f32) {
        self.target = center;
        self.distance = (radius / (self.fov_y * 0.5).sin()).max(0.2) * 1.15;
    }

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

    /// Pixel radius of a world-space sphere at `world_pos`, for drawing the
    /// brush cursor in 2D.
    pub fn world_radius_to_pixels(&self, world_pos: Vec3, r: f32, h: f32) -> f32 {
        let d = (world_pos - self.eye()).length().max(1e-4);
        let half = (self.fov_y * 0.5).tan() * d;
        r / half * (h * 0.5)
    }
}
