# Circular Dependency Detection 實現計畫

> **面向 AI 代理的工作者：** 必需子技能：使用 superpowers:subagent-driven-development（推薦）或 superpowers:executing-plans 逐任務實現此計畫。步驟使用複選框（`- [ ]`）語法來跟蹤進度。

**目標：** 在 `aegis check` CLI 命令中接入已有的 `DependencyGraph` Rust 核心，讓跨檔案的循環依賴可被偵測並回報。

**架構：** Rust 側新增 `get_imports()` 函式，重用現有的 tree-sitter query 回傳 import 字串清單；Python CLI 側讀取 policy YAML、建立模組名稱→路徑映射表、建圖並呼叫 `check_circular_dependency()`。

**技術棧：** Rust + PyO3 + tree-sitter-python、Python + Click + PyYAML、pytest

---

## 檔案清單

| 檔案 | 變更類型 | 職責 |
|------|----------|------|
| `aegis-core-rs/src/ast_parser.rs` | 修改 | 新增 `get_imports()` PyO3 函式 |
| `aegis-core-rs/src/lib.rs` | 修改 | export `get_imports` |
| `aegis/cli.py` | 修改 | 在 `check` 命令中實作循環依賴偵測 |
| `tests/test_ast_parser.py` | 修改 | 新增 `get_imports` 的 Python 測試 |
| `tests/test_cli.py` | 修改 | 新增循環依賴偵測的 CLI 端對端測試 |

---

### 任務 1：在 Rust 中新增 `get_imports()` 並通過 Rust 單元測試

**檔案：**
- 修改：`aegis-core-rs/src/ast_parser.rs`

- [ ] **步驟 1：在 `ast_parser.rs` 中新增 `get_imports` 函式**

在 `analyze_file` 函式之後（約第 68 行）加入：

```rust
#[pyfunction]
pub fn get_imports(filepath: &str) -> PyResult<Vec<String>> {
    let code = match fs::read_to_string(filepath) {
        Ok(c) => c,
        Err(e) => return Err(pyo3::exceptions::PyIOError::new_err(e.to_string())),
    };

    let mut parser = Parser::new();
    let lang = language();
    parser.set_language(lang).unwrap();

    let tree = parser.parse(&code, None).unwrap();
    let root_node = tree.root_node();

    let query_source = include_str!("../queries/python.scm");
    let query = match Query::new(lang, query_source) {
        Ok(q) => q,
        Err(_) => return Ok(vec![]),
    };

    let mut query_cursor = QueryCursor::new();
    let matches = query_cursor.matches(&query, root_node, code.as_bytes());
    let mut unique_imports = std::collections::HashSet::new();
    for m in matches {
        for cap in m.captures {
            if let Ok(text) = cap.node.utf8_text(code.as_bytes()) {
                unique_imports.insert(text.to_string());
            }
        }
    }

    let mut result: Vec<String> = unique_imports.into_iter().collect();
    result.sort();
    Ok(result)
}
```

- [ ] **步驟 2：在 `ast_parser.rs` 的 `#[cfg(test)]` 區塊中新增單元測試**

在現有 `test_query_imports` 測試之後加入：

```rust
#[test]
fn test_get_imports_returns_sorted_list() {
    use std::io::Write;
    let code = b"import os\nimport sys\nfrom mymodule import Foo\n";
    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    tmp.write_all(code).unwrap();
    let path = tmp.path().to_str().unwrap().to_string();

    let result = get_imports(&path).unwrap();
    assert!(result.contains(&"os".to_string()));
    assert!(result.contains(&"sys".to_string()));
    assert!(result.contains(&"mymodule".to_string()));
    assert_eq!(result, {
        let mut sorted = result.clone();
        sorted.sort();
        sorted
    });
}
```

注意：`tempfile` 是開發依賴，需確認 `Cargo.toml` 的 `[dev-dependencies]` 有加入：
```toml
tempfile = "3"
```

- [ ] **步驟 3：執行 Rust 單元測試，確認通過**

```bash
cd aegis-core-rs && cargo test test_get_imports_returns_sorted_list -- --nocapture
```

預期：`test test_get_imports_returns_sorted_list ... ok`

- [ ] **步驟 4：Commit**

```bash
git add aegis-core-rs/src/ast_parser.rs aegis-core-rs/Cargo.toml
git commit -m "feat(ring0): add get_imports() pyfunction to ast_parser"
```

---

### 任務 2：Export `get_imports` 並重建 Python 套件

**檔案：**
- 修改：`aegis-core-rs/src/lib.rs`

