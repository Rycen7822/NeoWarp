//! Live, bounded tool diffs. No workspace reads or input barriers.
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use futures::future::{Either, select};
use futures::stream::{BoxStream, StreamExt};
use warp_terminal::model::BlockId;
use warpui::r#async::{SpawnedFutureHandle, Timer};
use warpui::{Entity, EntityId, ModelContext, SingletonEntity};

use crate::localization::tr;
use crate::project_sessions::ProjectSessions;
use crate::project_sessions::config::Provider;
use crate::project_sessions::tool_changes::Change;
use crate::project_sessions::turns::{self, Cursor, Observation, Source};
use crate::util::path::local_path_for_io;

pub(crate) struct LastTurn {
    pub(crate) root: PathBuf,
    pub(crate) block: BlockId,
    pub(crate) provider: Provider,
    terminal: EntityId,
    path: Option<PathBuf>,
    cursor: Option<Cursor>,
    since: u64,
    generation: u64,
    changes: BTreeMap<String, Change>,
    message: String,
    poll: Option<SpawnedFutureHandle>,
    log_events: Option<BoxStream<'static, ()>>,
    closed: bool,
}

impl LastTurn {
    pub(crate) fn new(
        root: PathBuf,
        source: Source,
        terminal: EntityId,
        ctx: &mut ModelContext<Self>,
    ) -> Self {
        let mut me = Self {
            root,
            block: source.block.clone(),
            provider: source.provider(),
            terminal,
            path: None,
            cursor: None,
            since: now(),
            generation: 0,
            changes: BTreeMap::new(),
            message: tr("Captured tool changes · partial coverage").into(),
            poll: None,
            log_events: None,
            closed: false,
        };
        if source.supports_turns() {
            me.schedule_poll(ctx);
        }
        me
    }

    pub(crate) fn label(&self) -> &str {
        &self.message
    }

    pub(crate) fn changes(&self) -> impl Iterator<Item = &Change> {
        self.changes.values()
    }

    pub(crate) fn change(&self, path: &str) -> Option<&Change> {
        self.changes.get(path)
    }

    pub(super) fn generation(&self) -> u64 {
        self.generation
    }

    pub(crate) fn end(&mut self, ctx: &mut ModelContext<Self>) {
        if let Some(task) = self.poll.take() {
            task.abort();
        }
        self.log_events = None;
        self.cursor = None;
        self.closed = true;
        self.message = tr("Session ended").into();
        ctx.emit(());
    }

    fn schedule_poll(&mut self, ctx: &mut ModelContext<Self>) {
        if self.poll.is_some() || self.closed {
            return;
        }
        let Some(source) = ProjectSessions::as_ref(ctx)
            .turn_source(self.terminal)
            .filter(|source| source.block == self.block)
        else {
            self.end(ctx);
            return;
        };
        let path = self.path.clone();
        let cursor = self.cursor.clone();
        let since = self.since;
        let mut log_events = self.log_events.take();
        // A full read may leave more records without a new file notification.
        let interval =
            if log_events.is_some() && !cursor.as_ref().is_some_and(|cursor| cursor.full_batch) {
                Duration::from_secs(2)
            } else {
                Duration::from_millis(500)
            };
        self.poll = Some(ctx.spawn(
            async move {
                let timer = Timer::after(interval);
                if let Some(events) = log_events.as_mut() {
                    if matches!(select(events.next(), timer).await, Either::Left((None, _))) {
                        log_events = None;
                    }
                } else {
                    timer.await;
                }
                let result = blocking::unblock(move || -> anyhow::Result<_> {
                    let path = match match path {
                        Some(path) => Some(path),
                        None => source.locate()?,
                    } {
                        Some(path) => path,
                        None => return Ok(None),
                    };
                    let cursor = match cursor {
                        Some(cursor) => cursor,
                        None => Cursor::tail(&path)?,
                    };
                    let (cursor, observations) =
                        turns::read(&path, source.provider(), cursor, since)?;
                    Ok(Some((path, cursor, observations, source)))
                })
                .await;
                (log_events, result)
            },
            |me, (log_events, result), ctx| {
                if me.closed {
                    return;
                }
                me.poll = None;
                me.log_events = log_events;
                match result {
                    Ok(Some((path, cursor, observations, source))) => {
                        #[cfg(windows)]
                        if me.path.is_none() {
                            me.log_events = Some(crate::file_watcher::file_events(
                                &path,
                                &ctx.background_executor(),
                            ));
                        }
                        me.path = Some(path);
                        me.cursor = Some(cursor);
                        me.message = tr("Captured tool changes · partial coverage").into();
                        if !observations.is_empty() {
                            me.apply_observations(observations, &source);
                            ctx.emit(());
                        }
                    }
                    Ok(None) => (),
                    Err(error) => {
                        me.message = format!("{}: {error:#}", tr("Unable to read tool changes"));
                        ctx.emit(());
                    }
                }
                me.schedule_poll(ctx);
            },
        ));
    }

    fn apply_observations(&mut self, observations: Vec<Observation>, source: &Source) {
        const SEPARATOR: &str = "\n@@ next tool operation @@\n";
        const MAX_TURN_BYTES: usize = 2 * 1024 * 1024;
        let mut bytes: usize = self.changes.values().map(|change| change.diff.len()).sum();
        for observation in observations {
            let mut change = match observation {
                Observation::Request => {
                    self.changes.clear();
                    self.generation = self.generation.wrapping_add(1);
                    bytes = 0;
                    continue;
                }
                Observation::Change(change) => change,
            };
            let Some(path) = source
                .native_path(&change.path)
                .and_then(|native| relative_path(&self.root, &native))
            else {
                continue;
            };
            if let Some(previous) = self.changes.get_mut(&path) {
                let appended = SEPARATOR.len() + change.diff.len();
                if previous.diff.len() + appended <= 64 * 1024 && bytes + appended <= MAX_TURN_BYTES
                {
                    previous.diff.push_str(SEPARATOR);
                    previous.diff.push_str(&change.diff);
                    previous.additions += change.additions;
                    previous.deletions += change.deletions;
                    bytes += appended;
                }
            } else if self.changes.len() < 128 && bytes + change.diff.len() <= MAX_TURN_BYTES {
                change.path = path.clone();
                bytes += change.diff.len();
                self.changes.insert(path, change);
            }
        }
    }
}

fn relative_path(root: &Path, native: &Path) -> Option<String> {
    // A Git repository root can be canonicalized to verbatim UNC while tool paths use ordinary UNC.
    let root = local_path_for_io(root);
    let native = local_path_for_io(native);
    let path = native.strip_prefix(root.as_ref()).ok()?;
    let mut relative = PathBuf::new();
    for part in path.components() {
        match part {
            Component::Normal(part) => relative.push(part),
            Component::CurDir => (),
            _ => return None,
        }
    }
    (!relative.as_os_str().is_empty()).then(|| relative.to_string_lossy().into_owned())
}

impl Entity for LastTurn {
    type Event = ();
}
impl Drop for LastTurn {
    fn drop(&mut self) {
        if let Some(task) = self.poll.take() {
            task.abort();
        }
    }
}

pub(crate) fn now() -> u64 {
    chrono::Utc::now().timestamp_millis().max(0) as u64
}

#[cfg(all(test, windows))]
#[path = "last_turn_tests.rs"]
mod tests;
