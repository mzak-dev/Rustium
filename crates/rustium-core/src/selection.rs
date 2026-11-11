use std::collections::HashSet;
use std::sync::Arc;

use glam::IVec3;

use crate::palette::MaterialId;
use crate::shape::Shape;

/// A set of Voxels inside the Focus Model that later Tools act on, in the Model's own cell coordinates.
///
/// Cheap to clone: the cells are shared until one of the copies changes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selection {
    cells: Arc<HashSet<IVec3>>,
}

impl Selection {
    /// An empty Selection.
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of selected voxels.
    pub fn len(&self) -> usize {
        self.cells.len()
    }

    /// Whether nothing is selected.
    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    /// Whether `cell` is selected.
    pub fn contains(&self, cell: IVec3) -> bool {
        self.cells.contains(&cell)
    }

    /// The selected cells, in no particular order.
    pub fn iter(&self) -> impl Iterator<Item = IVec3> + '_ {
        self.cells.iter().copied()
    }

    /// The Selection with every cell of `other` added.
    #[must_use]
    pub fn union(&self, other: &Self) -> Self {
        let mut cells = (*self.cells).clone();
        cells.extend(other.cells.iter().copied());
        Self { cells: Arc::new(cells) }
    }

    /// The same cells moved by `offset`.
    #[must_use]
    pub fn translated(&self, offset: IVec3) -> Self {
        Self {
            cells: Arc::new(self.cells.iter().map(|cell| *cell + offset).collect()),
        }
    }

    /// The box around the selected cells.
    pub fn bounds(&self) -> Option<crate::Volume> {
        let mut cells = self.cells.iter();
        let first = cells.next()?;
        Some(cells.fold(crate::Volume::point(*first), |volume, cell| volume.including(*cell)))
    }
}

impl FromIterator<IVec3> for Selection {
    fn from_iter<I: IntoIterator<Item = IVec3>>(iter: I) -> Self {
        Self {
            cells: Arc::new(iter.into_iter().collect()),
        }
    }
}

/// Every filled voxel of `shape` inside the box spanned by the cells `a` and `b` (corners in any order).
pub fn select_box(shape: &Shape, a: IVec3, b: IVec3) -> Selection {
    let (min, max) = (a.min(b), a.max(b));
    let inside = |cell: IVec3| cell.cmpge(min).all() && cell.cmple(max).all();
    shape
        .voxels()
        .map(|(cell, _)| cell)
        .filter(|cell| inside(*cell))
        .collect()
}

/// Every voxel of `shape` that shows `material`.
pub fn select_by_material(shape: &Shape, material: MaterialId) -> Selection {
    shape
        .voxels()
        .filter(|(_, voxel)| *voxel == material)
        .map(|(cell, _)| cell)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn box_selection_takes_only_filled_voxels_inside() {
        let mut shape = Shape::new();
        let material = MaterialId::new(1);
        shape.set(IVec3::new(1, 1, 1), material);
        shape.set(IVec3::new(5, 5, 5), material);
        let selection = select_box(&shape, IVec3::new(3, 3, 3), IVec3::ZERO);
        assert_eq!(selection.len(), 1);
        assert!(selection.contains(IVec3::ONE));
    }

    #[test]
    fn color_selection_matches_the_material() {
        let mut shape = Shape::new();
        shape.set(IVec3::ZERO, MaterialId::new(1));
        shape.set(IVec3::X, MaterialId::new(2));
        let selection = select_by_material(&shape, MaterialId::FIRST);
        assert_eq!(selection.len(), 1);
        assert!(selection.contains(IVec3::ZERO));
    }
}
