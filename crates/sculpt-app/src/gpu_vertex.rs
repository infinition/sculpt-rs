//! What a vertex looks like once it reaches the card.
//!
//! The engine keeps a vertex as twelve floats: position, normal, colour, mask,
//! roughness and metalness, forty-eight bytes. That is the right shape for the
//! work the engine does with it, and the wrong one to send anywhere. Five
//! million vertices is two hundred and forty megabytes, and the card reads all
//! of it for every frame even in a matcap view, which uses a quarter of it.
//!
//! Here it becomes twenty-four bytes in two streams.
//!
//! The **hot** stream is what shading a pixel cannot do without: the position,
//! which stays at full precision because everything else is measured against
//! it, and the normal, folded onto an octahedron and kept as two sixteen bit
//! numbers. Octahedral encoding maps a direction onto a square with almost no
//! distortion, and sixteen bits a side puts the worst error near a hundredth of
//! a degree, which is far below anything a shading term can show.
//!
//! The **cold** stream is what only painting touches: colour, mask, roughness
//! and metalness, a byte each. A byte is what a colour picker offers and what a
//! texture would have held anyway.
//!
//! Nothing here is specific to a platform: two integer formats every backend
//! supports, and no assumption about byte order beyond the one wgpu already
//! makes.

use glam::{Vec2, Vec3};
use rayon::prelude::*;
use sculpt_core::{Mesh, Vertex};

/// Words in the hot stream: three of position, one of packed normal.
pub const HOT_WORDS: usize = 4;
/// Words in the cold stream: colour with the mask, then the two material
/// values.
pub const COLD_WORDS: usize = 2;
pub const HOT_BYTES: usize = HOT_WORDS * 4;
pub const COLD_BYTES: usize = COLD_WORDS * 4;

/// Folds a unit vector onto the octahedron and keeps two sixteen bit numbers.
///
/// The lower half of the sphere is reflected outwards into the corners of the
/// square, which is what makes the mapping continuous and the error even.
fn oct_encode(n: Vec3) -> u32 {
    let n = n.normalize_or(Vec3::Y);
    let denom = n.x.abs() + n.y.abs() + n.z.abs();
    let mut p = Vec2::new(n.x, n.y) / denom.max(1e-20);
    if n.z < 0.0 {
        let sign = |v: f32| if v >= 0.0 { 1.0 } else { -1.0 };
        p = Vec2::new(
            (1.0 - p.y.abs()) * sign(p.x),
            (1.0 - p.x.abs()) * sign(p.y),
        );
    }
    let q = (p * 0.5 + Vec2::splat(0.5)).clamp(Vec2::ZERO, Vec2::ONE);
    let x = (q.x * 65535.0).round() as u32;
    let y = (q.y * 65535.0).round() as u32;
    x | (y << 16)
}

#[inline]
fn unorm8(v: f32) -> u32 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u32
}

/// The four words the card reads to place and shade a vertex.
#[inline]
pub fn hot_from(pos: Vec3, nrm: Vec3) -> [u32; HOT_WORDS] {
    [pos.x.to_bits(), pos.y.to_bits(), pos.z.to_bits(), oct_encode(nrm)]
}

#[inline]
pub fn hot(v: &Vertex) -> [u32; HOT_WORDS] {
    hot_from(v.pos, v.nrm)
}

/// The two words that only painting writes to.
#[inline]
pub fn cold_from(col: Vec3, mask: f32, rough: f32, metal: f32) -> [u32; COLD_WORDS] {
    [
        unorm8(col.x) | (unorm8(col.y) << 8) | (unorm8(col.z) << 16) | (unorm8(mask) << 24),
        unorm8(rough) | (unorm8(metal) << 8),
    ]
}

#[inline]
pub fn cold(v: &Vertex) -> [u32; COLD_WORDS] {
    cold_from(v.col, v.mask, v.rough, v.metal)
}

