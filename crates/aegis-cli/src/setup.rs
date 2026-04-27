//! `aegis setup` — interactive wizard that writes
//! `~/.config/aegis/config.toml` so the user doesn't have to memorise
//! a half-dozen env vars to make `aegis chat` go.
//!
//! Strict scope: walks the user through provider choice, base URL
//! preset, model name, and API-key env-var name; writes the TOML.
//! It does NOT export env vars to the live shell — the wizard tells
//! the user to run `export OPENROUTER_API_KEY=...` themselves. (The
//! aegis runtime can't write to the parent shell's env even if we
//! wanted to.)

use std::io::{self, BufRead, Write};
use std::path::Path;

use crate::config;

/// Top-level entry point. Returns `Ok(())` on success or a string
/// error suitable for printing.
pub fn run() -> Result<(), String> {
    let path = config::default_path()
        .ok_or_else(|| "neither $XDG_CONFIG_HOME nor $HOME is set; can't pick a config path".to_string())?;

    println!("aegis setup — write {}", path.display());
    println!();

    if path.exists() {
        if !confirm(
            &format!("config already exists at {}. overwrite?", path.display()),
            false,
        )? {
            println!("aborted.");
            return Ok(());
        }
        println!();
    }

    let provider = pick_provider()?;
    println!();

    let body = match provider {
        Provider::OpenAiCompat => build_openai_section()?,
        Provider::Anthropic => build_anthropic_section()?,
        Provider::Gemini => build_gemini_section()?,
    };
    println!();

    write_config(&path, &body)?;
    println!("wrote {}", path.display());
    println!();
    print_next_steps(provider);

    Ok(())
}

#[derive(Clone, Copy)]
enum Provider {
    OpenAiCompat,
    Anthropic,
    Gemini,
}

fn pick_provider() -> Result<Provider, String> {
    println!("? pick provider:");
    println!("  1) OpenAI-compatible (OpenRouter / Groq / Ollama / vLLM / OpenAI)");
    println!("  2) Anthropic (Claude direct)");
    println!("  3) Gemini (Google direct)");
    let n = ask_number("> ", 1, 3)?;
    Ok(match n {
        1 => Provider::OpenAiCompat,
        2 => Provider::Anthropic,
        3 => Provider::Gemini,
        _ => unreachable!(),
    })
}

fn build_openai_section() -> Result<String, String> {
    println!("? OpenAI-compatible preset:");
    println!("  1) OpenRouter   (https://openrouter.ai/api/v1)");
    println!("  2) Groq         (https://api.groq.com/openai/v1)");
    println!("  3) OpenAI       (https://api.openai.com/v1)");
    println!("  4) Ollama       (http://127.0.0.1:11434/v1) — local, no API key");
    println!("  5) custom base_url");
    let n = ask_number("> ", 1, 5)?;

    let (base_url, default_key_env, hint) = match n {
        1 => (
            "https://openrouter.ai/api/v1".to_string(),
            "OPENROUTER_API_KEY",
            "model name format: 'vendor/model', e.g. anthropic/claude-haiku-4.5",
        ),
        2 => (
            "https://api.groq.com/openai/v1".to_string(),
            "GROQ_API_KEY",
            "model name e.g. llama-3.3-70b-versatile",
        ),
        3 => (
            "https://api.openai.com/v1".to_string(),
            "OPENAI_API_KEY",
            "model name e.g. gpt-4o, gpt-4o-mini",
        ),
        4 => (
            "http://127.0.0.1:11434/v1".to_string(),
            "",
            "ensure `ollama serve` is running. model name e.g. llama3.2",
        ),
        5 => (
            ask("? custom base_url: ")?,
            "",
            "any OpenAI-compatible endpoint",
        ),
        _ => unreachable!(),
    };

    println!();
    println!("{hint}");
    let model = ask("? model: ")?;

    let api_key_line = if base_url.starts_with("http://127.0.0.1") {
        // Local backend, no auth.
        String::new()
    } else {
        let env_name = ask_with_default(
            "? env var holding the API key (we read from this; never write the key to disk)",
            default_key_env,
        )?;
        format!("api_key_env = \"{env_name}\"\n")
    };

    Ok(format!(
        "[provider.openai]\nbase_url = \"{}\"\nmodel = \"{}\"\n{}",
        toml_escape(&base_url),
        toml_escape(&model),
        api_key_line,
    ))
}

