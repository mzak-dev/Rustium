@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var linear_sampler: sampler;

struct BlurParams {
    // xy: one texel step in UV along the blur direction.
    step: vec4<f32>,
};
@group(0) @binding(2) var<uniform> blur: BlurParams;

@fragment
fn fs_downsample(in: FullscreenOutput) -> @location(0) vec4<f32> {
    return textureSample(source, linear_sampler, in.uv);
}

@fragment
fn fs_blur(in: FullscreenOutput) -> @location(0) vec4<f32> {
    let weights = array<f32, 5>(0.2270270, 0.1945946, 0.1216216, 0.0540541, 0.0162162);
    var color = textureSample(source, linear_sampler, in.uv) * weights[0];
    for (var i = 1; i < 5; i += 1) {
        let offset = blur.step.xy * f32(i) * 1.5;
        color += textureSample(source, linear_sampler, in.uv + offset) * weights[i];
        color += textureSample(source, linear_sampler, in.uv - offset) * weights[i];
    }
    return color;
}
