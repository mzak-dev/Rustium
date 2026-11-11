// Greedy-meshed voxel chunks. Vertex packing matches rustium_core::MeshVertex.
@group(0) @binding(1) var<uniform> palette: array<vec4<f32>, 256>;

struct VertexOutput {
    @builtin(position) clip: vec4<f32>,
    @location(0) albedo: vec3<f32>,
    @location(1) @interpolate(flat) face: u32,
    @location(2) ao: f32,
};

@vertex
fn vs_main(@location(0) position: vec3<f32>, @location(1) packed: u32) -> VertexOutput {
    var out: VertexOutput;
    out.clip = camera.view_proj * vec4<f32>(position, 1.0);
    out.face = packed & 7u;
    out.ao = f32((packed >> 3u) & 3u) / 3.0;
    out.albedo = palette[(packed >> 8u) & 255u].rgb;
    return out;
}

fn face_normal(face: u32) -> vec3<f32> {
    let axis = face / 2u;
    let sign = select(1.0, -1.0, (face & 1u) == 1u);
    return vec3<f32>(f32(axis == 0u), f32(axis == 1u), f32(axis == 2u)) * sign;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let normal = face_normal(in.face);
    let sky = vec3<f32>(0.62, 0.68, 0.78);
    let ground = vec3<f32>(0.24, 0.22, 0.21);
    let ambient = mix(ground, sky, normal.y * 0.5 + 0.5);
    let sun = max(dot(normal, camera.sun.xyz), 0.0);
    let occlusion = mix(0.38, 1.0, in.ao);
    let light = ambient * 0.62 + vec3<f32>(1.0, 0.96, 0.9) * sun * 0.7;
    return vec4<f32>(in.albedo * light * occlusion, 1.0);
}
