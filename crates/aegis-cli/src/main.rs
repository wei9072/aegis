//! `aegis` — V1.9 Rust-native CLI binary.
//!
//! Subset of the V0.x Python `aegis.cli`. Today it covers two
//! subcommands that don't need an LLM provider — `aegis check` for
//! Ring 0 + Ring 0.5 file analysis, and `aegis languages` for the
//! supported-language registry. Pipeline + scenario subcommands
//! arrive once the Rust LLMPlanner has a wired-in HTTP provider
//! (per docs/v1_rust_port_plan.md V1.9 follow-up).
//!
//! No PyO3, no Python at runtime. Links directly against:
//!   - `aegis-core` for Ring 0 syntax check + signal extraction
//!   - `aegis-ir` for the patch IR (used by `apply` once it lands)
//!   - `aegis-runtime` for Executor + PlanValidator
//!   - `aegis-providers` for the (future) LLMPlanner

use std::path::PathBuf;
use std::process::ExitCode;

use aegis_core::ast::registry::LanguageRegistry;
use aegis_core::signal_layer_pyapi::extract_signals_native;
use aegis_providers::{LLMPlanner, OpenAIChatProvider, OpenAIChatProviderConfig};
use aegis_runtime::{
    run_pipeline, PipelineOptions, WorkspaceContextBuilder,
};
use clap::{Parser, Subcommand};

mod config;
mod input;
mod render;
mod session_store;
mod setup;

#[derive(Parser, Debug)]
#[command(
    name = "aegis",
    version,
    about = "Behavior harness for LLM-driven workflows. Rejects regressions instead of teaching the model.",
    long_about = "V1.9 — Rust-native CLI. The full Python `aegis` CLI \
                  remains the reference today; this binary covers the \
                  subset that doesn't need an LLM provider plugged in."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Print Ring 0 + Ring 0.5 analysis for one or more source files.
    Check {
        /// One or more source files (Python, TS, JS, Go, Java, C#,
        /// PHP, Swift, Kotlin, Dart — see `aegis languages` for the
        /// full registry).
        #[arg(required = true)]
        files: Vec<PathBuf>,
        /// Emit JSON instead of human-readable text.
        #[arg(long)]
        json: bool,
    },

    /// List every language adapter the registry knows about.
    Languages {
        /// Emit JSON instead of human-readable text.
        #[arg(long)]
        json: bool,
    },

    /// Pipeline operations.
    Pipeline {
        #[command(subcommand)]
        sub: PipelineSub,
    },

    /// Scan an entire workspace — Ring 0 + Ring 0.5 across every
    /// supported file, plus import-graph cycle detection. Parallel
    /// (rayon) + persistent mtime+size cache (`<workspace>/.aegis-cache/`)
    /// so re-scans on a maintained codebase finish in <1s.
    Scan {
        /// Workspace root to scan. Defaults to current directory.
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
        /// Top-N highest-cost files to print in the summary.
        #[arg(long, default_value_t = 10)]
        top: usize,
        /// Skip the persistent cache. Forces a full rescan even if
        /// nothing has changed since last run.
        #[arg(long)]
        no_cache: bool,
        /// Skip cross-file import-graph cycle detection.
        #[arg(long)]
        no_cycles: bool,
        /// After scan, also auto-detect + run the project's test
        /// suite (Cargo.toml → `cargo test`, pyproject.toml →
        /// `pytest`, etc.) and append the verdict.
        #[arg(long)]
        verify: bool,
        /// Emit the report as JSON instead of human-readable text.
        #[arg(long)]
        json: bool,
    },

    /// Interactive wizard that writes ~/.config/aegis/config.toml
    /// so you don't have to memorise env-var names.
    ///
    /// Walks you through provider choice (OpenAI-compat / Anthropic
    /// / Gemini), base URL preset (OpenRouter / Groq / Ollama / …),
    /// model name, and which env var holds your API key. Writes the
    /// TOML file; does NOT export anything to your shell.
    Setup,

    /// V3 coding agent — full agent surface with aegis core gates
    /// always-on by default.
    ///
    /// Defaults (no flags needed):
    ///   - Workspace = current directory
    ///   - Tools     = Read / Glob / Grep / Edit / Write
    ///   - Aegis     = LocalAegisPredictor + AegisCostObserver +
    ///                 StalemateDetector all active
    ///   - Permission = workspace-write
    ///
    /// Flags are opt-out / opt-extras, not opt-in. Picks a provider
    /// from env vars (first match wins):
    ///   AEGIS_OPENAI_BASE_URL + AEGIS_OPENAI_MODEL  → OpenAI-compat
    ///   AEGIS_ANTHROPIC_API_KEY + AEGIS_ANTHROPIC_MODEL → Anthropic
    ///   AEGIS_GEMINI_API_KEY + AEGIS_GEMINI_MODEL   → Gemini
    Chat {
        /// Prompt to send. If omitted: tty → REPL, pipe → read stdin.
        prompt: Option<String>,
        /// Optional system prompt prefix.
        #[arg(long)]
        system: Option<String>,
        /// Per-turn iteration budget.
        #[arg(long, default_value_t = 5)]
        max_iters: u32,
        /// Cumulative cost regression budget for the session.
        #[arg(long)]
        cost_budget: Option<f64>,
        /// Workspace root. Defaults to current directory.
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
        /// Permission mode: read-only | workspace-write | plan | danger-full-access.
        /// `plan` lets reads execute normally but routes writes through
        /// the aegis predictor for cost-delta scoring without touching
        /// disk — useful for proposing a refactor without committing it.
        #[arg(long, default_value = "workspace-write")]
        permission_mode: String,
        /// Run a verifier when the LLM signals "done".
        /// Auto-detect (Cargo.toml/pyproject.toml/etc.) or pass a
        /// custom shell command via --verifier-cmd.
        #[arg(long)]
        verify: bool,
        /// Custom shell verifier command (overrides auto-detect).
        #[arg(long)]
        verifier_cmd: Option<String>,
        /// Restrict to read-only tools (Read/Glob/Grep). Equivalent
        /// to --permission-mode read-only — LLM cannot modify files.
        #[arg(long)]
        read_only: bool,
        /// **DANGEROUS** — turn off aegis core's PreToolUse predictor
        /// and per-edit cost observer. Edit/Write tools become
        /// unprotected. Provided for debugging the agent loop in
        /// isolation; the framing-correct way to use `aegis chat`
        /// is WITHOUT this flag.
        #[arg(long)]
        no_aegis: bool,
        /// Mount one or more **extra** MCP servers as additional
        /// tool sources beyond the built-ins. Each value is a shell
        /// command (e.g. `node my-server.js`). The LLM gets the
        /// server's advertised tools alongside Read/Glob/Grep/Edit/Write.
        #[arg(long = "mcp", value_name = "COMMAND")]
        mcp: Vec<String>,
        /// Resume a saved session. Default (`--resume` with no value)
        /// is `latest` — picks the newest file from the sessions
        /// directory (`$XDG_DATA_HOME/aegis/sessions/`, fallback
        /// `~/.local/share/aegis/sessions/`). Pass an explicit path
        /// to load a specific session. The runtime auto-saves every
        /// turn so subsequent invocations can resume.
        #[arg(
            long,
            value_name = "PATH_OR_LATEST",
            num_args = 0..=1,
            default_missing_value = "latest",
        )]
        resume: Option<String>,
        /// Print every tool call's input + per-call structural cost
        /// delta inline in the REPL. Off by default — keeps the
        /// turn output uncluttered. Helpful when investigating why
        /// aegis rejected an edit or when a turn felt slow.
        #[arg(long)]
        verbose: bool,
        /// Print a commented `~/.config/aegis/config.toml` template
        /// to stdout and exit. No chat happens. Useful when you'd
        /// rather edit the file by hand than use `aegis setup`.
        #[arg(long)]
        print_config_template: bool,
        /// Emit the full result as JSON.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand, Debug)]
