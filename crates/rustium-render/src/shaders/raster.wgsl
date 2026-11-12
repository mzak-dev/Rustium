// Greedy-meshed voxel chunks. Vertex packing matches rustium_core::MeshVertex. All Color and Material data is read by
// `evaluate_material` (material.wgsl, concatenated before common.wgsl).
// Every Model is drawn as one instance: three columns (images of the X, Y and Z axes) and an origin place the
// Shape in the Scene. column_x.w is the dim factor of Models outside the Focus, column_y.w the drawing rank.

struct VertexOutput {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) @interpolate(flat) normal: vec3<f32>,
    @location(2) ao: f32,
    @location(3) @interpolate(flat) dim: f32,
    @location(4) @interpolate(flat) color_index: u32,
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
    out.color_index = (packed >> 8u) & 255u;
    out.dim = column_x.w;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // The mesh's per-vertex occlusion feeds the Ambient occlusion node.
    material_ao = in.ao;
    let surface = evaluate_material(in.color_index, in.world, in.normal);
    // The Material may bend the normal (Voxel bevel); it only changes the shading.
    let normal = surface.normal;
    // Glowing voxels show their emission color unlit (not dimmed); the path tracer is the reference for how they
    // light things. Transmission stays opaque here until the dithered transparency of M4.
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
