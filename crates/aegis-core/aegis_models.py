from typing import Optional, Literal
from pydantic import BaseModel, Field

class RuleConfig(BaseModel):
    enabled: bool = True
    action: Literal["error", "warning", "ignore"] = "error"
    message: str

class CouplingConfig(RuleConfig):
    max_fan_out: int = Field(default=10, ge=0)

class DemeterConfig(RuleConfig):
    max_chain_depth: int = Field(default=3, ge=0)

class GlobalPrinciples(BaseModel):
    syntax_validity: RuleConfig
    anti_circular_dependency: RuleConfig
    coupling_limits: CouplingConfig
    method_chain_limits: DemeterConfig

class AegisPolicy(BaseModel):
    global_principles: GlobalPrinciples
