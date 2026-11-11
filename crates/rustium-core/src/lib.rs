//! Core data model of the Rustium voxel editor.
//!
//! A [`Scene`] holds one [`Shape`] (voxel storage split into copy-on-write [`Chunk`]s) and a [`Palette`]
//! of [`Material`]s. Every change goes through a [`Command`]: [`apply`] turns it into an [`Edit`] that the
//! [`History`] can undo and redo. [`pick`] finds what a ray hits and [`mesh_chunk`] turns a chunk into
//! triangles for the rasterizer. This crate has no GPU and no async dependencies.
//!
//! ```
//! use glam::{IVec3, Vec3};
//! use rustium_core::{Brush, Command, History, MaterialId, Ray, Scene, VoxelTool, apply, pick};
//!
//! let mut scene = Scene::new();
//! let mut history = History::new(200);
//! let ray = Ray {
//!     origin: Vec3::new(0.5, 10.0, 0.5),
//!     direction: Vec3::NEG_Y,
//! };
//! let hit = pick(scene.shape(), ray, 100.0).expect("the ground is below the camera");
//! let material = MaterialId::new(1).expect("1 is a valid material index");
//! let command = Command::ApplyBrush {
//!     tool: VoxelTool::Attach,
//!     brush: Brush::default(),
//!     hit,
//!     material,
//! };
//! if let Some(edit) = apply(&mut scene, &command)? {
//!     history.push(edit);
//! }
//! assert_eq!(scene.shape().get(IVec3::ZERO), Some(material));
//! history.undo(&mut scene);
//! assert_eq!(scene.shape().get(IVec3::ZERO), None);
//! # Ok::<(), rustium_core::Error>(())
//! ```
#![warn(clippy::panic, clippy::expect_used)]

mod chunk;
mod command;
mod edit;
mod error;
mod face;
mod mesh;
mod palette;
mod raycast;
mod scene;
mod shape;

pub use chunk::{CHUNK_SIZE, Chunk, ChunkCoord};
pub use command::{Brush, Command, VoxelTool, apply, brush_bounds};
pub use edit::{Edit, EditKind, History};
pub use error::Error;
pub use face::Face;
pub use mesh::{ChunkMesh, MeshVertex, mesh_chunk};
pub use palette::{Material, MaterialId, Palette};
pub use raycast::{HitTarget, Ray, RayHit, pick};
pub use scene::Scene;
pub use shape::{Shape, Volume, Voxel};
