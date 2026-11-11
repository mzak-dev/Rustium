//! Material graphs: how a Scene's Colors are shaded.
//!
//! A Color (a [`Palette`](crate::Palette) entry) points at exactly one [`MaterialGraph`]; index 0 of the Scene's
//! list is the built-in Default material. A graph is a set of typed [`GraphNode`]s joined by [`Link`]s and ending in
//! one Surface output (plus, optionally, one Volume output). The renderer compiles it to WGSL behind
//! `evaluate_material`. Graphs change through [`GraphEdit`]s wrapped in [`Command::EditMaterial`](crate::Command).

mod edit;
mod nodes;
mod samples;

use std::collections::HashSet;

pub use edit::GraphEdit;
pub use nodes::{
    GraphNodeKind, InputSpec, InputValue, NodeCategory, NodeSpec, OutputSpec, ParamKind, ParamSpec, ParamValue,
    RAMP_STOPS, RampStop, SocketType,
};
pub use samples::SampleMaterial;
use serde::{Deserialize, Serialize};

/// Identifies a node inside one [`MaterialGraph`]. Ids are handed out in order and never reused in that graph, so a
/// removed node's id stays dead and undo can bring it back under the same id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct GraphNodeId(u32);

impl GraphNodeId {
    /// The raw number, for scripts and file formats.
    pub fn get(self) -> u32 {
        self.0
    }

    /// The id with number `raw`; it only names a node when the graph has one with that number.
    pub fn from_raw(raw: u32) -> Self {
        Self(raw)
    }
}

/// Identifies a [`Frame`] inside one [`MaterialGraph`]; never reused in that graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct FrameId(u32);

impl FrameId {
    /// The raw number, for scripts and file formats.
    pub fn get(self) -> u32 {
        self.0
    }

    /// The id with number `raw`.
    pub fn from_raw(raw: u32) -> Self {
        Self(raw)
    }
}

/// An output socket of a node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct OutputRef {
    /// The node.
    pub node: GraphNodeId,
    /// Index into the node kind's [`NodeSpec::outputs`].
    pub socket: u8,
}

/// An input socket of a node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct InputRef {
    /// The node.
    pub node: GraphNodeId,
    /// Index into the node kind's [`NodeSpec::inputs`].
    pub socket: u8,
}

/// A wire from an output socket to an input socket. An input has at most one link; an output any number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Link {
    /// Where the value comes from.
    pub from: OutputRef,
    /// Where it goes.
    pub to: InputRef,
}

/// One node of a graph.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GraphNode {
    /// Stable id inside the graph.
    pub id: GraphNodeId,
    /// What the node does.
    pub kind: GraphNodeKind,
    /// Title chosen by the user; empty means the kind's label.
    pub name: String,
    /// Top-left corner in graph space (the editor's coordinates).
    pub position: [f32; 2],
    /// While on, the node passes its bypass input through (see [`NodeSpec::bypass_input`]). Ignored on outputs.
    pub bypass: bool,
    /// One value per input socket, used while the socket has no link.
    pub inputs: Vec<InputValue>,
    /// One value per parameter of the kind.
    pub params: Vec<ParamValue>,
}

impl GraphNode {
    /// A node of `kind` with default inputs and parameters.
    pub fn new(id: GraphNodeId, kind: GraphNodeKind, position: [f32; 2]) -> Self {
        let spec = kind.spec();
        Self {
            id,
            kind,
            name: String::new(),
            position,
            bypass: false,
            inputs: spec.inputs.iter().map(|input| input.default).collect(),
            params: spec.params.iter().map(|param| param.kind.default_value()).collect(),
        }
    }

    /// The title to show: the user's name, or the kind's label.
    pub fn display_name(&self) -> &str {
        if self.name.is_empty() {
            self.kind.spec().label
        } else {
            &self.name
        }
    }
}

/// A labelled rectangle drawn behind nodes to group them. Purely visual.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Frame {
    /// Stable id inside the graph.
    pub id: FrameId,
    /// Text in the corner.
    pub label: String,
    /// Top-left corner in graph space.
    pub position: [f32; 2],
    /// Width and height in graph space.
    pub size: [f32; 2],
}

