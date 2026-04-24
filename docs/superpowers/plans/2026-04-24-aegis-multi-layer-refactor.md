# Aegis Multi-Layer Architecture Refactor 實現計畫

> **面向 AI 代理的工作者：** 必需子技能：使用 superpowers:subagent-driven-development（推薦）或 superpowers:executing-plans 逐任務實現此計畫。步驟使用複選框（`- [ ]`）語法來跟蹤進度。

**目標：** 將 Aegis 從 policy-based 系統重構為 multi-layer signal-driven 系統，讓 AST 提供事實、Graph 提供關係、Signals 提供觀察、LLM 做決策。

**架構：** Ring 0（強制執行：syntax + circular dep）→ Ring 0.5（Signal 觀察：fan-out/fan-in/chain depth，永不 block）→ Ring 1（LLM 決策）。Semantic IR 作為跨語言中間層，解耦 AST 語法與 Graph/Signal 邏輯。

**技術棧：** Rust + PyO3 + tree-sitter（AST/Enforcement/Signal）、Python（Layers orchestration）、petgraph（Graph）、Click（CLI）

---

## 檔案結構（重構後）

### Rust 側 (`aegis-core-rs/src/`)

| 檔案 | 職責 | 動作 |
|------|------|------|
| `models.rs` | `Violation` + 新增 `Signal` 結構 | 修改 |
| `ast_parser.rs` | AST 解析（語法樹、imports、chain depth）| 保留 |
| `graph_engine.rs` | DependencyGraph + cycle detection + fan-in | 保留 |
| `enforcement.rs` | Ring 0：語法檢查（新 pyfunction）| **新增** |
| `signal_layer.rs` | Ring 0.5：從 AstMetrics 提取 Signal vec | **新增** |
| `policy_validator.rs` | 舊式 YAML 強制執行 coupling/demeter | **刪除** |
| `lib.rs` | PyO3 模組出口更新 | 修改 |

### Python 側 (`aegis/`)

| 檔案 | 職責 | 動作 |
|------|------|------|
| `aegis/layers/__init__.py` | 層級 package | **新增** |
| `aegis/layers/semantic_ir.py` | SemanticNode dataclass + IRBuilder | **新增** |
| `aegis/layers/signal_layer.py` | SignalLayer.extract() + format_for_llm() | **新增** |
| `aegis/layers/enforcement.py` | Ring0Enforcer（syntax + circular dep） | **新增** |
| `aegis/agents/llm_gateway.py` | 重構：signals 注入 LLM prompt context | 修改 |
| `aegis/cli.py` | 更新 check command 用新 pipeline | 修改 |
| `templates/default_core_policy.yaml` | 移除 coupling/demeter 強制執行設定 | 修改 |

### 測試側 (`tests/`)

| 檔案 | 職責 | 動作 |
|------|------|------|
| `tests/test_signal_layer.py` | 驗證 Signal 提取與格式化 | **新增** |
| `tests/test_enforcement.py` | 驗證 Ring 0 只阻擋 syntax + circular | **新增** |
| `tests/test_semantic_ir.py` | 驗證 IR 建構邏輯 | **新增** |
| `tests/test_policy_validator.py` | 舊測試，隨 policy_validator 一起刪除 | **刪除** |
| `tests/test_core_integration.py` | 更新：移除 validate_file_policy 相關呼叫 | 修改 |

---

## 任務清單

---

### 任務 1：新增 `Signal` 結構到 `models.rs`

**Files:**
- 修改：`aegis-core-rs/src/models.rs`

- [ ] **步驟 1：在 `models.rs` 加入 `Signal` pyclass**

```rust
// aegis-core-rs/src/models.rs 末尾加入
#[pyclass(get_all)]
#[derive(Debug, Clone)]
pub struct Signal {
    pub name: String,
    pub value: f64,
    pub file_path: String,
    pub description: String,
}

#[pymethods]
impl Signal {
    #[new]
    pub fn new(name: String, value: f64, file_path: String, description: String) -> Self {
        Signal { name, value, file_path, description }
    }

    pub fn __repr__(&self) -> String {
        format!("Signal({} = {} @ {})", self.name, self.value, self.file_path)
    }
}
```

- [ ] **步驟 2：Commit**

```bash
git add aegis-core-rs/src/models.rs
git commit -m "feat(models): add Signal pyclass for Ring 0.5 observation layer"
```

---

### 任務 2：新增 `signal_layer.rs`（Ring 0.5 信號提取）

**Files:**
- 新增：`aegis-core-rs/src/signal_layer.rs`

Ring 0.5 從 `AstMetrics` 提取 `fan_out` 和 `max_chain_depth` 作為 Signal，**永遠不阻擋執行**。

- [ ] **步驟 1：寫失敗的 Rust 單元測試**

在 `aegis-core-rs/src/signal_layer.rs` 建立檔案並寫測試：

```rust
use pyo3::prelude::*;
use crate::models::Signal;
use crate::ast_parser::AstMetrics;

#[pyfunction]
pub fn extract_signals(filepath: &str) -> PyResult<Vec<Signal>> {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_signals_fan_out() {
        // 這個測試先確認函數簽名存在，impl 在步驟 3
        // 暫時 assert false 確認 todo!() 會 panic
        // 實際跑時先用 cargo build 確認結構
    }
}
```

