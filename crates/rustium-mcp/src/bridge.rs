use std::fmt;
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender, TrySendError};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use crate::Waker;
use crate::permission::{Permission, SharedPermissions};

/// Voxel Tool for [`Request::ApplyBrush`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum BrushTool {
    /// Adds voxels against the hit face.
    Attach,
    /// Removes voxels around the hit voxel.
    Erase,
    /// Recolors existing voxels around the hit voxel.
    Paint,
}

/// A face of a voxel, named by the axis it points along.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
pub enum FaceDirection {
    /// Faces +X.
    #[serde(rename = "+x")]
    PosX,
    /// Faces −X.
    #[serde(rename = "-x")]
    NegX,
    /// Faces +Y (up).
    #[serde(rename = "+y")]
    PosY,
    /// Faces −Y (down).
    #[serde(rename = "-y")]
    NegY,
    /// Faces +Z.
    #[serde(rename = "+z")]
    PosZ,
    /// Faces −Z.
    #[serde(rename = "-z")]
    NegZ,
}

/// One entry of a [`Request::SetVoxels`] batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VoxelWrite {
    /// Voxel position.
    pub position: [i32; 3],
    /// Color number 1–255, or `None` to erase.
    pub color: Option<u8>,
}

/// Surface constants of a Material; a missing field keeps the current value (a new Material starts matte).
#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SurfaceParams {
    /// 0 (mirror) to 1 (matte).
    pub roughness: Option<f32>,
    /// 0 for paint or plastic up to 1 for bare metal.
    pub metallic: Option<f32>,
    /// Color of the emitted light as `[r, g, b]`, 0-255 each.
    pub emission_color: Option<[u8; 3]>,
    /// Brightness of the emitted light, 0 (no glow) to 32.
    pub emission_strength: Option<f32>,
    /// 0 (opaque) to 1 (clear glass).
    pub transmission: Option<f32>,
    /// Index of refraction, 1 to 3, used when transmission is above 0.
    pub ior: Option<f32>,
}

/// What an MCP client asks the editor to do. The UI thread answers every request; the server never touches
/// the Scene itself.
#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    /// Voxel size, the Node tree, the Focus, counts and History state.
    SceneInfo,
    /// The Color at each position of a Model.
    GetVoxels {
        /// The Model's node id; `None` means the Focus Model.
        model: Option<u32>,
        /// Positions to look up, in the Model's cells.
        positions: Vec<[i32; 3]>,
    },
    /// Every filled voxel of a Model inside a box.
    GetRegion {
        /// The Model's node id; `None` means the Focus Model.
        model: Option<u32>,
        /// Lowest corner, inclusive.
        min: [i32; 3],
        /// Highest corner, inclusive.
        max: [i32; 3],
        /// Most voxels to return.
        limit: usize,
    },
    /// Every Palette entry.
    GetPalette,
    /// Sets or erases voxels in one undoable step.
    SetVoxels {
        /// The Model's node id; `None` means the Focus Model.
        model: Option<u32>,
        /// The voxels to change, in the Model's cells.
        voxels: Vec<VoxelWrite>,
    },
    /// Applies a Brush as if the user had clicked.
    ApplyBrush {
        /// The Model's node id; `None` means the Focus Model.
        model: Option<u32>,
        /// Which voxel Tool.
        tool: BrushTool,
        /// Edge length of the cube footprint.
        size: u32,
        /// The voxel that was hit (for Attach, the voxel the new ones are placed against).
        voxel: [i32; 3],
        /// The face that was hit.
        face: FaceDirection,
        /// Treat the hit as the ground plane instead of a voxel.
        ground: bool,
        /// Color for Attach and Paint.
        color: u8,
    },
    /// Changes one Palette color.
    SetPaletteColor {
        /// Palette entry, 1–255.
        id: u8,
        /// New color as 8-bit sRGB.
        color: [u8; 3],
    },
    /// Runs a Luau script.
    RunScript {
        /// The Model the script edits; `None` means the Focus Model.
        model: Option<u32>,
        /// The script text.
        source: String,
    },
    /// Starts a render.
    StartRender,
    /// Every Material with the Colors that use it and its node graph.
    GetMaterials,
    /// Every node kind a Material graph can hold, with its sockets and parameters.
    GetMaterialNodeKinds,
    /// Appends a Material.
    CreateMaterial {
        /// Its name.
        name: String,
        /// Label of a built-in sample graph to start from, for example "Roof slate".
        sample: Option<String>,
        /// Constants for a one-node Material; `None` with no sample starts from the Voxel color.
        surface: Option<SurfaceParams>,
    },
    /// Copies a Material and appends the copy.
    DuplicateMaterial {
        /// Index of the Material to copy.
        index: u32,
        /// Name of the copy; empty names it after the original.
        name: String,
    },
    /// Deletes a Material; its Colors fall back to the Default material.
    DeleteMaterial {
        /// Index of the Material.
        index: u32,
    },
    /// Renames a Material.
    RenameMaterial {
        /// Index of the Material.
        index: u32,
        /// The new name.
        name: String,
    },
    /// Turns "Casts shadow" of a Material on or off.
    SetMaterialCastsShadow {
        /// Index of the Material.
        index: u32,
        /// Whether its voxels block light.
        casts_shadow: bool,
    },
    /// Changes the Surface output's constants of a Material.
    SetMaterialSurface {
        /// Index of the Material.
        index: u32,
        /// The constants to change.
        surface: SurfaceParams,
    },
    /// Applies one change to the node graph of a Material.
    EditMaterialGraph {
        /// Index of the Material.
        index: u32,
        /// A `rustium_core::GraphEdit` as JSON; the editor parses it.
        edit: Value,
    },
    /// Points Palette entries at a Material.
    AssignMaterial {
        /// Palette entries, 1–255.
        colors: Vec<u8>,
        /// Index of the Material; 0 is the Default material.
        material: u32,
    },
}

