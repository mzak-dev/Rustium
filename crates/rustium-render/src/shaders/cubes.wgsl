// Overlay cubes: voxels in the middle of an edit animation, the cells a Tool will change and the face under the
// pointer. One instance per box; the 36 vertices of the box come from the vertex index.
struct CubeOutput {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
    // Position across the face, -1..1 on both axes, for the outline.
    @location(1) across: vec2<f32>,
    // x: outline strength, y: glow towards white, z: diffuse light.
    @location(2) extra: vec3<f32>,
};

@vertex
fn vs_main(
    @builtin(vertex_index) index: u32,
    @location(0) centre: vec4<f32>,
    @location(1) half_size: vec4<f32>,
    @location(2) color: vec4<f32>,
) -> CubeOutput {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0),
    );
    let face = index / 6u;
    let across = corners[index % 6u];
    let axis = face / 2u;
    let outward = select(-1.0, 1.0, face % 2u == 0u);
    // u x v points along the axis, so flipping u on the negative face keeps every face counter-clockwise outside.
    var local = vec3<f32>(0.0);
    var normal = vec3<f32>(0.0);
    local[axis] = outward;
    normal[axis] = outward;
    local[(axis + 1u) % 3u] = across.x * outward;
    local[(axis + 2u) % 3u] = across.y;
    let world = centre.xyz + local * half_size.xyz;

    var out: CubeOutput;
    out.clip = camera.view_proj * vec4<f32>(world, 1.0);
    out.color = color;
    out.across = across;
    out.extra = vec3<f32>(centre.w, half_size.w, 0.6 + 0.4 * max(dot(normal, camera.sun.xyz), 0.0));
    return out;
}

@fragment
fn fs_main(in: CubeOutput) -> @location(0) vec4<f32> {
    let edge = max(abs(in.across.x), abs(in.across.y));
    let rim = smoothstep(0.82, 0.92, edge) * in.extra.x;
    var rgb = mix(in.color.rgb * in.extra.z, vec3<f32>(1.0), in.extra.y);
    rgb = mix(rgb, vec3<f32>(1.0), rim);
    return vec4<f32>(rgb, max(in.color.a, rim));
}
