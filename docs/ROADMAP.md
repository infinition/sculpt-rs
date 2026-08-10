# Roadmap

What this engine does today, what a full sculpting application does, and what
stands between the two.

The gaps come from a teardown of a shipping commercial sculptor: two builds, a
59 MB desktop binary and a 15 MB WebAssembly one, read through their exported
symbols, their 1098 configuration keys and their 150 shader sources. That
teardown says what a finished product contains. This file says which of it is
here, which is not, and in what order the rest is worth doing.

Every item is measured against the code, not against the report. Where an entry
says "none", the module genuinely does not exist.

| | |
|---|---|
| **done** | in the engine and covered by tests |
| **partial** | there, narrower than the product |
| **none** | not started |

Effort is rough calendar time for one person who already knows this codebase.
Priority is what it buys the person using the tool, not how interesting it is.

---

## 1. Mesh core

| Item | Status | Notes |
|---|---|---|
| Gap-free vertex and index arrays | **done** | `swap_remove` plus index fixup |
| Incremental vertex to face adjacency | **done** | `SmallVec` per vertex, rebuilt across cores |
| Edge split and collapse with manifold guards | **done** | link condition, normal flip rejection |
| Spatial hash grid, incremental | **done** | vertices and faces, parallel build |
| Cluster partition with frustum and cone culling | **done** | Morton ordered, `drift` driven reorder |
| Undo log by written slot | **done** | survives topology changes |
| **Per-channel storage with quantisation** | **none** | see below |
| UV channel | **none** | needed by texturing, baking, glTF |
| Opacity, density, face group channels | **none** | product has them, engine does not |
| Half-edge structure | **none** | current adjacency is enough for what exists |

### The one that matters: per-channel storage

A vertex is one 48 byte struct: position, normal, colour, mask, roughness,
metalness, interleaved. The product stores each channel in its own buffer,
quantised to what that channel needs, and does not allocate a channel that is
still all zeros.

For a model that has never been painted, that is 16 bytes a vertex against 48.
On five million vertices it is 80 MB rather than 240, and every pass that only
reads positions stops dragging colour and material through the cache with them.
Recomputing normals, the second most expensive part of a dab, reads three floats
a vertex instead of twelve.

It touches every file that names `Vertex`, which is most of the engine, plus the
history and the file formats. It is the largest single change left and the one
with the largest measured return.

*Effort: 1 to 2 weeks. Priority: highest.*

---

## 2. Sculpting

| Item | Status | Notes |
|---|---|---|
| 16 brushes | **done** | Draw, Clay, Flatten, Smooth, Pinch, Crease, Inflate, Move, Drag, Twist, Scale, Paint, Smudge, Blur, Fill, Mask |
| 5 falloff curves | **done** | live graph in the interface |
| Alphas | **done** | 6 generated, any image loadable, angle follows the stroke |
| Symmetry | **done** | any axis, mirrored dab in the same pass |
| Masking | **done** | blur, sharpen, invert, clear, extract to a solid |
| Pressure to radius and strength | **done** | |
| Dyntopo detail against world, brush or screen | **done** | |
| Dab spacing along a stroke | **partial** | fixed at a quarter of the radius; the product exposes it and defaults to a twentieth |
| Screen-space brush radius | **none** | the radius is world only; the product carries both and converts |
| Deferred displacement accumulation | **none** | the product sums stamps into one buffer and commits once; here a dab writes straight to positions |
| Hardness curve with editable points | **none** | falloff is one of five fixed curves |
| **Sculpt layers** | **none** | a layer is a named displacement over a base, blendable and separable. Large feature, high value, and it is what makes a morph target on export |
| Trim, Shape, Lathe, Project, Stamp tools | **none** | 2D drawn shapes extruded through the model |
| Hole tool | **none** | boolean, fill or legacy |
| Matrix and repeater tools | **none** | array, mirror, radial, along a curve |
| Measure tool | **none** | surface and volume |

*Layers: 1 week, high priority. Trim and Shape: 1 week, high. The rest: days each, low.*

---

## 3. Topology

