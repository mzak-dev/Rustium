//! Starter Materials the interface can offer when creating a new Material (the ones in the Effects mock-up).

use super::{GraphNodeId, GraphNodeKind, InputValue, MaterialGraph, ParamValue, RampStop};

/// A ready-made Material that [`SampleMaterial::build`] turns into a [`MaterialGraph`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SampleMaterial {
    /// Polished yellow metal with worn edges.
    Brass,
    /// Clear glass.
    Glass,
    /// Grass with lighter tips and per-voxel hue variation.
    Grass,
    /// A glowing lamp.
    LampGlow,
    /// Slightly mottled plaster.
    Plaster,
    /// Slate tiles with variation and rounded edges (the mock-up's example graph).
    RoofSlate,
    /// Stone with dark cracks between cells.
    Stone,
    /// Wood with grain.
    Wood,
}

impl SampleMaterial {
    /// Every sample, in the mock-up's hotbar order.
    pub const ALL: [Self; 8] = [
        Self::RoofSlate,
        Self::Plaster,
        Self::Grass,
        Self::Wood,
        Self::Glass,
        Self::Stone,
        Self::Brass,
        Self::LampGlow,
    ];

    /// Name of the Material it creates.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Brass => "Brass",
            Self::Glass => "Glass",
            Self::Grass => "Grass",
            Self::LampGlow => "Lamp glow",
            Self::Plaster => "Plaster",
            Self::RoofSlate => "Roof slate",
            Self::Stone => "Stone",
            Self::Wood => "Wood",
        }
    }

    /// Highlight, mid and shadow colours (8-bit sRGB) of the ball that stands for the Material in the hotbar.
    pub const fn swatch(self) -> [[u8; 3]; 3] {
        match self {
            Self::RoofSlate => [[0x8f, 0xa3, 0xb5], [0x5b, 0x70, 0x83], [0x2c, 0x39, 0x45]],
            Self::Plaster => [[0xf2, 0xeb, 0xdd], [0xd6, 0xcc, 0xba], [0x8f, 0x86, 0x76]],
            Self::Grass => [[0xa5, 0xcf, 0x8c], [0x6e, 0x9b, 0x55], [0x34, 0x52, 0x2a]],
            Self::Wood => [[0xb9, 0x8a, 0x66], [0x7a, 0x5a, 0x40], [0x3a, 0x2a, 0x1e]],
            Self::Glass => [[0xe3, 0xf2, 0xf7], [0x8f, 0xb3, 0xc2], [0x3f, 0x5e, 0x6b]],
            Self::Stone => [[0xc2, 0xc2, 0xc2], [0x8a, 0x8a, 0x8a], [0x45, 0x45, 0x45]],
            Self::Brass => [[0xf3, 0xdc, 0x98], [0xb8, 0x92, 0x3c], [0x5a, 0x43, 0x18]],
            Self::LampGlow => [[0xff, 0xf6, 0xd6], [0xf2, 0xc7, 0x5c], [0xc9, 0x8a, 0x1e]],
        }
    }

    /// Builds the graph. The result is always valid.
    pub fn build(self) -> MaterialGraph {
        let mut g = MaterialGraph::empty(self.label());
        match self {
            Self::RoofSlate => roof_slate(&mut g),
            Self::Plaster => plaster(&mut g),
            Self::Grass => grass(&mut g),
            Self::Wood => wood(&mut g),
            Self::Glass => glass(&mut g),
            Self::Stone => stone(&mut g),
            Self::Brass => brass(&mut g),
            Self::LampGlow => lamp_glow(&mut g),
        }
        g
    }
}

/// An 8-bit sRGB color as floats.
fn rgb(r: u8, g: u8, b: u8) -> [f32; 3] {
    [r, g, b].map(|channel| f32::from(channel) / 255.0)
}

