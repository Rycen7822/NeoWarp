//! Bounded, read-only observations of native tool results.
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::Result;
use serde_json::Value;
use warp_terminal::model::BlockId;

use super::config::{Identity, Provider};
use super::provider::{self, Query};
use super::tool_changes::{Change, Tools};

#[derive(Clone)]
pub(crate) struct Source {
    pub(crate) block: BlockId,
    pub(super) query: Query,
    pub(crate) identity: Option<Identity>,
    pub(crate) ambiguous: bool,
}

impl Source {
    pub(crate) fn native_path(&self, path: &str) -> Option<PathBuf> {
        if path.starts_with('/') || Path::new(path).is_absolute() {
            self.query.host.native_path(path)
        } else if let Some(path) = path.strip_prefix("~/") {
            Some(self.query.host.native_path(&self.query.home)?.join(path))
        } else {
            Some(
                self.query
                    .host
                    .native_path(&self.query.directory)?
                    .join(path),
            )
        }
    }

    pub(crate) fn provider(&self) -> Provider {
        self.query.provider
    }

    pub(crate) fn supports_turns(&self) -> bool {
        matches!(
            self.query.provider,
            Provider::Codex | Provider::Claude | Provider::Pi
        )
    }

    pub(crate) fn locate(&self) -> Result<Option<PathBuf>> {
        if !self.supports_turns() {
            return Ok(None);
        }
        let Some(identity) = &self.identity else {
            return Ok(None);
        };
        if let Some(path) = &identity.transcript {
            return Ok(self.native_path(path));
        }
        // Old launch records can have an identity without its transcript path.
        // Reuse the bounded identity reader once, matching the exact session ID.
        let mut query = self.query.clone();
        query.resuming = true;
        query.started_at = 0;
        let candidates = provider::candidates(&query)?;
        if let Some(exact) = candidates
            .iter()
            .find(|candidate| candidate.same_session(identity, query.provider, &query.host))
        {
            return Ok(exact
                .transcript
                .as_deref()
                .and_then(|path| self.native_path(path)));
        }
        let mut matches = candidates.into_iter().filter(|candidate| {
            query.provider == Provider::Pi && candidate.id.starts_with(&identity.id)
        });
        let first = matches.next().and_then(|candidate| candidate.transcript);
        anyhow::ensure!(matches.next().is_none(), "Ambiguous Pi session ID");
        Ok(first.as_deref().and_then(|path| self.native_path(path)))
    }
}

#[derive(Clone, Default)]
pub(crate) struct Cursor {
    pub(crate) full_batch: bool,
    offset: u64,
    skip_partial: bool,
    checkpoint: Vec<u8>,
    modified: Option<SystemTime>,
    tools: Tools,
}

const MAX_READ: u64 = 4 * 1024 * 1024;
const CHECKPOINT_BYTES: usize = 256;

impl Cursor {
    fn tail(file: &mut File, length: u64) -> Result<Self> {
        let offset = length.saturating_sub(MAX_READ);
        let mut previous = [b'\n'];
        if offset > 0 {
            file.seek(SeekFrom::Start(offset - 1))?;
            file.read_exact(&mut previous)?;
        }
        Ok(Self {
            offset,
            skip_partial: previous[0] != b'\n',
            ..Self::default()
        })
    }
}

pub(crate) enum Observation {
    Request,
    Reset,
    Incomplete,
    Change(Change),
}

pub(crate) fn read(
    path: &Path,
    provider: Provider,
    cursor: Option<Cursor>,
) -> Result<(Cursor, Vec<Observation>)> {
    let mut file = File::open(path)?;
    let metadata = file.metadata()?;
    let length = metadata.len();
    let modified = metadata.modified().ok();
    let mut observations = Vec::new();
    let mut cursor = match cursor {
        Some(cursor) => cursor,
        None => Cursor::tail(&mut file, length)?,
    };
    if length == cursor.offset && modified.is_some() && modified == cursor.modified {
        cursor.full_batch = false;
        return Ok((cursor, observations));
    }
    let mut previous = [0; CHECKPOINT_BYTES];
    let check = &mut previous[..cursor.checkpoint.len()];
    if length >= cursor.offset && !check.is_empty() {
        file.seek(SeekFrom::Start(cursor.offset - check.len() as u64))?;
        file.read_exact(check)?;
    }
    let same_size_rewrite =
        length == cursor.offset && modified != cursor.modified && !check.is_empty();
    if length < cursor.offset || same_size_rewrite || &*check != cursor.checkpoint.as_slice() {
        cursor = Cursor::tail(&mut file, length)?;
        observations.push(Observation::Reset);
    }
    cursor.modified = modified;
    file.seek(SeekFrom::Start(cursor.offset))?;
    let mut bytes = Vec::new();
    file.take(MAX_READ).read_to_end(&mut bytes)?;
    cursor.full_batch = bytes.len() == MAX_READ as usize;
    let mut consumed = 0;
    for (index, byte) in bytes.iter().enumerate() {
        if *byte != b'\n' {
            continue;
        }
        let line = &bytes[consumed..index];
        consumed = index + 1;
        if std::mem::take(&mut cursor.skip_partial) || line.is_empty() {
            continue;
        }
        let Ok(record) = serde_json::from_slice::<Value>(line) else {
            if !matches!(observations.last(), Some(Observation::Incomplete)) {
                observations.push(Observation::Incomplete);
            }
            continue;
        };
        if user_request(&record, provider) {
            cursor.tools.new_request();
            observations.push(Observation::Request);
        } else {
            observations.extend(
                cursor
                    .tools
                    .observe(&record, provider)
                    .into_iter()
                    .map(Observation::Change),
            );
            if std::mem::take(&mut cursor.tools.limited) {
                observations.push(Observation::Incomplete);
            }
        }
    }
    if consumed == 0 && cursor.full_batch {
        // Skip an oversized record incrementally; never reread it forever.
        cursor.skip_partial = true;
        consumed = bytes.len();
        observations.push(Observation::Incomplete);
    }
    if consumed > 0 {
        cursor.checkpoint = bytes[consumed.saturating_sub(CHECKPOINT_BYTES)..consumed].to_vec();
    }
    cursor.offset += consumed as u64;
    Ok((cursor, observations))
}

fn user_request(record: &Value, provider: Provider) -> bool {
    match provider {
        Provider::Codex
            if record["type"] == "event_msg" && record["payload"]["type"] == "user_message" =>
        {
            if record["payload"]
                .get("kind")
                .is_some_and(|kind| kind != "plain")
            {
                return false;
            }
            true
        }
        Provider::Pi if record["type"] == "message" && record["message"]["role"] == "user" => true,
        Provider::Claude if record["type"] == "user" && record["message"]["role"] == "user" => {
            if ["isMeta", "isSidechain", "isCompactSummary"]
                .iter()
                .any(|key| record[*key] == true)
                || record["userType"] == "external"
            {
                return false;
            }
            let content = &record["message"]["content"];
            if !content.is_string()
                && !content.as_array().is_some_and(|parts| {
                    !parts.is_empty()
                        && parts
                            .iter()
                            .all(|part| matches!(part["type"].as_str(), Some("text" | "image")))
                })
            {
                return false;
            }
            true
        }
        _ => false,
    }
}

#[cfg(test)]
#[path = "turns_tests.rs"]
mod tests;
