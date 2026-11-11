// Infinite-looking ground grid on y = 0, faded towards its edge.
struct GridOutput {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> GridOutput {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0),
    );
    let point = camera.grid.zw + corners[index] * camera.grid.y;
    var out: GridOutput;
    out.clip = camera.view_proj * vec4<f32>(point.x, 0.0, point.y, 1.0);
    out.world = point;
    return out;
}

fn line_strength(coord: vec2<f32>) -> f32 {
    let width = fwidth(coord);
    let distance = abs(fract(coord - 0.5) - 0.5) / max(width, vec2<f32>(1e-4));
    let line = 1.0 - min(min(distance.x, distance.y), 1.0);
    // Fade lines out where they get denser than a few pixels apart, which would otherwise shimmer.
    let density = max(width.x, width.y);
    return line * (1.0 - smoothstep(0.15, 0.45, density));
}

@fragment
fn fs_main(in: GridOutput) -> @location(0) vec4<f32> {
    let minor = line_strength(in.world);
    let major = line_strength(in.world / camera.grid.x);
    let fade = 1.0 - smoothstep(camera.grid.y * 0.35, camera.grid.y, distance(in.world, camera.grid.zw));
    var color = vec3<f32>(1.0);
    var alpha = max(minor * 0.05, major * 0.13);
    let axis_width = fwidth(in.world) * 1.5;
    if (abs(in.world.y) < axis_width.y) {
        color = vec3<f32>(0.93, 0.36, 0.30);
        alpha = 0.5;
    } else if (abs(in.world.x) < axis_width.x) {
        color = vec3<f32>(0.33, 0.55, 0.95);
        alpha = 0.5;
    }
    return vec4<f32>(color, alpha * fade);
}
