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

- *Portable.* Windows, Linux, macOS, iPadOS, and the browser, with iOS behind
  the same work as iPadOS. No architecture intrinsics, no platform paths, no
  assumption about how many cores there are. The engine keeps no graphics
  dependency, so it compiles to WebAssembly unchanged. Phase 7 is where each
  platform actually gets stood up and kept working.
- *Fastest path the machine offers.* Every backend the device supports is
  available and the best one is chosen at start: Direct3D 12 or Vulkan on
  Windows, Vulkan on Linux, Metal on Apple, WebGPU in the browser with WebGL 2
  behind it. Features are asked for and degraded, never assumed. A phone-class
  GPU and a workstation card run the same code down different paths.
- *Parallel by default.* Anything touching more than a few thousand elements is
  spread across cores, with a threshold below which it stays sequential because
  handing work to a pool is not free. Work that belongs on the GPU goes to the
  GPU: the sparse vertex update is already a compute pass, and culling follows
  in R11.
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

**The mesh.** **Channels**: every vertex attribute in its own array,
quantised to what it needs, and a **dormant channel** costs nothing at all.
Twenty-four bytes a vertex on a model nobody has painted, against forty-eight
interleaved. Gap-free position and face arrays. An incremental **ring**, the
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
roughness and metalness, through a **tone curve** with exposure, contrast and
saturation. **Horizon sweep** occlusion, which darkens a crease and leaves a
convex surface alone. Normals, cavity, unlit and clay views. Wireframe, flat
shading, opacity, vertex colours. An infinite ground grid. Multisampling to 8x,
dropped automatically once triangles fall under a pixel. A GPU vertex of 24
bytes split into **hot and cold streams**. Sparse GPU updates through a compute
scatter.

**Files.** OBJ with vertex colours, PLY binary and ASCII, STL binary and ASCII,
the **scene container** version 2, which writes each channel as its own
compressed block and a dormant one as four bytes, brush sets as plain text,
PNG, JPEG, BMP and TGA in. Version 1 files still open, and a truncated one is
refused with a message.

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
| Vertex memory, nothing painted | 126 MB | 63 MB |
| The whole mesh, same model | 315 MB | 252 MB |

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

**Effort.** 1 to 2 weeks. **Done.** Twenty-four bytes a vertex against
forty-eight, sixty-three megabytes against a hundred and twenty-six on five
million triangles. Speed unchanged within the noise at that size, and the
spread of the normals pass roughly halved.

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

**Effort.** 2 days. **Done**, and it took the shape of F2 with it, since the
channels made per-block writing the natural thing to do. Version 1 files still
open. What remains under F2 is the manifest: materials, lights and a hierarchy,
none of which exist yet to write.

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

### F11. Baking

**Why.** Sculpted detail has to reach a renderer that will not take twenty
million triangles. Baking writes it into images over a low mesh.

**Rests on.** Ray cast from the low surface along its normal into the high one,
with a cage offset and edge bleed.

**Shape.** Colour, normal, roughness, metalness, opacity, occlusion, group and
mask, at a chosen size, with bleed past the island borders so filtering does not
pull in background.

**Depends on.** M2, T7.

**Done when.** A baked normal map applied to the low mesh reproduces the high
one at a glance, and no seam shows at an island border.

**Effort.** 1 week.

### F12. Autosave

**Why.** A session that dies takes the work with it. There is no recovery today.

**Shape.** Write the scene container to a rotating slot on a timer and after
every heavy command, and offer it at start if the last exit was not clean.

**Depends on.** F2. **Effort.** 2 days.

### F13. More images in

WebP, GIF and PSD arrive as reference and as alphas. Low priority next to the
formats already read. **Effort.** 3 days.

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

**Effort.** 3 days. **Attempted and reverted; read this before starting
again.**

The straightforward version, a quadric evaluated at the midpoint of the edge,
is worse than what it replaces. Measured on a cube of 432 triangles, reach from
the centre to the furthest vertex, which is the corner while the corner is
still there:

