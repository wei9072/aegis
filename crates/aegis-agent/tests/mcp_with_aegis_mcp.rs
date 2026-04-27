//! V3.2b — integration test: spawn the real `aegis-mcp` binary and
//! drive it end-to-end via `StdioTransport` + `McpClient`.
//!
//! Skipped if the binary is not built — run `cargo build -p
//! aegis-mcp` first (or just `cargo build --workspace`) to enable.
//!
//! This is the "wire format actually matches" check: validates that
//! the V3.2b MCP client speaks the same protocol that `aegis-mcp`
//! ships, end-to-end, with no stubbing.

use aegis_agent::mcp::{McpClient, McpToolExecutor, StdioTransport};
use aegis_agent::tool::ToolExecutor;
use serde_json::{json, Value};

/// Locate the built `aegis-mcp` binary in the workspace target dir.
/// Tries `target/debug/aegis-mcp` first, then `target/release/`.
/// Returns `None` if neither exists.
fn find_aegis_mcp_binary() -> Option<std::path::PathBuf> {
    // CARGO_MANIFEST_DIR points at the aegis-agent crate.
    let manifest_dir =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // Workspace root is two levels up (crates/aegis-agent → ..).
    let workspace_root = manifest_dir.parent()?.parent()?;
    let target_dir = workspace_root.join("target");

    let candidates = [
        target_dir.join("debug").join("aegis-mcp"),
        target_dir.join("release").join("aegis-mcp"),
    ];
    candidates.into_iter().find(|p| p.exists())
}

#[test]
fn round_trip_with_real_aegis_mcp_binary() {
    let Some(binary) = find_aegis_mcp_binary() else {
        eprintln!("aegis-mcp binary not found in target/ — skipping integration test.");
        eprintln!("Build with: cargo build -p aegis-mcp");
        return;
    };
    eprintln!("using aegis-mcp at {}", binary.display());

    // 1. Spawn + handshake.
    let transport = StdioTransport::spawn(binary.to_str().unwrap(), &[])
        .expect("spawn aegis-mcp");
    let mut client = McpClient::new(Box::new(transport)).expect("initialize handshake");

    eprintln!("server: {} v{}", client.server_name, client.server_version);
    eprintln!("server protocol: {}", client.server_protocol_version);
    assert_eq!(client.server_name, "aegis");
    assert_eq!(client.server_protocol_version, "2025-06-18");

    // 2. Discover tools.
    let tools = client.list_tools().expect("list_tools");
    assert!(
        tools.iter().any(|t| t.name == "validate_change"),
        "aegis-mcp should advertise validate_change; got {tools:?}"
    );

    // 3. Call validate_change against a clean Python file (should
    //    PASS — no syntax error, no signals to compare against).
    let result = client
        .call_tool(
            "validate_change",
            json!({
                "path": "trivial.py",
                "new_content": "x = 1\n"
            }),
        )
        .expect("call validate_change");
    assert!(!result.is_error, "validate_change on clean file must not error");
    let parsed: Value = serde_json::from_str(&result.text)
        .expect("aegis-mcp returns JSON-encoded verdict in text content");
    assert_eq!(parsed["decision"], "PASS");
}

#[test]
fn validate_change_blocks_on_syntax_error() {
    let Some(binary) = find_aegis_mcp_binary() else {
        eprintln!("aegis-mcp binary not found — skipping");
        return;
    };

    let transport = StdioTransport::spawn(binary.to_str().unwrap(), &[]).unwrap();
    let mut client = McpClient::new(Box::new(transport)).unwrap();

    let result = client
        .call_tool(
            "validate_change",
            json!({
                "path": "broken.py",
                "new_content": "def f(\n"  // Unbalanced paren — Ring 0 violation
            }),
        )
        .unwrap();

    let parsed: Value = serde_json::from_str(&result.text).unwrap();
    assert_eq!(parsed["decision"], "BLOCK");
    let reasons = parsed["reasons"].as_array().expect("reasons array");
    assert!(
        reasons.iter().any(|r| r["layer"] == "ring0"),
        "expected ring0 violation in reasons: {reasons:?}"
    );
}

#[test]
fn mcp_executor_wraps_aegis_mcp_for_runtime() {
    let Some(binary) = find_aegis_mcp_binary() else {
        eprintln!("aegis-mcp binary not found — skipping");
        return;
    };

    let transport = StdioTransport::spawn(binary.to_str().unwrap(), &[]).unwrap();
    let client = McpClient::new(Box::new(transport)).unwrap();
    let mut executor = McpToolExecutor::new(client).unwrap();

    // Verify the executor sees aegis-mcp's tool surface.
    let names = executor.tool_names();
    assert!(names.contains(&"validate_change"));

    // Drive an end-to-end call through the ToolExecutor trait —
    // the same interface the conversation runtime uses.
    let arguments = json!({
        "path": "ok.py",
        "new_content": "y = 2\n"
    })
    .to_string();
    let output = executor.execute("validate_change", &arguments).unwrap();
    let parsed: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(parsed["decision"], "PASS");
}
