//! Ce que coûte une coupe d'arête, isolément.
//!
//! `cargo run --release -p sculpt-core --example split_cost`
//!
//! La topologie dynamique passe l'essentiel de son temps ici: un coup de brosse
//! sur un maillage dense en fait quelques milliers. Une moyenne sur un trait
//! entier noie ce chiffre dans le reste; celui-ci le mesure seul, et prend le
//! meilleur de plusieurs essais parce que la machine est bruyante.

use sculpt_core::{primitives, query};
use std::time::Instant;

fn main() {
    let mut best = f64::MAX;
    let mut cuts = 0usize;

    for _ in 0..5 {
        let mut m = primitives::icosphere(6);
        m.ensure_accel(0.02);
        // Les arêtes d'une région, prises avant de toucher à quoi que ce soit.
        let center = glam::Vec3::new(0.0, 1.0, 0.0);
        let verts = query::verts_in_sphere(&m, center, 0.35);
        let mut edges: Vec<(u32, u32)> = Vec::new();
        for v in verts {
            for &f in &m.vfaces[v as usize] {
                let tri = m.faces[f as usize];
                for k in 0..3 {
                    let (a, b) = (tri[k], tri[(k + 1) % 3]);
                    if a == v && a < b {
                        edges.push((a, b));
                    }
                }
            }
        }
        edges.sort_unstable();
        edges.dedup();

        let t = Instant::now();
        let mut n = 0;
        for (a, b) in &edges {
            if m.split_edge(*a, *b).is_some() {
                n += 1;
            }
        }
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        if ms < best {
            best = ms;
            cuts = n;
        }
    }

    println!(
        "{cuts} coupes en {best:.2} ms, soit {:.2} us par coupe",
        best * 1000.0 / cuts.max(1) as f64
    );
}