| kept | quadric at the midpoint | shortest edge |
|---|---|---|
| 60% | 1.7321 | 1.7321 |
| 40% | 1.6008 | 1.7321 |
| 30% | 1.6008 | 1.7321 |
| 20% | 1.5635, worst deviation 0.258 | 1.7321, worst deviation 0.188 |

On a sphere at 15% it was marginally better, 0.0040 against 0.0051. Losing the
corners of a cube to gain four thousandths on a sphere is not a trade worth
making.

The reason is the forced position. `collapse_edge` puts the merged vertex at
the midpoint, so that is where the error has to be measured, and a corner
measured from the midpoint of one of its edges looks cheap enough to spend.
Real quadric decimation solves for the position that minimises the error, which
for a corner is the corner itself, and only then is its cost properly enormous.
Shortest edge meanwhile treats a uniform grid uniformly and spares the corner by
accident.

So this item now depends on `collapse_edge` taking a target position rather
than always using the midpoint, which is a change to the mesh and to what
dynamic topology asks of it. Worth doing, larger than three days, and pointless
without it.

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

### T10. Quad tidying

**Why.** A remesh leaves quads whose diagonals run against the surface flow,
which shades badly and subdivides worse.

**Rests on.** An in-circle test per quad, flipping the diagonal when the other
one is the better triangulation.

**Depends on.** T3. **Effort.** 3 days.

### T11. Surface remesh

**Why.** Not every cleanup wants a full rebuild through the field. Sometimes the
answer is to even out the triangles that are already there.

**Shape.** Three modes over the existing surface: decimate, subdivide, or
even out edge lengths in place, all of them respecting the mask so a region can
be left alone.

**Depends on.** T1. **Effort.** 4 days.

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

**Half of it is done.** How many dabs a pointer movement is cut into is no
longer a fixed thirty-two: it is whatever fits in ten milliseconds, measured
from what the last dabs actually cost. A dab on a light mesh costs microseconds
and thirty-two are free; on ten million triangles with live topology one costs
most of twenty milliseconds, and thirty-two was half a second of frozen window
for one movement of the pen. On a heavy mesh the dabs now land a little further
apart instead. What is left of this item is exposing the spacing itself.

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

### S13. Radial and limited symmetry

**Why.** Symmetry is one mirror plane. The product also repeats radially around
an axis, limits symmetry to part of the model, and works in local or world
space. A radial pattern is otherwise built by hand.

**Shape.** A count and an offset per axis for the radial case, a plane or a
masked region for the limited case, and a switch between the object's own axes
and the world's. Cutting along the mirror, and splitting into halves, follow
from the same plane.

**Effort.** 4 days.

### S14. Extraction, properly

**Why.** Extraction lifts a masked region into a solid. The product offers what
happens at the border and what the result is made of.

**Shape.** Shell, fill, layer or nothing at the border; carve inwards rather
than outwards; polish the border, the whole thing, or just the sharp parts; a
thickness; an edge loop; and a preview before it commits.

**Depends on.** T4 for a clean border. **Effort.** 4 days.

### S15. A remesh brush

**Why.** Remeshing is all or nothing. Painting where the density should go, then
remeshing to that, is how detail gets spent where it matters.

**Shape.** A brush writing the density channel, and a remesh that reads it.

**Depends on.** M3, T2. **Effort.** 4 days.

### S16. Selecting by mask

Grow, shrink, select by cavity, by group, by connected region, by angle. The
mask exists; the ways of making one do not. **Depends on.** M3.
**Effort.** 3 days.

### S17. More primitives

Six today: sphere from an icosahedron, sphere from a grid, cube, cylinder,
torus, plane. Missing: cone, tube, a cube projected to a sphere, and a surface
revolved from a drawn profile. **Depends on.** S7 for the last.
**Effort.** 3 days.

---

## Phase 5. Rendering

Ordered so the model reads better as early as possible, then by cost.

### R1. The tone curve

**Why.** Colour goes straight to an sRGB target with no curve, so a highlight
clips flat. One good curve changes how every view reads.

