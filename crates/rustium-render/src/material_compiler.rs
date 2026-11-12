//! Turns the Scene's Material graphs into WGSL.
//!
//! [`compile_materials`] returns the generated part of the material shader (one function per Material plus the
//! `switch` inside `evaluate_material`) and the flat list of numbers those functions read from the
//! `material_params` storage buffer. The source depends only on the *shape* of the graphs (which nodes, links,
//! bypasses, which socket holds a constant and which holds Position/Normal/Voxel color, how many ramp stops); every
//! number is a buffer slot. So editing a number rewrites the buffer and keeps the pipelines, while editing the shape
//! produces different source and a pipeline rebuild. The compiler is pure, which makes it easy to test.

use std::fmt::Write as _;

use rustium_core::{
    GraphNode, GraphNodeId, GraphNodeKind, InputRef, InputValue, MaterialGraph, ParamValue, SocketType,
};

/// Most `f32` slots the parameter buffer holds (a few hundred KiB; 64 graphs of 128 nodes fit with room to spare).
pub const PARAM_CAPACITY: usize = 64 * 128 * 40;

/// The marker in `material.wgsl` that the generated code replaces.
const MARKER: &str = "// @@GENERATED@@";

/// Why the graphs could not be compiled.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CompileError {
    /// A graph is malformed (only possible for data that did not come through the edit API).
    #[error("material \"{name}\" is invalid: {reason}")]
    InvalidGraph {
        /// The Material's name.
        name: String,
        /// What is wrong.
        reason: String,
    },
    /// The generated shader was rejected by the shader compiler or the driver.
    #[error("the material shader is invalid: {0}")]
    Shader(String),
    /// The graphs hold more constants than the parameter buffer can store.
    #[error("the materials hold too many values ({0} > {PARAM_CAPACITY})")]
    TooManyParameters(usize),
}

/// The output of [`compile_materials`].
#[derive(Debug, Clone, PartialEq)]
pub struct CompiledMaterials {
    /// Generated WGSL: replaces the marker line at the end of `material.wgsl`.
    pub generated: String,
    /// Contents of the `material_params` storage buffer.
    pub params: Vec<f32>,
}

impl CompiledMaterials {
    /// The complete material shader module: the fixed helper library followed by the generated code.
    pub fn shader_source(&self) -> String {
        assemble(&self.generated)
    }
}

/// Puts `generated` in place of the marker at the end of the helper library.
pub(crate) fn assemble(generated: &str) -> String {
    include_str!("shaders/material.wgsl").replace(MARKER, generated)
}

/// The Default material alone: what the pipelines start with. It always compiles.
pub(crate) fn default_compiled() -> CompiledMaterials {
    compile_materials(&[MaterialGraph::default_material()]).unwrap_or_else(|_| CompiledMaterials {
        generated: EMPTY_MATERIAL.to_owned(),
        params: vec![0.0],
    })
}

/// Compiles every Material of a Scene (index = the Color's `graph`).
///
/// # Errors
///
/// [`CompileError`] when a graph fails validation or the constants do not fit the buffer.
pub fn compile_materials(graphs: &[MaterialGraph]) -> Result<CompiledMaterials, CompileError> {
    let mut emitter = Emitter { params: Vec::new() };
    let mut generated = String::new();
    for (index, graph) in graphs.iter().enumerate() {
        graph.validate().map_err(|error| CompileError::InvalidGraph {
            name: graph.name.clone(),
            reason: error.to_string(),
        })?;
        generated.push_str(&emitter.graph_function(index, graph));
    }
    if emitter.params.len() > PARAM_CAPACITY {
        return Err(CompileError::TooManyParameters(emitter.params.len()));
    }
    // A buffer binding cannot be empty.
    if emitter.params.is_empty() {
        emitter.params.push(0.0);
    }
    generated.push_str(
        "fn evaluate_material(color_index: u32, position: vec3<f32>, normal: vec3<f32>) -> Surface {\n\
         \x20   let color = material_table.colors[color_index];\n\
         \x20   var surface: Surface;\n\
         \x20   switch color.graph {\n",
    );
    for index in 1..graphs.len() {
        let _ = writeln!(
            generated,
            "        case {index}u: {{ surface = material_{index}(color.albedo, position, normal); }}"
        );
    }
    generated.push_str(
        "        default: { surface = material_0(color.albedo, position, normal); }\n\
         \x20   }\n\
         \x20   return surface;\n\
         }\n",
    );
    if graphs.is_empty() {
        // No graphs at all: a constant matte surface so the module stays complete.
        generated.insert_str(0, EMPTY_MATERIAL);
    }
    Ok(CompiledMaterials {
        generated,
        params: emitter.params,
    })
}

