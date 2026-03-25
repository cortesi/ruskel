//! MCP server integration for the `ruskel` CLI.

/// Tools for exposing ruskel functionality via the Model Context Protocol.
mod server;

// Kept public for integration tests but hidden from generated docs.
#[doc(hidden)]
pub use server::RuskelServer;
#[doc(hidden)]
pub use server::RuskelServerDefaults;
pub use server::run_mcp_server;
