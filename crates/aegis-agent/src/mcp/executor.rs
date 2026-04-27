//! Bridge — wrap an `McpClient` as a `ToolExecutor` so MCP tools
//! plug straight into the conversation runtime.
//!
//! The executor caches the tool-list at construction time so the
//! `tool_definitions()` accessor (used to populate `ApiRequest.tools`)
//! doesn't re-issue `tools/list` on every turn. If the server's tool
//! list changes mid-session, callers can call `refresh()` to re-pull
//! — V3.2b doesn't subscribe to `notifications/tools/list_changed`
//! (deferred until we have a real consumer for it).

use std::collections::BTreeMap;

use serde_json::Value;

use crate::api::ToolDefinition;
use crate::tool::{ToolError, ToolExecutor};

use super::client::{McpClient, McpError, McpTool};

pub struct McpToolExecutor {
    client: McpClient,
    tools: BTreeMap<String, McpTool>,
}

impl McpToolExecutor {
    /// Construct from an already-handshaken `McpClient`. Pulls the
    /// tool list once and caches it.
    pub fn new(mut client: McpClient) -> Result<Self, McpError> {
        let tool_list = client.list_tools()?;
        let tools = tool_list
            .into_iter()
            .map(|tool| (tool.name.clone(), tool))
            .collect();
        Ok(Self { client, tools })
    }

    /// Convert cached MCP tools into the agent-facing `ToolDefinition`
    /// shape so the conversation runtime can advertise them to the LLM.
    pub fn tool_definitions(&self) -> Vec<ToolDefinition> {
        self.tools
            .values()
            .map(|tool| ToolDefinition {
                name: tool.name.clone(),
                description: tool.description.clone(),
                input_schema: tool.input_schema.clone(),
            })
            .collect()
    }

    /// Re-pull the server's tool list. Call this if a server signals
    /// its surface changed (V3.2b doesn't auto-subscribe).
    pub fn refresh(&mut self) -> Result<(), McpError> {
        let tool_list = self.client.list_tools()?;
        self.tools.clear();
        for tool in tool_list {
            self.tools.insert(tool.name.clone(), tool);
        }
        Ok(())
    }

    /// Direct access to the underlying client — for advanced use
    /// cases (custom tool calls outside the conversation loop).
    pub fn client_mut(&mut self) -> &mut McpClient {
        &mut self.client
    }

    /// Tool names served by this executor.
    pub fn tool_names(&self) -> Vec<&str> {
        self.tools.keys().map(String::as_str).collect()
    }
}

impl ToolExecutor for McpToolExecutor {
    fn execute(&mut self, tool_name: &str, input: &str) -> Result<String, ToolError> {
        if !self.tools.contains_key(tool_name) {
            return Err(ToolError::new(format!(
                "MCP tool not registered: {tool_name}"
            )));
        }

        // Parse the LLM-emitted argument string. If the LLM produced
        // empty input, default to an empty object — matching what
        // most MCP servers expect for parameter-less tools.
        let arguments: Value = if input.trim().is_empty() {
            Value::Object(serde_json::Map::new())
        } else {
            serde_json::from_str(input).map_err(|e| {
                ToolError::new(format!(
                    "MCP tool {tool_name}: invalid JSON arguments: {e}"
                ))
            })?
        };

        match self.client.call_tool(tool_name, arguments) {
            Ok(result) => {
                if result.is_error {
                    // Server-reported tool failure. Surface as ToolError
                    // so the conversation runtime emits a tool_result
                    // block with `is_error: true` — the LLM sees the
                    // failure text and decides what to do (its agency,
                    // not the runtime's).
                    return Err(ToolError::new(if result.text.is_empty() {
                        format!("MCP tool {tool_name} reported error with empty body")
                    } else {
                        result.text
                    }));
                }
                Ok(result.text)
            }
            Err(error) => {
                // Protocol-level failure (transport died, malformed
                // response, etc.). Surface as ToolError. The runtime
                // does NOT auto-restart the MCP server — the user
                // must start a new session.
                Err(ToolError::new(format!(
                    "MCP tool {tool_name} failed at protocol level: {error}"
                )))
            }
        }
    }
}
