// Software path tracer: two-level DDA over a sparse chunk grid per Model (ADR 0001). Concatenated after material.wgsl.
//
// Scene data comes from trace_scene.rs (chunk pool, one chunk index grid per Shape, a list of visible Models);
// everything below is shared traversal and shading; the nearest-hit and shadow-ray functions come from trace_dda.wgsl or
// trace_rq.wgsl.

struct Params {
    // Clip space to voxel space; unprojecting the near and far plane works for perspective and orthographic cameras.
    inv_view_proj: mat4x4<f32>,
    // xyz: camera position.
    eye: vec4<f32>,
    // xyz: unit vector towards the sun, w: angular radius of the sun disc in radians.
    sun_dir: vec4<f32>,
    // rgb: sun radiance as seen by a white diffuse surface facing the sun.
    sun_color: vec4<f32>,
    sky_zenith: vec4<f32>,
    sky_horizon: vec4<f32>,
    // x: image width, y: image height, z: samples already accumulated, w: max bounces.
    image: vec4<u32>,
    // Pixels outside [xy, zw) are not rendered.
    region: vec4<u32>,
    // x: ground (0 none, 1 studio, 2 shadow catcher), y: 1 = transparent background, z: samples this pass,
    // w: number of Models.
    options: vec4<u32>,
    // rgb: studio ground albedo, w: ground height.
    ground: vec4<f32>,
    // x: render style (STYLE_*), y: pixel size of the Pixel style, z: 1 = also write the G-buffer this pass.
    style: vec4<u32>,
    // x: lens radius in voxels, y: focus distance in voxels, z: 1 = depth of field is on.
    lens: vec4<f32>,
};

@group(0) @binding(0) var<uniform> params: Params;
// Binding 1 is the Color and Material table, declared in material.wgsl.
// 32768 voxels per chunk, one byte each (the Color id, 0 = empty), four to a word.
@group(0) @binding(2) var<storage, read> pool: array<u32>;
// The chunk grids of all Shapes, one after the other: pool slot + 1 per cell, or 0 for a cell without voxels.
@group(0) @binding(3) var<storage, read> chunk_grid: array<u32>;
@group(0) @binding(4) var history: texture_2d<f32>;
@group(0) @binding(5) var output: texture_storage_2d<rgba32float, write>;
// First-hit data of every pixel for the denoiser and the toon outlines (see trace_style.wgsl): xyz normal, w distance;
// and rgb albedo.
@group(0) @binding(7) var gbuffer_geometry: texture_storage_2d<rgba32float, write>;
@group(0) @binding(8) var gbuffer_albedo: texture_storage_2d<rgba32float, write>;

// A visible Model: scene = (column_x, column_y, column_z) * local + offset, plus where its Shape's chunk grid is.
struct ModelRecord {
    column_x: vec4<f32>,
    column_y: vec4<f32>,
    column_z: vec4<f32>,
    offset: vec4<f32>,
    // xyz: chunk coordinate of the grid's first cell, w: where the grid starts in `chunk_grid`.
    grid_min: vec4<i32>,
    // xyz: grid size in chunks.
    grid_size: vec4<u32>,
};

@group(0) @binding(6) var<storage, read> models: array<ModelRecord>;

const CHUNK: i32 = 32;
const WORDS_PER_CHUNK: u32 = 8192u;
const GROUND_ID: u32 = 256u;
const PI: f32 = 3.14159265;
const EPS: f32 = 0.002;
const FAR: f32 = 1.0e30;

// ---------------------------------------------------------------------------------------------------------------
// Random numbers (PCG).

var<private> rng_state: u32;

fn pcg(value: u32) -> u32 {
    let state = value * 747796405u + 2891336453u;
    let word = ((state >> ((state >> 28u) + 4u)) ^ state) * 277803737u;
    return (word >> 22u) ^ word;
}

fn rand() -> f32 {
    rng_state = pcg(rng_state);
    return f32(rng_state >> 8u) * (1.0 / 16777216.0);
}

// Orthonormal basis around `n` (Duff et al.): columns are tangent, bitangent, normal.
fn basis(n: vec3<f32>) -> mat3x3<f32> {
    let s = select(-1.0, 1.0, n.z >= 0.0);
    let a = -1.0 / (s + n.z);
    let b = n.x * n.y * a;
    return mat3x3<f32>(
        vec3<f32>(1.0 + s * n.x * n.x * a, s * b, -s * n.x),
        vec3<f32>(b, s + n.y * n.y * a, -n.y),
        n,
    );
}

