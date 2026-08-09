// Surface shading. One pipeline, several looks, picked by globals.params.z.

struct Globals {
    view_proj: mat4x4<f32>,
    view: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    eye: vec4<f32>,
    // x = mask tint, y = vertex colour amount, z = shading mode, w = opacity
    params: vec4<f32>,
    // x = curvature strength, y = flat shading, z = time, w = unused
    extra: vec4<f32>,
    bg_top: vec4<f32>,
    bg_bottom: vec4<f32>,
};

struct ObjectData {
    model: mat4x4<f32>,
    normal_mat: mat4x4<f32>,
    // rgb tint, a = 1 when this is the object being sculpted
    tint: vec4<f32>,
};

@group(0) @binding(0) var<uniform> g: Globals;
@group(0) @binding(1) var matcap_tex: texture_2d<f32>;
@group(0) @binding(2) var matcap_samp: sampler;
@group(1) @binding(0) var<uniform> obj: ObjectData;

struct VsIn {
    @location(0) pos: vec3<f32>,
    @location(1) nrm: vec3<f32>,
    @location(2) col: vec3<f32>,
    @location(3) mask: f32,
    @location(4) rough: f32,
    @location(5) metal: f32,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) nrm_view: vec3<f32>,
    @location(1) col: vec3<f32>,
    @location(2) mask: f32,
    @location(3) world: vec3<f32>,
    @location(4) material: vec2<f32>,
    @location(5) nrm_world: vec3<f32>,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    let world = obj.model * vec4<f32>(in.pos, 1.0);
    let nrm_world = (obj.normal_mat * vec4<f32>(in.nrm, 0.0)).xyz;
    out.clip = g.view_proj * world;
    out.world = world.xyz;
    out.nrm_world = nrm_world;
    out.nrm_view = (g.view * vec4<f32>(nrm_world, 0.0)).xyz;
    out.col = in.col;
    out.mask = in.mask;
    out.material = vec2<f32>(in.rough, in.metal);
    return out;
}

fn matcap(n: vec3<f32>) -> vec3<f32> {
    var uv = n.xy * 0.5 + vec2<f32>(0.5, 0.5);
    uv.y = 1.0 - uv.y;
    return textureSample(matcap_tex, matcap_samp, uv).rgb;
}

// Three-light rig with a GGX-ish specular lobe. Cheap, and it reacts to the
// painted roughness and metalness in a way a matcap cannot.
fn pbr(n: vec3<f32>, world: vec3<f32>, albedo: vec3<f32>, rough: f32, metal: f32) -> vec3<f32> {
    let v = normalize(g.eye.xyz - world);
    let r = clamp(rough, 0.05, 1.0);
    let a = r * r;
    let f0 = mix(vec3<f32>(0.04, 0.04, 0.04), albedo, metal);
    let diffuse_albedo = albedo * (1.0 - metal);

    var lit = albedo * 0.16;
    let dirs = array<vec3<f32>, 3>(
        normalize(vec3<f32>(-0.45, 0.72, 0.55)),
        normalize(vec3<f32>(0.75, 0.15, 0.55)),
        normalize(vec3<f32>(0.10, -0.70, -0.45)),
    );
    let cols = array<vec3<f32>, 3>(
        vec3<f32>(1.0, 0.96, 0.90) * 1.0,
        vec3<f32>(0.58, 0.70, 0.98) * 0.45,
        vec3<f32>(0.95, 0.82, 0.70) * 0.25,
    );
    for (var i = 0; i < 3; i = i + 1) {
        let l = dirs[i];
        let ndl = max(dot(n, l), 0.0);
        if (ndl <= 0.0) {
            continue;
        }
        let h = normalize(l + v);
        let ndh = max(dot(n, h), 0.0);
        let vdh = max(dot(v, h), 0.0);
        // GGX normal distribution.
        let d = a * a / max(3.14159 * pow(ndh * ndh * (a * a - 1.0) + 1.0, 2.0), 1e-4);
        let f = f0 + (vec3<f32>(1.0) - f0) * pow(1.0 - vdh, 5.0);
        let k = a * 0.5;
        let ndv = max(dot(n, v), 1e-3);
        let gv = ndv / (ndv * (1.0 - k) + k);
        let gl = ndl / (ndl * (1.0 - k) + k);
        let spec = d * f * gv * gl / max(4.0 * ndv * ndl, 1e-4);
        lit += (diffuse_albedo / 3.14159 + spec) * cols[i] * ndl;
    }
    // Sky and ground bounce.
    let up = max(dot(n, vec3<f32>(0.0, 1.0, 0.0)), 0.0);
    lit += diffuse_albedo * mix(vec3<f32>(0.10, 0.09, 0.10), vec3<f32>(0.24, 0.27, 0.34), up) ;
    return lit;
}

@fragment
fn fs_main(in: VsOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    // Flip on backfaces so the inside of an open surface still reads as lit.
    var nv = normalize(in.nrm_view);
    var nw = normalize(in.nrm_world);
    if (!front) {
        nv = -nv;
        nw = -nw;
    }
    // Flat shading rebuilds the face normal from screen-space derivatives.
    if (g.extra.y > 0.5) {
        let fn_world = normalize(cross(dpdx(in.world), dpdy(in.world)));
        nw = select(-fn_world, fn_world, dot(fn_world, nw) > 0.0);
        nv = (g.view * vec4<f32>(nw, 0.0)).xyz;
    }

    let tint = mix(vec3<f32>(1.0, 1.0, 1.0), in.col, g.params.y);
    let mode = i32(g.params.z + 0.5);
    var c: vec3<f32>;

    if (mode == 0) {
        c = matcap(normalize(nv)) * tint;
    } else if (mode == 1) {
        c = pbr(nw, in.world, tint * 0.8 + 0.1, in.material.x, in.material.y);
    } else if (mode == 2) {
        c = normalize(nw) * 0.5 + vec3<f32>(0.5, 0.5, 0.5);
    } else if (mode == 3) {
        // Cavity: how fast the normal turns tells us where the creases are.
        let d = length(dpdx(nw)) + length(dpdy(nw));
        let cav = clamp(1.0 - d * g.extra.x, 0.0, 1.0);
        c = matcap(normalize(nv)) * tint * mix(0.25, 1.0, cav);
    } else if (mode == 5) {
        // Unlit: the painted colour and nothing else, which is the only way to
        // judge hand-painted work without the shading lying to you.
        c = in.col;
    } else {
        // Untextured clay, the neutral view for judging form.
        let ndl = max(dot(normalize(nv), normalize(vec3<f32>(-0.3, 0.5, 0.8))), 0.0);
        c = vec3<f32>(0.62, 0.60, 0.58) * (0.25 + 0.75 * ndl);
    }

    c *= obj.tint.rgb;
    // Masked regions read blue, matching the convention in other sculpt tools.
    c = mix(c, vec3<f32>(0.22, 0.42, 0.85), clamp(in.mask, 0.0, 1.0) * g.params.x * 0.6);
    // Dim whatever is not the active object so the selection is obvious.
    if (obj.tint.a < 0.5) {
        c = mix(c, vec3<f32>(0.16, 0.17, 0.20), 0.45);
    }
    return vec4<f32>(c, g.params.w);
}

@fragment
fn fs_wire(in: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(0.06, 0.07, 0.09, 0.85);
}
