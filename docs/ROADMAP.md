# Roadmap

A plan to take this engine from what it is to a complete sculpting application,
in order, with a definition of done for every step.

The target was established by a teardown of a shipping commercial sculptor: a
59 MB desktop binary and a 15 MB WebAssembly one, read through their exported
symbols, 1098 configuration keys, 150 shader sources and two decoded project
files. That says what a finished product contains. This file says which of it is
here, which is not, how each missing piece is built, and how we will know it
works.

Every status below was checked against the code in this repository, not copied
from the teardown. Where an item says none, the module genuinely does not exist.

Every part is named in our own vocabulary, set out in [NAMES.md](NAMES.md),
with a **rests on** line pointing at the published work it implements. The name
is ours; the citation is what keeps the clean room claim checkable.

---

## How we work

One item at a time, in the order given, each landing as its own commit.

**Nothing lands without a measurement or a test.** A performance claim is made
by building both versions and alternating between them so the machine's own
noise falls on each equally. A feature claim is made by a test that fails before
the change. Three of the changes already in this repository were measured,
found to be regressions, and reverted; that is the process working.

**Invariants that hold across every item:**

- *Portable.* Runs on Windows, macOS, Linux, and a tablet through the browser.
  No architecture intrinsics, no platform paths, no assumption about core count.
  The engine keeps no graphics dependency, so it compiles to WebAssembly
  unchanged.
- *Parallel by default.* Anything touching more than a few thousand elements is
  spread across cores, with a threshold below which it stays sequential because
  handing work to a pool is not free.
- *Deterministic.* The same input gives the same mesh. Where a parallel pass
  could reorder work, the result is sorted into a total order. One known
  exception is recorded in the open questions below.
- *No assets.* The binary ships no files. Icons are drawn in code, alphas and
  matcaps are generated.
- *Clean room.* Published techniques only, cited per item. No third-party
  application code, and no dependency that needs an activation server.

---

## Phase 0. What is already here

Recorded so this document stands alone.

**The mesh.** Gap-free position and face arrays. An incremental **ring**, the
faces around each vertex. Edge split and collapse with a link condition and a
normal flip guard. The **grid**, an incremental spatial hash over vertices and
faces, built across cores and never rebuilt mid-stroke. **Packets**, contiguous
runs of faces carrying a box and a normal cone, ordered along a Morton curve,
with a **drift** measure that decides when reordering is worth more than
continuing. The **log**, which remembers a stroke as the slots it wrote over and
survives live topology.

**Sculpting.** Sixteen brushes. Five falloff curves. Alphas, six generated and
any image loadable. Symmetry with the mirrored dab computed in the same pass.
Masking with blur, sharpen, invert, clear and extraction to a solid. Pressure
mapped to radius and strength. **Live topology** with the **detail measure** read
in world units, brush radii or pixels.

**Topology.** Even subdivision, midpoint and Loop. Decimation to a target
fraction. A remesh through the **field** and the **dual mesher**, every pass
parallel. Hole filling. Mirror and symmetrize.

**Rendering.** Matcap in three forms. A three-light view that reads painted
roughness and metalness. Normals, cavity, unlit and clay views. Wireframe, flat
shading, opacity, vertex colours. An infinite ground grid. Multisampling to 8x,
dropped automatically once triangles fall under a pixel. A GPU vertex of 24
bytes split into **hot and cold streams**. Sparse GPU updates through a compute
scatter.

**Files.** OBJ with vertex colours, PLY binary and ASCII, STL binary and ASCII,
a scene file, brush sets as plain text, PNG, JPEG, BMP and TGA in.

**Interface.** Docked panels that tear out into floating windows. A radial menu.
Floating viewport controls and an orientation ball. Vector icons drawn in code.
Touch, pen and mouse, all rebindable, with gestures.

**Verification.** 43 engine tests, 17 application tests, and an offscreen
example that builds the real GPU pipelines with no window, draws a sphere, reads
the pixels back and checks them.

### Measured, five million triangles, six cores

