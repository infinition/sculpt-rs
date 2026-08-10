//! Où part le temps et la mémoire sur un maillage lourd.
//!
//! `cargo run --release -p sculpt-core --example heavy`
//!
//! Le bench voisin mesure les requêtes. Celui-ci mesure ce qui fait tomber une
//! session: le coût d'un début de trait, celui d'une subdivision, et la place
//! que tout cela prend réellement en mémoire.

use glam::Vec3;
use sculpt_core::{primitives, topology, Mesh, Sculptor, StrokeInput};
use std::time::Instant;

fn ms<T>(label: &str, f: impl FnOnce() -> T) -> T {
    let t = Instant::now();
    let out = f();
    println!("{label:<46} {:>9.1} ms", t.elapsed().as_secs_f64() * 1000.0);
    out
}

/// Ce que le maillage occupe, poste par poste.
fn footprint(m: &Mesh) {
    let v = m.verts.len();
    let f = m.faces.len();
    let verts = v * std::mem::size_of::<sculpt_core::Vertex>();
    let faces = f * 12;
    // Une liste de faces garde huit indices en ligne, qu'ils servent ou non.
    let adj = v * std::mem::size_of::<sculpt_core::mesh::FaceList>();
    println!(
        "  {v:>9} sommets  {f:>9} faces   sommets {:>6.0} Mo  faces {:>5.0} Mo  adjacence {:>6.0} Mo  total {:>6.0} Mo",
        verts as f64 / 1e6,
        faces as f64 / 1e6,
        adj as f64 / 1e6,
        (verts + faces + adj) as f64 / 1e6
    );
}

fn main() {
    println!("threads rayon: {}", rayon::current_num_threads());
    println!();

    // Le niveau de la sphère de départ, en argument. 8 fait 1,3 M de
    // triangles, 9 en fait 5,2 M.
    let level: u32 = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(8);
    let base = ms(&format!("icosphere({level})"), || primitives::icosphere(level));
    footprint(&base);

    let mut s = Sculptor::new(base);
    s.symmetry = false;
    s.dyntopo_enabled = false;
    s.brush.radius = 0.12;

    let hit = StrokeInput {
        point: Vec3::new(0.0, 1.0, 0.0),
        normal: Vec3::Y,
        view_right: Vec3::X,
        ..Default::default()
    };

    // Un début de trait: c'est là que l'historique prend sa photo.
    ms("begin_stroke (photo pour l'annulation)", || {
        s.begin_stroke();
    });
    ms("un coup de brosse", || s.stroke(&hit));
    ms("20 coups de brosse", || {
        for _ in 0..20 {
            s.stroke(&hit);
        }
    });
    s.end_stroke();

    println!("  historique: {:>6.0} Mo", s.history.used_bytes() as f64 / 1e6);

    // Dix traits de suite, ce que fait n'importe qui pendant dix secondes.
    let t = Instant::now();
    for _ in 0..10 {
        s.begin_stroke();
        for _ in 0..10 {
            s.stroke(&hit);
        }
        s.end_stroke();
    }
    println!(
        "{:<46} {:>9.1} ms   historique {:.0} Mo",
        "10 traits de 10 coups",
        t.elapsed().as_secs_f64() * 1000.0,
        s.history.used_bytes() as f64 / 1e6
    );

    // Ce qu'une remise en ordre coûte entre deux traits. C'est le seul à-coup
    // que l'utilisateur voit alors qu'il n'a rien demandé.
    println!();
    {
        let mut copy = s.mesh().clone();
        ms("reconstruire l'adjacence", || copy.rebuild_adjacency());
        ms("remettre les faces en ordre", || {
            sculpt_core::cluster::build(&mut copy, sculpt_core::cluster::TARGET_FACES)
        });
        let touched: Vec<u32> = (0..copy.vert_count() as u32).step_by(97).collect();
        println!("  ({} sommets touchés)", touched.len());
        ms("normales autour de ce qui a bougé", || {
            copy.update_normals(&touched)
        });
    }

    // Ce que le viewport demande à chaque image, curseur à côté du modèle.
    println!();
    {
        let m = s.mesh();
        let o = Vec3::new(1.4, 0.0, 5.0);
        let d = -Vec3::Z;
        let t = Instant::now();
        for _ in 0..60 {
            let _ = sculpt_core::query::nearest_to_ray(m, o, d, 0.05);
        }
        println!(
            "{:<46} {:>9.2} ms   par image",
            "surface la plus proche, curseur à côté",
            t.elapsed().as_secs_f64() * 1000.0 / 60.0
        );
        let o = Vec3::new(0.0, 0.0, 5.0);
        let t = Instant::now();
        for _ in 0..60 {
            let _ = sculpt_core::query::nearest_to_ray(m, o, d, 0.05);
        }
        println!(
            "{:<46} {:>9.2} ms   par image",
            "la même, curseur sur le modèle",
            t.elapsed().as_secs_f64() * 1000.0 / 60.0
        );
    }

    // Le mode réel de l'application: topologie dynamique activée.
    println!();
    s.dyntopo_enabled = true;
    s.reset_detail_to_mesh();
    s.dyntopo.detail *= 0.5;
    s.begin_stroke();
    let t = Instant::now();
    let mut moved = Vec3::new(0.0, 1.0, 0.0);
    for i in 0..20 {
        moved.x = (i as f32) * 0.01;
        s.stroke(&StrokeInput { point: moved, normal: Vec3::Y, ..Default::default() });
    }
    s.end_stroke();
    println!(
        "{:<46} {:>9.1} ms   soit {:.1} ms par coup, {} sommets ajoutés",
        "20 coups avec topologie dynamique",
        t.elapsed().as_secs_f64() * 1000.0,
        t.elapsed().as_secs_f64() * 1000.0 / 20.0,
        s.mesh().verts.len() as i64 - 2621442
    );

    // Décimer puis sculpter: la séquence qui a fermé la fenêtre.
    println!();
    let before = s.mesh().face_count();
    ms("décimation à la moitié", || s.decimate(0.5));
    println!("  {} faces -> {}", before, s.mesh().face_count());
    s.begin_stroke();
    let t = Instant::now();
    for i in 0..20 {
        let p = Vec3::new((i as f32) * 0.01, 1.0, 0.0);
        s.stroke(&StrokeInput { point: p, normal: Vec3::Y, ..Default::default() });
    }
    s.end_stroke();
    println!(
        "{:<46} {:>9.1} ms   {} faces",
        "20 coups après décimation",
        t.elapsed().as_secs_f64() * 1000.0,
        s.mesh().face_count()
    );

    // La subdivision, et le pic qu'elle demande.
    println!();
    let m = s.mesh().clone();
    let fine = ms("subdivision (x4 faces)", || topology::subdivide(&m, true));
    footprint(&fine);
    println!(
        "  pic pendant l'opération: l'ancien et le nouveau existent en même temps, donc environ {:.0} Mo",
        ((m.verts.len() + fine.verts.len()) * std::mem::size_of::<sculpt_core::Vertex>()
            + (m.faces.len() + fine.faces.len()) * 12
            + (m.verts.len() + fine.verts.len())
                * std::mem::size_of::<sculpt_core::mesh::FaceList>()) as f64
            / 1e6
    );
}
