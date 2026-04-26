use std::collections::HashMap;
use std::time::SystemTime;

/// Tracks file modification times to avoid redundant re-parsing.
pub struct FileCache {
    mtimes: HashMap<String, SystemTime>,
}

impl FileCache {
    pub fn new() -> Self {
        Self { mtimes: HashMap::new() }
    }

    /// Returns true if the file has changed since last seen.
    pub fn is_stale(&mut self, filepath: &str) -> bool {
        let Ok(meta) = std::fs::metadata(filepath) else { return true };
        let Ok(mtime) = meta.modified() else { return true };
        match self.mtimes.get(filepath) {
            Some(&prev) if prev == mtime => false,
            _ => {
                self.mtimes.insert(filepath.to_string(), mtime);
                true
            }
        }
    }

    pub fn invalidate(&mut self, filepath: &str) {
        self.mtimes.remove(filepath);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_file_not_stale_after_read() {
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        tmp.write_all(b"x = 1").unwrap();
        tmp.flush().unwrap();
        let path = tmp.path().to_str().unwrap();
        let mut cache = FileCache::new();
        assert!(cache.is_stale(path));   // first time → stale
        assert!(!cache.is_stale(path));  // unchanged → not stale
    }

    #[test]
    fn test_invalidate_marks_stale() {
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        tmp.write_all(b"x = 1").unwrap();
        tmp.flush().unwrap();
        let path = tmp.path().to_str().unwrap();
        let mut cache = FileCache::new();
        cache.is_stale(path);
        cache.invalidate(path);
        assert!(cache.is_stale(path));
    }
}
