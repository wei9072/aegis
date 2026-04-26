//! OpenAI-compatible chat-completions provider.
//!
//! Works against any service implementing `/v1/chat/completions`
//! (OpenAI, OpenRouter, Groq, Together, Anyscale, Ollama, vLLM,
//! local OpenAI-compat shims). Same surface as
//! `aegis.agents.openai.OpenAIProvider` — credential read from env
//! var (or explicit), configurable `base_url`, fixed JSON payload.
//!
//! V1.1 doesn't forward tools; the Python provider also doesn't.
//! When V1.3 ports the pipeline and finds a real tool-dispatch
//! consumer, the trait surface here grows.

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::ProviderError;
use crate::http::{HttpClient, HttpRequest, UreqClient};
use crate::LLMProvider;

const DEFAULT_OPENAI_BASE_URL: &str = "https://api.openai.com/v1";

/// Construction-time settings. Cheap to clone; held by the provider
/// and never mutated.
#[derive(Clone, Debug)]
pub struct OpenAIChatProviderConfig {
    pub model_name: String,
    pub api_key: String,
    pub base_url: String,
    pub timeout: Duration,
    /// Display name used by `LLMProvider::name`. Defaults to
    /// `"openai"`; callers configuring OpenRouter / Groq pass
    /// `"openrouter"` / `"groq"` so traces and sweep logs read
    /// naturally.
    pub display_name: String,
}

impl OpenAIChatProviderConfig {
    pub fn new(model_name: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            model_name: model_name.into(),
            api_key: api_key.into(),
            base_url: DEFAULT_OPENAI_BASE_URL.to_string(),
            timeout: Duration::from_secs(120),
            display_name: "openai".to_string(),
        }
    }

    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into().trim_end_matches('/').to_string();
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn with_display_name(mut self, name: impl Into<String>) -> Self {
        self.display_name = name.into();
        self
    }

    /// Read `api_key` from `env_var` if it isn't already set on the
    /// config. Lets callers do `.from_env("OPENAI_API_KEY")` once and
    /// not worry about the lookup.
    pub fn from_env(
        model_name: impl Into<String>,
        env_var: &str,
    ) -> Result<Self, ProviderError> {
        let api_key = std::env::var(env_var)
            .map_err(|_| ProviderError::MissingCredential { var: env_var.to_string() })?;
        Ok(Self::new(model_name, api_key))
    }
}

pub struct OpenAIChatProvider {
    config: OpenAIChatProviderConfig,
    http: Arc<dyn HttpClient>,
}

impl OpenAIChatProvider {
    /// Production constructor — uses the default `UreqClient`.
    pub fn new(config: OpenAIChatProviderConfig) -> Self {
        Self {
            config,
            http: Arc::new(UreqClient::new()),
        }
    }

    /// Test constructor — inject any `HttpClient`.
    pub fn with_http(config: OpenAIChatProviderConfig, http: Arc<dyn HttpClient>) -> Self {
        Self { config, http }
    }
}

#[derive(Serialize)]
struct ChatPayload<'a> {
    model: &'a str,
    messages: Vec<ChatMessage<'a>>,
}

#[derive(Serialize)]
struct ChatMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Deserialize)]
struct ChatChoice {
    message: ChatChoiceMessage,
}

#[derive(Deserialize)]
struct ChatChoiceMessage {
    content: Option<String>,
}

impl LLMProvider for OpenAIChatProvider {
    fn name(&self) -> &str {
        &self.config.display_name
    }