- [ ] **步驟 2：實現 `extract_signals`**

```rust
use pyo3::prelude::*;
use crate::models::Signal;
use std::fs;
use tree_sitter::{Language, Parser, Query, QueryCursor};
use std::cmp;

pub fn language() -> Language {
    tree_sitter_python::language()
}

#[pyfunction]
pub fn extract_signals(filepath: &str) -> PyResult<Vec<Signal>> {
    let code = fs::read_to_string(filepath)
        .map_err(|e| pyo3::exceptions::PyIOError::new_err(e.to_string()))?;

    let mut parser = Parser::new();
    let lang = language();
    parser.set_language(lang)
        .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;

    let tree = parser.parse(&code, None)
        .ok_or_else(|| pyo3::exceptions::PyRuntimeError::new_err("parse returned None"))?;
    let root_node = tree.root_node();

    // fan_out: 從 import 語句計算
    let query_source = include_str!("../queries/python.scm");
    let mut fan_out = 0usize;
    if let Ok(query) = Query::new(lang, query_source) {
        let mut qc = QueryCursor::new();
        let matches = qc.matches(&query, root_node, code.as_bytes());
        let mut seen = std::collections::HashSet::new();
        for m in matches {
            for cap in m.captures {
                if let Ok(text) = cap.node.utf8_text(code.as_bytes()) {
                    seen.insert(text.to_string());
                }
            }
        }
        fan_out = seen.len();
    }

    // max_chain_depth: 遞迴計算
    let max_chain_depth = calculate_max_chain_depth(root_node) as f64;

    Ok(vec![
        Signal::new(
            "fan_out".to_string(),
            fan_out as f64,
            filepath.to_string(),
            format!("Number of unique external imports (fan-out = {})", fan_out),
        ),
        Signal::new(
            "max_chain_depth".to_string(),
            max_chain_depth,
            filepath.to_string(),
            format!("Maximum method/attribute chain depth (depth = {})", max_chain_depth),
        ),
    ])
}

fn calculate_max_chain_depth(node: tree_sitter::Node) -> usize {
    let mut max = 0;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "attribute" || child.kind() == "call" {
            max = max.max(get_chain_depth(child));
        }
        max = max.max(calculate_max_chain_depth(child));
    }
    max
}

fn get_chain_depth(node: tree_sitter::Node) -> usize {
    match node.kind() {
        "attribute" => {
            node.child_by_field_name("object")
                .map(|obj| 1 + get_chain_depth(obj))
                .unwrap_or(1)
        }
        "call" => {
            node.child_by_field_name("function")
                .map(get_chain_depth)
                .unwrap_or(0)
        }
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_extract_signals_fan_out() {
        let code = b"import os\nimport sys\nfrom typing import List\n";
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        tmp.write_all(code).unwrap();
        let path = tmp.path().to_str().unwrap().to_string();

        let signals = extract_signals(&path).unwrap();
        let fan_out_sig = signals.iter().find(|s| s.name == "fan_out").unwrap();
        assert_eq!(fan_out_sig.value, 3.0);
    }

    #[test]
    fn test_extract_signals_chain_depth() {
        let code = b"a.b().c().d()\n";
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        tmp.write_all(code).unwrap();
        let path = tmp.path().to_str().unwrap().to_string();

        let signals = extract_signals(&path).unwrap();
        let chain_sig = signals.iter().find(|s| s.name == "max_chain_depth").unwrap();
        assert_eq!(chain_sig.value, 3.0);
    }

    #[test]
    fn test_extract_signals_always_returns_two_signals() {
        let code = b"x = 1\n";
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        tmp.write_all(code).unwrap();
        let path = tmp.path().to_str().unwrap().to_string();

        let signals = extract_signals(&path).unwrap();
        assert_eq!(signals.len(), 2);
    }
}
```

- [ ] **步驟 3：執行 Rust 測試確認通過**

```bash
cd aegis-core-rs && cargo test signal_layer -- --nocapture
```

預期：3 個測試全部通過

- [ ] **步驟 4：Commit**

```bash
git add aegis-core-rs/src/signal_layer.rs
git commit -m "feat(signal_layer): add extract_signals() for Ring 0.5 observations"
```

---

### 任務 3：新增 `enforcement.rs`（Ring 0 語法強制執行）

**Files:**
- 新增：`aegis-core-rs/src/enforcement.rs`

Ring 0 只做兩件事：語法檢查（file 級）和循環依賴（graph 級，已在 `graph_engine.rs`）。

- [ ] **步驟 1：建立 `enforcement.rs` 並寫測試**

