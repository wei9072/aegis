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
            Err(error) => Err(HttpError(error.to_string())),
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
