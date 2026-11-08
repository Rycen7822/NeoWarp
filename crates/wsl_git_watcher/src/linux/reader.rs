use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, Read};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;

use super::{Output, Response, send};

const MAX_BYTES: usize = 4_375_000;
const MAX_STDERR: usize = 64 * 1024;
pub(super) const READ_CONCURRENCY: usize = 3;

pub(super) struct ReadRequest {
    pub id: u64,
    pub root: PathBuf,
    pub path: Option<PathBuf>,
    pub base: Option<String>,
}

#[derive(Default)]
struct Queue {
    pending: VecDeque<(ReadRequest, Arc<AtomicBool>)>,
    cancellations: Vec<(u64, Arc<AtomicBool>)>,
}

pub(super) struct Reader {
    queue: Mutex<Queue>,
    available: Condvar,
    stopping: Arc<AtomicBool>,
}

impl Reader {
    pub(super) fn new(stopping: Arc<AtomicBool>) -> Self {
        Self {
            queue: Mutex::new(Queue::default()),
            available: Condvar::new(),
            stopping,
        }
    }

    pub(super) fn enqueue(&self, request: ReadRequest) -> Result<()> {
        let mut queue = self
            .queue
            .lock()
            .map_err(|_| anyhow::anyhow!("reader queue poisoned"))?;
        ensure!(
            !self.stopping.load(Ordering::Relaxed) && queue.cancellations.len() < READ_CONCURRENCY,
            "reader is busy or closed"
        );
        ensure!(
            !queue.cancellations.iter().any(|(id, _)| *id == request.id),
            "duplicate read request"
        );
        let cancelled = Arc::new(AtomicBool::new(false));
        queue.cancellations.push((request.id, cancelled.clone()));
        queue.pending.push_back((request, cancelled));
        self.available.notify_one();
        Ok(())
    }

    pub(super) fn cancel(&self, id: u64) {
        if let Ok(mut queue) = self.queue.lock() {
            queue.cancellations.retain(|(request_id, flag)| {
                if *request_id == id {
                    flag.store(true, Ordering::Relaxed);
                    return false;
                }
                true
            });
            queue.pending.retain(|(request, _)| request.id != id);
        }
    }

    pub(super) fn stop(&self) {
        self.stopping.store(true, Ordering::Relaxed);
        if let Ok(mut queue) = self.queue.lock() {
            queue.pending.clear();
            for (_, flag) in queue.cancellations.drain(..) {
                flag.store(true, Ordering::Relaxed);
            }
            self.available.notify_all();
        }
    }

    pub(super) fn run(&self, output: &Output) -> Result<()> {
        loop {
            let (request, cancelled) = {
                let mut queue = self
                    .queue
                    .lock()
                    .map_err(|_| anyhow::anyhow!("reader queue poisoned"))?;
                while queue.pending.is_empty() && !self.stopping.load(Ordering::Relaxed) {
                    queue = self
                        .available
                        .wait(queue)
                        .map_err(|_| anyhow::anyhow!("reader queue poisoned"))?;
                }
                if self.stopping.load(Ordering::Relaxed) {
                    return Ok(());
                }
                queue.pending.pop_front().context("missing queued read")?
            };
            let id = request.id;
            let result =
                read(request, &cancelled, &self.stopping).map_err(|error| format!("{error:#}"));
            self.queue
                .lock()
                .map_err(|_| anyhow::anyhow!("reader queue poisoned"))?
                .cancellations
                .retain(|(request_id, flag)| *request_id != id || !Arc::ptr_eq(flag, &cancelled));
            if !cancelled.load(Ordering::Relaxed) && !self.stopping.load(Ordering::Relaxed) {
                send(output, Response::Read { id, result })?;
            }
        }
    }
}

#[derive(Serialize)]
pub(super) struct Contents {
    base: Option<String>,
    current: String,
}

fn read(request: ReadRequest, cancelled: &AtomicBool, stopping: &AtomicBool) -> Result<Contents> {
    ensure!(
        request.root.is_absolute(),
        "repository root must be absolute"
    );
    let base = request
        .base
        .map(|base| read_base(&request.root, &base, cancelled, stopping))
        .transpose()?;
    ensure!(
        !cancelled.load(Ordering::Relaxed) && !stopping.load(Ordering::Relaxed),
        "read cancelled"
    );
    let current = if let Some(path) = request.path {
        ensure!(
            !path.as_os_str().is_empty()
                && path
                    .components()
                    .all(|part| matches!(part, Component::Normal(_))),
            "file path must be repository-relative"
        );
        let file = File::options()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(request.root.join(path))
            .context("cannot open current file")?;
        ensure!(file.metadata()?.is_file(), "not a regular file");
        let mut bytes = Vec::new();
        file.take((MAX_BYTES + 1) as u64).read_to_end(&mut bytes)?;
        ensure!(bytes.len() <= MAX_BYTES, "file is too large to display");
        String::from_utf8(bytes).context("current file is not UTF-8")?
    } else {
        String::new()
    };
    ensure!(
        !base.as_ref().is_some_and(|text| text.contains('\0')) && !current.contains('\0'),
        "binary file; open it in an external editor"
    );
    Ok(Contents { base, current })
}