enum PipelineSub {
    /// Run the multi-turn refactor pipeline against `--root` until
    /// the planner declares done, signals stalemate, or `--max-iters`
    /// is reached. Provider config comes from environment variables:
    ///
    ///   AEGIS_PROVIDER     openai (default) | openrouter | groq
    ///   AEGIS_MODEL        e.g. gpt-4o-mini, openai/gpt-4o-mini, llama-3.3-70b-versatile
    ///   AEGIS_API_KEY      provider API key (or per-provider env: see below)
    ///
    /// Per-provider key env vars (used when AEGIS_API_KEY isn't set):
    ///   OPENAI_API_KEY     for AEGIS_PROVIDER=openai
    ///   OPENROUTER_API_KEY for AEGIS_PROVIDER=openrouter
    ///   GROQ_API_KEY       for AEGIS_PROVIDER=groq
    Run {
        /// Refactor task description fed into the planner prompt.
        #[arg(long)]
        task: String,
        /// Workspace root.
        #[arg(long, default_value = ".")]
        root: PathBuf,
        /// Optional scope paths (relative to root); patches must
        /// stay inside these.
        #[arg(long)]
        scope: Vec<String>,
        /// Maximum loop iterations.
        #[arg(long, default_value_t = 3)]
        max_iters: u32,
        /// Skip file-snippet inclusion in prompts (faster on large repos).
        #[arg(long)]
        no_snippets: bool,
        /// Suppress per-iteration trace.
        #[arg(long)]
        quiet: bool,
        /// Emit final result as JSON.
        #[arg(long)]
        json: bool,
    },
}

fn main() -> ExitCode {
    // Apply config-file env vars BEFORE clap parses. Shell exports
    // always win — config is the fallback layer.
    match config::try_load() {
        Some(Ok(cfg)) => {
            let applied = config::apply_to_env(&cfg);
            if !applied.is_empty() {
                eprintln!(
                    "aegis: loaded {} env vars from config: {}",
                    applied.len(),
                    applied.join(", ")
                );
            }
        }
        Some(Err(e)) => {
            eprintln!("aegis: config error: {e}");
            // Non-fatal — proceed with whatever env shell provides.
        }
        None => {} // no config file, normal case
    }

    let cli = Cli::parse();
    match cli.command {
        Command::Check { files, json } => cmd_check(&files, json),
        Command::Languages { json } => cmd_languages(json),
        Command::Setup => match setup::run() {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("aegis setup: {e}");
                ExitCode::FAILURE
            }
        },
        Command::Scan {
            workspace,
            top,
            no_cache,
            no_cycles,
            verify,
            json,
        } => cmd_scan(workspace, top, no_cache, no_cycles, verify, json),
        Command::Pipeline { sub } => match sub {
            PipelineSub::Run {
                task,
                root,
                scope,
                max_iters,
                no_snippets,
                quiet,
                json,
            } => cmd_pipeline_run(
                &task,
                &root,
                if scope.is_empty() { None } else { Some(scope) },
                max_iters,
                !no_snippets,
                quiet,
                json,
            ),
        },
        Command::Chat {
            prompt,
            system,
            max_iters,
            cost_budget,
            workspace,
            permission_mode,
            verify,
            verifier_cmd,
            read_only,
            no_aegis,
            mcp,
            resume,
            verbose,
            print_config_template,
            json,
        } => {
            if print_config_template {
                print!("{}", config::TEMPLATE);
                return ExitCode::SUCCESS;
            }
            cmd_chat(
                prompt,
                system,
                max_iters,
                cost_budget,
                workspace,
                &permission_mode,
                verify,
                verifier_cmd,
                read_only,
                no_aegis,
                mcp,
                resume,
                verbose,
                json,
            )
        }
    }
}

/// Friendly relative-age string for the `/sessions` listing.
/// Coarse buckets are fine — this is glanceable UX, not a metric.
fn human_age(secs: u64) -> String {
    match secs {
        0..=59 => format!("{secs}s ago"),
        60..=3599 => format!("{}m ago", secs / 60),
        3600..=86_399 => format!("{}h ago", secs / 3600),
        _ => format!("{}d ago", secs / 86_400),
    }
}

