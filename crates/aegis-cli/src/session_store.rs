//! Session persistence for `aegis chat`.
//!
//! Saves each session's transcript to a JSON file under
//! `<data_dir>/aegis/sessions/<timestamp>.json`. `--resume <path>`
//! loads a saved session; `--resume latest` (or no arg with the
//! flag) picks the most recent file.
//!
//! `<data_dir>` resolution:
//!   - Honour `XDG_DATA_HOME` if set (Linux convention)
//!   - Otherwise `$HOME/.local/share` (Linux/macOS fallback)
//!   - Final fallback: temp dir (so the agent still runs in
//!     restricted-environment tests)
//!
//! Auto-save semantics: after every turn the runtime's session is
//! flushed to disk, so a crash/Ctrl-C mid-session preserves the
//! transcript. The auto-save target is the same file that
//! `--resume latest` will pick up next invocation.

use std::path::{Path, PathBuf};

use aegis_agent::message::Session;

/// Directory where sessions live. Created lazily by `auto_save`.
#[must_use]
pub fn sessions_dir() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("aegis").join("sessions");
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        if !home.is_empty() {
            return PathBuf::from(home)
                .join(".local")
                .join("share")
                .join("aegis")
                .join("sessions");
        }
    }
    std::env::temp_dir().join("aegis").join("sessions")
}

/// Build a fresh session filename: `<unix_ms>.json`. Sortable by
/// name → newest is always lexicographically last.
#[must_use]
pub fn fresh_session_path() -> PathBuf {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    sessions_dir().join(format!("{ms}.json"))
}

/// Resolve a `--resume` argument to an actual file path.
/// Accepts:
///   - "latest" — newest file in `sessions_dir()`
///   - any file path — used as-is
pub fn resolve_resume(arg: &str) -> Result<PathBuf, String> {
    if arg == "latest" {
        latest_session()
            .ok_or_else(|| format!("no saved sessions found in {}", sessions_dir().display()))
    } else {
        let p = PathBuf::from(arg);
        if !p.exists() {
            return Err(format!("session file not found: {}", p.display()));
        }
        Ok(p)
    }
}

/// Newest `*.json` under `sessions_dir()`, or `None` if dir is
/// empty / missing.
#[must_use]
pub fn latest_session() -> Option<PathBuf> {
    let dir = sessions_dir();
    let entries = std::fs::read_dir(&dir).ok()?;
    let mut newest: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let mtime = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()?;
        match &newest {
            None => newest = Some((mtime, path)),
            Some((cur, _)) if mtime > *cur => newest = Some((mtime, path)),
            _ => {}
        }
    }
    newest.map(|(_, p)| p)
}

/// Load a session from disk. Bubbles up IO + JSON errors.
pub fn load(path: &Path) -> std::io::Result<Session> {
    Session::load_from(path)
}

/// Atomic save. Creates parent dir if missing. Used both by
/// auto-save (every turn) and `/save` slash command.
pub fn save(session: &Session, path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    session.save_to(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_path_is_in_sessions_dir() {
        let p = fresh_session_path();
        assert!(p.parent().unwrap().ends_with("sessions"));
        assert_eq!(p.extension().and_then(|e| e.to_str()), Some("json"));
    }

    #[test]
    fn save_and_load_roundtrip_via_helpers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.json");
        let mut s = Session::new();
        s.push(aegis_agent::message::ConversationMessage::user_text("hi"));
        save(&s, &path).unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded, s);
    }

    #[test]
    fn resolve_resume_with_explicit_path_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.json");
        Session::new().save_to(&path).unwrap();
        let resolved = resolve_resume(path.to_str().unwrap()).unwrap();
        assert_eq!(resolved, path);
    }

    #[test]
    fn resolve_resume_with_missing_path_errors() {
        let err = resolve_resume("/definitely/not/a/real/file.json").unwrap_err();
        assert!(err.contains("not found"));
    }
}
