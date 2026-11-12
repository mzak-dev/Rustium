//! Answers MCP requests on the UI thread. Every Scene change is a `rustium_core::Command` applied through the
//! [`Editor`], so MCP edits are live in the viewport and undoable like the user's own.

use glam::IVec3;
use rustium_core::{
    Brush, BrushKind, CHUNK_SIZE, ChunkCoord, ColorId, Command, Face, GraphEdit, GraphNodeKind, HitTarget,
    MaterialGraph, Mirror, NodeId, NodeKind, RayHit, SampleMaterial, Shape, SurfaceConstants, Volume, VoxelTool,
};
use rustium_mcp::{BrushTool, FaceDirection, Reply, Request, SurfaceParams, VoxelWrite};
use rustium_script::Limits;
use serde_json::{Value, json};

use crate::editor::Editor;

/// Most positions one `get_voxels` call may look up.
const MAX_LOOKUPS: usize = 100_000;
/// Longest Material name an MCP client may set, in characters (the same limit as renaming).
const MATERIAL_NAME_MAX: usize = 64;

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
            color,
        } => apply_brush(editor, model, tool, size, voxel, face, ground, color),
        Request::SetPaletteColor { id, color } => {
            let id = color_id(id)?;
            let changed = execute(editor, &Command::SetColor { id, color })?;
            Ok(json!({ "changed": changed }))
        }
        Request::RunScript { model, source } => {
            let model = target_model(editor, model)?;
            let output = editor
                .run_script(model, &source, &Limits::default())
                .map_err(|error| error.to_string())?;
            Ok(json!({ "changed": output.changed, "log": output.log }))
        }
        Request::StartRender => {
            Err("Starting a render over MCP is not wired up yet; ask the user to start it in Render mode.".to_owned())
        }
        Request::GetMaterials => Ok(get_materials(editor)),
        Request::GetMaterialNodeKinds => Ok(material_node_kinds()),
        Request::CreateMaterial { name, sample, surface } => {
            let graph = new_material(&name, sample.as_deref(), surface)?;
            let changed = execute(editor, &Command::AddMaterial { graph: Box::new(graph) })?;
            Ok(json!({ "changed": changed, "index": last_material(editor) }))
        }
        Request::DuplicateMaterial { index, name } => {
            let changed = execute(editor, &Command::DuplicateMaterial { index, name })?;
            Ok(json!({ "changed": changed, "index": last_material(editor) }))
        }
        Request::DeleteMaterial { index } => changed(editor, &Command::DeleteMaterial { index }),
        Request::RenameMaterial { index, name } => changed(editor, &Command::RenameMaterial { index, name }),
        Request::SetMaterialCastsShadow { index, casts_shadow } => {
            changed(editor, &Command::SetCastsShadow { index, casts_shadow })
        }
        Request::SetMaterialSurface { index, surface } => {
            // An unknown or locked index keeps the matte base; the Command then reports why it is refused.
            let current = material(editor, index).map_or(SurfaceConstants::MATTE, MaterialGraph::surface_constants);
            let surface = merge_surface(current, surface);
            changed(editor, &Command::EditGraph { index, surface })
        }
        Request::EditMaterialGraph { index, edit } => {
            let edit: GraphEdit =
                serde_json::from_value(edit).map_err(|error| format!("`edit` is not a graph edit: {error}"))?;
            let graph = material(editor, index);
            let added_id = match edit {
                GraphEdit::AddNode { .. } => graph.map(|graph| graph.next_node_id().get()),
                GraphEdit::AddFrame { .. } => graph.map(|graph| graph.next_frame_id().get()),
                _ => None,
            };
            let changed = execute(editor, &Command::EditMaterial { index, edit })?;
            Ok(json!({ "changed": changed, "added_id": added_id }))
        }
        Request::AssignMaterial { colors, material } => {
            let ids = colors.into_iter().map(color_id).collect::<Result<Vec<_>, _>>()?;
            changed(editor, &Command::AssignGraphToColors { ids, graph: material })
        }
    }
}