#[allow(clippy::too_many_arguments)]
fn cmd_chat(
    prompt: Option<String>,
    system: Option<String>,
    max_iters: u32,
    cost_budget: Option<f64>,
    workspace: PathBuf,
    permission_mode_str: &str,
    verify: bool,
    verifier_cmd: Option<String>,
    read_only: bool,
    no_aegis: bool,
    mcp_servers: Vec<String>,
    resume: Option<String>,
    verbose: bool,
    json: bool,
) -> ExitCode {
    use aegis_agent::aegis_predict::LocalAegisPredictor;
    use aegis_agent::agent_tools::{ReadOnlyTools, WorkspaceTools};
    use aegis_agent::api::{ApiClient, ToolDefinition};
    use aegis_agent::cost_observer_aegis::AegisCostObserver;
    use aegis_agent::mcp::{McpClient, McpToolExecutor, StdioTransport};
    use aegis_agent::multi_tool::{MultiToolExecutor, ToolSource};
    use aegis_agent::permission::{PermissionMode, PermissionPolicy};
    use aegis_agent::providers::{
        AnthropicConfig, AnthropicProvider, GeminiConfig, GeminiProvider, OpenAiCompatConfig,
        OpenAiCompatProvider, UreqClient,
    };
    use aegis_agent::tool::ToolExecutor;
    use aegis_agent::verifier::{AgentTaskVerifier, ShellVerifier, TestVerifier};
    use aegis_agent::{AgentConfig, ConversationRuntime, Session};
    use std::io::{IsTerminal, Read};

    // Decide UX mode:
    //   prompt arg supplied            → one-shot
    //   no prompt + stdin is a tty     → interactive REPL
    //   no prompt + stdin is a pipe    → read all stdin as one prompt
    //                                    (preserves the previous shell-pipe contract)
    enum Mode {
        OneShot(String),
        Interactive,
    }
    let mode = match prompt {
        Some(p) if !p.trim().is_empty() => Mode::OneShot(p),
        _ => {
            if std::io::stdin().is_terminal() {
                Mode::Interactive
            } else {
                let mut buf = String::new();
                if std::io::stdin().read_to_string(&mut buf).is_err() || buf.trim().is_empty() {
                    eprintln!("aegis chat: no prompt provided (pass as arg or pipe via stdin)");
                    return ExitCode::from(2);
                }
                Mode::OneShot(buf)
            }
        }
    };

    // Resolve permission mode.
    let permission_mode = match permission_mode_str {
        "read-only" => PermissionMode::ReadOnly,
        "workspace-write" => PermissionMode::WorkspaceWrite,
        "plan" => PermissionMode::Plan,
        "danger-full-access" => PermissionMode::DangerFullAccess,
        other => {
            eprintln!(
                "aegis chat: unknown --permission-mode {other:?} (allowed: \
                 read-only | workspace-write | plan | danger-full-access)"
            );
            return ExitCode::from(2);
        }
    };

    // Pick a provider from env vars. First match wins.
    let (provider, provider_label): (Box<dyn aegis_agent::api::ChatProvider>, String) =
        if let Some(c) = OpenAiCompatConfig::from_env() {
            let label = format!("openai-compat ({} {})", c.base_url, c.model);
            (
                Box::new(OpenAiCompatProvider::new(c, Box::new(UreqClient::new()))),
                label,
            )
        } else if let Some(c) = AnthropicConfig::from_env() {
            let label = format!("anthropic ({})", c.model);
            (
                Box::new(AnthropicProvider::new(c, Box::new(UreqClient::new()))),
                label,
            )
        } else if let Some(c) = GeminiConfig::from_env() {
            let label = format!("gemini ({})", c.model);
            (
                Box::new(GeminiProvider::new(c, Box::new(UreqClient::new()))),
                label,
            )
        } else {
            eprintln!(
                "aegis chat: no provider env vars set. Set one of:\n  \
                 AEGIS_OPENAI_BASE_URL + AEGIS_OPENAI_MODEL (+ AEGIS_OPENAI_API_KEY)\n  \
                 AEGIS_ANTHROPIC_API_KEY + AEGIS_ANTHROPIC_MODEL\n  \
                 AEGIS_GEMINI_API_KEY + AEGIS_GEMINI_MODEL"
            );
            return ExitCode::from(2);
        };
    eprintln!("aegis chat: provider = {provider_label}");

    // Optional verifier.
    let verifier: Option<Box<dyn AgentTaskVerifier>> = if let Some(cmd) = verifier_cmd {
        let mut parts = cmd.split_whitespace();
        match parts.next() {
            Some(program) => Some(Box::new(
                ShellVerifier::new(program).args(parts.map(String::from)),
            )),
            None => None,
        }
    } else if verify {
        match TestVerifier::auto_detect_composite(&workspace) {
            Some(v) => Some(Box::new(v)),
            None => {
                eprintln!(
                    "aegis chat: --verify requested but no project marker found in {} \
                     (Cargo.toml / pyproject.toml / package.json / go.mod). Skipping.",
                    workspace.display()
                );
                None
            }
        }
    } else {
        None
    };

    // Resume from a saved session if requested. Failure surfaces
    // immediately — we don't silently start a fresh session when the
    // user explicitly asked to continue an old one.
    let initial_session = if let Some(arg) = resume.as_deref() {
        match session_store::resolve_resume(arg).and_then(|p| {
            session_store::load(&p).map(|s| (p, s)).map_err(|e| e.to_string())
        }) {
            Ok((path, session)) => {
                eprintln!(
                    "aegis chat: resumed {} ({} messages)",
                    path.display(),
                    session.messages.len()
                );
                session
            }
            Err(e) => {
                eprintln!("aegis chat: --resume failed: {e}");
                return ExitCode::from(2);
            }
        }
    } else {
        Session::new()
    };

    let system_prompt = system.map(|s| vec![s]).unwrap_or_default();

    // Tool surface: aegis core defaults to ON. Read-only mode strips
    // out Edit/Write; otherwise WorkspaceTools (Read/Glob/Grep + Edit/Write)
    // is the default.
    let effective_read_only = read_only || permission_mode == PermissionMode::ReadOnly;
    let mut sources: Vec<ToolSource> = Vec::new();
    if effective_read_only {
        eprintln!("aegis chat: tool surface = read-only (Read / Glob / Grep)");
        sources.push(ToolSource::new(
            "read_only",
            Box::new(ReadOnlyTools::new(workspace.clone())),
            ReadOnlyTools::definitions(),
        ));
    } else {
        let aegis_status = if no_aegis {
            "DISABLED (--no-aegis)"
        } else {
            "active (LocalAegisPredictor + AegisCostObserver)"
        };
        eprintln!(
            "aegis chat: tool surface = workspace (Read / Glob / Grep / Edit / Write); \
             aegis core = {aegis_status}"
        );
        sources.push(ToolSource::new(
            "workspace",
            Box::new(WorkspaceTools::new(workspace.clone())),
            WorkspaceTools::definitions(),
        ));
    }
    for spec in &mcp_servers {
        // Spec is a shell command — split on whitespace, first token
        // is the program, rest are args.
        let mut parts = spec.split_whitespace();
        let program = match parts.next() {
            Some(p) => p,
            None => {
                eprintln!("aegis chat: --mcp value is empty, skipping");
                continue;
            }
        };
        let args: Vec<&str> = parts.collect();
        let transport = match StdioTransport::spawn(program, &args) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("aegis chat: failed to spawn MCP server {spec:?}: {e}");
                return ExitCode::from(2);
            }
        };
        let client = match McpClient::new(Box::new(transport)) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("aegis chat: MCP handshake failed for {spec:?}: {e}");
                return ExitCode::from(2);
            }
        };
        eprintln!(
            "aegis chat: mounted MCP server {spec:?} (server: {} v{})",
            client.server_name, client.server_version
        );
        let executor = match McpToolExecutor::new(client) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("aegis chat: failed to load tool list from {spec:?}: {e}");
                return ExitCode::from(2);
            }
        };
        let defs = executor.tool_definitions();
        sources.push(ToolSource::new(
            format!("mcp:{program}"),
            Box::new(executor),
            defs,
        ));
    }

    let (executor, tool_defs): (Box<dyn ToolExecutor>, Vec<ToolDefinition>) = {
        let multi = MultiToolExecutor::new(sources);
        let defs = multi.all_definitions();
        (Box::new(multi), defs)
    };

    let mut rt = ConversationRuntime::new(
        initial_session,
        provider,
        executor,
        system_prompt,
        tool_defs,
        AgentConfig {
            max_iterations_per_turn: max_iters,
            session_cost_budget: cost_budget,
            workspace_root: Some(workspace.clone()),
        },
    )
    .with_permission_policy(PermissionPolicy::standard(permission_mode));

    // Aegis core defaults to ON. --no-aegis is the explicit
    // (warned) opt-out for debugging the agent loop in isolation.
    if !no_aegis {
        rt = rt
            .with_predictor(Box::new(LocalAegisPredictor::new(workspace.clone())))
            .with_cost_observer(Box::new(AegisCostObserver::new(workspace.clone())));
    } else {
        eprintln!(
            "aegis chat: ⚠ aegis core disabled (--no-aegis). \
             Edit/Write tools will not go through validate_change \
             before mutating files. Use only for agent-loop debugging."
        );
    }

    if let Some(v) = verifier {
        rt = rt.with_verifier(v);
    }

    match mode {
        Mode::OneShot(p) => {
            // One-shot: streaming would interleave with the JSON
            // output, so keep it non-streaming and print the full
            // response at the end.
            run_one_shot(&mut rt, &p, json)
        }
        Mode::Interactive => {
            // REPL: subscribe to streaming so text appears as the
            // model emits it. The renderer prints raw chunks
            // (markdown rendering still happens at end of turn for
            // the formatted block).
            run_repl(&mut rt, verbose)
        }
    }
}

