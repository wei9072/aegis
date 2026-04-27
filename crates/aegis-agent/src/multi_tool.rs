//! Multi-source tool dispatcher.
//!
//! When an agent needs to combine more than one tool source (e.g.
//! built-in `ReadOnlyTools` plus an `McpToolExecutor` pointing at
//! aegis-mcp), wrap them in a `MultiToolExecutor`. Tool name → source
//! mapping is built at construction time from each source's
//! advertised tool definitions; calls dispatch by name.

use std::collections::BTreeMap;

use crate::api::ToolDefinition;
use crate::tool::{ToolError, ToolExecutor};

/// One named tool source. The source is a boxed executor + the
/// tool definitions it serves.
pub struct ToolSource {
    pub label: String,
    pub executor: Box<dyn ToolExecutor + Send>,
    pub definitions: Vec<ToolDefinition>,
}

impl ToolSource {
    #[must_use]
    pub fn new(
        label: impl Into<String>,
        executor: Box<dyn ToolExecutor + Send>,
        definitions: Vec<ToolDefinition>,
    ) -> Self {
        Self {
            label: label.into(),
            executor,
            definitions,
        }
    }
}

/// Dispatches tool calls across multiple sources by tool name.
/// First source to claim a name wins (so order matters when two
/// sources expose the same tool name).
pub struct MultiToolExecutor {
    sources: Vec<ToolSource>,
    routing: BTreeMap<String, usize>,
    all_definitions: Vec<ToolDefinition>,
}

impl MultiToolExecutor {
    /// Build from an ordered list of sources. Earlier sources take
    /// precedence on duplicate tool names.
    #[must_use]
    pub fn new(sources: Vec<ToolSource>) -> Self {
        let mut routing: BTreeMap<String, usize> = BTreeMap::new();
        let mut all_definitions = Vec::new();
        for (i, src) in sources.iter().enumerate() {
            for def in &src.definitions {
                routing.entry(def.name.clone()).or_insert(i);
            }
            all_definitions.extend(src.definitions.iter().cloned());
        }
        Self {
            sources,
            routing,
            all_definitions,
        }
    }

    /// All tool definitions advertised by all sources, in source
    /// order. Hand this to `ConversationRuntime::new`.
    #[must_use]
    pub fn all_definitions(&self) -> Vec<ToolDefinition> {
        self.all_definitions.clone()
    }

    /// Diagnostic — which source claims a given tool name?
    pub fn source_label_for(&self, tool_name: &str) -> Option<&str> {
        self.routing
            .get(tool_name)
            .and_then(|&idx| self.sources.get(idx).map(|s| s.label.as_str()))
    }
}

impl ToolExecutor for MultiToolExecutor {
    fn execute(&mut self, tool_name: &str, input: &str) -> Result<String, ToolError> {
        let idx = match self.routing.get(tool_name).copied() {
            Some(i) => i,
            None => {
                return Err(ToolError::new(format!(
                    "MultiToolExecutor: no source claims tool {tool_name:?} (have: {:?})",
                    self.routing.keys().collect::<Vec<_>>()
                )));
            }
        };
        self.sources[idx].executor.execute(tool_name, input)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::ScriptedToolExecutor;
    use serde_json::json;

    fn make_source(label: &str, name: &str, output: &str) -> ToolSource {
        ToolSource::new(
            label,
            Box::new(ScriptedToolExecutor::new().with_ok(name, output)),
            vec![ToolDefinition::new(name, "", json!({}))],
        )
    }

    #[test]
    fn dispatch_routes_to_correct_source() {
        let mut multi = MultiToolExecutor::new(vec![
            make_source("a", "tool_a", "from a"),
            make_source("b", "tool_b", "from b"),
        ]);
        assert_eq!(multi.execute("tool_a", "{}").unwrap(), "from a");
        assert_eq!(multi.execute("tool_b", "{}").unwrap(), "from b");
    }

    #[test]
    fn unknown_tool_yields_helpful_error() {
        let mut multi = MultiToolExecutor::new(vec![make_source("a", "x", "x")]);
        let err = multi.execute("nonexistent", "{}").unwrap_err();
        assert!(err.message().contains("no source claims tool"));
    }

    #[test]
    fn first_source_wins_on_duplicate_name() {
        let mut multi = MultiToolExecutor::new(vec![
            make_source("first", "shared", "from first"),
            make_source("second", "shared", "from second"),
        ]);
        assert_eq!(multi.execute("shared", "{}").unwrap(), "from first");
    }

    #[test]
    fn all_definitions_includes_every_source() {
        let multi = MultiToolExecutor::new(vec![
            make_source("a", "x", ""),
            make_source("b", "y", ""),
        ]);
        let defs = multi.all_definitions();
        let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, vec!["x", "y"]);
    }

    #[test]
    fn source_label_for_returns_owner() {
        let multi = MultiToolExecutor::new(vec![
            make_source("first", "x", ""),
            make_source("second", "y", ""),
        ]);
        assert_eq!(multi.source_label_for("x"), Some("first"));
        assert_eq!(multi.source_label_for("y"), Some("second"));
        assert_eq!(multi.source_label_for("nope"), None);
    }
}