fn cosine_hemisphere(n: vec3<f32>) -> vec3<f32> {
    let phi = 2.0 * PI * rand();
    let r2 = rand();
    let r = sqrt(r2);
    return basis(n) * vec3<f32>(r * cos(phi), r * sin(phi), sqrt(max(1.0 - r2, 0.0)));
}

fn unit_sphere() -> vec3<f32> {
    let z = 1.0 - 2.0 * rand();
    let phi = 2.0 * PI * rand();
    let r = sqrt(max(1.0 - z * z, 0.0));
    return vec3<f32>(r * cos(phi), r * sin(phi), z);
}

// ---------------------------------------------------------------------------------------------------------------
// Scene traversal.

struct Hit {
    hit: bool,
    t: f32,
    // Face normal in Scene space, pointing back towards the ray origin.
    normal: vec3<f32>,
    color_index: u32,
    // Index of the Model that was hit.
    model: u32,
};

fn miss() -> Hit {
    return Hit(false, FAR, vec3<f32>(0.0, 1.0, 0.0), 0u, 0u);
}

fn chunk_slot(model: ModelRecord, cell: vec3<i32>) -> u32 {
    let size = vec3<i32>(model.grid_size.xyz);
    return chunk_grid[u32(model.grid_min.w) + u32(cell.x + size.x * (cell.z + size.z * cell.y))];
}

fn voxel_in(slot: u32, local: vec3<i32>) -> u32 {
    let index = u32(local.x + local.z * CHUNK + local.y * CHUNK * CHUNK);
    let word = pool[(slot - 1u) * WORDS_PER_CHUNK + (index >> 2u)];
    return (word >> ((index & 3u) * 8u)) & 255u;
}

// HOT PATH: runs for every ray segment and Model. Walks the chunk grid of `model` in its Shape's space; empty chunks are skipped in one jump, occupied
// chunks are walked voxel by voxel. With `inside` != 0 the ray travels through voxels of that Color (glass) and
// reports the first voxel that differs; otherwise it reports the first non-empty voxel.
fn trace_voxels(model: ModelRecord, origin: vec3<f32>, dir: vec3<f32>, inside: u32) -> Hit {
    let grid_min = model.grid_min.xyz * CHUNK;
    let size_voxels = vec3<i32>(model.grid_size.xyz) * CHUNK;
    let box_min = vec3<f32>(grid_min);
    let box_max = vec3<f32>(grid_min + size_voxels);
    let positive = dir >= vec3<f32>(0.0);
    let safe_dir = select(dir, vec3<f32>(1.0e-20), abs(dir) < vec3<f32>(1.0e-20));
    let inv = 1.0 / safe_dir;

    let t_a = (box_min - origin) * inv;
    let t_b = (box_max - origin) * inv;
    let t_lo = min(t_a, t_b);
    let t_hi = max(t_a, t_b);
    let t_enter = max(max(t_lo.x, t_lo.y), max(t_lo.z, 0.0));
    let t_exit = min(t_hi.x, min(t_hi.y, t_hi.z));
    if (t_enter >= t_exit) {
        return miss();
    }

    // The axis the ray entered the grid through, or -1 when it starts inside.
    var last_axis = -1;
    if (max(t_lo.x, max(t_lo.y, t_lo.z)) > 0.0) {
        last_axis = select(select(2, 1, t_lo.y >= t_lo.z), 0, t_lo.x >= t_lo.y && t_lo.x >= t_lo.z);
    }

    let start = origin + dir * (t_enter + 1.0e-3);
    var voxel = clamp(vec3<i32>(floor(start)), grid_min, grid_min + size_voxels - vec3<i32>(1));
    let step = select(vec3<i32>(-1), vec3<i32>(1), positive);
    let delta = abs(inv);
    var t_max = (vec3<f32>(voxel) + select(vec3<f32>(0.0), vec3<f32>(1.0), positive) - origin) * inv;
    var t_cur = t_enter;

    for (var iteration = 0; iteration < 4096; iteration += 1) {
        let rel = voxel - grid_min;
        if (any(rel < vec3<i32>(0)) || any(rel >= size_voxels)) {
            break;
        }
        let cell = vec3<i32>(rel.x >> 5u, rel.y >> 5u, rel.z >> 5u);
        let slot = chunk_slot(model, cell);
        if (slot == 0u) {
            // Jump to where the ray leaves this empty chunk.
            let cell_min = vec3<f32>(grid_min + cell * CHUNK);
            let boundary = select(cell_min, cell_min + vec3<f32>(f32(CHUNK)), positive);
            let t_leave = (boundary - origin) * inv;
            var axis = 0;
            var t_axis = t_leave.x;
            if (t_leave.y < t_axis) {
                axis = 1;
                t_axis = t_leave.y;
            }
            if (t_leave.z < t_axis) {
                axis = 2;
                t_axis = t_leave.z;
            }
            t_cur = t_axis;
            voxel = vec3<i32>(floor(origin + dir * t_axis));
            voxel[axis] = select(i32(cell_min[axis]) - 1, i32(cell_min[axis]) + CHUNK, positive[axis]);
            last_axis = axis;
            t_max = (vec3<f32>(voxel) + select(vec3<f32>(0.0), vec3<f32>(1.0), positive) - origin) * inv;
            continue;
        }
        let found = voxel_in(slot, rel - cell * CHUNK);
        if (select(found != 0u, found != inside, inside != 0u)) {
            var normal = -dir;
            if (last_axis >= 0) {
                normal = vec3<f32>(0.0);
                normal[last_axis] = -f32(step[last_axis]);
            }
            return Hit(true, t_cur, normal, found, 0u);
        }
        var axis = 0;
        var t_axis = t_max.x;
        if (t_max.y < t_axis) {
            axis = 1;
            t_axis = t_max.y;
        }
        if (t_max.z < t_axis) {
            axis = 2;
            t_axis = t_max.z;
        }
        t_cur = t_axis;
        t_max[axis] += delta[axis];
        voxel[axis] += step[axis];
        last_axis = axis;
    }
    return miss();
}

