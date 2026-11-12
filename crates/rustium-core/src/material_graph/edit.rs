//! Changes to the inside of one Material graph.

use serde::{Deserialize, Serialize};

use super::{
    Frame, FrameId, GraphError, GraphNode, GraphNodeId, GraphNodeKind, InputRef, InputValue, MaterialGraph, OutputRef,
    ParamValue,
};
use crate::edit::EditKind;

/// Longest node name or frame label kept, in characters.
const MAX_NAME: usize = 64;
/// Smallest width or height of a frame in graph space.
const MIN_FRAME: f32 = 16.0;

/// One undoable change to a Material graph. Apply it with [`Command::EditMaterial`](crate::Command).
///
/// A node added by [`GraphEdit::AddNode`] gets [`MaterialGraph::next_node_id`] as it was before the edit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum GraphEdit {
    /// Adds a node of `kind` with default inputs and parameters.
    AddNode {
        /// What kind of node.
        kind: GraphNodeKind,
        /// Top-left corner in graph space.
        position: [f32; 2],
    },
    /// Removes nodes with every link touching them. The Surface output cannot be removed.
    RemoveNodes {
        /// The nodes to remove.
        nodes: Vec<GraphNodeId>,
    },
    /// Moves nodes and frames by `delta` in graph space. Merge the Edits of one drag into a single History step.
    MoveNodes {
        /// The nodes to move.
        nodes: Vec<GraphNodeId>,
        /// The frames to move.
        frames: Vec<FrameId>,
        /// How far, in graph space.
        delta: [f32; 2],
    },
    /// Links an output to an input, replacing the input's current link. Rejected if it would create a cycle.
    Connect {
        /// Where the value comes from.
        from: OutputRef,
        /// Where it goes.
        to: InputRef,
    },
    /// Removes the link feeding an input.
    Disconnect {
        /// The input to free.
        to: InputRef,
    },
    /// Sets the constant an unlinked input uses.
    SetInput {
        /// The node.
        node: GraphNodeId,
        /// Index into the kind's inputs.
        input: u8,
        /// The new constant; it must fit the socket's type.
        value: InputValue,
    },
    /// Sets a parameter (clamped to its range).
    SetParam {
        /// The node.
        node: GraphNodeId,
        /// Index into the kind's parameters.
        param: u8,
        /// The new value; it must be the parameter's variant.
        value: ParamValue,
    },
    /// Gives a node its own title (empty restores the kind's label).
    RenameNode {
        /// The node.
        node: GraphNodeId,
        /// The new title.
        name: String,
    },
    /// Turns a node's Bypass on or off.
    SetBypass {
        /// The node.
        node: GraphNodeId,
        /// Whether the node is bypassed.
        bypass: bool,
    },
    /// Adds a labelled frame.
    AddFrame {
        /// Text in the corner.
        label: String,
        /// Top-left corner in graph space.
        position: [f32; 2],
        /// Width and height in graph space.
        size: [f32; 2],
    },
    /// Changes a frame's label, place and size.
    SetFrame {
        /// The frame.
        frame: FrameId,
        /// New text.
        label: String,
        /// New top-left corner.
        position: [f32; 2],
        /// New width and height.
        size: [f32; 2],
    },
    /// Removes a frame (the nodes inside stay).
    RemoveFrame {
        /// The frame.
        frame: FrameId,
    },
}

impl GraphEdit {
    /// The History label of this edit.
    pub fn kind(&self) -> EditKind {
        match self {
            Self::AddNode { .. } => EditKind::AddGraphNode,
            Self::RemoveNodes { .. } => EditKind::RemoveGraphNodes,
            Self::MoveNodes { .. } => EditKind::MoveGraphNodes,
            Self::Connect { .. } => EditKind::ConnectGraph,
            Self::Disconnect { .. } => EditKind::DisconnectGraph,
            Self::SetInput { .. } | Self::SetParam { .. } => EditKind::SetGraphParam,
            Self::RenameNode { .. } => EditKind::RenameGraphNode,
            Self::SetBypass { .. } => EditKind::BypassGraphNode,
            Self::AddFrame { .. } => EditKind::AddGraphFrame,
            Self::SetFrame { .. } | Self::RemoveFrame { .. } => EditKind::GraphFrame,
        }
    }
}

fn finite_pair(value: [f32; 2]) -> [f32; 2] {
    value.map(|v| if v.is_finite() { v.clamp(-1.0e5, 1.0e5) } else { 0.0 })
}

fn short(text: &str) -> String {
    text.chars().take(MAX_NAME).collect()
}

