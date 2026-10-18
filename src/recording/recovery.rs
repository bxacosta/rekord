//! Startup check for takes that were interrupted (crash, power cut, killed process).

use super::wav;
use std::path::{Path, PathBuf};

/// Repairs every interrupted recording in `dir` and returns the files that were fixed.
pub fn recover_dir(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut repaired: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("wav")))
        .filter(|path| wav::repair(path).unwrap_or(false))
        .collect();
    repaired.sort();
    repaired
}