**Rests on.** ACES fitted, or AGX. Exposure, contrast and saturation on top.

**Done when.** A bright highlight rolls off instead of clipping, checked in the
offscreen example.

**Effort.** 2 days. **Done.** Pushed three stops, the lit view without the
curve puts 24951 pixels at pure white and loses the form inside them; with it,
none. Exposure, contrast and saturation sit next to it.

Only the lit view goes through the curve, and that is deliberate. A matcap is
an image somebody already graded, the normals view is data rather than light,
and the unlit view exists precisely to show a painted colour untouched. A curve
on any of those changes an answer rather than shaping a picture. It becomes the
one that matters once R4 gives the lit view a real range to map.

### R2. Horizon sweep

**Why.** Occlusion in creases is most of what makes a sculpt read as a solid
object rather than a shaded shell. The cavity view approximates it from normal
derivatives; this is the real thing.

**Rests on.** The horizon-based approach of Bavoil and Sainz. The ground truth
integral of Jimenez et al. is the fuller version of the same idea and would
replace the estimator without touching anything around it.

**Effort.** 3 days. **Done.** A pass of its own after the scene one, reading
the depth buffer and multiplying what it finds onto the frame. Measured in the
offscreen example: a lone sphere, convex everywhere and unable to occlude
itself, has 0.0% of its pixels darkened; the crevice between two overlapping
spheres has 1.0%. A crevice is narrow by nature, and darkening it and nothing
else is the whole point.

Two things it does not have yet. There is no bilateral blur, so the estimate
carries some of the noise of its own sampling; eight directions and a per-pixel
rotation keep it below what shows on a matcap, and a blur wants a second target
to write into, which is R3. And the normal is rebuilt from the depth rather than
read from a pass that wrote it, which is exact on a flat surface and slightly
soft on a silhouette. R3 gives it a real one.

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

### R13. Shader variants

**Why.** There are seven shaders today and no way to share code between them.
The rendering phase multiplies that by every material kind, every light type and
every optional effect. Pasting the same twenty lines into each is how a renderer
rots.

**Shape.** Text inclusion and conditional blocks resolved before a shader
reaches the driver, with a compiled variant cached per set of flags. The product
does exactly this, down to a repeat directive that emits one block per light.

**Depends on.** Nothing. **Blocks.** R5, R7 in practice. **Effort.** 4 days.

### R14. Texture encodings and GPU compression

**Why.** An environment at high range costs four bytes a channel unless it is
encoded, and an uncompressed texture set will not fit on a tablet.

**Rests on.** RGBM and its relatives for range in eight bits per channel. ASTC
where the device offers it, BC7 elsewhere, both detected rather than assumed.

**Depends on.** R4, R6. **Effort.** 1 week.

### R15. The rest of the final pass

Chromatic aberration, a curvature pass that reads creases and ridges, and a
pixel art mode that renders low and holds the grid. Small, and each is a look
somebody wants. **Depends on.** R9. **Effort.** 4 days.

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

### A8. Settings that persist

**Why.** Bindings, theme, interface scale and every preference are lost on exit.
Nothing is written anywhere.

**Shape.** A versioned settings file next to the scene container, written on
change and read at start, with an unreadable one falling back to defaults rather
than refusing to launch.

**Effort.** 3 days. **Small, and it is felt on the second launch.**

### A9. The rest of the gestures

Palm rejection, so a hand resting on a tablet is not a stroke. An air stroke, so
a pen that leaves the surface mid-line does not break it. Four fingers to shrink
the interface out of the way. **Effort.** 4 days.

### A10. Snapping

The grid is drawn but nothing snaps to it. Position, rotation and scale to the
grid or to an increment, and a plane to work on. **Effort.** 3 days.

### A11. Script coverage for text

**Why.** A translation is worthless if its glyphs do not draw. Arabic, Hebrew,
Thai, Japanese, Korean and both Chinese sets need font coverage and, for two of
them, right to left layout.