- [ ] **步驟 1：在 `lib.rs` 的 `aegis_core_rs` 模組中加入 export**

在現有的 `m.add_function(wrap_pyfunction!(ast_parser::analyze_file, m)?)?;` 那行之後加：

```rust
m.add_function(wrap_pyfunction!(ast_parser::get_imports, m)?)?;
```

完整的 `#[pymodule]` 區塊應如下：

```rust
#[pymodule]
fn aegis_core_rs(_py: Python, m: &PyModule) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(ring0_status, m)?)?;
    m.add_function(wrap_pyfunction!(ts_parser::extract_ts_imports, m)?)?;
    m.add_class::<models::Violation>()?;
    m.add_class::<graph_engine::DependencyGraph>()?;
    m.add_class::<ast_parser::AstMetrics>()?;
    m.add_function(wrap_pyfunction!(ast_parser::analyze_file, m)?)?;
    m.add_function(wrap_pyfunction!(ast_parser::get_imports, m)?)?;
    m.add_function(wrap_pyfunction!(policy_validator::validate_file_policy, m)?)?;
    Ok(())
}
```

- [ ] **步驟 2：重建 Python 套件**

```bash
cd aegis-core-rs && source ../.venv/bin/activate && maturin develop
```

預期：`🔗 Found pyo3 bindings ... ✅ aegis-core-rs` 成功訊息，無 error。

- [ ] **步驟 3：確認 `get_imports` 出現在 Python 模組中**

```bash
.venv/bin/python -c "import aegis_core_rs; print('get_imports' in dir(aegis_core_rs))"
```

預期：`True`

- [ ] **步驟 4：Commit**

```bash
git add aegis-core-rs/src/lib.rs
git commit -m "feat(ring0): export get_imports in PyO3 module"
```

---

### 任務 3：新增 `get_imports` 的 Python 測試並確認通過

**檔案：**
- 修改：`tests/test_ast_parser.py`

- [ ] **步驟 1：在 `tests/test_ast_parser.py` 末尾新增測試**

```python
def test_get_imports_basic():
    code = b"import os\nimport sys\nfrom typing import List\n"
    with tempfile.NamedTemporaryFile(suffix=".py", delete=False) as f:
        f.write(code)
        f.flush()

    result = aegis_core_rs.get_imports(f.name)
    os.unlink(f.name)

    assert "os" in result
    assert "sys" in result
    assert "typing" in result
    assert result == sorted(result)


def test_get_imports_deduplication():
    code = b"import os\nimport os\nfrom os import path\n"
    with tempfile.NamedTemporaryFile(suffix=".py", delete=False) as f:
        f.write(code)
        f.flush()

    result = aegis_core_rs.get_imports(f.name)
    os.unlink(f.name)

    assert result.count("os") == 1


def test_get_imports_no_imports():
    code = b"x = 1\nprint(x)\n"
    with tempfile.NamedTemporaryFile(suffix=".py", delete=False) as f:
        f.write(code)
        f.flush()

    result = aegis_core_rs.get_imports(f.name)
    os.unlink(f.name)

    assert result == []
```

- [ ] **步驟 2：執行測試，確認三個新測試全部通過**

```bash
.venv/bin/pytest tests/test_ast_parser.py -v
```

預期：`test_get_imports_basic PASSED`、`test_get_imports_deduplication PASSED`、`test_get_imports_no_imports PASSED`

- [ ] **步驟 3：Commit**

```bash
git add tests/test_ast_parser.py
git commit -m "test: add Python tests for get_imports()"
```

---

### 任務 4：在 `cli.py` 的 `check` 命令中接入循環依賴偵測

**檔案：**
- 修改：`aegis/cli.py`

- [ ] **步驟 1：在 `cli.py` 頂部新增 `import yaml`**

在現有的 import 區塊（第 1-4 行）後加入：

```python
import yaml
from pathlib import Path
```

- [ ] **步驟 2：新增 `_build_module_map` 輔助函式**

在 `@cli.group()` 的定義之前（第 7 行之前）加入：

```python
def _build_module_map(root: str, py_files: list[str]) -> dict[str, str]:
    """將模組名稱映射到專案內部的 .py 檔案路徑。"""
    module_map = {}
    root_path = Path(root).resolve()
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
            parts[-1] = parts[-1][:-3]  # 去掉 .py
        module_name = ".".join(parts)
        module_map[module_name] = str(p)
        # 也加入最後一段（方便 `import submod` 匹配 `pkg/submod.py`）
        if "." in module_name:
            module_map[parts[-1]] = str(p)
    return module_map
```

