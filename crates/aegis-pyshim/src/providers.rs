//! PyO3 wrapper for `aegis_providers::OpenAIChatProvider`.
//!
//! Surfaces a Python class named `RustOpenAIProvider` with the same
//! `.generate(prompt, tools=None) -> str` shape as
//! `aegis.agents.openai.OpenAIProvider`. Mutating-tool rejection
//! happens here at the boundary because Python callers pass tools
//! as a tuple of callables — that type doesn't cross PyO3 cleanly,
//! and the rejection is a Python-defined invariant anyway.
//!
//! The Python pipeline still uses the Python `OpenAIProvider` for
//! V1.1; this wrapper is the foundation the V1.3 Rust pipeline
//! will call.

use std::time::Duration;

use aegis_providers::{
    LLMProvider, OpenAIChatProvider, OpenAIChatProviderConfig, ProviderError,
};
use pyo3::prelude::*;
use pyo3::types::PyAny;

const MUTATING_TOOL_NAMES: &[&str] = &[
    "write_file",
    "create_file",
    "append_file",
    "delete_file",
    "rename_file",
    "move_file",
    "execute_command",
    "run_shell",
    "patch_file",
];

#[pyclass(name = "RustOpenAIProvider", module = "aegis._core")]
pub struct PyRustOpenAIProvider {
    inner: OpenAIChatProvider,
    last_used_tool_names: std::sync::Mutex<Vec<String>>,
    display_name: String,
}

impl PyRustOpenAIProvider {
    fn new_inner(cfg: OpenAIChatProviderConfig) -> Self {
        let display = cfg.display_name.clone();
        Self {
            inner: OpenAIChatProvider::new(cfg),
            last_used_tool_names: std::sync::Mutex::new(Vec::new()),
            display_name: display,
        }
    }
}

#[pymethods]
impl PyRustOpenAIProvider {
    #[new]
    #[pyo3(signature = (
        model_name = "gpt-4o-mini".to_string(),
        api_key = None,
        base_url = None,
        api_key_env = "OPENAI_API_KEY".to_string(),
        timeout = 120,
        display_name = None,
    ))]
    fn new(
        model_name: String,
        api_key: Option<String>,
        base_url: Option<String>,
        api_key_env: String,
        timeout: u64,
        display_name: Option<String>,
    ) -> PyResult<Self> {
        let key = match api_key {
            Some(k) => k,
            None => std::env::var(&api_key_env).map_err(|_| {
                pyo3::exceptions::PyValueError::new_err(format!(
                    "{api_key_env} is not set; pass api_key= explicitly or export the env var."
                ))
            })?,
        };
        let mut cfg = OpenAIChatProviderConfig::new(model_name, key)
            .with_timeout(Duration::from_secs(timeout));
        if let Some(url) = base_url {
            cfg = cfg.with_base_url(url);
        }
        if let Some(name) = display_name {
            cfg = cfg.with_display_name(name);
        }
        Ok(Self::new_inner(cfg))
    }

    #[getter]
    fn model_name(&self) -> &str {
        // Re-read from inner via display API; the model lives on the
        // config which we've moved into inner. We expose via a
        // trait-like getter on inner — but our trait doesn't have it,
        // so we recompute by stripping the URL. Simpler: return the
        // Provider name + look up via a separate accessor. For V1.1
        // we just return the display name; future commits can add a
        // trait method if real consumers need the model id.
        &self.display_name
    }

    #[getter]
    fn name(&self) -> &str {
        &self.display_name
    }

    #[getter]
    fn last_used_tools<'py>(&self, py: Python<'py>) -> Vec<PyObject> {
        // Mirror the Python provider's shape: a tuple-like of names.
        // We return names as strings (Python provider returns the
        // callables themselves; the gateway's `_emit_tool_surface`
        // only reads `__name__`, so a list of names is observable
        // identically from the trace's perspective).
        let names = self.last_used_tool_names.lock().unwrap().clone();
        names.into_iter().map(|n| n.into_py(py)).collect()
    }

    #[pyo3(signature = (prompt, tools = None))]
    fn generate(&self, prompt: &str, tools: Option<&PyAny>) -> PyResult<String> {
        // Defence-in-depth: refuse mutating callables before we
        // touch the LLM. Same invariant as the Python provider.
        let mut tool_names: Vec<String> = Vec::new();
        if let Some(t) = tools {
            if let Ok(seq) = t.iter() {
                for item in seq {
                    let item = item?;
                    let name: String = item
                        .getattr("__name__")
                        .and_then(|n| n.extract())
                        .unwrap_or_default();
                    if MUTATING_TOOL_NAMES.iter().any(|m| *m == name) {
                        return Err(pyo3::exceptions::PyValueError::new_err(format!(
                            "Tool '{name}' is a state-mutating callable and cannot \
                             be exposed to the LLM. Route writes through \
                             aegis.runtime.executor.Executor instead."
                        )));
                    }
                    if !name.is_empty() {
                        tool_names.push(name);
                    }
                }
            }
        }
        *self.last_used_tool_names.lock().unwrap() = tool_names;

        match self.inner.generate(prompt) {
            Ok(s) => Ok(s),
            Err(e) => Err(provider_err_to_py(&e)),
        }
    }
}

fn provider_err_to_py(e: &ProviderError) -> PyErr {
    // Match the Python provider's error shape: every failure becomes
    // a RuntimeError carrying the URL + status / body. The gateway
    // only inspects exception messages textually for cross-trace
    // search — keeping the same prefixes preserves that.
    match e {
        ProviderError::MissingCredential { var } => {
            pyo3::exceptions::PyValueError::new_err(format!("{var} is not set"))
        }
        ProviderError::MutatingToolRejected { name } => {
            pyo3::exceptions::PyValueError::new_err(format!(
                "Tool '{name}' is a state-mutating callable and cannot be exposed to the LLM."
            ))
        }
        ProviderError::HttpStatus { url, code, body } => pyo3::exceptions::PyRuntimeError::new_err(
            format!("OpenAI-compatible API returned HTTP {code} from {url}: {body}"),
        ),
        ProviderError::Network { url, source } => pyo3::exceptions::PyRuntimeError::new_err(
            format!("OpenAI-compatible API request to {url} failed: {source}"),
        ),
        ProviderError::BadResponse { url, body } => pyo3::exceptions::PyRuntimeError::new_err(
            format!("Unexpected response shape from {url}: {body:?}"),
        ),
        ProviderError::Timeout { url, timeout_secs } => pyo3::exceptions::PyRuntimeError::new_err(
            format!("OpenAI-compatible API request to {url} exceeded total_timeout={timeout_secs}s"),
        ),
    }
}

pub fn register(m: &PyModule) -> PyResult<()> {
    m.add_class::<PyRustOpenAIProvider>()?;
    Ok(())
}
