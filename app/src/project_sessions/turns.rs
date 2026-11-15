//! Read-only native transcript observations. These are evidence, not TUI hooks.
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use serde_json::Value;
use warp_terminal::model::BlockId;

use super::config::{Identity, Provider};
use super::provider::{self, Query};

#[derive(Clone)]
pub(crate) struct Source {
    pub(crate) block: BlockId,
    pub(super) query: Query,
    pub(super) identity: Option<Identity>,
}

impl Source {
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
}

impl Cursor {
    pub(crate) fn at_end(path: &Path) -> Result<Self> {
        let mut file = File::open(path)?;
        let offset = file.metadata()?.len();
        let mut byte = [b'\n'];
        if offset > 0 {
            file.seek(SeekFrom::End(-1))?;
            file.read_exact(&mut byte)?;
        }
        Ok(Self {
            offset,
            skip_partial: byte[0] != b'\n',
        })
    }
}

pub(crate) struct UserRequest {
    pub(crate) created_at: u64,
}

pub(crate) fn read(
    path: &Path,
    provider: Provider,
    mut cursor: Cursor,
) -> Result<(Cursor, Vec<UserRequest>)> {
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
        if let Some(request) = user_request(&record, provider)? {
            requests.push(request);
        }
    }
    ensure!(
        consumed != 0 || bytes.len() < MAX_READ as usize,
        "Transcript record exceeds reader limit"
    );
    cursor.offset += consumed as u64;
    Ok((cursor, requests))
}

fn user_request(record: &Value, provider: Provider) -> Result<Option<UserRequest>> {
    let timestamp = match provider {
        Provider::Codex
            if record["type"] == "event_msg" && record["payload"]["type"] == "user_message" =>
        {
            if record["payload"]
                .get("kind")
                .is_some_and(|kind| kind != "plain")
            {
                return Ok(None);
            }
            &record["timestamp"]
        }
        Provider::Pi if record["type"] == "message" && record["message"]["role"] == "user" => {
            &record["message"]["timestamp"]
        }
        Provider::Claude if record["type"] == "user" && record["message"]["role"] == "user" => {
            if ["isMeta", "isSidechain", "isCompactSummary"]
                .iter()
                .any(|key| record[*key] == true)
                || record["userType"] == "external"
            {
                return Ok(None);
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
                return Ok(None);
            }
            &record["timestamp"]
        }
        _ => return Ok(None),
    };
    let created_at = if let Some(milliseconds) = timestamp.as_u64() {
        milliseconds
    } else {
        chrono::DateTime::parse_from_rfc3339(
            timestamp
                .as_str()
                .context("Missing user request timestamp")?,
        )?
        .timestamp_millis()
        .try_into()
        .context("Invalid user request timestamp")?
    };
    Ok(Some(UserRequest { created_at }))
}
