use libruskel::Ruskel;
use tenx_mcp::{
    schema::{
        CallToolResult, InitializeResult, ListToolsResult, ServerCapabilities, Tool,
        ToolInputSchema,
    },
    Connection, Result, Server,
};
use tracing::error;

use crate::tools::RuskelSkeletonTool;

pub struct RuskelConnection {
    ruskel: Ruskel,
}

impl RuskelConnection {
    pub fn new(ruskel: Ruskel) -> Self {
        Self { ruskel }
    }

    fn tool_metadata(&self) -> Tool {
        let properties = [
            ("target", serde_json::json!({
                "type": "string",
                "description": "Crate, module path, or filesystem path (optionally with @<semver>) whose API skeleton should be produced."
            })),
            ("private", serde_json::json!({
                "type": "boolean",
                "description": "Include non‑public (private / crate‑private) items.",
                "default": false
            })),
            ("no_default_features", serde_json::json!({
                "type": "boolean",
                "description": "Disable the crate's default Cargo features.",
                "default": false
            })),
            ("all_features", serde_json::json!({
                "type": "boolean",
                "description": "Enable every optional Cargo feature.",
                "default": false
            })),
            ("features", serde_json::json!({
                "type": "array",
                "items": {"type": "string"},
                "description": "Exact list of Cargo features to enable (ignored if all_features=true).",
                "default": []
            }))
        ].into_iter().map(|(k, v)| (k.to_string(), v)).collect();

        Tool::new(
            "ruskel",
            ToolInputSchema::default()
                .with_properties(properties)
                .with_required("target"),
        )
        .with_description(include_str!("../ruskel-description.txt"))
    }

    async fn execute_tool(&self, arguments: Option<serde_json::Value>) -> CallToolResult {
        let args = arguments.unwrap_or_default();

        let tool_params: RuskelSkeletonTool = match serde_json::from_value(args) {
            Ok(params) => params,
            Err(e) => {
                return CallToolResult::new()
                    .with_text_content(format!("Invalid parameters for ruskel tool: {e}"))
                    .is_error(true)
            }
        };

        match self.ruskel.render(
            &tool_params.target,
            tool_params.no_default_features,
            tool_params.all_features,
            tool_params.features,
            tool_params.private,
        ) {
            Ok(output) => CallToolResult::new().with_text_content(output),
            Err(e) => {
                error!("Failed to generate skeleton: {}", e);
                CallToolResult::new()
                    .with_text_content(format!(
                        "Failed to generate skeleton for '{}': {}",
                        tool_params.target, e
                    ))
                    .is_error(true)
            }
        }
    }
}

#[async_trait::async_trait]
impl Connection for RuskelConnection {
    async fn initialize(
        &mut self,
        _protocol_version: String,
        _capabilities: tenx_mcp::ClientCapabilities,
        _client_info: tenx_mcp::Implementation,
    ) -> Result<InitializeResult> {
        Ok(InitializeResult::new("Ruskel MCP Server", env!("CARGO_PKG_VERSION"))
            .with_tools(true)
            .with_instructions("Use the 'ruskel' tool to generate Rust API skeletons for crates, modules, or filesystem paths."))
    }

    async fn tools_list(&mut self) -> Result<ListToolsResult> {
        Ok(ListToolsResult::new().with_tool(self.tool_metadata()))
    }

    async fn tools_call(
        &mut self,
        name: String,
        arguments: Option<serde_json::Value>,
    ) -> Result<CallToolResult> {
        if name == "ruskel" {
            Ok(self.execute_tool(arguments).await)
        } else {
            Err(tenx_mcp::Error::ToolNotFound(name))
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

    let server = Server::default()
        .with_connection_factory(move || Box::new(RuskelConnection::new(ruskel.clone())))
        .with_capabilities(ServerCapabilities::default().with_tools(None));

    match addr {
        Some(addr) => {
            tracing::info!("Starting MCP server on {}", addr);
            server.serve_tcp(addr).await
        }
        None => server.serve_stdio().await,
    }
}
