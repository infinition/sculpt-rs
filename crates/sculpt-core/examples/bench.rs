//! Timings for the hot paths, so performance claims can be checked rather than
//! asserted.
//!
//! `cargo run --release -p sculpt-core --example bench`

use glam::Vec3;
use sculpt_core::{primitives, query, topology, Brush, BrushKind, Sculptor, StrokeInput};
use std::time::Instant;

fn time<T>(label: &str, f: impl FnOnce() -> T) -> T {
    let t = Instant::now();
    let out = f();
    println!("{label:<44} {:>9.2} ms", t.elapsed().as_secs_f64() * 1000.0);
    out
}

fn main() {
    println!("threads in the rayon pool: {}", rayon::current_num_threads());
    println!();

    for subdivisions in [5u32, 6, 7] {
        let mesh = primitives::icosphere(subdivisions);
        println!(
            "--- icosphere {subdivisions}: {} verts, {} tris ---",
            mesh.vert_count(),
            mesh.face_count()
        );

        // Sphere query, with and without the spatial grid.
        let center = Vec3::new(0.0, 1.0, 0.0);
        let radius = 0.15;
        let mut m = mesh.clone();
        let n = time("verts in sphere, linear scan", || {
            query::verts_in_sphere(&m, center, radius).len()
        });
        m.ensure_accel(radius);
        let g = time("verts in sphere, grid", || {
            query::verts_in_sphere(&m, center, radius).len()
        });
        assert_eq!(n, g);

        // Ray cast, with and without the grid.
        let (o, d) = (Vec3::new(0.4, 0.0, 5.0), -Vec3::Z);
        time("ray cast, grid", || query::raycast(&m, o, d));
        m.invalidate_accel();
        time("ray cast, linear scan", || query::raycast(&m, o, d));

        // A whole stroke, with and without dynamic topology, so the cost of
        // refinement can be separated from the cost of the brush itself.
        let dab = |i: usize| {
            let a = i as f32 * 0.04;
            let p =
                Vec3::new(a.sin() * 0.3, (1.0 - a * a * 0.05).max(0.2), a.cos() * 0.3).normalize();
            StrokeInput { point: p, normal: p, ..Default::default() }
        };
        for dyntopo in [false, true] {
            let mut s = Sculptor::new(mesh.clone());
            s.brush = Brush { kind: BrushKind::Clay, radius, strength: 0.5, ..Brush::default() };
            s.symmetry = false;
            s.dyntopo_enabled = dyntopo;
            let before = s.mesh().vert_count();
            let label = if dyntopo {
                "50 dabs, dynamic topology on"
            } else {
                "50 dabs, dynamic topology off"
            };
            time(label, || {
                s.begin_stroke();
                for i in 0..50 {
                    s.stroke(&dab(i));
                }
                s.end_stroke();
            });
            if dyntopo {
                println!(
                    "{:<44} {:>9}",
                    "  vertices added by the stroke",
                    s.mesh().vert_count() - before
                );
            }
        }

        // One-shot commands.
        let mut m2 = mesh.clone();
        time("recompute every normal", || m2.recompute_normals());
        time("subdivide (Loop)", || topology::subdivide(&m2, true));
        time("voxel remesh at 128", || {
            topology::voxel_remesh(
                &m2,
                &sculpt_core::RemeshOptions {
                    resolution: 128,
                    smoothing: 1,
                    transfer_colors: false,
                },
            )
        });
        println!();
    }
}
