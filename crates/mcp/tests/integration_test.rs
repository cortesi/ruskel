//! Integration tests for the MCP server
//!
//! These tests verify the MCP server protocol implementation using the tenx-mcp client.

use serde_json::json;
use std::process::Command;
use std::time::Duration;
use tenx_mcp::{
    client::Client,
    error::Result,
    schema::{ClientCapabilities, Implementation, InitializeResult},
};
use tokio::process::Command as TokioCommand;
use tokio::time::timeout;

/// Helper to create a test MCP client connected to the ruskel server process
async fn create_test_client() -> Result<(Client, tokio::process::Child)> {
    // Get the workspace root by going up from the test directory
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let workspace_root = std::path::Path::new(manifest_dir)
        .parent() // crates
        .unwrap()
        .parent() // workspace root
        .unwrap();

    // First ensure the binary is built
    let output = Command::new("cargo")
        .current_dir(workspace_root)
        .args(["build", "--bin", "ruskel"])
        .output()
        .expect("Failed to build ruskel");

    if !output.status.success() {
        panic!(
            "Failed to build ruskel: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    // Find the target directory
    let target_dir = workspace_root.join("target");
    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    let binary_path = target_dir.join(profile).join("ruskel");

    // Create client and connect to process
    let mut client = Client::new().with_request_timeout(Duration::from_secs(30));

    let mut cmd = TokioCommand::new(binary_path);
    cmd.arg("--mcp");

    let child = client.connect_process(cmd).await?;

    Ok((client, child))
}

/// Initialize the client connection
async fn initialize_client(client: &mut Client) -> Result<InitializeResult> {
    let client_info = Implementation {
        name: "test-client".to_string(),
        version: "1.0.0".to_string(),
    };

    let capabilities = ClientCapabilities::default();

    client.initialize(client_info, capabilities).await
}

#[tokio::test]
async fn test_mcp_server_initialize() {
    let (mut client, mut child) = create_test_client()
        .await
        .expect("Failed to create test client");

    let result = timeout(Duration::from_secs(10), initialize_client(&mut client))
        .await
        .expect("Timeout during initialization")
        .expect("Failed to initialize");

    // Verify response structure
    assert_eq!(result.protocol_version, "2025-03-26");
    assert_eq!(result.server_info.name, "Ruskel MCP Server");

    // Clean up
    let _ = child.kill().await;
}

#[tokio::test]
async fn test_mcp_server_list_tools() {
    let (mut client, mut child) = create_test_client()
        .await
        .expect("Failed to create test client");

    let _init_result = initialize_client(&mut client)
        .await
        .expect("Failed to initialize");

    let result = timeout(Duration::from_secs(10), client.list_tools())
        .await
        .expect("Timeout listing tools")
        .expect("Failed to list tools");

    // Verify response
    assert_eq!(result.tools.len(), 1);
    let tool = &result.tools[0];
    assert_eq!(tool.name, "ruskel");
    assert!(tool.description.is_some());

    // Clean up
    let _ = child.kill().await;
}

#[tokio::test]
async fn test_mcp_server_call_tool() {
    let (mut client, mut child) = create_test_client()
        .await
        .expect("Failed to create test client");

    let _init_result = initialize_client(&mut client)
        .await
        .expect("Failed to initialize");

    // Call tool with a crate that should exist
    let arguments = Some(json!({
        "target": "serde",
        "private": false
    }));

    let result = timeout(
        Duration::from_secs(30),
        client.call_tool("ruskel", arguments),
    )
    .await
    .expect("Timeout during tool call")
    .expect("Failed to call tool");

    // Verify response
    assert!(!result.content.is_empty());

    // Clean up
    let _ = child.kill().await;
}

#[tokio::test]
async fn test_mcp_server_invalid_tool() {
    let (mut client, mut child) = create_test_client()
        .await
        .expect("Failed to create test client");

    let _init_result = initialize_client(&mut client)
        .await
        .expect("Failed to initialize");

    // Call non-existent tool
    let result = client.call_tool("non_existent_tool", Some(json!({}))).await;

    // Should get an error
    assert!(result.is_err());

    // Clean up
    let _ = child.kill().await;
}

#[tokio::test]
async fn test_mcp_server_invalid_arguments() {
    let (mut client, mut child) = create_test_client()
        .await
        .expect("Failed to create test client");

    let _init_result = initialize_client(&mut client)
        .await
        .expect("Failed to initialize");

    // Call tool without required target parameter
    let arguments = Some(json!({
        "private": true
        // Missing required "target" field
    }));

    let result = client.call_tool("ruskel", arguments).await;

    // Should get an error
    assert!(result.is_ok());
    assert!(result.unwrap().is_error.unwrap());

    // Clean up
    let _ = child.kill().await;
}

#[tokio::test]
async fn test_mcp_server_multiple_requests() {
    let (mut client, mut child) = create_test_client()
        .await
        .expect("Failed to create test client");

    let _init_result = initialize_client(&mut client)
        .await
        .expect("Failed to initialize");

    // Test multiple sequential requests
    let test_targets = ["serde", "tokio", "async-trait"];

    for target in &test_targets {
        // List tools request
        let _list_result = timeout(Duration::from_secs(10), client.list_tools())
            .await
            .expect("Timeout listing tools")
            .expect("Failed to list tools");

        // Call tool request
        let arguments = Some(json!({
            "target": target,
            "private": false
        }));

        let result = timeout(
            Duration::from_secs(30),
            client.call_tool("ruskel", arguments),
        )
        .await
        .unwrap_or_else(|_| panic!("Timeout for target {target}"));

        if let Ok(call_result) = result {
            assert!(!call_result.content.is_empty());
        }
    }

    // Clean up
    let _ = child.kill().await;
}

#[tokio::test]
async fn test_mcp_server_error_recovery() {
    let (mut client, mut child) = create_test_client()
        .await
        .expect("Failed to create test client");

    let _init_result = initialize_client(&mut client)
        .await
        .expect("Failed to initialize");

    // 1. Valid request
    let result = timeout(Duration::from_secs(10), client.list_tools())
        .await
        .expect("Timeout listing tools")
        .expect("Failed to list tools");
    assert!(!result.tools.is_empty());

    // 2. Invalid tool name (should error)
    let result = client.call_tool("non_existent_tool", Some(json!({}))).await;
    assert!(result.is_err());

    // 3. Valid request after error (server should recover)
    let result = timeout(Duration::from_secs(10), client.list_tools())
        .await
        .expect("Timeout listing tools after error")
        .expect("Failed to list tools after error");
    assert!(!result.tools.is_empty());

    // 4. Invalid arguments (should error)
    let invalid_args = Some(json!({
        // Missing required "target"
        "private": true
    }));

    let result = client.call_tool("ruskel", invalid_args).await;
    assert!(result.is_ok());
    assert!(result.unwrap().is_error.unwrap());

    // 5. Valid request after another error
    let final_args = Some(json!({
        "target": "serde",
        "private": false
    }));

    let result = timeout(
        Duration::from_secs(30),
        client.call_tool("ruskel", final_args),
    )
    .await
    .expect("Timeout during final request");

    if let Ok(call_result) = result {
        assert!(!call_result.content.is_empty());
    }

    // Clean up
    let _ = child.kill().await;
}
