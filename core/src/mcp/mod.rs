use std::path::PathBuf;
use std::time::Duration;

use crate::Result;

/// Configuration for running the MCP server over stdio.
#[derive(Debug, Clone)]
pub struct StdioServerConfig {
    /// Repository root that backs the server.
    pub repo_path: PathBuf,
    /// Maximum duration allowed for the initial handshake.
    pub handshake_timeout: Duration,
    /// Optional idle timeout after which the server shuts down.
    pub idle_shutdown: Option<Duration>,
}

impl StdioServerConfig {
    pub fn new(repo_path: PathBuf) -> Self {
        Self {
            repo_path,
            handshake_timeout: Duration::from_secs(30),
            idle_shutdown: None,
        }
    }

    pub fn with_handshake_timeout(mut self, timeout: Duration) -> Self {
        self.handshake_timeout = timeout;
        self
    }

    pub fn with_idle_shutdown(mut self, timeout: Option<Duration>) -> Self {
        self.idle_shutdown = timeout;
        self
    }
}

/// Placeholder implementation of the stdio MCP server.
///
/// The full server lifecycle will be implemented in a subsequent task. For now we
/// simply validate the configuration and return immediately so that the CLI
/// subcommand can be exercised in tests.
pub async fn run_stdio_server(config: StdioServerConfig) -> Result<()> {
    let _ = config;
    Ok(())
}
