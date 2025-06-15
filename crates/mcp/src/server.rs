use async_trait::async_trait;
use libruskel::Ruskel;
use tenx_mcp::{
    error::{MCPError, Result},
    schema::{Content, ServerCapabilities, TextContent, Tool, ToolInputSchema, ToolsCapability},
    server::{MCPServer, ToolHandler},
    transport::StdioTransport,
};
use tracing::error;

use crate::tools::RuskelSkeletonTool;

pub struct RuskelToolHandler {
    ruskel: Ruskel,
}

impl RuskelToolHandler {
    pub fn new(ruskel: Ruskel) -> Self {
        Self { ruskel }
    }
}

#[async_trait]
impl ToolHandler for RuskelToolHandler {
    fn metadata(&self) -> Tool {
        let mut properties = std::collections::HashMap::new();

        properties.insert("target".to_string(), serde_json::json!({
            "type": "string",
            "description": "Crate, module path, or filesystem path (optionally with @<semver>) whose API skeleton should be produced."
        }));

        properties.insert(
            "private".to_string(),
            serde_json::json!({
                "type": "boolean",
                "description": "Include non‑public (private / crate‑private) items.",
                "default": false
            }),
        );

        properties.insert(
            "no_default_features".to_string(),
            serde_json::json!({
                "type": "boolean",
                "description": "Disable the crate's default Cargo features.",
                "default": false
            }),
        );

        properties.insert(
            "all_features".to_string(),
            serde_json::json!({
                "type": "boolean",
                "description": "Enable every optional Cargo feature.",
                "default": false
            }),
        );

        properties.insert("features".to_string(), serde_json::json!({
            "type": "array",
            "items": {"type": "string"},
            "description": "Exact list of Cargo features to enable (ignored if all_features=true).",
            "default": []
        }));

        Tool {
            name: "ruskel".to_string(),
            description: Some(include_str!("../ruskel-description.txt").to_string()),
            input_schema: ToolInputSchema {
                schema_type: "object".to_string(),
                properties: Some(properties),
                required: Some(vec!["target".to_string()]),
            },
            annotations: None,
        }
    }

    async fn execute(&self, arguments: Option<serde_json::Value>) -> Result<Vec<Content>> {
        let args = arguments.unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

        let tool_params: RuskelSkeletonTool =
            serde_json::from_value(args).map_err(|e| MCPError::InvalidParams {
                method: "ruskel".to_string(),
                message: e.to_string(),
            })?;

        match self.ruskel.render(
            &tool_params.target,
            tool_params.no_default_features,
            tool_params.all_features,
            tool_params.features,
            tool_params.private,
        ) {
            Ok(output) => Ok(vec![Content::Text(TextContent {
                text: output,
                annotations: None,
            })]),
            Err(e) => {
                error!("Failed to generate skeleton: {}", e);
                Err(MCPError::ToolExecutionFailed {
                    tool: "ruskel".to_string(),
                    message: e.to_string(),
                })
            }
        }
    }
}

pub async fn run_mcp_server(ruskel: Ruskel) -> Result<()> {
    // Only initialize tracing if RUST_LOG is set
    if std::env::var("RUST_LOG").is_ok() {
        tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::from_default_env()
                    .add_directive("ruskel_mcp=debug".parse().unwrap())
                    .add_directive("tenx_mcp=debug".parse().unwrap()),
            )
            .with_writer(std::io::stderr)
            .init();
    }

    let mut server = MCPServer::new(
        "Ruskel MCP Server".to_string(),
        env!("CARGO_PKG_VERSION").to_string(),
    )
    .with_capabilities(ServerCapabilities {
        tools: Some(ToolsCapability { list_changed: None }),
        ..Default::default()
    });

    let tool_handler = RuskelToolHandler::new(ruskel);
    server.register_tool(Box::new(tool_handler)).await;

    let transport = Box::new(StdioTransport::new());
    server.serve(transport).await?;

    Ok(())
}
