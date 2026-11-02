//! Local launch records for external CLI sessions. No conversations or credentials are imported.

pub(crate) mod command;
pub(crate) mod config;
pub(crate) mod launch;
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
    pub(crate) block: BlockId,
    pub(crate) host: Host,
    pub(crate) directory: String,
    pub(crate) home: String,
    pub(crate) shell: SavedShell,
    pub(crate) command: String,
    pub(crate) environment: BTreeMap<String, String>,
    pub(crate) parsed: Option<ParsedCommand>,
    pub(crate) launched: bool,
    pub(crate) started_at: i64,
}

struct Running {
    block: BlockId,
    query: Query,
    ambiguous: bool,
    lookup: Option<SpawnedFutureHandle>,
}

impl Drop for Running {
    fn drop(&mut self) {
        if let Some(task) = self.lookup.take() {
            task.abort();
        }
    }
}

struct LiveSession {
    session_id: Uuid,
    terminal: WeakViewHandle<TerminalView>,
    // None reserves this terminal while bootstrap and its pending command are starting.
    running: Option<Running>,
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

    pub(crate) fn starting(&self, terminal: EntityId) -> bool {
        self.live
            .get(&terminal)
            .is_some_and(|live| live.running.is_none())
    }

    pub(crate) fn active_terminal(&self, id: Uuid, app: &warpui::AppContext) -> Option<EntityId> {
        self.live
            .values()
            .filter(|live| live.session_id == id)
            .filter_map(|live| {
                let terminal = live.terminal.upgrade(app)?;
                if let Some(running) = &live.running {
                    terminal
                        .as_ref(app)
                        .project_session_is_running(&running.block)
                        .then_some((running.query.started_at, terminal.id()))
                } else {
                    Some((i64::MAX, terminal.id()))
                }
            })
            .max_by_key(|(time, _)| *time)
            .map(|(_, terminal)| terminal)
    }

    pub(crate) fn prepare_launch(
        &self,
        id: Uuid,
        intent: launch::Intent,
        app: &warpui::AppContext,
    ) -> Result<launch::Launch> {
        anyhow::ensure!(self.ready, "Project sessions are not available yet");
        let project = self
            .catalog
            .projects
            .iter()
            .find(|project| project.sessions.iter().any(|session| session.id == id))
            .ok_or_else(|| anyhow::anyhow!("Session no longer exists"))?;
        let session = project
            .sessions
            .iter()
            .find(|session| session.id == id)
            .expect("project contains the session");
        launch::prepare(project, session, intent, app)
    }

    pub(crate) fn begin_launch(
        &mut self,
        id: Uuid,
        terminal: WeakViewHandle<TerminalView>,
        ctx: &mut ModelContext<Self>,
    ) {
        self.live
            .retain(|_, live| live.terminal.upgrade(ctx).is_some());
        self.live.insert(
            terminal.id(),
            LiveSession {
                session_id: id,
                terminal,
                running: None,
            },
        );
        self.touch(id, ctx);
    }

    pub(crate) fn touch(&mut self, id: Uuid, ctx: &mut ModelContext<Self>) {
        let now = chrono::Utc::now().timestamp_millis();
        for project in &mut self.catalog.projects {
            if let Some(session) = project.sessions.iter_mut().find(|session| session.id == id) {
                session.last_used = now;
                project.last_used = now;
                break;
            }
        }
        self.save(ctx);
    }

