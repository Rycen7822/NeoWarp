//! Local launch records for external CLI sessions. No conversations or credentials are imported.

pub(crate) mod command;
pub(crate) mod config;
mod provider;

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Result;
use uuid::Uuid;
use warp_terminal::model::BlockId;
use warpui::r#async::{SpawnedFutureHandle, Timer};
use warpui::{Entity, EntityId, ModelContext, SingletonEntity, WeakViewHandle};

use self::command::ParsedCommand;
use self::config::{Activation, Catalog, Host, Identity, Project, SavedShell, Session};
use self::provider::Query;
use crate::terminal::TerminalView;

pub(crate) struct Observation {
    pub(crate) terminal: WeakViewHandle<TerminalView>,
    pub(crate) terminal_id: EntityId,
    pub(crate) block: BlockId,
    pub(crate) host: Host,
    pub(crate) directory: String,
    pub(crate) home: String,
    pub(crate) shell: SavedShell,
    pub(crate) command: String,
    pub(crate) environment: BTreeMap<String, String>,
    pub(crate) parsed: ParsedCommand,
    pub(crate) started_at: i64,
}

struct LiveSession {
    session_id: Uuid,
    terminal: WeakViewHandle<TerminalView>,
    block: BlockId,
    query: Query,
    ambiguous: bool,
    lookup: Option<SpawnedFutureHandle>,
}

pub(crate) struct ProjectSessions {
    catalog: Catalog,
    path: PathBuf,
    ready: bool,
    error: Option<String>,
    pending: Vec<Observation>,
    live: HashMap<EntityId, LiveSession>,
}

impl ProjectSessions {
    pub(crate) fn new(ctx: &mut ModelContext<Self>) -> Self {
        let path = warp_core::paths::data_dir().join("project_sessions.json");
        let load_path = path.clone();
        ctx.spawn(async move { load(&load_path) }, |me, result, ctx| {
            match result {
                Ok(catalog) => {
                    me.catalog = catalog;
                    me.ready = true;
                    for observation in std::mem::take(&mut me.pending) {
                        me.observe(observation, ctx);
                    }
                }
                Err(error) => {
                    me.error = Some(format!("Cannot load {}: {error:#}", me.path.display()));
                    me.pending.clear();
                }
            }
            ctx.notify();
        });
        Self {
            catalog: Catalog::default(),
            path,
            ready: false,
            error: None,
            pending: Vec::new(),
            live: HashMap::new(),
        }
    }

    pub(crate) fn observe(&mut self, observation: Observation, ctx: &mut ModelContext<Self>) {
        if !self.ready {
            if self.error.is_none() {
                self.pending.push(observation);
            }
            return;
        }
        let Observation {
            terminal,
            terminal_id,
            block,
            host,
            directory,
            home,
            shell,
            command,
            mut environment,
            parsed,
            started_at,
        } = observation;
        if self
            .live
            .get(&terminal_id)
            .is_some_and(|live| live.block == block)
        {
            return;
        }
        self.end(terminal_id, None, ctx);
        for (key, value) in parsed.environment {
            let key = if cfg!(windows)
                && host == Host::Local
                && shell.kind == warp_terminal::shell::ShellType::PowerShell
            {
                key.to_ascii_uppercase()
            } else {
                key
            };
            environment.insert(key, value);
        }
        let mut identity = parsed.identity;
        if parsed.provider == config::Provider::Pi
            && let Some(path) = identity
                .as_mut()
                .and_then(|identity| identity.transcript.as_mut())
        {
            if !path.starts_with('/') && !std::path::Path::new(path.as_str()).is_absolute() {
                *path = format!("{directory}/{path}");
            }
            *path = path.replace('\\', "/").replace("/./", "/");
        }
        let query = Query {
            host: host.clone(),
            directory: directory.clone(),
            home,
            provider: parsed.provider,
            environment: environment.clone(),
            started_at,
        };
        let activation = Activation::from_environment(&environment);
        environment.retain(|key, _| {
            !matches!(
                key.as_str(),
                "VIRTUAL_ENV" | "CONDA_PREFIX" | "CONDA_DEFAULT_ENV" | "CONDA_EXE"
            )
        });
        let project_index = self
            .catalog
            .projects
            .iter()
            .position(|project| {
                project.host == host && host.same_path(&project.directory, &directory)
            })
            .unwrap_or_else(|| {
                self.catalog.projects.push(Project {
                    id: Uuid::new_v4(),
                    host,
                    directory,
                    pinned: false,
                    last_used: started_at,
                    manual_order: false,
                    sessions: Vec::new(),
                });
                self.catalog.projects.len() - 1
            });
        let project = &mut self.catalog.projects[project_index];
        project.last_used = started_at;
        let existing = identity.as_ref().and_then(|identity| {
            project.sessions.iter().position(|session| {
                session.provider == parsed.provider
                    && session.identity.as_ref().is_some_and(|saved| {
                        saved.same_session(identity, parsed.provider, &project.host)
                    })
            })
        });
        let session = Session {
            id: existing
                .map(|index| project.sessions[index].id)
                .unwrap_or_else(Uuid::new_v4),
            title: existing
                .map(|index| project.sessions[index].title.clone())
                .unwrap_or_else(|| parsed.provider.name().to_owned()),
            provider: parsed.provider,
            shell,
            environment,
            activation,
            command,
            identity,
            resume_command: None,
            pinned: existing.is_some_and(|index| project.sessions[index].pinned),
            last_used: started_at,
        };
        let session_id = session.id;
        let unresolved = session.identity.is_none();
        if let Some(index) = existing {
            project.sessions[index] = session;
        } else {
            project.sessions.insert(0, session);
        }
        let mut ambiguous = false;
        if unresolved {
            let catalog = &self.catalog;
            for live in self.live.values_mut().filter(|live| {
                live.query.same_store(&query)
                    && query
                        .host
                        .same_path(&live.query.directory, &query.directory)
                    && catalog
                        .session(live.session_id)
                        .is_some_and(|session| session.identity.is_none())
            }) {
                live.ambiguous = true;
                ambiguous = true;
            }
        }
        self.live.insert(
            terminal_id,
            LiveSession {
                session_id,
                terminal,
                block,
                query,
                ambiguous,
                lookup: None,
            },
        );
        self.save(ctx);
        if unresolved {
            self.schedule_lookup(terminal_id, ctx);
        }
    }

