"""
Built-in scenarios for the Aegis eval harness.

Each scenario asserts the *current* trace shape. The `note` field
documents known gaps — places where a future layer (ToolCallValidator,
delivery, policy, intent, intent-bypass) is expected to introduce
additional events. When that layer ships, the corresponding scenario's
`expected_events` should be tightened to include the new gate's
decision; the scenario's GAP entry then becomes a regression assertion
instead of a TODO.

This file is the closest thing the project has to a behaviour spec.
"""
from __future__ import annotations

from aegis.eval.harness import ExpectedEvent, Scenario
from aegis.tools.file_system import list_directory, read_file


_E = ExpectedEvent

_CODE_OK_BARE = "x = 1"
_CODE_OK_FENCED = "```python\ndef ok():\n    pass\n```"
_CODE_BAD_FENCED = "```python\ndef bad(\n```"
_CODE_HEAVY_IMPORTS = (
    "```python\n"
    + "\n".join(f"import mod_{i}" for i in range(15))
    + "\nx = 1\n```"
)
_CODE_DEEP_CHAIN = "```python\nresult = a.b().c().d().e().upper()\n```"
_CODE_HALLUCINATED_TOOL_CALL = (
    "```python\ndef fibonacci(n):\n    return n\n```\n"
    "我已經為你創建了 fibonacci 資料夾，並寫入 fibonacci.py。"
)
_CODE_SYNTAX_BYPASS_VIA_STRING = '```python\nexample = "def bad("\nprint(example)\n```'


