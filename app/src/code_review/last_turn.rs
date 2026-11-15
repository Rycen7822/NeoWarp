//! A bounded candidate and baseline for a native CLI session. No harness hooks.
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use parking_lot::Mutex;
use uuid::Uuid;
use warp_terminal::model::BlockId;
use warpui::r#async::executor::Background;
use warpui::r#async::{SpawnedFutureHandle, Timer};
use warpui::{Entity, EntityId, ModelContext, SingletonEntity};
use wsl_git_watcher::snapshot::{Change, ChangeKind, Operation, Reply, Snapshot, Store};

use crate::localization::tr;
use crate::project_sessions::ProjectSessions;
use crate::project_sessions::config::Provider;
use crate::project_sessions::turns::{self, Cursor, Source};

struct Backend {
    root: PathBuf,
    local: Mutex<Option<Store>>,
    serial: Arc<tokio::sync::Semaphore>,
    executor: Arc<Background>,
    #[cfg(windows)]
    wsl: Option<crate::file_watcher::wsl::SnapshotClient>,
}

impl Backend {
    async fn execute(
        self: &Arc<Self>,
        operation: Operation,
        cancelled: Arc<AtomicBool>,
        keep: Option<Arc<Token>>,
    ) -> Result<Reply> {
        #[cfg(windows)]
        if let Some(wsl) = &self.wsl {
            return wsl.execute(Some(operation)).await;
        }
        let permit = self
            .serial
            .clone()
            .acquire_owned()
            .await
            .context("Snapshot worker closed")?;
        let backend = self.clone();
        blocking::unblock(move || {
            // An uncancellable blocking job keeps the slot alive until it exits.
            let _keep = keep;
            let _permit = permit;
            if matches!(operation, Operation::Capture { .. }) && cancelled.load(Ordering::Relaxed) {
                bail!("Snapshot cancelled");
            }
            let mut local = backend.local.lock();
            if local.is_none() {
                if matches!(operation, Operation::Release { .. }) {
                    return Ok(Reply::Released);
                }
                *local = Some(Store::new(
                    &backend.root,
                    &warp_core::paths::cache_dir().join("turn-snapshots"),
                )?);
            }
            local
                .as_mut()
                .context("Snapshot storage unavailable")?
                .execute(operation, &cancelled)
        })
        .await
    }

    async fn capture(self: &Arc<Self>, cancelled: Arc<AtomicBool>) -> Result<Arc<Captured>> {
        let token = Arc::new(Token {
            backend: self.clone(),
            slot: Uuid::new_v4().to_string(),
        });
        let mut cancel = CancelOnDrop(Some(cancelled.clone()));
        let reply = self
            .execute(
                Operation::Capture {
                    slot: token.slot.clone(),
                },
                cancelled,
                Some(token.clone()),
            )
            .await?;
        match reply {
            Reply::Captured(info) => {
                cancel.0 = None;
                Ok(Arc::new(Captured { token, info }))
            }
            _ => bail!("Unexpected snapshot response"),
        }
    }
}

impl Drop for Backend {
    fn drop(&mut self) {
        if let Some(store) = self.local.get_mut().take() {
            self.executor.spawn_blocking(move || drop(store)).detach();
        }
        #[cfg(windows)]
        if let Some(client) = self.wsl.take() {
            self.executor
                .spawn(async move {
                    if let Err(error) = client.execute(None).await {
                        log::warn!("Cannot dispose WSL turn snapshots: {error:#}");
                    }
                })
                .detach();
        }
    }
}

struct CancelOnDrop(Option<Arc<AtomicBool>>);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if let Some(flag) = &self.0 {
            flag.store(true, Ordering::Relaxed);
        }
    }
}

struct Token {
    backend: Arc<Backend>,
    slot: String,
}
impl Drop for Token {
    fn drop(&mut self) {
        let backend = self.backend.clone();
        let slot = self.slot.clone();
        self.backend
            .executor
            .spawn(async move {
                if let Err(error) = backend
                    .execute(
                        Operation::Release { slot },
                        Arc::new(AtomicBool::new(false)),
                        None,
                    )
                    .await
                {
                    log::warn!("Cannot reclaim turn snapshot: {error:#}");
                }
            })
            .detach();
    }
}

pub(crate) struct Captured {
    token: Arc<Token>,
    pub(crate) info: Snapshot,
}

pub(crate) struct Prepared {
    pub(crate) captured: Arc<Captured>,
    log: Option<(PathBuf, Cursor)>,
}

pub(crate) struct CaptureJob {
    backend: Arc<Backend>,
    source: Option<Source>,
    pub(crate) cancelled: Arc<AtomicBool>,
}

