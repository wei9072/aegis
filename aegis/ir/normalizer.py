from pathlib import Path


def build_module_map(root: str, py_files: list[str]) -> dict[str, str]:
    """Map module names to absolute file paths within a project root."""
    root_path = Path(root).resolve()
    module_map: dict[str, str] = {}
    for filepath in py_files:
        p = Path(filepath).resolve()
        try:
            rel = p.relative_to(root_path)
        except ValueError:
            continue
        parts = list(rel.parts)
        if parts[-1] == "__init__.py":
            parts = parts[:-1]
        else:
            parts[-1] = parts[-1][:-3]
        module_name = ".".join(parts)
        module_map[module_name] = str(p)
        if "." in module_name:
            module_map[parts[-1]] = str(p)
    return module_map
