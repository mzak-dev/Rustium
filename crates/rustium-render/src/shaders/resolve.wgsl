// Resolve passes between the accumulated image and what is shown or saved: the Pixel style's nearest upscale, an
// edge-aware a-trous denoiser guided by the G-buffer (normal, plane distance, albedo) and the Toon outlines.

struct Resolve {
    // Clip space to voxel space, the same matrix the path tracer used.
    inv_view_proj: mat4x4<f32>,
    // x: image width, y: image height, z: style (STYLE_*), w: pixel size of the Pixel style.
    image: vec4<u32>,
    // x: 1 when the source holds demodulated light that must get its albedo back, y: outline width in pixels,
    // z: samples per pixel accumulated so far, w: pass (0 beauty, others are traced as display-ready colours).
    options: vec4<u32>,
    // x: lens radius in voxels (0 = pinhole), y: focus distance in voxels.
    lens: vec4<f32>,
};

@group(0) @binding(0) var<uniform> resolve: Resolve;
@group(0) @binding(1) var source: texture_2d<f32>;
@group(0) @binding(2) var geometry: texture_2d<f32>;
@group(0) @binding(3) var albedo_texture: texture_2d<f32>;
@group(0) @binding(4) var destination: texture_storage_2d<rgba32float, write>;

const STYLE_TOON: u32 = 2u;
const STYLE_PIXEL: u32 = 3u;
const SKY_DISTANCE: f32 = 1.0e5;
const MIN_ALBEDO: f32 = 0.03;

fn last_pixel() -> vec2<i32> {
    return vec2<i32>(resolve.image.xy) - vec2<i32>(1);
}

// The pixel that holds the traced value for `pixel` (the top-left of its block in the Pixel style).
fn snap(pixel: vec2<i32>) -> vec2<i32> {
    let clamped = clamp(pixel, vec2<i32>(0), last_pixel());
    if (resolve.image.z == STYLE_PIXEL) {
        let block = i32(max(resolve.image.w, 1u));
        return clamped / block * block;
    }
    return clamped;
}

fn pixel_step() -> i32 {
    return select(1, i32(max(resolve.image.w, 1u)), resolve.image.z == STYLE_PIXEL);
}

fn centre_of(pixel: vec2<i32>) -> vec2<f32> {
    let offset = select(0.5, 0.5 * f32(max(resolve.image.w, 1u)), resolve.image.z == STYLE_PIXEL);
    return vec2<f32>(snap(pixel)) + vec2<f32>(offset);
}

// Scene position of the first hit of `pixel` at distance `t`.
fn position_of(pixel: vec2<i32>, t: f32) -> vec3<f32> {
    let uv = centre_of(pixel) / vec2<f32>(f32(resolve.image.x), f32(resolve.image.y));
    let ndc = vec2<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0);
    let near = resolve.inv_view_proj * vec4<f32>(ndc, 0.0, 1.0);
    let far = resolve.inv_view_proj * vec4<f32>(ndc, 1.0, 1.0);
    let origin = near.xyz / near.w;
    return origin + normalize(far.xyz / far.w - origin) * t;
}

fn luminance(color: vec3<f32>) -> f32 {
    return dot(color, vec3<f32>(0.2126, 0.7152, 0.0722));
}

// Radius in pixels of the blur depth of field gives a surface at `distance` from the camera; 0 for a pinhole. The
// guides (normal, distance, albedo) come from the pinhole ray through the pixel's centre, so where the picture is
// blurred they no longer describe it and the denoiser has to trust them less.
fn defocus_blur(pixel: vec2<i32>, hit_distance: f32) -> f32 {
    if (resolve.lens.x <= 0.0 || hit_distance > SKY_DISTANCE) {
        return 0.0;
    }
    let radius = resolve.lens.x * abs(hit_distance - resolve.lens.y) / max(resolve.lens.y, 1.0e-3);
    let footprint = distance(position_of(pixel + vec2<i32>(1, 0), hit_distance), position_of(pixel, hit_distance));
    return radius / max(footprint, 1.0e-4);
}

