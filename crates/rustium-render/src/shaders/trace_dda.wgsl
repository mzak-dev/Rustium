// Software traversal variant: every visible Model is walked with the two-level DDA of `trace_voxels`.
// Concatenated after trace_style.wgsl; the ray-query variant is trace_rq.wgsl.

// The nearest voxel hit over the visible Models (the ground is added by `trace`). A ray inside glass only looks at the
// Model it is in.
// ponytail: loops over all Models per ray; add a BVH over Model bounds when scenes hold hundreds of Models.
fn trace_scene_hit(origin: vec3<f32>, dir: vec3<f32>, inside: u32, inside_model: u32) -> Hit {
    if (inside != 0u) {
        return trace_model(inside_model, origin, dir, inside);
    }
    var result = miss();
    for (var index = 0u; index < params.options.w; index += 1u) {
        let candidate = trace_model(index, origin, dir, 0u);
        if (candidate.hit && candidate.t < result.t) {
            result = candidate;
        }
    }
    return result;
}

// Whether anything blocks the ray (shadow rays).
fn occluded(origin: vec3<f32>, dir: vec3<f32>) -> bool {
    for (var index = 0u; index < params.options.w; index += 1u) {
        if (trace_model(index, origin, dir, 0u).hit) {
            return true;
        }
    }
    return false;
}
