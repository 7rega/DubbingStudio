//! In-tree tempfile helper for unit tests (zero external dependencies).

use std::path::{Path, PathBuf};

pub struct TempDir(PathBuf);

impl TempDir {
    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn tempdir() -> std::io::Result<TempDir> {
    let p = std::env::temp_dir().join(format!("dubtmp_{}_{}", std::process::id(), uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&p)?;
    Ok(TempDir(p))
}