impl Request {
    /// The permission a client needs to send this request.
    pub fn permission(&self) -> Permission {
        match self {
            Self::SceneInfo
            | Self::GetVoxels { .. }
            | Self::GetRegion { .. }
            | Self::GetPalette
            | Self::GetMaterials
            | Self::GetMaterialNodeKinds => Permission::ReadModel,
            Self::SetVoxels { .. } | Self::ApplyBrush { .. } => Permission::EditVoxels,
            Self::SetPaletteColor { .. }
            | Self::CreateMaterial { .. }
            | Self::DuplicateMaterial { .. }
            | Self::DeleteMaterial { .. }
            | Self::RenameMaterial { .. }
            | Self::SetMaterialCastsShadow { .. }
            | Self::SetMaterialSurface { .. }
            | Self::EditMaterialGraph { .. }
            | Self::AssignMaterial { .. } => Permission::EditPalette,
            Self::RunScript { .. } => Permission::RunScripts,
            Self::StartRender => Permission::StartRenders,
        }
    }
}

/// The UI thread's answer: a JSON value for the client, or a message that explains why the request failed.
pub type Reply = Result<Value, String>;

/// A request on its way to the UI thread, with the channel for the answer.
#[derive(Debug)]
pub struct Envelope {
    /// What to do.
    pub request: Request,
    /// Where to send the answer. A dropped receiver means the client gave up; ignore the send error.
    pub reply: Sender<Reply>,
}

/// How long a client waits for the UI thread before the call fails.
const REPLY_TIMEOUT: Duration = Duration::from_secs(30);

/// Requests waiting for the UI thread beyond this number are refused with a "busy" error.
const QUEUE_CAPACITY: usize = 64;

/// The server's only link to the editor: permissions plus the channel to the UI thread.
///
/// This is the one place where async code meets the UI thread. A call checks the permission, queues an
/// [`Envelope`], wakes the event loop and waits for the reply on a blocking thread.
#[derive(Clone)]
pub struct Bridge {
    sender: Sender<Envelope>,
    waker: Waker,
    permissions: SharedPermissions,
}

impl fmt::Debug for Bridge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Bridge")
            .field("permissions", &self.permissions)
            .finish_non_exhaustive()
    }
}

/// Creates the channel between the server and the UI thread.
///
/// Keep the [`Receiver`] on the UI thread and drain it whenever `waker` fires; hand the [`Bridge`] to
/// [`Server::start`](crate::Server::start).
pub fn channel(permissions: SharedPermissions, waker: Waker) -> (Bridge, Receiver<Envelope>) {
    let (sender, receiver) = crossbeam_channel::bounded(QUEUE_CAPACITY);
    (
        Bridge {
            sender,
            waker,
            permissions,
        },
        receiver,
    )
}

