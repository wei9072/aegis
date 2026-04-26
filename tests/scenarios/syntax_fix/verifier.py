"""
syntax_fix verifier — does broken.py parse?

The task is binary: file parses or it doesn't. `ast.parse` is the
ground truth. No signal layer needed; the question is purely
syntactic.
"""
from __future__ import annotations

import ast
from pathlib import Path

from aegis.runtime.task_verifier import VerifierResult


class SyntaxFixVerifier:
    def verify(self, workspace: Path, trace) -> VerifierResult:
        target = workspace / "broken.py"
        if not target.exists():
            return VerifierResult(
                passed=False,
                rationale="broken.py not found in workspace",
                evidence={"path": str(target)},
            )
        src = target.read_text(encoding="utf-8")
        try:
            ast.parse(src)
        except SyntaxError as e:
            return VerifierResult(
                passed=False,
                rationale=f"broken.py still has SyntaxError: {e.msg} (line {e.lineno})",
                evidence={
                    "ast_parsed": False,
                    "error": f"{e.msg} at line {e.lineno}",
                },
            )
        return VerifierResult(
            passed=True,
            rationale="broken.py parses cleanly",
            evidence={"ast_parsed": True, "bytes": len(src)},
        )
