//! Local launch records for external CLI sessions. No conversations or credentials are imported.

pub(crate) mod command;
pub(crate) mod config;
pub(crate) mod editor;
pub(crate) mod launch;
mod provider;
pub(crate) mod view;

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
use self::config::{Activation, Catalog, Entry, Host, Identity, Project, SavedShell, Session};
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
    pub(crate) fn project(&self, id: Uuid) -> Option<&Project> {
        self.catalog
            .projects
            .iter()
            .find(|project| project.id == id)
    }

    pub(crate) fn entry(&self, id: Uuid) -> Option<(&Project, &Session)> {
        self.catalog.projects.iter().find_map(|project| {
            project
                .sessions
                .iter()
                .find(|session| session.id == id)
                .map(|session| (project, session))
        })
    }

    pub(crate) fn edit(
        &mut self,
        edit: config::Edit,
        ctx: &mut ModelContext<Self>,
    ) -> Result<Uuid> {
        anyhow::ensure!(self.ready, "Project sessions are not available yet");
        anyhow::ensure!(
            !self
                .live
                .values()
                .any(|live| live.session_id == edit.session.id
                    && live.running.is_none()
                    && live.terminal.upgrade(ctx).is_some()),
            "Wait for the session terminal to finish starting before modifying it"
        );
        let path = edit
            .host
            .native_path(&edit.directory)
            .ok_or_else(|| anyhow::anyhow!("Invalid directory"))?;
        anyhow::ensure!(
            path.is_absolute() && path.is_dir(),
            "Directory is unavailable: {}",
            edit.directory
        );
        anyhow::ensure!(
            !edit.session.title.trim().is_empty() && !edit.session.command.trim().is_empty(),
            "Name and startup command are required"
        );
        for key in edit.session.environment.keys() {
            anyhow::ensure!(command::variable_name(key), "Invalid variable name: {key}");
        }
        if let Some((_, previous)) = self.entry(edit.session.id)
            && previous.provider != edit.session.provider
        {
            anyhow::ensure!(
                self.active_terminal(edit.session.id, ctx).is_none(),
                "Create another session to change CLI type while it is running"
            );
            anyhow::ensure!(
                edit.session.identity.is_none(),
                "Clear the old CLI identity when changing CLI type"
            );
        }
        let slot = self
            .entry(edit.session.id)
            .filter(|(project, _)| {
                project.host == edit.host
                    && edit.host.same_path(&project.directory, &edit.directory)
            })
            .and_then(|(project, _)| {
                project
                    .sessions
                    .iter()
                    .position(|session| session.id == edit.session.id)
            });
        launch::resolve_shell(&edit.host, &edit.session.shell, ctx)?;
        let mut next = self.catalog.clone();
        for project in &mut next.projects {
            project
                .sessions
                .retain(|session| session.id != edit.session.id);
        }
        let project = match next.projects.iter().position(|project| {
            project.host == edit.host && edit.host.same_path(&project.directory, &edit.directory)
        }) {
            Some(index) => &mut next.projects[index],
            None => {
                next.projects.insert(
                    0,
                    Project {
                        id: Uuid::new_v4(),
                        host: edit.host,
                        directory: edit.directory,
                        pinned: false,
                        last_used: edit.session.last_used,
                        manual_order: false,
                        sessions: Vec::new(),
                    },
                );
                &mut next.projects[0]
            }
        };
        let id = edit.session.id;
        project.last_used = project.last_used.max(edit.session.last_used);
        project
            .sessions
            .insert(slot.unwrap_or(0).min(project.sessions.len()), edit.session);
        next.projects
            .retain(|project| project.pinned || !project.sessions.is_empty());
        self.replace_catalog(next, ctx)?;
        Ok(id)
    }

    pub(crate) fn toggle_pin(&mut self, entry: Entry, ctx: &mut ModelContext<Self>) -> Result<()> {
        let mut next = self.catalog.clone();
        match entry {
            Entry::Project(id) => {
                let project = next
                    .projects
                    .iter_mut()
                    .find(|project| project.id == id)
                    .ok_or_else(|| anyhow::anyhow!("Project no longer exists"))?;
                project.pinned = !project.pinned;
            }
            Entry::Session { project, session } => {
                let session = next
                    .projects
                    .iter_mut()
                    .find(|candidate| candidate.id == project)
                    .and_then(|project| {
                        project
                            .sessions
                            .iter_mut()
                            .find(|candidate| candidate.id == session)
                    })
                    .ok_or_else(|| anyhow::anyhow!("Session no longer exists"))?;
                session.pinned = !session.pinned;
            }
        }
        self.replace_catalog(next, ctx)
    }

    pub(crate) fn remove(&mut self, entry: Entry, ctx: &mut ModelContext<Self>) -> Result<()> {
        let mut next = self.catalog.clone();
        match entry {
            Entry::Project(id) => {
                let index = next
                    .projects
                    .iter()
                    .position(|project| project.id == id)
                    .ok_or_else(|| anyhow::anyhow!("Project no longer exists"))?;
                next.projects.remove(index);
            }
            Entry::Session { project, session } => {
                let project = next
                    .projects
                    .iter_mut()
                    .find(|candidate| candidate.id == project)
                    .ok_or_else(|| anyhow::anyhow!("Project no longer exists"))?;
                let index = project
                    .sessions
                    .iter()
                    .position(|candidate| candidate.id == session)
                    .ok_or_else(|| anyhow::anyhow!("Session no longer exists"))?;
                project.sessions.remove(index);
            }
        }
        next.projects
            .retain(|project| project.pinned || !project.sessions.is_empty());
        anyhow::ensure!(
            !self.live.values().any(|live| live.running.is_none()
                && self.catalog.session(live.session_id).is_some()
                && next.session(live.session_id).is_none()
                && live.terminal.upgrade(ctx).is_some()),
            "Wait for the session terminal to finish starting before modifying it"
        );
        self.replace_catalog(next, ctx)?;
        // Keep live blocks so repeated observations cannot recreate deleted records.
        for live in self.live.values_mut() {
            if self.catalog.session(live.session_id).is_none()
                && let Some(lookup) = live
                    .running
                    .as_mut()
                    .and_then(|running| running.lookup.take())
            {
                lookup.abort();
            }
        }
        Ok(())
    }

    pub(crate) fn reorder(
        &mut self,
        source: Entry,
        target: Entry,
        after: bool,
        ctx: &mut ModelContext<Self>,
    ) -> Result<()> {
        if source == target {
            return Ok(());
        }
        let mut next = self.catalog.clone();
        match (source, target) {
            (Entry::Project(source), Entry::Project(target)) => {
                if !next.manual_order {
                    next.projects
                        .sort_by_key(|project| std::cmp::Reverse(project.last_used));
                }
                move_entry(&mut next.projects, source, target, after, |project| {
                    (project.id, project.pinned)
                })?;
                next.manual_order = true;
            }
            (
                Entry::Session {
                    project,
                    session: source,
                },
                Entry::Session {
                    project: target_project,
                    session: target,
                },
            ) if project == target_project => {
                let project = next
                    .projects
                    .iter_mut()
                    .find(|candidate| candidate.id == project)
                    .ok_or_else(|| anyhow::anyhow!("Project no longer exists"))?;
                if !project.manual_order {
                    project
                        .sessions
                        .sort_by_key(|session| std::cmp::Reverse(session.last_used));
                }
                move_entry(&mut project.sessions, source, target, after, |session| {
                    (session.id, session.pinned)
                })?;
                project.manual_order = true;
            }
            _ => anyhow::bail!("Reorder within the same project and level"),
        }
        self.replace_catalog(next, ctx)
    }

    fn replace_catalog(&mut self, catalog: Catalog, ctx: &mut ModelContext<Self>) -> Result<()> {
        anyhow::ensure!(self.ready, "Project sessions are not available yet");
        write(&self.path, &catalog)?;
        self.catalog = catalog;
        self.error = None;
        ctx.notify();
        Ok(())
    }

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

    pub(crate) fn provider_for_terminal(&self, terminal: EntityId) -> Option<config::Provider> {
        self.live
            .get(&terminal)
            .and_then(|live| self.entry(live.session_id))
            .map(|(_, session)| session.provider)
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

    pub(crate) fn running_directory(&self, terminal: EntityId) -> Option<(&Host, &str)> {
        self.live
            .get(&terminal)
            .and_then(|live| live.running.as_ref())
            .map(|running| (&running.query.host, running.query.directory.as_str()))
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
        log::debug!(target: "project_sessions", "projects event=begin_launch record={id} terminal={:?}", terminal.id());
        self.live.retain(|terminal, live| {
            let keep = live.terminal.upgrade(ctx).is_some();
            if !keep {
                log::debug!(target: "project_sessions", "projects event=prune_live record={} terminal={terminal:?}", live.session_id);
            }
            keep
        });
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
        log::debug!(target: "project_sessions",
            "projects event=observe terminal={terminal_id:?} block={} record={:?} bound_block={:?} launched={} parsed={}",
            observation.block, self.live.get(&terminal_id).map(|live| live.session_id),
            self.live.get(&terminal_id).and_then(|live| live.running.as_ref()).map(|running| &running.block),
            observation.launched, observation.parsed.is_some());
        // The notifying terminal is temporarily taken out of the UI table during its update.
        self.live.retain(|id, live| {
            let keep = *id == terminal_id || live.terminal.upgrade(ctx).is_some();
            if !keep {
                log::debug!(target: "project_sessions", "projects event=prune_live record={} terminal={id:?}", live.session_id);
            }
            keep
        });
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
                resuming: command::parse(&session.command, session.shell.kind)
                    .is_some_and(|parsed| parsed.resuming),
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
            resuming: parsed.resuming,
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
                self.catalog.projects.insert(
                    0,
                    Project {
                        id: Uuid::new_v4(),
                        host,
                        directory,
                        pinned: false,
                        last_used: started_at,
                        manual_order: false,
                        sessions: Vec::new(),
                    },
                );
                0
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
        log::debug!(target: "project_sessions",
            "projects event=catalog_observe project={} record={id} terminal={terminal_id:?} block={block} inserted={} count={}",
            project.id, existing.is_none(), project.sessions.len());
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
        log::debug!(target: "project_sessions",
            "projects event=register_running record={id} terminal={terminal_id:?} block={block} unresolved={unresolved}");
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
        let provider = query.provider;
        let block = running.block.clone();
        log::debug!(target: "project_sessions",
            "projects event=lookup_scheduled record={} terminal={terminal_id:?} block={block}", self.live[&terminal_id].session_id);
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
                log::debug!(target: "project_sessions",
                    "projects event=lookup_result record={id} terminal={terminal_id:?} block={block} active={active} candidates={:?}",
                    result.as_ref().ok().map(Vec::len));
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
                        me.bind_identity(id, identity, provider, ctx);
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
        log::debug!(target: "project_sessions",
            "projects event=end_request record={} terminal={terminal:?} block={block:?} bound_block={:?}",
            live.session_id, live.running.as_ref().map(|running| &running.block));
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
        log::debug!(target: "project_sessions", "projects event=end_live record={} terminal={terminal:?}", live.session_id);
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
            let provider = query.provider;
            ctx.spawn(
                async move { provider::candidates(&query) },
                move |me, result, ctx| {
                    if let Ok(candidates) = result {
                        let mut candidates = me.unbound_candidates(id, candidates);
                        if candidates.len() == 1 {
                            me.bind_identity(id, candidates.remove(0), provider, ctx);
                        }
                    }
                },
            );
        }
        ctx.notify();
    }

    fn bind_identity(
        &mut self,
        id: Uuid,
        identity: Identity,
        provider: config::Provider,
        ctx: &mut ModelContext<Self>,
    ) {
        let Some(project) = self
            .catalog
            .projects
            .iter_mut()
            .find(|project| project.sessions.iter().any(|session| session.id == id))
        else {
            return;
        };
        let Some(index) = project.sessions.iter().position(|session| {
            session.id == id && session.identity.is_none() && session.provider == provider
        }) else {
            return;
        };
        let existing_id = project
            .sessions
            .iter()
            .find(|session| {
                session.id != id
                    && session.provider == provider
                    && session
                        .identity
                        .as_ref()
                        .is_some_and(|saved| saved.same_session(&identity, provider, &project.host))
            })
            .map(|session| session.id);
        project.sessions[index].identity = Some(identity);
        if let Some(existing_id) = existing_id {
            let mut session = project.sessions.remove(index);
            let previous = project
                .sessions
                .iter_mut()
                .find(|session| session.id == existing_id)
                .expect("existing session was found before removing the new entry");
            session.id = previous.id;
            session.title = previous.title.clone();
            session.pinned = previous.pinned;
            session.resume_command = previous.resume_command.take();
            *previous = session;
            for live in self.live.values_mut().filter(|live| live.session_id == id) {
                live.session_id = existing_id;
            }
        }
        log::debug!(target: "project_sessions",
            "projects event=identity_bound project={} record={id} merged_into={existing_id:?} count={}",
            project.id, project.sessions.len());
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
                        && self.live.values().any(|live| live.session_id == session.id)
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

fn move_entry<T>(
    items: &mut Vec<T>,
    source: Uuid,
    target: Uuid,
    after: bool,
    key: impl Fn(&T) -> (Uuid, bool),
) -> Result<()> {
    let from = items
        .iter()
        .position(|item| key(item).0 == source)
        .ok_or_else(|| anyhow::anyhow!("Dragged entry no longer exists"))?;
    let to = items
        .iter()
        .position(|item| key(item).0 == target)
        .ok_or_else(|| anyhow::anyhow!("Drop target no longer exists"))?;
    anyhow::ensure!(
        key(&items[from]).1 == key(&items[to]).1,
        "Reorder within the same pin group"
    );
    let item = items.remove(from);
    let insertion = to - usize::from(from < to) + usize::from(after);
    items.insert(insertion, item);
    Ok(())
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