// The albedo the denoiser divides by: the pinhole albedo fades to white as the pixel gets blurred, because a blurred
// colour is a mix of surfaces and no single albedo describes it.
fn effective_albedo(pixel: vec2<i32>) -> vec3<f32> {
    let albedo = textureLoad(albedo_texture, pixel, 0).rgb;
    let blur = defocus_blur(pixel, textureLoad(geometry, pixel, 0).w);
    return mix(albedo, vec3<f32>(1.0), clamp(blur * 0.5, 0.0, 1.0));
}

@compute @workgroup_size(8, 8, 1)
fn demodulate(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= resolve.image.x || id.y >= resolve.image.y) {
        return;
    }
    let pixel = snap(vec2<i32>(id.xy));
    let light = textureLoad(source, pixel, 0);
    let albedo = max(effective_albedo(pixel), vec3<f32>(MIN_ALBEDO));
    textureStore(destination, vec2<i32>(id.xy), vec4<f32>(light.rgb / albedo, light.a));
}

fn atrous(id: vec2<u32>, spacing: i32) {
    if (id.x >= resolve.image.x || id.y >= resolve.image.y) {
        return;
    }
    let pixel = vec2<i32>(id);
    let stride = spacing * pixel_step();
    let centre = textureLoad(source, pixel, 0);
    let centre_geometry = textureLoad(geometry, snap(pixel), 0);
    let centre_albedo = textureLoad(albedo_texture, snap(pixel), 0).rgb;
    let centre_position = position_of(pixel, centre_geometry.w);
    let centre_sky = centre_geometry.w > SKY_DISTANCE;
    // Blurred surfaces: the guides are loosened by the blur radius so the filter can average across the blur.
    let centre_blur = defocus_blur(snap(pixel), centre_geometry.w);
    let centre_luminance = luminance(centre.rgb);
    // Fewer samples mean more noise, so the colour term gets looser.
    let sigma_color = clamp(2.5 / sqrt(f32(max(resolve.options.z, 1u))), 0.12, 1.0);
    var weights = array<f32, 5>(0.0625, 0.25, 0.375, 0.25, 0.0625);

    var total = vec4<f32>(0.0);
    var total_weight = 0.0;
    for (var dy = -2; dy <= 2; dy += 1) {
        for (var dx = -2; dx <= 2; dx += 1) {
            let other_pixel = clamp(pixel + vec2<i32>(dx, dy) * stride, vec2<i32>(0), last_pixel());
            let other = textureLoad(source, other_pixel, 0);
            let other_geometry = textureLoad(geometry, snap(other_pixel), 0);
            let other_albedo = textureLoad(albedo_texture, snap(other_pixel), 0).rgb;
            var weight = weights[dx + 2] * weights[dy + 2];
            let relax = 1.0 + max(centre_blur, defocus_blur(snap(other_pixel), other_geometry.w));
            let across_sky = (other_geometry.w > SKY_DISTANCE) != centre_sky;
            if (across_sky && relax < 3.0) {
                continue;
            }
            // Across a blurred silhouette only the colour says what belongs together.
            if (!centre_sky && !across_sky) {
                // The normal stops mattering where the blur is wider than a few pixels.
                weight *= mix(pow(max(dot(centre_geometry.xyz, other_geometry.xyz), 0.0), 32.0), 1.0, clamp((relax - 1.0) * 0.25, 0.0, 1.0));
                let off_plane = abs(dot(position_of(other_pixel, other_geometry.w) - centre_position, centre_geometry.xyz));
                weight *= exp(-off_plane / ((0.25 + 0.004 * centre_geometry.w) * relax));
                let albedo_gap = centre_albedo - other_albedo;
                weight *= exp(-dot(albedo_gap, albedo_gap) / (0.02 * relax * relax));
            }
            let difference = luminance(other.rgb) - centre_luminance;
            weight *= exp(-difference * difference / (sigma_color * sigma_color * (centre_luminance * centre_luminance + 0.25)));
            total += other * weight;
            total_weight += weight;
        }
    }
    textureStore(destination, pixel, select(centre, total / total_weight, total_weight > 1.0e-6));
}

