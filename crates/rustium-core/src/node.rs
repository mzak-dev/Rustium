use crate::ids::{NodeId, ShapeId};
use crate::transform::Transform;

/// What a [`Node`] is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeKind {
    /// Holds child Nodes and moves them together; the children are drawn in order, first on top.
    Group {
        /// Child Nodes, topmost first.
        children: Vec<NodeId>,
    },
    /// Places a Shape in the Scene.
    Model {
        /// The Shape shown; other Models may show the same one (Instances).
        shape: ShapeId,
    },
}

/// One element of the Scene tree: a Group or a Model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub(crate) name: String,
    pub(crate) parent: Option<NodeId>,
    pub(crate) transform: Transform,
    pub(crate) hidden: bool,
    pub(crate) locked: bool,
    pub(crate) kind: NodeKind,
}

impl Node {
    /// Name shown in the Scene tree.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The parent Group; `None` for the root and for deleted Nodes.
    pub fn parent(&self) -> Option<NodeId> {
        self.parent
    }

    /// Where the Node sits relative to its parent.
    pub fn transform(&self) -> Transform {
        self.transform
    }

    /// Whether the Node itself is hidden (its ancestors may be too, see [`Scene::is_hidden`](crate::Scene::is_hidden)).
    pub fn is_hidden(&self) -> bool {
        self.hidden
    }

    /// Whether the Node itself is locked (its ancestors may be too, see [`Scene::is_locked`](crate::Scene::is_locked)).
    pub fn is_locked(&self) -> bool {
        self.locked
    }

    /// Group or Model.
    pub fn kind(&self) -> &NodeKind {
        &self.kind
    }

    /// Child Nodes, topmost first; empty for a Model.
    pub fn children(&self) -> &[NodeId] {
        match &self.kind {
            NodeKind::Group { children } => children,
            NodeKind::Model { .. } => &[],
        }
    }

    /// The Shape of a Model; `None` for a Group.
    pub fn shape(&self) -> Option<ShapeId> {
        match self.kind {
            NodeKind::Model { shape } => Some(shape),
            NodeKind::Group { .. } => None,
        }
    }

    /// Whether the Node is a Group.
    pub fn is_group(&self) -> bool {
        matches!(self.kind, NodeKind::Group { .. })
    }
}

/// The arena of Nodes. Deleted Nodes stay in it, detached from the tree, so an Edit can bring them back.
#[derive(Debug, Clone)]
pub(crate) struct Graph {
    nodes: Vec<Node>,
}

impl Graph {
    /// A graph holding only the root Group.
    pub(crate) fn new(root_name: &str) -> Self {
        Self {
            nodes: vec![Node {
                name: root_name.to_owned(),
                parent: None,
                transform: Transform::IDENTITY,
                hidden: false,
                locked: false,
                kind: NodeKind::Group { children: Vec::new() },
            }],
        }
    }

    /// Number of Nodes in the arena, deleted ones included.
    /// A graph of the given arena; the caller guarantees ids and parents are consistent.
    pub(crate) fn from_nodes(nodes: Vec<Node>) -> Self {
        Self { nodes }
    }

    pub(crate) fn len(&self) -> usize {
        self.nodes.len()
    }

    pub(crate) fn get(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(id.index())
    }

    pub(crate) fn get_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        self.nodes.get_mut(id.index())
    }

    /// Adds a detached Node to the arena.
    pub(crate) fn push(&mut self, node: Node) -> Option<NodeId> {
        let id = NodeId::from_index(self.nodes.len())?;
        self.nodes.push(node);
        Some(id)
    }

    /// Whether `id` is reachable from the root.
    pub(crate) fn is_attached(&self, id: NodeId) -> bool {
        let mut current = id;
        // The depth bound protects against a corrupted parent loop.
        for _ in 0..=self.nodes.len() {
            if current == NodeId::ROOT {
                return true;
            }
            match self.get(current).and_then(|node| node.parent) {
                Some(parent) => current = parent,
                None => return false,
            }
        }
        false
    }

    /// Whether `ancestor` is `node` or one of its ancestors.
    pub(crate) fn is_ancestor_or_self(&self, ancestor: NodeId, node: NodeId) -> bool {
        let mut current = Some(node);
        for _ in 0..=self.nodes.len() {
            match current {
                Some(id) if id == ancestor => return true,
                Some(id) => current = self.get(id).and_then(|node| node.parent),
                None => return false,
            }
        }
        false
    }

    /// Transform from the local space of `id` to the Scene.
    pub(crate) fn world_transform(&self, id: NodeId) -> Transform {
        let mut result = Transform::IDENTITY;
        let mut current = Some(id);
        for _ in 0..=self.nodes.len() {
            let Some(node) = current.and_then(|id| self.get(id)) else {
                break;
            };
            result = node.transform.compose(result);
            current = node.parent;
        }
        result
    }

    /// Inserts `child` into the Group `parent` at `index` (clamped) and points `child` at it.
    pub(crate) fn attach(&mut self, parent: NodeId, index: usize, child: NodeId) {
        let inserted = match self.get_mut(parent).map(|node| &mut node.kind) {
            Some(NodeKind::Group { children }) => {
                children.insert(index.min(children.len()), child);
                true
            }
            _ => false,
        };
        debug_assert!(inserted, "attach target {parent:?} is not a Group");
        if inserted && let Some(node) = self.get_mut(child) {
            node.parent = Some(parent);
        }
    }

    /// Removes `id` from its parent's children and clears its parent link; returns where it was.
    pub(crate) fn detach(&mut self, id: NodeId) -> Option<(NodeId, usize)> {
        let parent = self.get(id)?.parent?;
        let mut index = None;
        if let Some(NodeKind::Group { children }) = self.get_mut(parent).map(|node| &mut node.kind)
            && let Some(position) = children.iter().position(|child| *child == id)
        {
            children.remove(position);
            index = Some(position);
        }
        if let Some(node) = self.get_mut(id) {
            node.parent = None;
        }
        index.map(|index| (parent, index))
    }

    /// `root` and everything below it in drawing order (parents first, topmost child first).
    pub(crate) fn descendants(&self, root: NodeId) -> Vec<NodeId> {
        let mut result = Vec::new();
        let mut stack = vec![root];
        while let Some(id) = stack.pop() {
            result.push(id);
            if let Some(node) = self.get(id) {
                stack.extend(node.children().iter().rev().copied());
            }
        }
        result
    }
}