```rust
use pyo3::prelude::*;
use std::fs;
use tree_sitter::Parser;

fn python_language() -> tree_sitter::Language {
    tree_sitter_python::language()
}

/// Ring 0：檢查單一 Python 檔案的語法合法性。
/// 有語法錯誤回傳 violation message，否則回傳空 vec。
/// 循環依賴由 graph_engine::DependencyGraph::check_circular_dependency() 處理。
#[pyfunction]
pub fn check_syntax(filepath: &str) -> PyResult<Vec<String>> {
    let code = fs::read_to_string(filepath)
        .map_err(|e| pyo3::exceptions::PyIOError::new_err(e.to_string()))?;

    let mut parser = Parser::new();
    parser.set_language(python_language())
        .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;

    let tree = parser.parse(&code, None)
        .ok_or_else(|| pyo3::exceptions::PyRuntimeError::new_err("parse returned None"))?;

    if tree.root_node().has_error() {
        Ok(vec![format!(
            "[Ring 0] Syntax error detected in '{}'. Fix syntax before proceeding.",
            filepath
        )])
    } else {
        Ok(vec![])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_check_syntax_valid_file() {
        let code = b"def hello():\n    return 42\n";
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        tmp.write_all(code).unwrap();
        let violations = check_syntax(tmp.path().to_str().unwrap()).unwrap();
        assert!(violations.is_empty(), "Valid file should have no violations");
    }

    #[test]
    fn test_check_syntax_invalid_file() {
        let code = b"def err(\n";
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        tmp.write_all(code).unwrap();
        let violations = check_syntax(tmp.path().to_str().unwrap()).unwrap();
        assert_eq!(violations.len(), 1);
        assert!(violations[0].contains("[Ring 0]"));
    }

    #[test]
    fn test_check_syntax_coupling_does_not_block() {
        // 高 fan-out 不應被 Ring 0 阻擋
        let code = b"import a\nimport b\nimport c\nimport d\nimport e\nimport f\nimport g\nimport h\nimport i\nimport j\nimport k\nimport l\nimport m\nimport n\nimport o\nimport p\n";
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        tmp.write_all(code).unwrap();
        let violations = check_syntax(tmp.path().to_str().unwrap()).unwrap();
        assert!(violations.is_empty(), "High fan-out alone must NOT trigger Ring 0");
    }

    #[test]
    fn test_check_syntax_deep_chain_does_not_block() {
        // 深 chain depth 不應被 Ring 0 阻擋
        let code = b"a.b().c().d().e().f()\n";
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        tmp.write_all(code).unwrap();
        let violations = check_syntax(tmp.path().to_str().unwrap()).unwrap();
        assert!(violations.is_empty(), "Deep chain alone must NOT trigger Ring 0");
    }
}
```

- [ ] **步驟 2：執行 Rust 測試確認通過**

```bash
cd aegis-core-rs && cargo test enforcement -- --nocapture
```

預期：4 個測試全部通過

- [ ] **步驟 3：Commit**

```bash
git add aegis-core-rs/src/enforcement.rs
git commit -m "feat(enforcement): add check_syntax() as Ring 0 syntax-only enforcer"
```

---

### 任務 4：更新 `lib.rs`，移除 `policy_validator`

**Files:**
- 修改：`aegis-core-rs/src/lib.rs`
- 刪除：`aegis-core-rs/src/policy_validator.rs`

- [ ] **步驟 1：更新 `lib.rs`**

```rust
use pyo3::prelude::*;

mod models;
mod graph_engine;
pub mod ts_parser;
pub mod ast_parser;
pub mod enforcement;
pub mod signal_layer;

#[pyfunction]
fn ring0_status() -> PyResult<String> {
    Ok("Ring 0 Rust Core Initialized".to_string())
}

#[pymodule]
fn aegis_core_rs(_py: Python, m: &PyModule) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(ring0_status, m)?)?;
    m.add_function(wrap_pyfunction!(ts_parser::extract_ts_imports, m)?)?;
    m.add_class::<models::Violation>()?;
    m.add_class::<models::Signal>()?;
    m.add_class::<graph_engine::DependencyGraph>()?;
    m.add_class::<ast_parser::AstMetrics>()?;
    m.add_function(wrap_pyfunction!(ast_parser::analyze_file, m)?)?;
    m.add_function(wrap_pyfunction!(ast_parser::get_imports, m)?)?;
    m.add_function(wrap_pyfunction!(enforcement::check_syntax, m)?)?;
    m.add_function(wrap_pyfunction!(signal_layer::extract_signals, m)?)?;
    Ok(())
}
```

- [ ] **步驟 2：刪除 `policy_validator.rs`**

```bash
rm aegis-core-rs/src/policy_validator.rs
```

- [ ] **步驟 3：Build 確認編譯通過**

```bash
cd aegis-core-rs && cargo build 2>&1
```

預期：`Compiling aegis-core-rs`...`Finished`，無 error

- [ ] **步驟 4：重新安裝 Python wheel**

```bash
cd /home/a108222024/harness/aegis && maturin develop --manifest-path aegis-core-rs/Cargo.toml
```

預期：`Installed aegis-core-rs`

- [ ] **步驟 5：驗證新函數可從 Python 調用**

```bash
cd /home/a108222024/harness/aegis && .venv/bin/python -c "
import aegis_core_rs, tempfile, os
tmp = tempfile.NamedTemporaryFile(suffix='.py', delete=False)
tmp.write(b'import os\nimport sys\n')
tmp.flush()
tmp.close()
print('check_syntax:', aegis_core_rs.check_syntax(tmp.name))
print('extract_signals:', aegis_core_rs.extract_signals(tmp.name))
os.unlink(tmp.name)
"
```

