//! Brush alphas: a grey image that shapes the dab.
//!
//! A falloff decides how a dab fades from its centre to its rim, and that is
//! all it can do: every dab it makes is a circle. An alpha multiplies the
//! falloff by an image sampled across the dab, so the same brush can lay down
//! cracks, scales, a hatch or a photographed grain. It is the difference
//! between sculpting and stamping.
//!
//! The image is stored as plain 0..1 greys. Loading files is the application's
//! problem; this module only ever sees the decoded result, which keeps the
//! core free of image formats.

/// A grey image, sampled across the face of a dab.
#[derive(Clone, Debug)]
pub struct Alpha {
    pub name: String,
    pub w: u32,
    pub h: u32,
    /// Row-major, top row first, one value per pixel in 0..1.
    pub data: Vec<f32>,
}

/// Alphas that can be built without a file, so the brush is useful the moment
/// the application starts.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Shape {
    Ring,
    Cracks,
    Scales,
    Hatch,
    Noise,
    Chisel,
}

impl Shape {
    pub const ALL: [Shape; 6] = [
        Shape::Ring,
        Shape::Cracks,
        Shape::Scales,
        Shape::Hatch,
        Shape::Noise,
        Shape::Chisel,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Shape::Ring => "Ring",
            Shape::Cracks => "Cracks",
            Shape::Scales => "Scales",
            Shape::Hatch => "Hatch",
            Shape::Noise => "Noise",
            Shape::Chisel => "Chisel",
        }
    }
}

impl Alpha {
    /// Wraps decoded greys. Anything outside 0..1 is clamped, since a stamp
    /// that amplifies the brush past its own strength is never what was meant.
    pub fn new(name: impl Into<String>, w: u32, h: u32, data: Vec<f32>) -> Self {
        debug_assert_eq!(data.len(), (w as usize) * (h as usize));
        Self {
            name: name.into(),
            w: w.max(1),
            h: h.max(1),
            data: data.into_iter().map(|v| v.clamp(0.0, 1.0)).collect(),
        }
    }

    /// Builds one of the shapes that come with the application.
    pub fn procedural(shape: Shape, size: u32) -> Self {
        let size = size.clamp(8, 1024);
        let n = size as usize;
        let mut data = vec![0.0f32; n * n];
        for y in 0..n {
            for x in 0..n {
                // Centred coordinates in -1..1, so every shape can be written
                // in terms of the distance from the middle of the dab.
                let u = (x as f32 + 0.5) / n as f32 * 2.0 - 1.0;
                let v = (y as f32 + 0.5) / n as f32 * 2.0 - 1.0;
                let r = (u * u + v * v).sqrt();
                // Everything fades out before the rim: a stamp with a hard edge
                // leaves a visible disc no matter what is drawn inside it, and
                // stopping just inside the image means the border is empty all
                // the way round rather than only at the corners.
                let vignette = (1.0 - r / 0.95).clamp(0.0, 1.0).powf(0.6);
                let value = match shape {
                    Shape::Ring => {
                        // A raised band, hollow in the middle.
                        let d = (r - 0.62).abs() / 0.28;
                        (1.0 - d * d).clamp(0.0, 1.0)
                    }
                    Shape::Cracks => {
                        // Veins where two cells of a noise field meet.
                        let f = worley(u * 3.2, v * 3.2);
                        (1.0 - (f * 7.0).clamp(0.0, 1.0)).powf(1.6)
                    }
                    Shape::Scales => {
                        // Overlapping arcs, offset row by row.
                        let row = (v * 4.0).floor();
                        let sx = u * 4.0 + if (row as i32) % 2 == 0 { 0.0 } else { 0.5 };
                        let sy = v * 4.0;
                        let d = ((sx - sx.floor() - 0.5).powi(2)
                            + (sy - sy.floor() - 0.5).powi(2))
                        .sqrt();
                        (1.0 - (d / 0.5).clamp(0.0, 1.0)).powf(0.8)
                    }
                    Shape::Hatch => {
                        // Two crossed combs, useful for cloth and stone.
                        let a = (u * 22.0).sin().abs();
                        let b = (v * 22.0).sin().abs();
                        (a.max(b) - 0.4).clamp(0.0, 1.0) / 0.6
                    }
                    Shape::Noise => fbm(u * 3.0, v * 3.0),
                    Shape::Chisel => {
                        // A blade: full along one axis, tight across the other.
                        let across = (v * 3.4).abs();
                        (1.0 - across * across).clamp(0.0, 1.0)
                    }
                };
                data[y * n + x] = (value * vignette).clamp(0.0, 1.0);
            }
        }
        Self::new(shape.label(), size, size, data)
    }

