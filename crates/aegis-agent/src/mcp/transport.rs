//! Byte-level transport for the MCP JSON-RPC client.
//!
//! `JsonRpcTransport` is the boundary between the protocol layer
//! (`McpClient` — building requests, parsing responses) and the
//! actual byte channel. Production uses `StdioTransport` (subprocess
//! pipes); tests use `ScriptedTransport`.
//!
//! Per the V3 framing contract, the transport itself does NOT retry
//! on read/write failure or EOF. Failures bubble up as `McpError`
//! and the agent surfaces them — never auto-recovers behind the
//! user's back.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex};

use super::client::McpError;

/// Send/receive newline-delimited JSON-RPC frames. One frame per
/// line (the MCP stdio convention).
pub trait JsonRpcTransport: Send {
    fn send(&mut self, message: &str) -> Result<(), McpError>;
    fn recv(&mut self) -> Result<String, McpError>;
}

// ---------- production: subprocess over stdin/stdout ----------

/// Spawns an MCP server as a subprocess and pipes JSON-RPC over its
/// stdin/stdout. Stderr is sent to the parent process's stderr so
/// server diagnostics surface in operator logs.
pub struct StdioTransport {
    // Child must be kept alive (and dropped — which kills the
    // subprocess — when the transport is dropped). We keep it last
    // so it drops after stdin / stdout, ensuring graceful pipe
    // closure before the process is reaped.
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    _child: Child,
}

impl StdioTransport {
    /// Spawn `command` with `args`. The child inherits the parent's
    /// stderr (so its diagnostics surface in operator logs).
    pub fn spawn(command: &str, args: &[&str]) -> Result<Self, McpError> {
        let mut child = Command::new(command)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // stderr inherits from parent — server diagnostics are
            // a load-bearing signal for the human operator.
            .spawn()
            .map_err(|e| McpError::Spawn(format!("spawn `{command}`: {e}")))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| McpError::Spawn("child stdin missing".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| McpError::Spawn("child stdout missing".into()))?;
        Ok(Self {
            stdin,
            stdout: BufReader::new(stdout),
            _child: child,
        })
    }
}

impl JsonRpcTransport for StdioTransport {
    fn send(&mut self, message: &str) -> Result<(), McpError> {
        self.stdin
            .write_all(message.as_bytes())
            .map_err(|e| McpError::Transport(format!("write: {e}")))?;
        self.stdin
            .write_all(b"\n")
            .map_err(|e| McpError::Transport(format!("write newline: {e}")))?;
        self.stdin
            .flush()
            .map_err(|e| McpError::Transport(format!("flush: {e}")))
    }

    fn recv(&mut self) -> Result<String, McpError> {
        let mut line = String::new();
        let n = self
            .stdout
            .read_line(&mut line)
            .map_err(|e| McpError::Transport(format!("read: {e}")))?;
        if n == 0 {
            return Err(McpError::Transport(
                "EOF on subprocess stdout — server died".into(),
            ));
        }
        // Strip trailing newline(s) but keep the rest verbatim so
        // serde sees clean JSON.
        while line.ends_with('\n') || line.ends_with('\r') {
            line.pop();
        }
        Ok(line)
    }
}

// Allow construction with already-piped stdin/stdout (used by tests
// that bring their own subprocess wiring without going through Command).
impl<R, W> From<(W, R)> for InMemoryTransport<R, W>
where
    R: Read + Send,
    W: Write + Send,
{
    fn from((writer, reader): (W, R)) -> Self {
        Self {
            stdin: writer,
            stdout: BufReader::new(reader),
        }
    }
}

/// Generic transport over any reader+writer pair. Useful when the
/// MCP server lives elsewhere (network socket, in-process pipe, etc.).
pub struct InMemoryTransport<R, W>
where
    R: Read + Send,
    W: Write + Send,
{
    stdin: W,
    stdout: BufReader<R>,
}

impl<R, W> JsonRpcTransport for InMemoryTransport<R, W>
where
    R: Read + Send,
    W: Write + Send,
{
    fn send(&mut self, message: &str) -> Result<(), McpError> {
        self.stdin
            .write_all(message.as_bytes())
            .and_then(|_| self.stdin.write_all(b"\n"))
            .and_then(|_| self.stdin.flush())
            .map_err(|e| McpError::Transport(format!("write: {e}")))
    }

    fn recv(&mut self) -> Result<String, McpError> {
        let mut line = String::new();
        let n = self
            .stdout
            .read_line(&mut line)
            .map_err(|e| McpError::Transport(format!("read: {e}")))?;
        if n == 0 {
            return Err(McpError::Transport("EOF".into()));
        }
        while line.ends_with('\n') || line.ends_with('\r') {
            line.pop();
        }
        Ok(line)
    }
}

// ---------- test: scripted byte channel ----------

/// Test transport with deterministic behaviour: every `send` is
/// recorded; every `recv` pops a canned response off the queue.
/// When the queue is exhausted, returns a fixed `McpError`.
///
/// Clone-able: clones share inner state via `Arc<Mutex<>>`, so a
/// test can keep one handle for inspection while the client owns
/// another for driving the protocol.
#[derive(Clone, Default)]
pub struct ScriptedTransport {
    inner: Arc<Mutex<ScriptedInner>>,
}

#[derive(Default)]
struct ScriptedInner {
    pub recv_queue: VecDeque<Result<String, McpError>>,
    pub recorded_sends: Vec<String>,
}

impl ScriptedTransport {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Push a successful response to be returned by the next `recv`.
    pub fn push_recv(&self, response: impl Into<String>) -> &Self {
        self.inner
            .lock()
            .unwrap()
            .recv_queue
            .push_back(Ok(response.into()));
        self
    }

    /// Push an error to be returned by the next `recv`.
    pub fn push_recv_err(&self, error: McpError) -> &Self {
        self.inner
            .lock()
            .unwrap()
            .recv_queue
            .push_back(Err(error));
        self
    }

    pub fn recorded_sends(&self) -> Vec<String> {
        self.inner.lock().unwrap().recorded_sends.clone()
    }
}

impl JsonRpcTransport for ScriptedTransport {
    fn send(&mut self, message: &str) -> Result<(), McpError> {
        self.inner
            .lock()
            .unwrap()
            .recorded_sends
            .push(message.to_string());
        Ok(())
    }

    fn recv(&mut self) -> Result<String, McpError> {
        self.inner
            .lock()
            .unwrap()
            .recv_queue
            .pop_front()
            .unwrap_or_else(|| Err(McpError::Transport("scripted recv queue exhausted".into())))
    }
}
