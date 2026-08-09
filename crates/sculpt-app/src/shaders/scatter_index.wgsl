// The index-buffer counterpart of scatter.wgsl.
//
// Dynamic topology rewrites triangles all over the face array as it splits and
// collapses edges, so the same sparse trick applies: send the changed triangles
// and their slots, and let the GPU put them away.

/// Indices per triangle.
const STRIDE: u32 = 3u;

@group(0) @binding(0) var<storage, read_write> destination: array<u32>;
@group(0) @binding(1) var<storage, read> slots: array<u32>;
@group(0) @binding(2) var<storage, read> source: array<u32>;
@group(0) @binding(3) var<uniform> params: vec4<u32>;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    if (i >= params.x) {
        return;
    }
    let write_at = slots[i] * STRIDE;
    let read_at = i * STRIDE;
    for (var k = 0u; k < STRIDE; k = k + 1u) {
        destination[write_at + k] = source[read_at + k];
    }
}
