// This standalone Linux process has no Windows console or WASM target. Avoid
// pulling the application's cross-platform process/clock wrappers into it.
#![allow(clippy::disallowed_types)]

use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::io::{self, BufRead, Write};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use notify_debouncer_full::notify::{self, EventKind, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};

const BATCH_DELAY: Duration = Duration::from_millis(150);

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum Request {
    Watch {
        id: u64,
        path: PathBuf,
        #[serde(default)]
        scope: WatchScope,
    },
    Unwatch {
        id: u64,
    },
}

#[derive(Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
enum WatchScope {
    #[default]
    Repository,
    Directory,
}

#[derive(Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
enum Response {
    Ready {
        id: u64,
    },
    Dirty {
        id: u64,
        paths: HashSet<String>,
        full: bool,
        git_changed: bool,
    },
    Error {
        id: u64,
        message: String,
    },
}

#[derive(Default)]
struct Changes {
    paths: HashSet<String>,
    full: bool,
    git_changed: bool,
}

impl Changes {
    fn record(&mut self, repository: &Repository, path: &Path) {
        if repository.git_dirs.iter().any(|git| path.starts_with(git)) {
            self.git_changed = true;
            self.full = true;
        } else if !self.full {
            match path
                .strip_prefix(&repository.root)
                .ok()
                .and_then(Path::to_str)
            {
                Some(relative) if !relative.is_empty() && self.paths.len() < 1024 => {
                    self.paths.insert(relative.to_owned());
                }
                _ => self.full = true,
            }
        }
        if self.full {
            self.paths.clear();
        }
    }
}

enum Input {
    Request(Request),
    Files(notify::Result<notify::Event>),
    Closed,
    Invalid(String),
}

struct Repository {
    scope: WatchScope,
    root: PathBuf,
    git_dirs: Vec<PathBuf>,
    directories: HashSet<PathBuf>,
}

fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let output = Command::new("git")
        .current_dir(root)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args(args)
        .output()
        .context("cannot run git to resolve watch directories")?;
    if !output.status.success() {
        bail!(
            "cannot resolve watch directories: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(output.stdout)
}

fn watch_directory(
    path: &Path,
    watcher: &mut impl Watcher,
    registered: &mut HashSet<PathBuf>,
) -> Result<()> {
    // Re-register on discovery even when the pathname is unchanged: a directory
    // may have been removed/recreated, leaving the old inotify inode behind.
    watcher
        .watch(path, RecursiveMode::NonRecursive)
        .with_context(|| format!("cannot watch {}", path.display()))?;
    registered.insert(path.to_path_buf());
    Ok(())
}

impl Repository {
    fn scan(
        root: &Path,
        scope: WatchScope,
        watcher: &mut impl Watcher,
        registered: &mut HashSet<PathBuf>,
        stopping: &AtomicBool,
    ) -> Result<Self> {
        let root = root
            .canonicalize()
            .context("repository directory is unavailable")?;
        let mut directories = HashSet::from([root.clone()]);
        // Register each directory before enumerating its children. Events queued during
        // initial discovery are processed after Ready; the client also refreshes on Ready.
        watch_directory(&root, watcher, registered)?;
        if matches!(scope, WatchScope::Directory) {
            return Ok(Self {
                scope,
                root,
                git_dirs: Vec::new(),
                directories,
            });
        }
        let output = git(
            &root,
            &[
                "rev-parse",
                "--path-format=absolute",
                "--git-dir",
                "--git-common-dir",
            ],
        )?;
        let git_dirs: Vec<_> = output
            .split(|b| *b == b'\n')
            .filter(|s| !s.is_empty())
            .map(|s| PathBuf::from(OsStr::from_bytes(s)))
            .collect();
        for directory in &git_dirs {
            watch_directory(directory, watcher, registered)?;
            directories.insert(directory.clone());
            // Index/HEAD/config and refs matter; objects and reflogs are not scanned.
            for subdir in [directory.join("refs"), directory.join("info")] {
                if subdir.is_dir() {
                    for entry in ignore::WalkBuilder::new(subdir)
                        .standard_filters(false)
                        .build()
                    {
                        if stopping.load(Ordering::Relaxed) {
                            bail!("parent closed");
                        }
                        let entry = entry?;
                        if entry.file_type().is_some_and(|kind| kind.is_dir()) {
                            watch_directory(entry.path(), watcher, registered)?;
                            directories.insert(entry.into_path());
                        }
                    }
                }
            }
        }
        let tracked = git(&root, &["ls-files", "--cached", "-z"])?;
        for file in tracked.split(|b| *b == 0).filter(|s| !s.is_empty()) {
            let file = root.join(OsStr::from_bytes(file));
            for parent in file.parent().into_iter().flat_map(Path::ancestors) {
                if !parent.starts_with(&root) {
                    break;
                }
                directories.insert(parent.to_path_buf());
            }
        }
        // Gitignore rules apply only to untracked directories. Tracked parents above
        // remain watched even when a later .gitignore also matches their names.
        let mut tracked_parents: Vec<_> = directories.iter().cloned().collect();
        tracked_parents.sort_by_key(|p| p.components().count());
        for directory in tracked_parents {
            if stopping.load(Ordering::Relaxed) {
                bail!("parent closed");
            }
            if directory.is_dir() {
                watch_directory(&directory, watcher, registered)?;
            }
        }
        let mut walk = ignore::WalkBuilder::new(&root);
        walk.hidden(false)
            .ignore(false)
            .filter_entry(|entry| entry.file_name() != ".git");
        for entry in walk.build() {
            if stopping.load(Ordering::Relaxed) {
                bail!("parent closed");
            }
            let entry = entry?;
            if entry.file_type().is_some_and(|kind| kind.is_dir()) {
                watch_directory(entry.path(), watcher, registered)?;
                directories.insert(entry.into_path());
            }
        }
        Ok(Self {
            scope,
            root,
            git_dirs,
            directories,
        })
    }

    fn contains(&self, path: &Path) -> bool {
        // Other subscribers may watch ignored directories. Only deliver events
        // from this subscription's own nonrecursive watches.
        self.directories.contains(path)
            || path
                .parent()
                .is_some_and(|parent| self.directories.contains(parent))
    }
}

fn send(output: &mut impl Write, response: Response) -> Result<()> {
    serde_json::to_writer(&mut *output, &response)?;
    output.write_all(b"\n")?;
    output.flush()?;
    Ok(())
}

fn prune(
    repositories: &HashMap<u64, Repository>,
    watcher: &mut impl Watcher,
    registered: &mut HashSet<PathBuf>,
) -> Result<()> {
    let wanted: HashSet<_> = repositories
        .values()
        .flat_map(|r| r.directories.iter())
        .collect();
    for path in registered.clone() {
        if !wanted.contains(&path) {
            if let Err(error) = watcher.unwatch(&path) {
                // Inotify already removes watches when their directory vanishes.
                match error.kind {
                    notify::ErrorKind::WatchNotFound => {}
                    notify::ErrorKind::Io(ref error) if error.raw_os_error() == Some(22) => {}
                    _ => return Err(error.into()),
                }
            }
            registered.remove(&path);
        }
    }
    Ok(())
}

fn read_requests(tx: SyncSender<Input>, stopping: Arc<AtomicBool>) {
    for line in io::stdin().lock().lines() {
        let input = match line {
            Ok(line) => match serde_json::from_str(&line) {
                Ok(request) => Input::Request(request),
                Err(error) => Input::Invalid(error.to_string()),
            },
            Err(error) => Input::Invalid(error.to_string()),
        };
        if tx.send(input).is_err() {
            break;
        }
    }
    stopping.store(true, Ordering::Relaxed);
    let _ = tx.send(Input::Closed);
}

pub(super) fn run() -> Result<()> {
    let (tx, rx) = mpsc::sync_channel(64);
    let overflow = Arc::new(AtomicBool::new(false));
    let stopping = Arc::new(AtomicBool::new(false));
    let callback_tx = tx.clone();
    let callback_overflow = overflow.clone();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if event
            .as_ref()
            .is_ok_and(|event| matches!(event.kind, EventKind::Access(_)))
        {
            return;
        }
        if let Err(mpsc::TrySendError::Full(_)) = callback_tx.try_send(Input::Files(event)) {
            callback_overflow.store(true, Ordering::Relaxed);
        }
    })?;
    let thread_stop = stopping.clone();
    std::thread::spawn(move || read_requests(tx, thread_stop));
    run_loop(&rx, &mut watcher, &overflow, &stopping)
}

