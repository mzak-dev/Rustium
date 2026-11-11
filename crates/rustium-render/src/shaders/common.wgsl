// Shared camera uniform; every scene shader binds it at group 0, binding 0.
struct Camera {
    view_proj: mat4x4<f32>,
    // xyz: camera position in voxel space.
    eye: vec4<f32>,
    // xyz: unit direction towards the sun.
    sun: vec4<f32>,
    // x: major grid spacing in voxels, y: grid half-extent, zw: grid centre (x, z).
    grid: vec4<f32>,
};

@group(0) @binding(0) var<uniform> camera: Camera;