// Traces `model` (the record of Model `index`, or a one-chunk stand-in for it): the ray is moved into the Model's Shape
// space (the transforms are signed axis permutations, so lengths and `t` carry over unchanged) and the hit normal is
// moved back.
fn trace_record(model: ModelRecord, index: u32, origin: vec3<f32>, dir: vec3<f32>, inside: u32) -> Hit {
    let moved = origin - model.offset.xyz;
    let local_origin = vec3<f32>(dot(model.column_x.xyz, moved), dot(model.column_y.xyz, moved), dot(model.column_z.xyz, moved));
    let local_dir = vec3<f32>(dot(model.column_x.xyz, dir), dot(model.column_y.xyz, dir), dot(model.column_z.xyz, dir));
    var result = trace_voxels(model, local_origin, local_dir, inside);
    if (result.hit) {
        let n = result.normal;
        result.normal = model.column_x.xyz * n.x + model.column_y.xyz * n.y + model.column_z.xyz * n.z;
        result.model = index;
    }
    return result;
}

fn trace_model(index: u32, origin: vec3<f32>, dir: vec3<f32>, inside: u32) -> Hit {
    return trace_record(models[index], index, origin, dir, inside);
}

// `trace_scene_hit` (nearest hit over all visible Models) and `occluded` (shadow rays) come from the traversal
// variant that is compiled in: trace_dda.wgsl (software) or trace_rq.wgsl (hardware ray query). Both return the same
// `Hit`, so everything below is shared.

// The nearest surface along the ray: every visible Model, plus the ground plane when it is on. A ray travelling
// through glass (`inside` != 0) only looks at the Model it is in. Where Models overlap, the earlier one wins.
fn trace(origin: vec3<f32>, dir: vec3<f32>, inside: u32, inside_model: u32) -> Hit {
    var result = trace_scene_hit(origin, dir, inside, inside_model);
    if (inside == 0u && params.options.x != 0u && params.style.x != STYLE_WIRE && dir.y < -1.0e-5 && origin.y > params.ground.w) {
        let t_ground = (params.ground.w - origin.y) / dir.y;
        if (t_ground < result.t) {
            result = Hit(true, t_ground, vec3<f32>(0.0, 1.0, 0.0), GROUND_ID, 0u);
        }
    }
    return result;
}

// ---------------------------------------------------------------------------------------------------------------
// Lighting.

fn sky(dir: vec3<f32>) -> vec3<f32> {
    var color = mix(params.sky_horizon.rgb, params.sky_zenith.rgb, pow(clamp(dir.y, 0.0, 1.0), 0.45));
    if (dir.y < 0.0) {
        color = params.sky_horizon.rgb * 0.5;
    }
    let along_sun = max(dot(dir, params.sun_dir.xyz), 0.0);
    return color + params.sun_color.rgb * pow(along_sun, 48.0) * 0.06;
}

