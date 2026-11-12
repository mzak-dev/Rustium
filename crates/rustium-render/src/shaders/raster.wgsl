// Greedy-meshed voxel chunks. Vertex packing matches rustium_core::MeshVertex. All Color and Material data is read by
// `evaluate_material` (material.wgsl, concatenated before common.wgsl).
// Every Model is drawn as one instance: three columns (images of the X, Y and Z axes) and an origin place the
// Shape in the Scene. column_x.w is the dim factor of Models outside the Focus, column_y.w the drawing rank.

// x: frame number, which moves the dither pattern from frame to frame. Binding 2 of the scene group.
struct Dither {
    frame: vec4<f32>,
};

@group(0) @binding(2) var<uniform> dither: Dither;

// Interleaved gradient noise (Jimenez) gives every pixel its own offset; the frame then walks the threshold along
// the golden-ratio sequence, which spreads the thresholds of one pixel evenly over time (a low-discrepancy sequence),
// so the temporal average of the dither converges much faster than with random shifts and no pattern stays behind.
fn dither_threshold(pixel: vec2<f32>, frame: f32) -> f32 {
    let offset = fract(52.9829189 * fract(dot(pixel, vec2<f32>(0.06711056, 0.00583715))));
    let step = frame - 4096.0 * floor(frame / 4096.0);
    return fract(offset + 0.6180339887 * step);
}

// Fraction of the light the voxel's surface stops: an opaque Material 1, clear glass a little (its tint, a trace of
// reflection that grows at grazing angles), haze by its density as if it were two voxels thick. The path tracer is
// the reference for how transparency looks; this only has to read as glass or haze while editing.
fn surface_coverage(surface: Surface, facing: f32) -> f32 {
    let r0 = pow((surface.ior - 1.0) / (surface.ior + 1.0), 2.0);
    let reflection = r0 + (1.0 - r0) * pow(1.0 - clamp(facing, 0.0, 1.0), 5.0);
    let tint = 1.0 - luma(surface.base_color);
    let glass = clamp(reflection * 1.5 + tint * 0.9 + 0.1, 0.0, 1.0);
    let haze = 1.0 - exp(-surface.volume_density * 2.0);
    let through = surface.transmission * (1.0 - glass);
    return clamp(1.0 - through * (1.0 - haze), 0.0, 1.0);
}

struct VertexOutput {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) @interpolate(flat) normal: vec3<f32>,
    @location(2) ao: f32,
    @location(3) @interpolate(flat) dim: f32,
    @location(4) @interpolate(flat) color_index: u32,
    @location(5) local: vec3<f32>,
    @location(6) @interpolate(flat) local_normal: vec3<f32>,
};

fn face_normal(face: u32) -> vec3<f32> {
    let axis = face / 2u;
    let sign = select(1.0, -1.0, (face & 1u) == 1u);
    return vec3<f32>(f32(axis == 0u), f32(axis == 1u), f32(axis == 2u)) * sign;
}

@vertex
fn vs_main(
    @location(0) position: vec3<f32>,
    @location(1) packed: u32,
    @location(2) column_x: vec4<f32>,
    @location(3) column_y: vec4<f32>,
    @location(4) column_z: vec4<f32>,
    @location(5) origin: vec4<f32>,
) -> VertexOutput {
    var out: VertexOutput;
    let world = column_x.xyz * position.x + column_y.xyz * position.y + column_z.xyz * position.z + origin.xyz;
    out.clip = camera.view_proj * vec4<f32>(world, 1.0);
    // Where Models overlap the one higher in the tree (lower rank) must win; the nudge is far below one voxel.
    out.clip.z += min(column_y.w, 32.0) * 4e-7 * out.clip.w;
    let local_normal = face_normal(packed & 7u);
    out.normal = column_x.xyz * local_normal.x + column_y.xyz * local_normal.y + column_z.xyz * local_normal.z;
    out.ao = f32((packed >> 3u) & 3u) / 3.0;
    out.world = world;
    out.local = position;
    out.local_normal = local_normal;
    out.color_index = (packed >> 8u) & 255u;
    out.dim = column_x.w;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // The mesh's per-vertex occlusion feeds the Ambient occlusion node.
    material_ao = in.ao;
    let surface = evaluate_material(in.color_index, in.world, in.normal, in.local, in.local_normal);
    // The Material may bend the normal (Voxel bevel); it only changes the shading.
    let normal = surface.normal;
    // Glass and haze: draw only the share of pixels that the surface covers, a different share each frame, and let
    // the temporal pass average them (ADR 0006).
    if (surface.transmission > 0.0 || surface.volume_density > 0.0) {
        let facing = abs(dot(normalize(camera.eye.xyz - in.world), normal));
        if (dither_threshold(in.clip.xy, dither.frame.x) >= surface_coverage(surface, facing)) {
            discard;
        }
    }
    // Glowing voxels show their emission color unlit (not dimmed); the path tracer is the reference for how they
    // light things.
    if (surface.emission_strength > 0.0) {
        return vec4<f32>(surface.emission_color, 1.0);
    }
    let sky = vec3<f32>(0.62, 0.68, 0.78);
    let ground = vec3<f32>(0.24, 0.22, 0.21);
    let ambient = mix(ground, sky, normal.y * 0.5 + 0.5);
    let sun = max(dot(normal, camera.sun.xyz), 0.0);
    let occlusion = mix(0.38, 1.0, in.ao);
    let light = ambient * 0.62 + vec3<f32>(1.0, 0.96, 0.9) * sun * 0.7;
    var color = surface.base_color * light * occlusion;
    // Models outside the Focus stay readable but recede: desaturate and darken.
    let grey = vec3<f32>(dot(color, vec3<f32>(0.2126, 0.7152, 0.0722)));
    color = mix(color, grey, in.dim * 0.45) * mix(1.0, 0.42, in.dim);
    return vec4<f32>(color, 1.0);
}
