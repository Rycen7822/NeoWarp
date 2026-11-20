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

#[derive(Clone, Copy, PartialEq, Eq)]
enum CaptureState {
    Following,
    Draining,
    Ended,
}

pub(crate) struct LastTurn {
    pub(crate) root: PathBuf,
    pub(crate) block: BlockId,
    pub(crate) provider: Provider,
    pub(crate) terminal: EntityId,
    source: Source,
    path: Option<PathBuf>,
    cursor: Option<Cursor>,
    generation: u64,
    changes: BTreeMap<String, Change>,
    request_pending: bool,
    limited: bool,
    message: String,
    poll: Option<SpawnedFutureHandle>,
    log_events: Option<BoxStream<'static, ()>>,
    refresh: (async_channel::Sender<()>, async_channel::Receiver<()>),
    state: CaptureState,
}

impl LastTurn {
    pub(crate) fn new(
        root: PathBuf,
        source: Source,
        terminal: EntityId,
        ctx: &mut ModelContext<Self>,
    ) -> Self {
        let message = waiting_label(&source).into();
        let mut me = Self {
            root,
            block: source.block.clone(),
            provider: source.provider(),
            terminal,
            source,
            path: None,
            cursor: None,
            generation: 0,
            changes: BTreeMap::new(),
            request_pending: false,
            limited: false,
            message,
            poll: None,
            log_events: None,
            refresh: async_channel::bounded(1),
            state: CaptureState::Following,
        };
        if me.source.supports_turns() {
            ctx.observe(&ProjectSessions::handle(ctx), |me, sessions, ctx| {
                let source = sessions.as_ref(ctx).turn_source(me.terminal);
                me.sync_source(source, ctx);
            });
            if me.source.identity.is_some() {
                me.refresh();
            }
            me.schedule_poll(ctx);
        }
        me
    }

