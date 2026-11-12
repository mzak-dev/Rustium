// Temporal anti-aliasing resolve for the raster viewport (ADR 0006). Concatenated after fullscreen.wgsl, with
// DEPTH_TEXTURE replaced by `texture_depth_2d` or `texture_depth_multisampled_2d` (the scene pass may use MSAA).
//
// The scene pass draws glass and haze as stochastic dithering with a different pattern every frame and a sub-pixel
// jitter. This pass blends each pixel with its history, reprojected through the depth buffer to where the surface was
// last frame, and clamps the history to the colors in the 5x5 pixels around it in the new frame, so what moved or changed
// cannot ghost.

struct TaaParams {
    // Where a point of the current frame was on screen last frame: previous view-projection (without jitter).
    prev_view_proj: mat4x4<f32>,
    // Clip space of the current frame (without jitter) back to voxel space.
    inv_view_proj: mat4x4<f32>,
    // x: weight of the new frame, y: 1 = ignore the history, zw: image size in pixels.
    settings: vec4<f32>,
};

@group(0) @binding(0) var<uniform> taa: TaaParams;
@group(0) @binding(1) var raw: texture_2d<f32>;
@group(0) @binding(2) var depth: DEPTH_TEXTURE;
@group(0) @binding(3) var history: texture_2d<f32>;
@group(0) @binding(4) var history_sampler: sampler;

struct TaaOutput {
    // The pixel shown this frame.
    @location(0) color: vec4<f32>,
    // The same pixel, kept in higher precision for the next frame.
    @location(1) history: vec4<f32>,
};

@fragment
fn fs_taa(in: FullscreenOutput) -> TaaOutput {
    let size = taa.settings.zw;
    let pixel = vec2<i32>(in.clip.xy);
    let last = vec2<i32>(size) - vec2<i32>(1);
    let current = textureLoad(raw, pixel, 0).rgb;

    var low = current;
    var high = current;
    for (var y = -2; y <= 2; y += 1) {
        for (var x = -2; x <= 2; x += 1) {
            let sample = textureLoad(raw, clamp(pixel + vec2<i32>(x, y), vec2<i32>(0), last), 0).rgb;
            low = min(low, sample);
            high = max(high, sample);
        }
    }

    var result = current;
    if (taa.settings.y < 0.5) {
        let z = textureLoad(depth, pixel, 0);
        let ndc = vec2<f32>(in.clip.x / size.x * 2.0 - 1.0, 1.0 - in.clip.y / size.y * 2.0);
        let world4 = taa.inv_view_proj * vec4<f32>(ndc, z, 1.0);
        let previous = taa.prev_view_proj * vec4<f32>(world4.xyz / world4.w, 1.0);
        if (previous.w > 1.0e-4) {
            let previous_ndc = previous.xy / previous.w;
            let uv = vec2<f32>(previous_ndc.x * 0.5 + 0.5, 0.5 - previous_ndc.y * 0.5);
            if (all(uv >= vec2<f32>(0.0)) && all(uv <= vec2<f32>(1.0))) {
                let old = textureSampleLevel(history, history_sampler, uv, 0.0).rgb;
                result = mix(clamp(old, low, high), current, taa.settings.x);
            }
        }
    }
    var out: TaaOutput;
    out.color = vec4<f32>(result, 1.0);
    out.history = vec4<f32>(result, 1.0);
    return out;
}