| | before | now |
|---|---|---|
| First dab of a stroke | 94 to 102 ms | 2.4 ms |
| 20 dabs | 55 to 59 ms | 43 ms |
| 10 strokes of 10 dabs | 277 to 288 ms | 200 ms |
| A dab with live topology, 1.3 M triangles | 30 to 41 ms | 17 to 18 ms |
| The log for a stroke that cuts | 189 MB | 7 MB |
| Undo of that stroke | a full copy | 14 ms |
| Decimation to half | 4.6 s | 3.2 s |
| Remesh, 20 k triangles at 128 | 476 ms | 67 ms |

Reproduce with `cargo run --release -p sculpt-core --example heavy 9`, and
`--example dab_cost` for the breakdown of a single dab.

---

## Phase 1. The mesh core

Everything downstream gets cheaper once this is done. Do it first.

### M1. Channels

**Why.** A vertex is one 48 byte struct with position, normal, colour, mask,
roughness and metalness interleaved. Every pass drags all of it through the
cache even when it reads three floats. Recomputing normals after a dab, now the
second most expensive part of one, is exactly that case. Splitting into one
buffer per attribute, each quantised to what it actually needs, and never
allocating a **dormant channel**, gives 16 bytes a vertex on a model that has
never been painted rather than 48: 80 MB rather than 240 on five million
vertices.

**Rests on.** Structure of arrays, and fixed point quantisation per attribute.

**Depends on.** Nothing.

**Shape.** A `Channel<T>` that is either dormant, holding no memory, or a
quantised buffer. Position stays three floats. Colour becomes four bytes,
roughness, metalness, opacity and density one byte each, the mask two. A channel
wakes on the first write that is not its default.

**Touches.** The mesh throughout, and every caller of the vertex struct: brushes,
topology, the log, files, primitives, packets, the GPU streams, and the tests.

**Done when.** Every existing test passes with unchanged behaviour. `heavy 9`
reports vertex memory under half of today's. The normals of a dab are measurably
faster on 1.3 M triangles. A mesh loaded with no painted colour reports its
colour channel dormant.

**Effort.** 1 to 2 weeks. **The highest return item in this document.**

### M2. The texture channel

**Why.** Nothing involving a texture can start without one: baking, glTF round
trips, triplanar projection, the whole material system.

**Rests on.** Standard UV parameterisation.

**Depends on.** M1.

**Shape.** Two floats per vertex, dormant by default. A seam is a split vertex,
so the **flattener** produces a new mesh rather than mutating one.

**Done when.** Texture coordinates round trip through the scene file and OBJ,
and a mesh without them costs nothing for the channel existing.

**Effort.** 3 days.

### M3. Opacity, density and group channels

**Why.** Opacity feeds transparent materials. Density records the local detail
target per vertex, so live topology can vary across one model. Groups let a
region be named, coloured, selected and hidden.

**Depends on.** M1.

**Done when.** All three round trip, and a group can be named, coloured,
selected and hidden.

**Effort.** 4 days.

### M4. Four-sided faces

**Why.** The **quad pass**, Catmull-Clark **levels** and the scene container all
assume four-sided faces. Retrofitting later means touching each of them twice.

**Rests on.** Standard quad-dominant mesh representation.

**Depends on.** M1.

**Shape.** A face becomes four indices with a sentinel for a triangle, and the
GPU gets a triangulated index buffer built alongside. The ring, split, collapse
and every topology command follow.

**Done when.** A four-sided mesh imports, sculpts, renders and exports without
ever being flattened to triangles on disk. The Euler characteristic tests extend
to cover it.

**Effort.** 1 to 2 weeks. Invasive, and it only pays once T6 or T8 are wanted.
Defer if neither is near.

### M5. The edge book

**Why.** The **mender** and **solid ops** need to walk from an edge to its faces
in constant time. Today that scans the ring.

**Rests on.** Half-edge connectivity, or a hashed edge key.

**Depends on.** M1.

**Done when.** Edge to face lookup is constant time, the invariants hold through
a live topology stroke, and no existing test gets slower.

**Effort.** 1 week.

---

## Phase 2. Files, so work can be kept

A scene that cannot be saved is a demo. This phase is cheap and it is what makes
the tool usable for real work.

### F1. Compress the channel store

**Why.** A five million vertex scene writes about 300 MB today. Compressing each
channel separately, and writing nothing at all for a dormant one, cuts that hard.

