"""
Pure edit-application logic shared by Validator (virtual-fs simulation)
and Executor (real write).

Re-export module — the implementation lives in `aegis._core` (Rust,
via the aegis-pyshim crate) since the V1.3 IR-model port. The
public API shape (`apply_edit`, `apply_edits`, `is_ok`, `EditResult`)
is identical to V0.x. The line-aware fallback joiner (introduced to
fix the syntax_fix scenario byte-concat bug) lives in
`crates/aegis-ir/src/edit_engine.rs`.
"""
from __future__ import annotations

from aegis._core import EditResult, apply_edit, apply_edits, is_ok

__all__ = ["EditResult", "apply_edit", "apply_edits", "is_ok"]
