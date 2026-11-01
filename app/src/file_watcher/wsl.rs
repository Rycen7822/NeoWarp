//! WSL ext4 does not support Windows ReadDirectoryChangesW. A small Linux
//! sidecar supplies invalidations over stdio; Git calculation stays in the app.

use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Weak};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use async_channel::{Receiver, Sender};
use command::r#async::Command;
use futures::future::{Either, select};
use futures_lite::StreamExt;
use futures_lite::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::json;
use warpui::r#async::executor::Background;

use super::FileChanges;

type Services = HashMap<String, Weak<Sender<Control>>>;
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
}

pub(crate) struct Subscription {
    control: Arc<Sender<Control>>,
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
        let _ = self.control.try_send(Control::Unwatch { id: self.id });
        // The registry is weak. The last subscription drops the last sender,
        // closing the actor and the helper's stdin without touching WSL itself.
    }
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
    let control = services
        .get(&key)
        .and_then(Weak::upgrade)
        .filter(|sender| !sender.is_closed())
        .unwrap_or_else(|| {
            let (tx, rx) = async_channel::unbounded();
            let tx = Arc::new(tx);
            services.insert(key, Arc::downgrade(&tx));
            executor
                .spawn(async move {
                    if let Err(error) = run(&path.distro, rx).await {
                        log::warn!("WSL filesystem watcher unavailable: {error:#}");
                    }
                })
                .detach();
            tx
        });
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let (events, receiver) = async_channel::bounded(1);
    let pending = Arc::new(Mutex::new(FileChanges::default()));
    let _ = control.try_send(Control::Watch {
        id,
        path: path.linux_path,
        scope,
        events,
        pending: pending.clone(),
    });
    Some((
        Subscription {
            control,
            id,
            pending,
        },
        receiver,
    ))
}

async fn run(distro: &str, control: Receiver<Control>) -> Result<()> {
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
                        Event::Ready { id } => (
                            id,
                            FileChanges {
                                full: true,
                                git_changed: true,
                                ..Default::default()
                            },
                        ),
                        Event::Dirty { id, changes } => (id, changes),
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
    control.close();
    while control.try_recv().is_ok() {}
    // Closing stdin requests a Linux-side exit, including while the helper is
    // discovering directories. Never shut down or terminate the distribution.
    drop(input);
    drop(lines);
    drop(subscribers);
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
