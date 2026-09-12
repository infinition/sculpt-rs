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
    // x = exposure, y = contrast, z = saturation, w = 1 when the curve is on
    tone: vec4<f32>,
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

// Two streams. The first is what placing and shading a pixel cannot do
// without; the second is what only painting writes to. See gpu_vertex.rs.
struct VsIn {
    // Hot: position at full precision, normal folded onto an octahedron.
    @location(0) pos: vec3<f32>,
    @location(1) nrm_oct: vec2<f32>,
    // Cold: colour with the mask in its alpha, then roughness and metalness.
    @location(2) paint: vec4<f32>,
    @location(3) material: vec4<f32>,
};

// Unfolds the octahedron. The lower half of the sphere was reflected out into
// the corners of the square, so it is folded back the same way.
fn oct_decode(e: vec2<f32>) -> vec3<f32> {
    let f = e * 2.0 - vec2<f32>(1.0, 1.0);
    var n = vec3<f32>(f.x, f.y, 1.0 - abs(f.x) - abs(f.y));
    let t = max(-n.z, 0.0);
    n.x += select(t, -t, n.x >= 0.0);
    n.y += select(t, -t, n.y >= 0.0);
    return normalize(n);
}

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
    let nrm_world = (obj.normal_mat * vec4<f32>(oct_decode(in.nrm_oct), 0.0)).xyz;
    out.clip = g.view_proj * world;
    out.world = world.xyz;
    out.nrm_world = nrm_world;
    out.nrm_view = (g.view * vec4<f32>(nrm_world, 0.0)).xyz;
    out.col = in.paint.rgb;
    out.mask = in.paint.a;
    out.material = in.material.xy;
    return out;
}

// Light as a camera would have recorded it.
//
// Without this, everything above one is clipped flat: a specular highlight
// arrives as a white disc with no shape, and the brightest part of a form
// reads as a hole. The curve rolls the top off instead, so a highlight keeps
// its falloff and the rest of the range is left where it was.
//
// Rests on the fitted approximation of the academy transform published by
// Narkowicz, which is four constants and one divide and is what most engines
// reach for when they want the look without the cost.
fn tone_curve(linear: vec3<f32>) -> vec3<f32> {
    // Exposure is a camera control and applies either way. Only the roll-off
    // is optional, so turning it off leaves the same light clipping flat
    // against the top of the range, which is the thing worth comparing.
    var c = linear * g.tone.x;
    if (g.tone.w < 0.5) {
        return c;
    }
    let a = c * (2.51 * c + 0.03);
    let b = c * (2.43 * c + 0.59) + 0.14;
    c = clamp(a / b, vec3<f32>(0.0), vec3<f32>(1.0));
    // Contrast around the middle of the range, then saturation around the
    // luminance, both after the curve so neither can push anything back over.
    c = clamp((c - 0.5) * g.tone.y + 0.5, vec3<f32>(0.0), vec3<f32>(1.0));
    let lum = dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
    return clamp(mix(vec3<f32>(lum), c, g.tone.z), vec3<f32>(0.0), vec3<f32>(1.0));
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
        // The one mode that simulates light, and so the one with a range to
        // map. Everything else here is already a colour somebody chose.
        c = tone_curve(pbr(nw, in.world, tint * 0.8 + 0.1, in.material.x, in.material.y));
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
        // Broad studio key, cool fill and restrained rim. The rig follows the
        // camera so forms remain readable while orbiting, like a sculpt matcap.
        let n = normalize(nv);
        let key = normalize(vec3<f32>(-0.45, 0.65, 0.70));
        let fill = normalize(vec3<f32>(0.70, 0.10, 0.65));
        let rim = normalize(vec3<f32>(0.35, 0.45, -0.80));
        let diffuse = max((dot(n, key) + 0.15) / 1.15, 0.0);
        let bounce = max(dot(n, fill), 0.0);
        let edge = pow(max(dot(n, rim), 0.0), 3.0);
        let half_key = normalize(key + vec3<f32>(0.0, 0.0, 1.0));
        let sheen = pow(max(dot(n, half_key), 0.0), 32.0) * 0.055;
        let base = vec3<f32>(0.42, 0.37, 0.31);
        c = tone_curve(base * (0.13 + 0.95 * diffuse)
            + base * vec3<f32>(0.68, 0.80, 1.0) * bounce * 0.24
            + vec3<f32>(0.10, 0.12, 0.15) * edge + vec3<f32>(sheen));
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