預期輸出：
```
check_syntax: []
extract_signals: [Signal(fan_out = 2.0 @ ...), Signal(max_chain_depth = 0.0 @ ...)]
```

- [ ] **步驟 6：Commit**

```bash
git add aegis-core-rs/src/lib.rs
git rm aegis-core-rs/src/policy_validator.rs
git commit -m "refactor(lib): expose enforcement/signal_layer, remove policy_validator"
```

---

### 任務 5：新增 `aegis/layers/semantic_ir.py`（語義 IR 層）

**Files:**
- 新增：`aegis/layers/__init__.py`
- 新增：`aegis/layers/semantic_ir.py`
- 新增：`tests/test_semantic_ir.py`

Semantic IR 將 AST 解析結果轉成語言無關的中間表示，讓 Graph/Signal 層與語言語法解耦。

- [ ] **步驟 1：寫失敗的測試**

建立 `tests/test_semantic_ir.py`：

```python
import pytest
import tempfile, os
from aegis.layers.semantic_ir import SemanticNode, IRBuilder

def test_ir_builder_extracts_dependency_nodes(tmp_path):
    f = tmp_path / "app.py"
    f.write_text("import os\nfrom typing import List\n")
    
    builder = IRBuilder()
    nodes = builder.build_from_file(str(f))
    
    dep_nodes = [n for n in nodes if n.type == "dependency"]
    assert len(dep_nodes) == 2
    names = {n.name for n in dep_nodes}
    assert "os" in names
    assert "typing" in names

def test_ir_builder_stores_file_path(tmp_path):
    f = tmp_path / "mod.py"
    f.write_text("import sys\n")
    
    builder = IRBuilder()
    nodes = builder.build_from_file(str(f))
    
    assert all(n.file_path == str(f) for n in nodes)

def test_ir_builder_empty_file(tmp_path):
    f = tmp_path / "empty.py"
    f.write_text("")
    
    builder = IRBuilder()
    nodes = builder.build_from_file(str(f))
    
    assert nodes == []

def test_semantic_node_to_dict():
    node = SemanticNode(type="dependency", file_path="/a.py", name="os", metadata={})
    d = node.to_dict()
    assert d["type"] == "dependency"
    assert d["name"] == "os"
    assert d["file_path"] == "/a.py"
```

- [ ] **步驟 2：執行確認失敗**

```bash
cd /home/a108222024/harness/aegis && .venv/bin/pytest tests/test_semantic_ir.py -v
```

預期：`ModuleNotFoundError: No module named 'aegis.layers'`

- [ ] **步驟 3：實現 `semantic_ir.py`**

建立 `aegis/layers/__init__.py`（空白）。

建立 `aegis/layers/semantic_ir.py`：

```python
from dataclasses import dataclass, field
from typing import Literal
import aegis_core_rs

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
        imports = aegis_core_rs.get_imports(filepath)
        return [
            SemanticNode(type="dependency", file_path=filepath, name=imp)
            for imp in imports
        ]
```

- [ ] **步驟 4：執行測試確認通過**

```bash
cd /home/a108222024/harness/aegis && .venv/bin/pytest tests/test_semantic_ir.py -v
```

預期：4 個測試全部通過

- [ ] **步驟 5：Commit**

```bash
git add aegis/layers/__init__.py aegis/layers/semantic_ir.py tests/test_semantic_ir.py
git commit -m "feat(semantic_ir): add IRBuilder and SemanticNode as language-agnostic IR layer"
```

---

### 任務 6：新增 `aegis/layers/signal_layer.py`（Python Signal 層）

**Files:**
- 新增：`aegis/layers/signal_layer.py`
- 新增：`tests/test_signal_layer.py`

SignalLayer 包裝 Rust `extract_signals()`，並提供格式化輸出供 LLM 使用。

- [ ] **步驟 1：寫失敗的測試**

建立 `tests/test_signal_layer.py`：

```python
import pytest
import tempfile, os
from aegis.layers.signal_layer import SignalLayer

def test_signal_layer_extracts_fan_out(tmp_path):
    f = tmp_path / "app.py"
    f.write_text("import os\nimport sys\nfrom typing import List\n")
    
    layer = SignalLayer()
    signals = layer.extract(str(f))
    
    names = {s.name for s in signals}
    assert "fan_out" in names
    assert "max_chain_depth" in names

def test_signal_layer_fan_out_value(tmp_path):
    f = tmp_path / "app.py"
    f.write_text("import os\nimport sys\n")
    
    layer = SignalLayer()
    signals = layer.extract(str(f))
    
    fan_out = next(s for s in signals if s.name == "fan_out")
    assert fan_out.value == 2.0

def test_signal_layer_format_for_llm(tmp_path):
    f = tmp_path / "app.py"
    f.write_text("import os\nimport sys\n")
    
    layer = SignalLayer()
    signals = layer.extract(str(f))
    text = layer.format_for_llm(signals)
    
    assert "fan_out" in text
    assert "max_chain_depth" in text
    assert "Signal" in text or "signal" in text.lower()

def test_signal_layer_never_raises_on_valid_code(tmp_path):
    f = tmp_path / "app.py"
    f.write_text("x = 1\n")
    
    layer = SignalLayer()
    # 即使沒有 imports 也不應 raise
    signals = layer.extract(str(f))
    assert isinstance(signals, list)

def test_format_for_llm_empty_signals():
    layer = SignalLayer()
    text = layer.format_for_llm([])
    assert isinstance(text, str)
```