**Rests on.** LZ4 raw blocks. The uncompressed size is derived from count times
element size, so only the compressed length is stored.

**Depends on.** M1, which is what makes per-channel blocks natural.

**Done when.** A round trip is byte identical, a five million vertex scene
writes under a third of today's size, and a dormant channel occupies no bytes.

**Effort.** 2 days. **The best return per hour in this document.**

### F2. The scene container, version 2

**Why.** To carry everything the engine will grow: channels, materials, lights,
a hierarchy, groups, per-object topology settings.

**Shape.** A fixed binary header carrying offsets and lengths, then the
**manifest**, the readable part listing everything in the scene, then the
**channel store**, the packed blocks the manifest points into. Each channel entry
declares its count, offset, compressed length, type, and whether it is dormant.

**Depends on.** F1.

**Done when.** Version 1 files still open, version 2 round trips every object,
channel, material and light, and a truncated file is refused with a message
rather than a panic.

**Effort.** 4 days.

### F3. glTF and GLB

**Why.** The format everything else speaks, and the one that carries materials,
vertex colours and morph targets.

**Depends on.** M2.

**Done when.** A model exported here opens in Blender with its colours,
materials and texture coordinates intact, and one exported from Blender opens
here unchanged.

**Effort.** 1 week.

### F4. Layers as morph targets

One **layer** per morph target on export, which is how sculpted detail reaches a
rig. **Depends on.** F3 and S1. **Effort.** 2 days.

### F5. OBJ completion

Material libraries, groups, and an order-preserving marker so groups survive a
round trip. Optional zip. **Depends on.** M3. **Effort.** 3 days.

### F6. PLY channels

Roughness, metalness and mask as named properties, so a PLY carries what the
engine holds. **Depends on.** M1. **Effort.** 2 days.

### F7. FBX in

What arrives from other tools. Blend shapes map onto **layers**.
**Rests on.** A permissively licensed FBX parser. **Effort.** 1 week.

### F8. High range images in

**Why.** **Ambient harmonics** and the **roughness chain** need a high dynamic
range image to start from.

**Blocks.** R4. **Effort.** 2 days.

### F9. Mesh compression for glTF

A 25 million triangle glTF is unusable uncompressed.
**Rests on.** Draco, or vertex and index quantisation.
**Depends on.** F3. **Effort.** 4 days.

### F10. USD

Large dependency, narrow audience. Not planned until asked for.

---

## Phase 3. Topology

### T1. Shape cost collapse

**Why.** Decimation removes the shortest edge, which is fast and loses shape: a
flat region thins exactly as hard as a detailed one. Removing whichever edge
changes the surface least is what makes a decimated model still look like
itself.

**Rests on.** Error quadrics, Garland and Heckbert.

**Shape.** A quadric per vertex, summed from the planes of its faces. The cost
of a collapse is that quadric evaluated at the merged position. A heap ordered
by cost replaces the sort by length.

**Done when.** A sphere reduced to a tenth is still visibly a sphere, a cube
keeps its corners, and the validity test still passes. Both results recorded
side by side.

**Effort.** 3 days. **High return, low risk.**

### T2. The sparse field

**Why.** The **field** is a dense volume capped near 400 cells a side. Storing
only the cells the surface touches reaches 2048 a side inside 200 MB.

**Rests on.** Hashed sparse voxel storage.

**Done when.** A remesh at 1024 completes where 400 currently refuses, memory
stays under 200 MB, and the watertight test still passes.

**Effort.** 1 week.

### T3. The corner solve

**Why.** The **dual mesher** rounds every sharp edge. Solving for the corner a
cell's crossings imply keeps them, which is the difference between a remesh that
preserves a hard surface and one that melts it.

**Rests on.** Quadratic error functions, dual contouring, Ju et al.

**Depends on.** T2.

**Done when.** A remeshed cube keeps its edges, a remeshed sphere stays smooth,
and both stay watertight.

**Effort.** 1 week.

### T4. The mender

**Why.** Nothing downstream can assume a clean surface, and **solid ops** in
particular fail loudly on a broken one.

**Rests on.** Standard manifold repair.

**Depends on.** M5.

**Done when.** A deliberately broken mesh, with a fin, a non-manifold vertex and
a flipped face, comes back manifold and closed.

