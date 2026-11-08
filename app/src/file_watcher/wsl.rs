//! WSL ext4 does not support Windows ReadDirectoryChangesW. A small Linux
//! sidecar supplies invalidations and bounded review content over stdio.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Weak};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use async_channel::{Receiver, Sender};
use command::r#async::Command;
use futures::channel::oneshot;
use futures::future::{Either, select};
use futures::stream::{AbortHandle, Abortable};
use futures_lite::StreamExt;
use futures_lite::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::json;
use warpui::r#async::executor::Background;

use super::FileChanges;

struct Service {
    control: Sender<Control>,
    ready: Arc<AtomicBool>,
    read_lock: futures::lock::Mutex<()>,
}

type Services = HashMap<String, Weak<Service>>;
static SERVICES: LazyLock<Mutex<Services>> = LazyLock::new(Mutex::default);
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy)]
pub(crate) enum WatchScope {
    Repository,
    Directory,
}

enum Control {
    Watch {
        id: u64,
        path: String,
        scope: WatchScope,
        events: Sender<()>,
        pending: Arc<Mutex<FileChanges>>,
    },
    Unwatch {
        id: u64,
    },
    Read {
        id: u64,
        root: String,
        path: Option<String>,
        base: Option<String>,
        reply: oneshot::Sender<std::result::Result<ReviewContents, String>>,
    },
    CancelRead {
        id: u64,
    },
}

#[derive(Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
enum Event {
    Ready {
        id: u64,
    },
    Dirty {
        id: u64,
        #[serde(flatten)]
        changes: FileChanges,
    },
    Error {
        id: u64,
        message: String,
    },
    Read {
        id: u64,
        result: std::result::Result<ReviewContents, String>,
    },
}

#[derive(Deserialize)]
pub(crate) struct ReviewContents {
    pub base: Option<String>,
    pub current: String,
}

struct ReadGuard<'a> {
    service: &'a Service,
    id: u64,
}

impl Drop for ReadGuard<'_> {
    fn drop(&mut self) {
        let _ = self
            .service
            .control
            .try_send(Control::CancelRead { id: self.id });
    }
}

/// None means no healthy sidecar is available; reuse the ordinary local reader.
pub(crate) async fn read_review_file(
    root: &Path,
    path: Option<&str>,
    base: Option<String>,
) -> Option<Result<ReviewContents>> {
    let root = warp_util::path::parse_wsl_unc_path(root)?;
    let service = SERVICES
        .lock()
        .get(&root.distro.to_ascii_lowercase())?
        .upgrade()?;
    if service.control.is_closed() || !service.ready.load(Ordering::Relaxed) {
        return None;
    }
    let _permit = service.read_lock.lock().await;
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let (reply, result) = oneshot::channel();
    let _guard = ReadGuard {
        service: &service,
        id,
    };
    Some(
        async {
            service
                .control
                .try_send(Control::Read {
                    id,
                    root: root.linux_path,
                    path: path.map(str::to_owned),
                    base,
                    reply,
                })
                .context("WSL review pipe closed")?;
            match select(result, async_io::Timer::after(Duration::from_secs(30))).await {
                Either::Left((result, _)) => result
                    .context("WSL review pipe closed")?
                    .map_err(anyhow::Error::msg),
                Either::Right(_) => bail!("WSL review read timed out"),
            }
        }
        .await,
    )
}

pub(crate) struct Subscription {
    service: Arc<Service>,
    id: u64,
    pending: Arc<Mutex<FileChanges>>,
}

impl Subscription {
    pub(crate) fn take_changes(&self) -> FileChanges {
        std::mem::take(&mut *self.pending.lock())
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        let _ = self
            .service
            .control
            .try_send(Control::Unwatch { id: self.id });
        // The registry is weak. The last subscription drops the last sender,
        // closing the actor and the helper's stdin without touching WSL itself.
    }
}

pub(crate) struct FileWatch(AbortHandle);

