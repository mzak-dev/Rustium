use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{ServerCapabilities, ServerConfig};
use rmcp::{ServerHandler, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::bridge::{Bridge, BrushTool, FaceDirection, Request, VoxelWrite};

/// Most voxels `get_region` returns when the client does not say.
const DEFAULT_REGION_LIMIT: usize = 10_000;
/// Hard cap on what `get_region` returns.
const MAX_REGION_LIMIT: usize = 100_000;

const INSTRUCTIONS: &str = "Rustium is a voxel editor. Voxel positions are integer (x, y, z) with Y up, in the cells of a Model (a Model node of the scene tree; omit `model` to use the Focus Model); \
colors are palette numbers 1-255. Every edit is applied live in the editor and is one undo step for the \
user. Tools are grouped by permission; a tool whose permission the user switched off returns an error.";

/// The MCP tools of the editor; each one becomes a [`Request`] for the UI thread.
#[derive(Debug, Clone)]
pub struct Tools {
    bridge: Bridge,
    tool_router: ToolRouter<Self>,
}

/// Arguments of `get_voxels`.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct GetVoxelsParams {
    /// Node id of the Model to read or edit (see `scene_info`); defaults to the Focus Model.
    model: Option<u32>,
    /// Positions to look up, each `[x, y, z]`.
    positions: Vec<[i32; 3]>,
}

/// Arguments of `get_region`.
#[derive(Debug, Deserialize, JsonSchema)]
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
pub(crate) struct SetVoxelsParams {
    /// Node id of the Model to read or edit (see `scene_info`); defaults to the Focus Model.
    model: Option<u32>,
    /// The voxels to set; the whole batch is one undo step and is rejected if any entry is invalid.
    voxels: Vec<VoxelParam>,
}

/// Arguments of `erase_voxels`.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct EraseVoxelsParams {
    /// Node id of the Model to read or edit (see `scene_info`); defaults to the Focus Model.
    model: Option<u32>,
    /// Positions to erase, each `[x, y, z]`.
    positions: Vec<[i32; 3]>,
}

/// Arguments of `apply_brush`.
#[derive(Debug, Deserialize, JsonSchema)]
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
pub(crate) struct SetPaletteColorParams {
    /// Palette entry 1-255.
    id: u8,
    /// New color as `[r, g, b]`, 0-255 each.
    color: [u8; 3],
}

/// Arguments of `run_script`.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct RunScriptParams {
    /// Node id of the Model to read or edit (see `scene_info`); defaults to the Focus Model.
    model: Option<u32>,
    /// Luau source. API: `scene.get(x,y,z)`, `scene.set(x,y,z,color)`, `scene.erase(x,y,z)`,
    /// `scene.fill(x1,y1,z1,x2,y2,z2,color)`, `palette.len()`, `palette.color(n)`, `palette.nearest(r,g,b)`
    /// and `print(...)`.
    source: String,
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

    #[tool(description = "Starts a render of the scene. Not available until the path tracer milestone (M3).")]
    async fn start_render(&self) -> Result<String, String> {
        self.ask(Request::StartRender).await
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