const EMPTY_MATERIAL: &str = "fn material_0(albedo: vec3<f32>, position: vec3<f32>, normal: vec3<f32>) -> Surface {\n\
     \x20   var surface: Surface;\n\
     \x20   surface.base_color = albedo;\n\
     \x20   surface.normal = normal;\n\
     \x20   surface.roughness = 0.8;\n\
     \x20   surface.ior = 1.5;\n\
     \x20   return surface;\n\
     }\n";

struct Emitter {
    params: Vec<f32>,
}

fn srgb_to_linear(value: f32) -> f32 {
    let value = value.clamp(0.0, 1.0);
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

fn wgsl_type(ty: SocketType) -> &'static str {
    match ty {
        SocketType::Float => "f32",
        SocketType::Color | SocketType::Vector => "vec3<f32>",
    }
}

fn zero(ty: SocketType) -> &'static str {
    match ty {
        SocketType::Float => "0.0",
        SocketType::Color | SocketType::Vector => "vec3<f32>(0.0)",
    }
}

/// `expr` of type `from` as a value of type `to`: Float widens to grey, Color narrows to luminance, Vector to the
/// average of its components.
fn convert(expr: String, from: SocketType, to: SocketType) -> String {
    match (from, to) {
        (SocketType::Float, SocketType::Color | SocketType::Vector) => format!("vec3<f32>({expr})"),
        (SocketType::Color, SocketType::Float) => format!("luma({expr})"),
        (SocketType::Vector, SocketType::Float) => format!("average3({expr})"),
        _ => expr,
    }
}

impl Emitter {
    /// Appends `values` to the parameter buffer and returns the slot of the first one.
    fn alloc(&mut self, values: &[f32]) -> usize {
        let slot = self.params.len();
        self.params
            .extend(values.iter().map(|value| if value.is_finite() { *value } else { 0.0 }));
        slot
    }

    fn graph_function(&mut self, index: usize, graph: &MaterialGraph) -> String {
        let mut body = String::new();
        for id in graph.evaluation_order() {
            let Some(node) = graph.node(id) else { continue };
            if node.kind.is_output() {
                continue;
            }
            self.node(graph, node, &mut body);
        }
        let mut out = format!(
            "// Material {index}: {}\n\
             fn material_{index}(albedo: vec3<f32>, position: vec3<f32>, normal: vec3<f32>) -> Surface {{\n{body}",
            graph.name.replace(['\n', '\r'], " ")
        );
        out.push_str("    var surface: Surface;\n");
        if let Some(surface) = graph.surface_output() {
            let input = |emitter: &mut Self, socket: u8| emitter.input(graph, surface, socket);
            let base = input(self, 0);
            let metallic = input(self, 1);
            let roughness = input(self, 2);
            let surface_normal = input(self, 3);
            let emission = input(self, 4);
            let strength = input(self, 5);
            let transmission = input(self, 6);
            let ior = input(self, 7);
            let _ = write!(
                out,
                "    surface.base_color = max({base}, vec3<f32>(0.0));\n\
                 \x20   surface.normal = safe_normal({surface_normal}, normal);\n\
                 \x20   surface.metallic = clamp({metallic}, 0.0, 1.0);\n\
                 \x20   surface.roughness = clamp({roughness}, 0.0, 1.0);\n\
                 \x20   surface.emission_color = max({emission}, vec3<f32>(0.0));\n\
                 \x20   surface.emission_strength = clamp({strength}, 0.0, 32.0);\n\
                 \x20   surface.transmission = clamp({transmission}, 0.0, 1.0);\n\
                 \x20   surface.ior = clamp({ior}, 1.0, 3.0);\n"
            );
        }
        if let Some(volume) = graph.volume_output() {
            let density = self.input(graph, volume, 0);
            let scattering = self.input(graph, volume, 1);
            let _ = write!(
                out,
                "    surface.volume_density = max({density}, 0.0);\n\
                 \x20   surface.volume_color = max({scattering}, vec3<f32>(0.0));\n"
            );
        } else {
            out.push_str("    surface.volume_density = 0.0;\n    surface.volume_color = vec3<f32>(1.0);\n");
        }
        out.push_str("    return surface;\n}\n\n");
        out
    }

