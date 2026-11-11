//! The native `.ruv` file: a magic and a version, then an lz4-compressed postcard encoding of the Scene.
//!
//! Only what is attached to the root is written: Node and Shape arenas grow forever in memory (ADR 0011), so saving
//! compacts them and renumbers the ids in tree order. Voxels are stored chunk by chunk, one byte per voxel. The
//! material graphs go through their own serde derives, so the model of a graph can change without touching this
//! module; the format version covers any such change.

use std::collections::HashMap;
use std::sync::Arc;

use glam::IVec3;
use serde::{Deserialize, Serialize};

use crate::chunk::{CHUNK_VOLUME, Chunk, ChunkCoord};
use crate::file_error::FileError;
use crate::ids::{NodeId, ShapeId};
use crate::material_graph::MaterialGraph;
use crate::node::{Graph, Node, NodeKind};
use crate::palette::{Color, Palette};
use crate::scene::Scene;
use crate::shape::Shape;
use crate::transform::{Orientation, Transform};

/// First bytes of every `.ruv` file.
pub const MAGIC: [u8; 8] = *b"RUSTIUMV";

/// The newest format version this editor reads and the one it writes.
pub const VERSION: u32 = 1;

/// Largest body (uncompressed) accepted, so a damaged length cannot ask for gigabytes.
const MAX_BODY: usize = 1 << 30;

/// lz4 never expands data by more than this factor, which bounds a believable length field.
const LZ4_MAX_RATIO: usize = 255;

#[derive(Serialize, Deserialize)]
struct SceneDto {
    voxel_size: f32,
    nodes: Vec<NodeDto>,
    shapes: Vec<ShapeDto>,
    palette: Vec<ColorDto>,
    materials: Vec<MaterialGraph>,
}

#[derive(Serialize, Deserialize)]
struct NodeDto {
    name: String,
    columns: [[i32; 3]; 3],
    offset: [i32; 3],
    hidden: bool,
    locked: bool,
    kind: KindDto,
}

#[derive(Serialize, Deserialize)]
enum KindDto {
    Group(Vec<u32>),
    Model(u32),
}

#[derive(Serialize, Deserialize)]
struct ShapeDto {
    chunks: Vec<ChunkDto>,
}

#[derive(Serialize, Deserialize)]
struct ChunkDto {
    coord: [i32; 3],
    voxels: Vec<u8>,
}

#[derive(Serialize, Deserialize)]
struct ColorDto {
    rgb: [u8; 3],
    graph: u32,
}

/// Encodes `scene` as the bytes of a `.ruv` file.
///
/// # Errors
/// Fails only when the Scene is too large for 32-bit ids or the serializer rejects it.
pub fn encode_ruv(scene: &Scene) -> Result<Vec<u8>, FileError> {
    let dto = to_dto(scene)?;
    let body = postcard::to_allocvec(&dto).map_err(|error| FileError::TooLarge(error.to_string()))?;
    let packed = lz4_flex::block::compress_prepend_size(&body);
    let mut bytes = Vec::with_capacity(MAGIC.len() + 4 + packed.len());
    bytes.extend_from_slice(&MAGIC);
    bytes.extend_from_slice(&VERSION.to_le_bytes());
    bytes.extend_from_slice(&packed);
    Ok(bytes)
}

/// Reads a Scene from the bytes of a `.ruv` file.
///
/// # Errors
/// Reports a wrong signature, a newer version, truncation, or data that does not form a valid Scene. Never panics on
/// damaged input.
pub fn decode_ruv(bytes: &[u8]) -> Result<Scene, FileError> {
    let Some((magic, rest)) = bytes.split_first_chunk::<{ MAGIC.len() }>() else {
        return Err(if MAGIC.starts_with(bytes) {
            FileError::Truncated
        } else {
            FileError::BadMagic
        });
    };
    if *magic != MAGIC {
        return Err(FileError::BadMagic);
    }
    let Some((version, packed)) = rest.split_first_chunk::<4>() else {
        return Err(FileError::Truncated);
    };
    let version = u32::from_le_bytes(*version);
    if version == 0 || version > VERSION {
        return Err(FileError::UnsupportedVersion {
            found: version,
            supported: VERSION,
        });
    }
    let Some((size, compressed)) = packed.split_first_chunk::<4>() else {
        return Err(FileError::Truncated);
    };
    let size = usize::try_from(u32::from_le_bytes(*size)).unwrap_or(usize::MAX);
    if size > MAX_BODY || size > compressed.len().saturating_mul(LZ4_MAX_RATIO) {
        return Err(FileError::Corrupt("the stored size is implausible".to_owned()));
    }
    let body = lz4_flex::block::decompress(compressed, size).map_err(|error| FileError::Corrupt(error.to_string()))?;
    let dto: SceneDto = postcard::from_bytes(&body).map_err(|error| match error {
        postcard::Error::DeserializeUnexpectedEnd => FileError::Truncated,
        other => FileError::Corrupt(other.to_string()),
    })?;
    from_dto(dto)
}

