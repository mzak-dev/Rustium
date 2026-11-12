//! The editor's side of MCP: settings, server lifecycle and the request queue drained on the UI thread.
//!
//! The server itself lives in `rustium-mcp` and never touches the Scene; see `ARCHITECTURE.md` for the
//! threading model. A browser cannot listen on a port: on `wasm32` the settings stay (so a settings file means the
//! same everywhere) and [`McpHost`] is a host that is always off.

#[cfg(not(target_arch = "wasm32"))]
use std::io;
use std::net::SocketAddr;

#[cfg(not(target_arch = "wasm32"))]
use color_eyre::eyre::{Result, WrapErr};
#[cfg(not(target_arch = "wasm32"))]
use crossbeam_channel::Receiver;
#[cfg(not(target_arch = "wasm32"))]
use rustium_mcp::{Bridge, Envelope, Server, SharedPermissions};
use rustium_mcp::{DEFAULT_PORT, Permission, Permissions, Waker};
use serde::{Deserialize, Serialize};
use serde_json::json;
#[cfg(not(target_arch = "wasm32"))]
use tracing::{info, warn};

#[cfg(not(target_arch = "wasm32"))]
use crate::automation;
use crate::editor::Editor;
use crate::i18n::I18n;
#[cfg(not(target_arch = "wasm32"))]
use crate::settings::Settings;

/// How the user says they connect, which decides what "Copy client config" produces. The editor always serves
/// HTTP; `rustium --mcp-stdio` is a bridge to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub(crate) enum McpTransport {
    /// Clients spawn `rustium --mcp-stdio`.
    Stdio,
    /// Clients connect to the local URL.
    #[default]
    Http,
}

/// MCP section of the settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct McpSettings {
    pub(crate) enabled: bool,
    pub(crate) transport: McpTransport,
    pub(crate) port: u16,
    pub(crate) permissions: Permissions,
}

impl Default for McpSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            transport: McpTransport::Http,
            port: DEFAULT_PORT,
            permissions: Permissions::default(),
        }
    }
}

impl McpSettings {
    /// The URL HTTP clients connect to.
    pub(crate) fn url(&self) -> String {
        format!("http://127.0.0.1:{}/mcp", self.port)
    }

    /// A JSON snippet for the `mcpServers` section of an MCP client's configuration.
    pub(crate) fn client_config(&self) -> String {
        let server = match self.transport {
            McpTransport::Http => json!({ "type": "http", "url": self.url() }),
            McpTransport::Stdio => {
                let command =
                    std::env::current_exe().map_or_else(|_| "rustium".to_owned(), |path| path.display().to_string());
                json!({ "command": command, "args": ["--mcp-stdio"] })
            }
        };
        serde_json::to_string_pretty(&json!({ "mcpServers": { "rustium": server } })).unwrap_or_default()
    }
}

/// What the MCP tab shows next to the switch.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(
    target_arch = "wasm32",
    expect(dead_code, reason = "the browser's host is always off")
)]
pub(crate) enum McpStatus {
    Off,
    Listening(SocketAddr),
    Failed(String),
}

impl McpStatus {
    /// The line under "MCP server" in the MCP tab.
    pub(crate) fn text(&self, i18n: &I18n) -> String {
        match self {
            Self::Off => i18n.tr("mcp-off"),
            Self::Listening(address) => format!("{} {address}", i18n.tr("mcp-listening")),
            Self::Failed(_) => i18n.tr("mcp-failed"),
        }
    }
}

/// The message id of a permission's label.
pub(crate) fn permission_key(permission: Permission) -> &'static str {
    match permission {
        Permission::ReadModel => "mcp-read-model",
        Permission::EditVoxels => "mcp-edit-voxels",
        Permission::EditPalette => "mcp-edit-palette",
        Permission::RunScripts => "mcp-run-scripts",
        Permission::StartRenders => "mcp-start-renders",
    }
}

/// Owns the server and the receiving end of its request channel.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct McpHost {
    bridge: Bridge,
    requests: Receiver<Envelope>,
    permissions: SharedPermissions,
    server: Option<Server>,
    status: McpStatus,
    /// The port of the last start attempt, so a failed bind is not retried on every unrelated settings change.
    attempted_port: Option<u16>,
}