**Depends on.** A5. **Effort.** 1 week, and it is the real cost of translation.

### A12. Long operations without a frozen window

**Why.** A remesh at high resolution, a bake or a boolean can take a minute.
The window is unresponsive for all of it, and there is no way to tell what is
happening or to stop it.

**Shape.** Heavy commands run off the interface thread against a snapshot,
report progress by stage, and can be cancelled. The stages the product shows are
loading, binding, atlas, baking, voxel, boolean and remesh.

**Effort.** 1 week.

---

## Phase 7. Platforms and input

Nothing in the teardown covers this: the analysed product runs on Windows,
macOS and the web, and has no six degree of freedom device support at all. These
items are ours.

Two of them, P1 and P10, should be done early. A platform that is only stood up
at the end is a platform that never works.

### P1. Linux

**Why.** The cheapest platform to add, and the one most likely to be already
working: `wgpu` speaks Vulkan and `winit` speaks Wayland and X11.

**Shape.** Build, run and pass the offscreen check on both display servers.
Confirm the file dialogs, which go through a portal on Wayland and need one
installed. Confirm pen and touch, which arrive through libinput and not through
the same path as Windows.

**Done when.** The offscreen example passes on Wayland and on X11, in continuous
integration, on llvmpipe so it runs without a GPU present.

**Effort.** 3 days. **Do this early.**

### P2. macOS

**Why.** Metal is a first class `wgpu` backend and half the sculpting audience
is there.

**Shape.** A bundle with an icon and the usual metadata. Trackpad gestures,
which arrive differently from a touch screen. The system file dialogs.
Distribution needs signing and notarisation, which is an account and a
certificate rather than code.

**Done when.** It launches from a bundle on an Apple silicon machine, the
offscreen check passes on Metal, and a pinch on the trackpad zooms.

**Effort.** 4 days, plus whatever notarisation costs in paperwork.

### P3. The browser

**Why.** The engine was written with no graphics dependency precisely so this
would be possible, and it is how the tool reaches a tablet without an app store.

**Shape.** The engine compiles to WebAssembly unchanged. The shell needs a
canvas, `wgpu` on WebGPU with WebGL 2 behind it, files through the browser
rather than the disk, and pointer events for pen and touch.

**The hard part is threads.** Parallelism in the browser needs shared memory,
which needs cross-origin isolation headers on whoever serves the page. Without
them everything still runs, on one core. That has to be a supported
configuration, not a broken one, which means every threshold in the engine has
to behave when there is exactly one worker.

**Done when.** A model loads, sculpts and exports in a browser on WebGPU and
again on WebGL 2, and the same page still works with isolation headers absent,
slower and correct.

**Effort.** 2 to 3 weeks. **Depends on.** P10.

### P4. iPadOS, and iOS behind it

**Why.** A pen on a screen is the reason the interface was built the way it was.

**Two routes, and they are not equivalent.** The browser build of P3 reaches an
iPad with no app store and no account, and it cannot have pencil hover, the
double tap, the squeeze or the barrel roll, because Safari does not expose them.
A native shell can have all of it: a Rust static library under a thin platform
host that owns the window, the touch handling and the pencil.

**Shape.** The native route: the engine and the renderer build for the platform,
the host owns the view and forwards events into the same input layer the desktop
uses. Memory is the real constraint, not speed, which makes M1 and F1 load
bearing here rather than merely worthwhile.

**Done when.** A model of a few million triangles sculpts at a usable frame rate
on a recent iPad without the system killing the process for memory.

**Effort.** 3 weeks for the native shell after P3, which is why P3 comes first
even for people who want the native one.

### P5. Pen hover

**Why.** A pen that reports where it is before it touches lets the brush ring
follow it. Every sculpting tool worth using does this, and its absence is felt
immediately by anyone who has used one.

**Shape.** Proximity arrives differently everywhere: through the pointer API on
Windows, through libinput on Linux, and through the pencil interaction on Apple
hardware that has it. One event in our input layer, several ways of producing
it, and everything above it unchanged.