fn run_loop(
    rx: &Receiver<Input>,
    watcher: &mut impl Watcher,
    overflow: &AtomicBool,
    stopping: &AtomicBool,
) -> Result<()> {
    let mut repositories = HashMap::<u64, Repository>::new();
    let mut registered = HashSet::new();
    let mut dirty = HashMap::<u64, Changes>::new();
    let mut rebuild = HashSet::new();
    let mut deadline = None::<Instant>;
    let mut output = io::BufWriter::new(io::stdout().lock());
    loop {
        if stopping.load(Ordering::Relaxed) {
            return Ok(());
        }
        let input = if let Some(deadline) = deadline {
            match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(input) => Some(input),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
            }
        } else {
            Some(rx.recv()?)
        };
        match input {
            Some(Input::Request(Request::Watch { id, path, scope })) => {
                match Repository::scan(&path, scope, watcher, &mut registered, stopping) {
                    Ok(repository) => {
                        repositories.insert(id, repository);
                        send(&mut output, Response::Ready { id })?;
                    }
                    Err(error) => send(
                        &mut output,
                        Response::Error {
                            id,
                            message: error.to_string(),
                        },
                    )?,
                }
                prune(&repositories, watcher, &mut registered)?;
            }
            Some(Input::Request(Request::Unwatch { id })) => {
                repositories.remove(&id);
                dirty.remove(&id);
                rebuild.remove(&id);
                prune(&repositories, watcher, &mut registered)?;
                // Stdin owns the process lifetime. A new Watch may already be
                // queued when the last directory is collapsed and reopened.
            }
            Some(Input::Files(event)) => {
                let event = event?;
                if event.need_rescan() {
                    rebuild.extend(repositories.keys().copied());
                    for &id in repositories.keys() {
                        dirty.insert(
                            id,
                            Changes {
                                full: true,
                                git_changed: true,
                                ..Changes::default()
                            },
                        );
                    }
                }
                for path in &event.paths {
                    for (&id, repository) in &repositories {
                        if repository.contains(path) {
                            dirty.entry(id).or_default().record(repository, path);
                            if path.is_dir()
                                || registered.contains(path)
                                || matches!(
                                    path.file_name().and_then(OsStr::to_str),
                                    Some(".gitignore" | ".git" | "index" | "exclude")
                                )
                            {
                                rebuild.insert(id);
                                dirty.entry(id).or_default().full = true;
                            }
                        }
                    }
                }
            }
            Some(Input::Closed) => return Ok(()),
            Some(Input::Invalid(error)) => bail!("invalid control input: {error}"),
            None => {}
        }
        if overflow.swap(false, Ordering::Relaxed) {
            rebuild.extend(repositories.keys().copied());
            for &id in repositories.keys() {
                dirty.insert(
                    id,
                    Changes {
                        full: true,
                        git_changed: true,
                        ..Changes::default()
                    },
                );
            }
        }
        if (!dirty.is_empty() || !rebuild.is_empty()) && deadline.is_none() {
            deadline = Some(Instant::now() + BATCH_DELAY);
        }
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            // Publish known changes before directory discovery can block this loop.
            for (id, changes) in dirty.drain() {
                if repositories.contains_key(&id) {
                    send(
                        &mut output,
                        Response::Dirty {
                            id,
                            paths: changes.paths,
                            full: changes.full,
                            git_changed: changes.git_changed,
                        },
                    )?;
                }
            }
            if !rebuild.is_empty() {
                for id in rebuild.drain() {
                    let Some(repository) = repositories.get(&id) else {
                        continue;
                    };
                    let root = repository.root.clone();
                    let scope = repository.scope;
                    match Repository::scan(&root, scope, watcher, &mut registered, stopping) {
                        Ok(repository) => {
                            repositories.insert(id, repository);
                            // Discovery may have raced an edit in a new directory.
                            send(
                                &mut output,
                                Response::Dirty {
                                    id,
                                    paths: HashSet::new(),
                                    full: true,
                                    git_changed: true,
                                },
                            )?;
                        }
                        Err(error) => {
                            repositories.remove(&id);
                            send(
                                &mut output,
                                Response::Error {
                                    id,
                                    message: error.to_string(),
                                },
                            )?;
                        }
                    }
                }
                prune(&repositories, watcher, &mut registered)?;
            }
            deadline = None;
        }
    }
}