    fn schedule_lookup(&mut self, terminal_id: EntityId, ctx: &mut ModelContext<Self>) {
        let Some(live) = self.live.get(&terminal_id) else {
            return;
        };
        if live.ambiguous {
            return;
        }
        let query = live.query.clone();
        let block = live.block.clone();
        let task = ctx.spawn(
            async move {
                Timer::after(Duration::from_secs(2)).await;
                provider::candidates(&query)
            },
            move |me, result, ctx| {
                let Some(live) = me
                    .live
                    .get_mut(&terminal_id)
                    .filter(|live| live.block == block)
                else {
                    return;
                };
                live.lookup = None;
                let session_id = live.session_id;
                let running = live.terminal.upgrade(ctx).is_some_and(|terminal| {
                    terminal.as_ref(ctx).project_session_is_running(&block)
                });
                let ambiguous = live.ambiguous;
                if !ambiguous && let Ok(candidates) = result {
                    let candidates = me.unbound_candidates(session_id, candidates);
                    if candidates.len() > 1 {
                        me.live
                            .get_mut(&terminal_id)
                            .expect("live entry was checked")
                            .ambiguous = true;
                    } else if let Some(identity) = candidates.into_iter().next() {
                        me.bind_identity(session_id, identity, ctx);
                    }
                }
                if !running {
                    me.end(terminal_id, None, ctx);
                    return;
                }
                if me
                    .catalog
                    .session_mut(session_id)
                    .is_some_and(|session| session.identity.is_none())
                {
                    me.schedule_lookup(terminal_id, ctx);
                }
            },
        );
        self.live
            .get_mut(&terminal_id)
            .expect("live entry exists")
            .lookup = Some(task);
    }

    pub(crate) fn end(
        &mut self,
        terminal_id: EntityId,
        block: Option<&BlockId>,
        ctx: &mut ModelContext<Self>,
    ) {
        if self
            .live
            .get(&terminal_id)
            .is_none_or(|live| block.is_some_and(|block| *block != live.block))
        {
            return;
        }
        let Some(mut live) = self.live.remove(&terminal_id) else {
            return;
        };
        if let Some(task) = live.lookup.take() {
            task.abort();
        }
        if !live.ambiguous
            && self
                .catalog
                .session_mut(live.session_id)
                .is_some_and(|session| session.identity.is_none())
        {
            // A short CLI can exit before the first periodic lookup. Make one final bounded read.
            ctx.spawn(
                async move { provider::candidates(&live.query) },
                move |me, result, ctx| {
                    if let Ok(candidates) = result {
                        let candidates = me.unbound_candidates(live.session_id, candidates);
                        if candidates.len() != 1 {
                            return;
                        }
                        me.bind_identity(
                            live.session_id,
                            candidates.into_iter().next().expect("one candidate"),
                            ctx,
                        );
                    }
                },
            );
        }
        ctx.notify();
    }

    fn bind_identity(&mut self, id: Uuid, identity: Identity, ctx: &mut ModelContext<Self>) {
        let Some(session) = self
            .catalog
            .session_mut(id)
            .filter(|session| session.identity.is_none())
        else {
            return;
        };
        session.identity = Some(identity);
        self.save(ctx);
    }

    fn unbound_candidates(&self, id: Uuid, candidates: Vec<Identity>) -> Vec<Identity> {
        let Some(project) = self
            .catalog
            .projects
            .iter()
            .find(|project| project.sessions.iter().any(|session| session.id == id))
        else {
            return Vec::new();
        };
        let current = project
            .sessions
            .iter()
            .find(|session| session.id == id)
            .expect("project contains this session");
        candidates
            .into_iter()
            .filter(|candidate| {
                !project.sessions.iter().any(|session| {
                    session.id != id
                        && session.provider == current.provider
                        && session.identity.as_ref().is_some_and(|known| {
                            known.same_session(candidate, current.provider, &project.host)
                        })
                })
            })
            .collect()
    }

    fn save(&mut self, ctx: &mut ModelContext<Self>) {
        // Like the existing tab-config writer, one owner writes a small local file synchronously.
        // There is no detached writer that can overwrite a newer edit after shutdown.
        self.error = write(&self.path, &self.catalog)
            .err()
            .map(|error| format!("Cannot save project sessions: {error:#}"));
        ctx.notify();
    }
}

fn load(path: &Path) -> Result<Catalog> {
    match fs::read(path) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Catalog::default()),
        Err(error) => Err(error.into()),
    }
}

fn write(path: &Path, catalog: &Catalog) -> Result<()> {
    let parent = path.parent().expect("catalog path has a parent");
    fs::create_dir_all(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer_pretty(file.as_file_mut(), catalog)?;
    file.flush()?;
    file.persist(path)?;
    Ok(())
}

impl Entity for ProjectSessions {
    type Event = ();
}
impl SingletonEntity for ProjectSessions {}

impl Drop for ProjectSessions {
    fn drop(&mut self) {
        for live in self.live.values_mut() {
            if let Some(task) = live.lookup.take() {
                task.abort();
            }
        }
    }
}
