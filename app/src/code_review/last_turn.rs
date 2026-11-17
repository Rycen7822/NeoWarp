//! Live, bounded tool diffs. No workspace reads or input barriers.
use std::collections::BTreeMap;
use std::path::{Component, PathBuf};
use std::time::Duration;

use warp_terminal::model::BlockId;
use warpui::r#async::{SpawnedFutureHandle, Timer};
use warpui::{Entity, EntityId, ModelContext, SingletonEntity};

use crate::localization::tr;
use crate::project_sessions::ProjectSessions;
use crate::project_sessions::config::Provider;
use crate::project_sessions::tool_changes::Change;
use crate::project_sessions::turns::{self, Cursor, Observation, Source};

pub(crate) struct LastTurn {
    pub(crate) root: PathBuf,
    pub(crate) block: BlockId,
    pub(crate) provider: Provider,
    terminal: EntityId,
    path: Option<PathBuf>,
    cursor: Option<Cursor>,
    since: u64,
    changes: BTreeMap<String, Change>,
    message: String,
    poll: Option<SpawnedFutureHandle>,
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
            changes: BTreeMap::new(),
            message: tr("Captured tool changes · partial coverage").into(),
            poll: None,
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

    pub(crate) fn end(&mut self, ctx: &mut ModelContext<Self>) {
        if let Some(task) = self.poll.take() {
            task.abort();
        }
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
        self.poll = Some(ctx.spawn(
            async move {
                Timer::after(Duration::from_millis(500)).await;
                blocking::unblock(move || -> anyhow::Result<_> {
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
                .await
            },
            |me, result, ctx| {
                if me.closed {
                    return;
                }
                me.poll = None;
                match result {
                    Ok(Some((path, cursor, observations, source))) => {
                        me.path = Some(path);
                        me.cursor = Some(cursor);
                        let changed = !observations.is_empty();
                        for observation in observations {
                            match observation {
                                Observation::Request => me.changes.clear(),
                                Observation::Change(mut change) => {
                                    let Some(path) = relative_path(&me.root, &source, &change.path)
                                    else {
                                        continue;
                                    };
                                    change.path = path.clone();
                                    let bytes: usize =
                                        me.changes.values().map(|change| change.diff.len()).sum();
                                    if bytes + change.diff.len() > 2 * 1024 * 1024 {
                                        continue;
                                    }
                                    if let Some(previous) = me.changes.get_mut(&path) {
                                        if previous.diff.len() + change.diff.len() <= 64 * 1024 {
                                            previous.diff.push_str("\n@@ next tool operation @@\n");
                                            previous.diff.push_str(&change.diff);
                                            previous.additions += change.additions;
                                            previous.deletions += change.deletions;
                                        }
                                    } else if me.changes.len() < 128 {
                                        me.changes.insert(path, change);
                                    }
                                }
                            }
                        }
                        me.message = tr("Captured tool changes · partial coverage").into();
                        if changed {
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
}

fn relative_path(root: &std::path::Path, source: &Source, path: &str) -> Option<String> {
    let native = source.native_path(path)?;
    let path = native.strip_prefix(root).ok()?;
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
