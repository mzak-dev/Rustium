// Render styles, depth of field and the G-buffer of the path tracer. Concatenated after pathtracer.wgsl, which declares
// `Params`, the scene bindings and `radiance`; WGSL module scope is order independent, so this file only adds helpers.

const STYLE_TRACED: u32 = 0u;
const STYLE_CLAY: u32 = 1u;
const STYLE_TOON: u32 = 2u;
const STYLE_PIXEL: u32 = 3u;
const STYLE_WIRE: u32 = 4u;

struct Ray {
    origin: vec3<f32>,
    dir: vec3<f32>,
};

// The camera ray through `position`, a fractional pixel coordinate (pinhole, no depth of field).
fn primary_ray(position: vec2<f32>) -> Ray {
    let uv = position / vec2<f32>(f32(params.image.x), f32(params.image.y));
    let ndc = vec2<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0);
    let near = params.inv_view_proj * vec4<f32>(ndc, 0.0, 1.0);
    let far = params.inv_view_proj * vec4<f32>(ndc, 1.0, 1.0);
    let origin = near.xyz / near.w;
    return Ray(origin, normalize(far.xyz / far.w - origin));
}

// Centre of the pixel (or of the whole block in the Pixel style) that `pixel` belongs to.
fn pixel_centre(pixel: vec2<u32>) -> vec2<f32> {
    if (params.style.x == STYLE_PIXEL) {
        let block = max(params.style.y, 1u);
        return vec2<f32>(pixel / block * block) + vec2<f32>(f32(block) * 0.5);
    }
    return vec2<f32>(pixel) + vec2<f32>(0.5);
}

// Thin lens: the point where the ray crosses the focal plane stays, the ray's start moves over the aperture disc.
fn thin_lens(ray: Ray) -> Ray {
    if (params.lens.z < 0.5) {
        return ray;
    }
    let near = params.inv_view_proj * vec4<f32>(0.0, 0.0, 0.0, 1.0);
    let far = params.inv_view_proj * vec4<f32>(0.0, 0.0, 1.0, 1.0);
    let forward = normalize(far.xyz / far.w - near.xyz / near.w);
    let denominator = dot(ray.dir, forward);
    if (abs(denominator) < 1.0e-6) {
        return ray;
    }
    let plane_point = params.eye.xyz + forward * params.lens.y;
    let focus = ray.origin + ray.dir * (dot(plane_point - ray.origin, forward) / denominator);
    let radius = params.lens.x * sqrt(rand());
    let phi = 2.0 * PI * rand();
    let frame = basis(forward);
    let origin = ray.origin + (frame[0] * cos(phi) + frame[1] * sin(phi)) * radius;
    return Ray(origin, normalize(focus - origin));
}

// First-hit geometry and albedo of one pixel for the resolve pass.
fn write_gbuffer(pixel: vec2<u32>, centre: vec2<f32>) {
    let ray = primary_ray(centre);
    let hit = trace(ray.origin, ray.dir, 0u, 0u);
    var geometry = vec4<f32>(0.0, 1.0, 0.0, 1.0e6);
    var albedo = vec3<f32>(1.0);
    if (hit.hit) {
        geometry = vec4<f32>(hit.normal, min(hit.t, 1.0e6));
        if (hit.color_index == GROUND_ID) {
            albedo = params.ground.rgb;
        } else {
            albedo = evaluate_material(hit.color_index, ray.origin + ray.dir * hit.t, hit.normal).base_color;
        }
    }
    textureStore(gbuffer_geometry, vec2<i32>(pixel), geometry);
    textureStore(gbuffer_albedo, vec2<i32>(pixel), vec4<f32>(albedo, 1.0));
}

// ---------------------------------------------------------------------------------------------------------------
// Clay: every surface becomes the same matte grey.

