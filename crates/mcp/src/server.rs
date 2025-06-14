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
                name: "ruskel".to_string(),
                description: Some(include_str!("../ruskel-description.txt").to_string()),
                input_schema: {
                    let mut properties = std::collections::HashMap::new();

                    let mut target_schema = serde_json::Map::new();
                    target_schema.insert("type".to_string(), serde_json::json!("string"));
                    target_schema.insert("description".to_string(), serde_json::json!("Crate, module path, or filesystem path (optionally with @<semver>) whose API skeleton should be produced."));
                    properties.insert("target".to_string(), target_schema);

                    let mut private_schema = serde_json::Map::new();
                    private_schema.insert("type".to_string(), serde_json::json!("boolean"));
                    private_schema.insert(
                        "description".to_string(),
                        serde_json::json!("Include non‑public (private / crate‑private) items."),
                    );
                    private_schema.insert("default".to_string(), serde_json::json!(false));
                    properties.insert("private".to_string(), private_schema);

                    let mut no_default_features_schema = serde_json::Map::new();
                    no_default_features_schema
                        .insert("type".to_string(), serde_json::json!("boolean"));
                    no_default_features_schema.insert(
                        "description".to_string(),
                        serde_json::json!("Disable the crate's default Cargo features."),
                    );
                    no_default_features_schema
                        .insert("default".to_string(), serde_json::json!(false));
                    properties.insert(
                        "no_default_features".to_string(),
                        no_default_features_schema,
                    );

                    let mut all_features_schema = serde_json::Map::new();
                    all_features_schema.insert("type".to_string(), serde_json::json!("boolean"));
                    all_features_schema.insert(
                        "description".to_string(),
                        serde_json::json!("Enable every optional Cargo feature."),
                    );
                    all_features_schema.insert("default".to_string(), serde_json::json!(false));
                    properties.insert("all_features".to_string(), all_features_schema);

                    let mut features_schema = serde_json::Map::new();
                    features_schema.insert("type".to_string(), serde_json::json!("array"));
                    let mut items = serde_json::Map::new();
                    items.insert("type".to_string(), serde_json::json!("string"));
                    features_schema.insert("items".to_string(), serde_json::json!(items));
                    features_schema.insert("description".to_string(), serde_json::json!("Exact list of Cargo features to enable (ignored if all_features=true)."));
                    features_schema.insert("default".to_string(), serde_json::json!([]));
                    properties.insert("features".to_string(), features_schema);

                    rust_mcp_schema::ToolInputSchema::new(
                        vec!["target".to_string()],
                        Some(properties),
                    )
                },
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

        if params.name != "ruskel" {
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
            tool_params.private,
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
