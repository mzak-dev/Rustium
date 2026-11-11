/// Identifies a [`Node`](crate::Node) of the Scene; an index into the Scene's node arena that is never reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(u32);

impl NodeId {
    /// The root Group every Scene has.
    pub const ROOT: Self = Self(0);

    /// The raw arena index, for scripts and file formats.
    pub fn get(self) -> u32 {
        self.0
    }

    /// The id with arena index `index`; it only names a Node when the Scene has one there.
    pub fn from_raw(index: u32) -> Self {
        Self(index)
    }

    pub(crate) fn index(self) -> usize {
        // u32 always fits in usize on the supported 32- and 64-bit targets.
        usize::try_from(self.0).unwrap_or(usize::MAX)
    }

    pub(crate) fn from_index(index: usize) -> Option<Self> {
        u32::try_from(index).ok().map(Self)
    }
}

/// Identifies a [`Shape`](crate::Shape) of the Scene; an index into the Scene's shape arena that is never reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ShapeId(u32);

impl ShapeId {
    /// The raw arena index, for scripts and file formats.
    pub fn get(self) -> u32 {
        self.0
    }

    /// The id with arena index `index`; it only names a Shape when the Scene has one there.
    pub fn from_raw(index: u32) -> Self {
        Self(index)
    }

    pub(crate) fn index(self) -> usize {
        usize::try_from(self.0).unwrap_or(usize::MAX)
    }

    pub(crate) fn from_index(index: usize) -> Option<Self> {
        u32::try_from(index).ok().map(Self)
    }
}
