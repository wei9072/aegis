//! Input layer for `aegis chat` REPL.
//!
//! Adapted from claw-code (MIT) —
//! `rust/crates/rusty-claude-cli/src/input.rs`. Simplified: keep
//! rustyline editor + slash-command tab-completion + history; drop
//! claw-code's more elaborate hint / multiline / paste handling
//! (added back when a real consumer wants them).

use std::borrow::Cow;
use std::cell::RefCell;

use rustyline::completion::{Completer, Pair};
use rustyline::error::ReadlineError;
use rustyline::highlight::{CmdKind, Highlighter};
use rustyline::hint::Hinter;
use rustyline::history::DefaultHistory;
use rustyline::validate::Validator;
use rustyline::{CompletionType, Config, Context, EditMode, Editor, Helper};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadOutcome {
    Submit(String),
    Cancel, // Ctrl+C
    Exit,   // Ctrl+D / EOF
}

/// Slash-command completer backing the `aegis chat` editor.
pub struct SlashCommandHelper {
    completions: Vec<String>,
    current_line: RefCell<String>,
}

impl SlashCommandHelper {
    pub fn new(completions: Vec<String>) -> Self {
        Self {
            completions: normalize(completions),
            current_line: RefCell::new(String::new()),
        }
    }
}

impl Completer for SlashCommandHelper {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Self::Candidate>)> {
        let Some(prefix) = slash_prefix(line, pos) else {
            return Ok((0, Vec::new()));
        };
        let matches = self
            .completions
            .iter()
            .filter(|c| c.starts_with(prefix))
            .map(|c| Pair {
                display: c.clone(),
                replacement: c.clone(),
            })
            .collect();
        Ok((0, matches))
    }
}

impl Hinter for SlashCommandHelper {
    type Hint = String;
}

impl Highlighter for SlashCommandHelper {
    fn highlight<'l>(&self, line: &'l str, _pos: usize) -> Cow<'l, str> {
        *self.current_line.borrow_mut() = line.to_string();
        Cow::Borrowed(line)
    }
    fn highlight_char(&self, line: &str, _pos: usize, _kind: CmdKind) -> bool {
        *self.current_line.borrow_mut() = line.to_string();
        false
    }
}

impl Validator for SlashCommandHelper {}
impl Helper for SlashCommandHelper {}

fn normalize(commands: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    for c in commands {
        let trimmed = c.trim().to_string();
        if trimmed.is_empty() {
            continue;
        }
        let normalized = if trimmed.starts_with('/') {
            trimmed
        } else {
            format!("/{trimmed}")
        };
        if seen.insert(normalized.clone()) {
            out.push(normalized);
        }
    }
    out
}

fn slash_prefix(line: &str, pos: usize) -> Option<&str> {
    let prefix = &line[..pos];
    if !prefix.starts_with('/') {
        return None;
    }
    if prefix.contains(' ') {
        return None; // already moved past the command word
    }
    Some(prefix)
}

/// Wraps a rustyline editor so the chat loop can stay simple.
pub struct ChatInput {
    editor: Editor<SlashCommandHelper, DefaultHistory>,
}

impl ChatInput {
    pub fn new(slash_commands: Vec<String>) -> rustyline::Result<Self> {
        let config = Config::builder()
            .completion_type(CompletionType::List)
            .edit_mode(EditMode::Emacs)
            .auto_add_history(true)
            .build();
        let mut editor: Editor<SlashCommandHelper, DefaultHistory> =
            Editor::with_config(config)?;
        editor.set_helper(Some(SlashCommandHelper::new(slash_commands)));
        Ok(Self { editor })
    }

    /// Read one line. Returns:
    ///   - Submit(line)   on Enter
    ///   - Cancel         on Ctrl+C (the chat loop stays alive; user
    ///                    just abandons the in-progress line)
    ///   - Exit           on Ctrl+D / EOF / unrecoverable IO error
    ///
    /// Multi-line: a trailing backslash at the end of a line marks
    /// the input as continued — the backslash is stripped and the
    /// editor re-prompts (with a `... ` indent) until the user
    /// submits a line that doesn't end in backslash. Lines join
    /// with `\n`, so pasting code blocks is no longer awkward.
    /// Ctrl+C during continuation cancels the whole multi-line
    /// buffer.
    pub fn read_line(&mut self, prompt: &str) -> ReadOutcome {
        const CONT_PROMPT: &str = "  ... ";
        let mut buffer = String::new();
        let mut current_prompt = prompt;

        loop {
            match self.editor.readline(current_prompt) {
                Ok(line) => match line.strip_suffix('\\') {
                    Some(stripped) => {
                        // Continuation line — strip the trailing
                        // backslash and keep going.
                        if !buffer.is_empty() {
                            buffer.push('\n');
                        }
                        buffer.push_str(stripped);
                        current_prompt = CONT_PROMPT;
                    }
                    None => {
                        // Final line — return whatever we have.
                        if buffer.is_empty() {
                            return ReadOutcome::Submit(line);
                        }
                        buffer.push('\n');
                        buffer.push_str(&line);
                        return ReadOutcome::Submit(buffer);
                    }
                },
                Err(ReadlineError::Interrupted) => return ReadOutcome::Cancel,
                Err(ReadlineError::Eof) => return ReadOutcome::Exit,
                Err(_) => return ReadOutcome::Exit,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_adds_leading_slash_when_missing() {
        let n = normalize(vec!["exit".into(), "/help".into()]);
        assert_eq!(n, vec!["/exit", "/help"]);
    }

    #[test]
    fn normalize_dedupes() {
        let n = normalize(vec!["/exit".into(), "/exit".into()]);
        assert_eq!(n, vec!["/exit"]);
    }

    #[test]
    fn slash_prefix_recognises_command_start() {
        assert_eq!(slash_prefix("/he", 3), Some("/he"));
        assert_eq!(slash_prefix("/help arg", 9), None); // moved past space
        assert_eq!(slash_prefix("plain text", 5), None);
    }
}