fn sun_disc(dir: vec3<f32>) -> vec3<f32> {
    if (dot(dir, params.sun_dir.xyz) > cos(params.sun_dir.w)) {
        return params.sun_color.rgb * 24.0;
    }
    return vec3<f32>(0.0);
}

// A direction inside the cone of the sun disc, for soft shadows.
fn sun_sample() -> vec3<f32> {
    let frame = basis(params.sun_dir.xyz);
    let radius = tan(params.sun_dir.w) * sqrt(rand());
    let phi = 2.0 * PI * rand();
    return normalize(params.sun_dir.xyz + frame[0] * (radius * cos(phi)) + frame[1] * (radius * sin(phi)));
}

fn schlick(cos_theta: f32, r0: f32) -> f32 {
    return r0 + (1.0 - r0) * pow(clamp(1.0 - cos_theta, 0.0, 1.0), 5.0);
}

// Whether `point` can see the sun along `dir`: 1.0 in the open, 0.0 when something is in the way.
fn sun_visibility(point: vec3<f32>, dir: vec3<f32>) -> f32 {
    return select(1.0, 0.0, occluded(point, dir));
}

fn luminance(color: vec3<f32>) -> f32 {
    return dot(color, vec3<f32>(0.2126, 0.7152, 0.0722));
}

// HOT PATH: one path through the scene. Returns premultiplied radiance in rgb and coverage in a.
fn radiance(start: vec3<f32>, start_dir: vec3<f32>) -> vec4<f32> {
    var origin = start;
    var dir = start_dir;
    var throughput = vec3<f32>(1.0);
    var color = vec3<f32>(0.0);
    var alpha = 1.0;
    // The Color of the glass the ray is travelling through, or 0 in open air.
    var inside = 0u;
    var inside_model = 0u;
    var bounces = 0u;
    var specular = true;
    let max_bounces = params.image.w;

    for (var segment = 0u; segment < (max_bounces + 1u) * 2u + 2u; segment += 1u) {
        if (bounces > max_bounces) {
            break;
        }
        let hit = trace(origin, dir, inside, inside_model);

        if (!hit.hit) {
            if (bounces == 0u && params.options.y == 1u) {
                alpha = 0.0;
            } else if (bounces == 0u && params.style.x == STYLE_WIRE) {
                color = wire_background();
            } else {
                color += throughput * (sky(dir) + select(vec3<f32>(0.0), sun_disc(dir), specular));
            }
            break;
        }

        let point = origin + dir * hit.t;
        let facing = max(dot(-dir, hit.normal), 0.0);

        // Leaving glass: refract out or reflect back inside.
        if (inside != 0u) {
            let glass = evaluate_material(inside, point, hit.normal);
            let ratio = glass.ior;
            let sin2_t = ratio * ratio * (1.0 - facing * facing);
            let r0 = pow((1.0 - ratio) / (1.0 + ratio), 2.0);
            if (sin2_t >= 1.0 || rand() < schlick(sqrt(max(1.0 - sin2_t, 0.0)), r0)) {
                dir = reflect(dir, hit.normal);
                origin = point + hit.normal * EPS;
            } else {
                dir = refract(dir, hit.normal, ratio);
                origin = point - hit.normal * EPS;
                inside = 0u;
            }
            bounces += 1u;
            specular = true;
            continue;
        }

        var surface: Surface;
        if (hit.color_index == GROUND_ID) {
            surface.base_color = params.ground.rgb;
            surface.normal = hit.normal;
            surface.roughness = 1.0;
            surface.ior = 1.5;
        } else {
            surface = evaluate_material(hit.color_index, point, hit.normal);
            if (params.style.x == STYLE_CLAY) {
                surface = clay_surface(surface);
            }
            color += throughput * surface.emission_color * surface.emission_strength;
        }
        let normal = surface.normal;

        if (bounces == 0u && (params.style.x == STYLE_TOON || params.style.x == STYLE_WIRE)
            && !(hit.color_index == GROUND_ID && params.options.x == 2u)) {
            // Non-photographic styles shade the first hit only.
            color = select(toon_shade(point, hit, surface), wire_shade(point, hit, surface), params.style.x == STYLE_WIRE);
            break;
        }

        if (hit.color_index == GROUND_ID && bounces == 0u && params.options.x == 2u) {
            // Shadow catcher: show only how much light the ground loses to the model.
            let up = vec3<f32>(0.0, 1.0, 0.0);
            let sun = sun_sample();
            let open_sun = max(sun.y, 0.0) * luminance(params.sun_color.rgb);
            let open_sky = 0.5 * luminance(params.sky_horizon.rgb + params.sky_zenith.rgb);
            let lit = open_sun * sun_visibility(point + up * EPS, sun)
                + open_sky * select(1.0, 0.0, occluded(point + up * EPS, cosine_hemisphere(up)));
            let ratio = clamp(lit / max(open_sun + open_sky, 1.0e-4), 0.0, 1.0);
            if (params.options.y == 1u) {
                color = vec3<f32>(0.0);
                alpha = (1.0 - ratio) * 0.7;
            } else {
                color = params.sky_horizon.rgb * mix(0.45, 1.0, ratio);
            }
            break;
        }

        let r1 = rand();
        if (r1 < surface.transmission) {
            // Glass: reflect or refract at the surface.
            let r0 = pow((1.0 - surface.ior) / (1.0 + surface.ior), 2.0);
            if (rand() < schlick(facing, r0)) {
                dir = reflect(dir, normal);
                origin = point + normal * EPS;
            } else {
                dir = refract(dir, normal, 1.0 / surface.ior);
                if (surface.roughness > 0.02) {
                    dir = normalize(dir + surface.roughness * surface.roughness * 0.5 * unit_sphere());
                }
                origin = point - normal * EPS;
                inside = hit.color_index;
                inside_model = hit.model;
                throughput *= surface.base_color;
            }
            specular = true;
        } else if (r1 < surface.transmission + (1.0 - surface.transmission) * surface.metallic) {
            // Metal: tinted, roughness-blurred mirror.
            let mirror = reflect(dir, normal);
            var blurred = normalize(mirror + surface.roughness * surface.roughness * unit_sphere());
            if (dot(blurred, normal) <= 0.0) {
                blurred = mirror;
            }
            dir = blurred;
            origin = point + normal * EPS;
            throughput *= surface.base_color;
            specular = true;
        } else if (rand() < schlick(facing, 0.04) * clamp(1.0 - surface.roughness, 0.0, 1.0)) {
            // Clear coat on glossy dielectrics.
            dir = reflect(dir, normal);
            origin = point + normal * EPS;
            specular = true;
        } else {
            // Diffuse: sun by next-event estimation, everything else by a cosine-weighted bounce.
            let sun = sun_sample();
            let cos_sun = dot(normal, sun);
            if (cos_sun > 0.0) {
                let shadow_origin = point + normal * EPS;
                color += throughput * surface.base_color * params.sun_color.rgb
                    * (cos_sun * sun_visibility(shadow_origin, sun));
            }
            dir = cosine_hemisphere(normal);
            origin = point + normal * EPS;
            throughput *= surface.base_color;
            specular = false;
        }
        bounces += 1u;

        if (bounces > 3u) {
            let survive = clamp(max(throughput.x, max(throughput.y, throughput.z)), 0.05, 1.0);
            if (rand() > survive) {
                break;
            }
            throughput /= survive;
        }
    }
    return vec4<f32>(min(color, vec3<f32>(24.0)), alpha);
}