    /// Bilinear sample. `u` and `v` run 0..1 from the top left; anything
    /// outside the image reads as nothing, so a rotated stamp fades out at its
    /// corners rather than smearing the edge pixels across the dab.
    pub fn sample(&self, u: f32, v: f32) -> f32 {
        if !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) {
            return 0.0;
        }
        let x = u * (self.w - 1) as f32;
        let y = v * (self.h - 1) as f32;
        let x0 = x.floor() as u32;
        let y0 = y.floor() as u32;
        let x1 = (x0 + 1).min(self.w - 1);
        let y1 = (y0 + 1).min(self.h - 1);
        let fx = x - x0 as f32;
        let fy = y - y0 as f32;
        let at = |ix: u32, iy: u32| self.data[(iy * self.w + ix) as usize];
        let top = at(x0, y0) * (1.0 - fx) + at(x1, y0) * fx;
        let bottom = at(x0, y1) * (1.0 - fx) + at(x1, y1) * fx;
        top * (1.0 - fy) + bottom * fy
    }
}

// ---------------------------------------------------------------------------
// noise
// ---------------------------------------------------------------------------

fn hash(x: i32, y: i32) -> f32 {
    let mut h = (x as u32).wrapping_mul(374761393) ^ (y as u32).wrapping_mul(668265263);
    h = (h ^ (h >> 13)).wrapping_mul(1274126177);
    ((h ^ (h >> 16)) & 0xffffff) as f32 / 0xffffff as f32
}

/// Value noise with a smooth interpolant.
fn value_noise(x: f32, y: f32) -> f32 {
    let xi = x.floor();
    let yi = y.floor();
    let fx = x - xi;
    let fy = y - yi;
    let sx = fx * fx * (3.0 - 2.0 * fx);
    let sy = fy * fy * (3.0 - 2.0 * fy);
    let (xi, yi) = (xi as i32, yi as i32);
    let top = hash(xi, yi) * (1.0 - sx) + hash(xi + 1, yi) * sx;
    let bottom = hash(xi, yi + 1) * (1.0 - sx) + hash(xi + 1, yi + 1) * sx;
    top * (1.0 - sy) + bottom * sy
}

fn fbm(x: f32, y: f32) -> f32 {
    let mut sum = 0.0;
    let mut amp = 0.5;
    let mut freq = 1.0;
    for _ in 0..4 {
        sum += value_noise(x * freq, y * freq) * amp;
        freq *= 2.0;
        amp *= 0.5;
    }
    sum.clamp(0.0, 1.0)
}

/// Distance to the nearest of a scattered set of points, which is what gives
/// cracks their cell walls.
fn worley(x: f32, y: f32) -> f32 {
    let cx = x.floor() as i32;
    let cy = y.floor() as i32;
    let mut best = f32::MAX;
    for dy in -1..=1 {
        for dx in -1..=1 {
            let (gx, gy) = (cx + dx, cy + dy);
            let px = gx as f32 + hash(gx, gy);
            let py = gy as f32 + hash(gy, gx);
            let d = (px - x).powi(2) + (py - y).powi(2);
            best = best.min(d);
        }
    }
    best.sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_procedural_alpha_has_something_in_it() {
        for shape in Shape::ALL {
            let a = Alpha::procedural(shape, 64);
            let sum: f32 = a.data.iter().sum();
            let peak = a.data.iter().copied().fold(0.0f32, f32::max);
            assert!(sum > 0.0, "{shape:?} is empty");
            assert!(peak > 0.25, "{shape:?} never gets above {peak}");
            assert!(a.data.iter().all(|v| (0.0..=1.0).contains(v)), "{shape:?} out of range");
        }
    }

    /// The rim has to reach zero, or every dab leaves a visible disc.
    #[test]
    fn procedural_alphas_fade_out_at_the_rim() {
        for shape in Shape::ALL {
            let a = Alpha::procedural(shape, 64);
            for i in 0..64 {
                let u = i as f32 / 63.0;
                assert_eq!(a.sample(u, 0.0), 0.0, "{shape:?} top edge");
                assert_eq!(a.sample(0.0, u), 0.0, "{shape:?} left edge");
            }
        }
    }

    #[test]
    fn sampling_outside_the_image_reads_as_nothing() {
        let a = Alpha::new("flat", 2, 2, vec![1.0; 4]);
        assert_eq!(a.sample(0.5, 0.5), 1.0);
        assert_eq!(a.sample(-0.01, 0.5), 0.0);
        assert_eq!(a.sample(0.5, 1.01), 0.0);
    }

    #[test]
    fn sampling_interpolates_between_pixels() {
        let a = Alpha::new("ramp", 2, 1, vec![0.0, 1.0]);
        assert!((a.sample(0.5, 0.0) - 0.5).abs() < 1e-5);
        assert!((a.sample(0.25, 0.0) - 0.25).abs() < 1e-5);
    }
}
