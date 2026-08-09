// Scatters a sparse set of vertex updates into the vertex buffer.
//
// A brush touches a few thousand vertices out of millions, but they are
// scattered through the array, so a contiguous partial upload cannot cover
// them. Sending the whole buffer instead costs tens of megabytes every frame.
// This sends only the touched vertices plus their indices, and the GPU puts
// them where they belong.
//
// The buffers are plain float and integer arrays rather than structs: a WGSL
// struct holding vec3 fields would be padded to sixteen byte boundaries and no
// longer match the packed layout the vertex buffer uses.

/// Floats per vertex: position, normal, colour, mask, roughness, metalness.
const STRIDE: u32 = 12u;

@group(0) @binding(0) var<storage, read_write> destination: array<f32>;
@group(0) @binding(1) var<storage, read> indices: array<u32>;
@group(0) @binding(2) var<storage, read> source: array<f32>;
// x holds how many updates are live this dispatch.
@group(0) @binding(3) var<uniform> params: vec4<u32>;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    if (i >= params.x) {
        return;
    }
    let to = indices[i] * STRIDE;
    let from = i * STRIDE;
    for (var k = 0u; k < STRIDE; k = k + 1u) {
        destination[to + k] = source[from + k];
    }
}