- [ ] **步驟 2：執行確認失敗**

```bash
cd /home/a108222024/harness/aegis && .venv/bin/pytest tests/test_signal_layer.py -v
```

預期：`ModuleNotFoundError: No module named 'aegis.layers.signal_layer'`

- [ ] **步驟 3：實現 `signal_layer.py`**

建立 `aegis/layers/signal_layer.py`：

```python
import aegis_core_rs

class SignalLayer:
    def extract(self, filepath: str) -> list:
        return aegis_core_rs.extract_signals(filepath)

    def format_for_llm(self, signals: list) -> str:
        if not signals:
            return "No structural signals detected."
        lines = ["## Structural Signals (Ring 0.5 — Observations Only)"]
        for sig in signals:
            lines.append(f"- **{sig.name}** = {sig.value:.0f}  ({sig.description})")
        lines.append("\n> These signals are observations, not violations. Use them to guide code quality decisions.")
        return "\n".join(lines)
```

- [ ] **步驟 4：執行測試確認通過**

```bash
cd /home/a108222024/harness/aegis && .venv/bin/pytest tests/test_signal_layer.py -v
```

預期：5 個測試全部通過

- [ ] **步驟 5：Commit**

```bash
git add aegis/layers/signal_layer.py tests/test_signal_layer.py
git commit -m "feat(signal_layer): add SignalLayer wrapper with format_for_llm()"
```

---

### 任務 7：新增 `aegis/layers/enforcement.py`（Python Ring 0 執行層）

**Files:**
- 新增：`aegis/layers/enforcement.py`
- 新增：`tests/test_enforcement.py`

Ring0Enforcer 整合 file-level syntax check 和 project-level circular dep check。

- [ ] **步驟 1：寫失敗的測試**

建立 `tests/test_enforcement.py`：

```python
import pytest
from pathlib import Path
from aegis.layers.enforcement import Ring0Enforcer

def test_ring0_valid_file_passes(tmp_path):
    f = tmp_path / "ok.py"
    f.write_text("def hello():\n    return 42\n")
    
    enforcer = Ring0Enforcer()
    violations = enforcer.check_file(str(f))
    
    assert violations == []

def test_ring0_syntax_error_blocks(tmp_path):
    f = tmp_path / "bad.py"
    f.write_text("def err(\n")
    
    enforcer = Ring0Enforcer()
    violations = enforcer.check_file(str(f))
    
    assert len(violations) == 1
    assert "[Ring 0]" in violations[0]

def test_ring0_high_fan_out_does_not_block(tmp_path):
    imports = "\n".join(f"import mod_{i}" for i in range(20))
    f = tmp_path / "heavy.py"
    f.write_text(imports + "\n")
    
    enforcer = Ring0Enforcer()
    violations = enforcer.check_file(str(f))
    
    assert violations == [], "Ring 0 must NOT block on fan-out alone"

def test_ring0_deep_chain_does_not_block(tmp_path):
    f = tmp_path / "chain.py"
    f.write_text("a.b().c().d().e().f()\n")
    
    enforcer = Ring0Enforcer()
    violations = enforcer.check_file(str(f))
    
    assert violations == [], "Ring 0 must NOT block on chain depth alone"

def test_ring0_circular_dep_blocks(tmp_path):
    (tmp_path / "mod_a.py").write_text("from mod_b import Foo\n")
    (tmp_path / "mod_b.py").write_text("from mod_a import Bar\n")
    
    enforcer = Ring0Enforcer()
    py_files = [str(tmp_path / "mod_a.py"), str(tmp_path / "mod_b.py")]
    violations = enforcer.check_project(py_files, root=str(tmp_path))
    
    assert len(violations) == 1
    assert "circular" in violations[0].lower()

def test_ring0_no_circular_dep_passes(tmp_path):
    (tmp_path / "mod_a.py").write_text("from mod_b import Foo\n")
    (tmp_path / "mod_b.py").write_text("x = 1\n")
    
    enforcer = Ring0Enforcer()
    py_files = [str(tmp_path / "mod_a.py"), str(tmp_path / "mod_b.py")]
    violations = enforcer.check_project(py_files, root=str(tmp_path))
    
    assert violations == []
```

- [ ] **步驟 2：執行確認失敗**

```bash
cd /home/a108222024/harness/aegis && .venv/bin/pytest tests/test_enforcement.py -v
```

預期：`ModuleNotFoundError: No module named 'aegis.layers.enforcement'`

- [ ] **步驟 3：實現 `enforcement.py`**

建立 `aegis/layers/enforcement.py`：

