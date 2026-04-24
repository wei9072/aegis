from dataclasses import dataclass, field
from typing import Literal
from aegis.core.bindings import get_imports


@dataclass
class SemanticNode:
    type: Literal["dependency"]
    file_path: str
    name: str
    metadata: dict = field(default_factory=dict)

    def to_dict(self) -> dict:
        return {
            "type": self.type,
            "file_path": self.file_path,
            "name": self.name,
            "metadata": self.metadata,
        }


class IRBuilder:
    def build_from_file(self, filepath: str) -> list[SemanticNode]:
        imports = get_imports(filepath)
        return [
            SemanticNode(type="dependency", file_path=filepath, name=imp)
            for imp in imports
        ]
