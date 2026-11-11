// Material evaluation shared by the raster pass and the path tracer. Concatenated before every scene shader.
//
// `evaluate_material` is the ONLY place that reads material data. The Effects milestone replaces its body with
// code generated from the Material node graphs; callers keep using the same signature.

// One Color of the Palette: linear base color and the index of its material graph.
struct ColorEntry {
    albedo: vec3<f32>,
    graph: u32,
};

// The constant Surface parameters of one material graph (matches rustium_render::GpuSurface).
struct SurfaceRecord {
    // rgb: linear emission color, a: emission strength (0 = does not glow).
    emission: vec4<f32>,
    // x: roughness, y: metallic, z: transmission, w: index of refraction.
    params: vec4<f32>,
};

struct MaterialTable {
    colors: array<ColorEntry, 256>,
    surfaces: array<SurfaceRecord, 64>,
};

@group(0) @binding(1) var<uniform> material_table: MaterialTable;

// The Surface output of a material graph.
struct Surface {
    base_color: vec3<f32>,
    normal: vec3<f32>,
    metallic: f32,
    roughness: f32,
    emission_color: vec3<f32>,
    emission_strength: f32,
    transmission: f32,
    ior: f32,
};

// Shades the Color `color_index` (a voxel's byte, 1..=255) at `position` with geometric `normal`.
fn evaluate_material(color_index: u32, position: vec3<f32>, normal: vec3<f32>) -> Surface {
    let color = material_table.colors[color_index];
    let record = material_table.surfaces[color.graph];
    var surface: Surface;
    surface.base_color = color.albedo;
    surface.normal = normal;
    surface.roughness = record.params.x;
    surface.metallic = record.params.y;
    surface.transmission = record.params.z;
    surface.ior = record.params.w;
    surface.emission_color = record.emission.rgb;
    surface.emission_strength = record.emission.a;
    return surface;
}
