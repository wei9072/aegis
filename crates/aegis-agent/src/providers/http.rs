//! HTTP transport abstraction for LLM providers.
//!
//! `HttpClient` is the boundary between provider logic (request
//! building, response parsing) and the actual network IO. Production
//! impls hit real endpoints; `StubHttpClient` records calls and
//! replays canned responses for tests.
//!
//! Rationale for an internal trait rather than handing `ureq::Agent`
//! directly to providers: tests need to drive the provider end-to-end
//! without a live HTTP server, and the boundary keeps providers
//! testable in isolation.

use std::fmt::{Display, Formatter};
use std::sync::Mutex;

/// Translate a raw transport-layer error string into a one-paragraph
/// hint that names the URL and the most likely fix. Pattern-matches
/// the underlying HTTP client's wording — a real fix walks the user
/// from "DNS failed" to "you probably mistyped the host".
///
/// Both lines (raw + hint) are returned so the original error is
/// still visible in logs / `--verbose` mode.
#[must_use]
pub fn friendly_transport_error(url: &str, raw: &str) -> String {
    let lc = raw.to_lowercase();
    let host = url
        .split("://")
        .nth(1)
        .and_then(|rest| rest.split('/').next())
        .unwrap_or(url);

    let hint = if lc.contains("dns") || lc.contains("name resolution") || lc.contains("lookup") {
        format!(
            "hint: host {host:?} did not resolve. Common causes: typo in base_url, \
             missing/extra subdomain (e.g. 'api.openrouter.ai' instead of \
             'openrouter.ai/api/v1'), or no internet. Check AEGIS_*_BASE_URL or \
             [provider.*] base_url in ~/.config/aegis/config.toml."
        )
    } else if lc.contains("tls") || lc.contains("handshake") || lc.contains("certificate") {
        format!(
            "hint: TLS handshake to {host:?} failed. Common causes: corporate \
             MITM proxy, system clock skew, or 'http://' typo'd as 'https://' \
             on a plaintext-only endpoint."
        )
    } else if lc.contains("connection refused") || lc.contains("connect error") {
        format!(
            "hint: {host:?} refused the connection. Common causes: wrong port, \
             server down, firewall blocking outbound HTTPS."
        )
    } else if lc.contains("timed out") || lc.contains("timeout") {
        format!(
            "hint: request to {host:?} timed out. Common causes: slow upstream, \
             network congestion, or VPN required to reach the endpoint."
        )
    } else {
        format!("hint: see raw transport error for {host:?}")
    };

    format!("{raw}\n{hint}")
}

/// Same idea but for HTTP-layer failures (4xx / 5xx) where the
/// connection succeeded but the server rejected the request.
#[must_use]
pub fn friendly_http_status(url: &str, status: u16, body: &str) -> String {
    let host = url
        .split("://")
        .nth(1)
        .and_then(|rest| rest.split('/').next())
        .unwrap_or(url);

    let hint = match status {
        401 | 403 => format!(
            "hint: {host:?} rejected the API key. Check AEGIS_*_API_KEY env var \
             or [provider.*] api_key_env in ~/.config/aegis/config.toml."
        ),
        404 => format!(
            "hint: {host:?} returned 404. Common causes: base_url missing the \
             '/v1' (or '/api/v1') path segment, or model name has a typo."
        ),
        429 => format!(
            "hint: {host:?} rate-limited (429). Provider quota exceeded — \
             check your dashboard or use a different model."
        ),
        413 => format!(
            "hint: payload too large (413). The conversation history exceeds \
             this model's context window. Use /compact or pick a model with a \
             larger context."
        ),
        500..=599 => format!(
            "hint: {host:?} returned a server error ({status}). Provider-side \
             outage; retry later or switch provider."
        ),
        _ => format!("hint: {host:?} returned HTTP {status}"),
    };

    format!("HTTP {status} from {url}\n{body}\n{hint}")
}

/// One outbound HTTP POST.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedRequest {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

/// One inbound HTTP response. Kept minimal — provider impls only
/// need status code and body text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub body: String,
}

/// Transport error. Wraps the underlying client's failure into a
/// stable string so providers don't need to depend on `ureq` types
/// directly. Per the V3 framing contract, providers translate this
/// into `RuntimeError` and surface as `StoppedReason::ProviderError`
/// — they MUST NOT retry the request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpError(pub String);

impl Display for HttpError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for HttpError {}

/// Minimal HTTP contract. Sync by design — provider impls block on
/// each request. (V3.1 keeps the conversation loop synchronous;
/// async lands in a separate phase if a real consumer needs it.)
pub trait HttpClient: Send + Sync {
    fn post(
        &self,
        url: &str,
        headers: &[(String, String)],
        body: &str,
    ) -> Result<HttpResponse, HttpError>;
}

/// Production `HttpClient` backed by `ureq`.
pub struct UreqClient {
    agent: ureq::Agent,
}