#[cfg(not(target_arch = "wasm32"))]
impl std::fmt::Debug for McpHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpHost")
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl McpHost {
    /// A host that starts serving when `settings` say so; `waker` must wake the event loop.
    pub(crate) fn new(settings: &McpSettings, waker: Waker) -> Self {
        let permissions = SharedPermissions::new(settings.permissions);
        let (bridge, requests) = rustium_mcp::channel(permissions.clone(), waker);
        let mut host = Self {
            bridge,
            requests,
            permissions,
            server: None,
            status: McpStatus::Off,
            attempted_port: None,
        };
        host.apply(settings);
        host
    }

    pub(crate) fn status(&self) -> &McpStatus {
        &self.status
    }

    /// Brings the server in line with the settings: permissions apply at once, the switch and port restart it.
    pub(crate) fn apply(&mut self, settings: &McpSettings) {
        self.permissions.set(settings.permissions);
        if !settings.enabled {
            if self.server.take().is_some() {
                info!("MCP server switched off");
            }
            self.status = McpStatus::Off;
            self.attempted_port = None;
            return;
        }
        let running_on = self.server.as_ref().map(|server| server.address().port());
        if running_on == Some(settings.port) || (running_on.is_none() && self.attempted_port == Some(settings.port)) {
            return;
        }
        self.server = None;
        self.attempted_port = Some(settings.port);
        self.status = match Server::start(settings.port, self.bridge.clone()) {
            Ok(server) => {
                let address = server.address();
                self.server = Some(server);
                McpStatus::Listening(address)
            }
            Err(error) => {
                warn!(%error, "cannot start the MCP server");
                McpStatus::Failed(error.to_string())
            }
        };
    }

    /// Answers every request that is waiting. Call on the UI thread when the event loop is woken.
    pub(crate) fn drain(&self, editor: &mut Editor) {
        for envelope in self.requests.try_iter() {
            let reply = automation::handle(editor, envelope.request);
            // The client may have given up waiting; there is nobody to tell.
            let _ = envelope.reply.send(reply);
        }
    }
}

/// The browser's host: nothing can connect, so it is always off and never has requests.
#[cfg(target_arch = "wasm32")]
#[derive(Debug)]
pub(crate) struct McpHost;

#[cfg(target_arch = "wasm32")]
impl McpHost {
    pub(crate) fn new(_settings: &McpSettings, _waker: Waker) -> Self {
        Self
    }

    #[expect(clippy::unused_self, reason = "the same calls as the desktop host")]
    pub(crate) fn status(&self) -> &McpStatus {
        &McpStatus::Off
    }

    #[expect(clippy::unused_self, reason = "the same calls as the desktop host")]
    pub(crate) fn apply(&mut self, _settings: &McpSettings) {}

    #[expect(clippy::unused_self, reason = "the same calls as the desktop host")]
    pub(crate) fn drain(&self, _editor: &mut Editor) {}
}

