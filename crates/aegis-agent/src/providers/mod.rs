//! LLM provider implementations.
//!
//! Each provider is a concrete `ApiClient` impl. The trait itself is
//! the abstraction — provider-specific wire formats are translated
//! inside each impl, so the conversation loop never sees them.
//!
//! V3.1b ships **OpenAI Chat Completions** as the first provider,
//! which covers the broad open-source backend ecosystem via
//! configurable `base_url` (OpenRouter / Groq / Ollama / vLLM /
//! llama.cpp server / LMStudio / DashScope / etc.).
//!
//! V3.2 will add Anthropic Messages and (likely) Gemini — both
//! formats different enough from OpenAI to warrant their own impl.
//!
//! Adding a new provider is two files: a new module under this one
//! that implements `ApiClient`, and a re-export here.

pub mod http;
pub mod openai_compat;

pub use http::{HttpClient, HttpError, HttpResponse, RecordedRequest, StubHttpClient, UreqClient};
pub use openai_compat::{OpenAiCompatConfig, OpenAiCompatProvider};
