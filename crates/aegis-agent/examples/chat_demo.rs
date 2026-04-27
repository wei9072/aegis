//! V3.7 — minimal end-to-end chat demo.
//!
//! Run:
//!
//! ```bash
//! # OpenAI-compat backend (OpenRouter / Groq / Ollama / etc.)
//! export AEGIS_OPENAI_BASE_URL=https://api.openrouter.ai/api/v1
//! export AEGIS_OPENAI_API_KEY=sk-or-v1-...
//! export AEGIS_OPENAI_MODEL=meta-llama/llama-3.3-70b-instruct
//! cargo run -p aegis-agent --example chat_demo -- "What is 2+2?"
//!
//! # Anthropic
//! export AEGIS_ANTHROPIC_API_KEY=sk-ant-...
//! export AEGIS_ANTHROPIC_MODEL=claude-haiku-4-5
//! cargo run -p aegis-agent --example chat_demo -- "What is 2+2?"
//!
//! # Gemini
//! export AEGIS_GEMINI_API_KEY=AIza...
//! export AEGIS_GEMINI_MODEL=gemini-2.5-flash
//! cargo run -p aegis-agent --example chat_demo -- "What is 2+2?"
//! ```
//!
//! Picks whichever provider has its env vars set. Reports the
//! AgentTurnResult including stopped_reason — proving the V3
//! framing surfaces (PlanDoneNoVerifier, ProviderError, etc.) end
//! to end with a real LLM.

use aegis_agent::api::ApiClient;
use aegis_agent::providers::{
    AnthropicConfig, AnthropicProvider, GeminiConfig, GeminiProvider, OpenAiCompatConfig,
    OpenAiCompatProvider, UreqClient,
};
use aegis_agent::testing::ScriptedToolExecutor;
use aegis_agent::{AgentConfig, ConversationRuntime, Session, StoppedReason};

fn main() {
    let prompt: String = std::env::args()
        .skip(1)
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string();
    if prompt.is_empty() {
        eprintln!("usage: chat_demo <prompt>");
        std::process::exit(2);
    }

    eprintln!("aegis-agent V3.7 demo");
    eprintln!("---------------------");

    let provider: Box<dyn ApiClient> = if let Some(c) = OpenAiCompatConfig::from_env() {
        eprintln!("provider: openai-compat ({} {})", c.base_url, c.model);
        Box::new(OpenAiCompatProvider::new(c, Box::new(UreqClient::new())))
    } else if let Some(c) = AnthropicConfig::from_env() {
        eprintln!("provider: anthropic ({})", c.model);
        Box::new(AnthropicProvider::new(c, Box::new(UreqClient::new())))
    } else if let Some(c) = GeminiConfig::from_env() {
        eprintln!("provider: gemini ({})", c.model);
        Box::new(GeminiProvider::new(c, Box::new(UreqClient::new())))
    } else {
        eprintln!(
            "no provider env vars set. Set AEGIS_OPENAI_*, AEGIS_ANTHROPIC_*, \
             or AEGIS_GEMINI_* to pick one."
        );
        std::process::exit(2);
    };

    // ConversationRuntime is generic over (C: ApiClient, T: ToolExecutor).
    // We can't easily use Box<dyn ApiClient> directly — wrap in a
    // small adapter type.
    struct DynApi(Box<dyn ApiClient>);
    impl ApiClient for DynApi {
        fn stream(
            &mut self,
            request: aegis_agent::api::ApiRequest,
        ) -> Result<Vec<aegis_agent::api::AssistantEvent>, aegis_agent::api::RuntimeError> {
            self.0.stream(request)
        }
    }

    let mut rt = ConversationRuntime::new(
        Session::new(),
        DynApi(provider),
        ScriptedToolExecutor::new(), // no tools for the demo
        vec!["You are a concise assistant.".into()],
        vec![],
        AgentConfig {
            max_iterations_per_turn: 1,
            session_cost_budget: None,
            workspace_root: None,
        },
    );

    let result = rt.run_turn(prompt);

    eprintln!();
    eprintln!("--- result ---");
    eprintln!("stopped_reason: {:?}", result.stopped_reason);
    eprintln!("iterations:     {}", result.iterations);

    // Print the assistant's response so the human sees something.
    if let StoppedReason::PlanDoneNoVerifier = result.stopped_reason {
        if let Some(last) = rt.session().messages.last() {
            for block in &last.blocks {
                if let aegis_agent::ContentBlock::Text { text } = block {
                    println!("{text}");
                }
            }
        }
    } else if let StoppedReason::ProviderError(message) = result.stopped_reason {
        eprintln!("provider error: {message}");
        std::process::exit(1);
    }
}
