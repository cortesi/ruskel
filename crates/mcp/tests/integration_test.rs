//! Integration tests for the MCP server
//!
//! These tests verify the MCP server protocol implementation using the Transport trait.

use async_trait::async_trait;
use libruskel::Ruskel;
use serde_json::json;
use std::process::{Command, Stdio};
use std::time::Duration;
use tenx_mcp::{
    client::{ClientConfig, MCPClient},
    error::Result,
    schema::{ClientCapabilities, Implementation, InitializeResult},
    transport::{Transport, TransportStream},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
use tokio::process::Command as TokioCommand;
use tokio::time::timeout;
use tokio_util::codec::Framed;

/// Test transport that uses in-memory duplex streams
pub struct TestTransport {
    stream: DuplexStream,
}

impl TestTransport {
    pub fn new_pair() -> (Self, Self) {
        let (client_stream, server_stream) = tokio::io::duplex(8192);
        (
            Self {
                stream: client_stream,
            },
            Self {
                stream: server_stream,
            },
        )
    }
}

#[async_trait]
impl Transport for TestTransport {
    async fn connect(&mut self) -> Result<()> {
        Ok(())
    }

    fn framed(self: Box<Self>) -> Result<Box<dyn TransportStream>> {
        let framed = Framed::new(self.stream, tenx_mcp::codec::JsonRpcCodec::new());
        Ok(Box::new(framed))
    }
}

/// Helper to create a test MCP server and client pair
async fn create_test_pair() -> Result<(MCPClient, tokio::task::JoinHandle<Result<()>>)> {
    let (client_transport, server_transport) = TestTransport::new_pair();

    // Create and configure client
    let config = ClientConfig {
        request_timeout: Duration::from_secs(30),
        ..Default::default()
    };

    let mut client = MCPClient::with_config(config);
    client.connect(Box::new(client_transport)).await?;

    // Start server in background task
    let server_handle = tokio::spawn(async move {
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

        // Start the MCP server process
        let mut child = TokioCommand::new(binary_path)
            .args(["--mcp"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("Failed to start MCP server");

        let mut child_stdin = child.stdin.take().expect("Failed to get child stdin");
        let mut child_stdout = child.stdout.take().expect("Failed to get child stdout");

        // Split the server transport into read and write halves
        let (mut server_read, mut server_write) = tokio::io::split(server_transport.stream);

        // Pipe data between the test transport and the actual MCP server process
        let stdin_task =
            tokio::spawn(async move { tokio::io::copy(&mut server_read, &mut child_stdin).await });

        let stdout_task =
            tokio::spawn(
                async move { tokio::io::copy(&mut child_stdout, &mut server_write).await },
            );

        // Wait for the child process to complete or for piping to fail
        tokio::select! {
            result = child.wait() => {
                match result {
                    Ok(status) => {
                        if !status.success() {
                            eprintln!("MCP server exited with status: {}", status);
                        }
                    }
                    Err(e) => eprintln!("Error waiting for MCP server: {}", e),
                }
            }
            result = stdin_task => {
                if let Err(e) = result {
                    eprintln!("Error in stdin pipe: {}", e);
                }
            }
            result = stdout_task => {
                if let Err(e) = result {
                    eprintln!("Error in stdout pipe: {}", e);
                }
            }
        }

        // Clean up
        let _ = child.kill().await;
        Ok(())
    });

    Ok((client, server_handle))
}

/// Initialize the client connection
async fn initialize_client(client: &mut MCPClient) -> Result<InitializeResult> {
    let client_info = Implementation {
        name: "test-client".to_string(),
        version: "1.0.0".to_string(),
    };

    let capabilities = ClientCapabilities::default();

    client.initialize(client_info, capabilities).await
}

#[tokio::test]
async fn test_mcp_server_initialize() {
    let (mut client, server_handle) = create_test_pair()
        .await
        .expect("Failed to create test pair");

    let result = timeout(Duration::from_secs(10), initialize_client(&mut client))
        .await
        .expect("Timeout during initialization")
        .expect("Failed to initialize");

    // Verify response structure
    assert_eq!(result.protocol_version, "2025-03-26");
    assert_eq!(result.server_info.name, "Ruskel MCP Server");

    // Clean up
    server_handle.abort();
}

#[tokio::test]
async fn test_mcp_server_list_tools() {
    let (mut client, server_handle) = create_test_pair()
        .await
        .expect("Failed to create test pair");

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
    server_handle.abort();
}

#[tokio::test]
async fn test_mcp_server_call_tool() {
    let (mut client, server_handle) = create_test_pair()
        .await
        .expect("Failed to create test pair");

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
        client.call_tool("ruskel".to_string(), arguments),
    )
    .await
    .expect("Timeout during tool call")
    .expect("Failed to call tool");

    // Verify response
    assert!(!result.content.is_empty());

    // Clean up
    server_handle.abort();
}

#[tokio::test]
async fn test_mcp_server_invalid_tool() {
    let (mut client, server_handle) = create_test_pair()
        .await
        .expect("Failed to create test pair");

    let _init_result = initialize_client(&mut client)
        .await
        .expect("Failed to initialize");

    // Call non-existent tool
    let result = client
        .call_tool("non_existent_tool".to_string(), Some(json!({})))
        .await;

    // Should get an error
    assert!(result.is_err());

    // Clean up
    server_handle.abort();
}

#[tokio::test]
async fn test_mcp_server_invalid_arguments() {
    let (mut client, server_handle) = create_test_pair()
        .await
        .expect("Failed to create test pair");

    let _init_result = initialize_client(&mut client)
        .await
        .expect("Failed to initialize");

    // Call tool without required target parameter
    let arguments = Some(json!({
        "private": true
        // Missing required "target" field
    }));

    let result = client.call_tool("ruskel".to_string(), arguments).await;

    // Should get an error
    assert!(result.is_err());

    // Clean up
    server_handle.abort();
}

#[tokio::test]
async fn test_mcp_server_multiple_requests() {
    let (mut client, server_handle) = create_test_pair()
        .await
        .expect("Failed to create test pair");

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
            client.call_tool("ruskel".to_string(), arguments),
        )
        .await
        .unwrap_or_else(|_| panic!("Timeout for target {target}"));

        if let Ok(call_result) = result {
            assert!(!call_result.content.is_empty());
        }
    }

    // Clean up
    server_handle.abort();
}

#[tokio::test]
async fn test_mcp_server_error_recovery() {
    let (mut client, server_handle) = create_test_pair()
        .await
        .expect("Failed to create test pair");

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
    let result = client
        .call_tool("non_existent_tool".to_string(), Some(json!({})))
        .await;
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

    let result = client.call_tool("ruskel".to_string(), invalid_args).await;
    assert!(result.is_err());

    // 5. Valid request after another error
    let final_args = Some(json!({
        "target": "serde",
        "private": false
    }));

    let result = timeout(
        Duration::from_secs(30),
        client.call_tool("ruskel".to_string(), final_args),
    )
    .await
    .expect("Timeout during final request");

    if let Ok(call_result) = result {
        assert!(!call_result.content.is_empty());
    }

    // Clean up
    server_handle.abort();
}