fn run_one_shot<C, T>(
    rt: &mut aegis_agent::ConversationRuntime<C, T>,
    prompt: &str,
    json: bool,
) -> ExitCode
where
    C: aegis_agent::api::ApiClient,
    T: aegis_agent::tool::ToolExecutor,
{
    use aegis_agent::StoppedReason;

    let result = rt.run_turn(prompt);
    let response_text = collect_last_assistant_text(rt);

    if json {
        let payload = serde_json::json!({
            "stopped_reason": format!("{:?}", result.stopped_reason),
            "iterations": result.iterations,
            "task_verdict": result.task_verdict.as_ref().map(|v| serde_json::json!({
                "pattern": format!("{:?}", v.pattern),
                "rationale": v.verifier_result.as_ref().map(|r| r.rationale.clone()),
            })),
            "response": response_text,
        });
        println!("{}", serde_json::to_string_pretty(&payload).unwrap());
    } else {
        if !response_text.is_empty() {
            println!("{response_text}");
        }
        eprintln!();
        eprintln!("--- result ---");
        eprintln!("stopped_reason: {:?}", result.stopped_reason);
        eprintln!("iterations:     {}", result.iterations);
        if let Some(verdict) = &result.task_verdict {
            eprintln!("task_verdict:   {:?}", verdict.pattern);
            if let Some(r) = &verdict.verifier_result {
                eprintln!("rationale:      {}", r.rationale);
            }
        }
    }

    match result.stopped_reason {
        StoppedReason::PlanDoneVerified | StoppedReason::PlanDoneNoVerifier => ExitCode::SUCCESS,
        StoppedReason::PlanDoneVerifierRejected => ExitCode::from(1),
        StoppedReason::ProviderError(_) => ExitCode::from(2),
        _ => ExitCode::from(1),
    }
}