```python
from pathlib import Path
import aegis_core_rs

def _build_module_map(root: str, py_files: list[str]) -> dict[str, str]:
    root_path = Path(root).resolve()
    module_map = {}
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

class Ring0Enforcer:
    def check_file(self, filepath: str) -> list[str]:
        return aegis_core_rs.check_syntax(filepath)

    def check_project(self, py_files: list[str], root: str) -> list[str]:
        if len(py_files) < 2:
            return []
        module_map = _build_module_map(root, py_files)
        edges = []
        for py_file in py_files:
            try:
                imports = aegis_core_rs.get_imports(py_file)
                for imp in imports:
                    if imp in module_map:
                        edges.append((py_file, module_map[imp]))
            except Exception:
                pass
        if not edges:
            return []
        dg = aegis_core_rs.DependencyGraph()
        dg.build_from_edges(edges)
        if dg.check_circular_dependency():
            return ["[Ring 0] Circular dependency detected. Modules form a cycle (A→B→A). Extract a shared interface to break the loop."]
        return []
```

- [ ] **步驟 4：執行測試確認通過**

```bash
cd /home/a108222024/harness/aegis && .venv/bin/pytest tests/test_enforcement.py -v
```

預期：6 個測試全部通過

- [ ] **步驟 5：Commit**

```bash
git add aegis/layers/enforcement.py tests/test_enforcement.py
git commit -m "feat(enforcement): add Ring0Enforcer with check_file() and check_project()"
```

---

### 任務 8：重構 `llm_gateway.py`（注入 Signal 至 LLM 上下文）

**Files:**
- 修改：`aegis/agents/llm_gateway.py`
- 修改：`tests/test_llm_gateway.py`

將舊的 `AegisCoreValidator`（混合 coupling/demeter enforcement）替換為：
- `Ring0Validator`：只阻擋 syntax 錯誤（Ring 0）
- `LLMGateway` 在 prompt 中注入 signals（Ring 0.5）

- [ ] **步驟 1：查看現有 `test_llm_gateway.py`**

```bash
cat tests/test_llm_gateway.py
```

- [ ] **步驟 2：重寫 `llm_gateway.py`**

```python
from typing import Protocol, Optional
import os
import tempfile
import re
import aegis_core_rs
from aegis.layers.signal_layer import SignalLayer

class LLMProvider(Protocol):
    def generate(self, prompt: str) -> str: ...

class Ring0Validator:
    """Validates generated code against Ring 0 rules only (syntax)."""
    def validate(self, text: str) -> list[str]:
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
                # 純對話文字誤判為程式碼時放行
                if not re.search(r"\b(def|import|class|from|return|if|for|while)\b", code):
                    return []
            return violations
        finally:
            if os.path.exists(path):
                os.unlink(path)

class SignalContextBuilder:
    """Extracts Ring 0.5 signals from generated code and formats them for LLM context."""
    def __init__(self):
        self._layer = SignalLayer()

    def build_context(self, text: str) -> str:
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
            signals = self._layer.extract(path)
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
    ):
        self.llm_provider = llm_provider
        self.validator = validator or Ring0Validator()
        self.signal_builder = signal_builder or SignalContextBuilder()

    def generate_and_validate(self, prompt: str, max_retries: int = 3) -> str:
        current_prompt = prompt
        last_violations: list[str] = []

        for _ in range(max_retries):
            code = self.llm_provider.generate(current_prompt)
            violations = self.validator.validate(code)

            if not violations:
                # Append signal context for next turn (Ring 0.5 feedback loop)
                signal_ctx = self.signal_builder.build_context(code)
                if signal_ctx:
                    # Return code with signals appended as comment block
                    return code + f"\n\n# --- Aegis Signals ---\n# {signal_ctx.replace(chr(10), chr(10) + '# ')}"
                return code

            current_prompt = PromptFormatter.format_retry(current_prompt, violations)
            last_violations = violations

        violation_text = "\n".join(f"- {v}" for v in last_violations)
        raise RuntimeError(
            f"Failed to generate valid code after {max_retries} attempts.\n{violation_text}"
        )
```

- [ ] **步驟 3：更新 `tests/test_llm_gateway.py`**

讀取並更新現有測試，移除使用 `AegisCoreValidator` 和 `validate_file_policy` 的測試，改為測試 `Ring0Validator`：

```python
import pytest
from unittest.mock import MagicMock
from aegis.agents.llm_gateway import LLMGateway, Ring0Validator, PromptFormatter

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
    # 第一次回傳語法錯誤，第二次回傳正確程式碼
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
```

- [ ] **步驟 4：執行測試確認通過**

```bash
cd /home/a108222024/harness/aegis && .venv/bin/pytest tests/test_llm_gateway.py -v
```

預期：6 個測試全部通過

- [ ] **步驟 5：Commit**

```bash
git add aegis/agents/llm_gateway.py tests/test_llm_gateway.py
git commit -m "refactor(llm_gateway): replace AegisCoreValidator with Ring0Validator + SignalContextBuilder"
```

---

### 任務 9：更新 `cli.py`（使用新 Layer pipeline）

**Files:**
- 修改：`aegis/cli.py`