    /// The expression for input `socket` of `node`: the linked output converted to the socket's type, or the
    /// socket's constant (a buffer slot, or Position / Normal / Voxel color).
    fn input(&mut self, graph: &MaterialGraph, node: &GraphNode, socket: u8) -> String {
        let spec = node.kind.spec();
        let Some(input) = spec.inputs.get(usize::from(socket)) else {
            return zero(SocketType::Float).to_owned();
        };
        let link = graph.link_to(InputRef { node: node.id, socket });
        if let Some(link) = link
            && let Some(source) = graph.node(link.from.node)
            && let Some(output) = source.kind.spec().outputs.get(usize::from(link.from.socket))
        {
            return convert(var_name(source.id, link.from.socket), output.ty, input.ty);
        }
        match node.inputs.get(usize::from(socket)).copied().unwrap_or(input.default) {
            InputValue::Float(value) => format!("mp({}u)", self.alloc(&[value])),
            InputValue::Vector(value) => format!("mp3({}u)", self.alloc(&value)),
            InputValue::Color(value) => format!("mp3({}u)", self.alloc(&value.map(srgb_to_linear))),
            InputValue::Position => "position".to_owned(),
            InputValue::Normal => "normal".to_owned(),
            InputValue::VoxelColor => "albedo".to_owned(),
        }
    }

    /// Reserves slots for a parameter and returns its WGSL expression helpers.
    fn param(&mut self, node: &GraphNode, index: usize) -> Param {
        let value = node.params.get(index);
        match value {
            Some(ParamValue::Float(v)) => Param::Slot(self.alloc(&[*v])),
            #[expect(clippy::cast_precision_loss, reason = "small integer parameters")]
            Some(ParamValue::Int(v)) => Param::Slot(self.alloc(&[*v as f32])),
            Some(ParamValue::Bool(v)) => Param::Slot(self.alloc(&[f32::from(u8::from(*v))])),
            #[expect(clippy::cast_precision_loss, reason = "option indices are tiny")]
            Some(ParamValue::Choice(v)) => Param::Slot(self.alloc(&[*v as f32])),
            Some(ParamValue::Range(v)) => Param::Slot(self.alloc(v)),
            Some(ParamValue::Color(v)) => Param::Slot(self.alloc(&v.map(srgb_to_linear))),
            Some(ParamValue::Stops(stops)) => {
                let flat: Vec<f32> = stops
                    .iter()
                    .flat_map(|stop| {
                        let [r, g, b] = stop.color.map(srgb_to_linear);
                        [stop.position, r, g, b]
                    })
                    .collect();
                Param::Stops {
                    slot: self.alloc(&flat),
                    count: stops.len(),
                }
            }
            None => Param::Slot(self.alloc(&[0.0])),
        }
    }

