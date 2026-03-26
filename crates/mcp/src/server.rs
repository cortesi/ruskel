use std::{env, io::stdout, result::Result as StdResult};

use libruskel::{Ruskel, SearchDomain, SearchOptions, describe_domains, parse_domain_token};
use serde::{Deserialize, Serialize};
use tmcp::{Result, Server, ServerCtx, mcp_server, schema::CallToolResult, tool};
use tokio::signal::ctrl_c;
use tracing::error;
use tracing_subscriber::filter::LevelFilter;

/// Default request values applied by the MCP server when a tool call omits them.
#[derive(Debug, Clone, Copy)]
pub struct RuskelServerDefaults {
    /// Whether omitted requests should include private items.
    pub private: bool,
    /// Whether omitted requests should include frontmatter comments.
    pub frontmatter: bool,
}

impl Default for RuskelServerDefaults {
    fn default() -> Self {
        Self {
            private: false,
            frontmatter: true,
        }
    }
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
/// Parameters accepted by the ruskel MCP tool.
pub struct RuskelSkeletonTool {
    /// Target to skeletonize: crate, module, path, or crate@version.
    pub target: String,

    /// Include private items. Defaults to the server's configured setting when omitted.
    #[serde(default)]
    pub private: Option<bool>,

    /// Restrict output to matches for this query.
    #[serde(default)]
    pub search: Option<String>,

    /// Render a binary target as a library, with private items included.
    #[serde(default)]
    pub bin: Option<String>,

    /// Limit search to specific domains (name, doc, signature, path). Defaults to name, doc, signature.
    #[serde(default)]
    pub search_spec: Option<Vec<String>>,

    /// Include comment frontmatter. Defaults to the server's configured setting when omitted.
    #[serde(default)]
    pub frontmatter: Option<bool>,

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

/// Fully resolved MCP tool parameters after applying server defaults.
#[derive(Debug, Clone)]
struct ResolvedRuskelSkeletonTool {
    /// Target to skeletonize.
    target: String,
    /// Whether private items should be included.
    private: bool,
    /// Optional query used for search mode.
    search: Option<String>,
    /// Optional binary target override.
    bin: Option<String>,
    /// Optional list of search domains.
    search_spec: Option<Vec<String>>,
    /// Whether rendered output should include frontmatter comments.
    frontmatter: bool,
    /// Whether search matching should be case sensitive.
    search_case_sensitive: bool,
    /// Whether search results should avoid expanding matched containers.
    direct_match_only: bool,
    /// Whether Cargo default features should be disabled.
    no_default_features: bool,
    /// Whether all Cargo features should be enabled.
    all_features: bool,
    /// Explicit Cargo feature list.
    features: Vec<String>,
}

impl RuskelSkeletonTool {
    /// Resolve optional request fields against the server defaults.
    fn resolve(self, defaults: RuskelServerDefaults) -> ResolvedRuskelSkeletonTool {
        ResolvedRuskelSkeletonTool {
            target: self.target,
            private: self.private.unwrap_or(defaults.private),
            search: self.search,
            bin: self.bin,
            search_spec: self.search_spec,
            frontmatter: self.frontmatter.unwrap_or(defaults.frontmatter),
            search_case_sensitive: self.search_case_sensitive,
            direct_match_only: self.direct_match_only,
            no_default_features: self.no_default_features,
            all_features: self.all_features,
            features: self.features,
        }
    }
}

#[derive(Clone)]
/// MCP server implementation that forwards requests to an underlying `Ruskel` instance.
pub struct RuskelServer {
    /// Code skeleton renderer shared across tool invocations.
    ruskel: Ruskel,
    /// Default request values applied when tool calls omit optional flags.
    defaults: RuskelServerDefaults,
}

#[mcp_server]
impl RuskelServer {
    /// Create a new server wrapper around the provided `Ruskel` renderer.
    pub fn new(ruskel: Ruskel) -> Self {
        Self::with_defaults(ruskel, RuskelServerDefaults::default())
    }