/// Why a graph or a graph edit is invalid.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GraphError {
    /// The graph has no node with this id.
    #[error("node {} is not in the graph", .0.get())]
    UnknownNode(GraphNodeId),
    /// The graph has no frame with this id.
    #[error("frame {} is not in the graph", .0.get())]
    UnknownFrame(FrameId),
    /// The node has no such input socket.
    #[error("node {} has no input {socket}", node.get())]
    UnknownInput {
        /// The node.
        node: GraphNodeId,
        /// The missing socket index.
        socket: u8,
    },
    /// The node has no such output socket.
    #[error("node {} has no output {socket}", node.get())]
    UnknownOutput {
        /// The node.
        node: GraphNodeId,
        /// The missing socket index.
        socket: u8,
    },
    /// The node kind has no such parameter.
    #[error("node {} has no parameter {param}", node.get())]
    UnknownParam {
        /// The node.
        node: GraphNodeId,
        /// The missing parameter index.
        param: u8,
    },
    /// The link would make a node depend on itself.
    #[error("the link would create a cycle")]
    Cycle,
    /// The Surface output cannot be removed.
    #[error("the surface output cannot be removed")]
    OutputProtected,
    /// A graph holds at most one Surface output and one Volume output.
    #[error("the graph already has a {0:?} node")]
    SecondOutput(GraphNodeKind),
    /// The value does not fit the socket or parameter it was set on.
    #[error("the value does not fit this socket or parameter")]
    WrongValue,
    /// The graph already holds the most nodes allowed.
    #[error("a graph holds at most {} nodes", MaterialGraph::MAX_NODES)]
    TooManyNodes,
    /// The graph already holds the most frames allowed.
    #[error("a graph holds at most {} frames", MaterialGraph::MAX_FRAMES)]
    TooManyFrames,
    /// The graph has no Surface output.
    #[error("the graph has no surface output")]
    NoSurface,
    /// The graph is malformed (only possible for data that did not come through the edit API).
    #[error("malformed graph: {0}")]
    Malformed(&'static str),
}

/// The constant parameters of a graph's Surface output; the simple way to make or read a one-node Material.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SurfaceConstants {
    /// Surface roughness, `0.0` (mirror) to `1.0` (matte).
    pub roughness: f32,
    /// `0.0` for dielectrics (paint, plastic) up to `1.0` for bare metal.
    pub metallic: f32,
    /// Color of the light the voxel emits, as 8-bit sRGB.
    pub emission_color: [u8; 3],
    /// Brightness of the emitted light; `0.0` means the voxel does not glow.
    pub emission_strength: f32,
    /// How much light passes through instead of bouncing off, `0.0` (opaque) to `1.0` (clear glass).
    pub transmission: f32,
    /// Index of refraction used when `transmission` is above zero.
    pub ior: f32,
}

impl SurfaceConstants {
    /// Largest emission strength.
    pub const MAX_EMISSION: f32 = 32.0;
    /// Smallest and largest index of refraction.
    pub const IOR_RANGE: (f32, f32) = (1.0, 3.0);

    /// An opaque, matte, non-emissive surface.
    pub const MATTE: Self = Self {
        roughness: 0.8,
        metallic: 0.0,
        emission_color: [255, 255, 255],
        emission_strength: 0.0,
        transmission: 0.0,
        ior: 1.5,
    };

    /// The same constants with every parameter pulled into its valid range (NaN falls back to the matte default).
    #[must_use]
    pub fn sanitized(self) -> Self {
        let clamp = |value: f32, min: f32, max: f32, fallback: f32| {
            if value.is_nan() {
                fallback
            } else {
                value.clamp(min, max)
            }
        };
        Self {
            roughness: clamp(self.roughness, 0.0, 1.0, Self::MATTE.roughness),
            metallic: clamp(self.metallic, 0.0, 1.0, Self::MATTE.metallic),
            emission_strength: clamp(self.emission_strength, 0.0, Self::MAX_EMISSION, 0.0),
            transmission: clamp(self.transmission, 0.0, 1.0, 0.0),
            ior: clamp(self.ior, Self::IOR_RANGE.0, Self::IOR_RANGE.1, Self::MATTE.ior),
            ..self
        }
    }
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the value is clamped to 0..=255 before the cast"
)]
fn to_byte(channel: f32) -> u8 {
    (channel * 255.0).round().clamp(0.0, 255.0) as u8
}

