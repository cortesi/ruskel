//! Integration tests for the MCP server
//!
//! These tests verify the MCP server protocol implementation using the tmcp
//! client.

use std::{
    env, io,
    path::PathBuf,
    process,
    result::Result as StdResult,
    sync::OnceLock,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use libruskel::{Ruskel, toolchain::ensure_nightly_with_docs};
use ruskel_mcp::{RuskelServer, RuskelServerDefaults};
use tmcp::{
    Arguments, Client, Result, Server,
    schema::{CallToolResult, InitializeResult},
};
use tokio::{
    io::{duplex, split},
    task::JoinHandle,
    time::timeout,
};

static STD_DOCS_PREREQUISITE: OnceLock<()> = OnceLock::new();
type ServerTask = JoinHandle<()>;

/// Helper to create a test MCP client connected to an in-process server.
async fn create_test_client() -> Result<(Client, ServerTask)> {
    create_test_client_with_defaults(RuskelServerDefaults::default()).await
}

/// Helper to create a test MCP client with explicit default request values.
async fn create_test_client_with_defaults(
    defaults: RuskelServerDefaults,
) -> Result<(Client, ServerTask)> {
    create_test_client_with_defaults_and_cache(defaults, None).await
}

/// Helper to create a test MCP client with an optional isolated cache root.
async fn create_test_client_with_defaults_and_cache(
    defaults: RuskelServerDefaults,
    cache_dir: Option<PathBuf>,
) -> Result<(Client, ServerTask)> {
    let ruskel = Ruskel::new().with_silent(true).with_cache_dir(cache_dir);
    let server = Server::new(move || RuskelServer::with_defaults(ruskel.clone(), defaults));

    let (server_side, client_side) = duplex(64 * 1024);
    let (server_reader, server_writer) = split(server_side);
    let (client_reader, client_writer) = split(client_side);

    let server_task = tokio::spawn(async move {
        if let Err(err) = server.serve_stream(server_reader, server_writer).await {
            eprintln!("test MCP server stopped: {err}");
        }
    });

    let mut client = Client::new("test-client", "1.0.0");
    client
        .connect_stream_raw(client_reader, client_writer)
        .await?;

    Ok((client, server_task))
}

/// Return a cache path that this test can prove remains untouched.
fn isolated_cache_path() -> PathBuf {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be after the Unix epoch")
        .as_nanos();
    env::temp_dir().join(format!(
        "ruskel-mcp-empty-search-{}-{timestamp}",
        process::id()
    ))
}

/// Create a client for a request that needs installed standard-library JSON.
async fn create_rustdoc_test_client() -> Result<(Client, ServerTask)> {
    create_rustdoc_test_client_with_defaults(RuskelServerDefaults::default()).await
}

/// Create a standard-library client with explicit server defaults.
async fn create_rustdoc_test_client_with_defaults(
    defaults: RuskelServerDefaults,
) -> Result<(Client, ServerTask)> {
    STD_DOCS_PREREQUISITE.get_or_init(|| {
        let available =
            ensure_nightly_with_docs().expect("nightly toolchain prerequisite check failed");
        require_rustdoc_json(available).expect(
            "rust-docs-json is required for MCP integration tests; install it with: \
             rustup component add --toolchain nightly rust-docs-json",
        );
    });
    create_test_client_with_defaults(defaults).await
}

/// Validate the standard-library JSON prerequisite.
fn require_rustdoc_json(available: bool) -> StdResult<(), &'static str> {
    if available {
        Ok(())
    } else {
        Err("missing nightly rust-docs-json component")
    }
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
    use tmcp::schema::{ContentBlock, SupportedProtocolVersions};

    use super::*;

    fn response_text(result: &CallToolResult) -> &str {
        result
            .content
            .iter()
            .find_map(|content| match content {
                ContentBlock::Text(text) => Some(text.text.as_str()),
                _ => None,
            })
            .expect("tool response should contain text")
    }

    #[test]
    fn rustdoc_json_prerequisite_rejects_missing_component() {
        assert_eq!(
            require_rustdoc_json(false),
            Err("missing nightly rust-docs-json component")
        );
        assert_eq!(require_rustdoc_json(true), Ok(()));
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
        assert_eq!(
            &result.protocol_version,
            SupportedProtocolVersions::default().preferred()
        );
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
        let (mut client, mut child) = create_rustdoc_test_client()
            .await
            .expect("Failed to create test client");

        let _init_result = initialize_client(&mut client)
            .await
            .expect("Failed to initialize");

        let arguments = json!({
            "target": "std::option::Option",
            "private": false
        });

        let args = Arguments::from_struct(arguments).expect("invalid arguments struct");
        let result = timeout(Duration::from_secs(30), client.call_tool("ruskel", args))
            .await
            .expect("Timeout during tool call")
            .expect("Failed to call tool");

        assert_ne!(result.is_error, Some(true));
        let text = response_text(&result);
        assert!(text.contains("pub enum Option<T>"));
        assert!(text.contains("None"));
        assert!(text.contains("Some(T)"));

        // Clean up
        terminate_child(&mut child)
            .await
            .expect("Failed to stop MCP server");
    }

    #[tokio::test]
    async fn test_mcp_server_null_search_renders_normally() {
        let (mut client, mut child) = create_rustdoc_test_client()
            .await
            .expect("Failed to create test client");

        initialize_client(&mut client)
            .await
            .expect("Failed to initialize");

        let arguments = json!({
            "target": "std::option::Option",
            "search": null,
            "frontmatter": false
        });
        let args = Arguments::from_struct(arguments).expect("invalid arguments struct");
        let result = timeout(Duration::from_secs(30), client.call_tool("ruskel", args))
            .await
            .expect("Timeout during null-search render")
            .expect("Failed to call tool");

        assert_ne!(result.is_error, Some(true));
        let text = response_text(&result);
        assert!(text.contains("pub enum Option<T>"));
        assert!(text.contains("None"));
        assert!(text.contains("Some(T)"));

        terminate_child(&mut child)
            .await
            .expect("Failed to stop MCP server");
    }

    #[tokio::test]
    async fn test_mcp_server_applies_frontmatter_startup_default() {
        let defaults = RuskelServerDefaults {
            private: true,
            frontmatter: false,
        };
        let (mut client, mut child) = create_rustdoc_test_client_with_defaults(defaults)
            .await
            .expect("Failed to create test client");

        let _init_result = initialize_client(&mut client)
            .await
            .expect("Failed to initialize");

        let arguments = json!({
            "target": "std::option::Option"
        });

        let args = Arguments::from_struct(arguments).expect("invalid arguments struct");
        let result = timeout(Duration::from_secs(30), client.call_tool("ruskel", args))
            .await
            .expect("Timeout during tool call")
            .expect("Failed to call tool");

        let text = response_text(&result);
        assert!(text.contains("pub enum Option<T>"));
        assert!(!text.contains("Ruskel skeleton"));

        terminate_child(&mut child)
            .await
            .expect("Failed to stop MCP server");
    }

    #[tokio::test]
    async fn test_mcp_server_empty_search_returns_without_resolving_target() {
        let cache_dir = isolated_cache_path();
        assert!(!cache_dir.exists(), "isolated cache path should be new");

        let (mut client, mut child) = create_test_client_with_defaults_and_cache(
            RuskelServerDefaults::default(),
            Some(cache_dir.clone()),
        )
        .await
        .expect("Failed to create test client");
        initialize_client(&mut client)
            .await
            .expect("Failed to initialize");

        for search in ["", " \t\n"] {
            let arguments = json!({
                "target": "crate\n[workspace]",
                "search": search
            });
            let args = Arguments::from_struct(arguments).expect("invalid arguments struct");
            let result = client
                .call_tool("ruskel", args)
                .await
                .expect("Failed to call tool");

            assert_ne!(result.is_error, Some(true));
            assert_eq!(
                response_text(&result),
                "Search query is empty; nothing to do."
            );
        }

        terminate_child(&mut child)
            .await
            .expect("Failed to stop MCP server");
        assert!(
            !cache_dir.exists(),
            "empty search should not initialize the cache"
        );
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
        let result = client.call_tool("non_existent_tool", ()).await;

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
        let result = client.call_tool("ruskel", args).await;

        // Should get an error response in the content
        match result {
            Ok(call_result) => {
                // Check if it's an error response
                assert!(
                    call_result.is_error.unwrap_or(false)
                        || call_result.content.iter().any(|c| {
                            if let ContentBlock::Text(text) = c {
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
    async fn test_mcp_server_rejects_invalid_package_name() {
        let (mut client, mut child) = create_test_client()
            .await
            .expect("Failed to create test client");
        initialize_client(&mut client)
            .await
            .expect("Failed to initialize");

        let arguments = json!({
            "target": "crate\n[workspace]",
            "frontmatter": false
        });
        let args = Arguments::from_struct(arguments).expect("invalid arguments struct");
        let result = client
            .call_tool("ruskel", args)
            .await
            .expect("Failed to call tool");

        assert_eq!(result.is_error, Some(true));
        assert!(response_text(&result).contains("Invalid package name"));

        terminate_child(&mut child)
            .await
            .expect("Failed to stop MCP server");
    }

    #[tokio::test]
    async fn test_mcp_server_multiple_requests() {
        let (mut client, mut child) = create_rustdoc_test_client()
            .await
            .expect("Failed to create test client");

        let _init_result = initialize_client(&mut client)
            .await
            .expect("Failed to initialize");

        // Test multiple sequential requests
        let test_targets = [
            ("std::option::Option", "pub enum Option<T>"),
            ("std::result::Result", "pub enum Result<T, E>"),
            ("std::vec::Vec", "pub struct Vec<"),
        ];

        for (target, anchor) in test_targets {
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
            let result = timeout(Duration::from_secs(30), client.call_tool("ruskel", args))
                .await
                .unwrap_or_else(|_| panic!("Timeout for target {target}"))
                .expect("tool request failed");
            assert_ne!(result.is_error, Some(true));
            assert!(response_text(&result).contains(anchor));
        }

        // Clean up
        terminate_child(&mut child)
            .await
            .expect("Failed to stop MCP server");
    }

    #[tokio::test]
    async fn test_mcp_server_error_recovery() {
        let (mut client, mut child) = create_rustdoc_test_client()
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
        let result = client.call_tool("non_existent_tool", ()).await;
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
        let result = client.call_tool("ruskel", args).await;
        match result {
            Ok(call_result) => {
                // Check if it's an error response
                assert!(
                    call_result.is_error.unwrap_or(false)
                        || call_result.content.iter().any(|c| {
                            if let ContentBlock::Text(text) = c {
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
            "target": "std::option::Option",
            "private": false
        });

        let args = Arguments::from_struct(final_args).expect("invalid arguments struct");
        let result = timeout(Duration::from_secs(30), client.call_tool("ruskel", args))
            .await
            .expect("Timeout during final request");

        let call_result = result.expect("valid request after errors");
        assert!(response_text(&call_result).contains("pub enum Option<T>"));

        // Clean up
        terminate_child(&mut child)
            .await
            .expect("Failed to stop MCP server");
    }

    #[tokio::test]
    async fn test_mcp_server_rejects_invalid_search_spec() {
        let (mut client, mut child) = create_test_client()
            .await
            .expect("Failed to create test client");

        let _init_result = initialize_client(&mut client)
            .await
            .expect("Failed to initialize");

        let arguments = json!({
            "target": "serde",
            "search": "serde",
            "search_spec": ["bogus"]
        });

        let args = Arguments::from_struct(arguments).expect("invalid arguments struct");
        let result = client
            .call_tool("ruskel", args)
            .await
            .expect("Failed to call tool");

        assert_eq!(result.is_error, Some(true));
        assert!(result.content.iter().any(|content| {
            if let ContentBlock::Text(text) = content {
                text.text.contains("invalid search domain 'bogus'")
            } else {
                false
            }
        }));

        terminate_child(&mut child)
            .await
            .expect("Failed to stop MCP server");
    }

    #[tokio::test]
    async fn test_mcp_server_searches_real_std_json() {
        let (mut client, mut child) = create_rustdoc_test_client()
            .await
            .expect("Failed to create test client");
        initialize_client(&mut client)
            .await
            .expect("Failed to initialize");

        let arguments = json!({
            "target": "std::option::Option",
            "search": "std::option::Option::Some",
            "search_spec": ["path"],
            "frontmatter": false
        });
        let args = Arguments::from_struct(arguments).expect("invalid arguments struct");
        let result = timeout(Duration::from_secs(30), client.call_tool("ruskel", args))
            .await
            .expect("Timeout during search")
            .expect("Failed to search");

        assert_ne!(result.is_error, Some(true));
        let text = response_text(&result);
        assert!(text.contains("matches for \"std::option::Option::Some\""));
        assert!(text.contains("std::option::Option::Some [path]"));
        assert!(text.contains("Some(T)"));

        terminate_child(&mut child)
            .await
            .expect("Failed to stop MCP server");
    }

    #[tokio::test]
    async fn test_mcp_server_reports_real_render_error() {
        let (mut client, mut child) = create_rustdoc_test_client()
            .await
            .expect("Failed to create test client");
        initialize_client(&mut client)
            .await
            .expect("Failed to initialize");

        let arguments = json!({
            "target": "std::definitely_not_a_module",
            "frontmatter": false
        });
        let args = Arguments::from_struct(arguments).expect("invalid arguments struct");
        let result = timeout(Duration::from_secs(30), client.call_tool("ruskel", args))
            .await
            .expect("Timeout during failing render")
            .expect("Failed to call tool");

        assert_eq!(result.is_error, Some(true));
        let text = response_text(&result);
        assert!(text.contains("Failed to generate skeleton"));
        assert!(text.contains("std::definitely_not_a_module"));

        terminate_child(&mut child)
            .await
            .expect("Failed to stop MCP server");
    }
}
