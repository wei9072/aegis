from dataclasses import dataclass, field


@dataclass
class Ring0Config:
    syntax_validity: bool = True
    anti_circular_dependency: bool = True


@dataclass
class Ring05Config:
    fan_out_advisory: int = 15
    max_chain_depth_advisory: int = 3


@dataclass
class AegisConfig:
    ring0: Ring0Config = field(default_factory=Ring0Config)
    ring0_5: Ring05Config = field(default_factory=Ring05Config)
