use async_trait::async_trait;
use libruskel::Ruskel;
use rust_mcp_schema::schema_utils::CallToolError;
use rust_mcp_schema::{
    CallToolRequest, CallToolResult, Implementation, InitializeResult, ListToolsRequest,
    ListToolsResult, RpcError, ServerCapabilities, ServerCapabilitiesTools, Tool,
    LATEST_PROTOCOL_VERSION,
};
use rust_mcp_sdk::error::SdkResult;
use rust_mcp_sdk::mcp_server::{server_runtime, ServerHandler, ServerRuntime};
use rust_mcp_sdk::{MCPServer, StdioTransport, TransportOptions};
use tracing::error;

use crate::tools::RuskelSkeletonTool;

pub struct RuskelServerHandler {
    ruskel: Ruskel,
}

#[async_trait]
impl ServerHandler for RuskelServerHandler {
    async fn on_server_started(&self, _runtime: &dyn MCPServer) {
        // Do nothing - no output
    }

    async fn handle_list_tools_request(
        &self,
        _request: ListToolsRequest,
        _runtime: &dyn MCPServer,
    ) -> Result<ListToolsResult, RpcError> {
        Ok(ListToolsResult {
            tools: vec![Tool {
                name: "ruskel_skeleton".to_string(),
                description: Some("Generate a skeletonized outline of a Rust crate".to_string()),
                input_schema: rust_mcp_schema::ToolInputSchema::new(
                    vec!["target".to_string()],
                    None,
                ),
            }],
            meta: None,
            next_cursor: None,
        })
    }

    async fn handle_call_tool_request(
        &self,
        request: CallToolRequest,
        _runtime: &dyn MCPServer,
    ) -> Result<CallToolResult, CallToolError> {
        let params = &request.params;

        if params.name != "ruskel_skeleton" {
            return Err(CallToolError::new(
                rust_mcp_schema::schema_utils::UnknownTool(params.name.clone()),
            ));
        }

        let tool_params: RuskelSkeletonTool = serde_json::from_value(serde_json::Value::Object(
            params.arguments.clone().unwrap_or_default(),
        ))
        .map_err(CallToolError::new)?;

        match self.ruskel.render(
            &tool_params.target,
            tool_params.no_default_features,
            tool_params.all_features,
            tool_params.features,
        ) {
            Ok(output) => Ok(CallToolResult::text_content(output, None)),
            Err(e) => {
                error!("Failed to generate skeleton: {}", e);
                Err(CallToolError::new(e))
            }
        }
    }
}

pub async fn run_mcp_server(ruskel: Ruskel) -> SdkResult<()> {
    // Only initialize tracing if RUST_LOG is set
    if std::env::var("RUST_LOG").is_ok() {
        tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::from_default_env()
                    .add_directive("ruskel_mcp=debug".parse().unwrap())
                    .add_directive("rust_mcp_sdk=debug".parse().unwrap()),
            )
            .with_writer(std::io::stderr)
            .init();
    }

    let server_details = InitializeResult {
        protocol_version: LATEST_PROTOCOL_VERSION.to_string(),
        server_info: Implementation {
            name: "Ruskel MCP Server".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        },
        capabilities: ServerCapabilities {
            tools: Some(ServerCapabilitiesTools { list_changed: None }),
            ..Default::default()
        },
        meta: None,
        instructions: Some(
            "Ruskel MCP Server - Generate skeletonized outlines of Rust crates".to_string(),
        ),
    };

    let transport = StdioTransport::new(TransportOptions::default())?;
    let handler = RuskelServerHandler { ruskel };

    let server: ServerRuntime = server_runtime::create_server(server_details, transport, handler);

    server.start().await?;

    Ok(())
}