    fn node(&mut self, graph: &MaterialGraph, node: &GraphNode, out: &mut String) {
        let spec = node.kind.spec();
        let id = node.id;
        if node.bypass {
            // Bypass: the chosen input (converted) goes straight to the first output; others are zero.
            for (socket, output) in spec.outputs.iter().enumerate() {
                let value = match (socket, spec.bypass_input) {
                    (0, Some(input)) => {
                        let socket_u8 = u8::try_from(input).unwrap_or(0);
                        let from = spec.inputs.get(input).map_or(output.ty, |i| i.ty);
                        let expr = self.input(graph, node, socket_u8);
                        convert(expr, from, output.ty)
                    }
                    (0, None) if node.kind == GraphNodeKind::VoxelBevel => "normal".to_owned(),
                    _ => zero(output.ty).to_owned(),
                };
                let _ = writeln!(
                    out,
                    "    let {}: {} = {value};",
                    var_name(id, u8::try_from(socket).unwrap_or(0)),
                    wgsl_type(output.ty)
                );
            }
            return;
        }
        let v0 = var_name(id, 0);
        match node.kind {
            GraphNodeKind::VoxelColor => {
                let _ = writeln!(
                    out,
                    "    let {v0} = albedo;\n    let {} = position;\n    let {} = normal;",
                    var_name(id, 1),
                    var_name(id, 2)
                );
            }
            GraphNodeKind::AmbientOcclusion => {
                let strength = self.param(node, 0).scalar();
                let _ = writeln!(
                    out,
                    "    let {v0} = mix(1.0, material_ao, clamp({strength}, 0.0, 1.0));"
                );
            }
            GraphNodeKind::Value => {
                let value = self.param(node, 0).scalar();
                let _ = writeln!(out, "    let {v0} = {value};");
            }
            GraphNodeKind::Noise => {
                let vector = self.input(graph, node, 0);
                let scale = self.param(node, 0).scalar();
                let detail = self.param(node, 1).scalar();
                let seed = self.param(node, 2).scalar();
                let _space = self.param(node, 3);
                let per_voxel = self.param(node, 4).scalar();
                let _ = writeln!(
                    out,
                    "    let {v0} = noise_pattern({vector}, normal, {scale}, i32({detail}), u32({seed}), {per_voxel} > 0.5);"
                );
            }
            GraphNodeKind::Cells => {
                let vector = self.input(graph, node, 0);
                let scale = self.param(node, 0).scalar();
                let randomness = self.param(node, 1).scalar();
                let seed = self.param(node, 2).scalar();
                let _space = self.param(node, 3);
                let cells = format!("c{}", id.get());
                let _ = writeln!(
                    out,
                    "    let {cells} = cells_pattern({vector}, {scale}, {randomness}, u32({seed}));\n    let {v0} = {cells}.x;\n    let {} = {cells}.y;\n    let {} = {cells}.z;",
                    var_name(id, 1),
                    var_name(id, 2)
                );
            }
            GraphNodeKind::Gradient => {
                let vector = self.input(graph, node, 0);
                let axis = self.param(node, 0).scalar();
                let range = self.param(node, 1).vec2();
                let _space = self.param(node, 2);
                let _ = writeln!(out, "    let {v0} = gradient_pattern({vector}, i32({axis}), {range});");
            }
            GraphNodeKind::EdgeWear => {
                let width = self.param(node, 0).scalar();
                let breakup = self.param(node, 1).scalar();
                let seed = self.param(node, 2).scalar();
                let _ = writeln!(
                    out,
                    "    let {v0} = edge_wear(position, normal, {width}, {breakup}, u32({seed}));"
                );
            }
            GraphNodeKind::Mix => {
                let factor = self.input(graph, node, 0);
                let a = self.input(graph, node, 1);
                let b = self.input(graph, node, 2);
                let mode = self.param(node, 0).scalar();
                let _ = writeln!(out, "    let {v0} = mix_colors({a}, {b}, {factor}, i32({mode}));");
            }
            GraphNodeKind::ColorRamp => {
                let factor = self.input(graph, node, 0);
                let (slot, count) = match self.param(node, 0) {
                    Param::Stops { slot, count } => (slot, count),
                    Param::Slot(slot) => (slot, 1),
                };
                let _ = writeln!(out, "    let {v0} = ramp_color({slot}u, {count}u, {factor});");
            }
            GraphNodeKind::HueShift => {
                let color = self.input(graph, node, 0);
                let shift = self.input(graph, node, 1);
                let saturation = self.param(node, 0).scalar();
                let value = self.param(node, 1).scalar();
                let _ = writeln!(
                    out,
                    "    let {v0} = hue_shift({color}, {shift}, {saturation}, {value});"
                );
            }
            GraphNodeKind::Remap => {
                let value = self.input(graph, node, 0);
                let from = self.param(node, 0).vec2();
                let to = self.param(node, 1).vec2();
                let clamp = self.param(node, 2).scalar();
                let _ = writeln!(out, "    let {v0} = remap_value({value}, {from}, {to}, {clamp} > 0.5);");
            }
            GraphNodeKind::Multiply => {
                let a = self.input(graph, node, 0);
                let b = self.input(graph, node, 1);
                let _ = writeln!(out, "    let {v0} = {a} * {b};");
            }
            GraphNodeKind::Clamp => {
                let value = self.input(graph, node, 0);
                let low = self.input(graph, node, 1);
                let high = self.input(graph, node, 2);
                let _ = writeln!(out, "    let {v0} = clamp_value({value}, {low}, {high});");
            }
            GraphNodeKind::VoxelBevel => {
                let radius = self.param(node, 0).scalar();
                let segments = self.param(node, 1).scalar();
                let _ = writeln!(
                    out,
                    "    let {v0} = bevel_normal(position, normal, {radius}, i32({segments}));"
                );
            }
            GraphNodeKind::SurfaceOutput | GraphNodeKind::VolumeOutput => {}
        }
    }
}

fn var_name(node: GraphNodeId, socket: u8) -> String {
    format!("v{}_{socket}", node.get())
}

/// Where a parameter lives in the buffer.
enum Param {
    Slot(usize),
    Stops { slot: usize, count: usize },
}

impl Param {
    fn slot(&self) -> usize {
        match self {
            Self::Slot(slot) | Self::Stops { slot, .. } => *slot,
        }
    }