**Effort.** 1 week.

### T5. Solid ops

Union, difference and intersection between objects, which is how hard surface
work is done. **Depends on.** T4. **Effort.** 2 weeks.

### T6. The quad pass

**Why.** Triangles are for sculpting, four-sided faces are for everything after.

**Rests on.** Field aligned remeshing, Jakob et al., or QuadriFlow, Huang et al.

**Depends on.** M4. **Effort.** 3 weeks.

### T7. The flattener

**Rests on.** Boundary first flattening, Sawhney and Crane, or least squares
conformal maps as a smaller first step.

**Depends on.** M2, M5. **Effort.** 3 weeks.

### T8. Levels

**Why.** Sculpt coarse, subdivide, sculpt fine, and go back down without losing
the fine work. A different way of working from live topology, and a lot of
people prefer it.

**Rests on.** Multiresolution sculpting over a Catmull-Clark cage.

**Depends on.** M4. **Effort.** 2 weeks.

### T9. Settling

**Why.** A remesh loses the surface detail of the original. Projecting the new
vertices back onto the old surface recovers most of it.

**Rests on.** Ray projection along the normal, with relaxation, iterated.

**Depends on.** T2. **Effort.** 3 days.

---

## Phase 4. Sculpting

### S1. Layers

**Why.** The single missing idea that changes how the tool is used. A named
displacement over a base, with a weight, that can be turned off, blended,
reordered and pulled out into its own object. It is also what a morph target
export needs.

**Depends on.** M1.

**Shape.** A layer holds a sparse displacement per vertex, valid while the
topology holds still. A topology change either bakes the layers down or
resamples them, and which one happens must be stated to the person using it
rather than decided quietly.

**Done when.** A layer can be added, sculpted into, weighted, hidden, reordered,
merged down and extracted, and an undo of any of those is exact.

**Effort.** 1 week. **The highest user-facing return in this phase.**

### S2. The dab buffer

**Why.** A dab writes straight into positions, so two stamps overlapping inside
one stroke step compound instead of combining. Gathering displacement first and
applying the sum once fixes it and parallelises cleanly.

**Depends on.** M1. **Effort.** 3 days.

### S3. Radius measured on screen

The radius is world only, so the brush changes apparent size as you zoom. The
conversion already exists for the **detail measure**; generalise it and let a
brush say which it uses. **Effort.** 2 days.

### S4. Spacing along a stroke

Fixed at a quarter of the radius. Expose it, default lower.
**Effort.** half a day.

### S5. An editable falloff

Five fixed curves today. A curve with movable points and a hardness exponent,
with the editor to shape it. **Effort.** 3 days.

### S6. Trim and cut-out

**Why.** Draw a rectangle, a lasso or a polygon on screen, push it through the
model, and cut. It is how hard surface silhouettes get made.

**Depends on.** T5 for the clean version, though a first pass can cut and cap
without full **solid ops**. **Effort.** 1 week.

### S7. Revolve

Draw a profile, turn it around an axis. **Effort.** 3 days.

### S8. Project and stamp

Press a stored buffer or an image onto the surface. **Effort.** 4 days.

### S9. Holes

Cut or fill a hole, by **solid ops** or by closing the boundary loop.
**Depends on.** T5 for the first method. **Effort.** 3 days.

### S10. Measure

Surface area and volume, with a readout. **Effort.** 1 day.

### S11. Repeats

Array, mirror, radial and along a curve, live rather than baked.
**Depends on.** A2. **Effort.** 1 week.

### S12. Group selection

Select, name, colour, hide and isolate groups of faces.
**Depends on.** M3. **Effort.** 4 days.

---

## Phase 5. Rendering

Ordered so the model reads better as early as possible, then by cost.

### R1. The tone curve

**Why.** Colour goes straight to an sRGB target with no curve, so a highlight
clips flat. One good curve changes how every view reads.

**Rests on.** ACES fitted, or AGX. Exposure, contrast and saturation on top.

**Done when.** A bright highlight rolls off instead of clipping, checked in the
offscreen example.

**Effort.** 2 days. **The best visual return per hour.**

### R2. Horizon sweep

