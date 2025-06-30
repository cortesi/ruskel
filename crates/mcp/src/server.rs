use libruskel::Ruskel;
use serde::{Deserialize, Serialize};
use tenx_mcp::{Result, Server, ServerCtx, mcp_server, schema::CallToolResult, schemars, tool};
use tracing::error;

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct RuskelSkeletonTool {
    /// Crate, module path, or filesystem path (optionally with @<semver>) whose API skeleton should be produced.
    pub target: String,

    /// Include non‑public (private / crate‑private) items.
    #[serde(default)]
    pub private: bool,

    /// Disable the crate's default Cargo features.
    #[serde(default)]
    pub no_default_features: bool,

    /// Enable every optional Cargo feature.
    #[serde(default)]
    pub all_features: bool,

    /// Exact list of Cargo features to enable (ignored if all_features=true).
    #[serde(default)]
    pub features: Vec<String>,
}

#[derive(Clone)]
pub struct RuskelServer {
    ruskel: Ruskel,
}

impl RuskelServer {
    pub fn new(ruskel: Ruskel) -> Self {
        Self { ruskel }
    }
}

#[mcp_server]
impl RuskelServer {
    #[tool]
    /// **Ruskel Skeleton Generator** return a single Rust source file that lists the
    /// *public API (or optionally private items) of any crate or module path, with all
    /// bodies stripped*. Useful for large‑language models that need to look up item
    /// names, signatures, derives, feature‑gated cfgs, and doc‑comments while writing
    /// or reviewing Rust code.
    ///
    /// ### When a model should call this tool
    /// 1. It needs a function/trait/struct signature it can't recall.
    /// 2. The user asks for examples or docs from a crate.
    /// 3. The model wants to verify what features gate a symbol.
    ///
    /// ### Target syntax examples
    /// - `serde`               →  latest serde on crates.io
    /// - `serde@1.0.160`      →  specific published version
    /// - `serde::de::Deserialize` →  narrow output to one module/type for small contexts
    /// - `/path/to/crate` or `/path/to/crate::submod` →  local workspace paths
    ///
    /// ### Output format
    /// Plain UTF‑8 text containing valid Rust code, with implementation omitted.
    ///
    /// ### Tips for LLMs
    /// - Request deep module paths (e.g. `tokio::sync::mpsc`) to keep the reply below
    ///   your token budget.
    /// - Pass `all_features=true` or `features=[…]` when a symbol is behind a feature gate.
    async fn ruskel(&self, _ctx: &ServerCtx, params: RuskelSkeletonTool) -> Result<CallToolResult> {
        match self.ruskel.render(
            &params.target,
            params.no_default_features,
            params.all_features,
            params.features,
            params.private,
        ) {
            Ok(output) => Ok(CallToolResult::new().with_text_content(output)),
            Err(e) => {
                error!("Failed to generate skeleton: {}", e);
                Ok(CallToolResult::new()
                    .with_text_content(format!(
                        "Failed to generate skeleton for '{}': {}",
                        params.target, e
                    ))
                    .is_error(true))
            }
        }
    }
}

pub async fn run_mcp_server(
    ruskel: Ruskel,
    addr: Option<String>,
    log_level: Option<String>,
) -> Result<()> {
    // Initialize tracing for TCP mode only
    if addr.is_some() {
        let level = log_level.as_deref().unwrap_or("info");
        let filter = format!("ruskel_mcp={level},tenx_mcp={level}");

        tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::new(filter))
            .with_writer(std::io::stdout)
            .without_time()
            .init();
    }

    let server = Server::default().with_connection(move || RuskelServer::new(ruskel.clone()));

    match addr {
        Some(addr) => {
            tracing::info!("Starting MCP server on {}", addr);
            server.serve_tcp(addr).await
        }
        None => server.serve_stdio().await,
    }
}
