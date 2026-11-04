use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use uuid::Uuid;
use warp_terminal::shell::{ShellLaunchData, ShellType};

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) enum Host {
    #[default]
    Local,
    Wsl(String),
}

impl Host {
    pub(crate) fn same_path(&self, a: &str, b: &str) -> bool {
        if cfg!(windows) && *self == Self::Local {
            a.replace('/', "\\")
                .trim_end_matches('\\')
                .eq_ignore_ascii_case(b.replace('/', "\\").trim_end_matches('\\'))
        } else {
            a.trim_end_matches('/') == b.trim_end_matches('/')
        }
    }

    pub(crate) fn native_path(&self, path: &str) -> Option<PathBuf> {
        match self {
            Self::Local => Some(PathBuf::from(path)),
            Self::Wsl(distro) => ShellLaunchData::WSL {
                distro: distro.clone(),
            }
            .maybe_convert_absolute_path(path),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) enum Provider {
    Codex,
    Claude,
    Pi,
    Hermes,
    Custom,
}

impl Provider {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::Claude => "Claude Code",
            Self::Pi => "Pi",
            Self::Hermes => "Hermes",
            Self::Custom => "CLI",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct Identity {
    pub(crate) id: String,
    pub(crate) transcript: Option<String>,
}

impl Identity {
    pub(crate) fn same_session(&self, other: &Self, provider: Provider, host: &Host) -> bool {
        self.id == other.id
            || (provider == Provider::Pi
                && self
                    .transcript
                    .as_ref()
                    .zip(other.transcript.as_ref())
                    .is_some_and(|(a, b)| host.same_path(a, b)))
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct SavedShell {
    pub(crate) program: String,
    pub(crate) kind: ShellType,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) enum Activation {
    #[default]
    None,
    Venv(String),
    Conda {
        environment: String,
        executable: Option<String>,
    },
}

impl Activation {
    pub(crate) fn from_environment(env: &BTreeMap<String, String>) -> Self {
        if let Some(prefix) = env.get("CONDA_PREFIX") {
            Self::Conda {
                environment: prefix.clone(),
                executable: env.get("CONDA_EXE").cloned(),
            }
        } else if let Some(venv) = env.get("VIRTUAL_ENV") {
            Self::Venv(venv.clone())
        } else {
            Self::None
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Session {
    pub(crate) id: Uuid,
    pub(crate) title: String,
    pub(crate) provider: Provider,
    pub(crate) shell: SavedShell,
    pub(crate) environment: BTreeMap<String, String>,
    pub(crate) activation: Activation,
    pub(crate) command: String,
    pub(crate) identity: Option<Identity>,
    pub(crate) resume_command: Option<String>,
    pub(crate) pinned: bool,
    pub(crate) last_used: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Project {
    pub(crate) id: Uuid,
    pub(crate) host: Host,
    pub(crate) directory: String,
    pub(crate) pinned: bool,
    pub(crate) last_used: i64,
    pub(crate) manual_order: bool,
    pub(crate) sessions: Vec<Session>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct Catalog {
    pub(crate) manual_order: bool,
    pub(crate) projects: Vec<Project>,
}

pub(crate) struct Edit {
    pub(crate) host: Host,
    pub(crate) directory: String,
    pub(crate) session: Session,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum Entry {
    Project(Uuid),
    Session { project: Uuid, session: Uuid },
}

impl Entry {
    pub(crate) fn id(self) -> Uuid {
        match self {
            Self::Project(id) => id,
            Self::Session { session, .. } => session,
        }
    }
}

impl Catalog {
    pub(crate) fn session(&self, id: Uuid) -> Option<&Session> {
        self.projects
            .iter()
            .flat_map(|project| &project.sessions)
            .find(|session| session.id == id)
    }
}