- [ ] **步驟 3：在 `check` 命令中替換 TODO 區塊**

找到 `check` 命令中的以下區塊（約 `cli.py:40-41`）：

```python
    # TODO: Build Graph for Circular Dependency
    # For now, just analyze each file individually with AST Metrics
    for f in py_files:
```

替換為：

```python
    # 循環依賴偵測（跨檔案，graph 層）
    with open(policy) as _f:
        _policy_data = yaml.safe_load(_f)
    _circular_rule = _policy_data.get("global_principles", {}).get("anti_circular_dependency", {})
    if _circular_rule.get("enabled", False) and len(py_files) > 1:
        _root = path if os.path.isdir(path) else os.path.dirname(path)
        _module_map = _build_module_map(_root, py_files)
        _edges = []
        for _f in py_files:
            try:
                _imports = aegis_core_rs.get_imports(_f)
                for _imp in _imports:
                    if _imp in _module_map:
                        _edges.append((_f, _module_map[_imp]))
            except Exception as e:
                click.echo(f"Warning: failed to get imports for {_f}: {e}", err=True)
        if _edges:
            _dg = aegis_core_rs.DependencyGraph()
            _dg.build_from_edges(_edges)
            if _dg.check_circular_dependency():
                has_violations = True
                click.echo("Circular dependency detected across project files.")
                click.echo(f"  {_circular_rule.get('message', '')}")

    for f in py_files:
```

- [ ] **步驟 4：手動驗證（smoke test）**

建立一個臨時的循環依賴情境：

```bash
mkdir -p /tmp/aegis_cycle_test
echo "from b import x" > /tmp/aegis_cycle_test/a.py
echo "from a import x" > /tmp/aegis_cycle_test/b.py
.venv/bin/python -m aegis.cli check /tmp/aegis_cycle_test --policy templates/default_core_policy.yaml
```

預期輸出包含：
```
Circular dependency detected across project files.
  系統內核攔截：偵測到循環依賴 ...
Aegis check failed.
```
且 exit code 為 1（確認：`echo $?`）

- [ ] **步驟 5：Commit**

```bash
git add aegis/cli.py
git commit -m "feat(cli): wire DependencyGraph into check command for circular dep detection"
```

---

### 任務 5：新增 CLI 端對端測試

**檔案：**
- 修改：`tests/test_cli.py`

- [ ] **步驟 1：在 `tests/test_cli.py` 末尾新增測試**

```python
import tempfile
import os

def test_cli_check_detects_circular_dependency(tmp_path):
    # 建立 A -> B -> A 的循環依賴
    (tmp_path / "mod_a.py").write_text("from mod_b import Foo\n")
    (tmp_path / "mod_b.py").write_text("from mod_a import Bar\n")

    runner = CliRunner()
    result = runner.invoke(cli, [
        'check', str(tmp_path),
        '--policy', 'templates/default_core_policy.yaml'
    ])

    assert result.exit_code == 1
    assert "Circular dependency detected" in result.output


def test_cli_check_no_false_positive_on_clean_project(tmp_path):
    # 建立無循環的依賴：A -> B（單向）
    (tmp_path / "mod_a.py").write_text("from mod_b import Foo\n")
    (tmp_path / "mod_b.py").write_text("x = 1\n")

    runner = CliRunner()
    result = runner.invoke(cli, [
        'check', str(tmp_path),
        '--policy', 'templates/default_core_policy.yaml'
    ])

    assert "Circular dependency detected" not in result.output


def test_cli_check_single_file_skips_graph(tmp_path):
    # 單一檔案不應觸發 graph 建立（不會 false positive）
    (tmp_path / "solo.py").write_text("import os\n")

    runner = CliRunner()
    result = runner.invoke(cli, [
        'check', str(tmp_path),
        '--policy', 'templates/default_core_policy.yaml'
    ])

    assert "Circular dependency detected" not in result.output
```

- [ ] **步驟 2：執行測試，確認三個新測試全部通過**

```bash
.venv/bin/pytest tests/test_cli.py -v
```

預期：所有 `test_cli_check_*` 測試 PASSED，原有測試維持通過。

- [ ] **步驟 3：執行完整測試套件確認無回歸**

```bash
.venv/bin/pytest tests/ -v --ignore=tests/.venv
```

預期：所有測試通過，`0 failed`。

- [ ] **步驟 4：Commit**

```bash
git add tests/test_cli.py
git commit -m "test: add CLI end-to-end tests for circular dependency detection"
```