SCENARIOS: list[Scenario] = [
    # ---------- Happy path ----------
    Scenario(
        name="01-happy-path-bare-code",
        description="Plain valid Python returns immediately with no signals.",
        prompt="write x = 1",
        llm_responses=[_CODE_OK_BARE],
        expected_events=[
            _E("gateway", "observe", "request_started"),
            _E("provider", "observe", "tool_surface"),
            _E("ring0", "pass", "syntax_valid"),
            _E("gateway", "pass", "response_accepted"),
        ],
    ),

    # ---------- Retry mechanics ----------
    Scenario(
        name="02-syntax-error-then-recovery",
        description="LLM emits broken syntax once, then valid code on retry.",
        prompt="write a function",
        llm_responses=[_CODE_BAD_FENCED, _CODE_OK_FENCED],
        expected_events=[
            _E("ring0", "block", "syntax_invalid"),
            _E("gateway", "observe", "retry"),
            _E("ring0", "pass", "syntax_valid"),
            _E("gateway", "pass", "response_accepted"),
        ],
    ),
    Scenario(
        name="03-max-retries-exhausted",
        description="Repeated syntax errors exhaust the retry budget.",
        prompt="bad",
        llm_responses=[_CODE_BAD_FENCED] * 3,
        expects_raise=True,
        expected_events=[
            _E("ring0", "block", "syntax_invalid"),
            _E("ring0", "block", "syntax_invalid"),
            _E("ring0", "block", "syntax_invalid"),
            _E("gateway", "block", "max_retries_exhausted"),
        ],
    ),

    # ---------- Ring 0.5 observations ----------
    Scenario(
        name="04-high-fan-out-observed-warned",
        description=(
            "15 imports — Ring 0.5 observes fan_out, policy escalates to "
            "warn, delivery surfaces a banner before the code."
        ),
        prompt="lots of imports",
        llm_responses=[_CODE_HEAVY_IMPORTS],
        expected_events=[
            _E("ring0", "pass", "syntax_valid"),
            _E("ring0_5", "observe", "fan_out"),
            _E("policy", "warn", "high_fan_out_advisory"),
            _E("delivery", "observe", "warning_surfaced"),
            _E("gateway", "pass", "response_accepted"),
        ],
        note=(
            "Phase 1 closed: signal → policy → decision → action → trace. "
            "fan_out >= 10 triggers `policy:warn` and the delivery layer "
            "surfaces a banner before the code block (LLM-bound channel "
            "stays clean)."
        ),
    ),
    Scenario(
        name="05-deep-chain-observed-warned",
        description=(
            "Method chain depth 5 — Ring 0.5 observes max_chain_depth, "
            "policy escalates to a Demeter advisory."
        ),
        prompt="deep chain",
        llm_responses=[_CODE_DEEP_CHAIN],
        expected_events=[
            _E("ring0", "pass", "syntax_valid"),
            _E("ring0_5", "observe", "max_chain_depth"),
            _E("policy", "warn", "demeter_violation_advisory"),
            _E("delivery", "observe", "warning_surfaced"),
            _E("gateway", "pass", "response_accepted"),
        ],
        note=(
            "Phase 1 closed: max_chain_depth >= 5 triggers the Demeter "
            "advisory and the delivery layer surfaces it."
        ),
    ),

    # ---------- Non-code conversation ----------
    Scenario(
        name="06-non-code-conversation",
        description="Natural-language reply with no code blocks; Ring 0 short-circuits to non_code_response.",
        prompt="say hi",
        llm_responses=["哈囉！我是 AI 助手。"],
        expected_events=[
            _E("ring0", "pass", "non_code_response"),
            _E("gateway", "pass", "response_accepted"),
        ],
    ),

    # ---------- Tool surface visibility ----------
    Scenario(
        name="07-tool-surface-default-empty",
        description="No tools passed → fake provider records empty surface in trace.",
        prompt="hi",
        llm_responses=[_CODE_OK_BARE],
        tools=None,
        expected_events=[
            _E("provider", "observe", "tool_surface", metadata_includes={"tools": []}),
            _E("gateway", "pass", "response_accepted"),
        ],
    ),
    Scenario(
        name="08-tool-surface-explicit-readonly",
        description="Caller passes explicit read-only tools; trace records names.",
        prompt="hi",
        llm_responses=[_CODE_OK_BARE],
        tools=(read_file, list_directory),
        expected_events=[
            _E(
                "provider",
                "observe",
                "tool_surface",
                metadata_includes={"tools": ["read_file", "list_directory"]},
            ),
            _E("gateway", "pass", "response_accepted"),
        ],
    ),

    # ---------- Known gaps (currently pass; future layers will catch) ----------
    Scenario(
        name="09-syntax-bypass-via-string-literal",
        description=(
            "User asked to see broken syntax; LLM hides 'def bad(' inside a "
            "string. Technically valid Python, semantically completes the "
            "would-be-rejected intent."
        ),
        prompt="show me what broken syntax looks like",
        llm_responses=[_CODE_SYNTAX_BYPASS_VIA_STRING],
        expected_events=[
            _E("ring0", "pass", "syntax_valid"),
            _E("gateway", "pass", "response_accepted"),
        ],
        note=(
            "GAP: intent-bypass. Future intent-bypass layer should detect "
            "that the prompt asked for invalid code and the response "
            "satisfies that intent semantically. Today there is no "
            "semantic comparison, so the request silently passes."
        ),
    ),
    Scenario(
        name="10-hallucinated-side-effect-claim",
        description=(
            "LLM claims it created files / folders, but no Executor call "
            "happened. Aegis has no visibility into the gap between text "
            "and reality today."
        ),
        prompt="create a folder and write fibonacci.py inside",
        llm_responses=[_CODE_HALLUCINATED_TOOL_CALL],
        expected_events=[
            _E("ring0", "pass", "syntax_valid"),
            _E("gateway", "pass", "response_accepted"),
        ],
        note=(
            "GAP: hallucinated side-effect. Future ToolCallValidator "
            "(Tier-2 semantic) should compare the natural-language claim "
            "('我已經為你創建了...') against actual filesystem state and "
            "block / warn when they diverge. Tier-1 (existence check) "
            "alone won't catch this — the LLM never even attempted a "
            "tool call."
        ),
    ),
]