// Indices of the Surface output's inputs (see `GraphNodeKind::SurfaceOutput`).
const SURFACE_METALLIC: usize = 1;
const SURFACE_ROUGHNESS: usize = 2;
const SURFACE_EMISSION: usize = 4;
const SURFACE_EMISSION_STRENGTH: usize = 5;
const SURFACE_TRANSMISSION: usize = 6;
const SURFACE_IOR: usize = 7;

/// A named Material: a node graph that decides what a Color looks like beyond its base color.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MaterialGraph {
    /// Name shown in the interface, for example "Roof slate".
    pub name: String,
    /// Whether voxels with this Material block light. Stored for the renderers; honoured once they support it.
    pub casts_shadow: bool,
    nodes: Vec<GraphNode>,
    links: Vec<Link>,
    frames: Vec<Frame>,
    next_node: u32,
    next_frame: u32,
}

impl MaterialGraph {
    /// Most graphs one Scene can hold (the GPU table has this many records).
    pub const MAX_PER_SCENE: usize = 64;
    /// Most nodes one graph can hold.
    pub const MAX_NODES: usize = 128;
    /// Most frames one graph can hold.
    pub const MAX_FRAMES: usize = 32;

    fn empty(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            casts_shadow: true,
            nodes: Vec::new(),
            links: Vec::new(),
            frames: Vec::new(),
            next_node: 0,
            next_frame: 0,
        }
    }

    /// The built-in Default material every Scene starts with at index 0: just a Surface output.
    pub fn default_material() -> Self {
        Self::from_constants("Default material", SurfaceConstants::MATTE)
    }

    /// A starter Material: the Voxel color wired into a Surface output.
    pub fn blank(name: &str) -> Self {
        let mut graph = Self::empty(name);
        let color = graph.push_node(GraphNodeKind::VoxelColor, [20.0, 110.0]);
        let surface = graph.push_node(GraphNodeKind::SurfaceOutput, [520.0, 90.0]);
        graph.push_link(color, 0, surface, 0);
        graph
    }

    /// A Material that is only a Surface output with the given constants (clamped).
    pub fn from_constants(name: &str, surface: SurfaceConstants) -> Self {
        let mut graph = Self::empty(name);
        graph.push_node(GraphNodeKind::SurfaceOutput, [300.0, 90.0]);
        graph.set_surface_constants(surface);
        graph
    }

    // Builders for code that is known to be valid (blank graphs, samples); they cannot fail.
    pub(crate) fn push_node(&mut self, kind: GraphNodeKind, position: [f32; 2]) -> GraphNodeId {
        let id = GraphNodeId(self.next_node);
        self.next_node += 1;
        self.nodes.push(GraphNode::new(id, kind, position));
        id
    }

    pub(crate) fn push_link(&mut self, from: GraphNodeId, output: u8, to: GraphNodeId, input: u8) {
        self.links.retain(|link| {
            link.to
                != InputRef {
                    node: to,
                    socket: input,
                }
        });
        self.links.push(Link {
            from: OutputRef {
                node: from,
                socket: output,
            },
            to: InputRef {
                node: to,
                socket: input,
            },
        });
    }

    pub(crate) fn push_frame(&mut self, label: &str, position: [f32; 2], size: [f32; 2]) {
        let id = FrameId(self.next_frame);
        self.next_frame += 1;
        self.frames.push(Frame {
            id,
            label: label.to_owned(),
            position,
            size,
        });
    }

    pub(crate) fn node_mut(&mut self, id: GraphNodeId) -> Option<&mut GraphNode> {
        self.nodes.iter_mut().find(|node| node.id == id)
    }

    /// The id the next added node will get; use it to address a node in the same step as adding it.
    pub fn next_node_id(&self) -> GraphNodeId {
        GraphNodeId(self.next_node)
    }

    /// The id the next added frame will get.
    pub fn next_frame_id(&self) -> FrameId {
        FrameId(self.next_frame)
    }

    /// All nodes, in creation order.
    pub fn nodes(&self) -> &[GraphNode] {
        &self.nodes
    }

    /// All links.
    pub fn links(&self) -> &[Link] {
        &self.links
    }

    /// All frames.
    pub fn frames(&self) -> &[Frame] {
        &self.frames
    }

    /// The node with `id`.
    pub fn node(&self, id: GraphNodeId) -> Option<&GraphNode> {
        self.nodes.iter().find(|node| node.id == id)
    }

    /// The frame with `id`.
    pub fn frame(&self, id: FrameId) -> Option<&Frame> {
        self.frames.iter().find(|frame| frame.id == id)
    }

    /// The link feeding `input`, if any.
    pub fn link_to(&self, input: InputRef) -> Option<&Link> {
        self.links.iter().find(|link| link.to == input)
    }

    /// The Surface output node; every valid graph has exactly one.
    pub fn surface_output(&self) -> Option<&GraphNode> {
        self.nodes.iter().find(|node| node.kind == GraphNodeKind::SurfaceOutput)
    }

    /// The Volume output node, if the graph has one.
    pub fn volume_output(&self) -> Option<&GraphNode> {
        self.nodes.iter().find(|node| node.kind == GraphNodeKind::VolumeOutput)
    }

    /// Nodes `node` reads from: through all linked inputs, or only the bypass input while bypassed.
    fn dependencies(&self, node: GraphNodeId) -> Vec<GraphNodeId> {
        let Some(found) = self.node(node) else {
            return Vec::new();
        };
        let bypassed = found.bypass && !found.kind.is_output();
        let only = found.kind.spec().bypass_input;
        self.links
            .iter()
            .filter(|link| link.to.node == node)
            .filter(|link| !bypassed || Some(usize::from(link.to.socket)) == only)
            .map(|link| link.from.node)
            .collect()
    }

    /// Whether a link `from -> to` would close a cycle (counting every link, bypassed or not).
    pub(crate) fn would_cycle(&self, from: GraphNodeId, to: GraphNodeId) -> bool {
        if from == to {
            return true;
        }
        let mut seen = HashSet::new();
        let mut stack = vec![from];
        while let Some(current) = stack.pop() {
            if current == to {
                return true;
            }
            if seen.insert(current) {
                stack.extend(self.links.iter().filter(|l| l.to.node == current).map(|l| l.from.node));
            }
        }
        false
    }

    /// The nodes the outputs depend on, ordered so every node comes after the nodes it reads (Bypass honoured).
    /// Nodes no output reaches are left out; the compiler skips them.
    pub fn evaluation_order(&self) -> Vec<GraphNodeId> {
        fn visit(
            graph: &MaterialGraph,
            id: GraphNodeId,
            seen: &mut HashSet<GraphNodeId>,
            order: &mut Vec<GraphNodeId>,
        ) {
            if !seen.insert(id) {
                return;
            }
            for dependency in graph.dependencies(id) {
                visit(graph, dependency, seen, order);
            }
            order.push(id);
        }
        let mut seen = HashSet::new();
        let mut order = Vec::new();
        for node in self.nodes.iter().filter(|node| node.kind.is_output()) {
            visit(self, node.id, &mut seen, &mut order);
        }
        order
    }

    /// Checks that the graph is well formed: exactly one Surface output, at most one Volume output, every node
    /// consistent with its kind, links between existing sockets with at most one per input, and no cycles.
    ///
    /// Graphs built through [`GraphEdit`]s are always valid; call this on graphs read from a file.
    ///
    /// # Errors
    ///
    /// The first problem found, as a [`GraphError`].
    pub fn validate(&self) -> Result<(), GraphError> {
        if self.nodes.len() > Self::MAX_NODES {
            return Err(GraphError::TooManyNodes);
        }
        if self.frames.len() > Self::MAX_FRAMES {
            return Err(GraphError::TooManyFrames);
        }
        let mut ids = HashSet::new();
        for node in &self.nodes {
            if !ids.insert(node.id) || node.id.0 >= self.next_node {
                return Err(GraphError::Malformed("duplicate or unissued node id"));
            }
            let spec = node.kind.spec();
            if node.inputs.len() != spec.inputs.len() || node.params.len() != spec.params.len() {
                return Err(GraphError::Malformed("node does not match its kind"));
            }
            for (value, input) in node.inputs.iter().zip(spec.inputs) {
                if !value.fits(input.ty) {
                    return Err(GraphError::WrongValue);
                }
            }
            for (value, param) in node.params.iter().zip(spec.params) {
                if param.kind.sanitize(value).is_none() {
                    return Err(GraphError::WrongValue);
                }
            }
        }
        let mut frame_ids = HashSet::new();
        if !self
            .frames
            .iter()
            .all(|frame| frame_ids.insert(frame.id) && frame.id.0 < self.next_frame)
        {
            return Err(GraphError::Malformed("duplicate or unissued frame id"));
        }
        let mut inputs = HashSet::new();
        for link in &self.links {
            let from = self
                .node(link.from.node)
                .ok_or(GraphError::UnknownNode(link.from.node))?;
            let to = self.node(link.to.node).ok_or(GraphError::UnknownNode(link.to.node))?;
            if usize::from(link.from.socket) >= from.kind.spec().outputs.len() {
                return Err(GraphError::UnknownOutput {
                    node: from.id,
                    socket: link.from.socket,
                });
            }
            if usize::from(link.to.socket) >= to.kind.spec().inputs.len() {
                return Err(GraphError::UnknownInput {
                    node: to.id,
                    socket: link.to.socket,
                });
            }
            if !inputs.insert(link.to) {
                return Err(GraphError::Malformed("an input has two links"));
            }
        }
        for kind in [GraphNodeKind::SurfaceOutput, GraphNodeKind::VolumeOutput] {
            let count = self.nodes.iter().filter(|node| node.kind == kind).count();
            if count > 1 {
                return Err(GraphError::SecondOutput(kind));
            }
            if count == 0 && kind == GraphNodeKind::SurfaceOutput {
                return Err(GraphError::NoSurface);
            }
        }
        // A cycle shows up as a node that is not removed by repeatedly peeling nodes without unresolved inputs.
        let mut resolved = HashSet::new();
        loop {
            let before = resolved.len();
            for node in &self.nodes {
                if !resolved.contains(&node.id)
                    && self
                        .links
                        .iter()
                        .filter(|link| link.to.node == node.id)
                        .all(|link| resolved.contains(&link.from.node))
                {
                    resolved.insert(node.id);
                }
            }
            if resolved.len() == before {
                break;
            }
        }
        if resolved.len() != self.nodes.len() {
            return Err(GraphError::Cycle);
        }
        Ok(())
    }

    /// The Surface output's constants, read from its unlinked-input defaults (a linked input still reports its
    /// default). Used by the temporary interface and by `Command::EditGraph`.
    pub fn surface_constants(&self) -> SurfaceConstants {
        let Some(node) = self.surface_output() else {
            return SurfaceConstants::MATTE;
        };
        let float = |index: usize, fallback: f32| match node.inputs.get(index) {
            Some(&InputValue::Float(value)) => value,
            _ => fallback,
        };
        let emission_color = match node.inputs.get(SURFACE_EMISSION) {
            Some(&InputValue::Color(color)) => color.map(to_byte),
            _ => SurfaceConstants::MATTE.emission_color,
        };
        SurfaceConstants {
            roughness: float(SURFACE_ROUGHNESS, SurfaceConstants::MATTE.roughness),
            metallic: float(SURFACE_METALLIC, 0.0),
            emission_color,
            emission_strength: float(SURFACE_EMISSION_STRENGTH, 0.0),
            transmission: float(SURFACE_TRANSMISSION, 0.0),
            ior: float(SURFACE_IOR, SurfaceConstants::MATTE.ior),
        }
    }

    /// Writes `surface` (clamped) into the Surface output's input defaults; returns whether anything changed.
    pub fn set_surface_constants(&mut self, surface: SurfaceConstants) -> bool {
        let surface = surface.sanitized();
        let Some(node) = self
            .nodes
            .iter_mut()
            .find(|node| node.kind == GraphNodeKind::SurfaceOutput)
        else {
            return false;
        };
        let before = node.inputs.clone();
        let mut set = |index: usize, value: InputValue| {
            if let Some(slot) = node.inputs.get_mut(index) {
                *slot = value;
            }
        };
        set(SURFACE_METALLIC, InputValue::Float(surface.metallic));
        set(SURFACE_ROUGHNESS, InputValue::Float(surface.roughness));
        set(
            SURFACE_EMISSION,
            InputValue::Color(surface.emission_color.map(|c| f32::from(c) / 255.0)),
        );
        set(SURFACE_EMISSION_STRENGTH, InputValue::Float(surface.emission_strength));
        set(SURFACE_TRANSMISSION, InputValue::Float(surface.transmission));
        set(SURFACE_IOR, InputValue::Float(surface.ior));
        node.inputs != before
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitized_clamps_every_parameter_and_replaces_nan() {
        let wild = SurfaceConstants {
            roughness: -3.0,
            metallic: 7.0,
            emission_strength: 1e9,
            transmission: f32::NAN,
            ior: 0.2,
            ..SurfaceConstants::MATTE
        };
        let clean = wild.sanitized();
        assert_eq!(clean.roughness.to_bits(), 0.0_f32.to_bits());
        assert_eq!(clean.metallic.to_bits(), 1.0_f32.to_bits());
        assert_eq!(
            clean.emission_strength.to_bits(),
            SurfaceConstants::MAX_EMISSION.to_bits()
        );
        assert_eq!(clean.transmission.to_bits(), 0.0_f32.to_bits());
        assert_eq!(clean.ior.to_bits(), 1.0_f32.to_bits());
    }

    #[test]
    fn the_default_material_is_a_matte_surface_output() {
        let graph = MaterialGraph::default_material();
        assert_eq!(graph.surface_constants(), SurfaceConstants::MATTE);
        assert_eq!(graph.nodes().len(), 1);
        assert_eq!(graph.validate(), Ok(()));
    }

    #[test]
    fn constants_round_trip_through_the_surface_output() {
        let wanted = SurfaceConstants {
            roughness: 0.25,
            metallic: 1.0,
            emission_color: [255, 128, 0],
            emission_strength: 4.0,
            transmission: 0.5,
            ior: 1.33,
        };
        let graph = MaterialGraph::from_constants("Test", wanted);
        assert_eq!(graph.surface_constants(), wanted);
    }

    #[test]
    fn a_blank_material_links_the_voxel_color_to_the_surface() {
        let graph = MaterialGraph::blank("New");
        assert_eq!(graph.nodes().len(), 2);
        assert_eq!(graph.links().len(), 1);
        assert_eq!(graph.validate(), Ok(()));
        assert_eq!(graph.evaluation_order().len(), 2);
    }

    #[test]
    fn validation_rejects_a_missing_surface_and_a_cycle() {
        let mut graph = MaterialGraph::blank("Bad");
        let bad = serde_roundtrip_without_surface(&graph);
        assert_eq!(bad.validate(), Err(GraphError::NoSurface));

        let a = graph.push_node(GraphNodeKind::Multiply, [0.0, 0.0]);
        let b = graph.push_node(GraphNodeKind::Multiply, [0.0, 0.0]);
        graph.push_link(a, 0, b, 0);
        graph.push_link(b, 0, a, 0);
        assert_eq!(graph.validate(), Err(GraphError::Cycle));
    }

    fn serde_roundtrip_without_surface(graph: &MaterialGraph) -> MaterialGraph {
        let mut bad = graph.clone();
        bad.nodes.retain(|node| node.kind != GraphNodeKind::SurfaceOutput);
        bad.links.clear();
        bad
    }

    #[test]
    fn bypass_changes_what_the_order_depends_on() {
        let mut graph = MaterialGraph::blank("Order");
        let noise = graph.push_node(GraphNodeKind::Noise, [0.0, 0.0]);
        let mix = graph.push_node(GraphNodeKind::Mix, [0.0, 0.0]);
        let surface = graph.surface_output().map(|node| node.id).unwrap();
        graph.push_link(noise, 0, mix, 0);
        graph.push_link(mix, 0, surface, 0);
        assert!(graph.evaluation_order().contains(&noise));
        if let Some(node) = graph.node_mut(mix) {
            node.bypass = true;
        }
        assert!(
            !graph.evaluation_order().contains(&noise),
            "Factor is not the bypass input"
        );
    }
}