    /// The parameter as an `f32` expression.
    fn scalar(&self) -> String {
        format!("mp({}u)", self.slot())
    }

    /// The parameter as a `vec2<f32>` expression.
    fn vec2(&self) -> String {
        format!("mp2({}u)", self.slot())
    }
}

#[cfg(test)]
mod tests {
    use rustium_core::{GraphEdit, InputRef, MaterialGraph, ParamValue, SampleMaterial, SurfaceConstants};

    use super::*;

    fn compile(graph: MaterialGraph) -> CompiledMaterials {
        compile_materials(&[MaterialGraph::default_material(), graph]).unwrap()
    }

    #[test]
    fn the_default_material_compiles_to_one_function_and_a_switch() {
        let compiled = compile_materials(&[MaterialGraph::default_material()]).unwrap();
        assert!(compiled.generated.contains("fn material_0("));
        assert!(compiled.generated.contains("fn evaluate_material("));
        assert!(compiled.generated.contains("default: { surface = material_0("));
        assert!(compiled.shader_source().contains("fn evaluate_material("));
        assert!(!compiled.shader_source().contains(MARKER));
    }

    #[test]
    fn changing_a_number_changes_only_the_buffer() {
        let mut graph = SampleMaterial::RoofSlate.build();
        let before = compile(graph.clone());
        let noise = graph
            .nodes()
            .iter()
            .find(|node| node.kind == GraphNodeKind::Noise)
            .unwrap()
            .id;
        graph
            .apply_edit(&GraphEdit::SetParam {
                node: noise,
                param: 0,
                value: ParamValue::Float(9.0),
            })
            .unwrap();
        let after = compile(graph);
        assert_eq!(before.generated, after.generated, "same shape, same source");
        assert_ne!(before.params, after.params);
        assert_eq!(before.params.len(), after.params.len());
    }

    #[test]
    fn changing_the_shape_changes_the_source() {
        let mut graph = SampleMaterial::RoofSlate.build();
        let before = compile(graph.clone());
        let surface = graph.surface_output().unwrap().id;
        graph
            .apply_edit(&GraphEdit::Disconnect {
                to: InputRef {
                    node: surface,
                    socket: 3,
                },
            })
            .unwrap();
        assert_ne!(
            before.generated,
            compile(graph.clone()).generated,
            "unlinking the bevel recompiles"
        );
        let mix = graph
            .nodes()
            .iter()
            .find(|node| node.kind == GraphNodeKind::Mix)
            .unwrap()
            .id;
        let unbypassed = compile(graph.clone());
        graph
            .apply_edit(&GraphEdit::SetBypass {
                node: mix,
                bypass: true,
            })
            .unwrap();
        assert_ne!(
            unbypassed.generated,
            compile(graph.clone()).generated,
            "bypass recompiles"
        );
    }

    #[test]
    fn socket_types_convert_like_blender() {
        assert_eq!(
            convert("x".into(), SocketType::Float, SocketType::Color),
            "vec3<f32>(x)"
        );
        assert_eq!(convert("x".into(), SocketType::Color, SocketType::Float), "luma(x)");
        assert_eq!(
            convert("x".into(), SocketType::Vector, SocketType::Float),
            "average3(x)"
        );
        assert_eq!(convert("x".into(), SocketType::Vector, SocketType::Color), "x");
    }

    #[test]
    fn constants_land_in_the_buffer_in_linear_space() {
        let graph = MaterialGraph::from_constants(
            "Lamp",
            SurfaceConstants {
                emission_color: [255, 0, 0],
                emission_strength: 3.0,
                ..SurfaceConstants::MATTE
            },
        );
        let compiled = compile_materials(&[graph]).unwrap();
        assert!(compiled.params.iter().any(|v| (*v - 3.0).abs() < 1e-6), "strength");
        assert!(
            compiled
                .params
                .windows(3)
                .any(|w| (w[0] - 1.0).abs() < 1e-6 && w[1].abs() < 1e-6 && w[2].abs() < 1e-6)
        );
    }

    #[test]
    fn an_invalid_graph_is_reported_by_name() {
        let json =
            r#"{"name":"Broken","casts_shadow":true,"nodes":[],"links":[],"frames":[],"next_node":0,"next_frame":0}"#;
        let graph: MaterialGraph = serde_json::from_str(json).unwrap();
        let error = compile_materials(&[graph]).unwrap_err();
        assert!(matches!(error, CompileError::InvalidGraph { ref name, .. } if name == "Broken"));
    }
}