impl CaptureJob {
    pub(crate) async fn run(self) -> Result<Prepared> {
        let captured = self.backend.capture(self.cancelled).await?;
        let log = blocking::unblock(move || -> Option<(PathBuf, Cursor)> {
            let path = self.source?.locate().ok()??;
            Some((path.clone(), Cursor::at_end(&path).ok()?))
        })
        .await;
        Ok(Prepared { captured, log })
    }
}

struct Candidate {
    captured: Arc<Captured>,
    sent_at: u64,
}

#[derive(Clone, Copy)]
pub(crate) enum Event {
    Status,
    Baseline,
}

pub(crate) struct LastTurn {
    pub(crate) root: PathBuf,
    pub(crate) block: BlockId,
    pub(crate) provider: Provider,
    terminal: EntityId,
    backend: Arc<Backend>,
    baseline: Option<Arc<Captured>>,
    candidate: Option<Candidate>,
    path: Option<PathBuf>,
    cursor: Cursor,
    since: u64,
    latest_request: u64,
    ambiguous: bool,
    confirmed: bool,
    message: String,
    poll: Option<SpawnedFutureHandle>,
    closed: bool,
}

impl LastTurn {
    pub(crate) fn new(
        root: PathBuf,
        source: Source,
        terminal: EntityId,
        missed_input: bool,
        ctx: &mut ModelContext<Self>,
    ) -> Self {
        let executor = ctx.background_executor();
        let backend = Arc::new(Backend {
            #[cfg(windows)]
            wsl: crate::file_watcher::wsl::SnapshotClient::new(
                &root,
                Uuid::new_v4().to_string(),
                &executor,
            ),
            root: root.clone(),
            local: Mutex::new(None),
            serial: Arc::new(tokio::sync::Semaphore::new(1)),
            executor,
        });
        Self {
            root,
            block: source.block.clone(),
            provider: source.provider(),
            terminal,
            backend,
            baseline: None,
            candidate: None,
            path: None,
            cursor: Cursor::default(),
            since: now(),
            latest_request: 0,
            ambiguous: missed_input,
            confirmed: false,
            message: tr("No input baseline recorded").into(),
            poll: None,
            closed: false,
        }
    }

    pub(crate) fn needs_capture(&self) -> bool {
        !self.closed
            && (self.baseline.is_none()
                || (!self.ambiguous
                    && matches!(
                        self.provider,
                        Provider::Codex | Provider::Claude | Provider::Pi
                    )))
    }