fn changed(editor: &mut Editor, command: &Command) -> Reply {
    Ok(json!({ "changed": execute(editor, command)? }))
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

fn color_id(number: u8) -> Result<ColorId, String> {
    ColorId::new(number).ok_or_else(|| "color numbers start at 1".to_owned())
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
    let colors: Vec<Option<u8>> = positions
        .iter()
        .map(|position| shape.get(vec3(*position)).map(ColorId::get))
        .collect();
    Ok(json!({ "colors": colors }))
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
                    if let Some(color) = chunk.get(local) {
                        if voxels.len() >= limit {
                            truncated = true;
                            break 'chunks;
                        }
                        voxels.push(json!([position.x, position.y, position.z, color.get()]));
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
        .map(|(id, color)| {
            let [r, g, b] = color.color;
            json!({
                "color": id.get(),
                "hex": format!("#{r:02x}{g:02x}{b:02x}"),
                "rgb": color.color,
                "material": color.graph,
            })
        })
        .collect();
    json!({ "palette": entries })
}

fn material(editor: &Editor, index: u32) -> Option<&MaterialGraph> {
    editor.scene.material_graphs().get(usize::try_from(index).ok()?)
}

/// Index of the newest Material, which is where `AddMaterial` and `DuplicateMaterial` put theirs.
fn last_material(editor: &Editor) -> usize {
    editor.scene.material_graphs().len().saturating_sub(1)
}

/// Every Material with the Colors that use it. The graph is the same JSON `edit_material_graph` edits refer to.
fn get_materials(editor: &Editor) -> Value {
    let palette = editor.scene.palette();
    let mut materials = Vec::new();
    for (index, graph) in editor.scene.material_graphs().iter().enumerate() {
        let colors: Vec<u8> = palette
            .iter()
            .filter(|(_, color)| usize::try_from(color.graph).ok() == Some(index))
            .map(|(id, _)| id.get())
            .collect();
        materials.push(json!({
            "index": index,
            "name": graph.name,
            "locked": index == 0,
            "colors": colors,
            "surface": graph.surface_constants(),
            "graph": graph,
        }));
    }
    json!({ "materials": materials, "max_materials": MaterialGraph::MAX_PER_SCENE })
}

/// The node library: what `AddNode` accepts and what each socket and parameter number means.
fn material_node_kinds() -> Value {
    let mut kinds = Vec::new();
    for kind in GraphNodeKind::ALL {
        let spec = kind.spec();
        let inputs: Vec<Value> = spec
            .inputs
            .iter()
            .map(|input| json!({ "name": input.name, "type": format!("{:?}", input.ty), "default": input.default }))
            .collect();
        let outputs: Vec<Value> = spec
            .outputs
            .iter()
            .map(|output| json!({ "name": output.name, "type": format!("{:?}", output.ty) }))
            .collect();
        let params: Vec<Value> = spec
            .params
            .iter()
            .map(|param| json!({ "name": param.name, "kind": format!("{:?}", param.kind) }))
            .collect();
        kinds.push(json!({
            "kind": kind,
            "label": spec.label,
            "description": spec.description,
            "inputs": inputs,
            "outputs": outputs,
            "params": params,
        }));
    }
    json!({ "node_kinds": kinds })
}

/// The graph `create_material` adds: a named sample, a one-node graph of constants, or the blank starter.
fn new_material(name: &str, sample: Option<&str>, surface: Option<SurfaceParams>) -> Result<MaterialGraph, String> {
    let name: String = name.trim().chars().take(MATERIAL_NAME_MAX).collect();
    match (sample, surface) {
        (Some(_), Some(_)) => Err("pass `sample` or `surface`, not both".to_owned()),
        (Some(label), None) => {
            let Some(sample) = SampleMaterial::ALL
                .into_iter()
                .find(|sample| sample.label().eq_ignore_ascii_case(label.trim()))
            else {
                let known: Vec<&str> = SampleMaterial::ALL.iter().map(|sample| sample.label()).collect();
                return Err(format!("no sample called {label:?}; use one of {}", known.join(", ")));
            };
            let mut graph = sample.build();
            if !name.is_empty() {
                graph.name = name;
            }
            Ok(graph)
        }
        (None, Some(surface)) => Ok(MaterialGraph::from_constants(
            &name,
            merge_surface(SurfaceConstants::MATTE, surface),
        )),
        (None, None) => Ok(MaterialGraph::blank(&name)),
    }
}

fn merge_surface(base: SurfaceConstants, change: SurfaceParams) -> SurfaceConstants {
    SurfaceConstants {
        roughness: change.roughness.unwrap_or(base.roughness),
        metallic: change.metallic.unwrap_or(base.metallic),
        emission_color: change.emission_color.unwrap_or(base.emission_color),
        emission_strength: change.emission_strength.unwrap_or(base.emission_strength),
        transmission: change.transmission.unwrap_or(base.transmission),
        ior: change.ior.unwrap_or(base.ior),
    }
}

fn set_voxels(editor: &mut Editor, model: Option<u32>, writes: &[VoxelWrite]) -> Reply {
    let model = target_model(editor, model)?;
    let mut voxels = Vec::with_capacity(writes.len());
    for write in writes {
        let color = write.color.map(color_id).transpose()?;
        voxels.push((vec3(write.position), color));
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
    color: u8,
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
        color: color_id(color)?,
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
            .map(|(position, color)| VoxelWrite {
                position: *position,
                color: *color,
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
            focus_shape(&editor).get(IVec3::new(40, 1, 2)).map(ColorId::get),
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
        assert_eq!(placed.map(ColorId::get), Some(2));

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
        assert_eq!(voxels["colors"], json!([5, null]));

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
        let error = handle(&mut editor, Request::StartRender).unwrap_err();
        assert!(error.contains("Render mode"), "{error}");
    }

    fn create(editor: &mut Editor, sample: Option<&str>, surface: Option<SurfaceParams>) -> Reply {
        handle(
            editor,
            Request::CreateMaterial {
                name: "Lamp".to_owned(),
                sample: sample.map(str::to_owned),
                surface,
            },
        )
    }

    #[test]
    fn materials_are_created_assigned_and_listed_with_their_colors() {
        let mut editor = editor();
        let glow = SurfaceParams {
            emission_strength: Some(4.0),
            ..SurfaceParams::default()
        };
        let reply = create(&mut editor, None, Some(glow)).unwrap();
        assert_eq!(reply["index"], 1);
        let assign = Request::AssignMaterial {
            colors: vec![3, 4],
            material: 1,
        };
        assert_eq!(handle(&mut editor, assign).unwrap()["changed"], true);
        assert_eq!(editor.history.done().count(), 2, "each tool call is one undo step");

        let listed = handle(&mut editor, Request::GetMaterials).unwrap();
        let lamp = &listed["materials"][1];
        assert_eq!(lamp["name"], "Lamp");
        assert_eq!(lamp["colors"], json!([3, 4]));
        assert_eq!(lamp["surface"]["emission_strength"], 4.0);
        assert_eq!(lamp["surface"]["roughness"], json!(SurfaceConstants::MATTE.roughness));
        assert_eq!(listed["materials"][0]["locked"], true);
        let palette = handle(&mut editor, Request::GetPalette).unwrap();
        assert_eq!(palette["palette"][2]["material"], 1);

        let sample = create(&mut editor, Some("roof SLATE"), None).unwrap();
        assert_eq!(sample["index"], 2);
        assert_eq!(editor.scene.material_graphs()[2].name, "Lamp");
        let error = create(&mut editor, Some("Lava"), None).unwrap_err();
        assert!(error.contains("Roof slate"), "{error}");
        let error = create(&mut editor, Some("Glass"), Some(glow)).unwrap_err();
        assert!(error.contains("not both"), "{error}");

        editor.undo();
        editor.undo();
        let after_undo = handle(&mut editor, Request::GetMaterials).unwrap();
        assert_eq!(after_undo["materials"][1]["colors"], json!([]));
    }

    #[test]
    fn material_settings_change_one_step_at_a_time() {
        let mut editor = editor();
        create(&mut editor, None, None).unwrap();
        let surface = SurfaceParams {
            metallic: Some(1.0),
            ..SurfaceParams::default()
        };
        handle(&mut editor, Request::SetMaterialSurface { index: 1, surface }).unwrap();
        let graph = &editor.scene.material_graphs()[1];
        assert!((graph.surface_constants().metallic - 1.0).abs() < f32::EPSILON);
        handle(
            &mut editor,
            Request::RenameMaterial {
                index: 1,
                name: "Steel".to_owned(),
            },
        )
        .unwrap();
        handle(
            &mut editor,
            Request::SetMaterialCastsShadow {
                index: 1,
                casts_shadow: false,
            },
        )
        .unwrap();
        let copy = handle(
            &mut editor,
            Request::DuplicateMaterial {
                index: 1,
                name: String::new(),
            },
        )
        .unwrap();
        assert_eq!(copy["index"], 2);
        assert_eq!(editor.scene.material_graphs()[2].name, "Steel copy");
        assert!(!editor.scene.material_graphs()[2].casts_shadow);
        handle(&mut editor, Request::DeleteMaterial { index: 1 }).unwrap();
        assert_eq!(editor.scene.material_graphs().len(), 2);
        assert_eq!(editor.history.done().count(), 6);
    }

    #[test]
    fn graph_edits_arrive_as_json_and_report_new_ids() {
        let mut editor = editor();
        create(&mut editor, None, None).unwrap();
        let add = |kind: &str| Request::EditMaterialGraph {
            index: 1,
            edit: json!({ "AddNode": { "kind": kind, "position": [0.0, 0.0] } }),
        };
        let expected = editor.scene.material_graphs()[1].next_node_id().get();
        let reply = handle(&mut editor, add("Noise")).unwrap();
        assert_eq!(reply["added_id"], expected);
        let node = reply["added_id"].clone();
        let set = Request::EditMaterialGraph {
            index: 1,
            edit: json!({ "SetParam": { "node": node, "param": 0, "value": { "Float": 2.0 } } }),
        };
        assert_eq!(handle(&mut editor, set).unwrap()["changed"], true);

        let error = handle(&mut editor, add("Teapot")).unwrap_err();
        assert!(error.contains("not a graph edit"), "{error}");
        let locked = Request::EditMaterialGraph {
            index: 0,
            edit: json!({ "AddNode": { "kind": "Noise", "position": [0.0, 0.0] } }),
        };
        assert!(handle(&mut editor, locked).is_err(), "the Default material is locked");
        assert_eq!(editor.history.done().count(), 3);

        let kinds = handle(&mut editor, Request::GetMaterialNodeKinds).unwrap();
        let noise = kinds["node_kinds"]
            .as_array()
            .unwrap()
            .iter()
            .find(|kind| kind["kind"] == "Noise")
            .unwrap();
        assert_eq!(noise["params"][0]["name"], "Scale", "SetParam 0 above is the Scale");
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
                color: 3,
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
                id: 2,
                color: [1, 2, 3],
            },
        )
        .unwrap();
        assert!(editor.take_palette_dirty());
        assert!(!editor.take_palette_dirty());
        editor.undo();
        assert!(editor.take_palette_dirty(), "undo changes the palette too");
        let id = ColorId::new(2).unwrap();
        assert_ne!(editor.scene.palette().get(id).map(|color| color.color), Some([1, 2, 3]));
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
