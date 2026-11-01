//! Bounded reads of provider identity metadata, never a transcript index.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Days, Utc};
use serde::Deserialize;
use serde::de::{Deserializer, IgnoredAny, MapAccess, Visitor};

use super::config::{Host, Identity, Provider};

#[derive(Clone)]
pub(crate) struct Query {
    pub(crate) host: Host,
    pub(crate) directory: String,
    pub(crate) home: String,
    pub(crate) provider: Provider,
    pub(crate) environment: BTreeMap<String, String>,
    pub(crate) started_at: i64,
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
            join(&base, "sessions")
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

/// Hermes stores one JSON document. Stop after the three metadata fields, before messages.
struct HermesHeader {
    id: String,
    cwd: String,
    timestamp: String,
}

impl HermesHeader {
    fn read(reader: impl Read) -> Result<Self> {
        struct MetadataVisitor<'a>(&'a mut Option<HermesHeader>);
        impl<'de> Visitor<'de> for MetadataVisitor<'_> {
            type Value = ();

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("Hermes session metadata")
            }

            fn visit_map<A: MapAccess<'de>>(
                self,
                mut map: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                let (mut id, mut cwd, mut timestamp) = (None, None, None);
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "session_id" => id = Some(map.next_value::<String>()?),
                        "cwd" => cwd = Some(map.next_value::<String>()?),
                        "session_start" => timestamp = Some(map.next_value::<String>()?),
                        _ => {
                            map.next_value::<IgnoredAny>()?;
                        }
                    }
                    if let (Some(id), Some(cwd), Some(timestamp)) = (&id, &cwd, &timestamp) {
                        *self.0 = Some(HermesHeader {
                            id: id.clone(),
                            cwd: cwd.clone(),
                            timestamp: timestamp.clone(),
                        });
                        // Serde otherwise consumes the closing brace after visit_map returns.
                        // Stop here so a large messages array is never read or allocated.
                        return Err(serde::de::Error::custom("metadata complete"));
                    }
                }
                Err(serde::de::Error::custom("missing Hermes identity metadata"))
            }
        }
        let mut header = None;
        let result = serde_json::Deserializer::from_reader(reader)
            .deserialize_map(MetadataVisitor(&mut header));
        if let Some(header) = header {
            return Ok(header);
        }
        result?;
        bail!("missing Hermes identity metadata")
    }
}

fn read_header(path: &Path, provider: Provider) -> Result<Option<(String, String, String)>> {
    // A partial/oversize metadata record is unresolved, not permission to bind another candidate.
    let mut reader = BufReader::new(File::open(path)?.take(128 * 1024));
    if provider == Provider::Hermes {
        let header = HermesHeader::read(reader)?;
        return Ok(Some((header.id, header.cwd, header.timestamp)));
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
                    return Ok(Some((meta.id, meta.cwd, meta.timestamp)));
                }
            }
            Provider::Claude => {
                if let (Some(id), Some(cwd), Some(time)) =
                    (header.claude_id, header.cwd, header.timestamp)
                {
                    return Ok(Some((id, cwd, time)));
                }
            }
            Provider::Pi if header.kind == "session" => {
                if let (Some(id), Some(cwd), Some(time)) = (header.id, header.cwd, header.timestamp)
                {
                    return Ok(Some((id, cwd, time)));
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
        Provider::Codex => {
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
        Provider::Pi | Provider::Hermes => vec![native_root.clone()],
        Provider::Custom => return Ok(Vec::new()),
    };
    let mut result = Vec::new();
    let mut visited = 0;
    for scope in scopes {
        if !scope.exists() {
            continue;
        }
        let depth = if query.provider == Provider::Pi { 2 } else { 1 };
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
            let extension = if query.provider == Provider::Hermes {
                "json"
            } else {
                "jsonl"
            };
            if entry.path().extension().and_then(|value| value.to_str()) != Some(extension) {
                continue;
            }
            let modified: DateTime<Utc> = entry.metadata()?.modified()?.into();
            if modified.timestamp_millis() < query.started_at - 2000 {
                continue;
            }
            let Some((id, cwd, timestamp)) = read_header(entry.path(), query.provider)? else {
                continue;
            };
            let created = DateTime::parse_from_rfc3339(&timestamp)?.timestamp_millis();
            if created < query.started_at - 2000 || !query.host.same_path(&cwd, &query.directory) {
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
