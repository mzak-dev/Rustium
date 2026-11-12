// Material evaluation shared by the raster pass and the path tracer. Concatenated before every scene shader.
//
// `evaluate_material` is the ONLY place that reads material data. Everything below the marker line is generated
// from the Scene's Material node graphs by material_compiler.rs (one function per Material and a `switch` over the
// Color's Material inside `evaluate_material`); the code above it is the fixed helper library the generated code
// calls. Callers keep using the same signature.
//
// Units: `position` is in the Scene's voxel space, so one unit is one voxel. `local_position` and `local_normal` are
// the same point and face normal in the space of the Model's Shape (before the Model's Transform): the Space option
// "Model" of Noise, Cells and Gradient reads them, so a pattern moves with its Model.
//   fn evaluate_material(color_index, position, normal, local_position, local_normal) -> Surface

// One Color of the Palette: linear base color and the index of its Material.
struct ColorEntry {
    albedo: vec3<f32>,
    // Bits 0..16: index of the Color's Material. Bit 16 (COLOR_NO_SHADOW): the Material does not cast shadows.
    // Bit 17 (COLOR_VOLUMETRIC): the Material has a Volume output, so shadow rays are dimmed by its haze instead of
    // stopped. Bit 18 (COLOR_TRANSMISSIVE): see-through glass without a Volume output; shadow rays pass it, dimmed and
    // tinted. Bit 19 (COLOR_OUTER_EDGES): a Voxel bevel rounds only the outer edges.
    graph: u32,
};

const COLOR_GRAPH_MASK: u32 = 0xffffu;
const COLOR_NO_SHADOW: u32 = 0x10000u;
const COLOR_VOLUMETRIC: u32 = 0x20000u;
const COLOR_TRANSMISSIVE: u32 = 0x40000u;
// The Material has a Voxel bevel on its outer edges only: the path tracer looks up which edges those are.
const COLOR_OUTER_EDGES: u32 = 0x80000u;

struct MaterialTable {
    colors: array<ColorEntry, 256>,
};

@group(0) @binding(1) var<uniform> material_table: MaterialTable;
// The constants of every node of every Material, flattened by the compiler (a node's slots are literals in the
// generated code, so editing a number only rewrites this buffer; editing the graph's shape recompiles).
@group(0) @binding(9) var<storage, read> material_params: array<f32>;

// The Material index of a Color.
fn color_graph(color_index: u32) -> u32 {
    return material_table.colors[color_index].graph & COLOR_GRAPH_MASK;
}

// The shadow and haze flags of a Color's Material (COLOR_NO_SHADOW, COLOR_VOLUMETRIC).
fn color_flags(color_index: u32) -> u32 {
    return material_table.colors[color_index].graph & ~COLOR_GRAPH_MASK;
}

// The Surface and Volume outputs of a Material.
struct Surface {
    base_color: vec3<f32>,
    normal: vec3<f32>,
    metallic: f32,
    roughness: f32,
    emission_color: vec3<f32>,
    emission_strength: f32,
    transmission: f32,
    ior: f32,
    // Haze inside the voxel: extinction per voxel and the color it scatters.
    volume_density: f32,
    volume_color: vec3<f32>,
};

// Ambient occlusion at the shaded point, 1 = fully open. The caller sets it before `evaluate_material`: the raster
// pass passes the mesh's per-vertex occlusion; the path tracer leaves it at 1 because its bounce lighting already
// darkens corners (the Ambient occlusion node is therefore flat there).
var<private> material_ao: f32 = 1.0;

// The outer edges of the shaded voxel face, for the Voxel bevel's "Outer edges only": bit 2 * axis is the face's low
// side along Scene axis `axis`, bit 2 * axis + 1 its high side. The caller sets it before `evaluate_material` (the
// raster pass from the mesh, the path tracer from the neighbouring voxels); the default rounds every edge.
var<private> material_edges: u32 = 0x3fu;

// The `material_edges` bit of a voxel's side along its Model's axis `axis` (`positive`: the high side), for a Model
// whose axes point along `columns` in the Scene (signed axis permutations).
fn edge_bit(columns: mat3x3<f32>, axis: u32, positive: bool) -> u32 {
    let side = columns[axis] * select(-1.0, 1.0, positive);
    let a = abs(side);
    let scene_axis = select(select(2u, 1u, a.y > a.z), 0u, a.x > a.y && a.x > a.z);
    return 1u << (2u * scene_axis + u32(side[scene_axis] > 0.0));
}

