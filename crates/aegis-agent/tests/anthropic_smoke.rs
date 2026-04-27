//! V3.2a — env-gated live API smoke for Anthropic Messages.
//!
//! `#[ignore]` by default. Opt in with:
//!
//! ```bash
//! export AEGIS_ANTHROPIC_API_KEY=sk-ant-...
//! export AEGIS_ANTHROPIC_MODEL=claude-haiku-4-5
//! cargo test -p aegis-agent --test anthropic_smoke -- --ignored --nocapture
//! ```
//!
//! Falls back to `ANTHROPIC_API_KEY` if `AEGIS_ANTHROPIC_API_KEY`
//! is not set.

use aegis_agent::providers::{AnthropicConfig, AnthropicProvider, UreqClient};
use aegis_agent::testing::ScriptedToolExecutor;
use aegis_agent::{AgentConfig, ConversationRuntime, Session, StoppedReason};

#[test]
#[ignore = "live API — opt in with AEGIS_ANTHROPIC_* env vars"]
fn live_anthropic_returns_text_response() {
    let config = AnthropicConfig::from_env().expect(
        "set AEGIS_ANTHROPIC_API_KEY (or ANTHROPIC_API_KEY) + AEGIS_ANTHROPIC_MODEL",
    );

    eprintln!("--- live Anthropic smoke test ---");
    eprintln!("base_url: {}", config.base_url);
    eprintln!("model:    {}", config.model);

    let provider = AnthropicProvider::new(config, Box::new(UreqClient::new()));
    let tools = ScriptedToolExecutor::new();

    let mut rt = ConversationRuntime::new(
        Session::new(),
        provider,
        tools,
        vec!["You are a test assistant. Reply with one short sentence.".into()],
        vec![],
        AgentConfig {
            max_iterations_per_turn: 1,
            session_cost_budget: None,
            workspace_root: None,
        },
    );

    let result = rt.run_turn("Reply with the literal word 'ready'.");

    eprintln!("stopped_reason: {:?}", result.stopped_reason);
    eprintln!("iterations:     {}", result.iterations);

    match result.stopped_reason {
        StoppedReason::PlanDoneNoVerifier => {
            let last = rt
                .session()
                .messages
                .last()
                .expect("expected at least one message");
            eprintln!("assistant last message blocks: {:#?}", last.blocks);
        }
        StoppedReason::ProviderError(message) => {
            panic!("live Anthropic smoke failed: {message}");
        }
        other => panic!("unexpected stopped reason: {other:?}"),
    }
}
