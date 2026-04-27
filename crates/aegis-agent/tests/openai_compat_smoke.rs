//! V3.1b — env-gated live API smoke test for OpenAI-compat backends.
//!
//! Marked `#[ignore]` so it never runs in CI by default. To exercise
//! a real backend (OpenRouter / Groq / Ollama / etc.):
//!
//! ```bash
//! export AEGIS_OPENAI_BASE_URL=https://openrouter.ai/api/v1
//! export AEGIS_OPENAI_API_KEY=sk-or-v1-...
//! export AEGIS_OPENAI_MODEL=meta-llama/llama-3.3-70b-instruct
//! cargo test -p aegis-agent --test openai_compat_smoke -- --ignored --nocapture
//! ```
//!
//! For local Ollama:
//! ```bash
//! export AEGIS_OPENAI_BASE_URL=http://127.0.0.1:11434/v1
//! export AEGIS_OPENAI_MODEL=llama3.2
//! # AEGIS_OPENAI_API_KEY left unset
//! ```
//!
//! What the test does: one short prompt, expect a text response,
//! one HTTP round trip, no tool calls. This is the floor — proves
//! the wire format works against the actual backend.

use aegis_agent::providers::{OpenAiCompatConfig, OpenAiCompatProvider, UreqClient};
use aegis_agent::testing::ScriptedToolExecutor;
use aegis_agent::{AgentConfig, ConversationRuntime, Session, StoppedReason};

#[test]
#[ignore = "live API — opt in with AEGIS_OPENAI_* env vars"]
fn live_openai_compat_returns_text_response() {
    let config = OpenAiCompatConfig::from_env()
        .expect("set AEGIS_OPENAI_BASE_URL + AEGIS_OPENAI_MODEL (and optionally AEGIS_OPENAI_API_KEY)");

    eprintln!("--- live smoke test ---");
    eprintln!("base_url: {}", config.base_url);
    eprintln!("model:    {}", config.model);
    eprintln!("api_key:  {}", if config.api_key.is_some() { "<set>" } else { "<unset>" });

    let provider = OpenAiCompatProvider::new(config, Box::new(UreqClient::new()));
    let tools = ScriptedToolExecutor::new();

    let mut rt = ConversationRuntime::new(
        Session::new(),
        provider,
        tools,
        vec![
            "You are a test assistant. Reply with a single short sentence."
                .into(),
        ],
        vec![],
        AgentConfig {
            max_iterations_per_turn: 1,
            session_cost_budget: None,
        },
    );

    let result = rt.run_turn("Reply with the literal word 'ready'.");

    eprintln!("stopped_reason: {:?}", result.stopped_reason);
    eprintln!("iterations:     {}", result.iterations);

    match result.stopped_reason {
        StoppedReason::PlanDoneNoVerifier => {
            // Print the assistant's last text block for human inspection.
            let last = rt
                .session()
                .messages
                .last()
                .expect("expected at least one message in session");
            eprintln!("assistant last message blocks: {:#?}", last.blocks);
        }
        StoppedReason::ProviderError(message) => {
            panic!("live smoke failed with provider error: {message}");
        }
        other => panic!("unexpected stopped reason: {other:?}"),
    }
}