fn mp(i: u32) -> f32 {
    return material_params[i];
}

fn mp2(i: u32) -> vec2<f32> {
    return vec2<f32>(material_params[i], material_params[i + 1u]);
}

fn mp3(i: u32) -> vec3<f32> {
    return vec3<f32>(material_params[i], material_params[i + 1u], material_params[i + 2u]);
}

fn luma(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

fn average3(v: vec3<f32>) -> f32 {
    return (v.x + v.y + v.z) / 3.0;
}

fn safe_normal(n: vec3<f32>, fallback: vec3<f32>) -> vec3<f32> {
    let l = length(n);
    if (l < 1.0e-4) {
        return fallback;
    }
    return n / l;
}

// ---------------------------------------------------------------------------------------------------------------
// Hashing and noise.

fn hash_u32(value: u32) -> u32 {
    let state = value * 747796405u + 2891336453u;
    let word = ((state >> ((state >> 28u) + 4u)) ^ state) * 277803737u;
    return (word >> 22u) ^ word;
}

fn hash_cell(cell: vec3<i32>, seed: u32) -> u32 {
    var h = hash_u32(bitcast<u32>(cell.x) + seed * 2654435769u);
    h = hash_u32(h + bitcast<u32>(cell.y));
    return hash_u32(h + bitcast<u32>(cell.z));
}

// A hash in [0, 1).
fn hash_float(cell: vec3<i32>, seed: u32) -> f32 {
    return f32(hash_cell(cell, seed) >> 8u) / 16777216.0;
}

// Smooth value noise in [0, 1]: trilinear blend of random lattice values.
fn value_noise(p: vec3<f32>, seed: u32) -> f32 {
    let i = floor(p);
    let f = p - i;
    let u = f * f * (3.0 - 2.0 * f);
    let c = vec3<i32>(i);
    let n000 = hash_float(c, seed);
    let n100 = hash_float(c + vec3<i32>(1, 0, 0), seed);
    let n010 = hash_float(c + vec3<i32>(0, 1, 0), seed);
    let n110 = hash_float(c + vec3<i32>(1, 1, 0), seed);
    let n001 = hash_float(c + vec3<i32>(0, 0, 1), seed);
    let n101 = hash_float(c + vec3<i32>(1, 0, 1), seed);
    let n011 = hash_float(c + vec3<i32>(0, 1, 1), seed);
    let n111 = hash_float(c + vec3<i32>(1, 1, 1), seed);
    return mix(
        mix(mix(n000, n100, u.x), mix(n010, n110, u.x), u.y),
        mix(mix(n001, n101, u.x), mix(n011, n111, u.x), u.y),
        u.z,
    );
}

// Fractal noise in [0, 1]: `detail` extra octaves (0..=6) on top of the base one.
fn fbm(p: vec3<f32>, detail: i32, seed: u32) -> f32 {
    let count = clamp(detail, 0, 6);
    var sum = 0.0;
    var amplitude = 0.5;
    var total = 0.0;
    var q = p;
    for (var i = 0; i <= count; i += 1) {
        sum += amplitude * value_noise(q, seed + u32(i));
        total += amplitude;
        q = q * 2.0 + vec3<f32>(17.0);
        amplitude *= 0.5;
    }
    return sum / total;
}

// The centre of the voxel a surface point belongs to (the normal nudges the point off the voxel's own boundary).
fn voxel_snap(p: vec3<f32>, n: vec3<f32>) -> vec3<f32> {
    return floor(p - n * 0.01) + vec3<f32>(0.5);
}

// Noise node. One scale unit is one noise cell per 8 voxels; `per_voxel` gives every voxel one flat value.
fn noise_pattern(p: vec3<f32>, n: vec3<f32>, scale: f32, detail: i32, seed: u32, per_voxel: bool) -> f32 {
    var q = p;
    if (per_voxel) {
        q = voxel_snap(p, n);
    }
    return fbm(q * (scale / 8.0), detail, seed);
}

// Cells node. x: distance to the nearest point (0..1), y: random value of that cell, z: distance to the border
// between the two nearest cells (0 on the border, 1 well inside).
fn cells_pattern(p: vec3<f32>, scale: f32, randomness: f32, seed: u32) -> vec3<f32> {
    let q = p * (scale / 8.0);
    let base = vec3<i32>(floor(q));
    var f1 = 8.0;
    var f2 = 8.0;
    var id = 0.0;
    for (var z = -1; z <= 1; z += 1) {
        for (var y = -1; y <= 1; y += 1) {
            for (var x = -1; x <= 1; x += 1) {
                let cell = base + vec3<i32>(x, y, z);
                let h = hash_cell(cell, seed);
                let jitter = vec3<f32>(f32(h & 255u), f32((h >> 8u) & 255u), f32((h >> 16u) & 255u)) / 255.0;
                let point = vec3<f32>(cell) + vec3<f32>(0.5) + (jitter - vec3<f32>(0.5)) * randomness;
                let d = distance(q, point);
                if (d < f1) {
                    f2 = f1;
                    f1 = d;
                    id = f32(h >> 24u) / 255.0;
                } else if (d < f2) {
                    f2 = d;
                }
            }
        }
    }
    return vec3<f32>(min(f1, 1.0), id, clamp((f2 - f1) * 4.0, 0.0, 1.0));
}

// Gradient node: 0 at `range.x`, 1 at `range.y` along an axis (0 = X, 1 = Y, 2 = Z).
fn gradient_pattern(p: vec3<f32>, axis: i32, range: vec2<f32>) -> f32 {
    var c = p.x;
    if (axis == 1) {
        c = p.y;
    } else if (axis == 2) {
        c = p.z;
    }
    let span = range.y - range.x;
    if (abs(span) < 1.0e-5) {
        return select(0.0, 1.0, c >= range.x);
    }
    return clamp((c - range.x) / span, 0.0, 1.0);
}

// ---------------------------------------------------------------------------------------------------------------
// Voxel edges. Both renderers shade flat voxel faces, so the distance to the voxel's edges follows from the
// position and the face normal alone (exact for axis-aligned faces, which is all voxels have).

// Distance in voxels from the shaded point to the nearest edge of its face, 0 on the edge and 0.5 in the middle.
fn edge_distance(p: vec3<f32>, n: vec3<f32>) -> f32 {
    let cell = floor(p - n * 0.01);
    let local = p - cell;
    let edge = min(local, vec3<f32>(1.0) - local);
    let a = abs(n);
    let ex = select(edge.x, 1.0, a.x > 0.5);
    let ey = select(edge.y, 1.0, a.y > 0.5);
    let ez = select(edge.z, 1.0, a.z > 0.5);
    return min(ex, min(ey, ez));
}

// Edge wear node: 1 on voxel edges, fading to 0 over `width` voxels; `breakup` varies the width per voxel.
fn edge_wear(p: vec3<f32>, n: vec3<f32>, width: f32, breakup: f32, seed: u32) -> f32 {
    let d = edge_distance(p, n);
    let r = hash_float(vec3<i32>(floor(p - n * 0.01)), seed);
    let local_width = width * mix(1.0, r * 2.0, clamp(breakup, 0.0, 1.0));
    return 1.0 - smoothstep(0.0, max(local_width, 1.0e-4), d);
}

// Tilt contributed by one tangent axis: leans outwards near either end, in `segments` steps. `open` bit 0 rounds the
// low end, bit 1 the high end.
fn bevel_axis(local: f32, radius: f32, segments: f32, open: u32) -> f32 {
    var tilt = 0.0;
    if ((open & 1u) != 0u && local < radius) {
        tilt -= ceil((1.0 - local / radius) * segments) / segments;
    }
    if ((open & 2u) != 0u && 1.0 - local < radius) {
        tilt += ceil((1.0 - (1.0 - local) / radius) * segments) / segments;
    }
    return tilt;
}

// Voxel bevel node: shading-only normal that leans over voxel edges as if they were rounded off; with `outer` only
// over the edges in `material_edges`.
fn bevel_normal(p: vec3<f32>, n: vec3<f32>, radius: f32, segments: i32, outer: f32) -> vec3<f32> {
    if (radius <= 0.0) {
        return n;
    }
    let local = p - floor(p - n * 0.01);
    let a = abs(n);
    let s = f32(max(segments, 1));
    let edges = select(0x3fu, material_edges, outer > 0.5);
    let tilt = vec3<f32>(
        select(bevel_axis(local.x, radius, s, edges & 3u), 0.0, a.x > 0.5),
        select(bevel_axis(local.y, radius, s, (edges >> 2u) & 3u), 0.0, a.y > 0.5),
        select(bevel_axis(local.z, radius, s, (edges >> 4u) & 3u), 0.0, a.z > 0.5),
    );
    return normalize(n + tilt);
}

// ---------------------------------------------------------------------------------------------------------------
// Color operations. Colors inside a graph are linear.

fn rgb_to_hsv(c: vec3<f32>) -> vec3<f32> {
    let k = vec4<f32>(0.0, -1.0 / 3.0, 2.0 / 3.0, -1.0);
    let p = mix(vec4<f32>(c.bg, k.wz), vec4<f32>(c.gb, k.xy), step(c.b, c.g));
    let q = mix(vec4<f32>(p.xyw, c.r), vec4<f32>(c.r, p.yzx), step(p.x, c.r));
    let d = q.x - min(q.w, q.y);
    let e = 1.0e-10;
    return vec3<f32>(abs(q.z + (q.w - q.y) / (6.0 * d + e)), d / (q.x + e), q.x);
}

fn hsv_to_rgb(c: vec3<f32>) -> vec3<f32> {
    let k = vec4<f32>(1.0, 2.0 / 3.0, 1.0 / 3.0, 3.0);
    let p = abs(fract(vec3<f32>(c.x) + k.xyz) * 6.0 - k.www);
    return c.z * mix(k.xxx, clamp(p - k.xxx, vec3<f32>(0.0), vec3<f32>(1.0)), c.y);
}

// Hue shift node: rotates the hue by `degrees` and scales saturation and value.
fn hue_shift(c: vec3<f32>, degrees: f32, saturation: f32, value: f32) -> vec3<f32> {
    var hsv = rgb_to_hsv(max(c, vec3<f32>(0.0)));
    hsv.x = fract(hsv.x + degrees / 360.0);
    hsv.y = clamp(hsv.y * saturation, 0.0, 1.0);
    hsv.z = max(hsv.z * value, 0.0);
    return hsv_to_rgb(hsv);
}

// Mix node: blend modes 0 = Mix, 1 = Multiply, 2 = Add, 3 = Screen.
fn mix_colors(a: vec3<f32>, b: vec3<f32>, factor: f32, mode: i32) -> vec3<f32> {
    var blended = b;
    if (mode == 1) {
        blended = a * b;
    } else if (mode == 2) {
        blended = a + b;
    } else if (mode == 3) {
        blended = vec3<f32>(1.0) - (vec3<f32>(1.0) - a) * (vec3<f32>(1.0) - b);
    }
    return mix(a, blended, clamp(factor, 0.0, 1.0));
}

// Color ramp node. `base` is where the stops start in `material_params`, four floats each (position, linear rgb).
fn ramp_color(base: u32, count: u32, t: f32) -> vec3<f32> {
    if (t <= mp(base)) {
        return mp3(base + 1u);
    }
    for (var i = 1u; i < count; i += 1u) {
        let at = base + i * 4u;
        let end_t = mp(at);
        if (t <= end_t) {
            let start_t = mp(at - 4u);
            let f = (t - start_t) / max(end_t - start_t, 1.0e-6);
            return mix(mp3(at - 3u), mp3(at + 1u), f);
        }
    }
    return mp3(base + (count - 1u) * 4u + 1u);
}

// Remap node.
fn remap_value(v: f32, source: vec2<f32>, target_range: vec2<f32>, clamp_result: bool) -> f32 {
    let span = source.y - source.x;
    var t = 0.0;
    if (abs(span) > 1.0e-6) {
        t = (v - source.x) / span;
    }
    if (clamp_result) {
        t = clamp(t, 0.0, 1.0);
    }
    return mix(target_range.x, target_range.y, t);
}

// Clamp node (tolerates Min above Max).
fn clamp_value(v: f32, a: f32, b: f32) -> f32 {
    return clamp(v, min(a, b), max(a, b));
}

// @@GENERATED@@
