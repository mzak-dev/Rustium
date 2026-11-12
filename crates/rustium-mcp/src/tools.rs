use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{ServerCapabilities, ServerConfig};
use rmcp::{ServerHandler, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::bridge::{Bridge, BrushTool, FaceDirection, Request, SurfaceParams, VoxelWrite};

/// Most voxels `get_region` returns when the client does not say.
const DEFAULT_REGION_LIMIT: usize = 10_000;
/// Hard cap on what `get_region` returns.
const MAX_REGION_LIMIT: usize = 100_000;

const INSTRUCTIONS: &str = "Rustium is a voxel editor. Voxel positions are integer (x, y, z) with Y up, in the cells of a Model (a Model node of the scene tree; omit `model` to use the Focus Model); \
colors are palette numbers 1-255. Each color uses a Material (a node graph for roughness, metal, glow and glass; \
see `get_materials`). Every edit is applied live in the editor and is one undo step for the \
user. Tools are grouped by permission; a tool whose permission the user switched off returns an error.";

/// The MCP tools of the editor; each one becomes a [`Request`] for the UI thread.
#[derive(Debug, Clone)]
pub struct Tools {
    bridge: Bridge,
    tool_router: ToolRouter<Self>,
}

/// Arguments of `get_voxels`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct GetVoxelsParams {
    /// Node id of the Model to read or edit (see `scene_info`); defaults to the Focus Model.
    model: Option<u32>,
    /// Positions to look up, each `[x, y, z]`.
    positions: Vec<[i32; 3]>,
}

/// Arguments of `get_region`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct GetRegionParams {
    /// Node id of the Model to read or edit (see `scene_info`); defaults to the Focus Model.
    model: Option<u32>,
    /// Lowest corner `[x, y, z]`, inclusive.
    min: [i32; 3],
    /// Highest corner `[x, y, z]`, inclusive.
    max: [i32; 3],
    /// Most voxels to return (default 10000, at most 100000).
    limit: Option<usize>,
}

/// One voxel for `set_voxels`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct VoxelParam {
    /// X position.
    x: i32,
    /// Y position (up).
    y: i32,
    /// Z position.
    z: i32,
    /// Palette color 1-255; omit or 0 to erase the voxel.
    color: Option<u8>,
}

/// Arguments of `set_voxels`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct SetVoxelsParams {
    /// Node id of the Model to read or edit (see `scene_info`); defaults to the Focus Model.
    model: Option<u32>,
    /// The voxels to set; the whole batch is one undo step and is rejected if any entry is invalid.
    voxels: Vec<VoxelParam>,
}

/// Arguments of `erase_voxels`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct EraseVoxelsParams {
    /// Node id of the Model to read or edit (see `scene_info`); defaults to the Focus Model.
    model: Option<u32>,
    /// Positions to erase, each `[x, y, z]`.
    positions: Vec<[i32; 3]>,
}

/// Arguments of `apply_brush`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ApplyBrushParams {
    /// Node id of the Model to read or edit (see `scene_info`); defaults to the Focus Model.
    model: Option<u32>,
    /// attach adds voxels against the hit face, erase removes, paint recolors existing voxels.
    tool: BrushTool,
    /// X of the hit voxel.
    x: i32,
    /// Y of the hit voxel.
    y: i32,
    /// Z of the hit voxel.
    z: i32,
    /// Face of the hit voxel, e.g. "+y" for its top. Attach places the new voxels on this face.
    face: FaceDirection,
    /// Cube edge length in voxels (default 1, at most 64).
    size: Option<u32>,
    /// Palette color 1-255 for attach and paint (default 1).
    color: Option<u8>,
    /// Treat the hit as the ground plane under (x, z) instead of a voxel; set y to -1 and face to "+y".
    ground: Option<bool>,
}

/// Arguments of `set_palette_color`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct SetPaletteColorParams {
    /// Palette entry 1-255.
    id: u8,
    /// New color as `[r, g, b]`, 0-255 each.
    color: [u8; 3],
}

