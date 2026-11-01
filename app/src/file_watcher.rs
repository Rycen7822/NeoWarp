//! Filesystem invalidations shared by Git review and the directory browser.

use std::collections::HashSet;
use std::path::PathBuf;

#[cfg(all(windows, feature = "local_fs"))]
pub(crate) mod wsl;

#[derive(Debug, Clone, Default)]
#[cfg_attr(all(windows, feature = "local_fs"), derive(serde::Deserialize))]
pub struct FileChanges {
    /// Paths relative to the watched root. A full invalidation supersedes them.
    pub paths: HashSet<PathBuf>,
    pub full: bool,
    pub git_changed: bool,
}

#[cfg(all(windows, feature = "local_fs"))]
impl FileChanges {
    fn merge(&mut self, update: Self) {
        self.full |= update.full;
        self.git_changed |= update.git_changed;
        self.paths.extend(update.paths);
        if self.full || self.paths.len() > 1024 {
            self.full = true;
            self.paths.clear();
        }
    }
}