// std::process::Child does not kill/reap on drop. Every error and cancellation
// must release this owned Git process before the worker accepts another request.
struct GitChild(Child);

impl Drop for GitChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn nonblocking(pipe: &impl AsRawFd) -> io::Result<()> {
    // SAFETY: the pipe owns a live descriptor for both fcntl calls. Only its
    // status flags change; ownership and lifetime remain with the pipe.
    let flags = unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_GETFL) };
    if flags < 0
        || unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

enum PipeRead {
    Closed,
    Waiting,
    Progress,
}

fn drain(pipe: &mut impl Read, bytes: &mut Vec<u8>, limit: usize) -> Result<PipeRead> {
    let mut buffer = [0; 8192];
    let mut progressed = false;
    // Yield to cancellation even if Git continuously fills the pipe.
    for _ in 0..8 {
        match pipe.read(&mut buffer) {
            Ok(0) => return Ok(PipeRead::Closed),
            Ok(count) => {
                ensure!(
                    bytes.len() + count <= limit,
                    "Git output exceeds {limit} bytes"
                );
                bytes.extend_from_slice(&buffer[..count]);
                progressed = true;
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                return Ok(if progressed {
                    PipeRead::Progress
                } else {
                    PipeRead::Waiting
                });
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Ok(PipeRead::Progress)
}

fn read_base(
    root: &Path,
    base: &str,
    cancelled: &AtomicBool,
    stopping: &AtomicBool,
) -> Result<String> {
    ensure!(
        !base.starts_with('-') && base.contains(':'),
        "invalid Git blob specification"
    );
    let mut child = GitChild(
        Command::new("git")
            .current_dir(root)
            .env("GIT_OPTIONAL_LOCKS", "0")
            .args(["show", base, "--"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("cannot read comparison base")?,
    );
    let mut stdout = child.0.stdout.take().context("missing Git stdout")?;
    let mut stderr = child.0.stderr.take().context("missing Git stderr")?;
    nonblocking(&stdout)?;
    nonblocking(&stderr)?;
    let mut bytes = Vec::new();
    let mut errors = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        ensure!(
            !cancelled.load(Ordering::Relaxed) && !stopping.load(Ordering::Relaxed),
            "read cancelled"
        );
        ensure!(Instant::now() < deadline, "Git read timed out");
        let stdout_closed = drain(&mut stdout, &mut bytes, MAX_BYTES)?;
        let stderr_closed = drain(&mut stderr, &mut errors, MAX_STDERR)?;
        if matches!(stdout_closed, PipeRead::Closed)
            && matches!(stderr_closed, PipeRead::Closed)
            && let Some(status) = child.0.try_wait()?
        {
            if !status.success() {
                bail!(
                    "cannot read comparison base: {}",
                    String::from_utf8_lossy(&errors)
                );
            }
            let text = String::from_utf8_lossy(&bytes).into_owned();
            ensure!(
                text.len() <= MAX_BYTES,
                "base content is too large to display"
            );
            return Ok(text);
        }
        if !matches!(stdout_closed, PipeRead::Progress)
            && !matches!(stderr_closed, PipeRead::Progress)
        {
            let mut pipes = [
                libc::pollfd {
                    fd: if matches!(stdout_closed, PipeRead::Closed) {
                        -1
                    } else {
                        stdout.as_raw_fd()
                    },
                    events: libc::POLLIN,
                    revents: 0,
                },
                libc::pollfd {
                    fd: if matches!(stderr_closed, PipeRead::Closed) {
                        -1
                    } else {
                        stderr.as_raw_fd()
                    },
                    events: libc::POLLIN,
                    revents: 0,
                },
            ];
            // SAFETY: both descriptors remain owned by the pipes above. poll only
            // waits for readiness; -1 excludes closed pipes so EOF cannot spin.
            // Keep cancellation latency bounded without delaying available output.
            if unsafe { libc::poll(pipes.as_mut_ptr(), pipes.len() as libc::nfds_t, 5) } < 0 {
                let error = io::Error::last_os_error();
                if error.kind() != io::ErrorKind::Interrupted {
                    return Err(error.into());
                }
            }
        }
    }
}
