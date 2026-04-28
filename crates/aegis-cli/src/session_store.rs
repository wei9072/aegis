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

/// One row of the `/sessions` listing.
#[derive(Debug, Clone)]
pub struct SessionMeta {
    pub path: PathBuf,
    pub modified: std::time::SystemTime,
    pub size_bytes: u64,
}

/// All saved sessions, newest first by mtime. Returns `Vec` (not
/// `Result`) — a missing or unreadable sessions dir = empty list.
/// Cheap (`stat` only, no JSON parse), so safe to call from REPL.
#[must_use]
pub fn list_sessions() -> Vec<SessionMeta> {
    let dir = sessions_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out: Vec<SessionMeta> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                return None;
            }
            let meta = entry.metadata().ok()?;
            Some(SessionMeta {
                path,
                modified: meta.modified().ok()?,
                size_bytes: meta.len(),
            })
        })
        .collect();
    out.sort_by(|a, b| b.modified.cmp(&a.modified));
    out
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

    /// Both `list_sessions` tests set `XDG_DATA_HOME`, which is process-
    /// wide. Running them in parallel races (one removes the var while
    /// the other reads). Combining into one test serialises the env-var
    /// usage. (Could split with a `Mutex` static, but a single test is
    /// simpler and keeps assertion granularity.)
    #[test]
    fn list_sessions_empty_dir_then_three_files_sorts_newest_first() {
        // Phase 1 — empty dir → empty Vec, no panic.
        let td = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_DATA_HOME", td.path());
        assert!(list_sessions().is_empty(), "empty dir should yield empty Vec");

        // Phase 2 — write three sessions + one non-json, expect newest first.
        let sess_dir = td.path().join("aegis").join("sessions");
        std::fs::create_dir_all(&sess_dir).unwrap();
        for stem in ["a", "b", "c"] {
            let p = sess_dir.join(format!("{stem}.json"));
            Session::new().save_to(&p).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        std::fs::write(sess_dir.join("note.txt"), "x").unwrap();

        let listing = list_sessions();
        std::env::remove_var("XDG_DATA_HOME");

        assert_eq!(listing.len(), 3, "non-json file must be skipped");
        // Newest first → c, b, a
        assert!(listing[0].path.ends_with("c.json"));
        assert!(listing[2].path.ends_with("a.json"));
    }
}
