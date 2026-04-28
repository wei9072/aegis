//! Model registry — alias resolution, per-model metadata, and the
//! pre-dispatch token-count check.
//!
//! Negative-space framing reminder: claw-code's preflight quietly
//! truncates the message history when the request would overflow the
//! context window. Aegis does NOT. An oversized request is a
//! degradation we *reject* — the caller (REPL, CLI, test harness)
//! decides what to do (`/compact`, switch to a larger-window model,
//! abort). Aegis never silently throws away conversation history.

use crate::api::ApiRequest;
use crate::message::ContentBlock;
use serde::{Deserialize, Serialize};

/// Wire-format family. Each kind maps to one `ApiClient` impl in
/// this crate (`anthropic` / `gemini` / `openai_compat`). Unknown
/// models default to `OpenAiCompat` since OpenAI Chat Completions
/// is the de-facto interop wire format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    Anthropic,
    Gemini,
    OpenAiCompat,
}

/// Per-model facts the agent needs at runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelMetadata {
    /// Canonical ID — what the wire format expects in the `model`
    /// field of the outgoing request. Aliases resolve to this.
    pub id: &'static str,
    pub provider_kind: ProviderKind,
    /// Context window in tokens (input + output). Preflight rejects
    /// requests whose estimated token count exceeds this.
    pub ctx_window: u32,
    /// Max output tokens the provider will return per turn.
    pub max_output: u32,
    /// Whether the model supports inline `cache_control` annotations
    /// (Anthropic ephemeral cache). Wired in B5.4 — unused today.
    pub supports_cache_control: bool,
}

/// Curated set of common models. Add new ones as users hit them —
/// no attempt to mirror every provider's full catalogue (that's a
/// maintenance treadmill). Unknown models still work via
/// `default_metadata()`.
const MODELS: &[ModelMetadata] = &[
    // Anthropic — direct
    ModelMetadata {
        id: "claude-opus-4-7",
        provider_kind: ProviderKind::Anthropic,
        ctx_window: 200_000,
        max_output: 64_000,
        supports_cache_control: true,
    },
    ModelMetadata {
        id: "claude-sonnet-4-6",
        provider_kind: ProviderKind::Anthropic,
        ctx_window: 1_000_000,
        max_output: 64_000,
        supports_cache_control: true,
    },
    ModelMetadata {
        id: "claude-haiku-4-5",
        provider_kind: ProviderKind::Anthropic,
        ctx_window: 200_000,
        max_output: 8_192,
        supports_cache_control: true,
    },
    // OpenAI — direct
    ModelMetadata {
        id: "gpt-4o",
        provider_kind: ProviderKind::OpenAiCompat,
        ctx_window: 128_000,
        max_output: 16_384,
        supports_cache_control: false,
    },
    ModelMetadata {
        id: "gpt-4o-mini",
        provider_kind: ProviderKind::OpenAiCompat,
        ctx_window: 128_000,
        max_output: 16_384,
        supports_cache_control: false,
    },
    // Google — direct
    ModelMetadata {
        id: "gemini-2.5-flash",
        provider_kind: ProviderKind::Gemini,
        ctx_window: 1_000_000,
        max_output: 8_192,
        supports_cache_control: false,
    },
    ModelMetadata {
        id: "gemini-2.5-pro",
        provider_kind: ProviderKind::Gemini,
        ctx_window: 2_000_000,
        max_output: 8_192,
        supports_cache_control: false,
    },
    // OpenRouter routing — vendor-prefixed forms. Sit on the
    // OpenAiCompat wire even when the underlying model is Anthropic /
    // Google because OpenRouter exposes everything via Chat Completions.
    ModelMetadata {
        id: "anthropic/claude-haiku-4.5",
        provider_kind: ProviderKind::OpenAiCompat,
        ctx_window: 200_000,
        max_output: 8_192,
        supports_cache_control: false,
    },
    ModelMetadata {
        id: "anthropic/claude-sonnet-4.6",
        provider_kind: ProviderKind::OpenAiCompat,
        ctx_window: 1_000_000,
        max_output: 64_000,
        supports_cache_control: false,
    },
];