    fn generate(&self, prompt: &str) -> Result<String, ProviderError> {
        let url = format!("{}/chat/completions", self.config.base_url);
        let payload = ChatPayload {
            model: &self.config.model_name,
            messages: vec![ChatMessage { role: "user", content: prompt }],
        };
        let body =
            serde_json::to_vec(&payload).map_err(|e| ProviderError::BadResponse {
                url: url.clone(),
                body: format!("payload serialize failed: {e}"),
            })?;
        let req = HttpRequest {
            url: url.clone(),
            method: "POST".to_string(),
            // User-Agent matters: Groq sits behind Cloudflare and the
            // default `ureq` UA gets bounced by WAF rule 1010 before
            // reaching the API. Identifying as Aegis both gets through
            // and makes our traffic legible in their dashboards.
            headers: vec![
                ("Authorization".into(), format!("Bearer {}", self.config.api_key)),
                ("Content-Type".into(), "application/json".into()),
                ("User-Agent".into(), "aegis-control-plane/1.0".into()),
            ],
            body,
            timeout: self.config.timeout,
        };
        let resp = self.http.execute(req)?;
        if resp.status >= 400 {
            return Err(ProviderError::HttpStatus {
                url,
                code: resp.status,
                body: String::from_utf8_lossy(&resp.body)
                    .chars()
                    .take(300)
                    .collect(),
            });
        }
        let parsed: ChatResponse =
            serde_json::from_slice(&resp.body).map_err(|_| ProviderError::BadResponse {
                url: url.clone(),
                body: String::from_utf8_lossy(&resp.body)
                    .chars()
                    .take(300)
                    .collect(),
            })?;
        let content = parsed
            .choices
            .into_iter()
            .next()
            .and_then(|c| c.message.content)
            .ok_or_else(|| ProviderError::BadResponse {
                url: url.clone(),
                body: "missing choices[0].message.content".to_string(),
            })?;
        Ok(content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::{HttpResponse, StubHttpClient};

    fn ok_response(body: &str) -> Result<HttpResponse, ProviderError> {
        let payload = serde_json::json!({
            "choices": [{"message": {"content": body}}]
        });
        Ok(HttpResponse {
            status: 200,
            body: serde_json::to_vec(&payload).unwrap(),
        })
    }

    fn make_provider(stub: Arc<StubHttpClient>) -> OpenAIChatProvider {
        let cfg = OpenAIChatProviderConfig::new("gpt-4o-mini", "dummy");
        OpenAIChatProvider::with_http(cfg, stub)
    }

    #[test]
    fn returns_assistant_content() {
        let stub = Arc::new(StubHttpClient::new(vec![ok_response("hello")]));
        let p = make_provider(stub.clone());
        assert_eq!(p.generate("hi").unwrap(), "hello");

        let captured = stub.captured_requests();
        assert_eq!(captured.len(), 1);
        let req = &captured[0];
        assert_eq!(req.url, "https://api.openai.com/v1/chat/completions");
        assert!(req
            .headers
            .iter()
            .any(|(k, v)| k == "Authorization" && v == "Bearer dummy"));
        let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
        assert_eq!(body["model"], "gpt-4o-mini");
        assert_eq!(body["messages"][0]["role"], "user");
        assert_eq!(body["messages"][0]["content"], "hi");
    }

    #[test]
    fn translates_http_status_error() {
        let stub = Arc::new(StubHttpClient::new(vec![Ok(HttpResponse {
            status: 429,
            body: br#"{"error":"rate limited"}"#.to_vec(),
        })]));
        let p = make_provider(stub);
        let err = p.generate("hi").unwrap_err();
        match err {
            ProviderError::HttpStatus { code, .. } => assert_eq!(code, 429),
            other => panic!("expected HttpStatus, got {other:?}"),
        }
    }

    #[test]
    fn translates_network_error() {
        let stub = Arc::new(StubHttpClient::new(vec![Err(ProviderError::Network {
            url: "https://api.openai.com/v1/chat/completions".into(),
            source: "connection refused".into(),
        })]));
        let p = make_provider(stub);
        let err = p.generate("hi").unwrap_err();
        assert!(matches!(err, ProviderError::Network { .. }));
    }

    #[test]
    fn raises_on_malformed_json() {
        let stub = Arc::new(StubHttpClient::new(vec![Ok(HttpResponse {
            status: 200,
            body: b"<html>not json</html>".to_vec(),
        })]));
        let p = make_provider(stub);
        let err = p.generate("hi").unwrap_err();
        assert!(matches!(err, ProviderError::BadResponse { .. }));
    }

    #[test]
    fn raises_on_missing_choices() {
        let stub = Arc::new(StubHttpClient::new(vec![Ok(HttpResponse {
            status: 200,
            body: br#"{"object":"error","message":"auth"}"#.to_vec(),
        })]));
        let p = make_provider(stub);
        let err = p.generate("hi").unwrap_err();
        assert!(matches!(err, ProviderError::BadResponse { .. }));
    }

    #[test]
    fn from_env_reads_api_key() {
        std::env::set_var("AEGIS_TEST_PROVIDER_KEY", "env-key");
        let cfg = OpenAIChatProviderConfig::from_env("m", "AEGIS_TEST_PROVIDER_KEY").unwrap();
        std::env::remove_var("AEGIS_TEST_PROVIDER_KEY");
        assert_eq!(cfg.api_key, "env-key");
    }

    #[test]
    fn from_env_missing_yields_typed_error() {
        std::env::remove_var("AEGIS_TEST_PROVIDER_MISSING");
        let err = OpenAIChatProviderConfig::from_env("m", "AEGIS_TEST_PROVIDER_MISSING")
            .unwrap_err();
        match err {
            ProviderError::MissingCredential { var } => {
                assert_eq!(var, "AEGIS_TEST_PROVIDER_MISSING");
            }
            other => panic!("expected MissingCredential, got {other:?}"),
        }
    }

    #[test]
    fn with_base_url_strips_trailing_slash() {
        let cfg =
            OpenAIChatProviderConfig::new("m", "k").with_base_url("https://example.com/v1/");
        assert_eq!(cfg.base_url, "https://example.com/v1");
    }

    #[test]
    fn openrouter_configuration_carries_to_request() {
        let stub = Arc::new(StubHttpClient::new(vec![ok_response("done")]));
        let cfg = OpenAIChatProviderConfig::new("inclusionai/ling-2.6-1t:free", "router-key")
            .with_base_url("https://openrouter.ai/api/v1")
            .with_display_name("openrouter");
        let p = OpenAIChatProvider::with_http(cfg, stub.clone());
        p.generate("ping").unwrap();
        assert_eq!(p.name(), "openrouter");
        let captured = stub.captured_requests();
        assert_eq!(
            captured[0].url,
            "https://openrouter.ai/api/v1/chat/completions"
        );
        assert!(captured[0]
            .headers
            .iter()
            .any(|(k, v)| k == "Authorization" && v == "Bearer router-key"));
        let body: serde_json::Value = serde_json::from_slice(&captured[0].body).unwrap();
        assert_eq!(body["model"], "inclusionai/ling-2.6-1t:free");
    }

    #[test]
    fn groq_configuration_carries_to_request() {
        let stub = Arc::new(StubHttpClient::new(vec![ok_response("done")]));
        let cfg = OpenAIChatProviderConfig::new("llama-3.3-70b-versatile", "groq-key")
            .with_base_url("https://api.groq.com/openai/v1")
            .with_display_name("groq");
        let p = OpenAIChatProvider::with_http(cfg, stub.clone());
        p.generate("ping").unwrap();
        assert_eq!(p.name(), "groq");
        let captured = stub.captured_requests();
        assert_eq!(
            captured[0].url,
            "https://api.groq.com/openai/v1/chat/completions"
        );
    }
}
