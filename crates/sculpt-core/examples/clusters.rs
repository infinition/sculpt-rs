//! Ce que la partition coûte et ce qu'elle écarte.
//!
//! `cargo run --release -p sculpt-core --example clusters [niveau]`
//!
//! Le tri par localité ne vaut que ce qu'il fait gagner au dessin. Ce programme
//! mesure les deux: le temps de construction, et la part du modèle qui reste à
//! dessiner selon l'angle de vue.

use glam::{Mat4, Vec3};
use sculpt_core::{cluster, primitives};
use std::time::Instant;

/// Les six plans du frustum, extraits d'une matrice vue-projection.
fn planes_of(m: Mat4) -> [[f32; 4]; 6] {
    let r = m.transpose();
    let row = |i: usize| -> [f32; 4] { r.col(i).to_array() };
    let add = |a: [f32; 4], b: [f32; 4], sign: f32| -> [f32; 4] {
        let p = [
            a[0] + sign * b[0],
            a[1] + sign * b[1],
            a[2] + sign * b[2],
            a[3] + sign * b[3],
        ];
        // Normalisé, pour que la distance ait un sens.
        let n = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt().max(1e-9);
        [p[0] / n, p[1] / n, p[2] / n, p[3] / n]
    };
    let (r0, r1, r2, r3) = (row(0), row(1), row(2), row(3));
    [
        add(r3, r0, 1.0),
        add(r3, r0, -1.0),
        add(r3, r1, 1.0),
        add(r3, r1, -1.0),
        add(r3, r2, 1.0),
        add(r3, r2, -1.0),
    ]
}

fn main() {
    let level: u32 = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(7);
    let mut m = primitives::icosphere(level);
    println!("{} faces", m.face_count());

    let t = Instant::now();
    let p = cluster::build(&mut m, cluster::TARGET_FACES);
    println!(
        "partition en {:.0} ms, {} paquets de {} faces",
        t.elapsed().as_secs_f64() * 1000.0,
        p.clusters.len(),
        cluster::TARGET_FACES
    );

    let t = Instant::now();
    let mut cs = p.clusters.clone();
    cluster::remeasure(&m, &mut cs, &[0, 5000, 100_000], cluster::TARGET_FACES);
    println!(
        "remesure de trois paquets: {:.3} ms",
        t.elapsed().as_secs_f64() * 1000.0
    );

    println!();
    let total = m.face_count() as f32;
    // Des distances de caméra typiques, du plan large au gros plan.
    for distance in [4.0f32, 2.5, 1.6, 1.2] {
        let eye = Vec3::new(0.0, 0.0, distance);
        let view = glam::camera::rh::view::look_at_mat4(eye, Vec3::ZERO, Vec3::Y);
        let proj = glam::camera::rh::proj::directx::perspective(
            45f32.to_radians(),
            16.0 / 9.0,
            0.01,
            100.0,
        );
        let planes = planes_of(proj * view);

        let t = Instant::now();
        let ranges = p.visible_ranges(&planes, Some(eye));
        let us = t.elapsed().as_secs_f64() * 1e6;
        let kept = cluster::Partition::faces_in(&ranges) as f32;
        println!(
            "caméra à {distance:>4.1}: {:>5.1}% dessiné, {:>4} appels, tri en {us:>6.0} us",
            kept / total * 100.0,
            ranges.len()
        );
    }
}
