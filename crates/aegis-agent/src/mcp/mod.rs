//! MCP client — call out to external MCP servers (including
//! `aegis-mcp` itself for the V3.3 PreToolUse predict hook).
//!
//! Different concern from `crates/aegis-mcp/` (which is the MCP
//! **server** binary). This module is the **client** side: spawn an
//! MCP server as a subprocess, complete the JSON-RPC handshake,
//! discover its tools, and invoke them when the LLM asks.
//!
//! V3.2b scope:
//!   - One `McpClient` per server (multi-server dispatching arrives
//!     when V3.3+ needs it).
//!   - `JsonRpcTransport` trait abstracts the byte channel so tests
//!     can drive end-to-end without spawning subprocesses.
//!   - `StdioTransport` is the production impl (spawns + pipes).
//!   - `ScriptedTransport` is the test impl (replays canned bytes).
//!   - `McpToolExecutor` wraps an `McpClient` as a `ToolExecutor`
//!     so the conversation runtime sees MCP tools as normal tools.
//!
//! Negative-space discipline (mirrors the rest of the agent):
//!   - No auto-restart on subprocess death.
//!   - No retry on JSON-RPC errors.
//!   - Tool errors flow back to the LLM as `ToolError`; the runtime
//!     never coaches the LLM on why the call failed.

pub mod client;
pub mod executor;
pub mod protocol;
pub mod transport;

pub use client::{McpClient, McpError, McpTool, McpToolResult};
pub use executor::McpToolExecutor;
pub use transport::{JsonRpcTransport, ScriptedTransport, StdioTransport};
