use std::io::stdout;

use libruskel::{Ruskel, SearchDomain, SearchOptions, describe_domains};
use serde::{Deserialize, Serialize};
use tenx_mcp::{Result, Server, ServerCtx, mcp_server, schema::CallToolResult, schemars, tool};
use tracing::error;

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
/// Parameters accepted by the ruskel MCP tool.
pub struct RuskelSkeletonTool {
    /// Crate, module path, or filesystem path (optionally with @<semver>) whose API skeleton should be produced.
    pub target: String,

    /// Include non‑public (private / crate‑private) items.
    #[serde(default)]
    pub private: bool,

    /// Restrict output to matches for this search query instead of rendering the entire target.
    #[serde(default)]
    pub search: Option<String>,

    /// Include frontmatter comments describing the invocation context.
    #[serde(default = "default_frontmatter_enabled")]
    pub frontmatter: bool,

    /// Include item names when evaluating search matches.
    #[serde(default)]
    pub search_names: bool,

    /// Include documentation text when evaluating search matches.
    #[serde(default)]
    pub search_docs: bool,

    /// Include canonical module and item paths when evaluating search matches.
    #[serde(default)]
    pub search_paths: bool,

    /// Include rendered signatures when evaluating search matches.
    #[serde(default)]
    pub search_signatures: bool,

    /// Require case-sensitive matches.
    #[serde(default)]
    pub search_case_sensitive: bool,

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
/// MCP server implementation that forwards requests to an underlying `Ruskel` instance.
pub struct RuskelServer {
    /// Code skeleton renderer shared across tool invocations.
    ruskel: Ruskel,
}

#[mcp_server]
impl RuskelServer {
    /// Create a new server wrapper around the provided `Ruskel` renderer.
    pub fn new(ruskel: Ruskel) -> Self {
        Self { ruskel }
    }

    #[tool]
    /// **ruskel** returns a Rust skeleton that shows the API of any item with implementation
    /// bodies stripped. Useful for models that need to look up names, signatures, derives, APIs,
    /// and doc‑comments while writing or reviewing Rust code. An item can be a crate, module,
    /// struct, trait, function, or any other Rust entity that can be referred to with a Rust path.
    ///
    /// # When a model should call this tool
    /// 1. It needs to look up a function/trait/struct signature.
    /// 2. It wants an overview of a public or private API.
    /// 3. The user asks for examples or docs from a crate.
    ///
    /// # Target syntax examples
    /// - `mycrate::Struct` →  a struct in the current crate
    /// - `mycrate::Struct::method` →  a method on a struct in the current crate
    /// - `std::vec::Vec` →  Vec from the std lib
    /// - `serde` →  latest serde on crates.io
    /// - `serde@1.0.160` →  specific published version
    /// - `serde::de::Deserialize` →  narrow output to one module/type for small contexts
    /// - `/path/to/crate` or `/path/to/crate::submod` →  local workspace paths
    ///
    /// # Output format
    /// Plain UTF‑8 text containing valid Rust code, with implementation omitted.
    ///
    /// # Tips for LLMs
    /// - Request deep module paths (e.g. `tokio::sync::mpsc`) to keep the reply below
    ///   your token budget.
    /// - Pass `all_features=true` or `features=[…]` when a symbol is behind a feature gate.
    /// - Pass private=true to include non‑public items. Useful if you're looking up details of
    ///   items in the current codebase for development.
    /// - Pass `search="pattern"` (with optional `search_*` flags) to restrict output to matched
    ///   items instead of rendering the entire target.
    /// - Pass `frontmatter=false` when you need the raw Rust skeleton without the leading comment
    ///   block summarising context.
    async fn ruskel(&self, _ctx: &ServerCtx, params: RuskelSkeletonTool) -> Result<CallToolResult> {
        let ruskel = self.ruskel.clone().with_frontmatter(params.frontmatter);

        if let Some(query) = params
            .search
            .as_ref()
            .map(|q| q.trim())
            .filter(|q| !q.is_empty())
        {
            let mut options = SearchOptions::new(query);
            options.include_private = params.private;
            options.case_sensitive = params.search_case_sensitive;

            let mut domains = SearchDomain::empty();
            if params.search_names {
                domains |= SearchDomain::NAMES;
            }
            if params.search_docs {
                domains |= SearchDomain::DOCS;
            }
            if params.search_paths {
                domains |= SearchDomain::PATHS;
            }
            if params.search_signatures {
                domains |= SearchDomain::SIGNATURES;
            }
            if !domains.is_empty() {
                options.domains = domains;
            }

            match ruskel.search(
                &params.target,
                params.no_default_features,
                params.all_features,
                params.features.clone(),
                &options,
            ) {
                Ok(response) => {
                    if response.results.is_empty() {
                        return Ok(CallToolResult::new()
                            .with_text_content(format!("No matches found for \"{}\".", query)));
                    }

                    let mut summary = String::new();
                    summary.push_str(&format!(
                        "Found {} matches for \"{}\":\n",
                        response.results.len(),
                        query
                    ));
                    for result in &response.results {
                        let labels = describe_domains(result.matched);
                        if labels.is_empty() {
                            summary.push_str(&format!(" - {}\n", result.path_string));
                        } else {
                            summary.push_str(&format!(
                                " - {} [{}]\n",
                                result.path_string,
                                labels.join(", ")
                            ));
                        }
                    }
                    summary.push('\n');
                    summary.push_str(&response.rendered);

                    Ok(CallToolResult::new().with_text_content(summary))
                }
                Err(e) => {
                    error!("Failed to generate search results: {}", e);
                    Ok(CallToolResult::new()
                        .with_text_content(format!(
                            "Failed to search '{}' with query '{}': {}",
                            params.target, query, e
                        ))
                        .is_error(true))
                }
            }
        } else {
            match ruskel.render(
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
}

const fn default_frontmatter_enabled() -> bool {
    true
}

/// Serve the ruskel MCP API over TCP or stdio depending on configuration.
///
/// When `addr` is provided a TCP listener is started; otherwise the server exposes
/// stdio pipes suitable for process integration.
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
            .with_writer(stdout)
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
