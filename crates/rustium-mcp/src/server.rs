use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use hyper::server::conn::http1;
use hyper_util::rt::TokioIo;
use hyper_util::service::TowerToHyperService;
use rmcp::transport::streamable_http_server::session::never::NeverSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::bridge::Bridge;
use crate::error::Error;
use crate::tools::Tools;

/// The default port of the MCP endpoint.
pub const DEFAULT_PORT: u16 = 7420;

/// A running MCP server: streamable HTTP on `127.0.0.1`, served from its own thread.
///
/// The server only ever talks to the editor through the [`Bridge`]. Dropping it (or calling
/// [`Server::stop`]) stops accepting connections, closes the open ones and joins the thread.
#[derive(Debug)]
pub struct Server {
    address: SocketAddr,
    cancel: CancellationToken,
    thread: Option<JoinHandle<()>>,
}

impl Server {
    /// Binds `127.0.0.1:port` (0 picks a free port) and starts serving.
    ///
    /// # Errors
    ///
    /// [`Error::Bind`] when the port is taken, [`Error::Start`] when the thread cannot be created.
    pub fn start(port: u16, bridge: Bridge) -> Result<Self, Error> {
        let bind = |source| Error::Bind { port, source };
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port)).map_err(bind)?;
        listener.set_nonblocking(true).map_err(bind)?;
        let address = listener.local_addr().map_err(bind)?;
        let cancel = CancellationToken::new();
        let thread = thread::Builder::new()
            .name("rustium-mcp".to_owned())
            .spawn({
                let cancel = cancel.clone();
                move || run(listener, bridge, &cancel)
            })
            .map_err(Error::Start)?;
        info!(%address, "MCP server listening");
        Ok(Self {
            address,
            cancel,
            thread: Some(thread),
        })
    }

    /// The address the server listens on.
    pub fn address(&self) -> SocketAddr {
        self.address
    }

    /// The URL MCP clients connect to.
    pub fn url(&self) -> String {
        format!("http://{}/mcp", self.address)
    }

    /// Stops the server and waits for its thread to finish.
    pub fn stop(self) {
        drop(self);
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            warn!("the MCP server thread panicked");
        }
        info!(address = %self.address, "MCP server stopped");
    }
}

fn run(listener: TcpListener, bridge: Bridge, cancel: &CancellationToken) {
    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(error) => {
            warn!(%error, "cannot create the MCP runtime");
            return;
        }
    };
    runtime.block_on(serve(listener, bridge, cancel));
}

async fn serve(listener: TcpListener, bridge: Bridge, cancel: &CancellationToken) {
    let listener = match tokio::net::TcpListener::from_std(listener) {
        Ok(listener) => listener,
        Err(error) => {
            warn!(%error, "cannot register the MCP listener");
            return;
        }
    };
    // Stateless JSON responses: every POST is answered with one plain JSON body, which keeps `curl` and the
    // stdio bridge simple. Requests only ever read or edit the Scene, so there is nothing to keep in a session.
    let config = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .with_sse_keep_alive(None)
        // Host checks (loopback names only) are on by default and stop DNS rebinding; any request that carries
        // an Origin header comes from a web page, never from an MCP client, so refuse those too.
        .enforce_origin_validation()
        .with_cancellation_token(cancel.child_token());
    let service = StreamableHttpService::new(
        move || Ok(Tools::new(bridge.clone())),
        Arc::new(NeverSessionManager::default()),
        config,
    );
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            () = cancel.cancelled() => break,
            accepted = listener.accept() => match accepted {
                Ok((stream, peer)) => {
                    debug!(%peer, "MCP client connected");
                    let service = TowerToHyperService::new(service.clone());
                    let cancel = cancel.clone();
                    connections.spawn(async move {
                        let connection = http1::Builder::new().serve_connection(TokioIo::new(stream), service);
                        tokio::select! {
                            () = cancel.cancelled() => {}
                            result = connection => {
                                if let Err(error) = result {
                                    debug!(%error, "MCP connection ended with an error");
                                }
                            }
                        }
                    });
                }
                Err(error) => warn!(%error, "cannot accept an MCP connection"),
            },
            Some(_) = connections.join_next(), if !connections.is_empty() => {}
        }
    }
    connections.shutdown().await;
}
