# Names

The vocabulary of this engine, and what each name rests on.

## Why we name things ourselves

Borrowed names carry someone else's design with them. Calling a pass a GBuffer
imports a decade of assumptions about what belongs in it; calling a remesher
QuadriFlow suggests we shipped somebody's library. Neither is true here, and
both make the code harder to reason about than a name that says what the thing
does.

So every module, pass and structure has a name of ours, in the same plain
register the rest of the code uses: the grid, the packets, the log. If a name
needs a paragraph to justify itself it is the wrong name.

## Why we still cite the technique

Every entry below carries a **rests on** line naming the published work it
implements. That is not decoration and it does not weaken the ownership.

- It is what makes the clean room claim checkable. Anyone can read the paper,
  read our code, and see that one is an implementation of the other rather than
  a copy of somebody's source.
- It is honest. We did not invent error quadrics or the octahedral normal
  encoding, and pretending otherwise would be the one thing that could actually
  cost us the project.
- It is useful. The next person to touch the corner solver wants to know which
  paper to read first.

A name of ours over a technique we cite is what every serious engine does. The
name is the ownership; the citation is the paper trail.

---

## Mesh core

| Our name | What it is | Rests on |
|---|---|---|
| **the mesh** | positions and faces, gap free, plus channels | removal by swap and index fixup, standard practice |
| **channels** | one buffer per vertex attribute, quantised to its own need | structure of arrays |
| **a dormant channel** | a channel still at its default, holding no memory at all | the product's `only_zeros` flag, same idea, our name |
| **the ring** | the faces and vertices immediately around a vertex | standard adjacency |
| **the edge book** | edge to face lookup in constant time | half-edge, Weiler; or a hashed edge key |
| **the grid** | incremental spatial hash over vertices and faces | spatial hashing, Teschner et al. |
| **packets** | contiguous runs of faces carrying a box and a normal cone | meshlet culling, the cone test from Akenine-Moller |
| **drift** | how far packets have loosened since they were last ordered | ours |
| **the log** | the slots a stroke wrote over, and the lengths it started at | ours |
| **hot and cold streams** | the GPU vertex split by what shading needs | octahedral normal encoding, Cigolle et al. |

## Sculpting

| Our name | What it is | Rests on |
|---|---|---|
| **a dab** | one application of a brush at a point | ours |
| **a stroke** | the dabs between putting the pen down and lifting it | ours |
| **falloff** | the radial weighting inside the brush disc | Wyvill kernel |
| **an alpha** | an image multiplied into the falloff | standard stamping |
| **live topology** | detail created and destroyed under the brush | edge split and collapse |
| **detail measure** | whether detail is read in world units, brush radii or pixels | ours, the three modes the product also offers |
| **the dab buffer** | displacement gathered from every stamp, applied once | ours |
| **layers** | named displacements over a base, weighted and reorderable | standard sculpt layers |

## Topology

| Our name | What it is | Rests on |
|---|---|---|
| **even subdivision** | every triangle into four | midpoint, or Loop subdivision |
| **shape cost collapse** | decimation that removes what changes the surface least | error quadrics, Garland and Heckbert |
| **the field** | the signed distance volume a remesh is read from | signed distance fields |
| **the sparse field** | the same, storing only cells the surface touches | sparse voxel storage, hashed cells |
| **the dual mesher** | surface extracted from the field, one vertex per straddling cell | surface nets, Gibson; dual marching cubes, Schaefer and Warren |
| **the corner solve** | keeping a sharp edge instead of rounding it | quadratic error function, dual contouring, Ju et al. |
| **settling** | putting a remeshed surface back onto the one it came from | ray projection with relaxation |
| **the mender** | making a surface manifold and closed again | standard manifold repair |
| **solid ops** | union, difference and intersection between objects | mesh booleans |
| **the quad pass** | triangles reorganised into quads along the surface flow | field aligned remeshing, Jakob et al.; or QuadriFlow, Huang et al. |
| **the flattener** | UVs from a surface | boundary first flattening, Sawhney and Crane; or least squares conformal maps |
| **levels** | a coarse cage plus stored detail at each subdivision | multiresolution sculpting, Catmull-Clark for the cage |

## Rendering

| Our name | What it is | Rests on |
|---|---|---|
| **the surface pass** | one pass writing what shading needs, before any shading | deferred shading |
| **surface targets** | colour, linear depth, packed normal, material terms | standard deferred targets |
| **the tone curve** | mapping high range light onto a screen | ACES fitted, or AGX |
| **horizon sweep** | ambient occlusion swept from the depth buffer | ground truth ambient occlusion, Jimenez et al. |
| **ambient harmonics** | the sky's diffuse contribution in nine numbers | spherical harmonic irradiance, Ramamoorthi and Hanrahan |
| **the roughness chain** | an environment blurred progressively down its mip levels | prefiltered importance sampling, Karis |
| **the reflectance table** | the two term lookup that finishes a specular highlight | split sum approximation, Karis |
| **screen reflections** | reflections marched through the depth buffer | screen space reflection |
| **screen bounce** | one indirect light bounce, marched the same way | screen space global illumination |
| **frame accumulation** | jittering and blending across frames | temporal antialiasing |
| **edge smoothing** | a cheap final antialias | FXAA, Lottes |
| **contact shading** | short range occlusion where surfaces meet | contact shadows |
| **the packet tree** | packets simplified in groups so levels share their borders | virtualised geometry, the locked boundary idea from Nanite |
| **detail levels** | which rung of the packet tree a view asks for | projected error in pixels |

## Scene and files

| Our name | What it is | Rests on |
|---|---|---|
| **the scene container** | our project file: header, manifest, channel store | ours; LZ4 for the blocks |
| **the manifest** | the readable part of a scene file, listing everything in it | JSON |
| **the channel store** | the packed blobs a manifest points into | ours |
| **the packet reorder** | putting faces back in spatial order | Morton curve ordering |

## Names we keep

Some names are not anyone's design, they are the thing itself, and replacing
them would only cost a reader time: **LZ4**, **glTF**, **OBJ**, **PLY**,
**STL**, **FBX**, **PNG**, **sRGB**, **PBR**, **wgpu**, **WGSL**, **rayon**,
**Morton**, **matcap**. A file format is a contract with other programs, not a
design decision of ours.
