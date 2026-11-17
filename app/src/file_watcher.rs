//! Filesystem invalidations for Git review, the directory browser, and session logs.

use std::collections::HashSet;
use std::path::PathBuf;

#[cfg(all(windows, feature = "local_fs"))]
pub(crate) mod wsl;

#[cfg(all(windows, feature = "local_fs"))]
pub(crate) fn file_events(
    path: &std::path::Path,
    executor: &std::sync::Arc<warpui::r#async::executor::Background>,
) -> futures::stream::BoxStream<'static, ()> {
    use futures::StreamExt;
    use notify_debouncer_full::notify::{self, RecursiveMode, Watcher};

    if let Some(events) = wsl::file_events(path, executor) {
        return events.boxed();
    }
    let path = path.to_path_buf();
    async_stream::stream! {
        let (wake, events) = async_channel::bounded(1);
        let watcher = blocking::unblock(move || {
            let mut watcher =
                notify::recommended_watcher(move |event: notify::Result<notify::Event>| match event {
                    Ok(_) => { let _ = wake.try_send(()); }
                    Err(error) => {
                        log::warn!("Session log watcher stopped: {error:#}");
                        wake.close();
                    }
                })?;
            watcher.watch(&path, RecursiveMode::NonRecursive)?;
            Ok::<_, notify::Error>(watcher)
        }).await;
        let _watcher = match watcher {
            Ok(watcher) => watcher,
            Err(error) => {
                log::warn!("Cannot watch session log: {error:#}");
                return;
            }
        };
        // Catch any writes between the initial read and watch registration.
        yield ();
        while events.recv().await.is_ok() {
            yield ();
        }
    }
    .boxed()
}

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