impl MaterialGraph {
    /// Applies `edit`; returns whether the graph changed. On error the graph is left as it was.
    ///
    /// # Errors
    ///
    /// A [`GraphError`] naming what is wrong with the edit.
    pub fn apply_edit(&mut self, edit: &GraphEdit) -> Result<bool, GraphError> {
        let mut next = self.clone();
        next.apply_in_place(edit)?;
        if next == *self {
            return Ok(false);
        }
        *self = next;
        Ok(true)
    }

    fn require_node(&self, id: GraphNodeId) -> Result<&GraphNode, GraphError> {
        self.node(id).ok_or(GraphError::UnknownNode(id))
    }

    fn apply_in_place(&mut self, edit: &GraphEdit) -> Result<(), GraphError> {
        match edit {
            GraphEdit::AddNode { kind, position } => {
                if self.nodes.len() >= Self::MAX_NODES {
                    return Err(GraphError::TooManyNodes);
                }
                if kind.is_output() && self.nodes.iter().any(|node| node.kind == *kind) {
                    return Err(GraphError::SecondOutput(*kind));
                }
                self.push_node(*kind, finite_pair(*position));
            }
            GraphEdit::RemoveNodes { nodes } => {
                for id in nodes {
                    if self.require_node(*id)?.kind == GraphNodeKind::SurfaceOutput {
                        return Err(GraphError::OutputProtected);
                    }
                }
                self.nodes.retain(|node| !nodes.contains(&node.id));
                self.links
                    .retain(|link| !nodes.contains(&link.from.node) && !nodes.contains(&link.to.node));
            }
            GraphEdit::MoveNodes { nodes, frames, delta } => {
                let delta = finite_pair(*delta);
                for id in nodes {
                    self.require_node(*id)?;
                }
                for id in frames {
                    self.frame(*id).ok_or(GraphError::UnknownFrame(*id))?;
                }
                for node in self.nodes.iter_mut().filter(|node| nodes.contains(&node.id)) {
                    node.position = finite_pair([node.position[0] + delta[0], node.position[1] + delta[1]]);
                }
                for frame in self.frames.iter_mut().filter(|frame| frames.contains(&frame.id)) {
                    frame.position = finite_pair([frame.position[0] + delta[0], frame.position[1] + delta[1]]);
                }
            }
            GraphEdit::Connect { from, to } => {
                let source = self.require_node(from.node)?;
                if usize::from(from.socket) >= source.kind.spec().outputs.len() {
                    return Err(GraphError::UnknownOutput {
                        node: from.node,
                        socket: from.socket,
                    });
                }
                let target = self.require_node(to.node)?;
                if usize::from(to.socket) >= target.kind.spec().inputs.len() {
                    return Err(GraphError::UnknownInput {
                        node: to.node,
                        socket: to.socket,
                    });
                }
                if self.would_cycle(from.node, to.node) {
                    return Err(GraphError::Cycle);
                }
                self.push_link(from.node, from.socket, to.node, to.socket);
            }
            GraphEdit::Disconnect { to } => {
                self.links.retain(|link| link.to != *to);
            }
            GraphEdit::SetInput { node, input, value } => {
                let spec = self.require_node(*node)?.kind.spec();
                let socket = spec.inputs.get(usize::from(*input)).ok_or(GraphError::UnknownInput {
                    node: *node,
                    socket: *input,
                })?;
                if !value.fits(socket.ty) {
                    return Err(GraphError::WrongValue);
                }
                let mut value = value.sanitized();
                if let (InputValue::Float(number), (min, max)) = (&mut value, socket.range)
                    && max > min
                {
                    *number = number.clamp(min, max);
                }
                if let Some(slot) = self
                    .node_mut(*node)
                    .and_then(|found| found.inputs.get_mut(usize::from(*input)))
                {
                    *slot = value;
                }
            }
            GraphEdit::SetParam { node, param, value } => {
                let spec = self.require_node(*node)?.kind.spec();
                let kind = spec.params.get(usize::from(*param)).ok_or(GraphError::UnknownParam {
                    node: *node,
                    param: *param,
                })?;
                let value = kind.kind.sanitize(value).ok_or(GraphError::WrongValue)?;
                if let Some(slot) = self
                    .node_mut(*node)
                    .and_then(|found| found.params.get_mut(usize::from(*param)))
                {
                    *slot = value;
                }
            }
            GraphEdit::RenameNode { node, name } => {
                self.require_node(*node)?;
                if let Some(found) = self.node_mut(*node) {
                    found.name = short(name.trim());
                }
            }
            GraphEdit::SetBypass { node, bypass } => {
                self.require_node(*node)?;
                if let Some(found) = self.node_mut(*node) {
                    found.bypass = *bypass;
                }
            }
            GraphEdit::AddFrame { label, position, size } => {
                if self.frames.len() >= Self::MAX_FRAMES {
                    return Err(GraphError::TooManyFrames);
                }
                let size = finite_pair(*size).map(|v| v.max(MIN_FRAME));
                self.push_frame(&short(label), finite_pair(*position), size);
            }
            GraphEdit::SetFrame {
                frame,
                label,
                position,
                size,
            } => {
                let found: &mut Frame = self
                    .frames
                    .iter_mut()
                    .find(|candidate| candidate.id == *frame)
                    .ok_or(GraphError::UnknownFrame(*frame))?;
                found.label = short(label);
                found.position = finite_pair(*position);
                found.size = finite_pair(*size).map(|v| v.max(MIN_FRAME));
            }
            GraphEdit::RemoveFrame { frame } => {
                self.frame(*frame).ok_or(GraphError::UnknownFrame(*frame))?;
                self.frames.retain(|candidate| candidate.id != *frame);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output(node: GraphNodeId, socket: u8) -> OutputRef {
        OutputRef { node, socket }
    }

    fn input(node: GraphNodeId, socket: u8) -> InputRef {
        InputRef { node, socket }
    }

    fn surface(graph: &MaterialGraph) -> GraphNodeId {
        graph.surface_output().map(|node| node.id).unwrap()
    }

    fn add(graph: &mut MaterialGraph, kind: GraphNodeKind) -> GraphNodeId {
        let id = graph.next_node_id();
        assert!(
            graph
                .apply_edit(&GraphEdit::AddNode {
                    kind,
                    position: [10.0, 20.0],
                })
                .unwrap()
        );
        id
    }

    #[test]
    fn adding_a_node_uses_the_announced_id_and_ids_are_never_reused() {
        let mut graph = MaterialGraph::blank("Test");
        let first = add(&mut graph, GraphNodeKind::Noise);
        graph
            .apply_edit(&GraphEdit::RemoveNodes { nodes: vec![first] })
            .unwrap();
        let second = add(&mut graph, GraphNodeKind::Noise);
        assert_ne!(first, second);
        assert!(graph.node(first).is_none());
        assert_eq!(graph.validate(), Ok(()));
    }

    #[test]
    fn a_second_surface_output_is_rejected_but_one_volume_output_is_fine() {
        let mut graph = MaterialGraph::blank("Test");
        let again = GraphEdit::AddNode {
            kind: GraphNodeKind::SurfaceOutput,
            position: [0.0, 0.0],
        };
        assert_eq!(
            graph.apply_edit(&again),
            Err(GraphError::SecondOutput(GraphNodeKind::SurfaceOutput))
        );
        add(&mut graph, GraphNodeKind::VolumeOutput);
        let volume = GraphEdit::AddNode {
            kind: GraphNodeKind::VolumeOutput,
            position: [0.0, 0.0],
        };
        assert_eq!(
            graph.apply_edit(&volume),
            Err(GraphError::SecondOutput(GraphNodeKind::VolumeOutput))
        );
    }

    #[test]
    fn the_surface_output_cannot_be_removed() {
        let mut graph = MaterialGraph::blank("Test");
        let edit = GraphEdit::RemoveNodes {
            nodes: vec![surface(&graph)],
        };
        assert_eq!(graph.apply_edit(&edit), Err(GraphError::OutputProtected));
    }

    #[test]
    fn connecting_replaces_the_old_link_and_removing_a_node_drops_its_links() {
        let mut graph = MaterialGraph::blank("Test");
        let surface = surface(&graph);
        let noise = add(&mut graph, GraphNodeKind::Noise);
        let connect = GraphEdit::Connect {
            from: output(noise, 0),
            to: input(surface, 0),
        };
        assert!(graph.apply_edit(&connect).unwrap());
        assert_eq!(graph.links().len(), 1, "the voxel color link was replaced");
        assert!(!graph.apply_edit(&connect).unwrap(), "same link again changes nothing");
        graph
            .apply_edit(&GraphEdit::RemoveNodes { nodes: vec![noise] })
            .unwrap();
        assert_eq!(graph.links(), []);
    }

    #[test]
    fn cycles_and_bad_sockets_are_rejected_and_leave_the_graph_alone() {
        let mut graph = MaterialGraph::blank("Test");
        let a = add(&mut graph, GraphNodeKind::Multiply);
        let b = add(&mut graph, GraphNodeKind::Multiply);
        graph
            .apply_edit(&GraphEdit::Connect {
                from: output(a, 0),
                to: input(b, 0),
            })
            .unwrap();
        let before = graph.clone();
        let back = GraphEdit::Connect {
            from: output(b, 0),
            to: input(a, 1),
        };
        assert_eq!(graph.apply_edit(&back), Err(GraphError::Cycle));
        let itself = GraphEdit::Connect {
            from: output(a, 0),
            to: input(a, 0),
        };
        assert_eq!(graph.apply_edit(&itself), Err(GraphError::Cycle));
        let no_such_output = GraphEdit::Connect {
            from: output(a, 5),
            to: input(b, 1),
        };
        assert!(matches!(
            graph.apply_edit(&no_such_output),
            Err(GraphError::UnknownOutput { .. })
        ));
        let from_an_output_node = GraphEdit::Connect {
            from: output(surface(&graph), 0),
            to: input(b, 1),
        };
        assert!(matches!(
            graph.apply_edit(&from_an_output_node),
            Err(GraphError::UnknownOutput { .. })
        ));
        assert_eq!(graph, before);
    }

    #[test]
    fn inputs_and_params_are_type_checked_and_clamped() {
        let mut graph = MaterialGraph::blank("Test");
        let surface = surface(&graph);
        let noise = add(&mut graph, GraphNodeKind::Noise);
        let wrong = GraphEdit::SetInput {
            node: surface,
            input: 1,
            value: InputValue::Color([1.0; 3]),
        };
        assert_eq!(graph.apply_edit(&wrong), Err(GraphError::WrongValue));
        graph
            .apply_edit(&GraphEdit::SetInput {
                node: surface,
                input: 1,
                value: InputValue::Float(7.0),
            })
            .unwrap();
        assert_eq!(graph.surface_constants().metallic.to_bits(), 1.0_f32.to_bits());
        graph
            .apply_edit(&GraphEdit::SetParam {
                node: noise,
                param: 0,
                value: ParamValue::Float(1.0e9),
            })
            .unwrap();
        assert_eq!(graph.node(noise).unwrap().params[0], ParamValue::Float(64.0));
        let bad_param = GraphEdit::SetParam {
            node: noise,
            param: 0,
            value: ParamValue::Bool(true),
        };
        assert_eq!(graph.apply_edit(&bad_param), Err(GraphError::WrongValue));
        let missing = GraphEdit::SetParam {
            node: noise,
            param: 40,
            value: ParamValue::Bool(true),
        };
        assert!(matches!(
            graph.apply_edit(&missing),
            Err(GraphError::UnknownParam { .. })
        ));
    }

    #[test]
    fn moving_renaming_bypassing_and_frames() {
        let mut graph = MaterialGraph::blank("Test");
        let noise = add(&mut graph, GraphNodeKind::Noise);
        graph
            .apply_edit(&GraphEdit::AddFrame {
                label: "Surface variation".to_owned(),
                position: [0.0, 0.0],
                size: [1.0, 1.0],
            })
            .unwrap();
        let frame = graph.frames()[0].id;
        assert!(
            graph.frames()[0]
                .size
                .iter()
                .all(|side| (side - MIN_FRAME).abs() < f32::EPSILON)
        );
        graph
            .apply_edit(&GraphEdit::MoveNodes {
                nodes: vec![noise],
                frames: vec![frame],
                delta: [5.0, -5.0],
            })
            .unwrap();
        assert_eq!(graph.node(noise).unwrap().position, [15.0, 15.0]);
        assert_eq!(graph.frame(frame).unwrap().position, [5.0, -5.0]);
        graph
            .apply_edit(&GraphEdit::RenameNode {
                node: noise,
                name: "  Grain ".to_owned(),
            })
            .unwrap();
        assert_eq!(graph.node(noise).unwrap().display_name(), "Grain");
        graph
            .apply_edit(&GraphEdit::SetBypass {
                node: noise,
                bypass: true,
            })
            .unwrap();
        assert!(graph.node(noise).unwrap().bypass);
        graph.apply_edit(&GraphEdit::RemoveFrame { frame }).unwrap();
        assert_eq!(graph.frames(), []);
        assert_eq!(
            graph.apply_edit(&GraphEdit::RemoveFrame { frame }),
            Err(GraphError::UnknownFrame(frame))
        );
    }

    #[test]
    fn a_graph_never_holds_more_nodes_than_the_limit() {
        let mut graph = MaterialGraph::blank("Test");
        while graph.nodes().len() < MaterialGraph::MAX_NODES {
            add(&mut graph, GraphNodeKind::Value);
        }
        let more = GraphEdit::AddNode {
            kind: GraphNodeKind::Value,
            position: [0.0, 0.0],
        };
        assert_eq!(graph.apply_edit(&more), Err(GraphError::TooManyNodes));
    }
}
