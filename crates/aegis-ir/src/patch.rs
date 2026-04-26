//! `PatchPlan` and friends. Pure data; serde-friendly.
//!
//! The two enums (`PatchKind`, `PatchStatus`) serialize as the same
//! lowercase strings the Python `str, Enum` did, so a Rust-encoded
//! plan round-trips through any Python consumer that already knows
//! `"create" / "modify" / "delete"`.

use serde::{Deserialize, Serialize};

/// What kind of operation the patch represents.
///
/// String values match the Python `PatchKind(str, Enum)` exactly so
/// the wire format is identical.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PatchKind {
    Create,
    Modify,
    Delete,
}

impl PatchKind {
    pub fn as_str(self) -> &'static str {
        match self {
            PatchKind::Create => "create",
            PatchKind::Modify => "modify",
            PatchKind::Delete => "delete",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "create" => Some(PatchKind::Create),
            "modify" => Some(PatchKind::Modify),
            "delete" => Some(PatchKind::Delete),
            _ => None,
        }
    }
}

/// What happened when an edit was attempted.
///
/// Same string values as Python `PatchStatus(str, Enum)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PatchStatus {
    Applied,
    AlreadyApplied,
    NotFound,
    Ambiguous,
}

impl PatchStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            PatchStatus::Applied => "applied",
            PatchStatus::AlreadyApplied => "already_applied",
            PatchStatus::NotFound => "not_found",
            PatchStatus::Ambiguous => "ambiguous",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "applied" => Some(PatchStatus::Applied),
            "already_applied" => Some(PatchStatus::AlreadyApplied),
            "not_found" => Some(PatchStatus::NotFound),
            "ambiguous" => Some(PatchStatus::Ambiguous),
            _ => None,
        }
    }
}

/// A single anchored edit. `context_before` / `context_after` are
/// the surrounding text; the matcher prefers raw concat first then
/// falls back to a newline-aware join (see `edit_engine`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edit {
    pub old_string: String,
    pub new_string: String,
    #[serde(default)]
    pub context_before: String,
    #[serde(default)]
    pub context_after: String,
}

impl Edit {
    pub fn new(old_string: impl Into<String>, new_string: impl Into<String>) -> Self {
        Self {
            old_string: old_string.into(),
            new_string: new_string.into(),
            context_before: String::new(),
            context_after: String::new(),
        }
    }

    pub fn with_context(
        mut self,
        context_before: impl Into<String>,
        context_after: impl Into<String>,
    ) -> Self {
        self.context_before = context_before.into();
        self.context_after = context_after.into();
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Patch {
    pub id: String,
    pub kind: PatchKind,
    pub path: String,
    #[serde(default)]
    pub rationale: String,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub edits: Vec<Edit>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatchPlan {
    pub goal: String,
    pub strategy: String,
    #[serde(default)]
    pub patches: Vec<Patch>,
    #[serde(default)]
    pub target_files: Vec<String>,
    #[serde(default)]
    pub done: bool,
    #[serde(default)]
    pub iteration: u32,
    #[serde(default)]
    pub parent_id: Option<String>,
}

/// Serialize a patch to a JSON object string. The Python wrappers
/// json.loads() this to feed `dict`-shaped APIs; staying with JSON
/// keeps Rust unit tests independent of PyO3.
pub fn patch_to_json(patch: &Patch) -> serde_json::Value {
    serde_json::to_value(patch).expect("Patch serializes")
}

pub fn patch_from_json(value: &serde_json::Value) -> Result<Patch, serde_json::Error> {
    serde_json::from_value(value.clone())
}

pub fn plan_to_json(plan: &PatchPlan) -> serde_json::Value {
    serde_json::to_value(plan).expect("PatchPlan serializes")
}

pub fn plan_from_json(value: &serde_json::Value) -> Result<PatchPlan, serde_json::Error> {
    serde_json::from_value(value.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patchkind_string_round_trip() {
        for k in [PatchKind::Create, PatchKind::Modify, PatchKind::Delete] {
            assert_eq!(PatchKind::from_str(k.as_str()), Some(k));
        }
        assert_eq!(PatchKind::from_str("garbage"), None);
    }

    #[test]
    fn patchstatus_string_round_trip() {
        for s in [
            PatchStatus::Applied,
            PatchStatus::AlreadyApplied,
            PatchStatus::NotFound,
            PatchStatus::Ambiguous,
        ] {
            assert_eq!(PatchStatus::from_str(s.as_str()), Some(s));
        }
    }

    #[test]
    fn plan_round_trip_through_json() {
        let plan = PatchPlan {
            goal: "fix syntax".into(),
            strategy: "anchor-based".into(),
            patches: vec![Patch {
                id: "p1".into(),
                kind: PatchKind::Modify,
                path: "a/b.py".into(),
                rationale: "missing colon".into(),
                content: None,
                edits: vec![Edit::new("def add(a, b)", "def add(a, b):")
                    .with_context("", "    return a + b")],
            }],
            target_files: vec!["a/b.py".into()],
            done: false,
            iteration: 1,
            parent_id: Some("plan-0".into()),
        };
        let v = plan_to_json(&plan);
        let back = plan_from_json(&v).unwrap();
        assert_eq!(plan, back);
        // The wire format must use the same lowercase strings the
        // Python str-Enums produced, otherwise existing JSON consumers
        // break.
        assert_eq!(
            v["patches"][0]["kind"]
                .as_str()
                .expect("kind serializes as a string"),
            "modify"
        );
    }

    #[test]
    fn patch_round_trip_omits_optional_content_field_correctly() {
        let p = Patch {
            id: "p2".into(),
            kind: PatchKind::Create,
            path: "new.py".into(),
            rationale: "".into(),
            content: Some("print('hi')\n".into()),
            edits: vec![],
        };
        let v = patch_to_json(&p);
        let back = patch_from_json(&v).unwrap();
        assert_eq!(p, back);
    }

    #[test]
    fn plan_with_defaults_parses_minimal_payload() {
        // Python from_dict accepts a payload with only goal+strategy;
        // serde defaults must match.
        let v = serde_json::json!({
            "goal": "noop",
            "strategy": "noop",
        });
        let plan = plan_from_json(&v).unwrap();
        assert_eq!(plan.goal, "noop");
        assert!(plan.patches.is_empty());
        assert_eq!(plan.iteration, 0);
        assert_eq!(plan.parent_id, None);
        assert!(!plan.done);
    }
}
