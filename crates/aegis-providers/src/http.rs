//! HTTP abstraction so providers can be tested without a network.
//!
//! `HttpClient` is the contract; `UreqClient` is the production
//! impl using `ureq`. Tests inject a fake client.

use std::sync::Mutex;
use std::time::Duration;

use crate::ProviderError;

#[derive(Clone, Debug)]
pub struct HttpRequest {
    pub url: String,
    pub method: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub timeout: Duration,
}

#[derive(Clone, Debug)]
pub struct HttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

pub trait HttpClient: Send + Sync {
    fn execute(&self, req: HttpRequest) -> Result<HttpResponse, ProviderError>;
}

/// Default sync HTTP client built on `ureq`.
pub struct UreqClient {
    agent: ureq::Agent,
}

impl UreqClient {
    pub fn new() -> Self {
        Self {
            agent: ureq::AgentBuilder::new()
                .timeout_connect(Duration::from_secs(10))
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
    fn execute(&self, req: HttpRequest) -> Result<HttpResponse, ProviderError> {
        let mut request = self.agent.request(&req.method, &req.url);
        for (k, v) in &req.headers {
            request = request.set(k, v);
        }
        let request = request.timeout(req.timeout);
        let response = match request.send_bytes(&req.body) {
            Ok(r) => r,
            Err(ureq::Error::Status(code, r)) => {
                let url = req.url.clone();
                let body = r
                    .into_string()
                    .unwrap_or_else(|_| "<non-utf8>".to_string());
                return Err(ProviderError::HttpStatus { url, code, body });
            }
            Err(e) => {
                return Err(ProviderError::Network {
                    url: req.url.clone(),
                    source: Box::new(e),
                });
            }
        };
        let status = response.status();
        let mut body = Vec::new();
        if let Err(e) = response.into_reader().read_to_end(&mut body) {
            return Err(ProviderError::Network {
                url: req.url,
                source: Box::new(e),
            });
        }
        Ok(HttpResponse { status, body })
    }
}

/// Test-only HTTP client — collects requests + returns a queue of
/// canned responses (or errors) in FIFO order. Public so consumers
/// can write their own provider-level tests.
pub struct StubHttpClient {
    state: Mutex<StubState>,
}

struct StubState {
    captured: Vec<HttpRequest>,
    responses: Vec<Result<HttpResponse, ProviderError>>,
}

impl StubHttpClient {
    pub fn new(responses: Vec<Result<HttpResponse, ProviderError>>) -> Self {
        Self {
            state: Mutex::new(StubState {
                captured: Vec::new(),
                responses,
            }),
        }
    }

    pub fn captured_requests(&self) -> Vec<HttpRequest> {
        self.state.lock().unwrap().captured.clone()
    }
}

impl HttpClient for StubHttpClient {
    fn execute(&self, req: HttpRequest) -> Result<HttpResponse, ProviderError> {
        let mut state = self.state.lock().unwrap();
        state.captured.push(req.clone());
        if state.responses.is_empty() {
            return Err(ProviderError::Network {
                url: req.url,
                source: "stub: no more queued responses".into(),
            });
        }
        state.responses.remove(0)
    }
}

use std::io::Read;
