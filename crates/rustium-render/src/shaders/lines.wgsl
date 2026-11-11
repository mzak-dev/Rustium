// Overlay lines: Volume bounds and the hover box.
struct LineOutput {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_main(@location(0) position: vec3<f32>, @location(1) color: vec4<f32>) -> LineOutput {
    var out: LineOutput;
    out.clip = camera.view_proj * vec4<f32>(position, 1.0);
    out.color = color;
    return out;
}

@fragment
fn fs_main(in: LineOutput) -> @location(0) vec4<f32> {
    return in.color;
}
