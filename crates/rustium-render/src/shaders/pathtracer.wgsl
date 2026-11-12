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
    // Pixels outside [xy, zw) get no new samples: they keep what they held.
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
    // x: first row traced by this dispatch, y: one past the last row (the other rows were copied beforehand).
    band: vec4<u32>,
    // x: view pass (0 = beauty, 1 = albedo, 2 = normal, 3 = depth), y/z: nearest and farthest distance of the Models
    // along the view direction, for the depth pass.
    aov: vec4<f32>,
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
const GROUND_SLACK: f32 = 4.0 * EPS;
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
    // Shadow walks only: optical depth of the haze the ray crossed before the hit (or the end of the walk).
    optical: f32,
};

// How `trace_voxels` walks.
// MODE_NEAREST: the first non-empty voxel. MODE_INSIDE: through voxels of one Material (glass, haze) up to the first
// voxel of another Material, or air; that is always a hit, a grid edge counts as air (Color 0).
// MODE_SHADOW: the first voxel that blocks sunlight; voxels whose Material does not cast shadows are skipped and haze
// only adds to `Hit::optical`.
const MODE_NEAREST: u32 = 0u;
const MODE_INSIDE: u32 = 1u;
const MODE_SHADOW: u32 = 2u;
// Optical depth beyond which a shadow ray counts as blocked.
const OPAQUE_DEPTH: f32 = 12.0;

fn miss() -> Hit {
    return Hit(false, FAR, vec3<f32>(0.0, 1.0, 0.0), 0u, 0u, 0.0);
}