fn set_param(g: &mut MaterialGraph, node: GraphNodeId, name: &str, value: ParamValue) {
    if let Some(found) = g.node_mut(node)
        && let Some(index) = found.kind.spec().params.iter().position(|param| param.name == name)
    {
        found.params[index] = value;
    }
}

fn set_input(g: &mut MaterialGraph, node: GraphNodeId, name: &str, value: InputValue) {
    if let Some(found) = g.node_mut(node)
        && let Some(index) = found.kind.spec().inputs.iter().position(|input| input.name == name)
    {
        found.inputs[index] = value;
    }
}

/// Links `from`'s output called `output` to `to`'s input called `input`.
fn link(g: &mut MaterialGraph, from: GraphNodeId, output: &str, to: GraphNodeId, input: &str) {
    let output = g
        .node(from)
        .and_then(|node| node.kind.spec().outputs.iter().position(|spec| spec.name == output));
    let input = g
        .node(to)
        .and_then(|node| node.kind.spec().inputs.iter().position(|spec| spec.name == input));
    if let (Some(output), Some(input)) = (output, input)
        && let (Ok(output), Ok(input)) = (u8::try_from(output), u8::try_from(input))
    {
        g.push_link(from, output, to, input);
    }
}

fn roof_slate(g: &mut MaterialGraph) {
    let voxel = g.push_node(GraphNodeKind::VoxelColor, [20.0, 110.0]);
    let noise = g.push_node(GraphNodeKind::Noise, [270.0, 250.0]);
    let mix = g.push_node(GraphNodeKind::Mix, [530.0, 96.0]);
    let remap = g.push_node(GraphNodeKind::Remap, [530.0, 300.0]);
    let bevel = g.push_node(GraphNodeKind::VoxelBevel, [530.0, 504.0]);
    let surface = g.push_node(GraphNodeKind::SurfaceOutput, [800.0, 180.0]);
    set_param(g, noise, "Scale", ParamValue::Float(4.0));
    set_param(g, noise, "Detail", ParamValue::Int(2));
    set_param(g, noise, "Seed", ParamValue::Int(17));
    set_param(g, noise, "Per voxel", ParamValue::Bool(true));
    set_input(g, mix, "B", InputValue::Color(rgb(0x3e, 0x4a, 0x56)));
    set_param(g, remap, "To", ParamValue::Range([0.55, 0.85]));
    link(g, voxel, "Color", mix, "A");
    link(g, voxel, "Position", noise, "Vector");
    link(g, noise, "Value", mix, "Factor");
    link(g, noise, "Value", remap, "Value");
    link(g, mix, "Color", surface, "Base color");
    link(g, remap, "Result", surface, "Roughness");
    link(g, bevel, "Normal", surface, "Normal");
    g.push_frame("Surface variation", [250.0, 56.0], [484.0, 404.0]);
}

fn plaster(g: &mut MaterialGraph) {
    let voxel = g.push_node(GraphNodeKind::VoxelColor, [20.0, 90.0]);
    let noise = g.push_node(GraphNodeKind::Noise, [20.0, 280.0]);
    let mix = g.push_node(GraphNodeKind::Mix, [280.0, 90.0]);
    let surface = g.push_node(GraphNodeKind::SurfaceOutput, [540.0, 120.0]);
    set_param(g, noise, "Scale", ParamValue::Float(10.0));
    set_param(g, noise, "Detail", ParamValue::Int(1));
    set_param(g, noise, "Per voxel", ParamValue::Bool(true));
    set_param(g, mix, "Blend", ParamValue::Choice(1));
    set_input(g, mix, "B", InputValue::Color(rgb(0xd9, 0xd2, 0xc4)));
    set_input(g, surface, "Roughness", InputValue::Float(0.95));
    link(g, voxel, "Color", mix, "A");
    link(g, voxel, "Position", noise, "Vector");
    link(g, noise, "Value", mix, "Factor");
    link(g, mix, "Color", surface, "Base color");
}

