use glam::Vec3;

use crate::ids::NodeId;
use crate::raycast::{HitTarget, Ray, RayHit, raycast_ground, raycast_voxels};
use crate::scene::Scene;

/// Where a ray hit the Scene: the Model it hit, and the hit in both the Model's and the Scene's cell space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SceneHit {
    /// The Model that was hit; `None` when the ray hit the ground.
    pub node: Option<NodeId>,
    /// The hit in the cell coordinates of `node`'s Shape (equal to `world` for the ground).
    pub local: RayHit,
    /// The hit in Scene cell coordinates.
    pub world: RayHit,
}

/// Two hits closer than this are a tie, which the Model higher in the tree wins.
const TIE_DISTANCE: f32 = 1e-3;

/// Finds the first Voxel a ray hits among the visible, unlocked Models at or below `within`, falling back to the
/// ground plane y = 0.
///
/// Where Models overlap, the one higher in the tree (earlier in drawing order) wins.
pub fn pick_scene(scene: &Scene, ray: Ray, max_distance: f32, within: NodeId) -> Option<SceneHit> {
    let direction = ray.direction.normalize_or_zero();
    if direction == Vec3::ZERO {
        return None;
    }
    let ray = Ray {
        origin: ray.origin,
        direction,
    };
    let mut best: Option<(SceneHit, f32)> = None;
    for placement in scene.placements() {
        if !scene.is_ancestor_or_self(within, placement.node) || scene.is_locked(placement.node) {
            continue;
        }
        let Some(shape) = scene.shape(placement.shape) else {
            continue;
        };
        let inverse = placement.transform.inverse();
        // Rigid transforms keep the ray parameter, so distances stay comparable between Models.
        let local_ray = Ray {
            origin: inverse.point(ray.origin),
            direction: inverse.orientation.apply_float(ray.direction),
        };
        let Some((local, distance)) = raycast_voxels(shape, local_ray, max_distance) else {
            continue;
        };
        if best.is_none_or(|(_, nearest)| distance < nearest - TIE_DISTANCE) {
            best = Some((
                SceneHit {
                    node: Some(placement.node),
                    local,
                    world: placement.transform.hit(local),
                },
                distance,
            ));
        }
    }
    if let Some((ground, distance)) = raycast_ground(ray, max_distance)
        && best.is_none_or(|(_, nearest)| distance < nearest)
    {
        best = Some((
            SceneHit {
                node: None,
                local: ground,
                world: ground,
            },
            distance,
        ));
    }
    best.map(|(hit, _)| hit)
}

impl SceneHit {
    /// Whether the ray hit a Voxel rather than the ground.
    pub fn is_voxel(&self) -> bool {
        self.world.target == HitTarget::Voxel
    }
}

#[cfg(test)]
mod tests {
    use glam::IVec3;

    use super::*;
    use crate::command::{Command, VoxelTool, apply};
    use crate::face::Face;
    use crate::palette::ColorId;
    use crate::tools::{Brush, Mirror};
    use crate::transform::{Orientation, Transform};

    fn put(scene: &mut Scene, model: NodeId, at: IVec3, color: u8) {
        let command = Command::ApplyBrush {
            model,
            tool: VoxelTool::Attach,
            brush: Brush::default(),
            hit: RayHit {
                voxel: at - IVec3::Y,
                face: Face::PosY,
                target: HitTarget::Ground,
            },
            end: None,
            color: ColorId::new(color).unwrap(),
            mirror: Mirror::default(),
        };
        apply(scene, &command).unwrap();
    }

    fn down_ray(x: f32, z: f32) -> Ray {
        Ray {
            origin: Vec3::new(x, 20.0, z),
            direction: Vec3::NEG_Y,
        }
    }

    #[test]
    fn a_moved_model_is_hit_at_its_new_place_with_a_local_hit() {
        let mut scene = Scene::new();
        let model = scene.node(NodeId::ROOT).unwrap().children()[0];
        put(&mut scene, model, IVec3::ZERO, 1);
        let moved = Transform {
            orientation: Orientation::IDENTITY,
            offset: IVec3::new(10, 0, 0),
        };
        apply(
            &mut scene,
            &Command::SetTransform {
                node: model,
                transform: moved,
            },
        )
        .unwrap();
        let hit = pick_scene(&scene, down_ray(10.5, 0.5), 100.0, NodeId::ROOT).unwrap();
        assert_eq!(hit.node, Some(model));
        assert_eq!(hit.world.voxel, IVec3::new(10, 0, 0));
        assert_eq!(hit.local.voxel, IVec3::ZERO);
    }

    #[test]
    fn the_model_higher_in_the_tree_wins_where_models_overlap() {
        let mut scene = Scene::new();
        let lower = scene.node(NodeId::ROOT).unwrap().children()[0];
        put(&mut scene, lower, IVec3::ZERO, 1);
        let edit = apply(
            &mut scene,
            &Command::NewModel {
                parent: NodeId::ROOT,
                name: "Upper".to_owned(),
            },
        )
        .unwrap()
        .unwrap();
        let upper = edit.created_nodes()[0];
        put(&mut scene, upper, IVec3::ZERO, 2);
        let hit = pick_scene(&scene, down_ray(0.5, 0.5), 100.0, NodeId::ROOT).unwrap();
        assert_eq!(hit.node, Some(upper));
    }

    #[test]
    fn hidden_and_locked_models_are_not_picked() {
        let mut scene = Scene::new();
        let model = scene.node(NodeId::ROOT).unwrap().children()[0];
        put(&mut scene, model, IVec3::ZERO, 1);
        apply(
            &mut scene,
            &Command::SetLocked {
                node: model,
                locked: true,
            },
        )
        .unwrap();
        let hit = pick_scene(&scene, down_ray(0.5, 0.5), 100.0, NodeId::ROOT).unwrap();
        assert_eq!(hit.node, None);
        apply(
            &mut scene,
            &Command::SetLocked {
                node: model,
                locked: false,
            },
        )
        .unwrap();
        apply(
            &mut scene,
            &Command::SetHidden {
                node: model,
                hidden: true,
            },
        )
        .unwrap();
        let hit = pick_scene(&scene, down_ray(0.5, 0.5), 100.0, NodeId::ROOT).unwrap();
        assert_eq!(hit.node, None);
    }

    #[test]
    fn rotated_models_report_hits_in_their_own_cells() {
        let mut scene = Scene::new();
        let model = scene.node(NodeId::ROOT).unwrap().children()[0];
        put(&mut scene, model, IVec3::new(2, 0, 0), 1);
        let turned = Transform {
            orientation: Orientation::rotation(1, 1),
            offset: IVec3::ZERO,
        };
        apply(
            &mut scene,
            &Command::SetTransform {
                node: model,
                transform: turned,
            },
        )
        .unwrap();
        // A quarter turn about Y sends cell (2, 0, 0) to (0, 0, -2).
        let world_cell = turned.cell(IVec3::new(2, 0, 0));
        let hit = pick_scene(
            &scene,
            down_ray(world_cell.x as f32 + 0.5, world_cell.z as f32 + 0.5),
            100.0,
            NodeId::ROOT,
        )
        .unwrap();
        assert_eq!(hit.world.voxel, world_cell);
        assert_eq!(hit.local.voxel, IVec3::new(2, 0, 0));
        assert_eq!(hit.local.face, Face::PosY);
    }
}
