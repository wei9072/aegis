import yaml
from pathlib import Path
from aegis.config.schema import AegisConfig, Ring0Config, Ring05Config


def load(path: str) -> AegisConfig:
    data = yaml.safe_load(Path(path).read_text()) or {}
    ring0_data = data.get("ring0", {})
    ring05_data = data.get("ring0_5_signals", {})
    return AegisConfig(
        ring0=Ring0Config(
            syntax_validity=ring0_data.get("syntax_validity", {}).get("enabled", True),
            anti_circular_dependency=ring0_data.get("anti_circular_dependency", {}).get("enabled", True),
        ),
        ring0_5=Ring05Config(
            fan_out_advisory=ring05_data.get("fan_out_advisory", 15),
            max_chain_depth_advisory=ring05_data.get("max_chain_depth_advisory", 3),
        ),
    )