    pub(crate) fn label(&self) -> String {
        let mut label = self.message.clone();
        if self.request_pending && !self.changes.is_empty() {
            label.push_str(" · ");
            label.push_str(tr("Previous turn"));
        }
        if self.limited || self.changes.values().any(|change| change.truncated) {
            label.push_str(" · ");
            label.push_str(tr("Some tool changes or diff lines were omitted"));
        }
        label
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

    pub(crate) fn refresh(&self) {
        let _ = self.refresh.0.try_send(());
    }

    pub(crate) fn end(&mut self, ctx: &mut ModelContext<Self>) {
        if self.state != CaptureState::Following {
            return;
        }
        self.state = CaptureState::Draining;
        if let Some(task) = self.poll.take() {
            task.abort();
        }
        self.log_events = None;
        self.schedule_poll(ctx);
    }

    fn sync_source(&mut self, source: Option<Source>, ctx: &mut ModelContext<Self>) {
        if self.state != CaptureState::Following {
            return;
        }
        let Some(source) = source.filter(|source| source.block == self.block) else {
            self.end(ctx);
            return;
        };
        if source.identity == self.source.identity && source.ambiguous == self.source.ambiguous {
            return;
        }
        if let Some(task) = self.poll.take() {
            task.abort();
        }
        self.path = None;
        self.cursor = None;
        self.log_events = None;
        self.clear_changes();
        self.message = waiting_label(&source).into();
        self.source = source;
        ctx.emit(());
        self.refresh();
        self.schedule_poll(ctx);
    }

    fn schedule_poll(&mut self, ctx: &mut ModelContext<Self>) {
        if self.poll.is_some()
            || self.state == CaptureState::Ended
            || (self.state == CaptureState::Following && self.source.identity.is_none())
        {
            return;
        }
        let source = self.source.clone();
        let path = self.path.clone();
        let cursor = self.cursor.clone();
        let state = self.state;
        let draining = state == CaptureState::Draining;
        let refresh = self.refresh.1.clone();
        let generation = self.generation;
        let mut log_events = self.log_events.take();
        // A full read may leave more records without a new file notification.
        let interval = if draining || cursor.as_ref().is_some_and(|cursor| cursor.full_batch) {
            Duration::ZERO
        } else if log_events.is_some()
            || source
                .identity
                .as_ref()
                .is_some_and(|id| path.is_none() && id.transcript.is_none())
        {
            Duration::from_secs(2)
        } else {
            Duration::from_millis(500)
        };
        self.poll = Some(ctx.spawn(
            async move {
                if !interval.is_zero() {
                    let wait = async {
                        let timer = Timer::after(interval);
                        if let Some(events) = log_events.as_mut() {
                            if matches!(select(events.next(), timer).await, Either::Left((None, _)))
                            {
                                log_events = None;
                            }
                        } else {
                            timer.await;
                        }
                    };
                    let refresh = refresh.recv();
                    futures::pin_mut!(refresh, wait);
                    let _ = select(refresh, wait).await;
                }
                let result = blocking::unblock(move || -> anyhow::Result<_> {
                    let path = match match path {
                        Some(path) => Some(path),
                        None => source.locate()?,
                    } {
                        Some(path) => path,
                        None => return Ok(None),
                    };
                    let (cursor, observations) = turns::read(&path, source.provider(), cursor)?;
                    Ok(Some((path, cursor, observations, source)))
                })
                .await;
                (log_events, result)
            },
            move |me, (log_events, result), ctx| {
                if me.state != state || me.generation != generation {
                    return;
                }
                me.poll = None;
                me.log_events = log_events;
                let previous_message = me.message.clone();
                let unread = !matches!(&result, Ok(Some(_)));
                let mut changed = false;
                match result {
                    Ok(Some((path, cursor, observations, source))) => {
                        #[cfg(windows)]
                        if me.path.is_none() && !draining {
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
                            changed = true;
                        }
                    }
                    Ok(None) => (),
                    Err(error) => {
                        if let Some(cursor) = &mut me.cursor {
                            cursor.full_batch = false;
                        }
                        me.message = format!("{}: {error:#}", tr("Unable to read tool changes"));
                    }
                }
                if draining {
                    me.state = CaptureState::Ended;
                    me.limited |=
                        unread || me.cursor.take().is_some_and(|cursor| cursor.full_batch);
                    me.message = tr("Session ended").into();
                }
                if changed || me.message != previous_message {
                    ctx.emit(());
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
                    self.request_pending = true;
                    self.limited = false;
                    continue;
                }
                Observation::Reset => {
                    self.clear_changes();
                    bytes = 0;
                    continue;
                }
                Observation::Incomplete => {
                    self.limited = true;
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
            if std::mem::take(&mut self.request_pending) {
                self.changes.clear();
                self.generation = self.generation.wrapping_add(1);
                bytes = 0;
            }
            if let Some(previous) = self.changes.get_mut(&path) {
                let appended = SEPARATOR.len() + change.diff.len();
                if !previous.truncated
                    && previous.diff.len() + appended <= 64 * 1024
                    && bytes + appended <= MAX_TURN_BYTES
                {
                    previous.diff.push_str(SEPARATOR);
                    previous.diff.push_str(&change.diff);
                    bytes += appended;
                } else {
                    previous.truncated = true;
                }
                previous.additions += change.additions;
                previous.deletions += change.deletions;
                previous.truncated |= change.truncated;
            } else if self.changes.len() < 128 {
                if bytes + change.diff.len() > MAX_TURN_BYTES {
                    change.diff.clear();
                    change.truncated = true;
                }
                change.path = path.clone();
                bytes += change.diff.len();
                self.changes.insert(path, change);
            } else {
                self.limited = true;
            }
        }
    }

    fn clear_changes(&mut self) {
        self.changes.clear();
        self.generation = self.generation.wrapping_add(1);
        self.request_pending = false;
        self.limited = false;
    }
}

fn waiting_label(source: &Source) -> &'static str {
    if source.ambiguous {
        tr("CLI session log is ambiguous")
    } else {
        tr("Waiting for CLI session log…")
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

#[cfg(all(test, windows))]
#[path = "last_turn_tests.rs"]
mod tests;
