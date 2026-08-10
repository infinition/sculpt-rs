// Scatters a sparse set of updates into a buffer already on the card.
//
// A brush touches a few thousand vertices out of millions, but they are
// scattered through the array, so a contiguous partial upload cannot cover
// them. Sending the whole buffer instead costs tens of megabytes every frame.
// This sends only what changed plus the slots it belongs in, and the GPU puts
// it away.
//
// Everything is copied as plain words. What a word means is the caller's
// business: three of them are a triangle, four are a position and a folded
// normal, two are a colour and a pair of material values. A struct here would
// be padded to sixteen byte boundaries and would stop matching any of them.

@group(0) @binding(0) var<storage, read_write> destination: array<u32>;
@group(0) @binding(1) var<storage, read> slots: array<u32>;
@group(0) @binding(2) var<storage, read> source: array<u32>;
// x holds how many updates are live this dispatch, y the words in each.
@group(0) @binding(3) var<uniform> params: vec4<u32>;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    if (i >= params.x) {
        return;
    }
    let stride = params.y;
    // `from` and `to` are reserved words in WGSL, hence the longer names.
    let write_at = slots[i] * stride;
    let read_at = i * stride;
    for (var k = 0u; k < stride; k = k + 1u) {
        destination[write_at + k] = source[read_at + k];
    }
}