/// `rustium --mcp-stdio`: forwards MCP messages between stdin/stdout and the running editor.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn run_stdio_bridge() -> Result<()> {
    let settings = Settings::load();
    let url = settings.mcp.url();
    let stdin = io::stdin();
    let stdout = io::stdout();
    rustium_mcp::run_stdio_bridge(&url, stdin.lock(), stdout.lock()).wrap_err("the MCP stdio bridge stopped")
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    use glam::IVec3;
    use serde_json::{Value, json};

    use super::*;

    fn listening(host: &McpHost) -> SocketAddr {
        match host.status() {
            McpStatus::Listening(address) => *address,
            other => unreachable!("the host should be listening, but is {other:?}"),
        }
    }

    /// The voxel at (2, 0, 0) of the Focus Model.
    fn focus_voxel(editor: &Editor) -> rustium_core::Voxel {
        editor
            .scene
            .model_shape(editor.focus)
            .unwrap()
            .1
            .get(IVec3::new(2, 0, 0))
    }

    fn focus_volume_is_empty(editor: &Editor) -> bool {
        editor.scene.model_shape(editor.focus).unwrap().1.volume().is_none()
    }

    fn enabled_on_a_free_port() -> McpSettings {
        McpSettings {
            enabled: true,
            port: 0,
            ..McpSettings::default()
        }
    }

    #[test]
    fn settings_round_trip_and_default_when_missing() {
        let text = toml::to_string_pretty(&McpSettings::default()).unwrap();
        assert_eq!(toml::from_str::<McpSettings>(&text).unwrap(), McpSettings::default());
        let parsed: McpSettings = toml::from_str("enabled = true").unwrap();
        assert!(parsed.enabled && parsed.port == 7420 && parsed.permissions.edit_voxels);
    }

    #[test]
    fn client_config_matches_the_transport() {
        let http: Value = serde_json::from_str(&McpSettings::default().client_config()).unwrap();
        assert_eq!(http["mcpServers"]["rustium"]["url"], "http://127.0.0.1:7420/mcp");
        let stdio = McpSettings {
            transport: McpTransport::Stdio,
            ..McpSettings::default()
        };
        let stdio: Value = serde_json::from_str(&stdio.client_config()).unwrap();
        assert_eq!(stdio["mcpServers"]["rustium"]["args"], json!(["--mcp-stdio"]));
    }

    #[test]
    fn the_switch_starts_and_stops_the_server() {
        let mut host = McpHost::new(&McpSettings::default(), Arc::new(|| {}));
        assert_eq!(host.status(), &McpStatus::Off);
        host.apply(&enabled_on_a_free_port());
        assert!(matches!(host.status(), McpStatus::Listening(_)));
        host.apply(&McpSettings::default());
        assert_eq!(host.status(), &McpStatus::Off);
    }

    #[test]
    fn a_busy_port_shows_as_failed() {
        let mut first = McpHost::new(&enabled_on_a_free_port(), Arc::new(|| {}));
        let address = listening(&first);
        let mut second = McpHost::new(&McpSettings::default(), Arc::new(|| {}));
        second.apply(&McpSettings {
            enabled: true,
            port: address.port(),
            ..McpSettings::default()
        });
        assert!(matches!(second.status(), McpStatus::Failed(_)));
        first.apply(&McpSettings::default());
    }

    /// The whole path without a window: a client talks to the stdio bridge, which talks HTTP to the server,
    /// whose requests the "UI thread" (this test) drains into a real `Editor`.
    #[test]
    fn a_client_edits_the_editor_through_the_server_and_it_undoes() {
        let host = McpHost::new(&enabled_on_a_free_port(), Arc::new(|| {}));
        let address = listening(&host);
        let url = format!("http://{address}/mcp");
        let messages = [
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "0"}}}),
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {
                "name": "set_voxels", "arguments": {"voxels": [{"x": 2, "y": 0, "z": 0, "color": 9}]}}}),
            json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {
                "name": "scene_info", "arguments": {}}}),
        ]
        .map(|message| message.to_string())
        .join("\n");
        let client = thread::spawn(move || {
            let mut output = Vec::new();
            rustium_mcp::run_stdio_bridge(&url, messages.as_bytes(), &mut output).unwrap();
            String::from_utf8(output).unwrap()
        });
        let mut editor = Editor::new(0.25, 50);
        while !client.is_finished() {
            host.drain(&mut editor);
            thread::sleep(Duration::from_millis(5));
        }
        let output = client.join().unwrap();
        let answers: Vec<Value> = output.lines().map(|line| serde_json::from_str(line).unwrap()).collect();
        assert_eq!(answers.len(), 3, "{output}");
        let info = answers[2]["result"]["content"][0]["text"].as_str().unwrap();
        assert!(info.contains("\"can_undo\":true"), "{info}");

        assert_eq!(focus_voxel(&editor).map(rustium_core::ColorId::get), Some(9));
        editor.undo();
        assert!(focus_voxel(&editor).is_none());
    }

    #[test]
    fn a_disabled_permission_is_refused_end_to_end() {
        let mut settings = enabled_on_a_free_port();
        settings.permissions.edit_voxels = false;
        let host = McpHost::new(&settings, Arc::new(|| {}));
        let address = listening(&host);
        let url = format!("http://{address}/mcp");
        let call = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {
            "name": "set_voxels", "arguments": {"voxels": [{"x": 0, "y": 0, "z": 0, "color": 1}]}}})
        .to_string();
        let client = thread::spawn(move || {
            let mut output = Vec::new();
            rustium_mcp::run_stdio_bridge(&url, call.as_bytes(), &mut output).unwrap();
            String::from_utf8(output).unwrap()
        });
        let mut editor = Editor::new(0.25, 50);
        while !client.is_finished() {
            host.drain(&mut editor);
            thread::sleep(Duration::from_millis(5));
        }
        let output = client.join().unwrap();
        assert!(
            output.contains("\"isError\":true") && output.contains("Edit voxels"),
            "{output}"
        );
        assert!(focus_volume_is_empty(&editor));
    }
}