impl Bridge {
    /// Sends `request` to the UI thread and waits for its answer.
    ///
    /// # Errors
    ///
    /// A message for the MCP client when the permission is off, the editor is busy or closing, or does not
    /// answer in time; otherwise whatever error the UI thread reports.
    pub async fn call(&self, request: Request) -> Reply {
        let permission = request.permission();
        if !self.permissions.allows(permission) {
            return Err(format!(
                "The \"{}\" permission is turned off in Rustium. Ask the user to switch it on in the MCP tab.",
                permission.label()
            ));
        }
        let (reply, answer) = crossbeam_channel::bounded(1);
        self.sender
            .try_send(Envelope { request, reply })
            .map_err(|error| match error {
                TrySendError::Full(_) => "Rustium is busy with other requests; try again shortly.".to_owned(),
                TrySendError::Disconnected(_) => "Rustium is shutting down.".to_owned(),
            })?;
        (self.waker)();
        // The one sync/async boundary: the UI thread replies on a crossbeam channel, so wait for it on the
        // blocking pool instead of stalling the server's executor.
        let waited = tokio::task::spawn_blocking(move || answer.recv_timeout(REPLY_TIMEOUT)).await;
        match waited {
            Ok(Ok(reply)) => reply,
            Ok(Err(_)) => Err("Rustium did not answer in time.".to_owned()),
            Err(error) => Err(format!("internal error while waiting for Rustium: {error}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;

    use super::*;
    use crate::permission::Permissions;

    fn denied_everything() -> SharedPermissions {
        SharedPermissions::new(Permissions {
            read_model: false,
            edit_voxels: false,
            edit_palette: false,
            run_scripts: false,
            start_renders: false,
        })
    }

    #[tokio::test]
    async fn disabled_permission_is_refused_without_reaching_the_ui_thread() {
        let (bridge, receiver) = channel(denied_everything(), Arc::new(|| {}));
        let error = bridge.call(Request::GetPalette).await.unwrap_err();
        assert!(error.contains("Read model") && error.contains("turned off"), "{error}");
        assert!(receiver.try_recv().is_err());
    }

    #[tokio::test]
    async fn every_request_maps_to_a_permission() {
        let cases = [
            (Request::SceneInfo, Permission::ReadModel),
            (Request::GetPalette, Permission::ReadModel),
            (
                Request::SetVoxels {
                    model: None,
                    voxels: vec![],
                },
                Permission::EditVoxels,
            ),
            (
                Request::SetPaletteColor { id: 1, color: [0; 3] },
                Permission::EditPalette,
            ),
            (
                Request::RunScript {
                    model: None,
                    source: String::new(),
                },
                Permission::RunScripts,
            ),
            (Request::StartRender, Permission::StartRenders),
            (Request::GetMaterials, Permission::ReadModel),
            (Request::DeleteMaterial { index: 1 }, Permission::EditPalette),
        ];
        for (request, permission) in cases {
            assert_eq!(request.permission(), permission);
        }
    }

    #[tokio::test]
    async fn a_reply_from_the_ui_thread_reaches_the_caller_and_the_waker_fires() {
        let woken = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let waker: Waker = {
            let woken = Arc::clone(&woken);
            Arc::new(move || {
                woken.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            })
        };
        let (bridge, receiver) = channel(SharedPermissions::new(Permissions::default()), waker);
        let ui = std::thread::spawn(move || {
            let envelope = receiver.recv().unwrap();
            assert_eq!(envelope.request, Request::SceneInfo);
            envelope.reply.send(Ok(json!({"ok": true}))).unwrap();
        });
        let value = bridge.call(Request::SceneInfo).await.unwrap();
        assert_eq!(value, json!({"ok": true}));
        assert_eq!(woken.load(std::sync::atomic::Ordering::SeqCst), 1);
        ui.join().unwrap();
    }

    #[tokio::test]
    async fn a_closed_editor_is_reported() {
        let (bridge, receiver) = channel(SharedPermissions::new(Permissions::default()), Arc::new(|| {}));
        drop(receiver);
        let error = bridge.call(Request::SceneInfo).await.unwrap_err();
        assert!(error.contains("shutting down"), "{error}");
    }

    #[tokio::test]
    async fn errors_from_the_ui_thread_are_passed_on() {
        let (bridge, receiver) = channel(SharedPermissions::new(Permissions::default()), Arc::new(|| {}));
        let ui = std::thread::spawn(move || {
            receiver
                .recv()
                .unwrap()
                .reply
                .send(Err("color 9 is not in the palette".to_owned()))
                .unwrap();
        });
        let error = bridge.call(Request::SceneInfo).await.unwrap_err();
        assert!(error.contains("color 9"));
        ui.join().unwrap();
    }
}
