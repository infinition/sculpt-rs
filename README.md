# sculpt-rs

A real-time 3D sculpting tool written from scratch in Rust, with dynamic
topology. Native desktop, rendered on the GPU through wgpu, with an interface
built for a pen and a touch screen as much as for a mouse.

![sculpt-rs](docs/screenshot.png)

## What it does

**Sculpting**

- **Dynamic topology.** Detail is created and removed under the brush by
  splitting and collapsing edges, so you sculpt without worrying about the
  starting resolution. Subdivision and decimation can be toggled separately,
  and a vertex ceiling keeps a runaway stroke from eating the machine.
- **Thirteen brushes.** Draw, Clay, Flatten, Smooth, Pinch, Crease, Inflate,
  Move, Drag, Twist, Scale, Paint and Mask. Each one remembers its own radius,
  strength and options, so switching tools never loses your settings.
- **Five falloff curves** per brush, previewed as a live graph: Smooth,
  Linear, Sharp, Sphere and Constant.
- **Brush behaviour**: front-face culling, a locked stroke plane for carving
  straight ridges, auto smoothing folded into the stroke, and pen pressure
  mapped to radius, strength or both.
- **Symmetry** across any axis, with the mirrored dab computed in the same
  pass as the original.
- **Masking** with blur, sharpen, invert and clear, plus extraction of the
  masked region into a separate solid object.

**Topology and repair**

- **Uniform subdivision**, plain midpoint or the Loop scheme.
- **Decimation** to a target fraction of the triangle count, guarded so the
  mesh stays a valid manifold.
- **Voxel remeshing** through a signed distance field and surface nets, which
  rebuilds a clean uniform shell out of whatever mess a long session produced.
- **Hole filling** on every boundary loop.
- **Mirror** and **symmetrize**, the latter clipping the triangles that
  straddle the plane and welding the seam.

**Scene**

- Several objects, each with its own placement, visibility and history.
- Select by clicking in the viewport, duplicate, delete, merge everything
  visible, bake a placement into the vertices.
- Undo and redo across geometry, placement and structural edits, bounded by a
  memory budget.

**Display**

- Five shading modes: matcap, a lit PBR view that reads the painted roughness
  and metalness, world normals, cavity, and untextured clay for judging form.
- Five procedurally generated matcaps, flat shading, wireframe, adjustable
  opacity, vertex colours.
- An infinite ground grid with tinted world axes, a gradient background you
  can recolour, and multisampling up to 8x.
- Perspective or orthographic camera with named views, adjustable field of
  view, orbit speed and smoothing.

**Files**

- Import and export OBJ (with vertex colours), PLY (binary and ASCII) and STL
  (binary and ASCII).
- A native `.sculpt` scene file that keeps every object, its placement and
  every per-vertex attribute.

## Architecture

Two crates:

- `sculpt-core` is the engine. No graphics dependency, so it runs in headless
  tests and could later be compiled to WebAssembly unchanged. It owns the
  scene, the mesh, adjacency, the spatial index, dynamic topology, brushes,
  topology commands, history and file I/O.
- `sculpt-app` is the desktop shell: wgpu renderer, winit window, egui
  interface, orbit camera, and pointer, pen and touch handling.

The mesh keeps its vertex and index arrays gap-free so they upload to the GPU
without a compaction pass. Removal uses `swap_remove` plus an index fixup for
the element that moved into the hole.

Sculpting is local but a naive query is not: every dab would otherwise scan the
whole model. `accel.rs` keeps an incremental spatial hash grid over vertices and
faces, sized so a sphere query always walks a constant number of cells whatever
the zoom level. Every mesh mutation that goes through the safe API keeps it in
sync; anything that rewrites the arrays wholesale drops it, and the next query
rebuilds. Ray casts walk the same grid front to back and stop as soon as the
nearest hit is certain. Brushes and the heavier commands run in parallel over
rayon.

The renderer draws every object from one uniform buffer addressed with dynamic
offsets, so a scene costs one bind group rebind per object and no buffer writes
inside the render pass. The background, the model and the ground grid are three
pipelines sharing one set of globals; the grid is a fullscreen triangle
intersected with the ground plane in the fragment shader, which gives an
infinite grid with correct depth and nothing to tessellate.

## Interface

The interface is built around three fixed places: the tool you are holding is
always on the left rail, its settings are always in the same panel on the right,
and the actions you reach for constantly sit in the top bar. Nothing is buried
in a menu.

Every control is drawn for a fingertip. The icons are vector shapes painted in
code rather than an icon font, so they stay crisp at any size and the binary
ships no assets. Sliders are full-width rails you can grab anywhere, with the
label and the value printed inside so a finger never covers the number it is
setting. One switch in the View tab grows every target for a touch screen, and
an interface scale slider takes it further.

## Build and run

Needs a recent stable Rust toolchain.

```bash
cargo run --release -p sculpt-app
```

## Controls

| Input | Action |
|-------|--------|
| Left mouse, one finger | Sculpt |
| Middle or right mouse | Orbit |
| Two fingers | Orbit, pinch to zoom, drag together to pan |
| Three fingers | Pan |
| Shift + middle mouse | Pan |
| Wheel | Zoom |
| Alt + left mouse | Orbit without sculpting |
| Ctrl (hold) | Invert the brush |
| Shift (hold) | Smooth instead of sculpting |
| `1` to `0`, Shift+`1`..`3` | Pick a tool |
| `[` / `]` | Brush radius |
| `-` / `=` | Brush strength |
| `X` / `W` / `G` | Symmetry, wireframe, grid |
| `F` | Frame the model |
| `Tab` | Show or hide the panel |
| `H` | Shortcuts |
| Numpad `1` / `3` / `7` | Front, right and top views |
| Numpad `5` | Perspective or orthographic |
| Ctrl+Z / Ctrl+Shift+Z | Undo and redo |

A second finger landing mid-stroke cancels the stroke rather than smearing the
model while the view swings around.

## Tests

```bash
cargo test -p sculpt-core
```

The engine tests check the invariants directly: adjacency stays consistent
through edge split and collapse, the icosphere satisfies the Euler
characteristic of a sphere, dynamic topology refines without corrupting the
mesh or the spatial index, the grid returns exactly what a brute-force scan
returns, undo restores the previous state, a symmetric stroke moves both sides
by the same amount, subdivision quadruples the face count, decimation reduces
it while staying valid, a voxel remesh comes back watertight, hole filling
seals an open plane, and OBJ, PLY and scene round trips preserve the geometry.

## Notes on origin

This is a clean-room implementation. It reuses well known, published techniques
(edge split and collapse for dynamic topology, Loop subdivision, surface nets
over a signed distance field, a normal-indexed matcap, the Wyvill falloff
kernel, Moller-Trumbore ray casting, spatial hashing) but no third-party
application code.

## License

MIT. See [LICENSE](LICENSE).