fn grass(g: &mut MaterialGraph) {
    let voxel = g.push_node(GraphNodeKind::VoxelColor, [20.0, 130.0]);
    let gradient = g.push_node(GraphNodeKind::Gradient, [20.0, 330.0]);
    let noise = g.push_node(GraphNodeKind::Noise, [20.0, 520.0]);
    let remap = g.push_node(GraphNodeKind::Remap, [270.0, 520.0]);
    let mix = g.push_node(GraphNodeKind::Mix, [270.0, 130.0]);
    let hue = g.push_node(GraphNodeKind::HueShift, [540.0, 130.0]);
    let surface = g.push_node(GraphNodeKind::SurfaceOutput, [800.0, 200.0]);
    set_param(g, gradient, "Range", ParamValue::Range([0.0, 8.0]));
    set_param(g, noise, "Scale", ParamValue::Float(12.0));
    set_param(g, noise, "Detail", ParamValue::Int(1));
    set_param(g, noise, "Per voxel", ParamValue::Bool(true));
    set_param(g, remap, "To", ParamValue::Range([-14.0, 14.0]));
    set_input(g, mix, "B", InputValue::Color(rgb(0xb5, 0xd8, 0x6a)));
    set_input(g, surface, "Roughness", InputValue::Float(0.9));
    link(g, voxel, "Color", mix, "A");
    link(g, voxel, "Position", noise, "Vector");
    link(g, gradient, "Value", mix, "Factor");
    link(g, noise, "Value", remap, "Value");
    link(g, mix, "Color", hue, "Color");
    link(g, remap, "Result", hue, "Shift");
    link(g, hue, "Color", surface, "Base color");
}

fn wood(g: &mut MaterialGraph) {
    let voxel = g.push_node(GraphNodeKind::VoxelColor, [20.0, 90.0]);
    let noise = g.push_node(GraphNodeKind::Noise, [20.0, 300.0]);
    let ramp = g.push_node(GraphNodeKind::ColorRamp, [280.0, 300.0]);
    let mix = g.push_node(GraphNodeKind::Mix, [540.0, 120.0]);
    let surface = g.push_node(GraphNodeKind::SurfaceOutput, [800.0, 150.0]);
    set_param(g, noise, "Scale", ParamValue::Float(1.5));
    set_param(g, noise, "Detail", ParamValue::Int(3));
    set_param(
        g,
        ramp,
        "Stops",
        ParamValue::Stops(vec![
            RampStop {
                position: 0.0,
                color: rgb(0x5a, 0x3a, 0x22),
            },
            RampStop {
                position: 0.55,
                color: rgb(0xa8, 0x7a, 0x52),
            },
            RampStop {
                position: 1.0,
                color: rgb(0xc9, 0xa0, 0x74),
            },
        ]),
    );
    set_input(g, mix, "Factor", InputValue::Float(0.6));
    set_param(g, mix, "Blend", ParamValue::Choice(1));
    set_input(g, surface, "Roughness", InputValue::Float(0.65));
    link(g, voxel, "Color", mix, "A");
    link(g, voxel, "Position", noise, "Vector");
    link(g, noise, "Value", ramp, "Factor");
    link(g, ramp, "Color", mix, "B");
    link(g, mix, "Color", surface, "Base color");
}

fn glass(g: &mut MaterialGraph) {
    let voxel = g.push_node(GraphNodeKind::VoxelColor, [20.0, 110.0]);
    let roughness = g.push_node(GraphNodeKind::Value, [20.0, 300.0]);
    let surface = g.push_node(GraphNodeKind::SurfaceOutput, [300.0, 150.0]);
    set_param(g, roughness, "Value", ParamValue::Float(0.03));
    set_input(g, surface, "Transmission", InputValue::Float(1.0));
    set_input(g, surface, "IOR", InputValue::Float(1.5));
    link(g, voxel, "Color", surface, "Base color");
    link(g, roughness, "Value", surface, "Roughness");
    g.casts_shadow = false;
}

