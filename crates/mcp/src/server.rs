use std::{env, io::stdout};

use libruskel::{Ruskel, SearchDomain, SearchOptions, describe_domains, parse_domain_tokens};
use serde::{Deserialize, Serialize};
use tmcp::{Result, Server, ServerCtx, mcp_server, schema::CallToolResult, tool};
use tokio::signal::ctrl_c;
use tracing::error;
use tracing_subscriber::filter::LevelFilter;

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
/// Parameters accepted by the ruskel MCP tool.
pub struct RuskelSkeletonTool {
    /// Target to skeletonize: crate, module, path, or crate@version.
    pub target: String,

    /// Include private items.
    #[serde(default)]
    pub private: bool,

    /// Restrict output to matches for this query.
    #[serde(default)]
    pub search: Option<String>,

    /// Render a binary target as a library, with private items included.
    #[serde(default)]
    pub bin: Option<String>,

    /// Limit search to specific domains (name, doc, signature, path). Defaults to name, doc, signature.
    #[serde(default)]
    pub search_spec: Option<Vec<String>>,

    /// Include comment frontmatter.
    #[serde(default = "default_frontmatter_enabled")]
    pub frontmatter: bool,

    /// Require case-sensitive matches.
    #[serde(default)]
    pub search_case_sensitive: bool,

    /// Only render direct matches, not expanded containers.
    #[serde(default)]
    pub direct_match_only: bool,

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
    /// **ruskel** returns a Rust API skeleton with implementation stripped. Useful for looking up
    /// signatures, derives, APIs, and doc-comments.
    ///
    /// # When a model should call this tool
    /// 1. It needs to look up a function/trait/struct signature.
    /// 2. It wants an overview of a public or private API.
    /// 3. The user asks for examples or docs from a crate.
    ///
    /// # Target syntax examples
    /// - `mycrate::Struct` →  struct in current crate
    /// - `mycrate::Struct::method` →  method on struct in current crate
    /// - `std::vec::Vec` →  Vec from std lib
    /// - `serde` →  latest serde on crates.io
    /// - `serde@1.0.160` →  specific published version
    /// - `serde::de::Deserialize` →  narrow to one module/type
    /// - `/path/to/crate` or `/path/to/crate::submod` →  local workspace paths
    ///
    /// # Output format
    /// Valid Rust code with implementation omitted.
    ///
    /// # Tips for LLMs
    /// - Request deep module paths (e.g. `tokio::sync::mpsc`) to reduce output size.
    /// - Pass `all_features=true` or `features=[…]` when a symbol is behind a feature gate.
    /// - Pass `private=true` for private items in local codebases. **Caution:** Avoid using
    ///   `private=true` on entire crates since output can be extremely large. Prefer targeting
    ///   specific modules or items.
    /// - Pass `search="pattern"` to restrict output to matched items.
    /// - Pass `direct_match_only=true` to show only exact matches.
    /// - Pass `frontmatter=false` to omit the leading comment block.
    async fn ruskel(&self, _ctx: &ServerCtx, params: RuskelSkeletonTool) -> Result<CallToolResult> {
        if env::var_os("RUSKEL_MCP_TEST_MODE").is_some() {
            return Ok(run_test_mode(params));
        }

        let ruskel = self
            .ruskel
            .clone()
            .with_frontmatter(params.frontmatter)
            .with_bin_target(params.bin.clone());

        if let Some(query) = params
            .search
            .as_ref()
            .map(|q| q.trim())
            .filter(|q| !q.is_empty())
        {
            return Ok(self.run_search_mode(&ruskel, &params, query));
        }

        Ok(self.run_render_mode(&ruskel, params))
    }

    /// Build the MCP response for search invocations, including match summaries.
    fn run_search_mode(
        &self,
        ruskel: &Ruskel,
        params: &RuskelSkeletonTool,
        query: &str,
    ) -> CallToolResult {
        let mut options = SearchOptions::new(query);
        options.include_private = params.private;
        options.case_sensitive = params.search_case_sensitive;

        let domains = match params.search_spec.as_ref() {
            Some(spec) if !spec.is_empty() => parse_domain_tokens(spec.iter().map(|s| s.as_str())),
            _ => SearchDomain::default(),
        };
        options.domains = domains;
        options.expand_containers = !params.direct_match_only;

        match ruskel.search(
            &params.target,
            params.no_default_features,
            params.all_features,
            params.features.clone(),
            &options,
        ) {
            Ok(response) => {
                if response.results.is_empty() {
                    return CallToolResult::new()
                        .with_text_content(format!("No matches found for \"{}\".", query));
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

                CallToolResult::new().with_text_content(summary)
            }
            Err(e) => {
                error!("Failed to generate search results: {}", e);
                CallToolResult::new()
                    .with_text_content(format!(
                        "Failed to search '{}' with query '{}': {}",
                        params.target, query, e
                    ))
                    .mark_as_error()
            }
        }
    }

    /// Build the MCP response for render-only requests.
    fn run_render_mode(&self, ruskel: &Ruskel, params: RuskelSkeletonTool) -> CallToolResult {
        match ruskel.render(
            &params.target,
            params.no_default_features,
            params.all_features,
            params.features,
            params.private,
        ) {
            Ok(output) => CallToolResult::new().with_text_content(output),
            Err(e) => {
                error!("Failed to generate skeleton: {}", e);
                CallToolResult::new()
                    .with_text_content(format!(
                        "Failed to generate skeleton for '{}': {}",
                        params.target, e
                    ))
                    .mark_as_error()
            }
        }
    }
}

/// Lightweight stubbed response used when the MCP server is started in test mode.
///
/// This bypasses expensive rustdoc generation, allowing integration tests to run quickly
/// while still exercising the MCP protocol surface.
fn run_test_mode(params: RuskelSkeletonTool) -> CallToolResult {
    let mut summary = String::new();
    summary.push_str("ruskel test-mode output\n");
    summary.push_str(&format!("target: {}\n", params.target));
    summary.push_str(&format!("private: {}\n", params.private));

    if let Some(search) = params.search {
        summary.push_str(&format!("search: {}\n", search));
    }

    if let Some(spec) = params.search_spec
        && !spec.is_empty()
    {
        summary.push_str(&format!("search_spec: {}\n", spec.join(",")));
    }

    CallToolResult::new().with_text_content(summary)
}

/// Frontmatter is enabled by default when no user preference is provided.
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
    log_level: Option<LevelFilter>,
) -> Result<()> {
    // Initialize tracing for TCP mode only
    if addr.is_some() {
        let level = log_level.unwrap_or(LevelFilter::INFO);
        let filter = format!("ruskel_mcp={level},tmcp={level}");

        tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::new(filter))
            .with_writer(stdout)
            .without_time()
            .init();
    }

    let server = Server::new(move || RuskelServer::new(ruskel.clone()));

    match addr {
        Some(addr) => {
            tracing::info!("Starting MCP server on {}", addr);
            let handle = server.serve_tcp(addr).await?;
            ctrl_c().await?;
            handle.stop().await
        }
        None => server.serve_stdio().await,
    }
}
