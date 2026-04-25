"""
LLM adapter base + LLMGateway.
Moved from agents/llm_gateway.py; all Ring 0 + Signal logic stays here.
"""
from typing import Protocol, Optional
import os
import tempfile
import re
import aegis_core_rs
from aegis.analysis.signals import SignalLayer
from aegis.runtime.trace import BLOCK, OBSERVE, PASS, DecisionTrace


class LLMProvider(Protocol):
    """LLM backend contract.

    `tools` is the per-request tool surface — None means "let the provider
    fall back to its declared read-only default". Providers MUST refuse to
    accept any state-mutating callable here; mutations belong to Executor.
    """

    def generate(self, prompt: str, tools: tuple | None = None) -> str: ...


class Ring0Validator:
    """Validates generated code against Ring 0 rules only (syntax validity)."""

    def validate(
        self,
        text: str,
        trace: Optional[DecisionTrace] = None,
    ) -> list[str]:
        pattern = r"```(?:python|py)?\n(.*?)\n```"
        matches = re.findall(pattern, text, re.DOTALL | re.IGNORECASE)
        code = "\n\n".join(matches) if matches else text

        with tempfile.NamedTemporaryFile(suffix=".py", delete=False) as f:
            f.write(code.encode("utf-8"))
            f.flush()
            path = f.name

        try:
            violations = aegis_core_rs.check_syntax(path)
            if violations and not matches:
                if not re.search(r"\b(def|import|class|from|return|if|for|while)\b", code):
                    if trace is not None:
                        trace.emit(
                            layer="ring0",
                            decision=PASS,
                            reason="non_code_response",
                            metadata={"chars": len(text)},
                        )
                    return []
            if trace is not None:
                if violations:
                    trace.emit(
                        layer="ring0",
                        decision=BLOCK,
                        reason="syntax_invalid",
                        metadata={"violations": list(violations)},
                    )
                else:
                    trace.emit(
                        layer="ring0",
                        decision=PASS,
                        reason="syntax_valid",
                    )
            return violations
        finally:
            if os.path.exists(path):
                os.unlink(path)


class SignalContextBuilder:
    """Extracts Ring 0.5 signals from generated code and formats them for LLM context."""

    def __init__(self) -> None:
        self._layer = SignalLayer()

    def build_context(
        self,
        text: str,
        trace: Optional[DecisionTrace] = None,
    ) -> str:
        pattern = r"```(?:python|py)?\n(.*?)\n```"
        matches = re.findall(pattern, text, re.DOTALL | re.IGNORECASE)
        if not matches:
            return ""
        code = "\n\n".join(matches)
        with tempfile.NamedTemporaryFile(suffix=".py", delete=False) as f:
            f.write(code.encode("utf-8"))
            f.flush()
            path = f.name
        try:
            signals = self._layer.extract(path, trace=trace)
            return self._layer.format_for_llm(signals)
        finally:
            if os.path.exists(path):
                os.unlink(path)


class PromptFormatter:
    @staticmethod
    def format_retry(original_prompt: str, violations: list[str]) -> str:
        violation_text = "\n".join(f"- {v}" for v in violations)
        return (
            f"{original_prompt}\n\n"
            f"Previous attempt failed Ring 0 validation:\n{violation_text}\n"
            "Please fix the syntax error and regenerate."
        )

    @staticmethod
    def format_with_signals(prompt: str, signal_context: str) -> str:
        if not signal_context:
            return prompt
        return f"{prompt}\n\n{signal_context}"


class LLMGateway:
    def __init__(
        self,
        llm_provider: LLMProvider,
        validator: Optional[Ring0Validator] = None,
        signal_builder: Optional[SignalContextBuilder] = None,
    ) -> None:
        self.llm_provider = llm_provider
        self.validator = validator or Ring0Validator()
        self.signal_builder = signal_builder or SignalContextBuilder()
        # Most recent request's DecisionTrace; eval harness reads this after
        # generate_and_validate() returns. Reset on every call.
        self.last_trace: Optional[DecisionTrace] = None

    def generate_and_validate(
        self,
        prompt: str,
        max_retries: int = 3,
        tools: tuple | None = None,
    ) -> str:
        trace = DecisionTrace()
        self.last_trace = trace
        trace.emit(
            layer="gateway",
            decision=OBSERVE,
            reason="request_started",
            metadata={"prompt_chars": len(prompt), "max_retries": max_retries},
        )

        current_prompt = prompt
        last_violations: list[str] = []

        for attempt in range(max_retries):
            code = self.llm_provider.generate(current_prompt, tools=tools)
            self._emit_tool_surface(trace, attempt + 1)
            violations = self.validator.validate(code, trace=trace)

            if not violations:
                signal_ctx = self.signal_builder.build_context(code, trace=trace)
                trace.emit(
                    layer="gateway",
                    decision=PASS,
                    reason="response_accepted",
                    metadata={"attempt": attempt + 1, "has_signals": bool(signal_ctx)},
                )
                if signal_ctx:
                    sep = "\n# "
                    return code + "\n\n# --- Aegis Signals ---\n# " + signal_ctx.replace("\n", sep)
                return code

            current_prompt = PromptFormatter.format_retry(current_prompt, violations)
            last_violations = violations
            trace.emit(
                layer="gateway",
                decision=OBSERVE,
                reason="retry",
                metadata={"attempt": attempt + 1, "violations": list(violations)},
            )

        trace.emit(
            layer="gateway",
            decision=BLOCK,
            reason="max_retries_exhausted",
            metadata={"attempts": max_retries, "violations": list(last_violations)},
        )
        violation_text = "\n".join(f"- {v}" for v in last_violations)
        raise RuntimeError(
            f"Failed to generate valid code after {max_retries} attempts.\n{violation_text}"
        )

    def _emit_tool_surface(self, trace: DecisionTrace, attempt: int) -> None:
        """Record which tools the provider actually used for this attempt.

        The provider is expected to expose `last_used_tools` so the gateway
        can observe the resolved tool surface without coupling to any
        specific provider's internals. Providers that don't expose this
        attribute simply produce an empty list — the trace then records
        that the tool surface was not introspectable, which is itself a
        useful signal for the eval harness.
        """
        used = getattr(self.llm_provider, "last_used_tools", ())
        names = [getattr(t, "__name__", str(t)) for t in used]
        trace.emit(
            layer="provider",
            decision=OBSERVE,
            reason="tool_surface",
            metadata={"attempt": attempt, "tools": names},
        )
