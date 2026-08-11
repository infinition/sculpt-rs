// How much of the sky each pixel can actually see.
//
// A crease is dark because the surface around it blocks most of the light that
// would otherwise arrive. Nothing in a three-light rig knows that, so without
// this a fold and a flat both come back the same brightness and the form stops
// reading as solid. It is the single largest difference between a shaded shell
// and something that looks like an object.
//
// Rests on the horizon-based approach of Bavoil and Sainz: march a few
// directions away from the pixel across the depth buffer, find the highest
// angle at which the surface still rises above the tangent plane, and take what
// is left of the hemisphere. The ground truth integral of Jimenez et al. is the
// fuller version of the same idea and would replace the estimator here without
// touching anything around it.
//
// It reads the depth buffer and nothing else. There is no pass writing normals
// yet, so the normal is rebuilt from how the reconstructed position changes
// across neighbouring pixels, which is exact on a flat surface and good enough
// everywhere a crease matters.

struct Globals {
    view_proj: mat4x4<f32>,
    view: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    eye: vec4<f32>,
    params: vec4<f32>,
    extra: vec4<f32>,
    bg_top: vec4<f32>,
    bg_bottom: vec4<f32>,
    tone: vec4<f32>,
    // x = world radius, y = strength, z = 1 when on, w = unused
    occlusion: vec4<f32>,
};

@group(0) @binding(0) var<uniform> g: Globals;
@group(0) @binding(1) var depth_tex: DEPTH_TEXTURE_TYPE;

/// Directions swept per pixel, and steps along each.
///
/// Eight and five is where it stops looking like noise and starts looking like
/// shading, at a cost that does not show next to drawing the model.
const DIRS: i32 = 8;
const STEPS: i32 = 5;

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    // One triangle covering the screen: cheaper than two, and no seam.
    let x = f32(i32(i) / 2) * 4.0 - 1.0;
    let y = f32(i32(i) & 1) * 4.0 - 1.0;
    return vec4<f32>(x, y, 0.0, 1.0);
}

fn depth_at(c: vec2<i32>) -> f32 {
    return LOAD_DEPTH;
}

/// Where a pixel is in the world, from its depth alone.
fn world_at(c: vec2<i32>, size: vec2<f32>) -> vec3<f32> {
    let d = depth_at(c);
    let uv = (vec2<f32>(c) + vec2<f32>(0.5)) / size;
    let ndc = vec3<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, d);
    let p = g.inv_view_proj * vec4<f32>(ndc, 1.0);
    return p.xyz / p.w;
}

/// A different rotation for every pixel, so eight directions cover far more
/// than eight and the pattern does not print itself on the surface.
fn dither(c: vec2<f32>) -> f32 {
    return fract(52.9829189 * fract(dot(c, vec2<f32>(0.06711056, 0.00583715))));
}

@fragment
fn fs_main(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    if (g.occlusion.z < 0.5) {
        return vec4<f32>(1.0, 1.0, 1.0, 1.0);
    }
    let size = vec2<f32>(textureDimensions(depth_tex));
    let here = vec2<i32>(frag.xy);
    let d = depth_at(here);
    // Nothing was drawn here, so there is nothing to darken.
    if (d >= 1.0) {
        return vec4<f32>(1.0, 1.0, 1.0, 1.0);
    }

    let p = world_at(here, size);
    // The normal from how the position moves across neighbours. Taken from both
    // sides and the shorter step kept, so a silhouette does not invent a wall.
    let dx0 = p - world_at(here - vec2<i32>(1, 0), size);
    let dx1 = world_at(here + vec2<i32>(1, 0), size) - p;
    let dy0 = p - world_at(here - vec2<i32>(0, 1), size);
    let dy1 = world_at(here + vec2<i32>(0, 1), size) - p;
    let dx = select(dx1, dx0, dot(dx0, dx0) < dot(dx1, dx1));
    let dy = select(dy1, dy0, dot(dy0, dy0) < dot(dy1, dy1));
    var n = normalize(cross(dx, dy));
    if (dot(n, g.eye.xyz - p) < 0.0) {
        n = -n;
    }

    // How far the sweep should reach, in pixels, for the world radius asked
    // for. Measured rather than assumed: take a step of that size across the
    // screen at this depth and see where it lands.
    let radius = g.occlusion.x;
    let right = normalize(vec3<f32>(g.view[0][0], g.view[1][0], g.view[2][0]));
    var edge = g.view_proj * vec4<f32>(p + right * radius, 1.0);
    var mid = g.view_proj * vec4<f32>(p, 1.0);
    let edge_px = (edge.xy / edge.w) * 0.5 * size;
    let mid_px = (mid.xy / mid.w) * 0.5 * size;
    let reach = clamp(length(edge_px - mid_px), 2.0, 96.0);

    // A tangent of about six degrees, so the surface does not shade itself
    // through the roughness of its own depth.
    let bias = 0.1;
    let turn = dither(frag.xy) * 6.2831853;
    var occlusion = 0.0;
    for (var k = 0; k < DIRS; k = k + 1) {
        let a = turn + f32(k) * (6.2831853 / f32(DIRS));
        let dir = vec2<f32>(cos(a), sin(a));
        var highest = bias;
        for (var s = 1; s <= STEPS; s = s + 1) {
            let at = here + vec2<i32>(dir * reach * (f32(s) / f32(STEPS)));
            if (at.x < 0 || at.y < 0 || at.x >= i32(size.x) || at.y >= i32(size.y)) {
                break;
            }
            if (depth_at(at) >= 1.0) {
                continue;
            }
            let v = world_at(at, size) - p;
            let dist = length(v);
            if (dist < 1e-6 || dist > radius) {
                continue;
            }
            // How far above the tangent plane this sample sits. The reach
            // above already refuses anything further than the radius, so the
            // horizon is taken as it is: scaling it by distance as well pulled
            // every crease back towards nothing.
            highest = max(highest, dot(n, v / dist));
        }
        occlusion += max(highest - bias, 0.0);
    }

    let visible = clamp(1.0 - (occlusion / f32(DIRS)) * g.occlusion.y, 0.0, 1.0);
    return vec4<f32>(visible, visible, visible, 1.0);
}