fn run_repl<C, T>(
    rt: &mut aegis_agent::ConversationRuntime<C, T>,
    verbose: bool,
) -> ExitCode
where
    C: aegis_agent::api::ApiClient + aegis_agent::api::ConfigurableModel,
    T: aegis_agent::tool::ToolExecutor,
{
    use aegis_agent::StoppedReason;
    use crossterm::style::Stylize;
    use std::io::Write;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    let renderer = render::TerminalRenderer::new();
    let theme = *renderer.color_theme();

    // Auto-save target — fixed for the life of this REPL session,
    // so `--resume latest` next invocation reliably picks it up.
    let session_path = session_store::fresh_session_path();
    eprintln!(
        "aegis chat: auto-saving to {} (use --resume latest to continue next time)",
        session_path.display()
    );

    let banner_title = format!("{}", "aegis chat".with(theme.aegis_brand).bold());
    println!();
    println!("  ┌─ {} ─ V3 interactive mode", banner_title);
    println!(
        "  │  type your message; commands: {}",
        "/exit  /help  /cost  /history  /reset"
            .with(theme.quote)
    );
    println!("  └─");
    println!();

    let slash_commands = vec![
        "exit".into(),
        "quit".into(),
        "help".into(),
        "reset".into(),
        "cost".into(),
        "history".into(),
    ];
    let mut input = match input::ChatInput::new(slash_commands) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("aegis chat: failed to init readline: {e}");
            return ExitCode::from(2);
        }
    };

    let prompt_label = format!("{} ", "you>".with(theme.aegis_brand).bold());
    let mut last_exit = ExitCode::SUCCESS;

    loop {
        let trimmed = match input.read_line(&prompt_label) {
            input::ReadOutcome::Submit(line) => line.trim().to_string(),
            input::ReadOutcome::Cancel => {
                // Ctrl+C abandons the in-progress line; loop continues.
                println!();
                continue;
            }
            input::ReadOutcome::Exit => {
                println!();
                return last_exit;
            }
        };
        if trimmed.is_empty() {
            continue;
        }

        match trimmed.as_str() {
            "/exit" | "/quit" => {
                println!("{}", "bye".with(theme.quote));
                return last_exit;
            }
            "/help" => {
                println!("  /exit, /quit  — leave the session");
                println!("  /reset        — clear conversation history");
                println!("  /cost         — current cost-tracker snapshot");
                println!("  /history      — message count");
                println!("  /model [name] — show or switch the model (alias-resolved)");
                println!("  /plan         — toggle plan mode (writes dry-run; reads execute)");
                println!("  /compact      — replace early messages with a structured summary");
                println!("  /scan         — Ring 0 + Ring 0.5 + cycle detection across the workspace");
                println!("  /sessions     — list saved sessions (newest first)");
                println!("  /save [path]  — save session (default: auto-save target)");
                println!("  /load <path>  — load + replace current session");
                println!("  /help         — this list");
                continue;
            }
            "/sessions" => {
                let listing = session_store::list_sessions();
                if listing.is_empty() {
                    println!(
                        "{}",
                        format!(
                            "(no saved sessions in {})",
                            session_store::sessions_dir().display()
                        )
                        .with(theme.quote)
                    );
                } else {
                    let now = std::time::SystemTime::now();
                    println!("  {} saved sessions (newest first):", listing.len());
                    for (i, m) in listing.iter().take(20).enumerate() {
                        let age = now
                            .duration_since(m.modified)
                            .map(|d| d.as_secs())
                            .unwrap_or(0);
                        println!(
                            "  {:>2}. {}  ({}, {} bytes)",
                            i + 1,
                            m.path
                                .file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or("?"),
                            human_age(age),
                            m.size_bytes,
                        );
                    }
                    if listing.len() > 20 {
                        println!("  ... and {} more (showing newest 20)", listing.len() - 20);
                    }
                }
                continue;
            }
            cmd if cmd == "/model" || cmd.starts_with("/model ") => {
                let arg = trimmed.strip_prefix("/model").unwrap_or("").trim();
                if arg.is_empty() {
                    println!("  current model: {}", rt.api_client_mut().current_model());
                    println!("  usage: /model <name|alias>");
                    println!("  aliases: opus / sonnet / haiku / flash / pro / 4o / 4o-mini / mini");
                } else {
                    let canonical = aegis_agent::providers::resolve_alias(arg);
                    let unknown =
                        aegis_agent::providers::metadata_for(canonical).is_none();
                    rt.api_client_mut().set_model(canonical.to_string());
                    if unknown {
                        println!(
                            "  {} switched to {} (not in registry — preflight will use \
                             the conservative 32k-window default)",
                            "↳".with(theme.spinner_active),
                            canonical
                        );
                    } else {
                        println!(
                            "  {} switched to {}",
                            "↳".with(theme.spinner_active),
                            canonical
                        );
                    }
                }
                continue;
            }
            "/compact" => {
                use aegis_agent::compact::CompactionConfig;
                let result = rt.compact(&[], &CompactionConfig::default());
                match result {
                    Some(r) => {
                        println!(
                            "  {} compacted {} message(s) into a structured summary",
                            "↳".with(theme.spinner_active),
                            r.messages_compacted
                        );
                    }
                    None => {
                        println!(
                            "{}",
                            "(session too short to compact — keeps the last 5 turns intact)"
                                .with(theme.quote)
                        );
                    }
                }
                continue;
            }
            "/plan" => {
                use aegis_agent::permission::{PermissionMode, PermissionPolicy};
                let now = rt.permission_policy().map(|p| p.mode());
                let next = match now {
                    Some(PermissionMode::Plan) => PermissionMode::WorkspaceWrite,
                    _ => PermissionMode::Plan,
                };
                rt.set_permission_policy(Some(PermissionPolicy::standard(next)));
                let label = match next {
                    PermissionMode::Plan => "plan (writes dry-run, reads execute)",
                    PermissionMode::WorkspaceWrite => "workspace-write (writes execute)",
                    PermissionMode::ReadOnly => "read-only",
                    PermissionMode::DangerFullAccess => "danger-full-access",
                };
                println!(
                    "  {} permission mode → {label}",
                    "↳".with(theme.spinner_active)
                );
                continue;
            }
            "/reset" => {
                rt.reset_session();
                println!(
                    "{}",
                    "(session reset — conversation history + cost + stalemate cleared)"
                        .with(theme.quote)
                );
                continue;
            }
            "/cost" => {
                let snap = rt.cost_tracker().snapshot();
                if snap.is_empty() {
                    println!("{}", "(no cost observations recorded)".with(theme.quote));
                } else {
                    for entry in &snap {
                        println!(
                            "  {}  baseline={:.2}  current={:.2}  regression={:.2}",
                            entry.path.display(),
                            entry.baseline,
                            entry.current,
                            entry.regression()
                        );
                    }
                    println!(
                        "  cumulative regression = {:.2}",
                        rt.cost_tracker().cumulative_regression()
                    );
                }
                continue;
            }
            "/history" => {
                println!(
                    "  {} messages in session",
                    rt.session().messages.len()
                );
                continue;
            }
            "/scan" => {
                use aegis_core::scan::{scan_workspace, ScanOptions};
                // Use the workspace the runtime was configured with;
                // fall back to "." if none.
                let ws = std::env::current_dir().unwrap_or_else(|_| ".".into());
                eprintln!("  scanning {} ...", ws.display());
                let report = scan_workspace(&ws, &ScanOptions::default());
                println!(
                    "  {} files / cost {:.0} / {} cycles / {} syntax-err  ({} ms, {}h/{}m cache)",
                    report.files_scanned,
                    report.total_cost,
                    report.cyclic_dependencies.len(),
                    report.files_with_syntax_errors,
                    report.duration_ms,
                    report.cache_stats.hits,
                    report.cache_stats.misses,
                );
                if !report.cyclic_dependencies.is_empty() {
                    println!("  {} import cycles:", report.cyclic_dependencies.len());
                    for cycle in &report.cyclic_dependencies {
                        let parts: Vec<String> =
                            cycle.iter().map(|p| p.display().to_string()).collect();
                        println!("    - {}", parts.join(" → "));
                    }
                }
                let n = 5.min(report.files.len());
                if n > 0 {
                    println!("  top {n} by cost:");
                    for f in report.top_n_by_cost(n) {
                        println!("    {:>5.0}  {}", f.cost, f.relative_path.display());
                    }
                }
                continue;
            }
            cmd if cmd == "/save" || cmd.starts_with("/save ") => {
                let target = cmd
                    .strip_prefix("/save")
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                    .map(std::path::PathBuf::from)
                    .unwrap_or_else(|| session_path.clone());
                match session_store::save(rt.session(), &target) {
                    Ok(()) => println!(
                        "  saved {} messages to {}",
                        rt.session().messages.len(),
                        target.display()
                    ),
                    Err(e) => println!(
                        "  {}: {e}",
                        "save failed".with(theme.spinner_failed)
                    ),
                }
                continue;
            }
            cmd if cmd.starts_with("/load ") => {
                let arg = cmd.strip_prefix("/load ").unwrap_or("").trim();
                if arg.is_empty() {
                    println!("  /load needs a path argument");
                    continue;
                }
                match session_store::resolve_resume(arg)
                    .and_then(|p| session_store::load(&p).map_err(|e| e.to_string()))
                {
                    Ok(loaded) => {
                        let count = loaded.messages.len();
                        rt.replace_session(loaded);
                        println!("  loaded {count} messages");
                    }
                    Err(e) => println!(
                        "  {}: {e}",
                        "load failed".with(theme.spinner_failed)
                    ),
                }
                continue;
            }
            _ if trimmed.starts_with('/') => {
                println!(
                    "  {}  (try /help)",
                    format!("unknown command: {trimmed}").with(theme.spinner_failed)
                );
                continue;
            }
            _ => {}
        }

        // V3.8 — subscribe to streaming so text appears as the
        // model emits it. We accumulate the streamed text into a
        // buffer; once the turn completes, re-render the buffer as
        // markdown for the final formatted block.
        let prefix = format!("{}", "aegis>".with(theme.aegis_brand).bold());
        println!("{prefix}");
        print!("  ");
        let _ = std::io::stdout().flush();

        let streamed_buf: Arc<std::sync::Mutex<String>> =
            Arc::new(std::sync::Mutex::new(String::new()));
        let streamed_buf_for_cb = streamed_buf.clone();
        let saw_stream_text: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));
        let saw_stream_text_cb = saw_stream_text.clone();
        let verbose_cb = verbose;
        let callback: Box<dyn FnMut(&aegis_agent::api::AssistantEvent) + Send> =
            Box::new(move |ev: &aegis_agent::api::AssistantEvent| match ev {
                aegis_agent::api::AssistantEvent::TextDelta(text) => {
                    saw_stream_text_cb.store(true, Ordering::Relaxed);
                    print!("{text}");
                    let _ = std::io::stdout().flush();
                    if let Ok(mut buf) = streamed_buf_for_cb.lock() {
                        buf.push_str(text);
                    }
                }
                aegis_agent::api::AssistantEvent::ToolUse { name, input, .. } => {
                    let dim = crossterm::style::Color::DarkGrey;
                    if verbose_cb {
                        // Truncate to keep the line readable; users
                        // can re-run with /history to see full
                        // payloads.
                        let mut snippet = input.clone();
                        if snippet.len() > 200 {
                            snippet.truncate(200);
                            snippet.push_str(" …");
                        }
                        eprintln!(
                            "\n  {} {name} {}",
                            "↳ tool_use:".with(dim),
                            snippet.with(dim)
                        );
                    } else {
                        eprintln!("\n  {} {name}", "↳ tool_use:".with(dim));
                    }
                }
                aegis_agent::api::AssistantEvent::MessageStop => {}
            });

        // Hand the callback to the runtime for this turn only. We
        // re-build the runtime each turn? No — runtime persists, but
        // the callback is tied to per-turn rendering. Use the
        // builder method which replaces the field.
        // (set + run + clear pattern)
        rt.set_event_callback(Some(callback));
        let result = rt.run_turn(trimmed);
        rt.set_event_callback(None);

        // If streaming actually delivered text, end the line we've
        // been writing into. Otherwise (Anthropic / Gemini today
        // don't truly stream — they replay the full vec at the end),
        // pull the final assistant text from the session and render
        // it as markdown.
        if saw_stream_text.load(Ordering::Relaxed) {
            // Replace the raw streamed line with a markdown-rendered
            // version for prettier final display.
            println!();
            let final_text = streamed_buf.lock().map(|s| s.clone()).unwrap_or_default();
            // Move cursor up + clear lines covering the streamed
            // raw text, then re-render with markdown styling. This
            // keeps the streaming UX (text appears live) AND the
            // markdown formatting (final block looks polished).
            let raw_lines = (final_text.matches('\n').count() + 1) as u16;
            use crossterm::cursor::MoveUp;
            use crossterm::execute;
            use crossterm::terminal::{Clear, ClearType};
            let mut out = std::io::stdout();
            let _ = execute!(out, MoveUp(raw_lines + 1), Clear(ClearType::FromCursorDown));
            println!("{prefix}");
            let rendered = renderer.render_markdown(&final_text);
            for line in rendered.lines() {
                println!("  {line}");
            }
            println!();
        } else {
            // No streamed text — fall back to pulling final text from
            // the session and markdown-rendering it.
            let response_text = collect_last_assistant_text(rt);
            // Erase the empty `aegis>\n  ` we printed pre-stream.
            use crossterm::cursor::MoveUp;
            use crossterm::execute;
            use crossterm::terminal::{Clear, ClearType};
            let mut out = std::io::stdout();
            let _ = execute!(out, MoveUp(2), Clear(ClearType::FromCursorDown));
            if !response_text.is_empty() {
                println!("{prefix}");
                let rendered = renderer.render_markdown(&response_text);
                for line in rendered.lines() {
                    println!("  {line}");
                }
                println!();
            }
        }

        // Status footer — quiet on the boring path.
        match &result.stopped_reason {
            StoppedReason::PlanDoneNoVerifier => {}
            StoppedReason::PlanDoneVerified => {
                eprintln!(
                    "  {} ({} iterations)",
                    "verified".with(theme.spinner_done),
                    result.iterations
                );
            }
            StoppedReason::PlanDoneVerifierRejected => {
                eprintln!(
                    "  {} — see verdict",
                    "verifier rejected".with(theme.spinner_failed)
                );
                if let Some(v) = &result.task_verdict {
                    if let Some(r) = &v.verifier_result {
                        eprintln!("  rationale: {}", r.rationale);
                    }
                }
                last_exit = ExitCode::from(1);
            }
            StoppedReason::ProviderError(message) => {
                eprintln!(
                    "  {}: {}",
                    "provider error".with(theme.spinner_failed),
                    message
                );
                last_exit = ExitCode::from(2);
            }
            other => {
                eprintln!("  ({:?}, {} iterations)", other, result.iterations);
            }
        }

        if verbose {
            // Per-turn cost delta — what aegis attributed to the
            // tool calls in this turn. Only useful in verbose mode;
            // /cost gives the same data on demand.
            let cumulative = rt.cost_tracker().cumulative_regression();
            eprintln!(
                "  {} cumulative cost regression = {:.2}",
                "↳".with(crossterm::style::Color::DarkGrey),
                cumulative
            );
        }

        // Auto-save the session after every turn so a crash / Ctrl-C
        // never loses the transcript. Failures are non-fatal — print
        // a diagnostic but keep the REPL alive.
        if let Err(e) = session_store::save(rt.session(), &session_path) {
            eprintln!(
                "  {}: {e}",
                format!("auto-save to {} failed", session_path.display())
                    .with(theme.spinner_failed)
            );
        }
    }
}

