//! Matcaps, generated or loaded.
//!
//! A matcap is a pre-lit sphere looked up by the view-space normal. Generating
//! it in code instead of shipping PNGs keeps the repository asset-free and
//! license-clean, which matters for a project meant to be published.
//!
//! Every preset here is really a lightcap: a material and three lights, which
//! the renderer bakes into a sphere. Keeping the description rather than only
//! the picture is what lets a preset be taken apart and edited, and a light
//! dragged around the model in the middle of a session.

use glam::Vec3;

/// Side of the generated texture. Big enough that a broad highlight has no
/// visible steps, small enough to rebuild while a light is being dragged.
pub const SIZE: u32 = 256;

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Preset {
    Clay,
    Pearl,
    Skin,
    Jade,
    Metal,
}

impl Preset {
    pub const ALL: [Preset; 5] = [Preset::Clay, Preset::Pearl, Preset::Skin, Preset::Jade, Preset::Metal];

    pub fn label(self) -> &'static str {
        match self {
            Preset::Clay => "Clay",
            Preset::Pearl => "Pearl",
            Preset::Skin => "Skin",
            Preset::Jade => "Jade",
            Preset::Metal => "Metal",
        }
    }

    pub fn look(self) -> Look {
        match self {
            Preset::Clay => Look {
                base: Vec3::new(0.72, 0.40, 0.31),
                spec: Vec3::new(1.0, 0.95, 0.90),
                shininess: 18.0,
                spec_amt: 0.18,
                rim: Vec3::new(0.55, 0.35, 0.30),
                ambient: 0.26,
            },
            Preset::Pearl => Look {
                base: Vec3::new(0.86, 0.86, 0.88),
                spec: Vec3::new(1.0, 1.0, 1.0),
                shininess: 60.0,
                spec_amt: 0.45,
                rim: Vec3::new(0.45, 0.50, 0.62),
                ambient: 0.34,
            },
            Preset::Skin => Look {
                base: Vec3::new(0.85, 0.62, 0.52),
                spec: Vec3::new(1.0, 0.92, 0.88),
                shininess: 26.0,
                spec_amt: 0.22,
                rim: Vec3::new(0.72, 0.28, 0.22),
                ambient: 0.30,
            },
            Preset::Jade => Look {
                base: Vec3::new(0.30, 0.62, 0.48),
                spec: Vec3::new(0.85, 1.0, 0.92),
                shininess: 48.0,
                spec_amt: 0.38,
                rim: Vec3::new(0.20, 0.55, 0.45),
                ambient: 0.28,
            },
            Preset::Metal => Look {
                base: Vec3::new(0.55, 0.57, 0.62),
                spec: Vec3::new(1.0, 1.0, 1.0),
                shininess: 120.0,
                spec_amt: 0.85,
                rim: Vec3::new(0.30, 0.34, 0.42),
                ambient: 0.18,
            },
        }
    }
}

/// The surface a lightcap is lighting.
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, PartialEq, Debug)]
pub struct Look {
    pub base: Vec3,
    pub spec: Vec3,
    pub shininess: f32,
    pub spec_amt: f32,
    pub rim: Vec3,
    pub ambient: f32,
}

/// One of the three lights, aimed in view space: +X is to the right of the
/// screen, +Y is up it, +Z is out of it toward the viewer.
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, PartialEq, Debug)]
pub struct Light {
    pub dir: Vec3,
    pub color: Vec3,
    pub power: f32,
    pub on: bool,
}

/// A material and the lights on it: everything a matcap is baked from.
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, PartialEq, Debug)]
pub struct Lightcap {
    pub look: Look,
    pub lights: [Light; 3],
}

/// The lighting every preset starts from: a key over the left shoulder, a cool
/// fill from the right, and a warm bounce from below.
pub const DEFAULT_LIGHTS: [Light; 3] = [
    Light {
        dir: Vec3::new(-0.45, 0.62, 0.65),
        color: Vec3::new(1.0, 0.97, 0.92),
        power: 1.0,
        on: true,
    },
    Light {
        dir: Vec3::new(0.75, 0.05, 0.55),
        color: Vec3::new(0.62, 0.72, 0.95),
        power: 0.42,
        on: true,
    },
    Light {
        dir: Vec3::new(0.10, -0.75, 0.35),
        color: Vec3::new(0.95, 0.85, 0.75),
        power: 0.22,
        on: true,
    },
];

impl Lightcap {
    pub fn from_preset(preset: Preset) -> Self {
        Self { look: preset.look(), lights: DEFAULT_LIGHTS }
    }

    /// Renders the lit sphere into an RGBA8 buffer of `size` x `size`.
    pub fn render(&self, size: u32) -> Vec<u8> {
        render(&self.look, &self.lights, size)
    }
}

impl Default for Lightcap {
    fn default() -> Self {
        Self::from_preset(Preset::Clay)
    }
}

/// Renders a lit sphere into an RGBA8 buffer of `size` x `size`.
pub fn generate(preset: Preset, size: u32) -> Vec<u8> {
    render(&preset.look(), &DEFAULT_LIGHTS, size)
}

fn render(look: &Look, lights: &[Light; 3], size: u32) -> Vec<u8> {
    let view = Vec3::Z;
    let mut out = vec![0u8; (size * size * 4) as usize];
    for y in 0..size {
        for x in 0..size {
            let u = (x as f32 + 0.5) / size as f32 * 2.0 - 1.0;
            let v = 1.0 - (y as f32 + 0.5) / size as f32 * 2.0;
            let r2 = u * u + v * v;

            // Outside the disc, keep extending the silhouette normal so the
            // texture stays smooth under bilinear filtering at the edge.
            let n = if r2 <= 1.0 {
                Vec3::new(u, v, (1.0 - r2).max(0.0).sqrt())
            } else {
                Vec3::new(u, v, 0.0).normalize_or(Vec3::Z)
            };

            let mut c = look.base * look.ambient;
            for l in lights {
                if !l.on {
                    continue;
                }
                let dir = l.dir.normalize_or(Vec3::Z);
                let ndl = n.dot(dir).max(0.0);
                c += look.base * l.color * (ndl * l.power);
                if ndl > 0.0 {
                    let h = (dir + view).normalize_or(view);
                    let s = n.dot(h).max(0.0).powf(look.shininess);
                    c += look.spec * l.color * (s * look.spec_amt * l.power);
                }
            }
            let fres = (1.0 - n.dot(view).max(0.0)).powf(3.0);
            c += look.rim * fres * 0.55;

            // Reinhard, then sRGB-ish encode.
            let c = c / (c + Vec3::ONE);
            let px = ((y * size + x) * 4) as usize;
            for k in 0..3 {
                let lin = [c.x, c.y, c.z][k].clamp(0.0, 1.0);
                out[px + k] = (lin.powf(1.0 / 2.2) * 255.0).round() as u8;
            }
            out[px + 3] = 255;
        }
    }
    out
}