| Item | Status | Notes |
|---|---|---|
| Dynamic topology | **done** | split and collapse, three detail modes |
| Uniform subdivision, midpoint and Loop | **done** | |
| Decimation to a target fraction | **done** | shortest edge first, guarded |
| Voxel remesh | **partial** | signed distance field plus surface nets, parallel. Dense grid capped near 400 cubed |
| Hole filling | **done** | every boundary loop |
| Mirror and symmetrize | **done** | clips and welds the seam |
| **Sparse voxel grid** | **none** | the product uses hash maps for the crossed edges, reaching 2048 cubed under 200 MB. Removes the resolution ceiling |
| Dual marching cubes with QEF | **none** | surface nets rounds sharp edges; the product solves a quadric per cell and keeps them |
| Manifold repair | **none** | no guarantee the result is closed and clean |
| Booleans | **none** | union, difference, intersection between objects |
| Quad remeshing | **none** | QuadriFlow or Instant Meshes, both published |
| Quadric error decimation | **none** | current decimation is by edge length, which loses shape on flat regions |
| Multiresolution levels | **none** | Catmull-Clark base plus levels, with a level lock |
| UV unwrapping | **none** | Boundary First Flattening |

*Sparse voxel grid and QEF: 1 week, high. Quadric decimation: 3 days, high, it
visibly improves every decimation. Booleans and manifold: 2 weeks. Quad remesh
and UV: 3 weeks each, and both are large published algorithms.*

---

## 4. Rendering

| Item | Status | Notes |
|---|---|---|
| Matcap, generated, editable and loaded | **done** | |
| Simple three-light PBR | **partial** | reads painted roughness and metalness, no image based lighting |
| Normals, cavity, unlit, clay views | **done** | |
| Wireframe, flat shading, opacity, vertex colours | **done** | |
| Infinite ground grid | **done** | plane intersection in the fragment shader |
| Multisampling to 8x, dropped when triangles are subpixel | **done** | |
| Two-stream quantised GPU vertex | **done** | 24 bytes, octahedral normal |
| Sparse GPU update through a compute scatter | **done** | |
| **Deferred GBuffer** | **none** | base colour, linear depth, packed normal, metal rough spec id |
| Environment lighting | **none** | spherical harmonics for diffuse, prefiltered cubemap plus split-sum for specular |
| Shadows | **none** | shadow map, screen space, contact |
| Texture channels and triplanar projection | **none** | no textures at all today |
| Subsurface, refraction, shadow catcher materials | **none** | |
| Post-processing chain | **none** | bloom, depth of field, SSAO or GTAO, SSR, SSGI, TAA |
| Tone mapping | **none** | the product ships 15 operators; one good one would do |
| Colour curves, LUT, grain, vignette, FXAA | **none** | |
| Per-cluster level of detail | **none** | see below |
| GPU culling with indirect draws | **none** | culling is on the CPU, one thread, every frame |

### Level of detail

Cluster culling throws away what is off screen and what faces away. What is left
is drawn at full density, so 25 million triangles over a million pixels shades
25 triangles per pixel.

Doing this per cluster naively cracks the surface: two neighbouring clusters at
different levels no longer share their boundary vertices. The published fix is a
hierarchy where boundaries are locked between levels, groups of clusters are
simplified together, and the level is chosen by projected error in pixels. It is
a real piece of work and it should not be started as a quick win.

*Tone mapping and SSAO: 3 days, high, they change how the model reads more than
anything else on this list. Environment lighting: 1 week, high. Deferred and the
full post chain: 3 weeks. Level of detail: 3 weeks, and only worth it above 20
million triangles.*

---

## 5. Scene

| Item | Status | Notes |
|---|---|---|
| Several objects, placement, visibility | **done** | |
| Select, duplicate, delete, merge, bake placement | **done** | |
| Per-object history | **done** | |
| Instances | **none** | duplicates are full copies |
| Repeaters | **none** | array, mirror, radial, along a curve |
| Face groups | **none** | named, coloured, selectable |
| Booleans between objects | **none** | listed under topology |

*Repeaters: 1 week, medium. Instances: 3 days, medium, and they pay for
themselves the moment repeaters exist.*

---

## 6. Files

