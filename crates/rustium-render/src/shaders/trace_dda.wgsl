// Software traversal variant: every visible Model is walked with the two-level DDA of `trace_voxels`.
// Concatenated after trace_style.wgsl; the ray-query variant is trace_rq.wgsl.

// The nearest voxel hit over the visible Models (the ground is added by `trace`). A ray inside glass only looks at the
// Model it is in.
// ponytail: loops over all Models per ray; add a BVH over Model bounds when scenes hold hundreds of Models.
fn trace_scene_hit(origin: vec3<f32>, dir: vec3<f32>, inside: u32, inside_model: u32) -> Hit {
    if (inside != 0u) {
        return trace_model(inside_model, origin, dir, MODE_INSIDE, inside);
    }
    var result = miss();
    for (var index = 0u; index < params.options.w; index += 1u) {
        let candidate = trace_model(index, origin, dir, MODE_NEAREST, 0u);
        if (candidate.hit && candidate.t < result.t) {
            result = candidate;
        }
    }
    return result;
}

// The fraction of sunlight that gets along the ray: 1.0 in the open, 0.0 when a voxel that casts shadows is in the
// way, in between when it crosses haze (see MODE_SHADOW).
fn shadow_transmittance(origin: vec3<f32>, dir: vec3<f32>) -> f32 {
    var optical = 0.0;
    for (var index = 0u; index < params.options.w; index += 1u) {
        let hit = trace_model(index, origin, dir, MODE_SHADOW, 0u);
        if (hit.hit) {
            return 0.0;
        }
        optical += hit.optical;
    }
    return exp(-optical);
}
