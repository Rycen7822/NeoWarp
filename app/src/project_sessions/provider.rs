//! Bounded reads of provider identity metadata, never a transcript index.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Days, Utc};
use serde::Deserialize;

use super::config::{Host, Identity, Provider};

#[derive(Clone)]
pub(crate) struct Query {
    pub(crate) host: Host,
    pub(crate) directory: String,
    pub(crate) home: String,
    pub(crate) provider: Provider,
    pub(crate) environment: BTreeMap<String, String>,
    pub(crate) started_at: i64,
    pub(crate) resuming: bool,
}

impl Query {
    pub(crate) fn same_store(&self, other: &Self) -> bool {
        self.host == other.host
            && self.provider == other.provider
            && self.host.same_path(&self.root(), &other.root())
    }

    fn root(&self) -> String {
        let (variable, default) = match self.provider {
            Provider::Codex => ("CODEX_HOME", ".codex"),
            Provider::Claude => ("CLAUDE_CONFIG_DIR", ".claude"),
            Provider::Pi => ("PI_CODING_AGENT_DIR", ".pi/agent"),
            Provider::Hermes => ("HERMES_HOME", ".hermes"),
            Provider::Custom => return String::new(),
        };
        let base = self.environment.get(variable).cloned().unwrap_or_else(|| {
            if cfg!(windows) && self.host == Host::Local && self.provider == Provider::Hermes {
                let local = join(&self.home, "AppData/Local/hermes");
                if self
                    .host
                    .native_path(&local)
                    .is_some_and(|path| path.is_dir())
                {
                    return local;
                }
            }
            join(&self.home, default)
        });
        let base = if base.starts_with('~') {
            join(
                &self.home,
                base.trim_start_matches('~').trim_start_matches('/'),
            )
        } else if Path::new(&base).is_absolute() || base.starts_with('/') {
            base
        } else {
            join(&self.directory, &base)
        };
        if self.provider == Provider::Claude {
            join(&base, "projects")
        } else if self.provider == Provider::Pi
            && base
                .trim_end_matches(['/', '\\'])
                .rsplit(['/', '\\'])
                .next()
                == Some("sessions")
        {
            base
        } else {
            join(
                &base,
                if self.provider == Provider::Hermes {
                    "terminal-sessions"
                } else {
                    "sessions"
                },
            )
        }
    }
}

fn join(base: &str, child: &str) -> String {
    format!("{}/{child}", base.trim_end_matches(['/', '\\']))
}

#[derive(Default, Deserialize)]
struct Header {
    #[serde(rename = "type", default)]
    kind: String,
    id: Option<String>,
    cwd: Option<String>,
    timestamp: Option<String>,
    #[serde(rename = "sessionId")]
    claude_id: Option<String>,
    payload: Option<CodexHeader>,
}

#[derive(Deserialize)]
struct CodexHeader {
    id: String,
    cwd: String,
    timestamp: String,
}

/// Hermes publishes this small per-terminal record on launch and resume.
#[derive(Deserialize)]
struct HermesHeader {
    session_id: String,
    cwd: String,
    ts: f64,
}

fn read_header(path: &Path, provider: Provider) -> Result<Option<(String, String, i64)>> {
    // A partial/oversize metadata record is unresolved, not permission to bind another candidate.
    let mut reader = BufReader::new(File::open(path)?.take(128 * 1024));
    if provider == Provider::Hermes {
        let header: HermesHeader = serde_json::from_reader(reader)?;
        return Ok(Some((
            header.session_id,
            header.cwd,
            (header.ts * 1000.0) as i64,
        )));
    }
    let mut line = String::new();
    for _ in 0..64 {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        let header: Header = serde_json::from_str(&line)?;
        match provider {
            Provider::Codex if header.kind == "session_meta" => {
                if let Some(meta) = header.payload {
                    return Ok(Some((
                        meta.id,
                        meta.cwd,
                        DateTime::parse_from_rfc3339(&meta.timestamp)?.timestamp_millis(),
                    )));
                }
            }
            Provider::Claude => {
                if let (Some(id), Some(cwd), Some(time)) =
                    (header.claude_id, header.cwd, header.timestamp)
                {
                    return Ok(Some((
                        id,
                        cwd,
                        DateTime::parse_from_rfc3339(&time)?.timestamp_millis(),
                    )));
                }
            }
            Provider::Pi if header.kind == "session" => {
                if let (Some(id), Some(cwd), Some(time)) = (header.id, header.cwd, header.timestamp)
                {
                    return Ok(Some((
                        id,
                        cwd,
                        DateTime::parse_from_rfc3339(&time)?.timestamp_millis(),
                    )));
                }
            }
            _ => {}
        }
    }
    Ok(None)
}

pub(crate) fn candidates(query: &Query) -> Result<Vec<Identity>> {
    let root = query.root();
    let native_root = query
        .host
        .native_path(&root)
        .context("CLI data directory is not an absolute path")?;
    let scopes: Vec<PathBuf> = match query.provider {
        Provider::Codex if !query.resuming => {
            let date = Utc::now().date_naive();
            [
                date.pred_opt(),
                Some(date),
                date.checked_add_days(Days::new(1)),
            ]
            .into_iter()
            .flatten()
            .map(|date| native_root.join(date.format("%Y/%m/%d").to_string()))
            .collect()
        }
        Provider::Claude => {
            let encoded: String = query
                .directory
                .replace('\\', "/")
                .trim_end_matches('/')
                .chars()
                .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
                .collect();
            vec![native_root.join(encoded)]
        }
        Provider::Codex | Provider::Pi | Provider::Hermes => vec![native_root.clone()],
        Provider::Custom => return Ok(Vec::new()),
    };
    let mut result = Vec::new();
    let mut visited = 0;
    for scope in scopes {
        if !scope.exists() {
            continue;
        }
        let depth = match query.provider {
            Provider::Codex if query.resuming => 4,
            Provider::Pi => 2,
            _ => 1,
        };
        for entry in walkdir::WalkDir::new(scope)
            .max_depth(depth)
            .follow_links(false)
        {
            visited += 1;
            if visited > 4096 {
                bail!("CLI session directory exceeds the bounded identity lookup");
            }
            let entry = entry?;
            if !entry.file_type().is_file() {
                continue;
            }
            if query.provider != Provider::Hermes
                && entry.path().extension().and_then(|value| value.to_str()) != Some("jsonl")
            {
                continue;
            }
            let modified: DateTime<Utc> = entry.metadata()?.modified()?.into();
            if modified.timestamp_millis() < query.started_at - 2000 {
                continue;
            }
            let Some((id, cwd, created)) = read_header(entry.path(), query.provider)? else {
                continue;
            };
            // Resume appends to an old session: recent writes, not its original creation time,
            // identify candidates. New launches still require a freshly created session.
            if ((!query.resuming || query.provider == Provider::Hermes)
                && created < query.started_at - 2000)
                || !query.host.same_path(&cwd, &query.directory)
            {
                continue;
            }
            let transcript = if query.provider == Provider::Pi {
                let relative = entry.path().strip_prefix(&native_root)?;
                Some(join(&root, &relative.to_string_lossy().replace('\\', "/")))
            } else {
                None
            };
            result.push(Identity { id, transcript });
        }
    }
    Ok(result)
}
