// Tonemaps the accumulated path-traced image into the viewport, fitted into a frame rect with a letterbox around it.
// Concatenated after fullscreen.wgsl.
struct Present {
    // xy: frame origin in target pixels, zw: frame size in target pixels.
    frame: vec4<f32>,
    // x: image width, y: image height, z: exposure, w: 1 when the background is transparent.
    image: vec4<f32>,
    // rgb: linear color around the frame.
    letterbox: vec4<f32>,
    // x: 1 when the image is display-ready and skips exposure and tonemapping.
    options: vec4<f32>,
};

@group(0) @binding(0) var<uniform> present: Present;
@group(0) @binding(1) var accumulated: texture_2d<f32>;

// Narkowicz's fit of the ACES filmic curve.
fn aces(color: vec3<f32>) -> vec3<f32> {
    return clamp((color * (2.51 * color + 0.03)) / (color * (2.43 * color + 0.59) + 0.14), vec3<f32>(0.0), vec3<f32>(1.0));
}

fn load_clamped(pixel: vec2<i32>) -> vec4<f32> {
    let last = vec2<i32>(present.image.xy) - vec2<i32>(1);
    return textureLoad(accumulated, clamp(pixel, vec2<i32>(0), last), 0);
}

@fragment
fn fs_present(in: FullscreenOutput) -> @location(0) vec4<f32> {
    let relative = (in.clip.xy - present.frame.xy) / present.frame.zw;
    if (any(relative < vec2<f32>(0.0)) || any(relative >= vec2<f32>(1.0))) {
        return vec4<f32>(present.letterbox.rgb, 1.0);
    }
    // Bilinear fetch so a downscaled preview does not shimmer.
    let position = relative * present.image.xy - vec2<f32>(0.5);
    let base = floor(position);
    let blend = position - base;
    let corner = vec2<i32>(base);
    let top = mix(load_clamped(corner), load_clamped(corner + vec2<i32>(1, 0)), blend.x);
    let bottom = mix(load_clamped(corner + vec2<i32>(0, 1)), load_clamped(corner + vec2<i32>(1, 1)), blend.x);
    let texel = mix(top, bottom, blend.y);
    var color = select(aces(texel.rgb * present.image.z), texel.rgb, present.options.x > 0.5);
    if (present.image.w > 0.5) {
        // Premultiplied radiance over a checkerboard.
        let cell = vec2<i32>(floor(in.clip.xy / 12.0));
        let checker = select(0.10, 0.16, (cell.x + cell.y) % 2 == 0);
        color = color + vec3<f32>(checker) * (1.0 - texel.a);
    }
    return vec4<f32>(color, 1.0);
}
