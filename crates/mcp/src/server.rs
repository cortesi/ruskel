use async_trait::async_trait;
use libruskel::Ruskel;
use serde_json::Value;
use std::collections::HashMap;
use tenx_mcp::{
    server::{MCPServer, ToolHandler},
    schema::{Tool, ToolInputSchema, Content, TextContent},
    transport::StdioTransport,
    error::{MCPError, Result},
};
use tracing::error;

use crate::tools::RuskelSkeletonTool;

pub struct RuskelSkeletonToolHandler {
    ruskel: Ruskel,
}

#[async_trait]
impl ToolHandler for RuskelSkeletonToolHandler {
    fn metadata(&self) -> Tool {
        let mut properties = HashMap::new();
        properties.insert(
            "target".to_string(),
            serde_json::json!({
                "type": "string",
                "description": "Target to generate - a directory, file path, or a module name"
            }),
        );
        properties.insert(
            "no_default_features".to_string(),
            serde_json::json!({
                "type": "boolean",
                "description": "Disable default features",
                "default": false
            }),
        );
        properties.insert(
            "all_features".to_string(),
            serde_json::json!({
                "type": "boolean",
                "description": "Enable all features",
                "default": false
            }),
        );
        properties.insert(
            "features".to_string(),
            serde_json::json!({
                "type": "array",
                "items": { "type": "string" },
                "description": "Specify features to enable",
                "default": []
            }),
        );
        properties.insert(
            "private_items".to_string(),
            serde_json::json!({
                "type": "boolean",
                "description": "Render private items",
                "default": false
            }),
        );

        Tool {
            name: "ruskel_skeleton".to_string(),
            description: Some("Generate a skeletonized outline of a Rust crate".to_string()),
            input_schema: ToolInputSchema {
                schema_type: "object".to_string(),
                properties: Some(properties),
                required: Some(vec!["target".to_string()]),
            },
            annotations: None,
        }
    }

    async fn execute(&self, arguments: Option<Value>) -> Result<Vec<Content>> {
        let arguments = arguments
            .ok_or_else(|| MCPError::invalid_params("ruskel_skeleton", "Missing arguments"))?;

        let tool_params: RuskelSkeletonTool = serde_json::from_value(arguments)
            .map_err(|e| MCPError::invalid_params("ruskel_skeleton", e.to_string()))?;

        match self.ruskel.render(
            &tool_params.target,
            tool_params.no_default_features,
            tool_params.all_features,
            tool_params.features,
            tool_params.private_items,
        ) {
            Ok(output) => Ok(vec![Content::Text(TextContent {
                text: output,
                annotations: None,
            })]),
            Err(e) => {
                error!("Failed to generate skeleton: {}", e);
                Err(MCPError::tool_execution_failed("ruskel_skeleton", e.to_string()))
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
    );

    let handler = RuskelSkeletonToolHandler { ruskel };
    server.register_tool(Box::new(handler)).await;

    let transport = Box::new(StdioTransport::new());
    server.serve(transport).await?;

    Ok(())
}
