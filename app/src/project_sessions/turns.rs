//! Bounded, read-only observations of native tool results.
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use serde_json::Value;
use warp_terminal::model::BlockId;

use super::config::{Identity, Provider};
use super::provider::{self, Query};
use super::tool_changes::{Change, Tools};

#[derive(Clone)]
pub(crate) struct Source {
    pub(crate) block: BlockId,
    pub(super) query: Query,
    pub(super) identity: Option<Identity>,
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
            return Ok(self.query.host.native_path(path));
        }
        // Old launch records can have an identity without its transcript path.
        // Reuse the bounded identity reader once, matching the exact session ID.
        let mut query = self.query.clone();
        query.resuming = true;
        query.started_at = 0;
        Ok(provider::candidates(&query)?
            .into_iter()
            .find(|candidate| candidate.same_session(identity, query.provider, &query.host))
            .and_then(|candidate| candidate.transcript)
            .and_then(|path| query.host.native_path(&path)))
    }
}

#[derive(Clone, Default)]
pub(crate) struct Cursor {
    offset: u64,
    skip_partial: bool,
    tools: Tools,
}

impl Cursor {
    pub(crate) fn tail(path: &Path) -> Result<Self> {
        let offset = File::open(path)?
            .metadata()?
            .len()
            .saturating_sub(4 * 1024 * 1024);
        Ok(Self {
            offset,
            skip_partial: offset > 0,
            tools: Tools::default(),
        })
    }
}

pub(crate) enum Observation {
    Request,
    Change(Change),
}

pub(crate) fn read(
    path: &Path,
    provider: Provider,
    mut cursor: Cursor,
    since: u64,
) -> Result<(Cursor, Vec<Observation>)> {
    const MAX_READ: u64 = 4 * 1024 * 1024;
    let mut file = File::open(path)?;
    ensure!(
        file.metadata()?.len() >= cursor.offset,
        "Transcript was rewritten"
    );
    file.seek(SeekFrom::Start(cursor.offset))?;
    let mut bytes = Vec::new();
    file.take(MAX_READ).read_to_end(&mut bytes)?;
    let mut requests = Vec::new();
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
        let record: Value = serde_json::from_slice(line).context("Invalid transcript record")?;
        let timestamp = record["message"]["timestamp"].as_u64().or_else(|| {
            record["timestamp"]
                .as_str()
                .and_then(|text| chrono::DateTime::parse_from_rfc3339(text).ok())
                .map(|time| time.timestamp_millis().max(0) as u64)
        });
        if timestamp.is_none_or(|timestamp| timestamp < since) {
            continue;
        }
        if user_request(&record, provider) {
            cursor.tools = Tools::default();
            requests.push(Observation::Request);
        } else {
            requests.extend(
                cursor
                    .tools
                    .observe(&record, provider)
                    .into_iter()
                    .map(Observation::Change),
            );
        }
    }
    if consumed == 0 && bytes.len() == MAX_READ as usize {
        // Skip an oversized record incrementally; never reread it forever.
        cursor.skip_partial = true;
        consumed = bytes.len();
    }
    cursor.offset += consumed as u64;
    Ok((cursor, requests))
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