| Item | Status | Notes |
|---|---|---|
| OBJ, with vertex colours | **done** | |
| PLY, binary and ASCII | **done** | |
| STL, binary and ASCII | **done** | |
| Native scene format | **partial** | every object, placement and per-vertex attribute, uncompressed |
| Brush sets as plain text | **done** | |
| PNG, JPEG, BMP, TGA in | **done** | alphas and matcaps |
| **LZ4 on the native format** | **none** | the product compresses each channel blob separately. A 5 million vertex scene writes 300 MB today |
| glTF and GLB | **none** | the format to add first: PBR materials, vertex colours, and morph targets carry a sculpt layer |
| FBX | **none** | read through `ufbx` |
| USD | **none** | large dependency, narrow audience |
| HDR and EXR in | **none** | needed by environment lighting |
| Draco and meshoptimizer | **none** | glTF compression |

*LZ4: 2 days, high, it is the difference between a scene that saves and one that
fills a disk. glTF: 1 week, high. FBX: 1 week, medium. USD: skip until asked.*

---

## 7. Interface and input

| Item | Status | Notes |
|---|---|---|
| Docked panels, tear-out floating windows | **done** | |
| Radial menu with a size and force pad | **done** | |
| Floating viewport buttons, orientation ball | **done** | |
| Vector icons drawn in code | **done** | no assets ship |
| Touch, pen and mouse, all rebindable | **done** | |
| Gestures: two and three finger, double tap undo | **done** | |
| Interface scale and touch targets | **done** | |
| Translations | **none** | the product ships 23 languages; every string here is inline English |
| Tablet tilt and rotation | **none** | pressure only |

*Translations: 1 week including pulling the strings out. Only worth it once the
feature set settles.*

---

## 8. What is deliberately not planned

- **Licence checks, activation, telemetry, cloud sync, store integration.** The
  product carries all of it. This does not, and starting is unconditional.
- **Final-image denoising.** The product links Intel OpenImageDenoise for
  offline renders. There is no offline renderer here to denoise.
- **A Qt shell.** The product only uses Qt for a third-party licence dialog.
- **Third-party quad remeshers.** A published algorithm implemented here is
  worth more than a DLL that needs its own activation server.

---

## Order of work

Ranked by what it returns for what it costs, not by chapter.

1. **Per-channel vertex storage.** Cuts memory by more than half and speeds
   every memory-bound pass. Everything else on this list gets cheaper after it.
2. **LZ4 on the native format.** Two days, and large scenes become saveable.
3. **Quadric error decimation.** Three days, and every decimation keeps its
   shape.
4. **Tone mapping and ambient occlusion.** Three days, and the model reads as a
   solid object instead of a shaded shell.
5. **Sparse voxel grid with QEF.** Removes the remesh resolution ceiling and
   stops rounding sharp edges.
6. **Sculpt layers.** The one missing idea that changes how the tool is used,
   and it is what a morph target export needs.
7. **glTF in and out.** The format everything else speaks.
8. **Environment lighting.** Spherical harmonics and a prefiltered cubemap.
9. **Trim and Shape tools.** Drawn 2D shapes cut through the model.
10. **Booleans and manifold repair.**
11. **Repeaters and instances.**
12. **Quad remeshing, then UV unwrapping.**
13. **Level of detail**, once a scene is regularly past 20 million triangles.

---

## Performance, for reference

Measured on five million triangles, six cores, by alternating two builds so the
machine's own noise lands on both. `cargo run --release -p sculpt-core --example
heavy 9`, and `--example dab_cost` for the breakdown of a single dab.

| | before | now |
|---|---|---|
| First dab of a stroke | 94 to 102 ms | 2.4 ms |
| 20 dabs | 55 to 59 ms | 43 ms |
| 10 strokes of 10 dabs | 277 to 288 ms | 200 ms |
| A dab with dynamic topology, on 1.3 M triangles | 30 to 41 ms | 17 to 18 ms |
| History for a stroke that cuts | 189 MB | 7 MB |
| Undo of that stroke | a full copy | 14 ms |
| Decimation to half | 4.6 s | 3.2 s |
| Voxel remesh, 20 k triangles at 128 | 476 ms | 67 ms |

What a dab costs now, on 1.3 million triangles: cutting and merging 9 ms,
normals 5.4 ms, moving the vertices 1.8 ms, finding the edges 1.4 ms. The
cutting is inherently sequential, since each split changes what the next one
sees. The normals are the next thing worth attacking, and per-channel storage is
how.