fn clay_surface(surface: Surface) -> Surface {
    var clay = surface;
    clay.base_color = vec3<f32>(0.72);
    clay.metallic = 0.0;
    clay.roughness = 1.0;
    clay.transmission = 0.0;
    clay.emission_strength = 0.0;
    return clay;
}

// ---------------------------------------------------------------------------------------------------------------
// Toon: sun light quantised into three bands, hard shadows. The outlines are drawn by the resolve pass.

fn toon_shade(point: vec3<f32>, hit: Hit, surface: Surface) -> vec3<f32> {
    let normal = surface.normal;
    let sun = params.sun_dir.xyz;
    let cos_sun = dot(normal, sun);
    var lit = 0.0;
    if (cos_sun > 0.0) {
        lit = cos_sun * sun_visibility(point + normal * EPS, sun);
    }
    var band = 0.0;
    if (lit > 0.65) {
        band = 1.0;
    } else if (lit > 0.3) {
        band = 0.7;
    } else if (lit > 0.0) {
        band = 0.4;
    }
    let ambient = 0.5 * mix(params.sky_horizon.rgb, params.sky_zenith.rgb, clamp(normal.y, 0.0, 1.0));
    return surface.base_color * (ambient + params.sun_color.rgb * (band * 0.75)) + surface.emission_color * surface.emission_strength;
}

// ---------------------------------------------------------------------------------------------------------------
// Wire: dark faces with light lines along the edges between voxel faces that are not coplanar.

fn wire_background() -> vec3<f32> {
    return vec3<f32>(0.03, 0.035, 0.045);
}

fn solid_local(model: ModelRecord, cell: vec3<i32>) -> bool {
    let rel = cell - model.grid_min.xyz * CHUNK;
    let size_voxels = vec3<i32>(model.grid_size.xyz) * CHUNK;
    if (any(rel < vec3<i32>(0)) || any(rel >= size_voxels)) {
        return false;
    }
    let chunk = vec3<i32>(rel.x >> 5u, rel.y >> 5u, rel.z >> 5u);
    let slot = chunk_slot(model, chunk);
    if (slot == 0u) {
        return false;
    }
    return voxel_in(slot, rel - chunk * CHUNK) != 0u;
}

// 1.0 where `point` on the face of a voxel lies within a line width of a convex or concave edge, else 0.0.
fn wire_line(point: vec3<f32>, hit: Hit) -> f32 {
    let model = models[hit.model];
    let moved = point - model.offset.xyz;
    let local_point = vec3<f32>(dot(model.column_x.xyz, moved), dot(model.column_y.xyz, moved), dot(model.column_z.xyz, moved));
    let local_normal = round(vec3<f32>(
        dot(model.column_x.xyz, hit.normal),
        dot(model.column_y.xyz, hit.normal),
        dot(model.column_z.xyz, hit.normal),
    ));
    let cell = vec3<i32>(floor(local_point - local_normal * 0.5));
    let front = vec3<i32>(local_normal);
    let width = 0.06;
    for (var axis = 0; axis < 3; axis += 1) {
        if (front[axis] != 0) {
            continue;
        }
        let fraction = local_point[axis] - floor(local_point[axis]);
        if (min(fraction, 1.0 - fraction) >= width) {
            continue;
        }
        var step = vec3<i32>(0);
        step[axis] = select(-1, 1, fraction >= 0.5);
        let neighbour = cell + step;
        if (!solid_local(model, neighbour) || solid_local(model, neighbour + front)) {
            return 1.0;
        }
    }
    return 0.0;
}

fn wire_shade(point: vec3<f32>, hit: Hit, surface: Surface) -> vec3<f32> {
    var fill = mix(wire_background(), surface.base_color, 0.10);
    fill *= 0.55 + 0.45 * max(dot(surface.normal, params.sun_dir.xyz), 0.0);
    if (hit.color_index != GROUND_ID && wire_line(point, hit) > 0.5) {
        return vec3<f32>(1.7, 1.78, 1.9);
    }
    return fill;
}
