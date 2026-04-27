//! PreToolUse aegis-verdict prediction — V3.3 differentiation #1.
//!
//! Before executing each tool call, the conversation runtime asks the
//! injected `PreToolUsePredictor` for a verdict. If the predictor
//! says BLOCK, the tool call is skipped and the LLM gets a tool_result
//! with `is_error: true` carrying the block reason.
//!
//! The predictor never sees the LLM's prompt and never returns a
//! "fix suggestion". Its job is **veto, not coaching**. If it says
//! Block, the LLM sees the reason as raw observation; the LLM (its
//! agency, not aegis) decides what to try next.
//!
//! The default impl is `NullPredictor` — every call passes. The
//! aegis-specific impl `AegisPredictor` (in `aegis_predict`) calls
//! out to an MCP server (typically aegis-mcp itself) for each
//! file-write tool call.

/// Predictor's verdict on a single tool invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PredictVerdict {
    Allow,
    Block { reason: String },
}

/// PreToolUse predictor contract.
pub trait PreToolUsePredictor: Send {
    /// Inspect a tool call and return a verdict. The predictor MUST
    /// NOT execute the tool — it only judges whether the runtime
    /// should proceed.
    fn predict(&mut self, tool_name: &str, input: &str) -> PredictVerdict;
}

/// No-op predictor — every call passes. Default for the runtime
/// when nothing else is wired in.
pub struct NullPredictor;

impl PreToolUsePredictor for NullPredictor {
    fn predict(&mut self, _tool_name: &str, _input: &str) -> PredictVerdict {
        PredictVerdict::Allow
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_predictor_allows_everything() {
        let mut p = NullPredictor;
        assert_eq!(p.predict("anything", ""), PredictVerdict::Allow);
        assert_eq!(p.predict("write_file", "{}"), PredictVerdict::Allow);
    }
}
