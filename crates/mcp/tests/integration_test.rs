//! Integration tests for the MCP server
//!
//! These tests verify the MCP server protocol implementation using the tenx-mcp client.

use std::{env, io, sync::OnceLock, time::Duration};

use libruskel::Ruskel;
use ruskel_mcp::RuskelServer;
use tenx_mcp::{Arguments, Client, Result, Server, ServerAPI, schema::InitializeResult};
use tokio::{
    io::{duplex, split},
    task::JoinHandle,
    time::timeout,
};

static TEST_MODE_ENV: OnceLock<()> = OnceLock::new();
type ServerTask = JoinHandle<()>;

/// Helper to create a test MCP client connected to an in-process server.
async fn create_test_client() -> Result<(Client, ServerTask)> {
    TEST_MODE_ENV.get_or_init(|| unsafe {
        env::set_var("RUSKEL_MCP_TEST_MODE", "1");
    });

    let ruskel = Ruskel::new().with_silent(true);
    let server = Server::default().with_connection(move || RuskelServer::new(ruskel.clone()));

    let (server_side, client_side) = duplex(64 * 1024);
    let (server_reader, server_writer) = split(server_side);
    let (client_reader, client_writer) = split(client_side);

    let server_task = tokio::spawn(async move {
        if let Err(err) = server.serve_stream(server_reader, server_writer).await {
            eprintln!("test MCP server stopped: {err}");
        }
    });

    let mut client = Client::new("test-client", "1.0.0");
    client.connect_stream(client_reader, client_writer).await?;

    Ok((client, server_task))
}

/// Initialize the client connection
async fn initialize_client(client: &mut Client) -> Result<InitializeResult> {
    client.init().await
}

/// Terminate spawned MCP server process and surface unexpected failures.
async fn terminate_child(child: &mut ServerTask) -> io::Result<()> {
    child.abort();
    match child.await {
        Ok(()) => Ok(()),
        Err(err) if err.is_cancelled() => Ok(()),
        Err(err) => Err(io::Error::other(err)),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tenx_mcp::schema::Content;

    use super::*;

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
        assert_eq!(result.protocol_version, "2025-06-18");
        assert_eq!(result.server_info.name, "ruskel_server");

        // Clean up
        terminate_child(&mut child)
            .await
            .expect("Failed to stop MCP server");
    }

    #[tokio::test]
    async fn test_mcp_server_list_tools() {
        let (mut client, mut child) = create_test_client()
            .await
            .expect("Failed to create test client");

        let _init_result = initialize_client(&mut client)
            .await
            .expect("Failed to initialize");

        let result = timeout(Duration::from_secs(10), client.list_tools(None))
            .await
            .expect("Timeout listing tools")
            .expect("Failed to list tools");

        // Verify response
        assert_eq!(result.tools.len(), 1);
        let tool = &result.tools[0];
        assert_eq!(tool.name, "ruskel");
        assert!(tool.description.is_some());

        // Clean up
        terminate_child(&mut child)
            .await
            .expect("Failed to stop MCP server");
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
        let arguments = json!({
            "target": "serde",
            "private": false
        });

        let args = Arguments::from_struct(arguments).expect("invalid arguments struct");
        let result = timeout(
            Duration::from_secs(30),
            client.call_tool("ruskel", Some(args)),
        )
        .await
        .expect("Timeout during tool call")
        .expect("Failed to call tool");

        // Verify response
        assert!(!result.content.is_empty());

        // Clean up
        terminate_child(&mut child)
            .await
            .expect("Failed to stop MCP server");
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
        let result = client
            .call_tool("non_existent_tool", Some(Arguments::new()))
            .await;

        // Should get an error
        assert!(result.is_err());

        // Clean up
        terminate_child(&mut child)
            .await
            .expect("Failed to stop MCP server");
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
        let arguments = json!({
            "private": true
            // Missing required "target" field
        });

        let args = Arguments::from_struct(arguments).expect("invalid arguments struct");
        let result = client.call_tool("ruskel", Some(args)).await;

        // Should get an error response in the content
        match result {
            Ok(call_result) => {
                // Check if it's an error response
                assert!(
                    call_result.is_error.unwrap_or(false)
                        || call_result.content.iter().any(|c| {
                            if let Content::Text(text) = c {
                                text.text.contains("Invalid parameters")
                                    || text.text.contains("Failed to generate")
                            } else {
                                false
                            }
                        })
                );
            }
            Err(_) => {
                // This is also acceptable - the tool call failed
            }
        }

        // Clean up
        terminate_child(&mut child)
            .await
            .expect("Failed to stop MCP server");
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
            let _list_result = timeout(Duration::from_secs(10), client.list_tools(None))
                .await
                .expect("Timeout listing tools")
                .expect("Failed to list tools");

            // Call tool request
            let arguments = json!({
                "target": target,
                "private": false
            });

            let args = Arguments::from_struct(arguments).expect("invalid arguments struct");
            let result = timeout(
                Duration::from_secs(30),
                client.call_tool("ruskel", Some(args)),
            )
            .await
            .unwrap_or_else(|_| panic!("Timeout for target {target}"));

            if let Ok(call_result) = result {
                assert!(!call_result.content.is_empty());
            }
        }

        // Clean up
        terminate_child(&mut child)
            .await
            .expect("Failed to stop MCP server");
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
        let result = timeout(Duration::from_secs(10), client.list_tools(None))
            .await
            .expect("Timeout listing tools")
            .expect("Failed to list tools");
        assert!(!result.tools.is_empty());

        // 2. Invalid tool name (should error)
        let result = client
            .call_tool("non_existent_tool", Some(Arguments::new()))
            .await;
        assert!(result.is_err());

        // 3. Valid request after error (server should recover)
        let result = timeout(Duration::from_secs(10), client.list_tools(None))
            .await
            .expect("Timeout listing tools after error")
            .expect("Failed to list tools after error");
        assert!(!result.tools.is_empty());

        // 4. Invalid arguments (should error)
        let invalid_args = json!({
            // Missing required "target"
            "private": true
        });

        let args = Arguments::from_struct(invalid_args).expect("invalid arguments struct");
        let result = client.call_tool("ruskel", Some(args)).await;
        match result {
            Ok(call_result) => {
                // Check if it's an error response
                assert!(
                    call_result.is_error.unwrap_or(false)
                        || call_result.content.iter().any(|c| {
                            if let Content::Text(text) = c {
                                text.text.contains("Invalid parameters")
                                    || text.text.contains("Failed to generate")
                            } else {
                                false
                            }
                        })
                );
            }
            Err(_) => {
                // This is also acceptable - the tool call failed
            }
        }

        // 5. Valid request after another error
        let final_args = json!({
            "target": "serde",
            "private": false
        });

        let args = Arguments::from_struct(final_args).expect("invalid arguments struct");
        let result = timeout(
            Duration::from_secs(30),
            client.call_tool("ruskel", Some(args)),
        )
        .await
        .expect("Timeout during final request");

        if let Ok(call_result) = result {
            assert!(!call_result.content.is_empty());
        }

        // Clean up
        terminate_child(&mut child)
            .await
            .expect("Failed to stop MCP server");
    }
}
