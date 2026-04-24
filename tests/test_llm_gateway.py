import pytest
from aegis.agents.llm_adapter import LLMGateway, LLMProvider, Ring0Validator, PromptFormatter


class FakeProvider:
    def __init__(self, responses):
        self._responses = iter(responses)

    def generate(self, prompt: str) -> str:
        return next(self._responses)


def test_gateway_returns_valid_code():
    provider = FakeProvider(["x = 1"])
    gw = LLMGateway(llm_provider=provider)
    result = gw.generate_and_validate("write x = 1")
    assert "x = 1" in result


def test_gateway_retries_on_syntax_error():
    provider = FakeProvider([
        "```python\ndef err(\n```",
        "```python\ndef ok():\n    pass\n```",
    ])
    gw = LLMGateway(llm_provider=provider)
    result = gw.generate_and_validate("write a function")
    assert "ok" in result


def test_gateway_raises_after_max_retries():
    provider = FakeProvider(["```python\ndef err(\n```"] * 10)
    gw = LLMGateway(llm_provider=provider)
    with pytest.raises(RuntimeError, match="Failed to generate"):
        gw.generate_and_validate("bad prompt", max_retries=3)


def test_ring0_validator_allows_high_fan_out():
    validator = Ring0Validator()
    code = "```python\n" + "\n".join(f"import mod_{i}" for i in range(20)) + "\n```"
    violations = validator.validate(code)
    assert violations == []


def test_ring0_validator_blocks_syntax_error():
    validator = Ring0Validator()
    code = "```python\ndef err(\n```"
    violations = validator.validate(code)
    assert len(violations) == 1


def test_prompt_formatter_retry():
    result = PromptFormatter.format_retry("original", ["[Ring 0] syntax error"])
    assert "Ring 0" in result
    assert "original" in result


def test_gateway_conversational_text():
    provider = FakeProvider(["哈囉！我是 AI 助手。您有什麼需要幫忙的嗎？"])
    gw = LLMGateway(llm_provider=provider)
    result = gw.generate_and_validate("打招呼")
    assert "哈囉" in result


def test_gateway_successful_generation_legacy():
    class ValidClient:
        def generate(self, prompt): return "def process():\n    return 42\n"

    gw = LLMGateway(llm_provider=ValidClient())
    result = gw.generate_and_validate("Generate a process function")
    assert "def process():" in result


def test_gateway_retries_on_violation_legacy():
    class FixingClient:
        def __init__(self): self.attempts = 0
        def generate(self, prompt):
            self.attempts += 1
            return "def broken(\n" if self.attempts == 1 else "def fixed():\n    return 42\n"

    client = FixingClient()
    gw = LLMGateway(llm_provider=client)
    result = gw.generate_and_validate("Generate a function")
    assert "def fixed():" in result
    assert client.attempts == 2
