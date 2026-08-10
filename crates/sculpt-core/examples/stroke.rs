//! Un trait, coup par coup, dans les conditions de l'application.
//!
//! `cargo run --release -p sculpt-core --example stroke [faces_visées_en_millions]`
//!
//! Les autres mesures prennent des moyennes, ce qui cache exactement ce qui
//! gêne: un coup à trente millisecondes au milieu de coups à un. Celui-ci
//! imprime chaque coup.

use glam::Vec3;
use sculpt_core::{primitives, Sculptor, StrokeInput};
use std::time::Instant;

fn main() {
    let target: f64 = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(1.7);

    let mut s = Sculptor::new(primitives::icosphere(5));
    // Comme dans l'application: on subdivise jusqu'à la densité voulue.
    while (s.mesh().face_count() as f64) < target * 1.0e6 {
        let t = Instant::now();
        match s.subdivide(true) {
            Ok(faces) => println!(
                "subdivision -> {:>9} faces en {:>7.0} ms",
                faces,
                t.elapsed().as_secs_f64() * 1000.0
            ),
            Err(e) => {
                println!("subdivision refusée: {e}");
                break;
            }
        }
    }
    println!(
        "\n{} sommets, {} faces, détail {:.5}, arête moyenne {:.5}",
        s.mesh().vert_count(),
        s.mesh().face_count(),
        s.dyntopo.detail,
        s.mesh().mean_edge_len()
    );
    println!("topologie dynamique: {}", s.dyntopo_enabled);
    println!();

    s.symmetry = false;
    s.brush.radius = 0.15;
    s.brush.strength = 0.5;

    s.begin_stroke();
    let mut worst = 0.0f64;
    let mut total = 0.0f64;
    for i in 0..30 {
        // Un trait qui traverse la surface, comme une main le ferait.
        let a = i as f32 * 0.05;
        let p = Vec3::new(a.sin() * 0.6, (1.0 - a * a * 0.05).max(0.2), a.cos() * 0.6)
            .normalize()
            * 1.0;
        let t = Instant::now();
        s.stroke(&StrokeInput { point: p, normal: p, ..Default::default() });
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        total += ms;
        worst = worst.max(ms);
        let bar = "#".repeat((ms / 2.0).min(60.0) as usize);
        println!("coup {i:>2}  {ms:>8.2} ms  {bar}");
    }
    s.end_stroke();

    // Décomposition d'un coup, pour savoir où va le temps.
    println!();
    {
        let radius = s.brush.radius;
        let p = Vec3::new(0.3, 0.9, 0.3).normalize();
        let mesh = s.mesh();
        let t = Instant::now();
        let verts = sculpt_core::query::verts_in_sphere(mesh, p, radius);
        let t_verts = t.elapsed().as_secs_f64() * 1000.0;
        let t = Instant::now();
        let faces = sculpt_core::query::faces_in_sphere(mesh, p, radius * 1.15);
        let t_faces = t.elapsed().as_secs_f64() * 1000.0;
        let t = Instant::now();
        let plan = sculpt_core::dyntopo::plan(mesh, p, radius, &s.dyntopo);
        let t_plan = t.elapsed().as_secs_f64() * 1000.0;
        println!("sous la brosse: {} sommets, {} faces", verts.len(), faces.len());
        println!("  sommets dans la sphère {t_verts:>8.2} ms");
        println!("  faces dans la sphère   {t_faces:>8.2} ms");
        println!("  plan de raffinement    {t_plan:>8.2} ms  ({} à couper)", plan.len());
        drop(plan);
        // Le raffinement lui-même, sur un plan fraîchement calculé.
        let dyn_params = s.dyntopo;
        let mesh = s.mesh_mut().unwrap();
        let plan = sculpt_core::dyntopo::plan(mesh, p, radius, &dyn_params);
        let n = plan.len();
        let t = Instant::now();
        sculpt_core::dyntopo::apply(mesh, plan, p, radius, &dyn_params);
        let t_apply = t.elapsed().as_secs_f64() * 1000.0;
        println!(
            "  raffinement            {t_apply:>8.2} ms  ({n} arêtes, {:.2} us par arête)",
            t_apply * 1000.0 / n.max(1) as f64
        );
    }

    println!();
    println!(
        "moyenne {:.2} ms, pire {:.2} ms, total {:.0} ms pour 30 coups",
        total / 30.0,
        worst,
        total
    );
    println!(
        "{} faces à la fin, historique {:.1} Mo",
        s.mesh().face_count(),
        s.history.used_bytes() as f64 / 1.0e6
    );
}