    /// Create a new server wrapper with explicit request defaults.
    pub fn with_defaults(ruskel: Ruskel, defaults: RuskelServerDefaults) -> Self {
        Self { ruskel, defaults }
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
        let params = params.resolve(self.defaults);
        let search_domains = match resolve_search_domains(params.search_spec.as_deref()) {
            Ok(domains) => domains,
            Err(error) => {
                return Ok(CallToolResult::new()
                    .with_text_content(error)
                    .mark_as_error());
            }
        };

        if env::var_os("RUSKEL_MCP_TEST_MODE").is_some() {
            return Ok(run_test_mode(&params));
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
            return Ok(self.run_search_mode(&ruskel, &params, query, search_domains));
        }

        Ok(self.run_render_mode(&ruskel, &params))
    }

    /// Build the MCP response for search invocations, including match summaries.
    fn run_search_mode(
        &self,
        ruskel: &Ruskel,
        params: &ResolvedRuskelSkeletonTool,
        query: &str,
        domains: SearchDomain,
    ) -> CallToolResult {
        let options = SearchOptions::configured(
            query,
            domains,
            params.search_case_sensitive,
            params.private,
            !params.direct_match_only,
        );

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
    fn run_render_mode(
        &self,
        ruskel: &Ruskel,
        params: &ResolvedRuskelSkeletonTool,
    ) -> CallToolResult {
        match ruskel.render(
            &params.target,
            params.no_default_features,
            params.all_features,
            params.features.clone(),
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

/// Resolve search domains from optional MCP parameters, rejecting invalid tokens.
fn resolve_search_domains(search_spec: Option<&[String]>) -> StdResult<SearchDomain, String> {
    let Some(search_spec) = search_spec else {
        return Ok(SearchDomain::default());
    };
    if search_spec.is_empty() {
        return Ok(SearchDomain::default());
    }

    let mut domains = SearchDomain::empty();
    for token in search_spec {
        domains |= parse_domain_token(token)?;
    }
    if domains.is_empty() {
        Ok(SearchDomain::default())
    } else {
        Ok(domains)
    }
}

/// Lightweight stubbed response used when the MCP server is started in test mode.
///
/// This bypasses expensive rustdoc generation, allowing integration tests to run quickly
/// while still exercising the MCP protocol surface.
fn run_test_mode(params: &ResolvedRuskelSkeletonTool) -> CallToolResult {
    let mut summary = String::new();
    summary.push_str("ruskel test-mode output\n");
    summary.push_str(&format!("target: {}\n", params.target));
    summary.push_str(&format!("private: {}\n", params.private));
    summary.push_str(&format!("frontmatter: {}\n", params.frontmatter));

    if let Some(search) = &params.search {
        summary.push_str(&format!("search: {}\n", search));
    }

    if let Some(spec) = &params.search_spec
        && !spec.is_empty()
    {
        summary.push_str(&format!("search_spec: {}\n", spec.join(",")));
    }

    CallToolResult::new().with_text_content(summary)
}

/// Serve the ruskel MCP API over TCP or stdio depending on configuration.
///
/// When `addr` is provided a TCP listener is started; otherwise the server exposes
/// stdio pipes suitable for process integration.
pub async fn run_mcp_server(
    ruskel: Ruskel,
    addr: Option<String>,
    log_level: Option<LevelFilter>,
    defaults: RuskelServerDefaults,
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

    let server = Server::new(move || RuskelServer::with_defaults(ruskel.clone(), defaults));

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

#[cfg(test)]
mod tests {
    use libruskel::SearchDomain;

    use super::resolve_search_domains;

    #[test]
    fn resolve_search_domains_defaults_when_missing() {
        assert_eq!(
            resolve_search_domains(None).expect("default domains"),
            SearchDomain::default()
        );
    }

    #[test]
    fn resolve_search_domains_rejects_invalid_tokens() {
        let error = resolve_search_domains(Some(&[String::from("bogus")]))
            .expect_err("invalid token should fail");

        assert_eq!(
            error,
            "invalid search domain 'bogus'. Expected one of: name, doc, path, signature."
        );
    }
}