fn collect_last_assistant_text<C, T>(rt: &aegis_agent::ConversationRuntime<C, T>) -> String
where
    C: aegis_agent::api::ApiClient,
    T: aegis_agent::tool::ToolExecutor,
{
    rt.session()
        .messages
        .iter()
        .rev()
        .find(|m| m.role == aegis_agent::MessageRole::Assistant)
        .map(|msg| {
            msg.blocks
                .iter()
                .filter_map(|b| match b {
                    aegis_agent::ContentBlock::Text { text } => Some(text.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default()
}

fn cmd_scan(
    workspace: PathBuf,
    top: usize,
    no_cache: bool,
    no_cycles: bool,
    verify: bool,
    json: bool,
) -> ExitCode {
    use aegis_core::scan::{scan_workspace, ScanOptions};

    let opts = ScanOptions {
        use_cache: !no_cache,
        detect_cycles: !no_cycles,
        ..ScanOptions::default()
    };

    let report = scan_workspace(&workspace, &opts);

    // Optional verifier — runs after the scan, results appended to
    // the report. aegis-cli already depends on aegis-agent so we
    // can use the auto-detect TestVerifier directly.
    let verifier_result = if verify {
        use aegis_agent::verifier::{AgentTaskVerifier, TestVerifier};
        match TestVerifier::auto_detect_composite(&workspace) {
            Some(v) => Some(v.verify(&workspace)),
            None => {
                eprintln!(
                    "aegis scan: --verify requested but no project marker found in {} \
                     (Cargo.toml / pyproject.toml / package.json / go.mod). Skipping.",
                    workspace.display()
                );
                None
            }
        }
    } else {
        None
    };

    if json {
        let payload = serde_json::json!({
            "report": report,
            "verifier": verifier_result.as_ref().map(|r| serde_json::json!({
                "passed": r.passed,
                "rationale": r.rationale,
            })),
        });
        println!("{}", serde_json::to_string_pretty(&payload).unwrap());
    } else {
        print_scan_report_human(&report, top, verifier_result.as_ref());
    }

    let scan_clean = report.files_with_syntax_errors == 0
        && report.cyclic_dependencies.is_empty();
    let verify_clean = verifier_result.as_ref().map(|r| r.passed).unwrap_or(true);
    if scan_clean && verify_clean {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn print_scan_report_human(
    report: &aegis_core::scan::ScanReport,
    top: usize,
    verifier: Option<&aegis_decision::VerifierResult>,
) {
    println!();
    println!("aegis scan — {}", report.root.display());
    println!("  files scanned        : {}", report.files_scanned);
    if report.files_skipped_io_error > 0 {
        println!(
            "  files skipped (IO)   : {}",
            report.files_skipped_io_error
        );
    }
    if report.truncated_count > 0 {
        println!(
            "  files truncated cap  : {} (raise --max-files if you need them all)",
            report.truncated_count
        );
    }
    println!("  total cost           : {:.0}", report.total_cost);
    println!(
        "  files with syntax err: {}",
        report.files_with_syntax_errors
    );
    println!(
        "  import graph         : {} nodes / {} edges / {} cycles",
        report.import_graph.nodes,
        report.import_graph.edges,
        report.import_graph.cycle_count
    );
    println!(
        "  cache                : {} hits / {} misses  (took {} ms)",
        report.cache_stats.hits, report.cache_stats.misses, report.duration_ms
    );

    if !report.cyclic_dependencies.is_empty() {
        println!();
        println!("== import cycles ==");
        for (i, cycle) in report.cyclic_dependencies.iter().enumerate() {
            print!("  {}.", i + 1);
            for (j, p) in cycle.iter().enumerate() {
                if j > 0 {
                    print!(" → ");
                }
                print!(" {}", p.display());
            }
            println!();
        }
    }

    let viol = report.syntax_violations();
    if !viol.is_empty() {
        println!();
        println!("== syntax violations ==");
        for f in &viol {
            for v in &f.syntax_violations {
                println!("  {}: {}", f.relative_path.display(), v);
            }
        }
    }

    if !report.files.is_empty() {
        let n = top.min(report.files.len());
        if n > 0 {
            println!();
            println!("== top {} by cost ==", n);
            for f in report.top_n_by_cost(n) {
                let signal_summary: Vec<String> = f
                    .signals
                    .iter()
                    .filter(|(_, v)| *v > 0.0)
                    .map(|(name, value)| format!("{name}={value:.0}"))
                    .collect();
                println!(
                    "  {:>5.0}  {}  {}",
                    f.cost,
                    f.relative_path.display(),
                    signal_summary.join(" ")
                );
            }
        }
    }

    if let Some(r) = verifier {
        println!();
        println!("== verifier ==");
        println!(
            "  {} — {}",
            if r.passed { "PASSED" } else { "FAILED" },
            r.rationale
        );
    }
}

fn cmd_check(files: &[PathBuf], json: bool) -> ExitCode {
    let mut all_results = Vec::with_capacity(files.len());
    let mut had_error = false;
    for file in files {
        let path_str = file.to_string_lossy().into_owned();
        match extract_signals_native(&path_str) {
            Ok(signals) => {
                all_results.push((path_str, Ok(signals)));
            }
            Err(e) => {
                all_results.push((path_str, Err(e)));
                had_error = true;
            }
        }
    }

    if json {
        let mut arr = Vec::new();
        for (path, result) in &all_results {
            let entry = match result {
                Ok(signals) => serde_json::json!({
                    "path": path,
                    "ok": true,
                    "signals": signals
                        .iter()
                        .map(|s| serde_json::json!({
                            "name": s.name,
                            "value": s.value,
                            "description": s.description,
                        }))
                        .collect::<Vec<_>>(),
                }),
                Err(e) => serde_json::json!({
                    "path": path,
                    "ok": false,
                    "error": e,
                }),
            };
            arr.push(entry);
        }
        println!("{}", serde_json::to_string_pretty(&arr).unwrap());
    } else {
        for (path, result) in &all_results {
            println!("== {path} ==");
            match result {
                Ok(signals) if signals.is_empty() => {
                    println!("  (no signals)");
                }
                Ok(signals) => {
                    for s in signals {
                        println!("  {} = {:.0}  ({})", s.name, s.value, s.description);
                    }
                }
                Err(e) => {
                    println!("  ! error: {e}");
                }
            }
        }
    }

    if had_error {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

fn cmd_pipeline_run(
    task: &str,
    root: &std::path::Path,
    scope: Option<Vec<String>>,
    max_iters: u32,
    include_snippets: bool,
    quiet: bool,
    json: bool,
) -> ExitCode {
    let provider = match build_provider_from_env() {
        Ok(p) => p,
        Err(msg) => {
            eprintln!("provider config error: {msg}");
            return ExitCode::from(2);
        }
    };

    let planner = LLMPlanner::new(&provider);
    let ctx_builder = WorkspaceContextBuilder;
    let opts = PipelineOptions {
        max_iters,
        include_file_snippets: include_snippets,
    };

    let result = run_pipeline(
        task,
        root,
        scope.as_deref(),
        &planner,
        &ctx_builder,
        &opts,
        |ev| {
            if !quiet {
                print_iteration_event(ev);
            }
        },
    );

    if json {
        let summary = serde_json::json!({
            "success": result.success,
            "iterations": result.iterations,
            "error": result.error,
            "final_plan_done": result.final_plan.as_ref().map(|p| p.done),
            "final_plan_patches": result.final_plan.as_ref().map(|p| p.patches.len()),
            "execution_success": result.execution_result.as_ref().map(|r| r.success),
            "rolled_back": result.execution_result.as_ref().map(|r| r.rolled_back),
            "touched_paths": result.execution_result
                .as_ref()
                .map(|r| r.touched_paths.clone())
                .unwrap_or_default(),
        });
        println!("{}", serde_json::to_string_pretty(&summary).unwrap());
    } else {
        println!();
        println!("== pipeline result ==");
        println!("success     : {}", result.success);
        println!("iterations  : {}", result.iterations);
        if let Some(err) = &result.error {
            println!("error       : {err}");
        }
        if let Some(plan) = &result.final_plan {
            println!("final goal  : {}", plan.goal);
            println!("final done  : {}", plan.done);
            println!("patches     : {}", plan.patches.len());
        }
        if let Some(exec) = &result.execution_result {
            println!("touched     : {} paths", exec.touched_paths.len());
            for p in &exec.touched_paths {
                println!("  - {p}");
            }
        }
    }

    if result.success {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn build_provider_from_env() -> Result<OpenAIChatProvider, String> {
    let kind = std::env::var("AEGIS_PROVIDER").unwrap_or_else(|_| "openai".to_string());
    let model = std::env::var("AEGIS_MODEL").unwrap_or_else(|_| match kind.as_str() {
        "groq" => "llama-3.3-70b-versatile".to_string(),
        "openrouter" => "openai/gpt-4o-mini".to_string(),
        _ => "gpt-4o-mini".to_string(),
    });

    // Pick the right env var for the API key.
    let key_env = match kind.as_str() {
        "openai" => "OPENAI_API_KEY",
        "openrouter" => "OPENROUTER_API_KEY",
        "groq" => "GROQ_API_KEY",
        other => return Err(format!("unknown AEGIS_PROVIDER={other}")),
    };
    let api_key = std::env::var("AEGIS_API_KEY")
        .or_else(|_| std::env::var(key_env))
        .map_err(|_| {
            format!("missing API key — set AEGIS_API_KEY or {key_env}")
        })?;

    let mut config = OpenAIChatProviderConfig::new(model, api_key);
    config = match kind.as_str() {
        "openai" => config.with_display_name("openai"),
        "openrouter" => config
            .with_base_url("https://openrouter.ai/api/v1")
            .with_display_name("openrouter"),
        "groq" => config
            .with_base_url("https://api.groq.com/openai/v1")
            .with_display_name("groq"),
        _ => unreachable!("kind validated above"),
    };
    Ok(OpenAIChatProvider::new(config))
}

fn print_iteration_event(ev: &aegis_decision::IterationEvent) {
    println!(
        "iter {} [{}] plan={} patches={} applied={} rolled_back={}{}{}",
        ev.iteration,
        ev.plan_id,
        if ev.plan_done { "DONE" } else { "continuing" },
        ev.plan_patches,
        ev.applied,
        ev.rolled_back,
        if ev.regressed { " regressed" } else { "" },
        if ev.stalemate_detected {
            " STALEMATE"
        } else if ev.thrashing_detected {
            " THRASHING"
        } else {
            ""
        }
    );
    if !ev.validation_errors.is_empty() {
        for e in &ev.validation_errors {
            println!("    err: {e}");
        }
    }
    if !ev.regression_detail.is_empty() {
        for (k, v) in &ev.regression_detail {
            println!("    regression: {k} +{v:.4}");
        }
    }
}

fn cmd_languages(json: bool) -> ExitCode {
    let registry = LanguageRegistry::global();
    let names = registry.names();
    let extensions = registry.extensions();
    if json {
        let entry = serde_json::json!({
            "languages": names,
            "extensions": extensions,
        });
        println!("{}", serde_json::to_string_pretty(&entry).unwrap());
    } else {
        println!("# Supported languages");
        for name in &names {
            println!("- {name}");
        }
        println!("\n# File extensions");
        for ext in &extensions {
            println!("- {ext}");
        }
    }
    ExitCode::SUCCESS
}
