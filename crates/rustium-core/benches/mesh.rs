//! Chunk meshing benchmarks.

use glam::IVec3;
use rustium_core::{
    Brush, ChunkCoord, ChunkMesh, ColorId, Command, Face, HitTarget, Mirror, NodeId, RayHit, Scene, Shape,
    Transparency, VoxelTool,
};

fn main() {
    divan::main();
}

fn model_of(scene: &Scene) -> NodeId {
    scene.node(NodeId::ROOT).expect("the root exists").children()[0]
}

fn shape_of(scene: &Scene) -> &Shape {
    scene.model_shape(model_of(scene)).expect("the model has a shape").1
}

fn filled_scene(size: u32) -> Scene {
    let mut scene = Scene::new();
    let hit = RayHit {
        voxel: IVec3::new(15, -1, 15),
        face: Face::PosY,
        target: HitTarget::Ground,
    };
    let color = ColorId::new(20).expect("20 is a valid color index");
    let command = Command::ApplyBrush {
        model: model_of(&scene),
        tool: VoxelTool::Attach,
        brush: Brush {
            size,
            ..Brush::default()
        },
        hit,
        end: None,
        color,
        mirror: Mirror::default(),
    };
    rustium_core::apply(&mut scene, &command).expect("default palette has color 20");
    scene
}

#[divan::bench]
fn solid_cube_32(bencher: divan::Bencher<'_, '_>) {
    let scene = filled_scene(32);
    let mut mesh = ChunkMesh::default();
    bencher.bench_local(|| {
        rustium_core::mesh_chunk(
            shape_of(&scene),
            ChunkCoord(IVec3::ZERO),
            &Transparency::OPAQUE,
            &mut mesh,
        );
    });
}

#[divan::bench]
fn checkerboard_cubes(bencher: divan::Bencher<'_, '_>) {
    let mut scene = filled_scene(1);
    for x in (0..32).step_by(2) {
        for z in (0..32).step_by(2) {
            let hit = RayHit {
                voxel: IVec3::new(x, -1, z),
                face: Face::PosY,
                target: HitTarget::Ground,
            };
            let color = ColorId::new(u8::try_from(1 + (x + z) % 90).unwrap_or(1)).expect("non-zero");
            let command = Command::ApplyBrush {
                model: model_of(&scene),
                tool: VoxelTool::Attach,
                brush: Brush::default(),
                hit,
                end: None,
                color,
                mirror: Mirror::default(),
            };
            rustium_core::apply(&mut scene, &command).expect("valid command");
        }
    }
    let mut mesh = ChunkMesh::default();
    bencher.bench_local(|| {
        rustium_core::mesh_chunk(
            shape_of(&scene),
            ChunkCoord(IVec3::ZERO),
            &Transparency::OPAQUE,
            &mut mesh,
        );
    });
}
