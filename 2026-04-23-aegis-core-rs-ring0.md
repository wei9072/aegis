# Aegis Ring 0 (Rust Core) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the high-performance Rust core (`aegis-core-rs`) to enforce the four `core_policy.yaml` rules: AST Integrity, Anti-Circular Dependency, Coupling Limits (Fan-out), and Law of Demeter (Method Chain Depth). It exposes a Python API via `PyO3` / `Maturin`.

**Architecture:** The Ring 0 component handles computationally heavy tasks. It uses `tree-sitter` for AST parsing (Syntax Integrity, Fan-out via imports, Chain Depth) and `petgraph` for DAG analysis (Circular Dependency detection). It parses the exact `core_policy.yaml` definitions and applies these rules at the Ring 0 level.

**Tech Stack:** Rust, PyO3, Maturin, Tree-sitter, Petgraph, Serde (JSON/YAML).

---

### Task 1: Initialize the PyO3 Project & Dependencies

**Files:**
- Create: `aegis-core-rs/Cargo.toml`
- Create: `aegis-core-rs/pyproject.toml`
- Create: `aegis-core-rs/src/lib.rs`

- [ ] **Step 1: Create Cargo.toml**
Create `aegis-core-rs/Cargo.toml` with necessary dependencies.
```toml
[package]
name = "aegis-core-rs"
version = "0.1.0"
edition = "2021"

[lib]
name = "aegis_core_rs"
crate-type = ["cdylib"]

[dependencies]
pyo3 = { version = "0.20.2", features = ["extension-module"] }
tree-sitter = "0.20.10"
tree-sitter-typescript = "0.20.5"
tree-sitter-python = "0.20.4"
tree-sitter-go = "0.20.0"
petgraph = "0.6.4"
serde = { version = "1.0", features = ["derive"] }
serde_yaml = "0.9"
```

- [ ] **Step 2: Create basic pyproject.toml**
Create `aegis-core-rs/pyproject.toml`.
```toml
[build-system]
requires = ["maturin>=1.4,<2.0"]
build-backend = "maturin"

[project]
name = "aegis-core-rs"
version = "0.1.0"
description = "Aegis Ring 0 Core enforcing architectural policies"
requires-python = ">=3.9"
```

- [ ] **Step 3: Setup basic PyO3 lib.rs**
Create `aegis-core-rs/src/lib.rs` with a simple test function.
```rust
use pyo3::prelude::*;

#[pyfunction]
fn ring0_status() -> PyResult<String> {
    Ok("Ring 0 Rust Core Initialized".to_string())
}

#[pymodule]
fn aegis_core_rs(_py: Python, m: &PyModule) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(ring0_status, m)?)?;
    Ok(())
}
```

### Task 2: Setup Tree-sitter Queries for Policies

**Files:**
- Create: `aegis-core-rs/queries/python.scm`

- [ ] **Step 1: Create python.scm for Imports and Method Chains**
Create `aegis-core-rs/queries/python.scm` to detect imports (Fan-out) and method chains (Law of Demeter).
```scm
; Extract import statements for Fan-out
(import_statement name: (dotted_name) @import_path)
(import_from_statement module_name: (dotted_name) @import_path)

; Extract attribute/call chains for Demeter
; Match chains like a.b.c.d or a().b().c()
(attribute attribute: (identifier) @attr_chain)
```

### Task 3: Implement AST Parser Module (Syntax, Demeter & Fan-out)

**Files:**
- Create: `aegis-core-rs/src/ast_parser.rs`
- Modify: `aegis-core-rs/src/lib.rs`

- [ ] **Step 1: Write ast_parser.rs**
Implement logic to check for syntax errors, calculate fan-out, and check max chain depth.
```rust
use pyo3::prelude::*;
use std::fs;
use tree_sitter::{Parser, Language};

extern "C" { fn tree_sitter_python() -> Language; }

#[pyclass]
#[derive(Clone)]
pub struct AstMetrics {
    #[pyo3(get)]
    pub has_syntax_error: bool,
    #[pyo3(get)]
    pub fan_out: usize,
    #[pyo3(get)]
    pub max_chain_depth: usize,
}

#[pyfunction]
pub fn analyze_file(filepath: &str) -> PyResult<AstMetrics> {
    let code = fs::read_to_string(filepath)?;
    let mut parser = Parser::new();
    let language = unsafe { tree_sitter_python() };
    parser.set_language(language).unwrap();

    let tree = parser.parse(&code, None).unwrap();
    let has_syntax_error = tree.root_node().has_error();
    
    // Placeholder logic for AST deep-diving (fan_out and depth)
    // To be implemented rigorously during the task execution using cursors.
    Ok(AstMetrics {
        has_syntax_error,
        fan_out: 0, // TODO: Count imports
        max_chain_depth: 0, // TODO: Count depth
    })
}
```

- [ ] **Step 2: Export in lib.rs**
```rust
use pyo3::prelude::*;

mod ast_parser;

#[pyfunction]
fn ring0_status() -> PyResult<String> {
    Ok("Ring 0 Rust Core Initialized".to_string())
}

#[pymodule]
fn aegis_core_rs(_py: Python, m: &PyModule) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(ring0_status, m)?)?;
    m.add_class::<ast_parser::AstMetrics>()?;
    m.add_function(wrap_pyfunction!(ast_parser::analyze_file, m)?)?;
    Ok(())
}
```

