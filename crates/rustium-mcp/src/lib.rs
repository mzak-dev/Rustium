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
#![warn(clippy::panic, clippy::expect_used)]

mod bridge;
mod error;
mod permission;
mod server;
mod stdio;
mod tools;

pub use bridge::{Bridge, BrushTool, Envelope, FaceDirection, Reply, Request, VoxelWrite, Waker, channel};
pub use error::Error;
pub use permission::{Permission, Permissions, SharedPermissions};
pub use server::{DEFAULT_PORT, Server};
pub use stdio::run_stdio_bridge;
pub use tools::Tools;