fn build_anthropic_section() -> Result<String, String> {
    println!("? model (e.g. claude-sonnet-4-6, claude-haiku-4-5): ");
    let model = ask("> ")?;
    let env_name =
        ask_with_default("? env var holding the API key", "ANTHROPIC_API_KEY")?;
    Ok(format!(
        "[provider.anthropic]\nmodel = \"{}\"\napi_key_env = \"{}\"\n",
        toml_escape(&model),
        env_name,
    ))
}

fn build_gemini_section() -> Result<String, String> {
    println!("? model (e.g. gemini-2.5-flash, gemini-2.5-pro): ");
    let model = ask("> ")?;
    let env_name = ask_with_default("? env var holding the API key", "GOOGLE_API_KEY")?;
    Ok(format!(
        "[provider.gemini]\nmodel = \"{}\"\napi_key_env = \"{}\"\n",
        toml_escape(&model),
        env_name,
    ))
}

fn write_config(path: &Path, body: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("create {}: {e}", parent.display()))?;
    }
    let header = "# Generated by `aegis setup`. Edit by hand or rerun the wizard.\n\n";
    std::fs::write(path, format!("{header}{body}"))
        .map_err(|e| format!("write {}: {e}", path.display()))
}

fn print_next_steps(provider: Provider) {
    println!("next steps:");
    match provider {
        Provider::OpenAiCompat => {
            println!("  1. ensure your API key env var is exported in your shell");
            println!("     (e.g. `export OPENROUTER_API_KEY=...` in ~/.bashrc / ~/.zshrc)");
        }
        Provider::Anthropic => {
            println!("  1. `export ANTHROPIC_API_KEY=...` in your shell rc");
        }
        Provider::Gemini => {
            println!("  1. `export GOOGLE_API_KEY=...` in your shell rc");
        }
    }
    println!("  2. run `aegis chat` from any project directory");
    println!();
    println!("aegis core gates (predictor + cost observer + stalemate detector)");
    println!("are ON by default. Add --no-aegis to disable for debugging only.");
}

// ---------- prompt helpers ----------

fn ask(prompt: &str) -> Result<String, String> {
    print!("{prompt}");
    io::stdout().flush().map_err(|e| e.to_string())?;
    let mut line = String::new();
    io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|e| e.to_string())?;
    let trimmed = line.trim().to_string();
    if trimmed.is_empty() {
        return Err("empty input — aborting".into());
    }
    Ok(trimmed)
}

fn ask_with_default(prompt: &str, default: &str) -> Result<String, String> {
    let suffix = if default.is_empty() {
        String::new()
    } else {
        format!(" [{default}]")
    };
    print!("{prompt}{suffix}: ");
    io::stdout().flush().map_err(|e| e.to_string())?;
    let mut line = String::new();
    io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|e| e.to_string())?;
    let trimmed = line.trim();
    if trimmed.is_empty() {
        if default.is_empty() {
            return Err("empty input — aborting".into());
        }
        return Ok(default.to_string());
    }
    Ok(trimmed.to_string())
}

fn ask_number(prompt: &str, min: u32, max: u32) -> Result<u32, String> {
    loop {
        print!("{prompt}");
        io::stdout().flush().map_err(|e| e.to_string())?;
        let mut line = String::new();
        io::stdin()
            .lock()
            .read_line(&mut line)
            .map_err(|e| e.to_string())?;
        match line.trim().parse::<u32>() {
            Ok(n) if n >= min && n <= max => return Ok(n),
            _ => println!("  please enter a number between {min} and {max}"),
        }
    }
}

fn confirm(question: &str, default_yes: bool) -> Result<bool, String> {
    let suffix = if default_yes { " (Y/n) " } else { " (y/N) " };
    print!("? {question}{suffix}");
    io::stdout().flush().map_err(|e| e.to_string())?;
    let mut line = String::new();
    io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|e| e.to_string())?;
    let answer = line.trim().to_ascii_lowercase();
    if answer.is_empty() {
        return Ok(default_yes);
    }
    Ok(matches!(answer.as_str(), "y" | "yes"))
}

fn toml_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toml_escape_handles_quotes_and_backslash() {
        assert_eq!(toml_escape("plain"), "plain");
        assert_eq!(toml_escape(r#"a"b"#), r#"a\"b"#);
        assert_eq!(toml_escape(r"a\b"), r"a\\b");
    }
}