**Why.** Occlusion in creases is most of what makes a sculpt read as a solid
object rather than a shaded shell. The cavity view approximates it from normal
derivatives; this is the real thing.

**Rests on.** Ground truth ambient occlusion, Jimenez et al., with a bilateral
blur.

**Effort.** 3 days.

### R3. The surface pass

**Why.** The prerequisite for every screen-space effect after it.

**Rests on.** Deferred shading. Four **surface targets**: colour, linear depth,
packed normal, and the material terms. The octahedral packing already written
for the vertex stream applies unchanged.

**Blocks.** R5, R8. **Effort.** 1 week.

### R4. Ambient harmonics and the roughness chain

**Why.** Three hard-coded lights is a placeholder. Lighting a model from an
image is what makes a material look like a material.

**Rests on.** Spherical harmonic irradiance, Ramamoorthi and Hanrahan, for the
diffuse. Prefiltered importance sampling and the split sum approximation, Karis,
for the specular and the **reflectance table**. All three are computed on load,
never shipped.

**Depends on.** F8. **Effort.** 1 week.

### R5. Shadows

A shadow map for directional and spot lights, a screen-space march for the
middle range, and **contact shading** for the short.
**Depends on.** R3, A3. **Effort.** 1 week.

### R6. Textures

Colour, roughness, metalness, opacity and normal maps, projected through texture
coordinates or triplanar. **Depends on.** M2. **Effort.** 1 week.

### R7. Material kinds

Opaque, additive, blended, dithered, refractive, subsurface and shadow catcher,
as the scene container declares. **Depends on.** R3, R6. **Effort.** 2 weeks.

### R8. The effect chain

Bloom, depth of field, **screen reflections**, **screen bounce**, and **frame
accumulation**. **Depends on.** R3. **Effort.** 3 weeks.

### R9. The final pass

Colour curves, a lookup table, grain, scanline, vignette, sharpen, **edge
smoothing**, and blue noise dithering. **Depends on.** R1. **Effort.** 1 week.

### R10. The packet tree

**Why.** Culling throws away what is off screen and what faces away. What
remains draws at full density, so 25 million triangles over a million pixels
shades 25 triangles per pixel.

**Rests on.** Virtualised geometry. The naive version cracks the surface: two
neighbouring packets at different **detail levels** stop sharing their boundary
vertices. The fix locks boundaries between levels, simplifies groups of packets
together, and picks a level by projected error in pixels.

**Depends on.** T1, whose quadrics are what the simplification needs.

**Done when.** A 25 million triangle model at arm's length draws under a million
triangles with no visible seam, checked in the offscreen example against a full
density render.

**Effort.** 3 weeks. **Not a quick win.** Only worth it once scenes are
regularly past 20 million triangles.

### R11. Culling on the GPU

Culling runs on one CPU thread over every packet, every frame. A compute pass
writing an indirect buffer removes that, and the gap-merging heuristic with it.
**Depends on.** R10 to be worth it. **Effort.** 1 week.

### R12. Outline, xray, coloured backfaces

Shader variants, cheap, each a real aid while sculpting. **Effort.** 3 days.

---

## Phase 6. Scene and application

### A1. Hierarchy

Nodes with children, a pivot and a transform. The scene is a flat list today.
**Effort.** 4 days.

### A2. Shared meshes

A duplicate is a full copy. A shared mesh with its own transform is what makes
**repeats** affordable. **Depends on.** A1. **Effort.** 3 days.

### A3. Lights

Directional, point, spot and environment, with colour, temperature, intensity,
size, angle and softness, and per-light shadow settings.
**Depends on.** A1. **Blocks.** R5. **Effort.** 4 days.

### A4. Materials as objects

A material assigned per object rather than settings on the renderer.
**Depends on.** R6. **Effort.** 3 days.

### A5. Translations

Every string is inline English. Pull them into a catalogue and load a language
at start. **Effort.** 1 week, and only once the feature set settles.

### A6. Tilt and barrel rotation

Pressure only today. Tilt shapes the brush, rotation turns the alpha.
**Effort.** 2 days.

### A7. Turntable export

Rotate, render, write an animation with an alpha silhouette. **Effort.** 3 days.

---

## What is deliberately not planned