/// User-facing short names → canonical IDs. Lookup is
/// case-insensitive. Unknown names pass through unchanged so users
/// can still type arbitrary OpenRouter / vLLM model strings.
const ALIASES: &[(&str, &str)] = &[
    ("opus", "claude-opus-4-7"),
    ("sonnet", "claude-sonnet-4-6"),
    ("haiku", "claude-haiku-4-5"),
    ("flash", "gemini-2.5-flash"),
    ("pro", "gemini-2.5-pro"),
    ("4o", "gpt-4o"),
    ("4o-mini", "gpt-4o-mini"),
    ("mini", "gpt-4o-mini"),
];

/// Resolve an alias to the canonical model id. Returns `name`
/// unchanged when no alias matches.
#[must_use]
pub fn resolve_alias(name: &str) -> &str {
    ALIASES
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| *v)
        .unwrap_or(name)
}

/// Look up known metadata by id (alias-resolved first). `None` for
/// unknown models — `preflight` then uses `default_metadata()` so
/// unknown models still get a soft check rather than going through
/// blindly.
#[must_use]
pub fn metadata_for(id: &str) -> Option<&'static ModelMetadata> {
    let canonical = resolve_alias(id);
    MODELS.iter().find(|m| m.id == canonical)
}

/// Conservative defaults for unknown models. 32k window is what most
/// open-source models ship with today; supports_cache_control = false
/// avoids accidentally injecting Anthropic-only headers into a
/// generic OpenAI-compat backend.
const fn default_metadata(id: &'static str) -> ModelMetadata {
    ModelMetadata {
        id,
        provider_kind: ProviderKind::OpenAiCompat,
        ctx_window: 32_000,
        max_output: 4_096,
        supports_cache_control: false,
    }
}

/// Reason `preflight()` blocked the request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreflightError {
    /// Estimated request size exceeds the model's context window.
    /// Carries the numbers so the caller can render a useful error.
    OverContextWindow {
        model: String,
        estimated_tokens: u32,
        ctx_window: u32,
    },
}

impl std::fmt::Display for PreflightError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OverContextWindow {
                model,
                estimated_tokens,
                ctx_window,
            } => write!(
                f,
                "request for {model} estimates ~{estimated_tokens} tokens, \
                 model context window is {ctx_window}. Aegis does not \
                 silently truncate — use /compact, drop snippets, or \
                 switch to a larger-window model."
            ),
        }
    }
}

impl std::error::Error for PreflightError {}

/// Heuristic token count for an `ApiRequest`. ~3.5 chars/token is
/// the rough average across English prose + code; biased slightly
/// high (over-estimates) so a borderline request prefers PASS-through
/// to silent overflow. Upgrade to a real tokenizer (tiktoken /
/// HuggingFace) only when a real consumer hits a false reject.
#[must_use]
pub fn estimate_tokens(request: &ApiRequest) -> u32 {
    let mut chars: usize = 0;
    for sys in &request.system_prompt {
        chars += sys.len();
    }
    for msg in &request.messages {
        for block in &msg.blocks {
            chars += content_block_chars(block);
        }
    }
    for tool in &request.tools {
        // ToolDefinition is name + description + JSON schema.
        chars += tool.name.len() + tool.description.len() + tool.input_schema.to_string().len();
    }
    // Round up — better to over-estimate than miss a true overflow.
    ((chars as f64) / 3.5).ceil() as u32
}

fn content_block_chars(block: &ContentBlock) -> usize {
    match block {
        ContentBlock::Text { text } => text.len(),
        ContentBlock::ToolUse { name, input, .. } => name.len() + input.len(),
        ContentBlock::ToolResult { output, .. } => output.len(),
    }
}

