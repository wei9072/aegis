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
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Check { files, json } => cmd_check(&files, json),
        Command::Languages { json } => cmd_languages(json),
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