/// Packs a whole mesh, both streams at once and across every core.
///
/// Called when a mesh arrives or is rewritten from end to end, which on a large
/// model means several million vertices at a stroke.
///
/// The hot stream reads the two arrays the engine always holds. The cold one
/// asks the mesh for each vertex, and on a model nobody has painted every one
/// of those answers is the channel default, which costs no memory to store and
/// nothing to read.
pub fn pack_all(mesh: &Mesh) -> (Vec<u32>, Vec<u32>) {
    rayon::join(
        || {
            mesh.pos
                .par_iter()
                .zip(mesh.nrm.par_iter())
                .flat_map_iter(|(p, n)| hot_from(*p, *n))
                .collect()
        },
        || {
            (0..mesh.vert_count() as u32)
                .into_par_iter()
                .flat_map_iter(|v| {
                    cold_from(mesh.col(v), mesh.mask(v), mesh.rough(v), mesh.metal(v))
                })
                .collect()
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Decoding, written the way the shader writes it, so the two can be
    /// checked against each other.
    fn oct_decode(packed: u32) -> Vec3 {
        let ex = (packed & 0xffff) as f32 / 65535.0;
        let ey = (packed >> 16) as f32 / 65535.0;
        let f = Vec2::new(ex, ey) * 2.0 - Vec2::splat(1.0);
        let mut n = Vec3::new(f.x, f.y, 1.0 - f.x.abs() - f.y.abs());
        let t = (-n.z).max(0.0);
        n.x += if n.x >= 0.0 { -t } else { t };
        n.y += if n.y >= 0.0 { -t } else { t };
        n.normalize_or(Vec3::Y)
    }

    /// A normal has to survive the trip. The bound is what the shading can
    /// show, not what the encoding can manage.
    #[test]
    fn a_normal_survives_the_octahedron() {
        let mut worst: f32 = 0.0;
        // A spread of directions, including the seams of the mapping: the
        // poles, the equator and the diagonals where the fold happens.
        for i in 0..64 {
            for j in 0..64 {
                let u = i as f32 / 63.0 * std::f32::consts::TAU;
                let v = (j as f32 / 63.0) * std::f32::consts::PI;
                let n = Vec3::new(v.sin() * u.cos(), v.cos(), v.sin() * u.sin()).normalize();
                let back = oct_decode(oct_encode(n));
                worst = worst.max(n.angle_between(back).to_degrees());
            }
        }
        assert!(worst < 0.05, "worst error {worst} degrees, too coarse to shade with");
    }

    #[test]
    fn the_axes_come_back_exactly_enough() {
        for n in [Vec3::X, -Vec3::X, Vec3::Y, -Vec3::Y, Vec3::Z, -Vec3::Z] {
            let back = oct_decode(oct_encode(n));
            assert!(back.distance(n) < 1e-3, "{n} came back as {back}");
        }
    }

    /// The packing is what the vertex layout in the renderer promises; if the
    /// two disagree the model renders as noise, so the sizes are pinned here.
    #[test]
    fn a_vertex_costs_twenty_four_bytes() {
        assert_eq!(HOT_BYTES + COLD_BYTES, 24);
        assert!(HOT_BYTES + COLD_BYTES < std::mem::size_of::<Vertex>());
    }

    #[test]
    fn colour_and_material_land_in_the_right_bytes() {
        let mut v = Vertex::new(Vec3::ZERO);
        v.col = Vec3::new(1.0, 0.0, 0.0);
        v.mask = 1.0;
        v.rough = 0.0;
        v.metal = 1.0;
        let c = cold(&v);
        assert_eq!(c[0] & 0xff, 255, "red");
        assert_eq!((c[0] >> 8) & 0xff, 0, "green");
        assert_eq!((c[0] >> 16) & 0xff, 0, "blue");
        assert_eq!((c[0] >> 24) & 0xff, 255, "mask");
        assert_eq!(c[1] & 0xff, 0, "roughness");
        assert_eq!((c[1] >> 8) & 0xff, 255, "metalness");
    }
}
