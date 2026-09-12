//! CPU interaction latency, including the partition/picking path used by the app.
//! cargo run --release -p sculpt-core --example interaction -- 9
use glam::Vec3;
use sculpt_core::{BrushKind, Sculptor, StrokeInput, cluster, primitives, query};
use std::{hint::black_box, time::Instant};

fn sample(label: &str, n: usize, mut f: impl FnMut()) {
    let mut times = Vec::with_capacity(n);
    for _ in 0..n {
        let start = Instant::now();
        f();
        times.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    report(label, times);
}

fn report(label: &str, mut times: Vec<f64>) {
    let n = times.len();
    times.sort_by(f64::total_cmp);
    println!(
        "{label:36} p50 {:8.3} ms  p95 {:8.3} ms  max {:8.3} ms",
        times[n / 2],
        times[(n * 95 / 100).min(n - 1)],
        times[n - 1]
    );
}

fn main() {
    let level = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(9);
    let mut s = Sculptor::new(if std::env::args().nth(1).as_deref() == Some("40m") { primitives::uv_sphere(4096, 5121) } else { primitives::icosphere(level) });
    s.symmetry = false;
    s.dyntopo.max_verts = 40_000_000;
    s.brush.radius = 0.15;
    println!(
        "{} vertices, {} triangles, {} threads",
        s.mesh().vert_count(),
        s.mesh().face_count(),
        rayon::current_num_threads()
    );
    let mut partition = cluster::build(s.mesh_mut().unwrap(), cluster::TARGET_FACES);
    let _ = s.take_dirty();
    let hit = Vec3::new(0.0, 0.0, 3.0);
    let miss = Vec3::new(1.4, 0.0, 3.0);
    sample("pick after partition (no grid)", 30, || {
        black_box(query::raycast(s.mesh(), hit, -Vec3::Z));
    });
    sample("build missing grid", 1, || {
        s.mesh_mut().unwrap().ensure_accel_if_missing(0.15)
    });
    sample("pick hit (grid)", 60, || {
        black_box(query::raycast(s.mesh(), hit, -Vec3::Z));
    });
    sample("pick miss (grid)", 60, || {
        black_box(query::raycast(s.mesh(), miss, -Vec3::Z));
    });
    for dyn_on in [false, true] {
        s.dyntopo_enabled = dyn_on;
        s.brush.kind = BrushKind::Clay;
        sample("begin stroke", 1, || s.begin_stroke());
        let mut i = 0;
        let mut engine = Vec::new();
        let mut culling = Vec::new();
        sample(
            if dyn_on {
                "dab + refit + partition (dyntopo)"
            } else {
                "dab + refit + partition (fixed)"
            },
            60,
            || {
                let p = Vec3::new(i as f32 * 0.004, 0.0, 1.0).normalize();
                let start = Instant::now();
                s.stroke(&StrokeInput {
                    point: p,
                    normal: p,
                    ..Default::default()
                });
                engine.push(start.elapsed().as_secs_f64() * 1000.0);
                let (_, faces, full) = s.take_dirty();
                assert!(!full);
                let start = Instant::now();
                cluster::follow(s.mesh(), &mut partition, &faces, cluster::TARGET_FACES);
                culling.push(start.elapsed().as_secs_f64() * 1000.0);
                i += 1;
            },
        );
        sample("end stroke", 1, || s.end_stroke());
        report("  engine only", engine);
        report("  partition only", culling);
        println!(
            "  now {} triangles, undo {:.1} MB",
            s.mesh().face_count(),
            s.history.used_bytes() as f64 / 1e6
        );
    }
}
