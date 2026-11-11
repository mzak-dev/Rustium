struct Glass {
    // x: number of rects, y: 1 when blur is on.
    header: vec4<u32>,
    // Per rect: min.xy, max.xy in physical pixels.
    rects: array<vec4<f32>, 32>,
    // Per rect: x = corner radius in physical pixels.
    radii: array<vec4<f32>, 32>,
};

@group(0) @binding(0) var scene: texture_2d<f32>;
@group(0) @binding(1) var blurred: texture_2d<f32>;
@group(0) @binding(2) var composite_sampler: sampler;
@group(0) @binding(3) var<uniform> glass: Glass;

fn rounded_box(point: vec2<f32>, rect: vec4<f32>, radius: f32) -> f32 {
    let center = (rect.xy + rect.zw) * 0.5;
    let half_size = (rect.zw - rect.xy) * 0.5;
    let q = abs(point - center) - half_size + vec2<f32>(radius);
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - radius;
}

@fragment
fn fs_composite(in: FullscreenOutput) -> @location(0) vec4<f32> {
    let color = textureSample(scene, composite_sampler, in.uv);
    if (glass.header.y == 0u) {
        return color;
    }
    var coverage = 0.0;
    for (var i = 0u; i < min(glass.header.x, 32u); i += 1u) {
        let distance = rounded_box(in.clip.xy, glass.rects[i], glass.radii[i].x);
        coverage = max(coverage, clamp(0.5 - distance, 0.0, 1.0));
    }
    if (coverage <= 0.0) {
        return color;
    }
    let soft = textureSample(blurred, composite_sampler, in.uv).rgb;
    let grey = dot(soft, vec3<f32>(0.2126, 0.7152, 0.0722));
    let saturated = mix(vec3<f32>(grey), soft, 1.2);
    return vec4<f32>(mix(color.rgb, saturated, coverage), 1.0);
}
