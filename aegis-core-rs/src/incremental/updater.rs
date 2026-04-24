use pyo3::prelude::*;
use crate::incremental::cache::FileCache;
use crate::ir::builder::build_ir;
use crate::ir::model::IrNode;

/// Incremental updater: only re-parses files that changed since last run.
#[pyclass]
pub struct IncrementalUpdater {
    cache: FileCache,
}

#[pymethods]
impl IncrementalUpdater {
    #[new]
    pub fn new() -> Self {
        Self { cache: FileCache::new() }
    }

    /// Returns IR nodes only for files that changed. Skips unchanged files.
    pub fn update(&mut self, filepaths: Vec<String>) -> PyResult<Vec<IrNode>> {
        let mut result = Vec::new();
        for path in filepaths {
            if self.cache.is_stale(&path) {
                match build_ir(&path) {
                    Ok(nodes) => result.extend(nodes),
                    Err(e) => eprintln!("[incremental] skipping {path}: {e}"),
                }
            }
        }
        Ok(result)
    }

    pub fn invalidate(&mut self, filepath: &str) {
        self.cache.invalidate(filepath);
    }
}