用 `Ring0Enforcer` 取代舊的 `validate_file_policy` 呼叫；顯示 signals 作為資訊性輸出。

- [ ] **步驟 1：重寫 `check` command**

替換 `aegis/cli.py` 中的 `check` 函數：

```python
@cli.command()
@click.argument('path', type=click.Path(exists=True))
@click.option('--signals', is_flag=True, default=False, help='Show Ring 0.5 structural signals.')
def check(path, signals):
    """Check Ring 0 architectural rules on PATH."""
    from aegis.layers.enforcement import Ring0Enforcer
    from aegis.layers.signal_layer import SignalLayer

    enforcer = Ring0Enforcer()
    signal_layer = SignalLayer()
    has_violations = False

    py_files = []
    if os.path.isfile(path):
        if path.endswith('.py'):
            py_files.append(path)
    else:
        for root, dirs, files in os.walk(path):
            dirs[:] = [d for d in dirs if not d.startswith('.')]
            for file in files:
                if file.endswith('.py'):
                    py_files.append(os.path.join(root, file))

    if not py_files:
        click.echo(f"No Python files found in {path}")
        return

    # Ring 0: file-level syntax check
    for f in py_files:
        violations = enforcer.check_file(f)
        if violations:
            has_violations = True
            for v in violations:
                click.echo(v)

    # Ring 0: project-level circular dep check
    root = path if os.path.isdir(path) else os.path.dirname(path)
    project_violations = enforcer.check_project(py_files, root=root)
    if project_violations:
        has_violations = True
        for v in project_violations:
            click.echo(v)

    # Ring 0.5: optional signal display (never blocks)
    if signals:
        click.echo("\n--- Ring 0.5 Structural Signals ---")
        for f in py_files:
            try:
                sigs = signal_layer.extract(f)
                if sigs:
                    click.echo(f"\n{f}:")
                    for sig in sigs:
                        click.echo(f"  {sig.name} = {sig.value:.0f}  ({sig.description})")
            except Exception as e:
                click.echo(f"  Warning: could not extract signals from {f}: {e}", err=True)

    if has_violations:
        click.echo("Aegis check failed.")
        raise SystemExit(1)
    else:
        click.echo("Aegis check passed.")
```

移除舊的 `--policy` option 和 `_build_module_map` helper（已移至 `enforcement.py`）。保留 `generate` 和 `chat` commands 不變（只更新 import）。

- [ ] **步驟 2：執行 CLI 測試**

```bash
cd /home/a108222024/harness/aegis && .venv/bin/pytest tests/test_cli.py -v
```

預期：所有現有 CLI tests 通過（需要更新 circular dep 相關 test 中的 `--policy` 參數）

- [ ] **步驟 3：手動測試**

```bash
cd /home/a108222024/harness/aegis && .venv/bin/python -m aegis.cli check aegis/ --signals
```

預期：
- `Aegis check passed.`
- 若有 `--signals`，顯示各檔案的 fan_out 和 max_chain_depth

- [ ] **步驟 4：Commit**

```bash
git add aegis/cli.py
git commit -m "refactor(cli): replace policy-based check with Ring0Enforcer + optional signal display"
```

---

### 任務 10：更新測試 — 刪除 `test_policy_validator.py`，修正 `test_core_integration.py`

**Files:**
- 刪除：`tests/test_policy_validator.py`
- 修改：`tests/test_core_integration.py`

- [ ] **步驟 1：刪除 `test_policy_validator.py`**

```bash
git rm tests/test_policy_validator.py
```

- [ ] **步驟 2：更新 `test_core_integration.py`**

移除 `validate_file_policy` 相關測試，改為測試新的 `check_syntax` + `extract_signals`：

```python
import aegis_core_rs
import tempfile, os

def test_ring0_status():
    status = aegis_core_rs.ring0_status()
    assert status == "Ring 0 Rust Core Initialized"

def test_dependency_graph_cycle():
    dg = aegis_core_rs.DependencyGraph()
    dg.build_from_edges([("A.py", "B.py"), ("B.py", "C.py"), ("C.py", "A.py")])
    assert dg.check_circular_dependency() is True

def test_dependency_graph_no_cycle():
    dg = aegis_core_rs.DependencyGraph()
    dg.build_from_edges([("A.py", "B.py"), ("B.py", "C.py")])
    assert dg.check_circular_dependency() is False

def test_fan_out_signal():
    dg = aegis_core_rs.DependencyGraph()
    dg.build_from_edges([
        ("main.py", "auth.py"), ("main.py", "db.py"), ("main.py", "utils.py"),
        ("app.py", "db.py"),
    ])
    violations = dg.check_max_fan_out(2)
    assert len(violations) == 1
    assert violations[0][0] == "main.py"

def test_check_syntax_valid():
    with tempfile.NamedTemporaryFile(suffix=".py", delete=False) as f:
        f.write(b"def hello():\n    return 42\n")
        f.flush()
    violations = aegis_core_rs.check_syntax(f.name)
    os.unlink(f.name)
    assert violations == []

def test_check_syntax_invalid():
    with tempfile.NamedTemporaryFile(suffix=".py", delete=False) as f:
        f.write(b"def err(\n")
        f.flush()
    violations = aegis_core_rs.check_syntax(f.name)
    os.unlink(f.name)
    assert len(violations) == 1
    assert "[Ring 0]" in violations[0]

def test_extract_signals_returns_expected_names():
    with tempfile.NamedTemporaryFile(suffix=".py", delete=False) as f:
        f.write(b"import os\nimport sys\n")
        f.flush()
    signals = aegis_core_rs.extract_signals(f.name)
    os.unlink(f.name)
    names = {s.name for s in signals}
    assert "fan_out" in names
    assert "max_chain_depth" in names

def test_ts_parser_integration():
    code = "import { auth } from './auth';\nimport React from 'react';\n"
    imports = aegis_core_rs.extract_ts_imports(code)
    assert "./auth" in imports
    assert "react" in imports
```