### Task 4: Implement DAG Graph Engine (Circular Dependency)

**Files:**
- Create: `aegis-core-rs/src/graph_engine.rs`
- Modify: `aegis-core-rs/src/lib.rs`

- [ ] **Step 1: Write graph_engine.rs**
```rust
use pyo3::prelude::*;
use petgraph::graph::{DiGraph, NodeIndex};
use std::collections::HashMap;

#[pyclass]
pub struct DependencyGraph {
    graph: DiGraph<String, ()>,
    nodes: HashMap<String, NodeIndex>,
}

#[pymethods]
impl DependencyGraph {
    #[new]
    pub fn new() -> Self {
        DependencyGraph {
            graph: DiGraph::new(),
            nodes: HashMap::new(),
        }
    }

    pub fn add_dependency(&mut self, source: &str, target: &str) -> PyResult<()> {
        let src_idx = *self.nodes.entry(source.to_string()).or_insert_with(|| self.graph.add_node(source.to_string()));
        let tgt_idx = *self.nodes.entry(target.to_string()).or_insert_with(|| self.graph.add_node(target.to_string()));
        
        self.graph.add_edge(src_idx, tgt_idx, ());
        Ok(())
    }

    pub fn check_circular(&self) -> PyResult<bool> {
        Ok(petgraph::algo::is_cyclic_directed(&self.graph))
    }
}
```

- [ ] **Step 2: Export in lib.rs**
```rust
use pyo3::prelude::*;

mod ast_parser;
mod graph_engine;

#[pyfunction]
fn ring0_status() -> PyResult<String> {
    Ok("Ring 0 Rust Core Initialized".to_string())
}

#[pymodule]
fn aegis_core_rs(_py: Python, m: &PyModule) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(ring0_status, m)?)?;
    m.add_class::<ast_parser::AstMetrics>()?;
    m.add_function(wrap_pyfunction!(ast_parser::analyze_file, m)?)?;
    m.add_class::<graph_engine::DependencyGraph>()?;
    Ok(())
}
```

### Task 5: Implement Policy Validator (core_policy.yaml logic)

**Files:**
- Create: `aegis-core-rs/src/policy_validator.rs`
- Modify: `aegis-core-rs/src/lib.rs`

- [ ] **Step 1: Write policy_validator.rs**
Read the `core_policy.yaml` and evaluate the metrics against the rules.
```rust
use pyo3::prelude::*;
use serde::Deserialize;
use std::fs;
use crate::ast_parser::AstMetrics;

#[derive(Deserialize)]
struct CorePolicy {
    global_principles: GlobalPrinciples,
}

#[derive(Deserialize)]
struct GlobalPrinciples {
    syntax_validity: PolicyRule,
    anti_circular_dependency: PolicyRule,
    coupling_limits: CouplingLimits,
    method_chain_limits: MethodChainLimits,
}

#[derive(Deserialize)]
struct PolicyRule { enabled: bool, action: String, message: String }

#[derive(Deserialize)]
struct CouplingLimits { enabled: bool, max_fan_out_per_file: usize, action: String, message: String }

#[derive(Deserialize)]
struct MethodChainLimits { enabled: bool, max_chain_depth: usize, action: String, message: String }

#[pyfunction]
pub fn validate_file_policy(yaml_path: &str, metrics: &AstMetrics) -> PyResult<Vec<String>> {
    let yaml_content = fs::read_to_string(yaml_path)?;
    let policy: CorePolicy = serde_yaml::from_str(&yaml_content).unwrap();
    let mut violations = Vec::new();

    if policy.global_principles.syntax_validity.enabled && metrics.has_syntax_error {
        violations.push(policy.global_principles.syntax_validity.message.clone());
    }

    if policy.global_principles.coupling_limits.enabled && metrics.fan_out > policy.global_principles.coupling_limits.max_fan_out_per_file {
        violations.push(policy.global_principles.coupling_limits.message.clone());
    }

    if policy.global_principles.method_chain_limits.enabled && metrics.max_chain_depth > policy.global_principles.method_chain_limits.max_chain_depth {
        violations.push(policy.global_principles.method_chain_limits.message.clone());
    }

    Ok(violations)
}
```

- [ ] **Step 2: Export in lib.rs**
```rust
use pyo3::prelude::*;

mod ast_parser;
mod graph_engine;
mod policy_validator;

#[pyfunction]
fn ring0_status() -> PyResult<String> {
    Ok("Ring 0 Rust Core Initialized".to_string())
}

#[pymodule]
fn aegis_core_rs(_py: Python, m: &PyModule) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(ring0_status, m)?)?;
    m.add_class::<ast_parser::AstMetrics>()?;
    m.add_function(wrap_pyfunction!(ast_parser::analyze_file, m)?)?;
    m.add_class::<graph_engine::DependencyGraph>()?;
    m.add_function(wrap_pyfunction!(policy_validator::validate_file_policy, m)?)?;
    Ok(())
}
```