fn stone(g: &mut MaterialGraph) {
    let voxel = g.push_node(GraphNodeKind::VoxelColor, [20.0, 110.0]);
    let cells = g.push_node(GraphNodeKind::Cells, [20.0, 300.0]);
    let mix = g.push_node(GraphNodeKind::Mix, [300.0, 110.0]);
    let surface = g.push_node(GraphNodeKind::SurfaceOutput, [570.0, 140.0]);
    set_param(g, cells, "Scale", ParamValue::Float(3.0));
    set_input(g, mix, "A", InputValue::Color(rgb(0x4a, 0x4a, 0x4a)));
    set_input(g, surface, "Roughness", InputValue::Float(0.9));
    link(g, voxel, "Color", mix, "B");
    link(g, voxel, "Position", cells, "Vector");
    link(g, cells, "Border", mix, "Factor");
    link(g, mix, "Color", surface, "Base color");
}

fn brass(g: &mut MaterialGraph) {
    let voxel = g.push_node(GraphNodeKind::VoxelColor, [20.0, 110.0]);
    let wear = g.push_node(GraphNodeKind::EdgeWear, [20.0, 300.0]);
    let remap = g.push_node(GraphNodeKind::Remap, [280.0, 300.0]);
    let surface = g.push_node(GraphNodeKind::SurfaceOutput, [550.0, 150.0]);
    set_param(g, remap, "To", ParamValue::Range([0.25, 0.7]));
    set_input(g, surface, "Metallic", InputValue::Float(1.0));
    link(g, voxel, "Color", surface, "Base color");
    link(g, wear, "Mask", remap, "Value");
    link(g, remap, "Result", surface, "Roughness");
}

fn lamp_glow(g: &mut MaterialGraph) {
    let voxel = g.push_node(GraphNodeKind::VoxelColor, [20.0, 110.0]);
    let surface = g.push_node(GraphNodeKind::SurfaceOutput, [300.0, 90.0]);
    set_input(g, surface, "Emission strength", InputValue::Float(4.0));
    link(g, voxel, "Color", surface, "Emission");
    link(g, voxel, "Color", surface, "Base color");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_sample_is_a_valid_graph_with_its_name() {
        for sample in SampleMaterial::ALL {
            let graph = sample.build();
            assert_eq!(graph.validate(), Ok(()), "{}", sample.label());
            assert_eq!(graph.name, sample.label());
        }
    }

    #[test]
    fn roof_slate_matches_the_mock_up() {
        let graph = SampleMaterial::RoofSlate.build();
        assert_eq!(graph.nodes().len(), 6);
        assert_eq!(graph.links().len(), 7);
        assert_eq!(graph.frames().len(), 1);
    }

    #[test]
    fn samples_with_a_custom_look_use_the_surface_inputs() {
        assert!(SampleMaterial::Glass.build().surface_constants().transmission > 0.9);
        assert!(SampleMaterial::LampGlow.build().surface_constants().emission_strength > 1.0);
        assert!(SampleMaterial::Brass.build().surface_constants().metallic > 0.9);
    }

    #[test]
    fn graphs_survive_a_serde_round_trip() {
        for sample in SampleMaterial::ALL {
            let graph = sample.build();
            let text = serde_json::to_string(&graph).unwrap();
            let back: MaterialGraph = serde_json::from_str(&text).unwrap();
            assert_eq!(back, graph, "{}", sample.label());
            assert_eq!(back.validate(), Ok(()));
        }
    }

    #[test]
    fn rgb_decodes_channels() {
        let [r, g, b] = rgb(0x3e, 0x4a, 0x56);
        assert!((r - 62.0 / 255.0).abs() < 1e-6 && (g - 74.0 / 255.0).abs() < 1e-6 && (b - 86.0 / 255.0).abs() < 1e-6);
    }
}