- [ ] **步驟 3：執行全套測試**

```bash
cd /home/a108222024/harness/aegis && .venv/bin/pytest tests/ -v --tb=short
```

預期：所有測試通過，無 `test_policy_validator.py` 相關測試

- [ ] **步驟 4：Commit**

```bash
git rm tests/test_policy_validator.py
git add tests/test_core_integration.py
git commit -m "test: remove test_policy_validator, update core_integration for new architecture"
```

---

### 任務 11：更新 `default_core_policy.yaml`（移除 coupling/demeter 強制執行）

**Files:**
- 修改：`templates/default_core_policy.yaml`

- [ ] **步驟 1：更新 YAML**

```yaml
# templates/default_core_policy.yaml
version: "2.0"
enforcement_level: "ring0_only"

ring0:
  syntax_validity:
    enabled: true
    message: "系統內核攔截：程式碼存在致命語法錯誤，無法被 Tree-sitter 解析。"
  anti_circular_dependency:
    enabled: true
    message: "系統內核攔截：偵測到循環依賴 A→B→A。請提取共用介面以打破迴圈。"

ring0_5_signals:
  # 這些設定僅作為 LLM 決策的參考閾值，永遠不阻擋執行
  fan_out_advisory: 15
  max_chain_depth_advisory: 3
  note: "Signal 層觀察值由 LLM 判斷，非強制規則。"
```

- [ ] **步驟 2：確認 `aegis check` 不再讀取 YAML（已由 Ring0Enforcer 取代）**

```bash
cd /home/a108222024/harness/aegis && .venv/bin/python -m aegis.cli check --help
```

確認 `--policy` option 已移除。

- [ ] **步驟 3：Commit**

```bash
git add templates/default_core_policy.yaml
git commit -m "config: update policy yaml to reflect ring0-only enforcement + ring0.5 advisory signals"
```

---

### 任務 12：最終驗證

- [ ] **步驟 1：執行全套測試**

```bash
cd /home/a108222024/harness/aegis && .venv/bin/pytest tests/ -v
```

預期：所有測試通過

- [ ] **步驟 2：Rust 測試**

```bash
cd /home/a108222024/harness/aegis/aegis-core-rs && cargo test
```

預期：所有 Rust unit tests 通過

- [ ] **步驟 3：CLI 端對端測試**

```bash
# 測試 1: 正常專案通過
cd /home/a108222024/harness/aegis && .venv/bin/python -m aegis.cli check aegis/
# 預期: Aegis check passed.

# 測試 2: 顯示 signals
.venv/bin/python -m aegis.cli check aegis/layers/ --signals
# 預期: 顯示 fan_out 和 max_chain_depth

# 測試 3: 循環依賴
python -c "
import tempfile, os, subprocess
d = tempfile.mkdtemp()
open(os.path.join(d, 'a.py'), 'w').write('from b import X\n')
open(os.path.join(d, 'b.py'), 'w').write('from a import Y\n')
r = subprocess.run(['.venv/bin/python', '-m', 'aegis.cli', 'check', d], capture_output=True, text=True)
print('exit:', r.returncode, '| output:', r.stdout.strip())
assert r.returncode == 1
assert 'Circular' in r.stdout
print('PASS')
"
```

- [ ] **步驟 4：最終 Commit（若有殘餘未提交的變更）**

```bash
git status
# 確認無殘餘改動
```

---

## 規格覆蓋度自檢

| 架構要求 | 對應任務 | 狀態 |
|---------|---------|------|
| Ring 0：Syntax errors → BLOCK | 任務 3, 7, 9 | ✅ |
| Ring 0：Circular dep → BLOCK | 任務 7, 9 | ✅ |
| Ring 0.5：fan-out → Signal only | 任務 2, 6 | ✅ |
| Ring 0.5：chain depth → Signal only | 任務 2, 6 | ✅ |
| Ring 0.5：永不 block | 任務 3 `test_check_syntax_coupling_does_not_block` | ✅ |
| Semantic IR 層 | 任務 5 | ✅ |
| Graph 層保留 | 任務 4（graph_engine 保留）| ✅ |
| LLM 接收 signals | 任務 8 | ✅ |
| `policy_validator.rs` 移除 | 任務 4 | ✅ |
| Multi-language 基礎（TS parser 保留）| 任務 4 | ✅ |
