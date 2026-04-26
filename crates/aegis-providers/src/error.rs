use thiserror::Error;

/// All failure modes a `LLMProvider::generate` call can surface.
///
/// Mirrors the four `RuntimeError` shapes the Python `OpenAIProvider`
/// raises so the PyShim can map back to the same Python exception
/// types and trace metadata.
#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("missing credential: {var} not set; pass api_key explicitly or export the env var")]
    MissingCredential { var: String },

    #[error("tool '{name}' is a state-mutating callable and cannot be exposed to the LLM")]
    MutatingToolRejected { name: String },

    #[error("HTTP {code} from {url}: {body}")]
    HttpStatus { url: String, code: u16, body: String },

    #[error("request to {url} failed: {source}")]
    Network {
        url: String,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    #[error("unexpected response shape from {url}: {body}")]
    BadResponse { url: String, body: String },

    #[error("request to {url} exceeded total_timeout={timeout_secs}s")]
    Timeout { url: String, timeout_secs: u64 },
}