/// Arguments of `run_script`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct RunScriptParams {
    /// Node id of the Model to read or edit (see `scene_info`); defaults to the Focus Model.
    model: Option<u32>,
    /// Luau source. API: `scene.get(x,y,z)`, `scene.set(x,y,z,color)`, `scene.erase(x,y,z)`,
    /// `scene.fill(x1,y1,z1,x2,y2,z2,color)`, `palette.len()`, `palette.color(n)`, `palette.nearest(r,g,b)`
    /// and `print(...)`.
    source: String,
}

/// Arguments of `create_material`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreateMaterialParams {
    /// Name of the new Material.
    name: String,
    /// Start from a built-in graph: "Roof slate", "Plaster", "Grass", "Wood", "Glass", "Stone", "Brass" or
    /// "Lamp glow". Cannot be combined with `surface`.
    sample: Option<String>,
    /// Make a one-node Material with these Surface constants (missing ones are matte defaults). Without `sample`
    /// or `surface` the Material starts as the Voxel color wired into a Surface output.
    surface: Option<SurfaceParams>,
}

/// Arguments of `duplicate_material`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct DuplicateMaterialParams {
    /// Index of the Material to copy (see `get_materials`).
    index: u32,
    /// Name of the copy; omit to name it "NAME copy".
    name: Option<String>,
}

/// Arguments of tools that take only a Material.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct MaterialParams {
    /// Index of the Material (see `get_materials`); the Default material (0) cannot be changed.
    index: u32,
}

/// Arguments of `rename_material`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct RenameMaterialParams {
    /// Index of the Material (see `get_materials`).
    index: u32,
    /// The new name.
    name: String,
}

/// Arguments of `set_material_casts_shadow`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct SetCastsShadowParams {
    /// Index of the Material (see `get_materials`).
    index: u32,
    /// Whether voxels with this Material block light.
    casts_shadow: bool,
}

/// Arguments of `set_material_surface`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct SetMaterialSurfaceParams {
    /// Index of the Material (see `get_materials`).
    index: u32,
    /// The constants to change; omitted ones keep their value. Values are clamped to their ranges.
    surface: SurfaceParams,
}

/// Arguments of `edit_material_graph`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct EditMaterialGraphParams {
    /// Index of the Material (see `get_materials`).
    index: u32,
    /// Exactly one edit, as an object with one key. Node and frame ids come from `get_materials`; socket and
    /// parameter numbers index the lists of `get_material_node_kinds`.
    /// `{"AddNode":{"kind":"Noise","position":[x,y]}}`, `{"RemoveNodes":{"nodes":[id]}}`,
    /// `{"MoveNodes":{"nodes":[id],"frames":[id],"delta":[dx,dy]}}`,
    /// `{"Connect":{"from":{"node":id,"socket":n},"to":{"node":id,"socket":n}}}`,
    /// `{"Disconnect":{"to":{"node":id,"socket":n}}}`,
    /// `{"SetInput":{"node":id,"input":n,"value":V}}` with V one of `{"Float":0.5}`, `{"Color":[r,g,b]}` (sRGB 0-1),
    /// `{"Vector":[x,y,z]}`, `"Position"`, `"Normal"`, `"VoxelColor"`,
    /// `{"SetParam":{"node":id,"param":n,"value":P}}` with P one of `{"Float":1.0}`, `{"Int":3}`, `{"Bool":true}`,
    /// `{"Choice":0}`, `{"Range":[low,high]}`, `{"Color":[r,g,b]}`, `{"Stops":[{"position":0.0,"color":[r,g,b]}]}`,
    /// `{"RenameNode":{"node":id,"name":"..."}}`, `{"SetBypass":{"node":id,"bypass":true}}`,
    /// `{"AddFrame":{"label":"...","position":[x,y],"size":[w,h]}}`,
    /// `{"SetFrame":{"frame":id,"label":"...","position":[x,y],"size":[w,h]}}`, `{"RemoveFrame":{"frame":id}}`.
    edit: serde_json::Value,
}