@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let width = params.image.x;
    let height = params.image.y;
    if (id.x >= width || id.y >= height) {
        return;
    }
    var result = vec4<f32>(0.0);
    let in_region = id.x >= params.region.x && id.y >= params.region.y && id.x < params.region.z && id.y < params.region.w;
    let block = max(params.style.y, 1u);
    let pixelated = params.style.x == STYLE_PIXEL;
    // The Pixel style traces one pixel per block; the resolve pass copies it over the rest of the block.
    let traced = in_region && (!pixelated || (id.x % block == 0u && id.y % block == 0u));
    if (traced) {
        let done = params.image.z;
        let count = params.options.z;
        let spread = select(1.0, f32(block), pixelated);
        var sum = vec4<f32>(0.0);
        for (var s = 0u; s < count; s += 1u) {
            rng_state = pcg(id.x + id.y * width + pcg(done + s + 1u) * 2654435761u);
            let position = vec2<f32>(id.xy) + vec2<f32>(rand(), rand()) * spread;
            var ray = primary_ray(position);
            ray = thin_lens(ray);
            sum += radiance(ray.origin, ray.dir);
        }
        let previous = textureLoad(history, vec2<i32>(id.xy), 0);
        result = mix(previous, sum / f32(count), f32(count) / f32(done + count));
        if (params.style.z == 1u) {
            write_gbuffer(id.xy, pixel_centre(id.xy));
        }
    }
    textureStore(output, vec2<i32>(id.xy), result);
}