/// Run the pre-dispatch check. PASS = request fits the model's
/// context window. BLOCK (`Err(PreflightError::OverContextWindow)`) =
/// caller must reduce the request before sending; aegis will not
/// truncate.
pub fn preflight(model: &str, request: &ApiRequest) -> Result<(), PreflightError> {
    let canonical = resolve_alias(model);
    let owned;
    let meta = match metadata_for(canonical) {
        Some(m) => m,
        None => {
            // Synthesize a conservative metadata so unknown models
            // still get checked rather than going through blind.
            owned = default_metadata("__unknown__");
            &owned
        }
    };
    let estimated = estimate_tokens(request);
    if estimated > meta.ctx_window {
        return Err(PreflightError::OverContextWindow {
            model: canonical.to_string(),
            estimated_tokens: estimated,
            ctx_window: meta.ctx_window,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::ToolDefinition;
    use crate::message::ConversationMessage;
    use serde_json::json;

    #[test]
    fn alias_resolves_canonical_form() {
        assert_eq!(resolve_alias("sonnet"), "claude-sonnet-4-6");
        assert_eq!(resolve_alias("opus"), "claude-opus-4-7");
        assert_eq!(resolve_alias("haiku"), "claude-haiku-4-5");
        assert_eq!(resolve_alias("4o-mini"), "gpt-4o-mini");
        assert_eq!(resolve_alias("flash"), "gemini-2.5-flash");
    }

    #[test]
    fn alias_lookup_is_case_insensitive() {
        assert_eq!(resolve_alias("SONNET"), "claude-sonnet-4-6");
        assert_eq!(resolve_alias("Sonnet"), "claude-sonnet-4-6");
    }

    #[test]
    fn unknown_alias_passes_through() {
        assert_eq!(
            resolve_alias("anthropic/claude-haiku-4.5"),
            "anthropic/claude-haiku-4.5"
        );
        assert_eq!(resolve_alias("llama-3.3-70b-versatile"), "llama-3.3-70b-versatile");
    }

    #[test]
    fn metadata_lookup_through_alias() {
        let m = metadata_for("sonnet").expect("sonnet alias should resolve");
        assert_eq!(m.id, "claude-sonnet-4-6");
        assert_eq!(m.provider_kind, ProviderKind::Anthropic);
        assert!(m.supports_cache_control);
    }

    #[test]
    fn metadata_returns_none_for_unknown_model() {
        assert!(metadata_for("totally-made-up-model").is_none());
    }

    #[test]
    fn estimate_grows_with_text() {
        let small = ApiRequest {
            system_prompt: vec![],
            messages: vec![],
            tools: vec![],
        };
        let large = ApiRequest {
            system_prompt: vec!["a".repeat(7000)],
            messages: vec![],
            tools: vec![],
        };
        assert!(estimate_tokens(&large) > estimate_tokens(&small));
        // 7000 chars / 3.5 = 2000 tokens
        assert_eq!(estimate_tokens(&large), 2000);
    }

    #[test]
    fn preflight_passes_for_small_request() {
        let req = ApiRequest {
            system_prompt: vec!["short".to_string()],
            messages: vec![ConversationMessage::user_text("hi")],
            tools: vec![],
        };
        assert!(preflight("sonnet", &req).is_ok());
    }

    #[test]
    fn preflight_blocks_oversized_request() {
        // Build a request that exceeds Haiku's 200k window. ~800k
        // chars / 3.5 ≈ 228k tokens.
        let huge_text = "x".repeat(800_000);
        let req = ApiRequest {
            system_prompt: vec![huge_text],
            messages: vec![],
            tools: vec![],
        };
        let err = preflight("haiku", &req).unwrap_err();
        match err {
            PreflightError::OverContextWindow {
                model,
                estimated_tokens,
                ctx_window,
            } => {
                assert_eq!(model, "claude-haiku-4-5");
                assert!(estimated_tokens > ctx_window);
                assert_eq!(ctx_window, 200_000);
            }
        }
    }

    #[test]
    fn preflight_uses_conservative_default_for_unknown_model() {
        // Unknown model → 32k default window. 200k chars / 3.5 ≈ 57k
        // tokens, exceeds 32k.
        let req = ApiRequest {
            system_prompt: vec!["x".repeat(200_000)],
            messages: vec![],
            tools: vec![],
        };
        let err = preflight("unknown-llm-3000", &req).unwrap_err();
        match err {
            PreflightError::OverContextWindow { ctx_window, .. } => {
                assert_eq!(ctx_window, 32_000);
            }
        }
    }

    #[test]
    fn estimate_includes_tools() {
        let req = ApiRequest {
            system_prompt: vec![],
            messages: vec![],
            tools: vec![ToolDefinition {
                name: "Edit".to_string(),
                description: "Edit a file".to_string(),
                input_schema: json!({"type": "object"}),
            }],
        };
        assert!(estimate_tokens(&req) > 0);
    }

    #[test]
    fn preflight_error_message_mentions_remediation() {
        let err = PreflightError::OverContextWindow {
            model: "haiku".to_string(),
            estimated_tokens: 250_000,
            ctx_window: 200_000,
        };
        let msg = err.to_string();
        assert!(msg.contains("250000"));
        assert!(msg.contains("200000"));
        assert!(msg.contains("/compact") || msg.contains("compact"));
    }
}