// The face normal for a ray that crossed into its current voxel through `axis` (-1: it started there).
fn crossing_normal(dir: vec3<f32>, step: vec3<i32>, axis: i32) -> vec3<f32> {
    var normal = -dir;
    if (axis >= 0) {
        normal = vec3<f32>(0.0);
        normal[axis] = -f32(step[axis]);
    }
    return normal;
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
// chunks are walked voxel by voxel. `mode` picks what counts as a hit (see MODE_*); `inside` is the Material index + 1
// of the glass or haze the ray travels through in MODE_INSIDE.
fn trace_voxels(model: ModelRecord, origin: vec3<f32>, dir: vec3<f32>, mode: u32, inside: u32) -> Hit {
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
        if (mode == MODE_INSIDE) {
            return Hit(true, 0.0, -dir, 0u, 0u, 0.0);
        }
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
    var optical = 0.0;

    for (var iteration = 0; iteration < 4096; iteration += 1) {
        let rel = voxel - grid_min;
        if (any(rel < vec3<i32>(0)) || any(rel >= size_voxels)) {
            break;
        }
        let cell = vec3<i32>(rel.x >> 5u, rel.y >> 5u, rel.z >> 5u);
        let slot = chunk_slot(model, cell);
        if (slot == 0u && mode == MODE_INSIDE) {
            // An empty chunk is air: the medium ends here.
            return Hit(true, t_cur, crossing_normal(dir, step, last_axis), 0u, 0u, 0.0);
        }
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
        if (mode == MODE_INSIDE) {
            if (found == 0u || color_graph(found) + 1u != inside) {
                return Hit(true, t_cur, crossing_normal(dir, step, last_axis), found, 0u, 0.0);
            }
        } else if (mode == MODE_SHADOW) {
            if (found != 0u) {
                var flags = color_flags(found);
                if (params.style.x == STYLE_CLAY || params.style.x == STYLE_TOON) {
                    // These styles draw glass and haze as solid, so they shade like solids.
                    flags = 0u;
                }
                if ((flags & COLOR_NO_SHADOW) == 0u) {
                    if ((flags & COLOR_VOLUMETRIC) == 0u) {
                        return Hit(true, t_cur, crossing_normal(dir, step, last_axis), found, 0u, optical);
                    }
                    // Haze dims the light by its density over the stretch of the voxel the ray crosses.
                    let t_next = min(t_max.x, min(t_max.y, t_max.z));
                    let centre = vec3<f32>(voxel) + vec3<f32>(0.5);
                    optical += evaluate_material(found, centre, vec3<f32>(0.0, 1.0, 0.0)).volume_density
                        * max(t_next - t_cur, 0.0);
                    if (optical > OPAQUE_DEPTH) {
                        return Hit(true, t_cur, crossing_normal(dir, step, last_axis), found, 0u, optical);
                    }
                }
            }
        } else if (found != 0u) {
            return Hit(true, t_cur, crossing_normal(dir, step, last_axis), found, 0u, 0.0);
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
    if (mode == MODE_INSIDE) {
        // The ray left the grid (or the walk ran out): beyond is air.
        return Hit(true, t_cur, crossing_normal(dir, step, last_axis), 0u, 0u, 0.0);
    }
    var result = miss();
    result.optical = optical;
    return result;
}

// Traces `model` (the record of Model `index`, or a one-chunk stand-in for it): the ray is moved into the Model's Shape
// space (the transforms are signed axis permutations, so lengths and `t` carry over unchanged) and the hit normal is
// moved back. `mode` and `inside` as for `trace_voxels`.
fn trace_record(model: ModelRecord, index: u32, origin: vec3<f32>, dir: vec3<f32>, mode: u32, inside: u32) -> Hit {
    let moved = origin - model.offset.xyz;
    let local_origin = vec3<f32>(dot(model.column_x.xyz, moved), dot(model.column_y.xyz, moved), dot(model.column_z.xyz, moved));
    let local_dir = vec3<f32>(dot(model.column_x.xyz, dir), dot(model.column_y.xyz, dir), dot(model.column_z.xyz, dir));
    var result = trace_voxels(model, local_origin, local_dir, mode, inside);
    if (result.hit) {
        let n = result.normal;
        result.normal = model.column_x.xyz * n.x + model.column_y.xyz * n.y + model.column_z.xyz * n.z;
        result.model = index;
    }
    return result;
}

fn trace_model(index: u32, origin: vec3<f32>, dir: vec3<f32>, mode: u32, inside: u32) -> Hit {
    return trace_record(models[index], index, origin, dir, mode, inside);
}

// `trace_scene_hit` (nearest hit over all visible Models) and `shadow_transmittance` (the fraction of sunlight
// along a ray: 0 behind a voxel that casts shadows, dimmed by haze, 1 in the open) come from the traversal
// variant that is compiled in: trace_dda.wgsl (software) or trace_rq.wgsl (hardware ray query). Both return the same
// `Hit`, so everything below is shared.

// The nearest surface along the ray: every visible Model, plus the ground plane when it is on. A ray travelling
// through glass or haze (`inside` = Material index + 1) only looks at the Model it is in. Where Models overlap, the earlier one wins.
fn trace(origin: vec3<f32>, dir: vec3<f32>, inside: u32, inside_model: u32) -> Hit {
    var result = trace_scene_hit(origin, dir, inside, inside_model);
    // A ray that has just left the underside of a Model (or glass resting on the ground) starts a hair below the
    // ground, so the plane counts from `GROUND_SLACK` under it; such a ray meets it at once.
    if (inside == 0u && params.options.x != 0u && params.style.x != STYLE_WIRE && dir.y < -1.0e-5
        && origin.y > params.ground.w - GROUND_SLACK) {
        let t_ground = max((params.ground.w - origin.y) / dir.y, 0.0);
        if (t_ground < result.t) {
            result = Hit(true, t_ground, vec3<f32>(0.0, 1.0, 0.0), GROUND_ID, 0u, 0.0);
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
    return shadow_transmittance(point, dir);
}

fn luminance(color: vec3<f32>) -> f32 {
    return dot(color, vec3<f32>(0.2126, 0.7152, 0.0722));
}

// Unpolarised Fresnel reflectance of a dielectric interface for light going from index `n_i` into `n_t`: 0 when the
// indices match, 1 under total internal reflection.
fn fresnel_dielectric(cos_i: f32, n_i: f32, n_t: f32) -> f32 {
    let eta = n_i / n_t;
    let sin2_t = eta * eta * (1.0 - cos_i * cos_i);
    if (sin2_t >= 1.0) {
        return 1.0;
    }
    let cos_t = sqrt(1.0 - sin2_t);
    let rs = (n_i * cos_i - n_t * cos_t) / max(n_i * cos_i + n_t * cos_t, 1.0e-6);
    let rp = (n_t * cos_i - n_i * cos_t) / max(n_t * cos_i + n_i * cos_t, 1.0e-6);
    return 0.5 * (rs * rs + rp * rp);
}

// Henyey-Greenstein phase function for the cosine between the directions of travel before and after scattering.
fn hg_phase(cos_theta: f32, g: f32) -> f32 {
    let denominator = 1.0 + g * g - 2.0 * g * cos_theta;
    return (1.0 - g * g) / (4.0 * PI * pow(max(denominator, 1.0e-4), 1.5));
}

// A new direction of travel after scattering, distributed by the Henyey-Greenstein function around `dir`.
fn sample_hg(dir: vec3<f32>, g: f32) -> vec3<f32> {
    let u = rand();
    var cos_theta = 1.0 - 2.0 * u;
    if (abs(g) > 1.0e-3) {
        let square = (1.0 - g * g) / (1.0 - g + 2.0 * g * u);
        cos_theta = (1.0 + g * g - square * square) / (2.0 * g);
    }
    let sin_theta = sqrt(max(1.0 - cos_theta * cos_theta, 0.0));
    let phi = 2.0 * PI * rand();
    return basis(dir) * vec3<f32>(sin_theta * cos(phi), sin_theta * sin(phi), cos_theta);
}

// Forward scattering of haze, shared by the scatter direction and the sun's phase value.
const HAZE_G: f32 = 0.3;

// Russian roulette: after a few bounces dim paths may end; the survivors are brightened to stay unbiased.
fn russian_roulette(throughput: ptr<function, vec3<f32>>, bounces: u32) -> bool {
    if (bounces > 3u) {
        let value = *throughput;
        let survive = clamp(max(value.x, max(value.y, value.z)), 0.05, 1.0);
        if (rand() > survive) {
            return false;
        }
        *throughput = value / survive;
    }
    return true;
}

// HOT PATH: one path through the scene. Returns premultiplied radiance in rgb and coverage in a.
//
// The path may travel through a medium: glass or haze voxels of one Material (`medium` = Material index + 1, 0 in
// open air). Inside it, haze scatters the ray (free-flight distance from the Material's density, sun by next-event
// estimation) and the first voxel of a different Material is an interface: Fresnel decides between reflecting and
// refracting, total internal reflection included.
fn radiance(start: vec3<f32>, start_dir: vec3<f32>) -> vec4<f32> {
    var origin = start;
    var dir = start_dir;
    var throughput = vec3<f32>(1.0);
    var color = vec3<f32>(0.0);
    var alpha = 1.0;
    var medium = 0u;
    var medium_color = 0u;
    var medium_model = 0u;
    var medium_ior = 1.0;
    var bounces = 0u;
    var specular = true;
    // The ray has not been bent or scattered since the camera, so a sky behind it is the background.
    var straight = true;
    let max_bounces = params.image.w;

    for (var segment = 0u; segment < (max_bounces + 1u) * 3u + 2u; segment += 1u) {
        if (bounces > max_bounces) {
            break;
        }
        let hit = trace(origin, dir, medium, medium_model);

        if (medium != 0u) {
            // Haze: sample where the ray scatters; the Material's colour is the albedo.
            let haze = evaluate_material(medium_color, origin, -dir);
            if (haze.volume_density > 0.0) {
                let flight = -log(max(1.0 - rand(), 1.0e-7)) / haze.volume_density;
                if (flight < hit.t || !hit.hit) {
                    let at = origin + dir * flight;
                    throughput *= min(haze.volume_color, vec3<f32>(1.0));
                    let sun = sun_sample();
                    color += throughput * params.sun_color.rgb
                        * (PI * hg_phase(dot(dir, sun), HAZE_G) * shadow_transmittance(at, sun));
                    dir = sample_hg(dir, HAZE_G);
                    origin = at;
                    bounces += 1u;
                    specular = false;
                    straight = false;
                    if (!russian_roulette(&throughput, bounces)) {
                        break;
                    }
                    continue;
                }
            }
        }

        if (!hit.hit) {
            if (straight && params.options.y == 1u) {
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

        // Color 0 only comes back from inside a medium: the medium ended and air follows.
        let air = hit.color_index == 0u;
        var surface: Surface;
        if (air) {
            surface.base_color = vec3<f32>(1.0);
            surface.normal = hit.normal;
            surface.transmission = 1.0;
            surface.ior = 1.0;
        } else if (hit.color_index == GROUND_ID) {
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

        // Toon sees through glass and haze: the ray passes straight, tinted, and the first opaque surface behind is shaded.
        let toon_clear = params.style.x == STYLE_TOON
            && (air || (hit.color_index != GROUND_ID && (surface.transmission > 0.5 || surface.volume_density > 0.0)));
        if (toon_clear) {
            surface.ior = 1.0;
            surface.roughness = 0.0;
            surface.transmission = 1.0;
        }
        if (bounces == 0u && !toon_clear && (params.style.x == STYLE_TOON || params.style.x == STYLE_WIRE)
            && !(hit.color_index == GROUND_ID && params.options.x == 2u)) {
            // Non-photographic styles shade the first hit only.
            color = throughput * select(toon_shade(point, hit, surface), wire_shade(point, hit, surface), params.style.x == STYLE_WIRE);
            break;
        }

        if (hit.color_index == GROUND_ID && bounces == 0u && params.options.x == 2u) {
            // Shadow catcher: show only how much light the ground loses to the model.
            let up = vec3<f32>(0.0, 1.0, 0.0);
            let sun = sun_sample();
            let open_sun = max(sun.y, 0.0) * luminance(params.sun_color.rgb);
            let open_sky = 0.5 * luminance(params.sky_horizon.rgb + params.sky_zenith.rgb);
            let lit = open_sun * sun_visibility(point + up * EPS, sun)
                + open_sky * shadow_transmittance(point + up * EPS, cosine_hemisphere(up));
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
        var bounced = true;
        if (air || r1 < surface.transmission) {
            // An interface between the current medium (or air) and the next one: reflect or refract.
            let n_from = medium_ior;
            let n_to = select(surface.ior, 1.0, air);
            var roughness = surface.roughness;
            if (air) {
                roughness = evaluate_material(medium_color, point, hit.normal).roughness;
            }
            // Rough glass: the microfacet normal is the face normal scattered by the roughness.
            var facet = hit.normal;
            let alpha_r = roughness * roughness;
            if (alpha_r > 1.0e-4) {
                let scattered = normalize(hit.normal + alpha_r * unit_sphere());
                if (dot(scattered, -dir) > 0.05) {
                    facet = scattered;
                }
            }
            let cos_i = max(dot(-dir, facet), 0.0);
            var refracted = refract(dir, facet, n_from / n_to);
            if (dot(refracted, hit.normal) >= 0.0 || dot(refracted, refracted) < 0.25) {
                refracted = refract(dir, hit.normal, n_from / n_to);
            }
            let can_refract = dot(refracted, refracted) > 0.25;
            if (!can_refract || rand() < fresnel_dielectric(cos_i, n_from, n_to)) {
                var mirror = reflect(dir, facet);
                if (dot(mirror, hit.normal) <= 0.0) {
                    mirror = reflect(dir, hit.normal);
                }
                dir = mirror;
                origin = point + hit.normal * EPS;
                straight = false;
            } else {
                // A straight pass (same index, no roughness) is not a bounce, so fog and clear sheets cost nothing.
                bounced = abs(n_from - n_to) > 1.0e-4 || alpha_r > 1.0e-4;
                straight = straight && !bounced;
                dir = normalize(refracted);
                origin = point - hit.normal * EPS;
                if (air) {
                    medium = 0u;
                    medium_ior = 1.0;
                } else {
                    // Glass is tinted by its base color; haze that does not bend light is tinted by its scattering.
                    if (surface.ior > 1.001 || surface.volume_density <= 0.0) {
                        throughput *= surface.base_color;
                    }
                    medium = color_graph(hit.color_index) + 1u;
                    medium_color = hit.color_index;
                    medium_model = hit.model;
                    medium_ior = surface.ior;
                }
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
            straight = false;
        } else if (rand() < schlick(facing, 0.04) * clamp(1.0 - surface.roughness, 0.0, 1.0)) {
            // Clear coat on glossy dielectrics.
            dir = reflect(dir, normal);
            origin = point + normal * EPS;
            specular = true;
            straight = false;
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
            straight = false;
        }
        if (bounced) {
            bounces += 1u;
        }
        if (!russian_roulette(&throughput, bounces)) {
            break;
        }
    }
    return vec4<f32>(min(color, vec3<f32>(24.0)), alpha);
}

@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) global_id: vec3<u32>) {
    // The dispatch covers the rows of this pass's band only.
    let id = vec3<u32>(global_id.x, global_id.y + params.band.x, global_id.z);
    let width = params.image.x;
    let height = params.image.y;
    if (id.x >= width || id.y >= height || id.y >= params.band.y) {
        return;
    }
    let previous = textureLoad(history, vec2<i32>(id.xy), 0);
    var result = vec4<f32>(0.0);
    let in_region = id.x >= params.region.x && id.y >= params.region.y && id.x < params.region.z && id.y < params.region.w;
    if (!in_region) {
        // Outside the region nothing new is traced: the picture there stays as it was.
        textureStore(output, vec2<i32>(id.xy), previous);
        return;
    }
    let block = max(params.style.y, 1u);
    let pixelated = params.style.x == STYLE_PIXEL;
    // The Pixel style traces one pixel per block; the resolve pass copies it over the rest of the block.
    let traced = !pixelated || (id.x % block == 0u && id.y % block == 0u);
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
            if (params.aov.x > 0.5) {
                sum += aov_sample(ray);
            } else {
                sum += radiance(ray.origin, ray.dir);
            }
        }
        result = mix(previous, sum / f32(count), f32(count) / f32(done + count));
        if (params.style.z == 1u) {
            write_gbuffer(id.xy, pixel_centre(id.xy));
        }
    }
    textureStore(output, vec2<i32>(id.xy), result);
}
