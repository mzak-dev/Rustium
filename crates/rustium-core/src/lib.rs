//! Core data model of the Rustium voxel editor.
//!
//! A [`Scene`] is a tree of [`Node`]s: Groups hold child Nodes and Models place a [`Shape`] (voxel storage split
//! into copy-on-write [`Chunk`]s) with a [`Transform`]. Models that show the same Shape are Instances. The Scene
//! also owns a [`Palette`] of [`Material`]s. Every change goes through a [`Command`]: [`apply`] turns it into an
//! [`Edit`] that the [`History`] can undo and redo. [`pick_scene`] finds what a ray hits and [`mesh_chunk`] turns
//! a chunk into triangles for the rasterizer. This crate has no GPU and no async dependencies.
//!
//! ```
//! use glam::{IVec3, Vec3};
//! use rustium_core::{
//!     Brush, Command, History, MaterialId, Mirror, NodeId, Ray, Scene, VoxelTool, apply,
//!     pick_scene,
//! };
//!
//! let mut scene = Scene::new();
//! let mut history = History::new(200);
//! let model = scene
//!     .node(NodeId::ROOT)
//!     .expect("the root exists")
//!     .children()[0];
//! let ray = Ray {
//!     origin: Vec3::new(0.5, 10.0, 0.5),
//!     direction: Vec3::NEG_Y,
//! };
//! let hit = pick_scene(&scene, ray, 100.0, NodeId::ROOT).expect("the ground is below the camera");
//! let material = MaterialId::new(1).expect("1 is a valid material index");
//! let command = Command::ApplyBrush {
//!     model,
//!     tool: VoxelTool::Attach,
//!     brush: Brush::default(),
//!     hit: hit.local,
//!     end: None,
//!     material,
//!     mirror: Mirror::default(),
//! };
//! if let Some(edit) = apply(&mut scene, &command)? {
//!     history.push(edit);
//! }
//! let shape = scene.model_shape(model).expect("the model has a shape").1;
//! assert_eq!(shape.get(IVec3::ZERO), Some(material));
//! history.undo(&mut scene);
//! let shape = scene.model_shape(model).expect("the model has a shape").1;
//! assert_eq!(shape.get(IVec3::ZERO), None);
//! # Ok::<(), rustium_core::Error>(())
//! ```
#![warn(clippy::panic, clippy::expect_used)]

mod chunk;
mod command;
mod edit;
mod error;
mod face;
mod ids;
mod material_graph;
mod mesh;
mod node;
mod palette;
mod pick;
mod raycast;
mod scene;
mod scene_ops;
mod selection;
mod shape;
mod tools;
mod transform;

pub use chunk::{CHUNK_SIZE, Chunk, ChunkCoord};
pub use command::{COORD_LIMIT, Command, MAX_BATCH, VoxelTool, apply};
pub use edit::{Edit, EditKind, History};
pub use error::Error;
pub use face::Face;
pub use ids::{NodeId, ShapeId};
pub use material_graph::{MaterialGraph, SurfaceConstants};
pub use mesh::{ChunkMesh, MeshVertex, mesh_chunk};
pub use node::{Node, NodeKind};
pub use palette::{Material, MaterialId, Palette};
pub use pick::{SceneHit, pick_scene};
pub use raycast::{HitTarget, Ray, RayHit, pick};
pub use scene::{Placement, Scene};
pub use selection::{Selection, select_box, select_by_material};
pub use shape::{Shape, Volume, Voxel};
pub use tools::{Brush, BrushKind, FILL_LIMIT, FillMode, Mirror, brush_bounds};
pub use transform::{Orientation, Transform};
