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
