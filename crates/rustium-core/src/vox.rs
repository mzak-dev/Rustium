//! MagicaVoxel `.vox` reader and writer.
//!
//! MagicaVoxel is right-handed with Z up; Rustium has Y up. A MagicaVoxel point `(x, y, z)` becomes `(x, z, -y)`,
//! which is a proper rotation, so handedness is kept. A voxel is a unit cube, so a *cell* index maps as
//! `(x, y, z) -> (x, z, -y - 1)`.
//!
//! MagicaVoxel puts the origin of a model at `size / 2` (integer division) and rotates about it. The importer stores
//! each model as a Shape in that centred frame, so the scene graph's `nTRN` rotations and translations map straight
//! onto Node Transforms. A rotation byte stores, for each axis `r`, the axis and sign `e_r` is sent to.
//!
//! Colors: voxel index `i` (1 to 255) uses entry `i - 1` of the `RGBA` chunk, which is the same as Rustium's
//! `ColorId` `i` and Palette entry `i - 1`; the importer keeps the 255 entries and the exporter writes the Palette.
//! The exporter bakes every placed Model into world space (rotations and Instances included) and splits what exceeds
//! 256 voxels along an axis into several models.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use glam::IVec3;

use crate::file_error::FileError;
use crate::ids::{NodeId, ShapeId};
use crate::material_graph::MaterialGraph;
use crate::node::{Graph, Node, NodeKind};
use crate::palette::{Color, ColorId, Palette};
use crate::scene::Scene;
use crate::shape::Shape;
use crate::transform::{Orientation, Transform};

/// Largest side of one exported model.
const MAX_MODEL_SIDE: i32 = 256;

/// Version number written to the header.
const VOX_VERSION: u32 = 150;

/// Deepest scene graph followed on import.
const MAX_DEPTH: usize = 64;

/// Largest model side accepted on import.
const MAX_IMPORT_SIDE: u32 = 4096;

fn fail<T>(reason: impl Into<String>) -> Result<T, FileError> {
    Err(FileError::Vox(reason.into()))
}

/// MagicaVoxel cell (Z up) to Rustium cell (Y up).
fn cell_to_y_up(cell: IVec3) -> IVec3 {
    IVec3::new(cell.x, cell.z, -cell.y - 1)
}

/// Rustium cell (Y up) to MagicaVoxel cell (Z up).
fn cell_to_z_up(cell: IVec3) -> IVec3 {
    IVec3::new(cell.x, -cell.z - 1, cell.y)
}

/// MagicaVoxel vector (Z up) to Rustium vector (Y up).
fn vector_to_y_up(vector: IVec3) -> IVec3 {
    IVec3::new(vector.x, vector.z, -vector.y)
}

/// Rustium vector (Y up) to MagicaVoxel vector (Z up).
fn vector_to_z_up(vector: IVec3) -> IVec3 {
    IVec3::new(vector.x, -vector.z, vector.y)
}