@compute @workgroup_size(8, 8, 1)
fn atrous_1(@builtin(global_invocation_id) id: vec3<u32>) {
    atrous(id.xy, 1);
}

@compute @workgroup_size(8, 8, 1)
fn atrous_2(@builtin(global_invocation_id) id: vec3<u32>) {
    atrous(id.xy, 2);
}

@compute @workgroup_size(8, 8, 1)
fn atrous_4(@builtin(global_invocation_id) id: vec3<u32>) {
    atrous(id.xy, 4);
}

// Whether the Toon outline covers `pixel`: a neighbour in front of it, or a crease between two faces.
fn on_outline(pixel: vec2<i32>) -> bool {
    let centre_geometry = textureLoad(geometry, pixel, 0);
    let centre_sky = centre_geometry.w > SKY_DISTANCE;
    let centre_position = position_of(pixel, centre_geometry.w);
    let reach = i32(max(resolve.options.y, 1u));
    for (var side = 0; side < 4; side += 1) {
        var direction = vec2<i32>(reach, 0);
        if (side == 1) {
            direction = vec2<i32>(-reach, 0);
        } else if (side == 2) {
            direction = vec2<i32>(0, reach);
        } else if (side == 3) {
            direction = vec2<i32>(0, -reach);
        }
        let other_pixel = clamp(pixel + direction, vec2<i32>(0), last_pixel());
        let other = textureLoad(geometry, other_pixel, 0);
        let other_sky = other.w > SKY_DISTANCE;
        if (centre_sky) {
            if (!other_sky) {
                return true;
            }
            continue;
        }
        if (other_sky) {
            continue;
        }
        let off_plane = abs(dot(position_of(other_pixel, other.w) - centre_position, centre_geometry.xyz));
        let tolerance = 0.2 + 0.003 * centre_geometry.w;
        if (off_plane > tolerance && other.w < centre_geometry.w) {
            return true;
        }
        if (dot(centre_geometry.xyz, other.xyz) < 0.5 && off_plane < tolerance && other.w <= centre_geometry.w + 0.05) {
            return true;
        }
    }
    return false;
}

@compute @workgroup_size(8, 8, 1)
fn finish(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= resolve.image.x || id.y >= resolve.image.y) {
        return;
    }
    let pixel = vec2<i32>(id.xy);
    let source_pixel = snap(pixel);
    var color = textureLoad(source, source_pixel, 0);
    if (resolve.options.x == 1u) {
        let albedo = max(effective_albedo(source_pixel), vec3<f32>(MIN_ALBEDO));
        color = vec4<f32>(color.rgb * albedo, color.a);
    }
    // View passes were traced as display-ready colours: no outlines on them.
    if (resolve.image.z == STYLE_TOON && resolve.options.w == 0u && on_outline(pixel)) {
        color = vec4<f32>(0.004, 0.005, 0.007, 1.0);
    }
    textureStore(destination, pixel, color);
}

// A view pass: `source` is layer 0 of the accumulated passes (albedo, coverage) and `geometry` layer 1 (normal colour,
// depth grey). They are display-ready already, so there is nothing to denoise and no outline.
@compute @workgroup_size(8, 8, 1)
fn finish_pass(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= resolve.image.x || id.y >= resolve.image.y) {
        return;
    }
    let source_pixel = snap(vec2<i32>(id.xy));
    let first = textureLoad(source, source_pixel, 0);
    let second = textureLoad(geometry, source_pixel, 0);
    var color = first;
    if (resolve.options.w == 2u) {
        color = vec4<f32>(second.rgb, first.a);
    } else if (resolve.options.w == 3u) {
        color = vec4<f32>(vec3<f32>(second.a), first.a);
    }
    textureStore(destination, vec2<i32>(id.xy), color);
}
