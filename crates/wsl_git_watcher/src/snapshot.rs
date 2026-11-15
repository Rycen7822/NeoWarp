//! A disposable Git index and object directory. The source repository is read-only.
#![allow(clippy::disallowed_types)]

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
#[cfg(windows)]
use std::io::{BufRead, BufReader, BufWriter};
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
    object_format: gix_index::hash::Kind,
    text_files: usize,
    snapshots: BTreeMap<String, Snapshot>,
}

enum Scanned {
    Cached(usize, u64),
    Binary(PathBuf),
    Text {
        path: PathBuf,
        size: u64,
        position: Option<usize>,
        #[cfg(windows)]
        stat: gix_index::entry::Stat,
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
            &mut git_command(
                &root,
                &[
                    "init",
                    "--bare",
                    "--quiet",
                    "--template=",
                    &format!("--object-format={}", format.trim()),
                    directory.path().to_str().context("Invalid cache path")?,
                ],
            ),
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
            object_format: match format.trim() {
                "sha1" => gix_index::hash::Kind::Sha1,
                "sha256" => gix_index::hash::Kind::Sha256,
                _ => bail!("Unsupported Git object format"),
            },
            text_files: 0,
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
            #[cfg(windows)]
            self.git(&["update-ref", "-d", "refs/snapshot-index"], None)?;
            self.text_files = 0;
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
        let previous = gix_index::File::at(
            self.directory.path().join("index"),
            self.object_format,
            false,
            Default::default(),
        )?;
        let mut seen = vec![false; previous.entries().len()];
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
        let mut text_files = 0;
        let mut binary = BTreeSet::new();
        let mut source_bytes = 0u64;
        let mut remaining = NamedTempFile::new_in(self.directory.path())?;
        #[cfg(windows)]
        let mut initial = (self.text_files == 0).then(|| {
            gix_index::File::from_state(
                gix_index::State::new(self.object_format),
                self.directory.path().join("index"),
            )
        });
        let scan = |_input: &mut dyn Write| -> Result<()> {
            let mut consume = |entry: Scanned, prefix: Cow<'_, [u8]>| -> Result<()> {
                match entry {
                    Scanned::Cached(position, size) => {
                        source_bytes = source_bytes.saturating_add(size);
                        seen[position] = true;
                        text_files += 1;
                    }
                    Scanned::Binary(path) => {
                        binary.insert(path);
                    }
                    Scanned::Text {
                        path,
                        size,
                        position,
                        #[cfg(windows)]
                        stat,
                    } => {
                        #[cfg(windows)]
                        {
                            if let Some(index) = &mut initial {
                                let mark = index.entries().len() + 1;
                                let exact = Self::import_blob(prefix, size, mark, _input)?;
                                if exact {
                                    let path = git_path(&path)?;
                                    // Each walked regular file has one entry. Sort after scanning;
                                    // no path lookup is made while entries are appended.
                                    index.dangerously_push_entry(
                                        stat,
                                        self.object_format.null(),
                                        gix_index::entry::Flags::empty(),
                                        gix_index::entry::Mode::FILE,
                                        path.as_bytes().into(),
                                    );
                                } else {
                                    write_path(&mut remaining, &path)?;
                                }
                            } else {
                                write_path(&mut remaining, &path)?;
                            }
                        }
                        #[cfg(not(windows))]
                        {
                            let _ = prefix;
                            write_path(&mut remaining, &path)?;
                        }
                        if let Some(position) = position {
                            seen[position] = true;
                        }
                        source_bytes = source_bytes.saturating_add(size);
                        text_files += 1;
                    }
                }
                Ok(())
            };
            #[cfg(windows)]
            if self.text_files == 0 {
                return self.prefetch(walker, &previous, cancelled, consume);
            }
            let mut prefix = vec![0; 8192];
            for entry in walker {
                ensure!(!cancelled.load(Ordering::Relaxed), "Snapshot cancelled");
                if let Some(entry) = self.inspect_entry(entry, &previous, &mut prefix)? {
                    consume(entry, Cow::Borrowed(&prefix))?;
                }
            }
            Ok(())
        };
        #[cfg(windows)]
        if self.text_files == 0 {
            let marks = NamedTempFile::new_in(self.directory.path())?;
            // Git returns the IDs of the exact bytes it imported. The native
            // index writer records them with the metadata sampled before reading.
            self.git_stream(
                &[
                    "fast-import",
                    "--quiet",
                    "--done",
                    &format!("--export-marks={}", marks.path().display()),
                ],
                |input| {
                    scan(input)?;
                    input.write_all(b"done\n")?;
                    Ok(())
                },
            )?;
            let mut index = initial.context("Missing initial snapshot index")?;
            for line in BufReader::new(File::open(marks.path())?).lines() {
                let line = line?;
                let (mark, object) = line.split_once(' ').context("Invalid import mark")?;
                let position: usize = mark
                    .strip_prefix(':')
                    .context("Invalid import mark")?
                    .parse()?;
                let entry = index
                    .entries_mut()
                    .get_mut(position - 1)
                    .context("Invalid import mark position")?;
                entry.id = gix_index::hash::ObjectId::from_hex(object.as_bytes())?;
            }
            ensure!(
                index.entries().iter().all(|entry| !entry.id.is_null()),
                "Missing imported snapshot object"
            );
            index.sort_entries();
            index.write(Default::default())?;
            drop(index);
        } else {
            scan(&mut std::io::sink())?;
        }
        #[cfg(not(windows))]
        scan(&mut std::io::sink())?;
        ensure!(!cancelled.load(Ordering::Relaxed), "Snapshot cancelled");
        let mut removed = NamedTempFile::new_in(self.directory.path())?;
        for (position, entry) in previous.entries().iter().enumerate() {
            if !seen[position] {
                let path = std::str::from_utf8(entry.path(&previous))?;
                write_path(&mut removed, Path::new(path))?;
            }
        }
        drop(previous);
        if removed.as_file().metadata()?.len() != 0 {
            self.git(
                &["update-index", "--force-remove", "-z", "--stdin"],
                Some(File::open(removed.path())?),
            )?;
        }
        // Unchanged entries retain their original stat cache and objects. Only
        // changed/new text is passed to Git; it keeps the existing clean rules.
        if remaining.as_file().metadata()?.len() != 0 {
            self.git(
                &["update-index", "--add", "-z", "--stdin"],
                Some(File::open(remaining.path())?),
            )?;
        }
        ensure!(!cancelled.load(Ordering::Relaxed), "Snapshot cancelled");
        #[cfg(windows)]
        if self.text_files == 0 {
            self.import_trees(cancelled)?;
        }
        self.text_files = text_files;
        let tree = self.git(&["write-tree"], None)?.trim().to_owned();
        let captured_at = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u64;
        #[cfg(windows)]
        self.git_stream(&["update-ref", "--stdin"], |input| {
            writeln!(input, "start\nupdate refs/snapshots/{slot} {tree}\nupdate refs/snapshot-index {tree}\nprepare\ncommit")?;
            Ok(())
        })?;
        #[cfg(not(windows))]
        self.git(
            &["update-ref", &format!("refs/snapshots/{slot}"), &tree],
            None,
        )?;
        let snapshot = Snapshot {
            slot: slot.to_owned(),
            captured_at,
            elapsed_ms: started.elapsed().as_millis() as u64,
            text_files,
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
        previous: &gix_index::File,
        cancelled: &AtomicBool,
        mut consume: impl FnMut(Scanned, Cow<'_, [u8]>) -> Result<()>,
    ) -> Result<()> {
        let walker = std::sync::Mutex::new(walker);
        std::thread::scope(|scope| {
            // Each reader holds at most one pending file; rendezvous adds no queue.
            let (sender, receiver) = std::sync::mpsc::sync_channel(0);
            let mut readers = Vec::with_capacity(4);
            for _ in 0..4 {
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
                            self.inspect_entry(entry, previous, &mut prefix)
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
        previous: &gix_index::File,
        prefix: &mut Vec<u8>,
    ) -> Result<Option<Scanned>> {
        let entry = entry?;
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            return Ok(None);
        }
        let path = entry.path().strip_prefix(&self.root)?.to_path_buf();
        let position = if previous.entries().is_empty() {
            None
        } else {
            git_path(&path)
                .ok()
                .and_then(|path| previous.entry_index_by_path(path.as_bytes().into()).ok())
        };
        if let Some(position) = position {
            #[cfg(windows)]
            let metadata = entry.metadata()?.into();
            #[cfg(not(windows))]
            let metadata = gix_index::fs::Metadata::from_path_no_follow(entry.path())?;
            let actual = gix_index::entry::Stat::from_fs(&metadata)?;
            let entry = &previous.entries()[position];
            let options = gix_index::entry::stat::Options {
                use_nsec: true,
                use_stdev: true,
                ..Default::default()
            };
            // Compare the native stat fields, retaining whole-second racy-index
            // checks. Preserving all checked metadata can still hide a change.
            if entry.stat.matches(&actual, options)
                && !entry.stat.is_racy(previous.timestamp(), Default::default())
            {
                return Ok(Some(Scanned::Cached(position, metadata.len())));
            }
        }
        let mut file = File::open(entry.path())
            .with_context(|| format!("Cannot read {}", entry.path().display()))?;
        let metadata = file.metadata()?;
        let size = metadata.len();
        #[cfg(windows)]
        let stat = gix_index::entry::Stat::from_fs(&metadata.into())?;
        prefix.resize(8192, 0);
        let prefix_len = file.read(prefix)?;
        prefix.truncate(prefix_len);
        if content_inspector::inspect(&prefix[..prefix_len]).is_binary() {
            Ok(Some(Scanned::Binary(path)))
        } else {
            #[cfg(windows)]
            if self.text_files == 0 && size <= INITIAL_BLOB_LIMIT {
                prefix.truncate(prefix.len().min(size as usize));
                (&mut file)
                    .take(size - prefix.len() as u64)
                    .read_to_end(prefix)?;
                ensure!(prefix.len() as u64 == size, "Snapshot file was truncated");
            }
            Ok(Some(Scanned::Text {
                path,
                size,
                position,
                #[cfg(windows)]
                stat,
            }))
        }
    }

    #[cfg(windows)]
    fn import_blob(
        prefix: Cow<'_, [u8]>,
        size: u64,
        mark: usize,
        input: &mut dyn Write,
    ) -> Result<bool> {
        if size > INITIAL_BLOB_LIMIT {
            return Ok(false);
        }
        // Bound extra memory, and only normalize unambiguous text into an empty index.
        // Ambiguous conversions go through Git's ordinary clean/write path.
        let mut bytes = prefix.into_owned();
        ensure!(bytes.len() as u64 == size, "Snapshot file was truncated");
        if bytes.contains(&b'\r') {
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
        writeln!(input, "blob\nmark :{mark}\ndata {}", bytes.len())?;
        input.write_all(&bytes)?;
        input.write_all(b"\n")?;
        Ok(true)
    }

    #[cfg(windows)]
    fn import_trees(&self, cancelled: &AtomicBool) -> Result<()> {
        let entries = NamedTempFile::new_in(self.directory.path())?;
        git_output(
            self.git_command(&["ls-files", "--stage"])?
                .stdout(entries.reopen()?),
            None,
        )?;
        let reference = "refs/snapshot-import";
        let result = self.import_objects(|input| {
            writeln!(input, "commit {reference}\ncommitter NeoWarp <snapshot@localhost> 0 +0000\ndata 0\n\ndeleteall")?;
            for line in BufReader::new(File::open(entries.path())?).lines() {
                ensure!(!cancelled.load(Ordering::Relaxed), "Snapshot cancelled");
                let line = line?;
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
        // update-index can emit many line-ending warnings. A file avoids filling
        // its stderr pipe while the producer is waiting for input backpressure.
        let errors = NamedTempFile::new_in(self.directory.path())?;
        let mut child = self
            .git_command(args)?
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
            #[cfg(windows)]
            self.git(&["update-ref", "-d", "refs/snapshot-index"], None)?;
            self.text_files = 0;
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
        {
            let packs = self.directory.path().join("objects/pack");
            let previous = fs::read_dir(&packs)?.collect::<std::io::Result<Vec<_>>>()?;
            let prefix = packs.join("pack");
            let objects = NamedTempFile::new_in(self.directory.path())?;
            git_output(
                self.git_command(&["rev-list", "--objects", "--all", "--no-object-names"])?
                    .stdout(objects.reopen()?),
                None,
            )?;
            // The scratch tree ref replaces --indexed-objects, so packing doesn't
            // load another index. Separate traversal from packing to release its
            // object graph first; names aren't needed with delta search disabled.
            let created = self.git(
                &[
                    "-c",
                    "core.bigFileThreshold=1m",
                    "pack-objects",
                    "--local",
                    "--non-empty",
                    "--delta-base-offset",
                    "--window=0",
                    "--threads=1",
                    prefix.to_str().context("Invalid pack path")?,
                ],
                Some(File::open(objects.path())?),
            )?;
            let retained = format!("pack-{}", created.trim());
            // Only this store owns these packs. Delete old files only after the
            // replacement pack and its index have been successfully installed.
            for entry in previous {
                let path = entry.path();
                if path.file_stem().and_then(|name| name.to_str()) != Some(&retained)
                    && matches!(
                        path.extension().and_then(|ext| ext.to_str()),
                        Some("pack" | "idx" | "rev")
                    )
                {
                    fs::remove_file(path)?;
                }
            }
        }
        let mut prune = self.git_command(&["prune", "--expire=now"])?;
        // Slots and the scratch tree ref already pin every live object.
        #[cfg(windows)]
        prune.env("GIT_INDEX_FILE", self.directory.path().join("empty-index"));
        git_output(&mut prune, None)?;
        Ok(())
    }

    fn git(&self, args: &[&str], input: Option<File>) -> Result<String> {
        String::from_utf8(self.git_bytes(args, input)?).context("Invalid Git output")
    }

    fn git_bytes(&self, args: &[&str], input: Option<File>) -> Result<Vec<u8>> {
        git_output(&mut self.git_command(args)?, input)
    }

    fn git_command(&self, args: &[&str]) -> Result<Command> {
        let directory = self
            .directory
            .path()
            .to_str()
            .context("Invalid cache path")?;
        let root = self.root.to_str().context("Invalid workspace path")?;
        let mut command = vec!["--git-dir", directory, "--work-tree", root];
        command.extend_from_slice(args);
        Ok(git_command(&self.root, &command))
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
    String::from_utf8(git_output(&mut git_command(root, args), None)?)
        .context("Invalid Git metadata")
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

fn git_output(command: &mut Command, input: Option<File>) -> Result<Vec<u8>> {
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
