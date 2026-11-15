//! A disposable Git index and object directory. The source repository is read-only.
#![allow(clippy::disallowed_types)]

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
#[cfg(windows)]
use std::io::BufWriter;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail, ensure};
use fs4::fs_std::FileExt;
use serde::{Deserialize, Serialize};
use tempfile::{NamedTempFile, TempDir};

#[cfg(windows)]
const INITIAL_BLOB_LIMIT: u64 = 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Snapshot {
    pub slot: String,
    pub captured_at: u64,
    pub elapsed_ms: u64,
    pub text_files: usize,
    pub binary_files: usize,
    pub source_bytes: u64,
    #[serde(skip)]
    tree: String,
    #[serde(skip)]
    binary: BTreeSet<PathBuf>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Operation {
    Capture {
        slot: String,
    },
    Release {
        slot: String,
    },
    Changes {
        before: String,
        after: String,
    },
    Read {
        slot: String,
        path: PathBuf,
        max_bytes: usize,
    },
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum Reply {
    Captured(Snapshot),
    Released,
    Changes(Vec<Change>),
    Text(String),
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
    Binary,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Change {
    pub path: String,
    pub kind: ChangeKind,
    pub additions: usize,
    pub deletions: usize,
}

/// All methods run on one background worker. A slot pins a complete immutable tree;
/// only explicit release permits reclamation. Source refs are never written.
pub struct Store {
    root: PathBuf,
    cache: PathBuf,
    directory: TempDir,
    _lock: File,
    paths: BTreeSet<PathBuf>,
    snapshots: BTreeMap<String, Snapshot>,
}

enum Scanned {
    Cached(PathBuf, u64),
    Binary(PathBuf),
    Text {
        path: PathBuf,
        file: File,
        size: u64,
    },
}

impl Store {
    pub fn new(root: &Path, cache: &Path) -> Result<Self> {
        fs::create_dir_all(cache)?;
        let cache = canonical_path(cache)?;
        // Serialize creation with abandoned-directory cleanup. Otherwise another
        // instance could observe a newly created owner.lock before it is locked.
        let creation_lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(cache.join("creation.lock"))?;
        creation_lock.lock_exclusive()?;
        reclaim_abandoned(&cache)?;
        let root = canonical_path(root)?;
        let directory = tempfile::Builder::new()
            .prefix("snapshot-")
            .tempdir_in(&cache)?;
        let lock = OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(directory.path().join("owner.lock"))?;
        lock.lock_exclusive()?;
        let format = source_git(&root, &["rev-parse", "--show-object-format"])?;
        let objects = source_git(
            &root,
            &[
                "rev-parse",
                "--path-format=absolute",
                "--git-path",
                "objects",
            ],
        )?;
        let objects = canonical_path(Path::new(objects.trim()))?;
        ensure!(
            !objects.to_string_lossy().contains(['\n', '\r']),
            "Invalid Git object directory"
        );
        git_output(
            &root,
            &[
                "init",
                "--bare",
                "--quiet",
                "--template=",
                &format!("--object-format={}", format.trim()),
                directory.path().to_str().context("Invalid cache path")?,
            ],
            None,
        )?;
        fs::write(
            directory.path().join("objects/info/alternates"),
            format!("{}\n", objects.display()),
        )?;
        // Local attributes have higher priority than .gitattributes. Never invoke
        // the source repository's filters (including LFS) or use its Git config.
        fs::create_dir_all(directory.path().join("info"))?;
        fs::write(
            directory.path().join("info/attributes"),
            b"* text=auto -filter -ident -working-tree-encoding\n",
        )?;
        let store = Self {
            root,
            cache,
            directory,
            _lock: lock,
            paths: BTreeSet::new(),
            snapshots: BTreeMap::new(),
        };
        store.git(&["config", "core.bare", "false"], None)?;
        store.git(&["config", "core.autocrlf", "input"], None)?;
        store.git(&["config", "gc.auto", "0"], None)?;
        #[cfg(windows)]
        {
            store.git(&["config", "core.compression", "1"], None)?;
            store.git(&["config", "core.fscache", "true"], None)?;
        }
        store.git(&["read-tree", "--empty"], None)?;
        Ok(store)
    }

    pub fn directory(&self) -> &Path {
        self.directory.path()
    }

    pub fn snapshot(&self, slot: &str) -> Option<&Snapshot> {
        self.snapshots.get(slot)
    }

    pub fn execute(&mut self, operation: Operation, cancelled: &AtomicBool) -> Result<Reply> {
        match operation {
            Operation::Capture { slot } => self.capture(&slot, cancelled).map(Reply::Captured),
            Operation::Release { slot } => self.release(&slot).map(|()| Reply::Released),
            Operation::Changes { before, after } => {
                self.changes(&before, &after).map(Reply::Changes)
            }
            Operation::Read {
                slot,
                path,
                max_bytes,
            } => self.read(&slot, &path, max_bytes).map(Reply::Text),
        }
    }

    /// Includes ignored/hidden text, but never follows links or enters any .git
    /// directory or our cache. A failed scan leaves published slots unchanged.
    pub fn capture(&mut self, slot: &str, cancelled: &AtomicBool) -> Result<Snapshot> {
        validate_slot(slot)?;
        ensure!(
            !self.snapshots.contains_key(slot),
            "Snapshot slot is already in use"
        );
        let started = Instant::now();
        let result = self.capture_inner(slot, cancelled, started);
        if result.is_err() {
            // A failed add may leave a partially updated index and loose objects.
            // Clear the scratch index before pruning; published tree refs survive.
            self.git(&["read-tree", "--empty"], None)?;
            self.paths.clear();
        }
        // Rebuilding or clearing the index can unpin the last preview's blobs.
        self.prune()?;
        result
    }

    fn capture_inner(
        &mut self,
        slot: &str,
        cancelled: &AtomicBool,
        started: Instant,
    ) -> Result<Snapshot> {
        let changed: BTreeSet<PathBuf> = if self.paths.is_empty() {
            BTreeSet::new()
        } else {
            self.git_bytes(
                &["diff-files", "--name-only", "--no-ext-diff", "-z", "--"],
                None,
            )?
            .split(|byte| *byte == 0)
            .filter(|path| !path.is_empty())
            .map(|path| Ok(PathBuf::from(std::str::from_utf8(path)?)))
            .collect::<Result<_>>()?
        };
        let cache = self.cache.clone();
        let walker = ignore::WalkBuilder::new(&self.root)
            .hidden(false)
            .ignore(false)
            .parents(false)
            .git_ignore(false)
            .git_global(false)
            .git_exclude(false)
            .follow_links(false)
            .filter_entry(move |entry| entry.file_name() != ".git" && entry.path() != cache)
            .build();
        let mut paths = BTreeSet::new();
        let mut binary = BTreeSet::new();
        let mut source_bytes = 0u64;
        #[cfg(windows)]
        let mut remaining = NamedTempFile::new_in(self.directory.path())?;
        let scan = |_input: &mut dyn Write, _index: &mut dyn Write| -> Result<()> {
            let mut consume = |entry: Scanned, prefix: Cow<'_, [u8]>| -> Result<()> {
                match entry {
                    Scanned::Cached(path, size) => {
                        source_bytes = source_bytes.saturating_add(size);
                        paths.insert(path);
                    }
                    Scanned::Binary(path) => {
                        binary.insert(path);
                    }
                    Scanned::Text { path, file, size } => {
                        #[cfg(windows)]
                        {
                            let exact = self.import_file(file, prefix, size, _input)?;
                            if self.paths.is_empty() {
                                if exact {
                                    write_path(_index, &path)?;
                                } else {
                                    write_path(&mut remaining, &path)?;
                                }
                            }
                        }
                        #[cfg(not(windows))]
                        let _ = (file, prefix);
                        source_bytes = source_bytes.saturating_add(size);
                        paths.insert(path);
                    }
                }
                Ok(())
            };
            #[cfg(windows)]
            if self.paths.is_empty() {
                return self.prefetch(walker, &changed, cancelled, consume);
            }
            let mut prefix = vec![0; 8192];
            for entry in walker {
                ensure!(!cancelled.load(Ordering::Relaxed), "Snapshot cancelled");
                if let Some(entry) = self.inspect_entry(entry, &changed, &mut prefix)? {
                    consume(entry, Cow::Borrowed(&prefix))?;
                }
            }
            Ok(())
        };
        #[cfg(windows)]
        if self.paths.is_empty() {
            // Preloaded bytes have Git's clean form. Let Git compute the same IDs
            // and stat cache concurrently, without writing duplicate loose blobs.
            self.git_stream(
                &["update-index", "--add", "--info-only", "-z", "--stdin"],
                |index| self.import_objects(|input| scan(input, index)),
            )?;
            self.finish_initial_index(&mut remaining)?;
        } else {
            self.import_objects(|input| scan(input, &mut std::io::sink()))?;
        }
        #[cfg(not(windows))]
        scan(&mut std::io::sink(), &mut std::io::sink())?;
        ensure!(!cancelled.load(Ordering::Relaxed), "Snapshot cancelled");
        let mut removed = NamedTempFile::new_in(self.directory.path())?;
        for path in self.paths.difference(&paths) {
            write_path(&mut removed, path)?;
        }
        if removed.as_file().metadata()?.len() != 0 {
            self.git(
                &["update-index", "--force-remove", "-z", "--stdin"],
                Some(File::open(removed.path())?),
            )?;
        }
        // Keep Git's stat cache between captures. This has Git's normal metadata
        // freshness semantics; deliberate preservation of all checked metadata
        // can hide a change. Immutable published trees still pin their objects.
        if !paths.is_empty() && (!cfg!(windows) || !self.paths.is_empty()) {
            let mut selected = NamedTempFile::new_in(self.directory.path())?;
            for path in &paths {
                write_path(&mut selected, path)?;
            }
            self.git(
                &["update-index", "--add", "-z", "--stdin"],
                Some(File::open(selected.path())?),
            )?;
        }
        ensure!(!cancelled.load(Ordering::Relaxed), "Snapshot cancelled");
        #[cfg(windows)]
        if self.paths.is_empty() {
            self.import_trees(cancelled)?;
        }
        self.paths = paths;
        let tree = self.git(&["write-tree"], None)?.trim().to_owned();
        let captured_at = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u64;
        self.git(
            &["update-ref", &format!("refs/snapshots/{slot}"), &tree],
            None,
        )?;
        let snapshot = Snapshot {
            slot: slot.to_owned(),
            captured_at,
            elapsed_ms: started.elapsed().as_millis() as u64,
            text_files: self.paths.len(),
            binary_files: binary.len(),
            source_bytes,
            tree,
            binary: BTreeSet::new(),
        };
        // Only the store needs binary membership; replies carry the counts.
        self.snapshots.insert(
            slot.to_owned(),
            Snapshot {
                binary,
                ..snapshot.clone()
            },
        );
        Ok(snapshot)
    }

    #[cfg(windows)]
    fn prefetch(
        &self,
        walker: ignore::Walk,
        changed: &BTreeSet<PathBuf>,
        cancelled: &AtomicBool,
        mut consume: impl FnMut(Scanned, Cow<'_, [u8]>) -> Result<()>,
    ) -> Result<()> {
        let walker = std::sync::Mutex::new(walker);
        std::thread::scope(|scope| {
            // Two readers may each hold one pending file; rendezvous adds no queue.
            let (sender, receiver) = std::sync::mpsc::sync_channel(0);
            let mut readers = Vec::with_capacity(2);
            for _ in 0..2 {
                let walker = &walker;
                let sender = sender.clone();
                readers.push(std::thread::Builder::new().spawn_scoped(scope, move || {
                    let mut prefix = vec![0; 8192];
                    loop {
                        let entry = match walker.lock() {
                            Ok(mut walker) => match walker.next() {
                                Some(entry) => entry,
                                None => break,
                            },
                            Err(_) => break, // The joined reader reports the panic.
                        };
                        let next = if cancelled.load(Ordering::Relaxed) {
                            Err(anyhow::anyhow!("Snapshot cancelled"))
                        } else {
                            self.inspect_entry(entry, changed, &mut prefix)
                        };
                        let next = match next {
                            Ok(Some(entry)) => {
                                let len = match &entry {
                                    Scanned::Text { .. } => prefix.len(),
                                    _ => 0,
                                };
                                Ok((entry, prefix[..len].to_vec()))
                            }
                            Ok(None) => continue,
                            Err(error) => Err(error),
                        };
                        let failed = next.is_err();
                        if sender.send(next).is_err() || failed {
                            break;
                        }
                    }
                })?);
            }
            drop(sender);
            let mut result = (|| {
                for entry in &receiver {
                    let (entry, prefix) = entry?;
                    consume(entry, Cow::Owned(prefix))?;
                }
                Ok(())
            })();
            drop(receiver); // Unblock a pending send if consuming failed.
            for reader in readers {
                let joined = reader
                    .join()
                    .map_err(|_| anyhow::anyhow!("Snapshot reader panicked"));
                if result.is_ok() {
                    result = joined;
                }
            }
            result
        })
    }

    fn inspect_entry(
        &self,
        entry: std::result::Result<ignore::DirEntry, ignore::Error>,
        changed: &BTreeSet<PathBuf>,
        prefix: &mut Vec<u8>,
    ) -> Result<Option<Scanned>> {
        let entry = entry?;
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            return Ok(None);
        }
        let path = entry.path().strip_prefix(&self.root)?.to_path_buf();
        if self.paths.contains(&path) && !changed.contains(&path) {
            return Ok(Some(Scanned::Cached(path, entry.metadata()?.len())));
        }
        let mut file = File::open(entry.path())
            .with_context(|| format!("Cannot read {}", entry.path().display()))?;
        let size = file.metadata()?.len();
        prefix.resize(8192, 0);
        let prefix_len = file.read(prefix)?;
        prefix.truncate(prefix_len);
        if content_inspector::inspect(&prefix[..prefix_len]).is_binary() {
            Ok(Some(Scanned::Binary(path)))
        } else {
            #[cfg(windows)]
            if self.paths.is_empty() && size <= INITIAL_BLOB_LIMIT {
                prefix.truncate(prefix.len().min(size as usize));
                (&mut file)
                    .take(size - prefix.len() as u64)
                    .read_to_end(prefix)?;
                ensure!(prefix.len() as u64 == size, "Snapshot file was truncated");
            }
            Ok(Some(Scanned::Text { path, file, size }))
        }
    }

    #[cfg(windows)]
    fn finish_initial_index(&self, remaining: &mut NamedTempFile) -> Result<()> {
        let entries = self.git_bytes(&["ls-files", "--stage", "-z"], None)?;
        let mut objects = NamedTempFile::new_in(self.directory.path())?;
        let entries = std::str::from_utf8(&entries)?;
        for entry in entries.split('\0').filter(|entry| !entry.is_empty()) {
            let object = entry
                .split_ascii_whitespace()
                .nth(1)
                .context("Invalid snapshot index")?;
            writeln!(objects, "{object}")?;
        }
        let checked = self.git(
            &["cat-file", "--batch-check=%(objectname)"],
            Some(File::open(objects.path())?),
        )?;
        let mut removed = NamedTempFile::new_in(self.directory.path())?;
        // A file may change between the preload and Git's read. Re-add only
        // entries whose new content is absent, using Git's normal clean/write path.
        for (entry, object) in entries
            .split('\0')
            .filter(|entry| !entry.is_empty())
            .zip(checked.lines())
        {
            if object.ends_with(" missing") {
                let (_, path) = entry.split_once('\t').context("Invalid snapshot index")?;
                write_path(&mut removed, Path::new(path))?;
                write_path(remaining, Path::new(path))?;
            }
        }
        if removed.as_file().metadata()?.len() != 0 {
            self.git(
                &["update-index", "--force-remove", "-z", "--stdin"],
                Some(File::open(removed.path())?),
            )?;
        }
        if remaining.as_file().metadata()?.len() != 0 {
            self.git(
                &["update-index", "--add", "-z", "--stdin"],
                Some(File::open(remaining.path())?),
            )?;
        }
        Ok(())
    }

    #[cfg(windows)]
    fn import_file(
        &self,
        file: File,
        prefix: Cow<'_, [u8]>,
        size: u64,
        input: &mut dyn Write,
    ) -> Result<bool> {
        if self.paths.is_empty() && size > INITIAL_BLOB_LIMIT {
            return Ok(false);
        }
        let copied = size.min(prefix.len() as u64);
        let mut rest = file.take(size - copied);
        // Bound extra memory, and only normalize unambiguous text into an empty index.
        // Final content and metadata still come from the normal Git index update.
        let exact = if self.paths.is_empty() {
            let mut bytes = prefix.into_owned();
            bytes.truncate(copied as usize);
            rest.read_to_end(&mut bytes)?;
            ensure!(bytes.len() as u64 == size, "Snapshot file was truncated");
            let has_cr = bytes.contains(&b'\r');
            if has_cr {
                let clean = std::str::from_utf8(&bytes).is_ok()
                    && bytes.iter().all(|byte| {
                        (*byte >= b' ' && *byte != 127) || matches!(byte, b'\t' | b'\r' | b'\n')
                    })
                    && bytes.last() != Some(&b'\r')
                    && bytes
                        .windows(2)
                        .all(|pair| pair[0] != b'\r' || pair[1] == b'\n');
                if !clean {
                    return Ok(false);
                }
                bytes.retain(|byte| *byte != b'\r');
            }
            writeln!(input, "blob\ndata {}", bytes.len())?;
            input.write_all(&bytes)?;
            true
        } else {
            writeln!(input, "blob\ndata {size}")?;
            input.write_all(&prefix[..copied as usize])?;
            ensure!(
                std::io::copy(&mut rest, input)? == size - copied,
                "Snapshot file was truncated"
            );
            false
        };
        input.write_all(b"\n")?;
        Ok(exact)
    }

    #[cfg(windows)]
    fn import_trees(&self, cancelled: &AtomicBool) -> Result<()> {
        let entries = self.git(&["ls-files", "--stage"], None)?;
        let reference = "refs/snapshot-import";
        let result = self.import_objects(|input| {
            writeln!(input, "commit {reference}\ncommitter NeoWarp <snapshot@localhost> 0 +0000\ndata 0\n\ndeleteall")?;
            for line in entries.lines() {
                ensure!(!cancelled.load(Ordering::Relaxed), "Snapshot cancelled");
                let (entry, path) = line.split_once('\t').context("Invalid snapshot index entry")?;
                let entry = entry.strip_suffix(" 0").context("Unmerged snapshot index")?;
                writeln!(input, "M {entry} {path}")?;
            }
            input.write_all(b"\n")?;
            Ok(())
        });
        // The commit only asks Git to build packed trees; slots still pin trees.
        // Remove its reference on failure too, before normal reachability cleanup.
        self.git(&["update-ref", "-d", reference], None)?;
        result
    }

    #[cfg(windows)]
    fn import_objects(&self, write: impl FnOnce(&mut dyn Write) -> Result<()>) -> Result<()> {
        self.git_stream(&["fast-import", "--quiet", "--done"], |input| {
            write(input)?;
            input.write_all(b"done\n")?;
            Ok(())
        })
    }

    #[cfg(windows)]
    fn git_stream(
        &self,
        args: &[&str],
        write: impl FnOnce(&mut dyn Write) -> Result<()>,
    ) -> Result<()> {
        let directory = self
            .directory
            .path()
            .to_str()
            .context("Invalid cache path")?;
        let root = self.root.to_str().context("Invalid workspace path")?;
        let mut command = vec!["--git-dir", directory, "--work-tree", root];
        command.extend_from_slice(args);
        // update-index can emit many line-ending warnings. A file avoids filling
        // its stderr pipe while the producer is waiting for input backpressure.
        let errors = NamedTempFile::new_in(self.directory.path())?;
        let mut child = git_command(&self.root, &command)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(errors.reopen()?)
            .spawn()?;
        let result = (|| -> Result<()> {
            let mut input = BufWriter::new(child.stdin.take().context("Missing Git input")?);
            write(&mut input)?;
            input.flush()?;
            Ok(())
        })();
        if result.is_err() && child.try_wait()?.is_none() {
            child.kill()?;
        }
        let status = child.wait()?;
        if let Err(error) = result {
            return Err(error.context(format!(
                "Snapshot input failed: {}",
                fs::read_to_string(errors.path())?
            )));
        }
        ensure!(
            status.success(),
            "Snapshot import failed: {}",
            fs::read_to_string(errors.path())?
        );
        Ok(())
    }

    pub fn release(&mut self, slot: &str) -> Result<()> {
        validate_slot(slot)?;
        self.git(
            &["update-ref", "-d", &format!("refs/snapshots/{slot}")],
            None,
        )?;
        self.snapshots.remove(slot);
        if self.snapshots.is_empty() {
            self.git(&["read-tree", "--empty"], None)?;
            self.paths.clear();
        }
        self.prune()
    }

    pub fn changes(&self, before: &str, after: &str) -> Result<Vec<Change>> {
        let before = self
            .snapshots
            .get(before)
            .context("Baseline was released")?;
        let after = self.snapshots.get(after).context("Preview was released")?;
        let output = self.git_bytes(
            &[
                "diff-tree",
                "--no-commit-id",
                "--no-renames",
                "--no-ext-diff",
                "--name-status",
                "-r",
                "-z",
                &before.tree,
                &after.tree,
                "--",
            ],
            None,
        )?;
        let mut tokens = output
            .split(|byte| *byte == 0)
            .filter(|part| !part.is_empty());
        let mut changes = Vec::new();
        while let Some(status) = tokens.next() {
            let path = std::str::from_utf8(tokens.next().context("Missing diff path")?)?.to_owned();
            let kind = if before.binary.contains(Path::new(&path))
                || after.binary.contains(Path::new(&path))
            {
                ChangeKind::Binary
            } else {
                match status {
                    b"A" => ChangeKind::Added,
                    b"M" | b"T" => ChangeKind::Modified,
                    b"D" => ChangeKind::Deleted,
                    _ => bail!("Unsupported snapshot change"),
                }
            };
            changes.push(Change {
                path,
                kind,
                additions: 0,
                deletions: 0,
            });
        }
        let stats = self.git(
            &[
                "diff-tree",
                "--no-commit-id",
                "--no-renames",
                "--no-ext-diff",
                "--numstat",
                "-r",
                "-z",
                &before.tree,
                &after.tree,
                "--",
            ],
            None,
        )?;
        let indices: BTreeMap<_, _> = changes
            .iter()
            .enumerate()
            .map(|(index, change)| (change.path.clone(), index))
            .collect();
        for line in stats.split('\0').filter(|line| !line.is_empty()) {
            let mut parts = line.splitn(3, '\t');
            let additions = parts.next().context("Missing additions")?;
            let deletions = parts.next().context("Missing deletions")?;
            let path = parts.next().context("Missing statistics path")?;
            if let Some(index) = indices.get(path) {
                let change = &mut changes[*index];
                if additions == "-" || deletions == "-" {
                    change.kind = ChangeKind::Binary;
                } else {
                    change.additions = additions.parse()?;
                    change.deletions = deletions.parse()?;
                }
            }
        }
        Ok(changes)
    }

    pub fn read(&self, slot: &str, path: &Path, max_bytes: usize) -> Result<String> {
        ensure!(
            !path.as_os_str().is_empty()
                && path
                    .components()
                    .all(|part| matches!(part, Component::Normal(_))),
            "Invalid snapshot path"
        );
        let snapshot = self.snapshots.get(slot).context("Snapshot was released")?;
        let spec = format!("{}:{}", snapshot.tree, git_path(path)?);
        let size: usize = self.git(&["cat-file", "-s", &spec], None)?.trim().parse()?;
        ensure!(size <= max_bytes, "File exceeds review size limit");
        String::from_utf8(self.git_bytes(&["cat-file", "blob", &spec], None)?)
            .context("File is not UTF-8 text")
    }

    fn prune(&self) -> Result<()> {
        // Only this process owns this private database. Live trees are pinned;
        // Windows imports also need packed, unreachable blobs reclaimed.
        // Keep the streaming threshold local to packing to preserve clean/diff rules.
        #[cfg(windows)]
        self.git(
            &[
                "-c",
                "core.bigFileThreshold=1m",
                "repack",
                "-a",
                "-d",
                "-l",
                "--window=0",
                "--threads=1",
            ],
            None,
        )?;
        self.git(&["prune", "--expire=now"], None)?;
        Ok(())
    }

    fn git(&self, args: &[&str], input: Option<File>) -> Result<String> {
        String::from_utf8(self.git_bytes(args, input)?).context("Invalid Git output")
    }

    fn git_bytes(&self, args: &[&str], input: Option<File>) -> Result<Vec<u8>> {
        let directory = self
            .directory
            .path()
            .to_str()
            .context("Invalid cache path")?;
        let root = self.root.to_str().context("Invalid workspace path")?;
        let mut command = vec!["--git-dir", directory, "--work-tree", root];
        command.extend_from_slice(args);
        git_output(&self.root, &command, input)
    }
}

fn validate_slot(slot: &str) -> Result<()> {
    ensure!(
        !slot.is_empty()
            && slot.len() <= 80
            && slot.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-'),
        "Invalid snapshot slot"
    );
    Ok(())
}

fn canonical_path(path: &Path) -> Result<PathBuf> {
    let path = path.canonicalize()?;
    // Git for Windows does not accept Rust's verbatim canonical path spelling.
    #[cfg(windows)]
    if let Some(path) = path.to_str() {
        if let Some(path) = path.strip_prefix(r"\\?\UNC\") {
            return Ok(PathBuf::from(format!(r"\\{path}")));
        }
        if let Some(path) = path.strip_prefix(r"\\?\") {
            return Ok(PathBuf::from(path));
        }
    }
    Ok(path)
}

fn write_path(file: &mut (impl Write + ?Sized), path: &Path) -> Result<()> {
    file.write_all(git_path(path)?.as_bytes())?;
    file.write_all(&[0])?;
    Ok(())
}

fn git_path(path: &Path) -> Result<String> {
    let path = path.to_str().context("Non-UTF-8 snapshot path")?;
    Ok(if cfg!(windows) {
        path.replace('\\', "/")
    } else {
        path.to_owned()
    })
}

fn source_git(root: &Path, args: &[&str]) -> Result<String> {
    String::from_utf8(git_output(root, args, None)?).context("Invalid Git metadata")
}

fn git_command(root: &Path, args: &[&str]) -> Command {
    let mut command = Command::new("git");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW, retaining the caller's job constraints.
    }
    #[cfg(target_os = "linux")]
    unsafe {
        use std::os::unix::process::CommandExt;
        let parent = libc::getpid();
        // The sidecar may be stopped while Git is populating its private cache.
        // Only async-signal-safe syscalls run between fork and exec.
        command.pre_exec(move || {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::getppid() != parent {
                return Err(std::io::Error::from_raw_os_error(libc::ESRCH));
            }
            Ok(())
        });
    }
    // Do not inherit a caller's index, object directory, config injection, or
    // external diff configuration. No source config is copied to the snapshot.
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("GIT_") {
            command.env_remove(key);
        }
    }
    command
        .current_dir(root)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        );
    command
}

fn git_output(root: &Path, args: &[&str], input: Option<File>) -> Result<Vec<u8>> {
    let mut command = git_command(root, args);
    if let Some(input) = input {
        command.stdin(Stdio::from(input));
    } else {
        command.stdin(Stdio::null());
    }
    let output = command.output().context("Cannot execute snapshot Git")?;
    ensure!(
        output.status.success(),
        "Snapshot Git failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(output.stdout)
}

fn reclaim_abandoned(cache: &Path) -> Result<()> {
    for entry in fs::read_dir(cache)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir()
            || !entry.file_name().to_string_lossy().starts_with("snapshot-")
        {
            continue;
        }
        let path = entry.path();
        let Ok(lock) = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path.join("owner.lock"))
        else {
            continue;
        };
        if lock.try_lock_exclusive().is_ok() {
            drop(lock);
            fs::remove_dir_all(&path)
                .with_context(|| format!("Cannot reclaim {}", path.display()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "snapshot_tests.rs"]
mod tests;