fn to_dto(scene: &Scene) -> Result<SceneDto, FileError> {
    let too_large = || FileError::TooLarge("more than four billion nodes".to_owned());
    // Tree order; the root becomes id 0.
    let order = scene.descendants(NodeId::ROOT);
    let mut node_ids = HashMap::new();
    let mut shape_ids = HashMap::<ShapeId, u32>::new();
    let mut shape_order = Vec::new();
    for (new, old) in order.iter().enumerate() {
        node_ids.insert(*old, u32::try_from(new).map_err(|_| too_large())?);
        if let Some(shape) = scene.node(*old).and_then(Node::shape)
            && !shape_ids.contains_key(&shape)
        {
            shape_ids.insert(shape, u32::try_from(shape_order.len()).map_err(|_| too_large())?);
            shape_order.push(shape);
        }
    }
    let mut nodes = Vec::with_capacity(order.len());
    for old in &order {
        let Some(node) = scene.node(*old) else {
            continue;
        };
        let transform = node.transform();
        let kind = match node.kind() {
            NodeKind::Group { children } => KindDto::Group(
                children
                    .iter()
                    .filter_map(|child| node_ids.get(child).copied())
                    .collect(),
            ),
            NodeKind::Model { shape } => KindDto::Model(shape_ids.get(shape).copied().unwrap_or(0)),
        };
        nodes.push(NodeDto {
            name: node.name().to_owned(),
            columns: transform.orientation.columns().map(|column| column.to_array()),
            offset: transform.offset.to_array(),
            hidden: node.is_hidden(),
            locked: node.is_locked(),
            kind,
        });
    }
    let shapes = shape_order
        .iter()
        .map(|id| {
            let mut chunks: Vec<ChunkDto> = scene
                .shape(*id)
                .map(|shape| {
                    shape
                        .chunks()
                        .map(|(coord, chunk)| ChunkDto {
                            coord: coord.0.to_array(),
                            voxels: chunk.to_raw(),
                        })
                        .collect()
                })
                .unwrap_or_default();
            // HashMap order is random; sorting makes saving the same Scene twice byte-identical.
            chunks.sort_by_key(|chunk| chunk.coord);
            ShapeDto { chunks }
        })
        .collect();
    Ok(SceneDto {
        voxel_size: scene.voxel_size(),
        nodes,
        shapes,
        palette: scene
            .palette()
            .iter()
            .map(|(_, color)| ColorDto {
                rgb: color.color,
                graph: color.graph,
            })
            .collect(),
        materials: scene.material_graphs().to_vec(),
    })
}

fn corrupt<T>(reason: &str) -> Result<T, FileError> {
    Err(FileError::Corrupt(reason.to_owned()))
}

