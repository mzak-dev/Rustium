use std::io;

/// Why the MCP server or the stdio bridge could not start or keep running.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The port is taken or the address cannot be bound.
    #[error("cannot listen on 127.0.0.1:{port}: {source}")]
    Bind {
        /// The port that was requested.
        port: u16,
        /// The operating system's reason.
        source: io::Error,
    },
    /// A thread or runtime could not be created.
    #[error("cannot start the MCP server: {0}")]
    Start(#[source] io::Error),
    /// Standard input or output of the stdio bridge failed.
    #[error("stdio failed: {0}")]
    Stdio(#[source] io::Error),
}
