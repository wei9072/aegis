use pyo3::prelude::*;
use crate::graph::{dependency::DependencyGraph as Inner, cycle, traversal};

/// Python-facing wrapper around the internal DependencyGraph.
#[pyclass]
pub struct DependencyGraph {
    inner: Inner,
}

#[pymethods]
impl DependencyGraph {
    #[new]
    pub fn new() -> Self {
        Self { inner: Inner::new() }
    }

    pub fn build_from_edges(&mut self, edges: Vec<(String, String)>) {
        self.inner.build_from_edges(edges);
    }

    pub fn check_circular_dependency(&self) -> bool {
        cycle::has_cycle(&self.inner)
    }

    pub fn check_max_fan_out(&self, limit: usize) -> Vec<(String, usize)> {
        traversal::fan_out_violations(&self.inner, limit)
    }

    pub fn nodes(&self) -> Vec<String> {
        self.inner.nodes()
    }
}