**Depends on.** The platform it is being read on. **Effort.** 4 days per
platform family.

### P6. Apple Pencil gestures

**Why.** The double tap, the squeeze and the barrel roll are how a pencil
changes tool without reaching for the screen.

**Shape.** All three are surfaced only through the platform's pencil
interaction, so they need the native shell of P4. They map onto actions in the
binding system like any other input, so nothing above the input layer knows they
are special. Barrel roll drives the alpha angle, which already exists for tilt.

**Depends on.** P4, A6. **Effort.** 3 days once the native shell exists.

### P7. Stylus buttons everywhere else

Barrel buttons on a graphics tablet, and the eraser end. Some arrive as mouse
buttons, some through a tablet driver protocol. They should reach the binding
system as themselves, not as a right click.

**Effort.** 4 days.

### P8. Six degree of freedom devices

**Why.** A SpaceMouse in the off hand while the pen is in the other is how the
model gets turned without ever stopping the stroke. There is no other input that
changes sculpting ergonomics as much.

**Rests on.** The device is a plain USB human interface device. Its reports
carry six signed axes, three of translation and three of rotation, plus its
buttons. Reading the raw reports works identically on Windows, macOS and Linux
and needs no vendor runtime, no driver install and no licence. The vendor
software offers a higher level path on two of the three platforms; taking it
would mean a dependency that we do not control on a device that does not need
one.

**Shape.** A device layer that opens anything matching the known vendor and
product identifiers, decodes the axis reports, applies a deadzone and a
per-axis curve, and feeds the camera. Translation pans and dollies, rotation
orbits, and the whole thing can be locked per axis for people who want two of
the six. Buttons reach the binding system. Hot plug is handled: a device
appearing mid-session is picked up, one disappearing is not an error.

**Done when.** A SpaceMouse orbits and pans on all three desktop platforms while
a stroke is in progress and the stroke is unaffected, and unplugging it
mid-stroke changes nothing.

**Effort.** 1 week. **Medium priority, high delight.**

### P9. One input layer

**Why.** Mouse, pen, touch, gestures, keyboard, gamepad and now a six axis
device all arrive differently on five platforms. Without one place where they
become actions, every feature above pays for the difference.

**Shape.** Devices produce events, events map to actions through the bindings,
actions are what the application handles. The mapping already exists for
keyboard and pointer; this is extending it to cover everything and keeping the
platform differences underneath it.

**Depends on.** A8 to keep the bindings. **Effort.** 1 week.

### P10. Backend selection and capability degradation

**Why.** The same code has to run on a workstation card and on a tablet. Today
the adapter is asked for its limits and they are taken; there is no fallback if
a feature is missing, and no way to choose a backend.

**Shape.** Rank the backends the platform offers and take the best that starts.
Ask for features, notice which were refused, and take the cheaper path where one
was: no compute scatter without compute, no wireframe without polygon mode, a
smaller vertex ceiling on a smaller buffer limit. Report what was chosen and
what was degraded, in the interface, where somebody can read it.

**Done when.** Forcing each backend in turn produces a working window or a clear
message, and forcing a minimal feature set still sculpts.

**Effort.** 1 week. **Do this early: it is what makes P3 and P4 possible at
all.**

### P11. Keeping every platform working

**Why.** Five platforms and one pair of hands is how platforms quietly rot.

**Shape.** Continuous integration builds every target and runs the tests. The
offscreen example runs wherever a software rasteriser is available, which makes
it a real rendering check and not just a compile. A performance run on a known
model, recorded per platform, so a regression is visible as a number.

**Effort.** 4 days, then it pays for itself.

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
- **Automatic updates and a shell thumbnail handler.** The product ships both.
  Both are packaging concerns, and both are per-platform work that buys nothing
  for the engine.
- **An interface drawn from a sprite atlas.** The product draws its own widgets
  in OpenGL from an atlas of 453 icons. We draw ours in code through egui, which
  is why the binary ships no assets at all. A deliberate divergence, not a gap.
