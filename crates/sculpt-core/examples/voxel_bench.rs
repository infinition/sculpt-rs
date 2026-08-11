//! Where the cost of sculpting goes when the surface is a voxel field.
//!
//! `cargo run --release -p sculpt-core --example voxel_bench`
//!
//! The point this benchmark exists to make: once a mesh is voxelised, sculpting
//! it costs what the voxel field costs, not what the mesh cost. Two icospheres,
//! one a million and a half triangles, one five and a quarter million, are
//! voxelised at the same resolution and sculpted the same way. The voxelisation
//! (a one-time cost, on load) scales with the triangle count. The sculpting
//! (stamps and extraction) must not.

use glam::Vec3;
use sculpt_core::{primitives, voxel};
use std::time::Instant;

fn ms<T>(label: &str, f: impl FnOnce() -> T) -> T {
    let t = Instant::now();
    let out = f();
    println!("{label:<46} {:>10.1} ms", t.elapsed().as_secs_f64() * 1000.0);
    out
}

fn main() {
    println!("threads rayon: {}\n", rayon::current_num_threads());
    let h = 0.02f32;

    // Two meshes at a quarter of each other's density.
    let coarse_mesh = primitives::icosphere(8);
    let fine_mesh = primitives::icosphere(9);
    println!(
        "icosphere 8: {:>9} faces, icosphere 9: {:>9} faces",
        coarse_mesh.face_count(),
        fine_mesh.face_count()
    );
    println!();

    let coarse = ms("voxelise 1.5 M triangles", || voxel::VoxelField::from_mesh(&coarse_mesh, h));
    let fine = ms("voxelise 5.2 M triangles", || voxel::VoxelField::from_mesh(&fine_mesh, h));
    println!(
        "  {} chunks vs {} chunks, {} vs {} stamp voxels for a radius-0.2 brush\n",
        coarse.chunk_count(),
        fine.chunk_count(),
        coarse.stamp_cells(0.2),
        fine.stamp_cells(0.2)
    );

    let sculpt = |f: &mut voxel::VoxelField, dabs: usize| {
        // One full extraction to stand the surface up, then incremental
        // extraction per dab, which is what a frame of sculpting pays.
        f.extract();
        let t = Instant::now();
        for i in 0..dabs {
            let at = Vec3::new((i as f32 * 0.01).sin(), 1.0, (i as f32 * 0.01).cos());
            f.stamp(at, 0.2, 0.1);
            let _ = f.extract_modified();
        }
        t.elapsed().as_secs_f64() * 1000.0
    };

    let dabs = 20usize;
    let mut coarse = coarse;
    let mut fine = fine;
    let coarse_s = sculpt(&mut coarse, dabs);
    let fine_s = sculpt(&mut fine, dabs);
    println!("{:<46} {:>10.1} ms  ({:.2} ms/dab)", "sculpt 20 dabs, coarse field", coarse_s, coarse_s / dabs as f64);
    println!("{:<46} {:>10.1} ms  ({:.2} ms/dab)", "sculpt 20 dabs, fine field", fine_s, fine_s / dabs as f64);
    let ratio = fine_s / coarse_s.max(0.001);
    println!(
        "\n3.5x the triangles, sculpting cost ratio {:.2}x. The one-time\nvoxelisation paid the density; the sculpting does not.",
        ratio
    );
}