/// Arguments of `assign_material`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct AssignMaterialParams {
    /// Palette entries 1-255 whose voxels should use the Material.
    colors: Vec<u8>,
    /// Index of the Material (see `get_materials`); 0 is the Default material.
    material: u32,
}

#[tool_router(router = tool_router)]
impl Tools {
    /// Tools backed by `bridge`.
    pub fn new(bridge: Bridge) -> Self {
        Self {
            bridge,
            tool_router: Self::tool_router(),
        }
    }

    async fn ask(&self, request: Request) -> Result<String, String> {
        self.bridge.call(request).await.map(|value| value.to_string())
    }

    #[tool(
        description = "Voxel size in metres, the node tree (Groups and Models with ids, names, hidden/locked, volume), the Focus Model, palette size and undo state."
    )]
    async fn scene_info(&self) -> Result<String, String> {
        self.ask(Request::SceneInfo).await
    }

    #[tool(
        description = "Color number (or null when empty) at each of the given positions of a Model (cell coordinates)."
    )]
    async fn get_voxels(&self, Parameters(params): Parameters<GetVoxelsParams>) -> Result<String, String> {
        self.ask(Request::GetVoxels {
            model: params.model,
            positions: params.positions,
        })
        .await
    }

    #[tool(description = "Every filled voxel of a Model inside a box, as [x, y, z, color] lists (cell coordinates).")]
    async fn get_region(&self, Parameters(params): Parameters<GetRegionParams>) -> Result<String, String> {
        let limit = params.limit.unwrap_or(DEFAULT_REGION_LIMIT).min(MAX_REGION_LIMIT);
        self.ask(Request::GetRegion {
            model: params.model,
            min: params.min,
            max: params.max,
            limit,
        })
        .await
    }

    #[tool(description = "The palette: id, hex color and rgb of every entry.")]
    async fn get_palette(&self) -> Result<String, String> {
        self.ask(Request::GetPalette).await
    }

    #[tool(description = "Sets voxels at explicit positions as one undoable step. Color 0 or omitted erases.")]
    async fn set_voxels(&self, Parameters(params): Parameters<SetVoxelsParams>) -> Result<String, String> {
        let model = params.model;
        let voxels = params
            .voxels
            .into_iter()
            .map(|voxel| VoxelWrite {
                position: [voxel.x, voxel.y, voxel.z],
                color: voxel.color.filter(|color| *color != 0),
            })
            .collect();
        self.ask(Request::SetVoxels { model, voxels }).await
    }

    #[tool(description = "Erases the voxels at the given positions as one undoable step.")]
    async fn erase_voxels(&self, Parameters(params): Parameters<EraseVoxelsParams>) -> Result<String, String> {
        let model = params.model;
        let voxels = params
            .positions
            .into_iter()
            .map(|position| VoxelWrite { position, color: None })
            .collect();
        self.ask(Request::SetVoxels { model, voxels }).await
    }

    #[tool(
        description = "Applies the attach, erase or paint brush at a hit voxel and face, like a click in the viewport."
    )]
    async fn apply_brush(&self, Parameters(params): Parameters<ApplyBrushParams>) -> Result<String, String> {
        self.ask(Request::ApplyBrush {
            model: params.model,
            tool: params.tool,
            size: params.size.unwrap_or(1),
            voxel: [params.x, params.y, params.z],
            face: params.face,
            ground: params.ground.unwrap_or(false),
            color: params.color.unwrap_or(1),
        })
        .await
    }

    #[tool(description = "Changes the color of one palette entry (undoable).")]
    async fn set_palette_color(&self, Parameters(params): Parameters<SetPaletteColorParams>) -> Result<String, String> {
        self.ask(Request::SetPaletteColor {
            id: params.id,
            color: params.color,
        })
        .await
    }

    #[tool(
        description = "Runs a sandboxed Luau script against the scene as one undoable step; returns what it printed."
    )]
    async fn run_script(&self, Parameters(params): Parameters<RunScriptParams>) -> Result<String, String> {
        self.ask(Request::RunScript {
            model: params.model,
            source: params.source,
        })
        .await
    }

    #[tool(
        description = "Starts a render of the scene. Not wired to MCP yet: it always fails; the user starts renders in Render mode."
    )]
    async fn start_render(&self) -> Result<String, String> {
        self.ask(Request::StartRender).await
    }

    #[tool(
        description = "Every Material by index: name, the palette colors that use it, its Surface constants and its node graph (nodes with ids, inputs and params; links; frames). Index 0 is the locked Default material."
    )]
    async fn get_materials(&self) -> Result<String, String> {
        self.ask(Request::GetMaterials).await
    }

    #[tool(
        description = "Every node kind a Material graph can hold, with its inputs, outputs and parameters in socket order. Use it to build `edit_material_graph` edits."
    )]
    async fn get_material_node_kinds(&self) -> Result<String, String> {
        self.ask(Request::GetMaterialNodeKinds).await
    }

    #[tool(description = "Appends a Material as one undoable step and returns its index.")]
    async fn create_material(&self, Parameters(params): Parameters<CreateMaterialParams>) -> Result<String, String> {
        self.ask(Request::CreateMaterial {
            name: params.name,
            sample: params.sample,
            surface: params.surface,
        })
        .await
    }

    #[tool(description = "Copies a Material and appends the copy (undoable); returns the copy's index.")]
    async fn duplicate_material(
        &self,
        Parameters(params): Parameters<DuplicateMaterialParams>,
    ) -> Result<String, String> {
        self.ask(Request::DuplicateMaterial {
            index: params.index,
            name: params.name.unwrap_or_default(),
        })
        .await
    }

    #[tool(
        description = "Deletes a Material (undoable). Its colors fall back to the Default material and later indices shift down by one."
    )]
    async fn delete_material(&self, Parameters(params): Parameters<MaterialParams>) -> Result<String, String> {
        self.ask(Request::DeleteMaterial { index: params.index }).await
    }

    #[tool(description = "Renames a Material (undoable).")]
    async fn rename_material(&self, Parameters(params): Parameters<RenameMaterialParams>) -> Result<String, String> {
        self.ask(Request::RenameMaterial {
            index: params.index,
            name: params.name,
        })
        .await
    }

    #[tool(description = "Turns \"Casts shadow\" of a Material on or off (undoable).")]
    async fn set_material_casts_shadow(
        &self,
        Parameters(params): Parameters<SetCastsShadowParams>,
    ) -> Result<String, String> {
        self.ask(Request::SetMaterialCastsShadow {
            index: params.index,
            casts_shadow: params.casts_shadow,
        })
        .await
    }

    #[tool(
        description = "Sets roughness, metallic, emission, transmission or IOR on a Material's Surface output as one undoable step. Linked inputs stay linked."
    )]
    async fn set_material_surface(
        &self,
        Parameters(params): Parameters<SetMaterialSurfaceParams>,
    ) -> Result<String, String> {
        self.ask(Request::SetMaterialSurface {
            index: params.index,
            surface: params.surface,
        })
        .await
    }

    #[tool(
        description = "Applies one change to a Material's node graph (add, remove, move, connect, set an input or parameter, frames) as one undoable step. Returns the id of an added node or frame."
    )]
    async fn edit_material_graph(
        &self,
        Parameters(params): Parameters<EditMaterialGraphParams>,
    ) -> Result<String, String> {
        self.ask(Request::EditMaterialGraph {
            index: params.index,
            edit: params.edit,
        })
        .await
    }

    #[tool(description = "Points palette colors at a Material as one undoable step.")]
    async fn assign_material(&self, Parameters(params): Parameters<AssignMaterialParams>) -> Result<String, String> {
        self.ask(Request::AssignMaterial {
            colors: params.colors,
            material: params.material,
        })
        .await
    }
}

#[expect(
    clippy::unused_async_trait_impl,
    reason = "the tool_handler macro generates async methods"
)]
#[tool_handler(router = self.tool_router)]
impl ServerHandler for Tools {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build()).with_instructions(INSTRUCTIONS)
    }
}