impl UreqClient {
    #[must_use]
    pub fn new() -> Self {
        Self {
            agent: ureq::AgentBuilder::new()
                .timeout(std::time::Duration::from_secs(120))
                .build(),
        }
    }
}

impl Default for UreqClient {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpClient for UreqClient {
    fn post(
        &self,
        url: &str,
        headers: &[(String, String)],
        body: &str,
    ) -> Result<HttpResponse, HttpError> {
        let mut req = self.agent.post(url);
        for (name, value) in headers {
            req = req.set(name, value);
        }
        match req.send_string(body) {
            Ok(response) => {
                let status = response.status();
                let body = response
                    .into_string()
                    .map_err(|e| HttpError(format!("read body failed: {e}")))?;
                Ok(HttpResponse { status, body })
            }
            // ureq::Error::Status indicates an HTTP status >= 400.
            // We surface it as a normal HttpResponse so providers can
            // include the body in their error message — that body
            // often contains the API's error detail.
            Err(ureq::Error::Status(status, response)) => {
                let body = response
                    .into_string()
                    .unwrap_or_else(|_| "<unreadable body>".into());
                Ok(HttpResponse { status, body })
            }
            Err(error) => Err(HttpError(friendly_transport_error(
                url,
                &error.to_string(),
            ))),
        }
    }
}

/// Test `HttpClient` that records every request and replays a queue
/// of canned responses. When the queue is exhausted, returns a fixed
/// "exhausted" `HttpError`.
#[derive(Default)]
pub struct StubHttpClient {
    inner: Mutex<StubInner>,
}

#[derive(Default)]
struct StubInner {
    pub responses: std::collections::VecDeque<Result<HttpResponse, HttpError>>,
    pub recorded_requests: Vec<RecordedRequest>,
}

impl StubHttpClient {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push_ok(&self, status: u16, body: impl Into<String>) -> &Self {
        self.inner
            .lock()
            .unwrap()
            .responses
            .push_back(Ok(HttpResponse {
                status,
                body: body.into(),
            }));
        self
    }

    pub fn push_err(&self, message: impl Into<String>) -> &Self {
        self.inner
            .lock()
            .unwrap()
            .responses
            .push_back(Err(HttpError(message.into())));
        self
    }

    #[must_use]
    pub fn recorded_requests(&self) -> Vec<RecordedRequest> {
        self.inner.lock().unwrap().recorded_requests.clone()
    }
}

impl HttpClient for StubHttpClient {
    fn post(
        &self,
        url: &str,
        headers: &[(String, String)],
        body: &str,
    ) -> Result<HttpResponse, HttpError> {
        let mut inner = self.inner.lock().unwrap();
        inner.recorded_requests.push(RecordedRequest {
            url: url.to_string(),
            headers: headers.to_vec(),
            body: body.to_string(),
        });
        inner
            .responses
            .pop_front()
            .unwrap_or_else(|| Err(HttpError("stub http client: response queue exhausted".into())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_dns_hint_names_host_and_config_keys() {
        let out = friendly_transport_error(
            "https://api.openrouter.ai/api/v1/chat/completions",
            "Dns Failed: failed to lookup address",
        );
        assert!(out.contains("api.openrouter.ai"), "out: {out}");
        assert!(out.contains("did not resolve"), "out: {out}");
        assert!(out.contains("AEGIS_"));
        assert!(out.contains("Dns Failed")); // raw error preserved
    }

    #[test]
    fn transport_tls_hint() {
        let out = friendly_transport_error(
            "https://api.example.com/v1",
            "tls handshake failed",
        );
        assert!(out.contains("TLS handshake"));
        assert!(out.contains("api.example.com"));
    }

    #[test]
    fn transport_connection_refused_hint() {
        let out = friendly_transport_error(
            "http://127.0.0.1:11434/v1",
            "connection refused",
        );
        assert!(out.contains("refused the connection"));
        assert!(out.contains("127.0.0.1:11434"));
    }

    #[test]
    fn http_status_404_suggests_v1_path() {
        let out = friendly_http_status(
            "https://openrouter.ai/api/v1/chat/completions",
            404,
            "{\"error\":\"not found\"}",
        );
        assert!(out.contains("404"));
        assert!(out.contains("/v1"), "out: {out}");
    }

    #[test]
    fn http_status_401_suggests_api_key() {
        let out = friendly_http_status("https://api.example.com/v1", 401, "");
        assert!(out.contains("API key"));
        assert!(out.contains("AEGIS_"));
    }

    #[test]
    fn http_status_429_mentions_rate_limit() {
        let out = friendly_http_status("https://api.example.com/v1", 429, "rate limit");
        assert!(out.contains("rate-limited"));
    }

    #[test]
    fn http_status_5xx_says_server_outage() {
        let out = friendly_http_status("https://api.example.com/v1", 503, "");
        assert!(out.contains("server error"));
        assert!(out.contains("503"));
    }
}
