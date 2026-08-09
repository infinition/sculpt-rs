# sculpt-rs

A real-time 3D sculpting tool written from scratch in Rust, with dynamic
topology. Native desktop, rendered on the GPU through wgpu.

![sculpt-rs](docs/screenshot.png)

## What it does

- **Dynamic topology.** Detail is created and removed under the brush by
  splitting and collapsing edges, so you sculpt without worrying about the
  starting resolution.
- **Ten brushes.** Draw, Clay, Flatten, Smooth, Pinch, Crease, Inflate, Move,
  Paint, Mask.
- **Symmetry** across the X axis.
- **Masking** to protect parts of the surface from further edits.
- **Matcap shading**, five procedurally generated materials, plus optional
  vertex colours and wireframe.
- **Undo/redo** with a memory budget.
- **Import and export** OBJ (with vertex colours) and binary STL.
- **Primitives**: icosphere, cube, cylinder, torus, plane.

## Architecture

Two crates:

- `sculpt-core` is the engine. No graphics dependency, so it runs in headless
  tests and could later be compiled to WebAssembly unchanged. It owns the mesh,
  adjacency, dynamic topology, brushes, spatial queries, history and file I/O.
- `sculpt-app` is the desktop shell: wgpu renderer, winit window, egui panel,
  orbit camera and input handling.

The mesh keeps its vertex and index arrays gap-free so they upload to the GPU
without a compaction pass. Brushes and queries run in parallel over rayon.
Spatial queries are brute force by design; a note in `query.rs` explains where
a BVH would go if profiling ever calls for one.

## Build and run

Needs a recent stable Rust toolchain.

```bash
cargo run --release -p sculpt-app
```

## Controls

| Input | Action |
|-------|--------|
| Left mouse | Sculpt |
| Middle or right mouse | Orbit |
| Shift + middle mouse | Pan |
| Wheel | Zoom |
| Ctrl (hold) | Invert the brush |
| `[` / `]` | Brush radius |
| `X` | Toggle X symmetry |
| `W` | Toggle wireframe |
| `F` | Frame the model |
| Ctrl+Z / Ctrl+Shift+Z | Undo / redo |

## Tests

```bash
cargo test -p sculpt-core
```

The engine tests check the mesh invariants directly: adjacency stays consistent
through edge split and collapse, the icosphere satisfies the Euler
characteristic of a sphere, dynamic topology refines without corrupting the
mesh, undo restores the previous state, the raycast hits the unit sphere, and
an OBJ round trip preserves the geometry.

## Notes on origin

This is a clean-room implementation. It reuses well known, published techniques
(edge split and collapse for dynamic topology, a normal-indexed matcap, the
Wyvill falloff kernel, Moller-Trumbore ray casting) but no third-party
application code.

## License

MIT. See [LICENSE](LICENSE).