fn from_dto(dto: SceneDto) -> Result<Scene, FileError> {
    if !(dto.voxel_size.is_finite() && dto.voxel_size > 0.0) {
        return corrupt("the voxel size is not a positive number");
    }
    if dto.materials.is_empty() || dto.materials.len() > MaterialGraph::MAX_PER_SCENE {
        return corrupt("the number of material graphs is out of range");
    }
    let palette_len = dto.palette.len();
    let mut colors = Vec::with_capacity(palette_len);
    for color in &dto.palette {
        if usize::try_from(color.graph).map_or(true, |graph| graph >= dto.materials.len()) {
            return corrupt("a color points at a material graph that does not exist");
        }
        colors.push(Color {
            color: color.rgb,
            graph: color.graph,
        });
    }
    let Some(palette) = Palette::from_colors(colors) else {
        return corrupt("the palette size is out of range");
    };

    let mut shapes = Vec::with_capacity(dto.shapes.len());
    for shape in dto.shapes {
        let mut chunks = Vec::with_capacity(shape.chunks.len());
        for chunk in shape.chunks {
            if chunk.voxels.len() != CHUNK_VOLUME {
                return corrupt("a chunk has the wrong size");
            }
            if chunk.voxels.iter().any(|byte| usize::from(*byte) > palette_len) {
                return corrupt("a voxel uses a color the palette does not have");
            }
            let Some(parsed) = Chunk::from_raw(&chunk.voxels) else {
                return corrupt("a chunk cannot be rebuilt");
            };
            chunks.push((ChunkCoord(IVec3::from_array(chunk.coord)), Arc::new(parsed)));
        }
        shapes.push(Shape::from_chunks(chunks));
    }

    let node_count = dto.nodes.len();
    let mut nodes = Vec::with_capacity(node_count);
    let mut parents: Vec<Option<NodeId>> = vec![None; node_count];
    for (index, node) in dto.nodes.iter().enumerate() {
        let Some(orientation) = Orientation::from_columns(node.columns.map(IVec3::from_array)) else {
            return corrupt("a node has an invalid orientation");
        };
        let id = NodeId::from_raw(u32::try_from(index).unwrap_or(u32::MAX));
        let kind = match &node.kind {
            KindDto::Group(children) => {
                let mut ids = Vec::with_capacity(children.len());
                for child in children {
                    let slot = usize::try_from(*child)
                        .ok()
                        .filter(|slot| *slot != 0 && *slot < node_count);
                    let Some(slot) = slot else {
                        return corrupt("a group lists a node that does not exist");
                    };
                    if parents.get_mut(slot).and_then(|parent| parent.replace(id)).is_some() {
                        return corrupt("a node has two parents");
                    }
                    ids.push(NodeId::from_raw(*child));
                }
                NodeKind::Group { children: ids }
            }
            KindDto::Model(shape) => {
                if usize::try_from(*shape).map_or(true, |shape| shape >= shapes.len()) {
                    return corrupt("a model uses a shape that does not exist");
                }
                NodeKind::Model {
                    shape: ShapeId::from_raw(*shape),
                }
            }
        };
        nodes.push(Node {
            name: node.name.clone(),
            parent: None,
            transform: Transform {
                orientation,
                offset: IVec3::from_array(node.offset),
            },
            hidden: node.hidden,
            locked: node.locked,
            kind,
        });
    }
    if !nodes.first().is_some_and(Node::is_group) {
        return corrupt("the scene has no root group");
    }
    if parents.iter().skip(1).any(Option::is_none) {
        return corrupt("a node is not attached to the scene");
    }
    for (node, parent) in nodes.iter_mut().zip(parents) {
        node.parent = parent;
    }
    let graph = Graph::from_nodes(nodes);
    // One parent each and all reachable from the root means a tree; a loop would leave nodes unreachable.
    if graph.descendants(NodeId::ROOT).len() != node_count {
        return corrupt("the nodes do not form a tree");
    }
    Ok(Scene::from_parts(graph, shapes, palette, dto.materials, dto.voxel_size))
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::command::{Command, apply};
    use crate::palette::ColorId;

    type Description = Vec<(String, Transform, bool, bool, Vec<([i32; 3], u8)>)>;

    fn sample() -> Scene {
        let mut scene = Scene::with_voxel_size(0.125);
        let model = scene.node(NodeId::ROOT).map(|root| root.children()[0]).unwrap();
        let color = ColorId::new(7);
        let shape = scene.node(model).and_then(Node::shape).unwrap();
        let target = scene.shape_mut(shape).unwrap();
        for x in -40..40 {
            target.set(IVec3::new(x, x / 2, 3), color);
        }
        scene
    }

    fn describe(scene: &Scene) -> Description {
        scene
            .descendants(NodeId::ROOT)
            .into_iter()
            .filter_map(|id| scene.node(id))
            .map(|node| {
                let mut voxels: Vec<([i32; 3], u8)> = node
                    .shape()
                    .and_then(|shape| scene.shape(shape))
                    .map(|shape| shape.voxels().map(|(p, c)| (p.to_array(), c.get())).collect())
                    .unwrap_or_default();
                voxels.sort_unstable();
                (
                    node.name().to_owned(),
                    node.transform(),
                    node.is_hidden(),
                    node.is_locked(),
                    voxels,
                )
            })
            .collect()
    }

    fn same(a: &Scene, b: &Scene) -> bool {
        describe(a) == describe(b)
            && a.palette() == b.palette()
            && a.material_graphs() == b.material_graphs()
            && (a.voxel_size() - b.voxel_size()).abs() < f32::EPSILON
    }

    #[test]
    fn a_scene_round_trips_through_bytes() {
        let mut scene = sample();
        let model = scene.node(NodeId::ROOT).unwrap().children()[0];
        apply(
            &mut scene,
            &Command::Rename {
                node: model,
                name: "Hull".to_owned(),
            },
        )
        .unwrap();
        let bytes = encode_ruv(&scene).unwrap();
        assert_eq!(&bytes[..8], &MAGIC);
        let loaded = decode_ruv(&bytes).unwrap();
        assert!(same(&scene, &loaded));
        assert_eq!(encode_ruv(&loaded).unwrap(), bytes, "saving is deterministic");
    }

    #[test]
    fn saving_drops_detached_nodes_and_unused_shapes() {
        let mut scene = sample();
        let model = scene.node(NodeId::ROOT).unwrap().children()[0];
        apply(
            &mut scene,
            &Command::NewModel {
                parent: NodeId::ROOT,
                name: "Other".to_owned(),
            },
        )
        .unwrap();
        apply(&mut scene, &Command::Delete { node: model }).unwrap();
        let loaded = decode_ruv(&encode_ruv(&scene).unwrap()).unwrap();
        assert_eq!(loaded.descendants(NodeId::ROOT).len(), 2);
        assert_eq!(loaded.used_voxels(NodeId::ROOT), 0);
        assert!(
            loaded.shape(ShapeId::from_raw(1)).is_none(),
            "the deleted model's shape is gone"
        );
    }

    #[test]
    fn instances_keep_sharing_a_shape() {
        let mut scene = sample();
        let model = scene.node(NodeId::ROOT).unwrap().children()[0];
        apply(&mut scene, &Command::DuplicateAsInstance { node: model }).unwrap();
        let loaded = decode_ruv(&encode_ruv(&scene).unwrap()).unwrap();
        assert_eq!(loaded.instance_counts().values().copied().collect::<Vec<_>>(), vec![2]);
    }

    #[test]
    fn a_wrong_magic_is_reported() {
        assert!(matches!(
            decode_ruv(b"NOTAFILE-and-more-bytes"),
            Err(FileError::BadMagic)
        ));
        assert!(matches!(decode_ruv(b""), Err(FileError::Truncated)));
    }

    #[test]
    fn a_newer_version_is_reported() {
        let mut bytes = encode_ruv(&Scene::new()).unwrap();
        bytes[8..12].copy_from_slice(&(VERSION + 1).to_le_bytes());
        assert!(matches!(
            decode_ruv(&bytes),
            Err(FileError::UnsupportedVersion { found, .. }) if found == VERSION + 1
        ));
    }

    #[test]
    fn every_truncation_is_an_error_not_a_panic() {
        let bytes = encode_ruv(&sample()).unwrap();
        for len in 0..bytes.len() {
            assert!(decode_ruv(&bytes[..len]).is_err(), "prefix of {len} bytes");
        }
    }

    proptest! {
        #[test]
        fn flipped_bytes_never_panic(index in 0_usize..400, value in any::<u8>()) {
            let mut bytes = encode_ruv(&sample()).unwrap();
            let slot = index % bytes.len();
            bytes[slot] = value;
            let _ = decode_ruv(&bytes);
        }

        #[test]
        fn random_voxels_round_trip(
            points in proptest::collection::vec((-70..70_i32, -70..70_i32, -70..70_i32, 1..=96_u8), 0..60)
        ) {
            let mut scene = Scene::new();
            let model = scene.node(NodeId::ROOT).unwrap().children()[0];
            let shape = scene.node(model).and_then(Node::shape).unwrap();
            for (x, y, z, color) in points {
                scene.shape_mut(shape).unwrap().set(IVec3::new(x, y, z), ColorId::new(color));
            }
            let loaded = decode_ruv(&encode_ruv(&scene).unwrap()).unwrap();
            prop_assert!(same(&scene, &loaded));
        }
    }
}