    pub(crate) fn prepare(&mut self, ctx: &mut ModelContext<Self>) -> CaptureJob {
        self.confirmed = false;
        self.message = tr("Capturing input baseline…").into();
        ctx.emit(Event::Status);
        let source = self
            .path
            .is_none()
            .then(|| ProjectSessions::as_ref(ctx).turn_source(self.terminal))
            .flatten();
        CaptureJob {
            backend: self.backend.clone(),
            source,
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    pub(crate) fn sent(&mut self, prepared: Prepared, ctx: &mut ModelContext<Self>) {
        let first_baseline = self.baseline.is_none();
        if self.path.is_none()
            && let Some((path, cursor)) = prepared.log
        {
            self.path = Some(path);
            self.cursor = cursor;
        }
        if self.candidate.is_some() {
            self.ambiguous = true;
        }
        if self.baseline.is_none() {
            self.baseline = Some(prepared.captured.clone());
        }
        self.candidate = Some(Candidate {
            captured: prepared.captured,
            sent_at: now(),
        });
        self.confirmed = false;
        self.message = tr("Session changes · turn unconfirmed").into();
        ctx.emit(Event::Status);
        if self.ambiguous || matches!(self.provider, Provider::Custom | Provider::Hermes) {
            self.candidate = None;
        } else {
            self.schedule_poll(ctx);
        }
        if first_baseline {
            ctx.emit(Event::Baseline);
        }
    }

    pub(crate) fn failed(&mut self, message: String, ctx: &mut ModelContext<Self>) {
        if let Some(task) = self.poll.take() {
            task.abort();
        }
        self.confirmed = false;
        self.ambiguous = true;
        self.candidate = None;
        self.message = format!("{}: {message}", tr("Turn unconfirmed"));
        ctx.emit(Event::Status);
    }

    pub(crate) fn label(&self) -> &str {
        &self.message
    }
    pub(crate) fn confirmed(&self) -> bool {
        self.confirmed
    }

    pub(crate) fn end(&mut self, ctx: &mut ModelContext<Self>) {
        if let Some(task) = self.poll.take() {
            task.abort();
        }
        self.closed = true;
        self.candidate = None;
        self.baseline = None;
        self.confirmed = false;
        self.message = tr("Session ended").into();
        ctx.emit(Event::Baseline);
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
        self.poll = Some(ctx.spawn(
            async move {
                Timer::after(Duration::from_millis(500)).await;
                blocking::unblock(move || -> Result<_> {
                    let path = match path {
                        Some(path) => path,
                        None => match source.locate()? {
                            Some(path) => path,
                            None => return Ok(None),
                        },
                    };
                    let (cursor, requests) = turns::read(&path, source.provider(), cursor)?;
                    Ok(Some((path, cursor, requests)))
                })
                .await
            },
            |me, result, ctx| {
                if me.closed {
                    return;
                }
                me.poll = None;
                match result {
                    Ok(Some((path, cursor, requests))) => {
                        me.path = Some(path);
                        me.cursor = cursor;
                        for request in requests {
                            if request.created_at < me.since
                                || request.created_at < me.latest_request
                            {
                                continue;
                            }
                            me.latest_request = request.created_at;
                            let matches =
                                me.candidate.as_ref().is_some_and(|candidate| {
                                    request.created_at >= candidate.sent_at
                                }) && !me.ambiguous;
                            if matches {
                                me.baseline =
                                    me.candidate.take().map(|candidate| candidate.captured);
                                me.confirmed = true;
                                me.message = tr("Last turn changes").into();
                                ctx.emit(Event::Baseline);
                            } else {
                                me.ambiguous = true;
                                me.candidate = None;
                                me.confirmed = false;
                                me.message = tr("Session changes · turn unconfirmed").into();
                                ctx.emit(Event::Status);
                            }
                        }
                    }
                    Ok(None) => (),
                    Err(error) => {
                        me.failed(format!("{error:#}"), ctx);
                        return;
                    }
                }
                if !me.ambiguous {
                    me.schedule_poll(ctx);
                }
            },
        ));
    }

    pub(crate) fn preview(
        &self,
    ) -> Result<impl std::future::Future<Output = Result<Preview>> + Send + 'static + use<>> {
        let base = self
            .baseline
            .clone()
            .context("No input baseline recorded")?;
        let backend = self.backend.clone();
        Ok(async move {
            let current = backend.capture(Arc::new(AtomicBool::new(false))).await?;
            let reply = backend
                .execute(
                    Operation::Changes {
                        before: base.token.slot.clone(),
                        after: current.token.slot.clone(),
                    },
                    Arc::new(AtomicBool::new(false)),
                    None,
                )
                .await?;
            match reply {
                Reply::Changes(files) => Ok(Preview {
                    base,
                    current,
                    files,
                }),
                _ => bail!("Unexpected snapshot response"),
            }
        })
    }
}

impl Entity for LastTurn {
    type Event = Event;
}
impl Drop for LastTurn {
    fn drop(&mut self) {
        if let Some(task) = self.poll.take() {
            task.abort();
        }
    }
}

pub(crate) struct Preview {
    base: Arc<Captured>,
    current: Arc<Captured>,
    pub(crate) files: Vec<Change>,
}

impl Preview {
    pub(crate) fn sampled_at(&self) -> u64 {
        self.current.info.captured_at
    }
    pub(crate) fn baseline_at(&self) -> u64 {
        self.base.info.captured_at
    }

    pub(crate) async fn read(
        &self,
        path: &Path,
        kind: ChangeKind,
        max_bytes: usize,
    ) -> Result<(Arc<String>, String)> {
        if kind == ChangeKind::Binary {
            bail!("File changed between text and binary");
        }
        async fn text(capture: &Captured, path: &Path, max_bytes: usize) -> Result<String> {
            match capture
                .token
                .backend
                .execute(
                    Operation::Read {
                        slot: capture.token.slot.clone(),
                        path: path.to_owned(),
                        max_bytes,
                    },
                    Arc::new(AtomicBool::new(false)),
                    None,
                )
                .await?
            {
                Reply::Text(text) => Ok(text),
                _ => bail!("Unexpected snapshot response"),
            }
        }
        let base = if kind == ChangeKind::Added {
            String::new()
        } else {
            text(&self.base, path, max_bytes).await?
        };
        let current = if kind == ChangeKind::Deleted {
            String::new()
        } else {
            text(&self.current, path, max_bytes).await?
        };
        Ok((Arc::new(base), current))
    }
}

pub(crate) fn now() -> u64 {
    chrono::Utc::now().timestamp_millis().max(0) as u64
}
