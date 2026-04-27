//! `AegisCostObserver` — feed CostTracker using aegis-core signals.
//!
//! After a successful Edit/Write tool call, re-extract Ring 0.5
//! signals from the touched file and report `(path, total_cost)` so
//! the conversation runtime's `CostTracker` can update the running
//! cumulative-regression total.
//!
//! Pure observation — emits values, never modifies them. The runtime
//! decides whether the cumulative budget is exceeded.

use std::path::PathBuf;

use serde_json::Value;

use crate::conversation::CostObserver;

/// Cost observer that uses `aegis_core::signal_layer_pyapi::extract_signals_native`
/// to attribute per-file structural cost after each Edit/Write call.
pub struct AegisCostObserver {
    workspace: PathBuf,
    /// Tool names this observer attributes cost to. Calls to other
    /// tools (Read / Glob / Grep / etc.) do not produce observations.
    file_write_tools: std::collections::BTreeSet<String>,
}

impl AegisCostObserver {
    #[must_use]
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        Self {
            workspace: workspace.into(),
            file_write_tools: [
                "write_file",
                "edit_file",
                "Edit",
                "Write",
                "MultiEdit",
            ]
            .iter()
            .map(|s| (*s).to_string())
            .collect(),
        }
    }

    pub fn watch_tool(mut self, name: impl Into<String>) -> Self {
        self.file_write_tools.insert(name.into());
        self
    }
}

impl CostObserver for AegisCostObserver {
    fn observe(&mut self, tool_name: &str, input: &str) -> Vec<(PathBuf, f64)> {
        if !self.file_write_tools.contains(tool_name) {
            return Vec::new();
        }
        let args: Value = match serde_json::from_str(input) {
            Ok(v) => v,
            Err(_) => return Vec::new(),
        };
        let path_str = match args.get("path").and_then(|v| v.as_str()) {
            Some(p) => p.to_string(),
            None => return Vec::new(),
        };
        let path = if std::path::Path::new(&path_str).is_absolute() {
            PathBuf::from(&path_str)
        } else {
            self.workspace.join(&path_str)
        };
        let signals = match aegis_core::signal_layer_pyapi::extract_signals_native(
            path.to_string_lossy().as_ref(),
        ) {
            Ok(v) => v,
            Err(_) => return Vec::new(),
        };
        let cost: f64 = signals.iter().map(|s| s.value).sum();
        vec![(path, cost)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unwatched_tool_yields_no_observations() {
        let mut obs = AegisCostObserver::new("/tmp");
        let v = obs.observe("Read", r#"{"path":"x.py"}"#);
        assert!(v.is_empty());
    }

    #[test]
    fn malformed_input_yields_no_observations() {
        let mut obs = AegisCostObserver::new("/tmp");
        let v = obs.observe("Edit", "not json");
        assert!(v.is_empty());
    }

    #[test]
    fn missing_path_field_yields_no_observations() {
        let mut obs = AegisCostObserver::new("/tmp");
        let v = obs.observe("Edit", r#"{"content":"x"}"#);
        assert!(v.is_empty());
    }

    #[test]
    fn observes_real_python_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("foo.py");
        std::fs::write(&path, "import os\nimport sys\n").unwrap();

        let mut obs = AegisCostObserver::new(dir.path());
        let v = obs.observe("Edit", r#"{"path":"foo.py","new_string":"x","old_string":"y"}"#);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].0, path);
        // Cost should be > 0 since fan_out picks up the imports.
        assert!(v[0].1 >= 2.0);
    }
}
