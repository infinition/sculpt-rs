// Ground plane grid. A fullscreen triangle is intersected with y = 0 in the
// fragment shader, which gives an infinite grid with correct depth and no
// geometry to tessellate.

struct Globals {
    view_proj: mat4x4<f32>,
    view: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    eye: vec4<f32>,
    params: vec4<f32>,
    // x = curvature, y = flat shading, z = grid spacing, w = grid fade distance
    extra: vec4<f32>,
    bg_top: vec4<f32>,
    bg_bottom: vec4<f32>,
};

@group(0) @binding(0) var<uniform> g: Globals;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) ndc: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VsOut {
    var out: VsOut;
    let x = f32((i << 1u) & 2u) * 2.0 - 1.0;
    let y = f32(i & 2u) * 2.0 - 1.0;
    out.clip = vec4<f32>(x, y, 1.0, 1.0);
    out.ndc = vec2<f32>(x, y);
    return out;
}

struct FsOut {
    @location(0) color: vec4<f32>,
    @builtin(frag_depth) depth: f32,
};

fn unproject(ndc: vec2<f32>, z: f32) -> vec3<f32> {
    let p = g.inv_view_proj * vec4<f32>(ndc.x, ndc.y, z, 1.0);
    return p.xyz / p.w;
}

/// Anti-aliased coverage for lines `spacing` apart, using the pixel footprint.
fn grid_coverage(p: vec2<f32>, spacing: f32, width: f32) -> f32 {
    let scaled = p / spacing;
    let deriv = fwidth(scaled) * width;
    let dist = abs(fract(scaled - vec2<f32>(0.5, 0.5)) - vec2<f32>(0.5, 0.5))
        / max(deriv, vec2<f32>(1e-5, 1e-5));
    return 1.0 - min(min(dist.x, dist.y), 1.0);
}

@fragment
fn fs_main(in: VsOut) -> FsOut {
    var out: FsOut;

    let near = unproject(in.ndc, 0.0);
    let far = unproject(in.ndc, 1.0);
    let dir = far - near;
    if (abs(dir.y) < 1e-6) {
        discard;
    }
    let t = -near.y / dir.y;
    if (t < 0.0 || t > 1.0) {
        discard;
    }
    let hit = near + dir * t;

    let spacing = max(g.extra.z, 1e-4);
    let fine = grid_coverage(hit.xz, spacing, 1.0);
    let coarse = grid_coverage(hit.xz, spacing * 10.0, 1.4);
    var a = max(fine * 0.30, coarse * 0.55);

    // Fade with distance so the horizon does not alias into a grey wash.
    let dist = length(hit.xz - g.eye.xz);
    a *= clamp(1.0 - dist / max(g.extra.w, 1e-3), 0.0, 1.0);
    if (a <= 0.002) {
        discard;
    }

    var color = vec3<f32>(0.42, 0.45, 0.52);
    let px = fwidth(hit.x) * 1.5;
    let pz = fwidth(hit.z) * 1.5;
    if (abs(hit.z) < pz) {
        color = vec3<f32>(0.90, 0.36, 0.38); // the X axis runs along z = 0
        a = max(a, 0.55);
    }
    if (abs(hit.x) < px) {
        color = vec3<f32>(0.36, 0.55, 0.95); // the Z axis runs along x = 0
        a = max(a, 0.55);
    }

    let clip = g.view_proj * vec4<f32>(hit, 1.0);
    out.depth = clamp(clip.z / clip.w, 0.0, 1.0);
    out.color = vec4<f32>(color, a);
    return out;
}
