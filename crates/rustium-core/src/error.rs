use crate::ids::NodeId;

/// Why a [`Command`](crate::Command) could not be applied.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The Command names a Material the Palette does not have.
    #[error("material {0} is not in the palette")]
    UnknownMaterial(u8),
    /// The brush is empty or larger than the editor allows.
    #[error("brush size {size} is outside 1..={max}")]
    BrushSize {
        /// Requested size in voxels.
        size: u32,
        /// Largest allowed size in voxels.
        max: u32,
    },
    /// The Command names a Node that does not exist or was deleted.
    #[error("node {} is not in the scene", .0.get())]
    UnknownNode(NodeId),
    /// The Command needs a Model but the Node is a Group.
    #[error("node {} is not a model", .0.get())]
    NotAModel(NodeId),
    /// The Command needs a Group but the Node is a Model.
    #[error("node {} is not a group", .0.get())]
    NotAGroup(NodeId),
    /// A voxel Tool was aimed at a Model that is hidden or locked.
    #[error("model {} is hidden or locked", .0.get())]
    ModelUnavailable(NodeId),
    /// The Command cannot be applied to the root Group.
    #[error("the root group cannot be changed this way")]
    RootProtected,
    /// A Node cannot be moved into itself or into one of its own descendants.
    #[error("node {} cannot be moved into its own subtree", .0.get())]
    MoveIntoSelf(NodeId),
    /// A 3D Fill would visit more cells than allowed.
    #[error("fill region is larger than {limit} cells")]
    FillTooLarge {
        /// Largest allowed region.
        limit: usize,
    },
    /// The Scene already holds [`MaterialGraph::MAX_PER_SCENE`](crate::MaterialGraph::MAX_PER_SCENE) material graphs.
    #[error("the scene already has {0} material graphs")]
    TooManyGraphs(usize),
    /// The Command names a material graph the Scene does not have.
    #[error("material graph {0} does not exist")]
    UnknownGraph(u32),
    /// The built-in Default material (graph 0) cannot be edited.
    #[error("the default material cannot be edited")]
    DefaultMaterialLocked,
    /// The Scene ran out of Node or Shape ids.
    #[error("the scene has too many nodes")]
    TooManyNodes,
}
