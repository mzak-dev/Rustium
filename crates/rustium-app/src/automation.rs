//! Answers MCP requests on the UI thread. Every Scene change is a `rustium_core::Command` applied through the
//! [`Editor`], so MCP edits are live in the viewport and undoable like the user's own.

use glam::IVec3;
use rustium_core::{
    Brush, BrushKind, CHUNK_SIZE, ChunkCoord, Command, Face, HitTarget, MaterialId, Mirror, NodeId, NodeKind, RayHit,
    Shape, Volume, VoxelTool,
};
use rustium_mcp::{BrushTool, FaceDirection, Reply, Request, VoxelWrite};
use rustium_script::Limits;
use serde_json::{Value, json};

use crate::editor::Editor;

/// Most positions one `get_voxels` call may look up.
const MAX_LOOKUPS: usize = 100_000;

/// Applies `request` to the editor and builds the JSON answer for the MCP client.
pub(crate) fn handle(editor: &mut Editor, request: Request) -> Reply {
    match request {
        Request::SceneInfo => Ok(scene_info(editor)),
        Request::GetVoxels { model, positions } => get_voxels(editor, model, &positions),
        Request::GetRegion { model, min, max, limit } => get_region(editor, model, min, max, limit),
        Request::GetPalette => Ok(get_palette(editor)),
        Request::SetVoxels { model, voxels } => set_voxels(editor, model, &voxels),
        Request::ApplyBrush {
            model,
            tool,
            size,
            voxel,
            face,
            ground,
            material,
        } => apply_brush(editor, model, tool, size, voxel, face, ground, material),
        Request::SetPaletteColor { material, color } => {
            let material = material_id(material)?;
            let changed = execute(editor, &Command::SetMaterialColor { material, color })?;
            Ok(json!({ "changed": changed }))
        }
        Request::RunScript { model, source } => {
            let model = target_model(editor, model)?;
            let output = editor
                .run_script(model, &source, &Limits::default())
                .map_err(|error| error.to_string())?;
            Ok(json!({ "changed": output.changed, "log": output.log }))
        }
        Request::StartRender => Err("Rendering is not available until milestone M3 (the path tracer).".to_owned()),
    }
}

fn execute(editor: &mut Editor, command: &Command) -> Result<bool, String> {
    editor.execute_checked(command).map_err(|error| error.to_string())
}

/// The Model a request targets: the given node id, or the Focus when the Focus is a Model.
fn target_model(editor: &Editor, model: Option<u32>) -> Result<NodeId, String> {
    let id = model.map_or(editor.focus, NodeId::from_raw);
    let node = editor
        .scene
        .node(id)
        .filter(|_| editor.scene.is_attached(id))
        .ok_or_else(|| format!("node {} is not in the scene", id.get()))?;
    if node.is_group() {
        let hint = if model.is_some() { "" } else { " (the Focus is a Group)" };
        return Err(format!(
            "node {} is a group{hint}; pass `model` with the id of a Model from scene_info",
            id.get()
        ));
    }
    Ok(id)
}

fn model_shape(editor: &Editor, model: Option<u32>) -> Result<&Shape, String> {
    let id = target_model(editor, model)?;
    editor
        .scene
        .model_shape(id)
        .map(|(_, shape)| shape)
        .ok_or_else(|| format!("node {} has no shape", id.get()))
}

fn material_id(number: u8) -> Result<MaterialId, String> {
    MaterialId::new(number).ok_or_else(|| "material numbers start at 1".to_owned())
}

fn vec3(position: [i32; 3]) -> IVec3 {
    IVec3::from_array(position)
}

fn volume_json(volume: Volume) -> Value {
    json!({
        "min": volume.min.to_array(),
        "max": volume.max.to_array(),
        "size": volume.size().to_array(),
    })
}

fn scene_info(editor: &Editor) -> Value {
    let scene = &editor.scene;
    json!({
        "voxel_size_metres": scene.voxel_size(),
        "focus": editor.focus.get(),
        "tree": node_json(editor, NodeId::ROOT),
        "palette_len": scene.palette().len(),
        "used_voxels": scene.used_voxels(NodeId::ROOT),
        "undo_steps": editor.history.done().count(),
        "can_undo": editor.history.can_undo(),
        "can_redo": editor.history.can_redo(),
    })
}

