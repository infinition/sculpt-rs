//! Procedurally generated matcaps.
//!
//! A matcap is a pre-lit sphere looked up by the view-space normal. Generating
//! it in code instead of shipping PNGs keeps the repository asset-free and
//! license-clean, which matters for a project meant to be published.

use glam::Vec3;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
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

    fn look(self) -> Look {
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

struct Look {
    base: Vec3,
    spec: Vec3,
    shininess: f32,
    spec_amt: f32,
    rim: Vec3,
    ambient: f32,
}

struct Light {
    dir: Vec3,
    color: Vec3,
    power: f32,
}

/// Renders a lit sphere into an RGBA8 buffer of `size` x `size`.
pub fn generate(preset: Preset, size: u32) -> Vec<u8> {
    let look = preset.look();
    let lights = [
        Light { dir: Vec3::new(-0.45, 0.62, 0.65).normalize(), color: Vec3::new(1.0, 0.97, 0.92), power: 1.0 },
        Light { dir: Vec3::new(0.75, 0.05, 0.55).normalize(), color: Vec3::new(0.62, 0.72, 0.95), power: 0.42 },
        Light { dir: Vec3::new(0.10, -0.75, 0.35).normalize(), color: Vec3::new(0.95, 0.85, 0.75), power: 0.22 },
    ];
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
            for l in &lights {
                let ndl = n.dot(l.dir).max(0.0);
                c += look.base * l.color * (ndl * l.power);
                if ndl > 0.0 {
                    let h = (l.dir + view).normalize_or(view);
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
