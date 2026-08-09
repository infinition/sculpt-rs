// Matcap shading: the view-space normal indexes a pre-lit sphere texture.

struct Uniforms {
    view_proj: mat4x4<f32>,
    view: mat4x4<f32>,
    // x = mask tint amount, y = vertex colour amount, z = wire mode, w unused
    params: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var matcap_tex: texture_2d<f32>;
@group(0) @binding(2) var matcap_samp: sampler;

struct VsIn {
    @location(0) pos: vec3<f32>,
    @location(1) nrm: vec3<f32>,
    @location(2) col: vec3<f32>,
    @location(3) mask: f32,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) nrm_view: vec3<f32>,
    @location(1) col: vec3<f32>,
    @location(2) mask: f32,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    out.clip = u.view_proj * vec4<f32>(in.pos, 1.0);
    out.nrm_view = (u.view * vec4<f32>(in.nrm, 0.0)).xyz;
    out.col = in.col;
    out.mask = in.mask;
    return out;
}

@fragment
fn fs_main(in: VsOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    // Flip on backfaces so the inside of an open surface still reads as lit.
    var n = normalize(in.nrm_view);
    if (!front) {
        n = -n;
    }

    var uv = n.xy * 0.5 + vec2<f32>(0.5, 0.5);
    uv.y = 1.0 - uv.y;
    let mc = textureSample(matcap_tex, matcap_samp, uv).rgb;

    // Vertex colour modulates the matcap; params.y fades painting in and out.
    let tint = mix(vec3<f32>(1.0, 1.0, 1.0), in.col, u.params.y);
    var c = mc * tint;

    // Masked regions read blue, matching the convention in other sculpt tools.
    c = mix(c, vec3<f32>(0.22, 0.42, 0.85), clamp(in.mask, 0.0, 1.0) * u.params.x * 0.6);

    return vec4<f32>(c, 1.0);
}

@fragment
fn fs_wire(in: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(0.05, 0.05, 0.06, 1.0);
}