    pub(crate) fn observe(&mut self, mut observation: Observation, ctx: &mut ModelContext<Self>) {
        if !self.ready {
            if self.error.is_none() {
                self.pending.push(observation);
            }
            return;
        }
        let terminal_id = observation.terminal.id();
        // The notifying terminal is temporarily taken out of the UI table during its update.
        self.live
            .retain(|id, live| *id == terminal_id || live.terminal.upgrade(ctx).is_some());
        if self
            .live
            .get(&terminal_id)
            .and_then(|live| live.running.as_ref())
            .is_some_and(|running| running.block == observation.block)
        {
            return;
        }
        if observation.launched
            && let Some(live) = self
                .live
                .get(&terminal_id)
                .filter(|live| live.running.is_none())
        {
            let id = live.session_id;
            let project = self
                .catalog
                .projects
                .iter()
                .find(|project| project.sessions.iter().any(|session| session.id == id))
                .expect("launch record was checked before creating the terminal");
            let session = project
                .sessions
                .iter()
                .find(|session| session.id == id)
                .expect("project contains the session");
            observation.environment.extend(session.environment.clone());
            let query = Query {
                host: project.host.clone(),
                directory: project.directory.clone(),
                home: observation.home,
                provider: session.provider,
                environment: observation.environment,
                started_at: observation.started_at,
            };
            self.register_running(id, observation.terminal, observation.block, query, ctx);
            return;
        }
        let Some(parsed) = observation.parsed else {
            return;
        };
        self.end(terminal_id, None, ctx);
        let Observation {
            terminal,
            block,
            host,
            directory,
            home,
            shell,
            command,
            mut environment,
            started_at,
            ..
        } = observation;
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
            resume_command: existing
                .and_then(|index| project.sessions[index].resume_command.clone()),
            pinned: existing.is_some_and(|index| project.sessions[index].pinned),
            last_used: started_at,
        };
        let id = session.id;
        if let Some(index) = existing {
            project.sessions[index] = session;
        } else {
            project.sessions.insert(0, session);
        }
        self.register_running(id, terminal, block, query, ctx);
        self.save(ctx);
    }

    fn register_running(
        &mut self,
        id: Uuid,
        terminal: WeakViewHandle<TerminalView>,
        block: BlockId,
        query: Query,
        ctx: &mut ModelContext<Self>,
    ) {
        let unresolved = self.catalog.session(id).is_some_and(|session| {
            session.identity.is_none() && session.provider != config::Provider::Custom
        });
        let mut ambiguous = false;
        if unresolved {
            let catalog = &self.catalog;
            for live in self.live.values_mut() {
                if let Some(running) = &mut live.running
                    && running.query.same_store(&query)
                    && query
                        .host
                        .same_path(&running.query.directory, &query.directory)
                    && catalog
                        .session(live.session_id)
                        .is_some_and(|session| session.identity.is_none())
                {
                    running.ambiguous = true;
                    ambiguous = true;
                }
            }
        }
        let terminal_id = terminal.id();
        self.live.insert(
            terminal_id,
            LiveSession {
                session_id: id,
                terminal,
                running: Some(Running {
                    block,
                    query,
                    ambiguous,
                    lookup: None,
                }),
            },
        );
        if unresolved {
            self.schedule_lookup(terminal_id, ctx);
        }
        ctx.notify();
    }

    fn schedule_lookup(&mut self, terminal_id: EntityId, ctx: &mut ModelContext<Self>) {
        let Some(running) = self
            .live
            .get(&terminal_id)
            .and_then(|live| live.running.as_ref())
        else {
            return;
        };
        if running.ambiguous {
            return;
        }
        let query = running.query.clone();
        let block = running.block.clone();
        let task = ctx.spawn(
            async move {
                Timer::after(Duration::from_secs(2)).await;
                provider::candidates(&query)
            },
            move |me, result, ctx| {
                let Some(live) = me.live.get_mut(&terminal_id) else {
                    return;
                };
                let Some(running) = live
                    .running
                    .as_mut()
                    .filter(|running| running.block == block)
                else {
                    return;
                };
                running.lookup = None;
                let id = live.session_id;
                let active = live.terminal.upgrade(ctx).is_some_and(|terminal| {
                    terminal.as_ref(ctx).project_session_is_running(&block)
                });
                if !running.ambiguous
                    && let Ok(candidates) = result
                {
                    let candidates = me.unbound_candidates(id, candidates);
                    if candidates.len() > 1 {
                        me.live
                            .get_mut(&terminal_id)
                            .and_then(|live| live.running.as_mut())
                            .expect("running entry was checked")
                            .ambiguous = true;
                    } else if let Some(identity) = candidates.into_iter().next() {
                        me.bind_identity(id, identity, ctx);
                    }
                }
                if !active {
                    me.end(terminal_id, None, ctx);
                    return;
                }
                if me
                    .catalog
                    .session(id)
                    .is_some_and(|session| session.identity.is_none())
                {
                    me.schedule_lookup(terminal_id, ctx);
                }
            },
        );
        self.live
            .get_mut(&terminal_id)
            .and_then(|live| live.running.as_mut())
            .expect("running entry exists")
            .lookup = Some(task);
    }

    pub(crate) fn end(
        &mut self,
        terminal: EntityId,
        block: Option<&BlockId>,
        ctx: &mut ModelContext<Self>,
    ) {
        let Some(live) = self.live.get(&terminal) else {
            return;
        };
        if let Some(block) = block
            && live
                .running
                .as_ref()
                .is_none_or(|running| *block != running.block)
        {
            return;
        }
        let Some(live) = self.live.remove(&terminal) else {
            return;
        };
        if let Some(running) = &live.running
            && !running.ambiguous
            && self
                .catalog
                .session(live.session_id)
                .is_some_and(|session| {
                    session.identity.is_none() && session.provider != config::Provider::Custom
                })
        {
            let id = live.session_id;
            let query = running.query.clone();
            ctx.spawn(
                async move { provider::candidates(&query) },
                move |me, result, ctx| {
                    if let Ok(candidates) = result {
                        let mut candidates = me.unbound_candidates(id, candidates);
                        if candidates.len() == 1 {
                            me.bind_identity(id, candidates.remove(0), ctx);
                        }
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
            .expect("project contains the session");
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
        // A single owner writes the small file; no detached writer can overwrite a newer edit after shutdown.
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
