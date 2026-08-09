// Vertical gradient behind the model, drawn as one oversized triangle so there
// is no vertex buffer to keep around.

struct Globals {
    view_proj: mat4x4<f32>,
    view: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    eye: vec4<f32>,
    params: vec4<f32>,
    extra: vec4<f32>,
    bg_top: vec4<f32>,
    bg_bottom: vec4<f32>,
};

@group(0) @binding(0) var<uniform> g: Globals;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VsOut {
    var out: VsOut;
    let x = f32((i << 1u) & 2u) * 2.0 - 1.0;
    let y = f32(i & 2u) * 2.0 - 1.0;
    out.clip = vec4<f32>(x, y, 1.0, 1.0);
    out.uv = vec2<f32>(x, y) * 0.5 + vec2<f32>(0.5, 0.5);
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let t = clamp(in.uv.y, 0.0, 1.0);
    var c = mix(g.bg_bottom.rgb, g.bg_top.rgb, t);
    // A soft pool of light behind the model keeps the silhouette readable.
    let d = length((in.uv - vec2<f32>(0.5, 0.55)) * vec2<f32>(1.35, 1.0));
    c += vec3<f32>(0.05, 0.052, 0.06) * clamp(1.0 - d * 1.6, 0.0, 1.0);
    return vec4<f32>(c, 1.0);
}