impl Drop for FileWatch {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Keeps an open file current independently of the directory sidebar.
pub(crate) fn watch_file(
    path: &Path,
    executor: &Arc<Background>,
) -> Option<(FileWatch, impl futures::Stream<Item = ()> + use<>)> {
    let parent = path.parent()?.to_path_buf();
    let initial = subscribe(&parent, WatchScope::Directory, executor)?;
    let filename = PathBuf::from(path.file_name()?);
    let path = path.to_path_buf();
    let executor = executor.clone();
    let events = async_stream::stream! {
        let mut connection = Some(initial);
        let mut ready = false;
        let mut previous = None;
        loop {
            let tick = async_io::Timer::after(Duration::from_secs(5));
            let notification = if let Some((_, events)) = &connection {
                let next = events.recv();
                futures::pin_mut!(next);
                match select(next, tick).await {
                    Either::Left((event, _)) => Some(event),
                    Either::Right(_) => None,
                }
            } else {
                tick.await;
                None
            };
            match notification {
                Some(Ok(())) => {
                    ready = true;
                    let changes = connection.as_ref().expect("received from this connection").0.take_changes();
                    if changes.full || changes.paths.contains(&filename) {
                        yield ();
                    }
                }
                Some(Err(_)) => {
                    connection = None;
                    ready = false;
                }
                None if !ready => {
                    // Only failed/startup watches poll metadata; healthy watches do no file I/O here.
                    let current = async_fs::metadata(&path).await.ok()
                        .and_then(|metadata| Some((metadata.modified().ok()?, metadata.len())));
                    if current != previous {
                        previous = current;
                        yield ();
                    }
                    if connection.is_none() {
                        connection = subscribe(&parent, WatchScope::Directory, &executor);
                    }
                }
                None => {}
            }
        }
    };
    let (abort, registration) = AbortHandle::new_pair();
    Some((FileWatch(abort), Abortable::new(events, registration)))
}

pub(crate) fn subscribe(
    path: &Path,
    scope: WatchScope,
    executor: &Background,
) -> Option<(Subscription, Receiver<()>)> {
    let path = warp_util::path::parse_wsl_unc_path(path)?;
    let mut services = SERVICES.lock();
    services.retain(|_, sender| sender.strong_count() > 0);
    let key = path.distro.to_ascii_lowercase();
    let service = services
        .get(&key)
        .and_then(Weak::upgrade)
        .filter(|service| !service.control.is_closed())
        .unwrap_or_else(|| {
            let (tx, rx) = async_channel::unbounded();
            let ready = Arc::new(AtomicBool::new(false));
            let service = Arc::new(Service {
                control: tx,
                ready: ready.clone(),
                read_lock: Default::default(),
            });
            services.insert(key, Arc::downgrade(&service));
            executor
                .spawn(async move {
                    if let Err(error) = run(&path.distro, rx, ready).await {
                        log::warn!("WSL filesystem watcher unavailable: {error:#}");
                    }
                })
                .detach();
            service
        });
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let (events, receiver) = async_channel::bounded(1);
    let pending = Arc::new(Mutex::new(FileChanges::default()));
    let _ = service.control.try_send(Control::Watch {
        id,
        path: path.linux_path,
        scope,
        events,
        pending: pending.clone(),
    });
    Some((
        Subscription {
            service,
            id,
            pending,
        },
        receiver,
    ))
}

async fn run(distro: &str, control: Receiver<Control>, ready: Arc<AtomicBool>) -> Result<()> {
    let helper = std::env::current_exe()?.with_file_name("neowarp-wsl-watch");
    // Validate before starting wsl.exe, so ordinary development builds without
    // the sidecar immediately fall back rather than launching a failing child.
    async_fs::metadata(&helper)
        .await
        .context("missing neowarp-wsl-watch beside executable")?;
    let helper = dunce::simplified(&helper)
        .to_str()
        .context("WSL helper path is not UTF-8")?;
    let helper = warp_util::path::convert_windows_path_to_wsl(helper);
    let mut child = Command::new("wsl.exe")
        .args(["--distribution", distro, "--exec", &helper])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .context("cannot start WSL filesystem watcher")?;
    let mut input = child.stdin.take().context("missing helper stdin")?;
    let mut lines = BufReader::new(child.stdout.take().context("missing helper stdout")?).lines();
    let mut subscribers = HashMap::<u64, (Sender<()>, Arc<Mutex<FileChanges>>)>::new();
    let mut reads = HashMap::new();
    let result = async {
        loop {
            // Keep the lines stream alive across cancelled selects; partially
            // received JSON must remain buffered when a control message wins.
            let next = lines.next();
            let message = control.recv();
            futures::pin_mut!(message, next);
            match select(message, next).await {
                Either::Left((Ok(message), _)) => {
                    let request = match message {
                        Control::Watch {
                            id,
                            path,
                            scope,
                            events,
                            pending,
                        } => {
                            subscribers.insert(id, (events, pending));
                            let scope = match scope {
                                WatchScope::Repository => "repository",
                                WatchScope::Directory => "directory",
                            };
                            json!({"op": "watch", "id": id, "path": path, "scope": scope})
                        }
                        Control::Unwatch { id } => {
                            subscribers.remove(&id);
                            json!({"op": "unwatch", "id": id})
                        }
                        Control::Read { id, root, path, base, reply } => {
                            reads.insert(id, reply);
                            json!({"op": "read", "id": id, "root": root, "path": path, "base": base})
                        }
                        Control::CancelRead { id } => {
                            reads.remove(&id);
                            json!({"op": "cancel_read", "id": id})
                        }
                    };
                    let mut request = serde_json::to_vec(&request)?;
                    request.push(b'\n');
                    input.write_all(&request).await?;
                    input.flush().await?;
                }
                Either::Left((Err(_), _)) => break,
                Either::Right((line, _)) => {
                    let line = line.context("helper closed its notification pipe")??;
                    let (id, changes) = match serde_json::from_str(&line)
                        .context("invalid WSL watcher notification")?
                    {
                        Event::Ready { id } => {
                            ready.store(true, Ordering::Relaxed);
                            (
                                id,
                                FileChanges {
                                    full: true,
                                    git_changed: true,
                                    ..Default::default()
                                },
                            )
                        }
                        Event::Dirty { id, changes } => (id, changes),
                        Event::Read { id, result } => {
                            if let Some(reply) = reads.remove(&id) {
                                let _ = reply.send(result);
                            }
                            continue;
                        }
                        Event::Error { id, message } => {
                            log::warn!("WSL filesystem watcher subscription failed: {message}");
                            subscribers.remove(&id);
                            if subscribers.is_empty() {
                                break;
                            }
                            continue;
                        }
                    };
                    if let Some((events, pending)) = subscribers.get(&id) {
                        // Merge before waking: a full channel must not discard
                        // paths from subsequent saves.
                        pending.lock().merge(changes);
                        let _ = events.try_send(());
                    }
                }
            }
        }
        Ok(())
    }
    .await;
    ready.store(false, Ordering::Relaxed);
    control.close();
    while control.try_recv().is_ok() {}
    // Closing stdin requests a Linux-side exit, including while the helper is
    // discovering directories. Never shut down or terminate the distribution.
    drop(input);
    drop(lines);
    drop(subscribers);
    drop(reads);
    let status = child.status();
    futures::pin_mut!(status);
    match select(status, async_io::Timer::after(Duration::from_secs(5))).await {
        Either::Left((status, _)) => {
            let status = status?;
            if !status.success() && result.is_ok() {
                bail!("WSL watcher exited with {status}");
            }
        }
        Either::Right(_) => {
            child
                .kill()
                .context("cannot stop owned WSL watcher launcher")?;
            child.status().await?;
        }
    }
    result
}