- **An octree or a bounding volume hierarchy for queries.** The product uses an
  octree on the web and a ray tracing library on the desktop. We measured a
  chained flat structure against our hash grid and the grid won on every hot
  path; the reasons are in `accel.rs`. Revisit only if a measurement says so.

---

## Order

Dependencies first, then return for effort. Phases overlap where they do not
depend on each other; the rendering items can proceed alongside the topology
ones given a second pair of hands.

| # | Item | Why here | Effort |
|---|---|---|---|
| ~~1~~ | ~~M1 channels~~ | **done** | |
| ~~2~~ | ~~F1 compress the channel store~~ | **done** | |
| 3 | T1 shape cost collapse | needs a chosen collapse position first, see the entry | 1 wk |
| ~~4~~ | ~~R1 the tone curve~~ | **done** | |
| ~~5~~ | ~~R2 horizon sweep~~ | **done** | |
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
| 33 | F7 FBX, F9 mesh compression, A5 and A11 translations | | 4 wk |
| 34 | F11 baking, F13 images, R14 encodings, R15 final extras | | 3 wk |

Platforms run alongside rather than after, and two of them come early:

| # | Item | Why here | Effort |
|---|---|---|---|
| early | P10 backend selection | nothing else ports without it | 1 wk |
| early | P1 Linux | cheapest platform, catches assumptions now | 3 d |
| early | P11 continuous checks | five platforms rot without them | 4 d |
| with phase 2 | P2 macOS | | 4 d |
| with phase 3 | P9 one input layer | before more devices arrive | 1 wk |
| with phase 3 | P5 pen hover | felt immediately, cheap per platform | 4 d each |
| with phase 4 | P8 six degree of freedom devices | | 1 wk |
| with phase 4 | P7 stylus buttons | | 4 d |
| after M1 and F1 | P3 the browser | memory has to come down first | 2-3 wk |
| after P3 | P4 iPadOS native | | 3 wk |
| after P4 | P6 pencil gestures | needs the native shell | 3 d |

Slotted in wherever they fit, because each is small and none blocks anything:
A8 settings that persist, A10 snapping, S4 spacing, S10 measure, S13 symmetry,
S16 mask selection, S17 primitives, T10 quad tidying, R12 view modes. Two more
belong early despite sitting low in the table: **R13 shader variants**, before
the rendering work multiplies the shaders it has to share code between, and
**A12 long operations off the interface thread**, before the first command that
takes a minute ships.

Roughly ten to eleven months for one pair of hands. The first five items, about
three weeks together, carry most of the day-to-day improvement.

---

## Coverage of the teardown

Every section of the analysis, and where it lands here. This is the table to
check when asking whether something was missed.