struct Reader<'a> {
    data: &'a [u8],
    position: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, position: 0 }
    }

    fn remaining(&self) -> usize {
        self.data.len() - self.position
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8], FileError> {
        let end = self.position.checked_add(len).filter(|end| *end <= self.data.len());
        let Some(end) = end else {
            return fail("the file ends in the middle of a chunk");
        };
        let slice = &self.data[self.position..end];
        self.position = end;
        Ok(slice)
    }

    fn u8(&mut self) -> Result<u8, FileError> {
        Ok(self.take(1)?[0])
    }

    fn i32(&mut self) -> Result<i32, FileError> {
        let bytes = self.take(4)?;
        Ok(i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn u32(&mut self) -> Result<u32, FileError> {
        let bytes = self.take(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    /// A length-prefixed string.
    fn string(&mut self) -> Result<String, FileError> {
        let len = usize::try_from(self.i32()?).or_else(|_| fail("a string has a negative length"))?;
        Ok(String::from_utf8_lossy(self.take(len)?).into_owned())
    }

    /// A dictionary of strings.
    fn dict(&mut self) -> Result<HashMap<String, String>, FileError> {
        let count = usize::try_from(self.i32()?).or_else(|_| fail("a dictionary has a negative size"))?;
        // Every pair needs at least eight bytes, which bounds the allocation by the real data.
        if count > self.remaining() / 8 {
            return fail("a dictionary is larger than the file");
        }
        let mut dict = HashMap::with_capacity(count);
        for _ in 0..count {
            let key = self.string()?;
            let value = self.string()?;
            dict.insert(key, value);
        }
        Ok(dict)
    }
}

struct VoxModel {
    size: [u32; 3],
    voxels: Vec<[u8; 4]>,
}

enum VoxNode {
    Transform {
        name: Option<String>,
        hidden: bool,
        child: i32,
        rotation: u8,
        translation: IVec3,
    },
    Group {
        children: Vec<i32>,
    },
    Shape {
        models: Vec<i32>,
    },
}

#[derive(Default)]
struct VoxFile {
    models: Vec<VoxModel>,
    nodes: HashMap<i32, VoxNode>,
    palette: Option<Vec<[u8; 3]>>,
}

fn parse(bytes: &[u8]) -> Result<VoxFile, FileError> {
    let mut reader = Reader::new(bytes);
    if reader
        .take(4)
        .map_err(|_| FileError::Vox("the file is empty".to_owned()))?
        != b"VOX "
    {
        return fail("the file does not start with VOX");
    }
    let _version = reader.u32()?;
    if reader.take(4)? != b"MAIN" {
        return fail("the MAIN chunk is missing");
    }
    let content = usize::try_from(reader.u32()?).unwrap_or(usize::MAX);
    reader.take(content)?;
    let children = usize::try_from(reader.u32()?).unwrap_or(usize::MAX);
    // Files cut short by a download are common; read what is there instead of refusing everything.
    let mut reader = Reader::new(reader.take(children.min(reader.remaining()))?);

    let mut file = VoxFile::default();
    let mut pending_size: Option<[u32; 3]> = None;
    while reader.remaining() >= 12 {
        let id: [u8; 4] = reader.take(4)?.try_into().unwrap_or_default();
        let content = usize::try_from(reader.u32()?).unwrap_or(usize::MAX);
        let child_bytes = usize::try_from(reader.u32()?).unwrap_or(usize::MAX);
        let body = reader.take(content)?;
        reader.take(child_bytes)?;
        let mut body = Reader::new(body);
        match &id {
            b"SIZE" => {
                let (x, y, z) = (body.u32()?, body.u32()?, body.u32()?);
                if x > MAX_IMPORT_SIDE || y > MAX_IMPORT_SIDE || z > MAX_IMPORT_SIDE {
                    return fail("a model is larger than 4096 voxels on a side");
                }
                pending_size = Some([x, y, z]);
            }
            b"XYZI" => {
                let Some(size) = pending_size.take() else {
                    return fail("XYZI without a SIZE before it");
                };
                let count = usize::try_from(body.u32()?).unwrap_or(usize::MAX);
                if count > body.remaining() / 4 {
                    return fail("a model announces more voxels than it holds");
                }
                let mut voxels = Vec::with_capacity(count);
                for _ in 0..count {
                    voxels.push([body.u8()?, body.u8()?, body.u8()?, body.u8()?]);
                }
                file.models.push(VoxModel { size, voxels });
            }
            b"RGBA" => {
                let mut colors = Vec::with_capacity(255);
                for _ in 0..255 {
                    let [r, g, b, _alpha] = [body.u8()?, body.u8()?, body.u8()?, body.u8()?];
                    colors.push([r, g, b]);
                }
                file.palette = Some(colors);
            }
            b"nTRN" => {
                let node = body.i32()?;
                let attributes = body.dict()?;
                let child = body.i32()?;
                let _reserved = body.i32()?;
                let _layer = body.i32()?;
                let frames = body.i32()?;
                let frame = if frames > 0 { Some(body.dict()?) } else { None };
                let rotation = frame
                    .as_ref()
                    .and_then(|frame| frame.get("_r"))
                    .and_then(|text| text.trim().parse::<u8>().ok())
                    .unwrap_or(4);
                let translation = frame
                    .as_ref()
                    .and_then(|frame| frame.get("_t"))
                    .map_or(IVec3::ZERO, |text| {
                        let mut parts = text.split_whitespace().map(|part| part.parse::<i32>().unwrap_or(0));
                        IVec3::new(
                            parts.next().unwrap_or(0),
                            parts.next().unwrap_or(0),
                            parts.next().unwrap_or(0),
                        )
                    });
                file.nodes.insert(
                    node,
                    VoxNode::Transform {
                        name: attributes.get("_name").filter(|name| !name.trim().is_empty()).cloned(),
                        hidden: attributes.get("_hidden").is_some_and(|value| value.trim() == "1"),
                        child,
                        rotation,
                        translation,
                    },
                );
            }
            b"nGRP" => {
                let node = body.i32()?;
                let _attributes = body.dict()?;
                let count = usize::try_from(body.i32()?).or_else(|_| fail("a group has a negative size"))?;
                if count > body.remaining() / 4 {
                    return fail("a group lists more children than it holds");
                }
                let children = (0..count).map(|_| body.i32()).collect::<Result<Vec<_>, _>>()?;
                file.nodes.insert(node, VoxNode::Group { children });
            }
            b"nSHP" => {
                let node = body.i32()?;
                let _attributes = body.dict()?;
                let count = usize::try_from(body.i32()?).or_else(|_| fail("a shape has a negative size"))?;
                if count > body.remaining() / 8 {
                    return fail("a shape lists more models than it holds");
                }
                let mut models = Vec::with_capacity(count);
                for _ in 0..count {
                    models.push(body.i32()?);
                    body.dict()?;
                }
                file.nodes.insert(node, VoxNode::Shape { models });
            }
            _ => {}
        }
    }
    Ok(file)
}

/// The orientation a MagicaVoxel rotation byte stands for, in MagicaVoxel's axes.
fn rotation_columns(byte: u8) -> Option<[IVec3; 3]> {
    let first = usize::from(byte & 3);
    let second = usize::from((byte >> 2) & 3);
    let third = 3_usize.checked_sub(first + second)?;
    if first > 2 || second > 2 || third > 2 || first == second {
        return None;
    }
    let mut columns = [IVec3::ZERO; 3];
    for (row, (axis, bit)) in [(first, 4), (second, 5), (third, 6)].into_iter().enumerate() {
        columns[row][axis] = if byte & (1 << bit) == 0 { 1 } else { -1 };
    }
    Some(columns)
}

/// A MagicaVoxel node transform as a Rustium one: rotations are conjugated by the Z-up to Y-up change of axes.
fn node_transform(rotation: u8, translation: IVec3) -> Transform {
    let columns = rotation_columns(rotation).unwrap_or([IVec3::X, IVec3::Y, IVec3::Z]);
    let apply = |vector: IVec3| columns[0] * vector.x + columns[1] * vector.y + columns[2] * vector.z;
    let converted = [IVec3::X, IVec3::Y, IVec3::Z].map(|axis| vector_to_y_up(apply(vector_to_z_up(axis))));
    Transform {
        orientation: Orientation::from_columns(converted).unwrap_or_default(),
        offset: vector_to_y_up(translation),
    }
}

struct Importer<'a> {
    file: &'a VoxFile,
    scene: Scene,
    shapes: HashMap<i32, ShapeId>,
    visited: HashSet<i32>,
    counter: usize,
}

impl Importer<'_> {
    fn shape_for(&mut self, model: i32) -> Option<ShapeId> {
        if let Some(shape) = self.shapes.get(&model) {
            return Some(*shape);
        }
        let source = self.file.models.get(usize::try_from(model).ok()?)?;
        let half = IVec3::new(
            i32::try_from(source.size[0] / 2).ok()?,
            i32::try_from(source.size[1] / 2).ok()?,
            i32::try_from(source.size[2] / 2).ok()?,
        );
        let mut shape = Shape::new();
        for [x, y, z, index] in &source.voxels {
            let cell = IVec3::new(i32::from(*x), i32::from(*y), i32::from(*z));
            let inside = cell.cmplt(IVec3::new(
                i32::try_from(source.size[0]).ok()?,
                i32::try_from(source.size[1]).ok()?,
                i32::try_from(source.size[2]).ok()?,
            ));
            // Index 0 is "no voxel"; voxels outside the declared size are dropped rather than trusted.
            if let Some(color) = ColorId::new(*index).filter(|_| inside.all()) {
                shape.set(cell_to_y_up(cell - half), Some(color));
            }
        }
        let id = self.scene.add_shape(shape)?;
        self.shapes.insert(model, id);
        Some(id)
    }

    fn push(&mut self, parent: NodeId, node: Node) -> Option<NodeId> {
        let graph = Arc::make_mut(&mut self.scene.graph);
        let id = graph.push(node)?;
        graph.attach(parent, usize::MAX, id);
        Some(id)
    }

    /// Adds the MagicaVoxel node `id` below `parent`.
    fn visit(&mut self, id: i32, parent: NodeId, depth: usize) {
        if depth > MAX_DEPTH || !self.visited.insert(id) {
            return;
        }
        let file = self.file;
        match file.nodes.get(&id) {
            Some(VoxNode::Transform {
                name,
                hidden,
                child,
                rotation,
                translation,
            }) => {
                let transform = node_transform(*rotation, *translation);
                self.counter += 1;
                match file.nodes.get(child) {
                    Some(VoxNode::Shape { models }) => {
                        let Some(shape) = models.first().and_then(|model| self.shape_for(*model)) else {
                            return;
                        };
                        let name = name.clone().unwrap_or_else(|| format!("Model {}", self.counter));
                        self.push(
                            parent,
                            Node {
                                name,
                                parent: None,
                                transform,
                                hidden: *hidden,
                                locked: false,
                                kind: NodeKind::Model { shape },
                            },
                        );
                        self.visited.insert(*child);
                    }
                    Some(VoxNode::Group { children }) => {
                        let target = if depth == 0 && transform == Transform::IDENTITY && name.is_none() {
                            // The invisible root of the file: its children go straight into the Scene.
                            Some(parent)
                        } else {
                            let name = name.clone().unwrap_or_else(|| format!("Group {}", self.counter));
                            self.push(
                                parent,
                                Node {
                                    name,
                                    parent: None,
                                    transform,
                                    hidden: *hidden,
                                    locked: false,
                                    kind: NodeKind::Group { children: Vec::new() },
                                },
                            )
                        };
                        self.visited.insert(*child);
                        if let Some(target) = target {
                            for grandchild in children {
                                self.visit(*grandchild, target, depth + 1);
                            }
                        }
                    }
                    _ => {}
                }
            }
            Some(VoxNode::Group { children }) => {
                for child in children {
                    self.visit(*child, parent, depth + 1);
                }
            }
            Some(VoxNode::Shape { models }) => {
                if let Some(shape) = models.first().and_then(|model| self.shape_for(*model)) {
                    self.counter += 1;
                    let name = format!("Model {}", self.counter);
                    self.push(
                        parent,
                        Node {
                            name,
                            parent: None,
                            transform: Transform::IDENTITY,
                            hidden: false,
                            locked: false,
                            kind: NodeKind::Model { shape },
                        },
                    );
                }
            }
            None => {}
        }
    }
}

/// Reads a MagicaVoxel file into a new Scene named `name`.
///
/// # Errors
/// Fails when the data is not a `.vox` file or is damaged. Never panics on bad input.
pub fn import_vox(bytes: &[u8], name: &str) -> Result<Scene, FileError> {
    let file = parse(bytes)?;
    let mut colors: Vec<Color> = match &file.palette {
        Some(entries) => entries.iter().map(|rgb| Color::new(*rgb)).collect(),
        None => Palette::default().iter().map(|(_, color)| *color).collect(),
    };
    colors.resize(255, Color::new([128, 128, 128]));
    let Some(palette) = Palette::from_colors(colors) else {
        return fail("the palette cannot be built");
    };
    let root_name = if name.trim().is_empty() { "Scene" } else { name };
    let scene = Scene::from_parts(
        Graph::new(root_name),
        Vec::new(),
        palette,
        vec![MaterialGraph::default_material()],
        Scene::DEFAULT_VOXEL_SIZE,
    );
    let mut importer = Importer {
        file: &file,
        scene,
        shapes: HashMap::new(),
        visited: HashSet::new(),
        counter: 0,
    };
    if file.nodes.contains_key(&0) {
        importer.visit(0, NodeId::ROOT, 0);
    } else {
        // Files from before the scene graph: every model sits at the origin.
        for model in 0..file.models.len() {
            let Some(shape) = i32::try_from(model).ok().and_then(|model| importer.shape_for(model)) else {
                continue;
            };
            importer.push(
                NodeId::ROOT,
                Node {
                    name: format!("Model {}", model + 1),
                    parent: None,
                    transform: Transform::IDENTITY,
                    hidden: false,
                    locked: false,
                    kind: NodeKind::Model { shape },
                },
            );
        }
    }
    let mut scene = importer.scene;
    if scene.node(NodeId::ROOT).is_some_and(|root| root.children().is_empty()) {
        let Some(shape) = scene.add_shape(Shape::new()) else {
            return fail("the scene cannot be built");
        };
        let graph = Arc::make_mut(&mut scene.graph);
        if let Some(model) = graph.push(Node {
            name: "Model".to_owned(),
            parent: None,
            transform: Transform::IDENTITY,
            hidden: false,
            locked: false,
            kind: NodeKind::Model { shape },
        }) {
            graph.attach(NodeId::ROOT, 0, model);
        }
    }
    Ok(scene)
}

struct Writer {
    bytes: Vec<u8>,
}

impl Writer {
    fn i32(&mut self, value: i32) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    fn string(&mut self, text: &str) {
        self.i32(i32::try_from(text.len()).unwrap_or(i32::MAX));
        self.bytes.extend_from_slice(text.as_bytes());
    }

    fn dict(&mut self, pairs: &[(&str, &str)]) {
        self.i32(i32::try_from(pairs.len()).unwrap_or(0));
        for (key, value) in pairs {
            self.string(key);
            self.string(value);
        }
    }

    fn chunk(&mut self, id: [u8; 4], content: &[u8]) {
        self.bytes.extend_from_slice(&id);
        self.bytes
            .extend_from_slice(&u32::try_from(content.len()).unwrap_or(u32::MAX).to_le_bytes());
        self.bytes.extend_from_slice(&0_u32.to_le_bytes());
        self.bytes.extend_from_slice(content);
    }
}

struct ExportModel {
    name: String,
    size: IVec3,
    translation: IVec3,
    voxels: Vec<[u8; 4]>,
}

/// Bakes every placed Model into MagicaVoxel cells and cuts them into models of at most 256 voxels per side.
fn export_models(scene: &Scene) -> Vec<ExportModel> {
    let mut models = Vec::new();
    for placement in scene.placements() {
        let Some(shape) = scene.shape(placement.shape) else {
            continue;
        };
        let name = scene
            .node(placement.node)
            .map(|node| node.name().to_owned())
            .unwrap_or_default();
        let mut blocks: HashMap<IVec3, Vec<(IVec3, u8)>> = HashMap::new();
        for (cell, color) in shape.voxels() {
            let world = cell_to_z_up(placement.transform.cell(cell));
            blocks
                .entry(world.div_euclid(IVec3::splat(MAX_MODEL_SIDE)))
                .or_default()
                .push((world, color.get()));
        }
        let mut keys: Vec<IVec3> = blocks.keys().copied().collect();
        keys.sort_unstable_by_key(|key| (key.x, key.y, key.z));
        for key in keys {
            let Some(cells) = blocks.remove(&key) else {
                continue;
            };
            // Trim each block to the voxels it holds so models stay small.
            let min = cells.iter().fold(IVec3::MAX, |min, (cell, _)| min.min(*cell));
            let max = cells.iter().fold(IVec3::MIN, |max, (cell, _)| max.max(*cell));
            let size = max - min + IVec3::ONE;
            let voxels = cells
                .iter()
                .filter_map(|(cell, color)| {
                    let local = *cell - min;
                    Some([
                        u8::try_from(local.x).ok()?,
                        u8::try_from(local.y).ok()?,
                        u8::try_from(local.z).ok()?,
                        *color,
                    ])
                })
                .collect();
            models.push(ExportModel {
                name: name.clone(),
                size,
                translation: min + size / 2,
                voxels,
            });
        }
    }
    models
}

/// Writes the Scene as a MagicaVoxel file.
///
/// # Errors
/// Fails when the Scene has no voxels at all, because MagicaVoxel cannot open a file without models.
pub fn export_vox(scene: &Scene) -> Result<Vec<u8>, FileError> {
    let models = export_models(scene);
    if models.is_empty() {
        return fail("the scene has no voxels to export");
    }
    let mut body = Writer { bytes: Vec::new() };
    for model in &models {
        let size: Vec<u8> = [model.size.x, model.size.y, model.size.z]
            .into_iter()
            .flat_map(|side| side.unsigned_abs().to_le_bytes())
            .collect();
        body.chunk(*b"SIZE", &size);
        let mut voxels = Vec::with_capacity(4 + model.voxels.len() * 4);
        voxels.extend_from_slice(&u32::try_from(model.voxels.len()).unwrap_or(0).to_le_bytes());
        voxels.extend(model.voxels.iter().flatten());
        body.chunk(*b"XYZI", &voxels);
    }

    let mut node = Writer { bytes: Vec::new() };
    // Root transform (0) -> group (1) -> one transform + shape pair per model.
    node.i32(0);
    node.dict(&[]);
    node.i32(1);
    node.i32(-1);
    node.i32(-1);
    node.i32(1);
    node.dict(&[]);
    body.chunk(*b"nTRN", &node.bytes);

    let mut group = Writer { bytes: Vec::new() };
    group.i32(1);
    group.dict(&[]);
    group.i32(i32::try_from(models.len()).unwrap_or(0));
    for index in 0..models.len() {
        group.i32(i32::try_from(2 + index * 2).unwrap_or(0));
    }
    body.chunk(*b"nGRP", &group.bytes);

    for (index, model) in models.iter().enumerate() {
        let transform_id = i32::try_from(2 + index * 2).unwrap_or(0);
        let mut transform = Writer { bytes: Vec::new() };
        transform.i32(transform_id);
        if model.name.is_empty() {
            transform.dict(&[]);
        } else {
            transform.dict(&[("_name", &model.name)]);
        }
        transform.i32(transform_id + 1);
        transform.i32(-1);
        transform.i32(0);
        transform.i32(1);
        let translation = format!(
            "{} {} {}",
            model.translation.x, model.translation.y, model.translation.z
        );
        transform.dict(&[("_t", &translation)]);
        body.chunk(*b"nTRN", &transform.bytes);

        let mut shape = Writer { bytes: Vec::new() };
        shape.i32(transform_id + 1);
        shape.dict(&[]);
        shape.i32(1);
        shape.i32(i32::try_from(index).unwrap_or(0));
        shape.dict(&[]);
        body.chunk(*b"nSHP", &shape.bytes);
    }

    let mut layer = Writer { bytes: Vec::new() };
    layer.i32(0);
    layer.dict(&[]);
    layer.i32(-1);
    body.chunk(*b"LAYR", &layer.bytes);

    let mut rgba = Vec::with_capacity(1024);
    for (_, color) in scene.palette().iter().take(255) {
        rgba.extend_from_slice(&[color.color[0], color.color[1], color.color[2], 255]);
    }
    while rgba.len() < 255 * 4 {
        rgba.extend_from_slice(&[255, 255, 255, 255]);
    }
    rgba.extend_from_slice(&[0, 0, 0, 0]);
    body.chunk(*b"RGBA", &rgba);

    let mut bytes = Vec::with_capacity(body.bytes.len() + 20);
    bytes.extend_from_slice(b"VOX ");
    bytes.extend_from_slice(&VOX_VERSION.to_le_bytes());
    bytes.extend_from_slice(b"MAIN");
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    bytes.extend_from_slice(&u32::try_from(body.bytes.len()).unwrap_or(u32::MAX).to_le_bytes());
    bytes.extend_from_slice(&body.bytes);
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    fn chunk(id: &[u8], content: &[u8]) -> Vec<u8> {
        let mut bytes = id.to_vec();
        bytes.extend_from_slice(&u32::try_from(content.len()).unwrap().to_le_bytes());
        bytes.extend_from_slice(&0_u32.to_le_bytes());
        bytes.extend_from_slice(content);
        bytes
    }

    fn file(chunks: &[Vec<u8>]) -> Vec<u8> {
        let body: Vec<u8> = chunks.concat();
        let mut bytes = b"VOX ".to_vec();
        bytes.extend_from_slice(&150_u32.to_le_bytes());
        bytes.extend_from_slice(b"MAIN");
        bytes.extend_from_slice(&0_u32.to_le_bytes());
        bytes.extend_from_slice(&u32::try_from(body.len()).unwrap().to_le_bytes());
        bytes.extend_from_slice(&body);
        bytes
    }

    fn model(size: [u32; 3], voxels: &[[u8; 4]]) -> Vec<Vec<u8>> {
        let mut dims = Vec::new();
        for side in size {
            dims.extend_from_slice(&side.to_le_bytes());
        }
        let mut xyzi = u32::try_from(voxels.len()).unwrap().to_le_bytes().to_vec();
        xyzi.extend(voxels.iter().flatten());
        vec![chunk(b"SIZE", &dims), chunk(b"XYZI", &xyzi)]
    }

    fn dict(pairs: &[(&str, &str)]) -> Vec<u8> {
        let mut writer = Writer { bytes: Vec::new() };
        writer.dict(pairs);
        writer.bytes
    }

    fn ntrn(id: i32, child: i32, name: Option<&str>, rotation: Option<&str>, translation: &str) -> Vec<u8> {
        let mut writer = Writer { bytes: Vec::new() };
        writer.i32(id);
        writer.bytes.extend(match name {
            Some(name) => dict(&[("_name", name)]),
            None => dict(&[]),
        });
        for value in [child, -1, 0, 1] {
            writer.i32(value);
        }
        let mut pairs = vec![("_t", translation)];
        if let Some(rotation) = rotation {
            pairs.push(("_r", rotation));
        }
        writer.bytes.extend(dict(&pairs));
        chunk(b"nTRN", &writer.bytes)
    }

    fn ngrp(id: i32, children: &[i32]) -> Vec<u8> {
        let mut writer = Writer { bytes: Vec::new() };
        writer.i32(id);
        writer.bytes.extend(dict(&[]));
        writer.i32(i32::try_from(children.len()).unwrap());
        for child in children {
            writer.i32(*child);
        }
        chunk(b"nGRP", &writer.bytes)
    }

    fn nshp(id: i32, model: i32) -> Vec<u8> {
        let mut writer = Writer { bytes: Vec::new() };
        writer.i32(id);
        writer.bytes.extend(dict(&[]));
        writer.i32(1);
        writer.i32(model);
        writer.bytes.extend(dict(&[]));
        chunk(b"nSHP", &writer.bytes)
    }

    fn rgba() -> Vec<u8> {
        let mut content = Vec::new();
        for index in 0..256_u32 {
            content.extend_from_slice(&[u8::try_from(index).unwrap(), 255 - u8::try_from(index).unwrap(), 7, 255]);
        }
        chunk(b"RGBA", &content)
    }

    /// Every voxel of the Scene in Scene space.
    fn world_voxels(scene: &Scene) -> BTreeSet<([i32; 3], u8)> {
        let mut result = BTreeSet::new();
        for placement in scene.placements() {
            if let Some(shape) = scene.shape(placement.shape) {
                for (cell, color) in shape.voxels() {
                    result.insert((placement.transform.cell(cell).to_array(), color.get()));
                }
            }
        }
        result
    }

    #[test]
    fn models_without_a_scene_graph_sit_at_the_origin_and_use_y_up() {
        // A 4x4x4 model; the voxel at (3, 0, 2) is 2 above the floor in MagicaVoxel (Z up).
        let mut chunks = model([4, 4, 4], &[[3, 0, 2, 5], [0, 0, 0, 1]]);
        chunks.push(rgba());
        let scene = import_vox(&file(&chunks), "ship").unwrap();
        assert_eq!(scene.node(NodeId::ROOT).unwrap().name(), "ship");
        let voxels = world_voxels(&scene);
        // half = 2: (3,0,2) -> (1,-2,0) -> Y up (1, 0, 1); (0,0,0) -> (-2,-2,-2) -> (-2, -2, 1).
        assert!(voxels.contains(&([1, 0, 1], 5)), "{voxels:?}");
        assert!(voxels.contains(&([-2, -2, 1], 1)), "{voxels:?}");
    }

    #[test]
    fn the_palette_index_shift_matches_color_ids() {
        let mut chunks = model([1, 1, 1], &[[0, 0, 0, 10]]);
        chunks.push(rgba());
        let scene = import_vox(&file(&chunks), "x").unwrap();
        assert_eq!(scene.palette().len(), 255);
        let color = scene.palette().get(ColorId::new(10).unwrap()).unwrap();
        assert_eq!(color.color, [9, 246, 7], "index 10 is RGBA entry 9");
        assert_eq!(world_voxels(&scene).iter().next().map(|(_, color)| *color), Some(10));
    }

    #[test]
    fn a_scene_graph_places_instances_with_translation_and_rotation() {
        let mut chunks = model([2, 2, 2], &[[0, 0, 0, 3]]);
        chunks.push(ntrn(0, 1, None, None, "0 0 0"));
        chunks.push(ngrp(1, &[2, 4]));
        chunks.push(ntrn(2, 3, Some("left"), None, "10 0 0"));
        chunks.push(nshp(3, 0));
        // Quarter turn about Z: x -> y, y -> -x, z -> z.
        chunks.push(ntrn(4, 5, Some("right"), Some("33"), "0 20 0"));
        chunks.push(nshp(5, 0));
        chunks.push(rgba());
        let scene = import_vox(&file(&chunks), "pair").unwrap();
        let root = scene.node(NodeId::ROOT).unwrap();
        assert_eq!(root.children().len(), 2, "the invisible root is flattened away");
        let names: Vec<&str> = root
            .children()
            .iter()
            .filter_map(|id| scene.node(*id))
            .map(Node::name)
            .collect();
        assert_eq!(names, ["left", "right"]);
        assert_eq!(scene.instance_counts().values().copied().collect::<Vec<_>>(), vec![2]);
        // One voxel at model-local (-1,-1,-1) after the centring shift.
        let voxels = world_voxels(&scene);
        // left: lower corner (-1,-1,-1)+(10,0,0) = (9,-1,-1) -> Y up (9,-1,0).
        assert!(voxels.contains(&([9, -1, 0], 3)), "{voxels:?}");
        // right: rotated (-1,-1,-1) cell (corner) -> centre (-.5,-.5,-.5) -> (.5,-.5,-.5), +(0,20,0) = (.5,19.5,-.5)
        // -> cell (0,19,-1) -> Y up (0,-1,-20).
        assert!(voxels.contains(&([0, -1, -20], 3)), "{voxels:?}");
    }

    #[test]
    fn exporting_and_importing_again_keeps_every_voxel_in_place() {
        let mut scene = Scene::new();
        let node = scene.node(NodeId::ROOT).unwrap().children()[0];
        let shape = scene.node(node).and_then(Node::shape).unwrap();
        let target = scene.shape_mut(shape).unwrap();
        for (x, y, z, color) in [
            (0, 0, 0, 1),
            (5, -3, 2, 9),
            (-7, 4, -6, 200),
            (31, 31, 31, 255),
            (-33, 0, 33, 4),
        ] {
            target.set(IVec3::new(x, y, z), ColorId::new(color));
        }
        let bytes = export_vox(&scene).unwrap();
        let loaded = import_vox(&bytes, "again").unwrap();
        assert_eq!(world_voxels(&loaded), world_voxels(&scene));
        let first = scene.palette().get(ColorId::FIRST).unwrap();
        assert_eq!(loaded.palette().get(ColorId::FIRST).unwrap().color, first.color);
    }

    #[test]
    fn a_rotated_node_is_baked_into_the_export() {
        let mut scene = Scene::new();
        let node = scene.node(NodeId::ROOT).unwrap().children()[0];
        let shape = scene.node(node).and_then(Node::shape).unwrap();
        scene
            .shape_mut(shape)
            .unwrap()
            .set(IVec3::new(3, 1, 0), ColorId::new(2));
        let turned = Transform {
            orientation: Orientation::rotation(1, 1),
            offset: IVec3::new(5, 0, 0),
        };
        Arc::make_mut(&mut scene.graph).get_mut(node).unwrap().transform = turned;
        let loaded = import_vox(&export_vox(&scene).unwrap(), "baked").unwrap();
        assert_eq!(world_voxels(&loaded), world_voxels(&scene));
    }

    #[test]
    fn models_larger_than_256_are_split_on_export() {
        let mut scene = Scene::new();
        let node = scene.node(NodeId::ROOT).unwrap().children()[0];
        let shape = scene.node(node).and_then(Node::shape).unwrap();
        let target = scene.shape_mut(shape).unwrap();
        for x in [0, 255, 256, 600] {
            target.set(IVec3::new(x, 0, 0), ColorId::new(3));
        }
        let models = export_models(&scene);
        assert!(models.len() >= 3, "{} models", models.len());
        assert!(models.iter().all(|model| model.size.max_element() <= 256));
        let loaded = import_vox(&export_vox(&scene).unwrap(), "wide").unwrap();
        assert_eq!(world_voxels(&loaded), world_voxels(&scene));
    }

    #[test]
    fn an_empty_scene_cannot_be_exported() {
        assert!(export_vox(&Scene::new()).is_err());
    }

    #[test]
    fn rotation_bytes_decode_to_signed_permutations() {
        assert_eq!(rotation_columns(4), Some([IVec3::X, IVec3::Y, IVec3::Z]));
        assert_eq!(rotation_columns(33), Some([IVec3::Y, IVec3::NEG_X, IVec3::Z]));
        assert_eq!(rotation_columns(0), None, "two rows on the same axis");
        assert_eq!(rotation_columns(255), None);
    }

    #[test]
    fn damaged_files_are_errors_not_panics() {
        let mut chunks = model([4, 4, 4], &[[3, 0, 2, 5]]);
        chunks.push(ntrn(0, 1, None, None, "0 0 0"));
        chunks.push(ngrp(1, &[2]));
        chunks.push(ntrn(2, 3, None, None, "1 2 3"));
        chunks.push(nshp(3, 0));
        chunks.push(rgba());
        let bytes = file(&chunks);
        for len in 0..bytes.len() {
            let _ = import_vox(&bytes[..len], "cut");
        }
        for index in 0..bytes.len() {
            let mut damaged = bytes.clone();
            damaged[index] = damaged[index].wrapping_add(0x91);
            let _ = import_vox(&damaged, "flipped");
        }
        assert!(import_vox(b"not a vox file at all", "x").is_err());
        assert!(import_vox(b"", "x").is_err());
    }

    #[test]
    fn a_cyclic_scene_graph_terminates() {
        let mut chunks = model([2, 2, 2], &[[0, 0, 0, 1]]);
        chunks.push(ntrn(0, 1, None, None, "0 0 0"));
        chunks.push(ngrp(1, &[0, 1, 2]));
        chunks.push(ntrn(2, 3, None, None, "0 0 0"));
        chunks.push(nshp(3, 0));
        let scene = import_vox(&file(&chunks), "loop").unwrap();
        assert!(scene.descendants(NodeId::ROOT).len() >= 2);
    }
}
