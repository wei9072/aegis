//! `LLMProvider` trait + first Rust impl (OpenAI-compatible chat).
//!
//! Surface mirrors `aegis.agents.llm_adapter.LLMProvider` Protocol:
//! one method, `generate(prompt) -> Result<String, _>`.
//!
//! V1.1 scope (per `docs/v1_rust_port_plan.md`): only the OpenAI-
//! compatible surface ships. That covers OpenAI, OpenRouter, and
//! Groq via configurable `base_url` + `api_key_env`. Gemini's
//! native REST API is deferred until a real consumer (the V1.3
//! Rust pipeline) needs it.
//!
//! HTTP is abstracted behind `HttpClient` so tests can run without
//! a network. The default `UreqClient` is the only production
//! implementation.

pub mod error;
pub mod http;
pub mod openai;

pub use error::ProviderError;
pub use http::{HttpClient, HttpRequest, HttpResponse, UreqClient};
pub use openai::{OpenAIChatProvider, OpenAIChatProviderConfig};

/// Single-method provider contract. Implementors are
/// `Send + Sync` so they can be shared across threads (the V1.3
/// Rust pipeline runs the loop on a worker thread; sweeps run
/// many providers in parallel).
pub trait LLMProvider: Send + Sync {
    fn generate(&self, prompt: &str) -> Result<String, ProviderError>;

    fn name(&self) -> &str;

    /// Names of the tools recorded for the most recent generate
    /// call. Mirrors V0.x `last_used_tools` so the gateway's
    /// `provider:tool_surface` trace event still has data.
    /// Default returns empty — providers without a tool surface
    /// (the OpenAI-compatible one in V1.1) don't need to override.
    fn last_used_tools(&self) -> Vec<String> {
        Vec::new()
    }
}