| Section | Subject | Where it lands |
|---|---|---|
| 1.1 to 1.7 | builds, stack, third-party libraries | context; the library list is replaced item by item |
| 1.8 | denoising a final image | not planned |
| 2.1 to 2.5 | project container, per-channel blocks, compression | F1, F2 |
| 2.6 | material entries | R7, A4 |
| 2.7 | light entries | A3 |
| 2.8, 2.9 | writing and reading order | F2 |
| 3.1 | hybrid deferred and forward | R3 |
| 3.2 | shader preprocessor and variants | R13 |
| 3.3 | surface targets | R3 |
| 3.4 | direct lighting, image based lighting, shadows | R4, R5 |
| 3.4 | occlusion, reflections, indirect, refraction | R2, R7, R8 |
| 3.5 | material kinds and channels | R6, R7 |
| 3.6 | the effect chain and tone curves | R1, R8, R9, R15 |
| 3.7 | scene passes, outline, cursor, navigation cube | R12, done |
| 3.8 | atlases, environments, texture encodings | R4, R14, and a divergence |
| 4.1 | channels and quantisation | M1, M2, M3 |
| 4.2 | four-sided faces, half-edge, adjacency | M4, M5 |
| 4.3 | layer separation, channel copy, UV reorder | S1, M1, T7 |
| 4.4 | mesh kinds, levels, primitives, groups | T8, S17, M3 |
| 4.5 | CPU parallelism | done |
| 4.6 | spatial indexing | done, with the octree recorded as not planned |
| 4.7 | packets for incremental upload | done |
| 5.1 | the tool list | done for 16, S1 and S6 to S12 for the rest |
| 5.2.1 | the remesh pipeline, seven stages | T2, T3, T4, T9, T10 |
| 5.2.2 | live topology and its detail measures | done |
| 5.2.3 | quad remeshing | T6 |
| 5.2.4 | decimation, surface remesh, levels, repair | T1, T11, T8, T4 |
| 5.3 | scene operations, extraction, repeats | S11, S14, T5, A1 |
| 5.4 | primitives | S17 |
| 5.5 | history | done |
| 5.6 | masks and painting | done, S16 for making a mask |
| 5.7 | symmetry, radial and limited | S13 |
| 5.8 | the brush kernel, spacing, screen radius | S2, S3, S4, S5 |
| 5.9 | ray casting and picking | done |
| 6.1 | the format table | F3 to F10, F13 |
| 6.2 | the OBJ writer | F5 |
| 6.3 | asynchronous export with progress | A12 |
| 6.4 | glTF, morph targets, compression extensions | F3, F4, F9 |
| 6.5 | FBX | F7 |
| 6.6 | baking | F11 |
| 6.7 | compression and encoders | F1, F9, R14 |
| 7.1 | a custom drawn interface | a divergence, recorded |
| 7.2 | bindings and their file | done, A8 for keeping them |
| 7.3 | pointer, pen, gestures | done, A6 and A9 for the rest |
| 7.4 | the radial menu | done |
| 7.5 | translations | A5, A11 |
| 7.6 | navigation cube, snapping, gizmo, measure, stats | done, A10 and S10 |
| annexe | how the analysis was made | not applicable |

**Phase 7 has no row above, and that is correct.** The analysed product ships
on Windows, macOS and the web, has no support for a six degree of freedom
device, and reaches a tablet only through a browser. Everything in that phase is
ours to decide, so there is nothing to trace it back to.

**One section of the analysis is missing from the folder.** Its index lists
`08-licences.md`, covering activation, licensing and commercial integration, and
the file is not there. Nothing in this roadmap depends on it: everything in that
domain is already under what is deliberately not planned. Worth knowing rather
than discovering later.

## Open questions

Known, unresolved, and written down so they are not rediscovered.

1. **Resolved, kept as a warning.** A model of ten million triangles was
   reported as running at seven frames a second at rest and sixty to a hundred
   and twenty while a stroke was in progress. The readout was measuring the gap
   between frames, and since nothing is drawn while nothing moves, that gap was
   mostly time spent waiting. The arithmetic settles at one over the gap, so
   seven meant a hundred and forty-three milliseconds between events. The same
   report gives the real answer: sixty to a hundred and twenty while drawing
   continuously is a frame that costs eight to sixteen milliseconds, at ten
   million triangles, which is the number that was there all along. The readout
   now measures what a frame costs to produce.

2. **A dab is not identical across core counts.** A stroke lands on 7639 new
   vertices on six cores and 7649 on one, each repeatable. The edge search was
   proved not to be the cause: its output is sorted into a total order and is a
   pure function of the mesh. Something else in a dab still follows how the work
   was divided.
3. **The packet reorder still stalls between strokes.** 185 ms on five million
   triangles, of which 60 ms is rebuilding the ring. Doing it on a worker thread
   against a snapshot would hide it, at the cost of a frame of staleness.
4. **The ring costs 48 bytes a vertex.** Eight face indices held inline. A
   packed row layout would halve it but cannot be mutated in place, which live
   topology needs constantly.
5. **Hardware backface culling is off.** The comment says a locally inverted
   surface would show holes. The packet normal cone already discards
   back-facing packets, so the remaining gain is smaller than it looks, and it
   has never been measured.
