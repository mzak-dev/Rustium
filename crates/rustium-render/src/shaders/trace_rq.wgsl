// Hardware traversal variant (wgpu ray query). Each Shape is a BLAS of one AABB per occupied chunk, each visible Model
// a TLAS instance of its Shape's BLAS with the Model's Transform. The hardware finds the chunk boxes along the ray;
// inside a box the same voxel DDA as the software variant (`trace_voxels`) finds the voxel, so hits are identical.
// Needs `enable wgpu_ray_query;` at the top of the module. Concatenated after trace_style.wgsl.

// Instances are listed in the order of `models`, so instance_index is the Model index and
// instance_custom_data is where the Shape's boxes start in `chunk_boxes`.
@group(0) @binding(10) var scene_tlas: acceleration_structure;
// xyz: chunk coordinate of box `custom_data + primitive_index`.
@group(0) @binding(11) var<storage, read> chunk_boxes: array<vec4<i32>>;

// The Model's record narrowed to one chunk, so `trace_voxels` walks just that chunk: a grid of a single cell whose
// entry sits where the chunk's cell sits in the Shape's chunk grid.
fn chunk_record(model: ModelRecord, chunk: vec3<i32>) -> ModelRecord {
    let rel = vec3<u32>(chunk - model.grid_min.xyz);
    let size = model.grid_size.xyz;
    var narrowed = model;
    narrowed.grid_min = vec4<i32>(chunk, model.grid_min.w + i32(rel.x + size.x * (rel.z + size.z * rel.y)));
    narrowed.grid_size = vec4<u32>(1u, 1u, 1u, 0u);
    return narrowed;
}

// HOT PATH: the voxel hit inside chunk box `box_index` of Model `index`. (Ray queries cannot be passed to functions,
// so the callers read the candidate themselves.)
fn trace_chunk(index: u32, box_index: u32, origin: vec3<f32>, dir: vec3<f32>, mode: u32) -> Hit {
    let chunk = chunk_boxes[box_index].xyz;
    return trace_record(chunk_record(models[index], chunk), index, origin, dir, mode, 0u);
}

// The nearest voxel hit over the visible Models. A ray inside glass stays with the Model it is in and uses the DDA.
fn trace_scene_hit(origin: vec3<f32>, dir: vec3<f32>, inside: u32, inside_model: u32) -> Hit {
    if (inside != 0u) {
        return trace_model(inside_model, origin, dir, MODE_INSIDE, inside);
    }
    var result = miss();
    var rq: ray_query;
    rayQueryInitialize(&rq, scene_tlas, RayDesc(0u, 0xFFu, 0.0, FAR, origin, dir));
    loop {
        if (!rayQueryProceed(&rq)) {
            break;
        }
        let box = rayQueryGetCandidateIntersection(&rq);
        if (box.kind != RAY_QUERY_INTERSECTION_AABB) {
            continue;
        }
        let candidate = trace_chunk(box.instance_index, box.instance_custom_data + box.primitive_index, origin, dir, MODE_NEAREST);
        // A hit past the nearest one found so far is skipped; where Models overlap exactly, the earlier one wins.
        if (candidate.hit && candidate.t < result.t) {
            result = candidate;
            let hit_t = result.t;
            rayQueryGenerateIntersection(&rq, hit_t);
        }
    }
    return result;
}

// The fraction of sunlight that gets along the ray (see the DDA variant). Every chunk box on the ray is walked in
// MODE_SHADOW: a blocking voxel ends the query, haze adds its optical depth chunk by chunk.
fn shadow_transmittance(origin: vec3<f32>, dir: vec3<f32>) -> f32 {
    var optical = 0.0;
    var rq: ray_query;
    rayQueryInitialize(&rq, scene_tlas, RayDesc(0u, 0xFFu, 0.0, FAR, origin, dir));
    loop {
        if (!rayQueryProceed(&rq)) {
            break;
        }
        let box = rayQueryGetCandidateIntersection(&rq);
        if (box.kind != RAY_QUERY_INTERSECTION_AABB) {
            continue;
        }
        let found = trace_chunk(box.instance_index, box.instance_custom_data + box.primitive_index, origin, dir, MODE_SHADOW);
        if (found.hit) {
            return 0.0;
        }
        optical += found.optical;
    }
    return exp(-optical);
}
