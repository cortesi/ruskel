//! MCP server integration for the `ruskel` CLI.

/// Tools for exposing ruskel functionality via the Model Context Protocol.
mod server;

pub use server::run_mcp_server;

// Kept public for integration tests but hidden from generated docs.
#[doc(hidden)]
pub use server::RuskelServer;
