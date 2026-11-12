//! In-process MCP server for the Rustium voxel editor.
//!
//! MCP clients (AI assistants, the MCP inspector, `curl`) connect over streamable HTTP to
//! `127.0.0.1:7420/mcp`; clients that can only spawn a process run `rustium --mcp-stdio`, which
//! [`run_stdio_bridge`] turns into plain forwarding to the same HTTP endpoint of the already-running editor.
//!
//! # Threading
//!
//! This is the only crate that uses tokio, and it keeps it on one thread named `rustium-mcp`. The server
//! never touches Scene state. A tool call becomes a [`Request`]; [`Bridge::call`] checks the permission,
//! puts an [`Envelope`] on a bounded `crossbeam-channel`, fires the [`Waker`] (the editor wakes its winit
//! event loop with an `EventLoopProxy`) and waits for the answer on a reply channel. The editor drains the
//! [`Receiver`](crossbeam_channel::Receiver), applies the request on its UI thread through
//! `rustium_core::Command`s, and sends back a [`Reply`].
//!
//! ```no_run
//! use std::sync::Arc;
//!
//! use rustium_mcp::{Permissions, Server, SharedPermissions, channel};
//!
//! let permissions = SharedPermissions::new(Permissions::default());
//! let (bridge, requests) = channel(permissions, Arc::new(|| { /* wake the UI thread */ }));
//! let server = Server::start(7420, bridge)?;
//! // UI thread: for envelope in requests.try_iter() { ...answer on envelope.reply... }
//! server.stop();
//! # Ok::<(), rustium_mcp::Error>(())
//! ```
//!
//! # WebAssembly
//!
//! A browser cannot listen on a port, so on `wasm32` the crate offers only the [`Permission`] switches,
//! [`DEFAULT_PORT`] and [`Waker`], which the editor uses either way.
#![warn(clippy::panic, clippy::expect_used)]

#[cfg(not(target_arch = "wasm32"))]
mod bridge;
#[cfg(not(target_arch = "wasm32"))]
mod error;
mod permission;
#[cfg(not(target_arch = "wasm32"))]
mod server;
#[cfg(not(target_arch = "wasm32"))]
mod stdio;
#[cfg(not(target_arch = "wasm32"))]
mod tools;

#[cfg(not(target_arch = "wasm32"))]
pub use bridge::{Bridge, BrushTool, Envelope, FaceDirection, Reply, Request, SurfaceParams, VoxelWrite, channel};
#[cfg(not(target_arch = "wasm32"))]
pub use error::Error;
pub use permission::{Permission, Permissions, SharedPermissions};
#[cfg(not(target_arch = "wasm32"))]
pub use server::Server;
#[cfg(not(target_arch = "wasm32"))]
pub use stdio::run_stdio_bridge;
#[cfg(not(target_arch = "wasm32"))]
pub use tools::Tools;

/// The port the editor serves MCP on unless the settings say otherwise.
pub const DEFAULT_PORT: u16 = 7420;

/// Wakes the UI thread's event loop so it looks at its request channel.
pub type Waker = std::sync::Arc<dyn Fn() + Send + Sync>;
