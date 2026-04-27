//! Conversation message and content block types.
//!
//! Adapted from claw-code (MIT) — `rust/crates/runtime/src/session.rs`.
//! V3.7 adds serde + JSON persistence + a simple message-count
//! compaction helper.

use serde::{Deserialize, Serialize};

/// Speaker role associated with a conversation message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    System,
    User,
    Assistant,
    Tool,
}

/// Structured message content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: String,
    },
    ToolResult {
        tool_use_id: String,
        tool_name: String,
        output: String,
        is_error: bool,
    },
}

/// One conversation message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationMessage {
    pub role: MessageRole,
    pub blocks: Vec<ContentBlock>,
}

impl ConversationMessage {
    #[must_use]
    pub fn user_text(text: impl Into<String>) -> Self {
        Self {
            role: MessageRole::User,
            blocks: vec![ContentBlock::Text { text: text.into() }],
        }
    }

    #[must_use]
    pub fn assistant(blocks: Vec<ContentBlock>) -> Self {
        Self {
            role: MessageRole::Assistant,
            blocks,
        }
    }

    #[must_use]
    pub fn tool_result(
        tool_use_id: impl Into<String>,
        tool_name: impl Into<String>,
        output: impl Into<String>,
        is_error: bool,
    ) -> Self {
        Self {
            role: MessageRole::Tool,
            blocks: vec![ContentBlock::ToolResult {
                tool_use_id: tool_use_id.into(),
                tool_name: tool_name.into(),
                output: output.into(),
                is_error,
            }],
        }
    }
}

/// Persistable session state.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    pub messages: Vec<ConversationMessage>,
    /// V3.7: optional summary of older messages compacted away.
    /// `None` means nothing has been compacted yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compaction_summary: Option<String>,
}

impl Session {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, message: ConversationMessage) {
        self.messages.push(message);
    }

    /// Save to a JSON file. Atomic write via temp file + rename so a
    /// crash mid-write never leaves a half-file.
    pub fn save_to(&self, path: &std::path::Path) -> std::io::Result<()> {
        let json = serde_json::to_string_pretty(self).map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, format!("serialise: {e}"))
        })?;
        let tmp = path.with_extension("session.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Load from a JSON file.
    pub fn load_from(path: &std::path::Path) -> std::io::Result<Self> {
        let body = std::fs::read_to_string(path)?;
        serde_json::from_str(&body).map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, format!("parse: {e}"))
        })
    }

    /// V3.7 minimum-viable compaction: drop the oldest `drop_count`
    /// messages and stash a summary placeholder. Real LLM-driven
    /// summarisation is a follow-up — this lets long sessions still
    /// fit in context by trimming, with the trim recorded so the
    /// next prompt can include the summary as a system message.
    ///
    /// Always preserves the first `keep_recent` messages and drops
    /// older ones beyond that. Returns the number actually dropped.
    pub fn compact_drop_oldest(
        &mut self,
        keep_recent: usize,
        summary: impl Into<String>,
    ) -> usize {
        if self.messages.len() <= keep_recent {
            return 0;
        }
        let drop_count = self.messages.len() - keep_recent;
        self.messages.drain(0..drop_count);
        self.compaction_summary = Some(summary.into());
        drop_count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_and_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.json");

        let mut s = Session::new();
        s.push(ConversationMessage::user_text("hello"));
        s.push(ConversationMessage::assistant(vec![ContentBlock::Text {
            text: "hi".into(),
        }]));
        s.save_to(&path).unwrap();

        let loaded = Session::load_from(&path).unwrap();
        assert_eq!(loaded, s);
    }

    #[test]
    fn compact_drops_oldest_keeps_recent() {
        let mut s = Session::new();
        for i in 0..6 {
            s.push(ConversationMessage::user_text(format!("msg {i}")));
        }
        let dropped = s.compact_drop_oldest(2, "summary of older messages");
        assert_eq!(dropped, 4);
        assert_eq!(s.messages.len(), 2);
        assert!(s.compaction_summary.is_some());
        // The two we kept must be the most recent.
        match &s.messages[0].blocks[0] {
            ContentBlock::Text { text } => assert_eq!(text, "msg 4"),
            _ => panic!("unexpected"),
        }
    }

    #[test]
    fn compact_no_op_when_under_threshold() {
        let mut s = Session::new();
        s.push(ConversationMessage::user_text("only one"));
        let dropped = s.compact_drop_oldest(5, "n/a");
        assert_eq!(dropped, 0);
        assert!(s.compaction_summary.is_none());
    }
}
