//! Où part le temps d'un seul coup de brosse, poste par poste.
//!
//! `cargo run --release -p sculpt-core --example dab_cost [niveau]`
//!
//! Les autres mesures donnent le total d'un trait, ce qui suffit à dire si ça
//! va mieux et pas à dire quoi reprendre. Celle-ci découpe une dab en ses
//! quatre morceaux, sur un maillage assez dense pour que la topologie dynamique
//! travaille vraiment.

use glam::Vec3;
use sculpt_core::{brush, dyntopo, primitives, Brush, BrushKind, Dyntopo, StrokeInput};
use std::time::Instant;

fn main() {
    let level: u32 = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(8);

    let mut mesh = if std::env::args().nth(1).as_deref() == Some("40m") { primitives::uv_sphere(4096, 5121) } else { primitives::icosphere(level) };
    println!(
        "maillage: {} sommets, {} faces",
        mesh.vert_count(),
        mesh.face_count()
    );
    println!("threads rayon: {}\n", rayon::current_num_threads());

    let radius = 0.15f32;
    let fixed = std::env::args().any(|a| a == "fixed");
    let params = Dyntopo {
        detail: mesh.mean_edge_len() * 0.5,
        subdivide: true,
        decimate: true,
        max_verts: 40_000_000,
        ..Default::default()
    };
    let b = Brush { kind: BrushKind::Clay, radius, strength: 0.6, ..Brush::default() };
    let mut state = brush::StrokeState::default();

    mesh.ensure_accel(radius);
    let mut totals = [0f64; 5];
    let mut edges = 0usize;
    let mut moved = 0usize;
    const DABS: usize = 10;

    for i in 0..DABS {
        let at = Vec3::new(i as f32 * 0.004, 0.0, 1.0).normalize();

        let t = Instant::now();
        let plan = (!fixed).then(|| dyntopo::plan(&mesh, at, radius, &params));
        totals[0] += t.elapsed().as_secs_f64() * 1000.0;
        edges += plan.as_ref().map_or(0, |p| p.len());

        let t = Instant::now();
        if let Some(plan) = plan { dyntopo::apply(&mut mesh, plan, at, radius, &params); }
        totals[1] += t.elapsed().as_secs_f64() * 1000.0;

        let input = StrokeInput { point: at, normal: at, ..Default::default() };
        let t = Instant::now();
        let touched = brush::apply(&mut mesh, &b, &input, &mut state, None);
        totals[2] += t.elapsed().as_secs_f64() * 1000.0;
        moved += touched.len();

        let t = Instant::now();
        mesh.update_sculpt_normals(&touched);
        totals[3] += t.elapsed().as_secs_f64() * 1000.0;

        let t = Instant::now();
        mesh.flush_refit();
        totals[4] += t.elapsed().as_secs_f64() * 1000.0;
    }

    let names = [
        "chercher les arêtes à couper (plan)",
        "couper et fusionner (apply)",
        "déplacer les sommets (brosse)",
        "refaire les normales",
        "remettre les faces dans l'index",
    ];
    let total: f64 = totals.iter().sum();
    println!("{:<40} {:>9} {:>8}", "", "ms/coup", "part");
    for (name, ms) in names.iter().zip(totals) {
        println!(
            "{name:<40} {:>9.2} {:>7.0}%",
            ms / DABS as f64,
            ms / total * 100.0
        );
    }
    println!("{:<40} {:>9.2}", "total", total / DABS as f64);
    println!(
        "\n{} arêtes examinées et {} sommets déplacés par coup, {} faces à la fin",
        edges / DABS,
        moved / DABS,
        mesh.face_count()
    );
}