/// One Node of the tree: Groups list their children, Models their Volume (in the Model's own cells).
fn node_json(editor: &Editor, id: NodeId) -> Value {
    let scene = &editor.scene;
    let Some(node) = scene.node(id) else {
        return Value::Null;
    };
    let mut entry = json!({
        "id": id.get(),
        "name": node.name(),
        "hidden": node.is_hidden(),
        "locked": node.is_locked(),
    });
    match node.kind() {
        NodeKind::Group { children } => {
            entry["kind"] = json!("group");
            entry["children"] = Value::Array(children.iter().map(|child| node_json(editor, *child)).collect());
        }
        NodeKind::Model { .. } => {
            entry["kind"] = json!("model");
            let shape = scene.model_shape(id).map(|(_, shape)| shape);
            entry["volume"] = json!(shape.and_then(Shape::volume).map(volume_json));
            entry["voxels"] = json!(shape.map_or(0, Shape::used_count));
        }
    }
    entry
}

fn get_voxels(editor: &Editor, model: Option<u32>, positions: &[[i32; 3]]) -> Reply {
    let shape = model_shape(editor, model)?;
    if positions.len() > MAX_LOOKUPS {
        return Err(format!(
            "{} positions in one call exceed the limit of {MAX_LOOKUPS}",
            positions.len()
        ));
    }
    let materials: Vec<Option<u8>> = positions
        .iter()
        .map(|position| shape.get(vec3(*position)).map(MaterialId::get))
        .collect();
    Ok(json!({ "materials": materials }))
}

/// Filled voxels in the box, in chunk order then local order, so a truncated answer is stable.
fn get_region(editor: &Editor, model: Option<u32>, min: [i32; 3], max: [i32; 3], limit: usize) -> Reply {
    let shape = model_shape(editor, model)?;
    let (low, high) = (vec3(min).min(vec3(max)), vec3(min).max(vec3(max)));
    let mut chunks: Vec<ChunkCoord> = shape
        .chunks()
        .map(|(coord, _)| coord)
        .filter(|coord| {
            let origin = coord.origin();
            origin.cmple(high).all() && (origin + IVec3::splat(CHUNK_SIZE - 1)).cmpge(low).all()
        })
        .collect();
    chunks.sort_by_key(|coord| coord.0.to_array());
    let mut voxels = Vec::new();
    let mut truncated = false;
    'chunks: for coord in chunks {
        let Some(chunk) = shape.chunk(coord) else {
            continue;
        };
        let origin = coord.origin();
        for z in 0..CHUNK_SIZE {
            for y in 0..CHUNK_SIZE {
                for x in 0..CHUNK_SIZE {
                    let local = IVec3::new(x, y, z);
                    let position = origin + local;
                    if position.cmplt(low).any() || position.cmpgt(high).any() {
                        continue;
                    }
                    if let Some(material) = chunk.get(local) {
                        if voxels.len() >= limit {
                            truncated = true;
                            break 'chunks;
                        }
                        voxels.push(json!([position.x, position.y, position.z, material.get()]));
                    }
                }
            }
        }
    }
    Ok(json!({ "voxels": voxels, "truncated": truncated }))
}

fn get_palette(editor: &Editor) -> Value {
    let entries: Vec<Value> = editor
        .scene
        .palette()
        .iter()
        .map(|(id, material)| {
            let [r, g, b] = material.color;
            json!({ "material": id.get(), "hex": format!("#{r:02x}{g:02x}{b:02x}"), "rgb": material.color })
        })
        .collect();
    json!({ "palette": entries })
}

fn set_voxels(editor: &mut Editor, model: Option<u32>, writes: &[VoxelWrite]) -> Reply {
    let model = target_model(editor, model)?;
    let mut voxels = Vec::with_capacity(writes.len());
    for write in writes {
        let material = write.material.map(material_id).transpose()?;
        voxels.push((vec3(write.position), material));
    }
    let count = voxels.len();
    let changed = execute(editor, &Command::SetVoxels { model, voxels })?;
    Ok(json!({ "changed": changed, "requested": count }))
}

