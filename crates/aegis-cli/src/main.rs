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

    /// V3 — drive a one-shot conversation with the aegis-agent loop.
    ///
    /// Picks a provider from env vars (first match wins):
    ///   AEGIS_OPENAI_BASE_URL + AEGIS_OPENAI_MODEL  → OpenAI-compat
    ///   AEGIS_ANTHROPIC_API_KEY + AEGIS_ANTHROPIC_MODEL → Anthropic
    ///   AEGIS_GEMINI_API_KEY + AEGIS_GEMINI_MODEL   → Gemini
    ///
    /// Outputs the assistant's response on stdout; the structured
    /// AgentTurnResult (stopped_reason / iterations / verdict) goes
    /// to stderr unless --json is set, in which case the whole
    /// result is one JSON object on stdout.
    Chat {
        /// Prompt to send. If omitted, reads from stdin.
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
        /// Workspace path passed to the verifier.
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
        /// Permission mode: read-only | workspace-write | danger-full-access.
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
    let cli = Cli::parse();
    match cli.command {
        Command::Check { files, json } => cmd_check(&files, json),
        Command::Languages { json } => cmd_languages(json),
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
            json,
        } => cmd_chat(
            prompt,
            system,
            max_iters,
            cost_budget,
            workspace,
            &permission_mode,
            verify,
            verifier_cmd,
            json,
        ),
    }
}

fn cmd_chat(
    prompt: Option<String>,
    system: Option<String>,
    max_iters: u32,
    cost_budget: Option<f64>,
    workspace: PathBuf,
    permission_mode_str: &str,
    verify: bool,
    verifier_cmd: Option<String>,
    json: bool,
) -> ExitCode {
    use aegis_agent::api::ApiClient;
    use aegis_agent::permission::{PermissionMode, PermissionPolicy};
    use aegis_agent::providers::{
        AnthropicConfig, AnthropicProvider, GeminiConfig, GeminiProvider, OpenAiCompatConfig,
        OpenAiCompatProvider, UreqClient,
    };
    use aegis_agent::testing::ScriptedToolExecutor;
    use aegis_agent::verifier::{AgentTaskVerifier, ShellVerifier, TestVerifier};
    use aegis_agent::{AgentConfig, ConversationRuntime, Session, StoppedReason};

    // Resolve prompt — flag → arg → stdin.
    let prompt = match prompt {
        Some(p) if !p.trim().is_empty() => p,
        _ => {
            use std::io::Read;
            let mut buf = String::new();
            if std::io::stdin().read_to_string(&mut buf).is_err() || buf.trim().is_empty() {
                eprintln!("aegis chat: no prompt provided (pass as arg or pipe via stdin)");
                return ExitCode::from(2);
            }
            buf
        }
    };

    // Resolve permission mode.
    let permission_mode = match permission_mode_str {
        "read-only" => PermissionMode::ReadOnly,
        "workspace-write" => PermissionMode::WorkspaceWrite,
        "danger-full-access" => PermissionMode::DangerFullAccess,
        other => {
            eprintln!(
                "aegis chat: unknown --permission-mode {other:?} (allowed: \
                 read-only | workspace-write | danger-full-access)"
            );
            return ExitCode::from(2);
        }
    };

    // Pick a provider from env vars. First match wins.
    let provider: Box<dyn ApiClient> = if let Some(c) = OpenAiCompatConfig::from_env() {
        eprintln!(
            "aegis chat: provider = openai-compat ({} {})",
            c.base_url, c.model
        );
        Box::new(OpenAiCompatProvider::new(c, Box::new(UreqClient::new())))
    } else if let Some(c) = AnthropicConfig::from_env() {
        eprintln!("aegis chat: provider = anthropic ({})", c.model);
        Box::new(AnthropicProvider::new(c, Box::new(UreqClient::new())))
    } else if let Some(c) = GeminiConfig::from_env() {
        eprintln!("aegis chat: provider = gemini ({})", c.model);
        Box::new(GeminiProvider::new(c, Box::new(UreqClient::new())))
    } else {
        eprintln!(
            "aegis chat: no provider env vars set. Set one of:\n  \
             AEGIS_OPENAI_BASE_URL + AEGIS_OPENAI_MODEL (+ AEGIS_OPENAI_API_KEY)\n  \
             AEGIS_ANTHROPIC_API_KEY + AEGIS_ANTHROPIC_MODEL\n  \
             AEGIS_GEMINI_API_KEY + AEGIS_GEMINI_MODEL"
        );
        return ExitCode::from(2);
    };

    // Optional verifier.
    let verifier: Option<Box<dyn AgentTaskVerifier>> = if let Some(cmd) = verifier_cmd {
        // Custom shell command — split on whitespace.
        let mut parts = cmd.split_whitespace();
        match parts.next() {
            Some(program) => Some(Box::new(
                ShellVerifier::new(program).args(parts.map(String::from)),
            )),
            None => None,
        }
    } else if verify {
        // Auto-detect (composite — every test suite must pass).
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

    let system_prompt = system.map(|s| vec![s]).unwrap_or_default();

    let mut rt = ConversationRuntime::new(
        Session::new(),
        provider,
        ScriptedToolExecutor::new(), // CLI demo has no tools wired; real tools land in V3.6+
        system_prompt,
        vec![],
        AgentConfig {
            max_iterations_per_turn: max_iters,
            session_cost_budget: cost_budget,
            workspace_root: Some(workspace),
        },
    )
    .with_permission_policy(PermissionPolicy::standard(permission_mode));

    if let Some(v) = verifier {
        rt = rt.with_verifier(v);
    }

    let result = rt.run_turn(prompt);

    // Collect the assistant's final text response.
    let response_text: String = rt
        .session()
        .messages
        .last()
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
        .unwrap_or_default();

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