- **Licence checks, activation, telemetry, cloud sync, store integration.** The
  analysed product carries all of it. Starting here is unconditional and stays
  that way.
- **Denoising a final image.** The product links a denoiser for offline renders.
  There is no offline renderer here to denoise. Revisit only if R8 grows into a
  path traced export.
- **A second UI toolkit.** The product uses one only for a third-party licence
  dialog.
- **Third-party remeshers.** A published algorithm implemented here is worth
  more than a library that needs its own activation server.
- **USD.** Large dependency, narrow audience, nobody has asked.

---

## Order

Dependencies first, then return for effort. Phases overlap where they do not
depend on each other; the rendering items can proceed alongside the topology
ones given a second pair of hands.

| # | Item | Why here | Effort |
|---|---|---|---|
| 1 | M1 channels | everything downstream gets cheaper | 1-2 wk |
| 2 | F1 compress the channel store | 2 days, and large scenes become saveable | 2 d |
| 3 | T1 shape cost collapse | 3 days, every decimation keeps its shape | 3 d |
| 4 | R1 the tone curve | 2 days, changes how everything reads | 2 d |
| 5 | R2 horizon sweep | 3 days, the model becomes solid | 3 d |
| 6 | M2 texture channel | unblocks texturing and glTF | 3 d |
| 7 | F2 scene container v2 | before the scene grows further | 4 d |
| 8 | S1 layers | the missing idea | 1 wk |
| 9 | T2 the sparse field | removes the resolution ceiling | 1 wk |
| 10 | T3 the corner solve | stops rounding sharp edges | 1 wk |
| 11 | F3 glTF | the format everything speaks | 1 wk |
| 12 | F8 high range images | unblocks lighting from an image | 2 d |
| 13 | R4 ambient harmonics | materials start looking like materials | 1 wk |
| 14 | M3 extra channels | opacity, density, groups | 4 d |
| 15 | A1 hierarchy | before shared meshes and lights | 4 d |
| 16 | A3 lights | unblocks shadows | 4 d |
| 17 | R3 the surface pass | unblocks every screen-space effect | 1 wk |
| 18 | R5 shadows | | 1 wk |
| 19 | M5 the edge book | unblocks the mender and solid ops | 1 wk |
| 20 | T4 the mender | | 1 wk |
| 21 | T5 solid ops | | 2 wk |
| 22 | S6 trim and cut-out | hard surface work becomes possible | 1 wk |
| 23 | R6 textures | | 1 wk |
| 24 | S2 to S5, S7 to S12 | small tools, days each | 3 wk |
| 25 | A2, A4, A6, A7 | | 2 wk |
| 26 | R8 the effect chain, R9 final pass | | 4 wk |
| 27 | R7 material kinds | | 2 wk |
| 28 | M4 four-sided faces | only if T6 or T8 are wanted | 1-2 wk |
| 29 | T6 the quad pass | | 3 wk |
| 30 | T7 the flattener | | 3 wk |
| 31 | T8 levels | | 2 wk |
| 32 | R10 the packet tree, R11 GPU culling | once scenes pass 20 M triangles | 4 wk |
| 33 | F7 FBX, F9 mesh compression, A5 translations | | 3 wk |

Roughly nine months for one pair of hands, and the first five items, about three
weeks together, carry most of the day-to-day improvement.

---

## Open questions

Known, unresolved, and written down so they are not rediscovered.

1. **A dab is not identical across core counts.** A stroke lands on 7639 new
   vertices on six cores and 7649 on one, each repeatable. The edge search was
   proved not to be the cause: its output is sorted into a total order and is a
   pure function of the mesh. Something else in a dab still follows how the work
   was divided.
2. **The packet reorder still stalls between strokes.** 185 ms on five million
   triangles, of which 60 ms is rebuilding the ring. Doing it on a worker thread
   against a snapshot would hide it, at the cost of a frame of staleness.
3. **The ring costs 48 bytes a vertex.** Eight face indices held inline. A
   packed row layout would halve it but cannot be mutated in place, which live
   topology needs constantly.
4. **Hardware backface culling is off.** The comment says a locally inverted
   surface would show holes. The packet normal cone already discards
   back-facing packets, so the remaining gain is smaller than it looks, and it
   has never been measured.