#[expect(clippy::too_many_arguments, reason = "one parameter per field of the MCP request")]
fn apply_brush(
    editor: &mut Editor,
    model: Option<u32>,
    tool: BrushTool,
    size: u32,
    voxel: [i32; 3],
    face: FaceDirection,
    ground: bool,
    material: u8,
) -> Reply {
    let command = Command::ApplyBrush {
        model: target_model(editor, model)?,
        tool: match tool {
            BrushTool::Attach => VoxelTool::Attach,
            BrushTool::Erase => VoxelTool::Erase,
            BrushTool::Paint => VoxelTool::Paint,
        },
        brush: Brush {
            kind: BrushKind::Voxel,
            size,
        },
        end: None,
        mirror: Mirror::default(),
        hit: RayHit {
            voxel: vec3(voxel),
            face: match face {
                FaceDirection::PosX => Face::PosX,
                FaceDirection::NegX => Face::NegX,
                FaceDirection::PosY => Face::PosY,
                FaceDirection::NegY => Face::NegY,
                FaceDirection::PosZ => Face::PosZ,
                FaceDirection::NegZ => Face::NegZ,
            },
            target: if ground { HitTarget::Ground } else { HitTarget::Voxel },
        },
        material: material_id(material)?,
    };
    let changed = execute(editor, &command)?;
    Ok(json!({ "changed": changed }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor() -> Editor {
        Editor::new(0.25, 50)
    }

    fn set(editor: &mut Editor, voxels: &[([i32; 3], Option<u8>)]) -> Reply {
        set_in(editor, None, voxels)
    }

    fn set_in(editor: &mut Editor, model: Option<u32>, voxels: &[([i32; 3], Option<u8>)]) -> Reply {
        let voxels = voxels
            .iter()
            .map(|(position, material)| VoxelWrite {
                position: *position,
                material: *material,
            })
            .collect();
        handle(editor, Request::SetVoxels { model, voxels })
    }

    fn focus_shape(editor: &Editor) -> &Shape {
        editor.scene.model_shape(editor.focus).unwrap().1
    }

    #[test]
    fn mcp_edits_are_live_and_undo_as_one_step() {
        let mut editor = editor();
        let reply = set(
            &mut editor,
            &[([0, 0, 0], Some(5)), ([40, 1, 2], Some(6)), ([1, 0, 0], None)],
        )
        .unwrap();
        assert_eq!(reply["changed"], true);
        assert_eq!(editor.history.done().count(), 1);
        assert_eq!(
            focus_shape(&editor).get(IVec3::new(40, 1, 2)).map(MaterialId::get),
            Some(6)
        );
        let shape = editor.scene.model_shape(editor.focus).unwrap().0;
        let placements = editor.scene.placements();
        let dirty = editor.take_dirty(&placements).1;
        assert!(dirty.contains(&(shape, ChunkCoord::of(IVec3::new(40, 1, 2)))));
        let revision = editor.revision;

        editor.undo();
        assert!(focus_shape(&editor).get(IVec3::ZERO).is_none());
        assert!(focus_shape(&editor).volume().is_none());
        assert!(editor.revision > revision, "the path tracer must see the undo");
    }

    #[test]
    fn requests_target_a_model_by_id_and_default_to_the_focus() {
        let mut editor = editor();
        let other = editor.execute(&Command::NewModel {
            parent: NodeId::ROOT,
            name: "Other".to_owned(),
        })[0];
        set_in(&mut editor, Some(other.get()), &[([0, 0, 0], Some(2))]).unwrap();
        assert!(
            focus_shape(&editor).get(IVec3::ZERO).is_none(),
            "the Focus Model is untouched"
        );
        let placed = editor.scene.model_shape(other).unwrap().1.get(IVec3::ZERO);
        assert_eq!(placed.map(MaterialId::get), Some(2));

        let info = handle(&mut editor, Request::SceneInfo).unwrap();
        let children = info["tree"]["children"].as_array().unwrap();
        assert_eq!(children.len(), 2);
        let listed = children.iter().find(|child| child["name"] == "Other").unwrap();
        assert_eq!(listed["id"], other.get());
        assert_eq!(listed["voxels"], 1);
        assert_eq!(info["focus"], editor.focus.get());
    }

    #[test]
    fn a_group_or_unknown_node_is_not_a_target() {
        let mut editor = editor();
        let error = set_in(&mut editor, Some(0), &[([0, 0, 0], Some(1))]).unwrap_err();
        assert!(error.contains("group"), "{error}");
        let error = set_in(&mut editor, Some(99), &[([0, 0, 0], Some(1))]).unwrap_err();
        assert!(error.contains("not in the scene"), "{error}");
        editor.focus = NodeId::ROOT;
        let error = set(&mut editor, &[([0, 0, 0], Some(1))]).unwrap_err();
        assert!(error.contains("Focus is a Group"), "{error}");
        assert!(!editor.history.can_undo());
    }

    #[test]
    fn locked_models_refuse_edits() {
        let mut editor = editor();
        let model = editor.focus;
        editor.execute(&Command::SetLocked {
            node: model,
            locked: true,
        });
        let error = set_in(&mut editor, Some(model.get()), &[([0, 0, 0], Some(1))]).unwrap_err();
        assert!(error.contains("hidden or locked"), "{error}");
    }

    #[test]
    fn reads_report_the_model() {
        let mut editor = editor();
        set(&mut editor, &[([1, 2, 3], Some(5)), ([4, 2, 3], Some(7))]).unwrap();
        let info = handle(&mut editor, Request::SceneInfo).unwrap();
        let model = info["tree"]["children"][0].clone();
        assert_eq!(model["volume"]["size"], json!([4, 1, 1]));
        assert_eq!(info["palette_len"], 96);
        assert_eq!(info["can_undo"], true);

        let voxels = handle(
            &mut editor,
            Request::GetVoxels {
                model: None,
                positions: vec![[1, 2, 3], [0, 0, 0]],
            },
        )
        .unwrap();
        assert_eq!(voxels["materials"], json!([5, null]));

        let region = handle(
            &mut editor,
            Request::GetRegion {
                model: None,
                min: [4, 2, 3],
                max: [0, 0, 0],
                limit: 10,
            },
        )
        .unwrap();
        assert_eq!(region["voxels"], json!([[1, 2, 3, 5], [4, 2, 3, 7]]));
        assert_eq!(region["truncated"], false);
        let one = handle(
            &mut editor,
            Request::GetRegion {
                model: None,
                min: [0, 0, 0],
                max: [9, 9, 9],
                limit: 1,
            },
        )
        .unwrap();
        assert_eq!(one["voxels"].as_array().map(Vec::len), Some(1));
        assert_eq!(one["truncated"], true);

        let palette = handle(&mut editor, Request::GetPalette).unwrap();
        assert_eq!(palette["palette"][0]["hex"], "#f5f5f5");
    }

    #[test]
    fn invalid_requests_change_nothing_and_say_why() {
        let mut editor = editor();
        let error = set(&mut editor, &[([0, 0, 0], Some(5)), ([1, 0, 0], Some(200))]).unwrap_err();
        assert!(error.contains("200"), "{error}");
        assert!(focus_shape(&editor).get(IVec3::ZERO).is_none());
        assert!(!editor.history.can_undo());
        let error = set(&mut editor, &[([i32::MAX, 0, 0], Some(1))]).unwrap_err();
        assert!(error.contains("outside"), "{error}");
        assert!(handle(&mut editor, Request::StartRender).unwrap_err().contains("M3"));
    }

    #[test]
    fn brush_places_voxels_like_a_click() {
        let mut editor = editor();
        let reply = handle(
            &mut editor,
            Request::ApplyBrush {
                model: None,
                tool: BrushTool::Attach,
                size: 2,
                voxel: [0, -1, 0],
                face: FaceDirection::PosY,
                ground: true,
                material: 3,
            },
        )
        .unwrap();
        assert_eq!(reply["changed"], true);
        assert_eq!(focus_shape(&editor).volume().map(Volume::size), Some(IVec3::splat(2)));
        editor.undo();
        assert!(focus_shape(&editor).volume().is_none());
    }

    #[test]
    fn palette_color_edits_mark_the_palette_for_upload_and_undo() {
        let mut editor = editor();
        editor.take_palette_dirty();
        handle(
            &mut editor,
            Request::SetPaletteColor {
                material: 2,
                color: [1, 2, 3],
            },
        )
        .unwrap();
        assert!(editor.take_palette_dirty());
        assert!(!editor.take_palette_dirty());
        editor.undo();
        assert!(editor.take_palette_dirty(), "undo changes the palette too");
        let id = MaterialId::new(2).unwrap();
        assert_ne!(
            editor.scene.palette().get(id).map(|material| material.color),
            Some([1, 2, 3])
        );
    }

    #[test]
    fn a_script_is_one_undo_step_and_reports_its_log() {
        let mut editor = editor();
        let reply = handle(
            &mut editor,
            Request::RunScript {
                model: None,
                source: "for i = 0, 9 do scene.set(i, 0, 0, 4) end print('done')".to_owned(),
            },
        )
        .unwrap();
        assert_eq!(reply["log"], json!(["done"]));
        assert_eq!(editor.history.done().count(), 1);
        editor.undo();
        assert!(focus_shape(&editor).volume().is_none());
        let error = handle(
            &mut editor,
            Request::RunScript {
                model: None,
                source: "error('nope')".to_owned(),
            },
        )
        .unwrap_err();
        assert!(error.contains("nope"));
        assert!(!editor.history.can_undo());
    }
}
