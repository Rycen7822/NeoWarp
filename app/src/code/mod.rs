use std::any::Any;
use std::fmt::{Debug, Display};

use serde::{Deserialize, Serialize};
use warp_errors::{ErrorExt, register_error};
use warp_util::file::FileSaveError;
use warpui::AppContext;
use warpui::elements::DropTargetData;

use crate::terminal::shell::ShellType;

pub mod buffer_location;
pub mod code_block;
pub mod editor;
pub mod editor_management;
pub mod global_buffer_model;
pub mod local_code_editor;

#[derive(Debug, thiserror::Error)]
pub enum ImmediateSaveError {
    #[error("No FileId")]
    NoFileId,
    #[error("failed to save file: {0:#}")]
    FailedToSave(#[from] FileSaveError),
    #[error("There is no file tab currently selected")]
    NoActiveFileTab,
    #[error("Remote session disconnected")]
    RemoteDisconnected,
}

impl ErrorExt for ImmediateSaveError {
    fn is_actionable(&self) -> bool {
        match self {
            ImmediateSaveError::NoFileId | ImmediateSaveError::NoActiveFileTab => true,
            ImmediateSaveError::FailedToSave(err) => err.is_actionable(),
            ImmediateSaveError::RemoteDisconnected => false,
        }
    }
}
register_error!(ImmediateSaveError);
#[derive(Debug)]
pub enum SaveStatus {
    /// Save completed immediately and successfully.
    SavedImmediately,
    /// Save operation is in progress asynchronously (e.g., save-as dialog).
    AsyncSaveInProgress,
    /// Save failed with an error.
    Failed(#[allow(unused)] ImmediateSaveError),
}

#[derive(Debug, Eq, PartialEq)]
pub enum SaveOutcome {
    Canceled,
    Failed,
    Succeeded,
}

mod icon;

pub mod opened_files;
pub(crate) mod persisted_workspace;
pub use icon::icon_from_file_path;

#[path = "view.rs"]
pub mod view;

pub fn init(app: &mut AppContext) {
    self::view::init(app);
}

#[derive(Debug)]
pub struct EditorTabBarDropTargetData {
    index: usize,
}

impl DropTargetData for EditorTabBarDropTargetData {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[allow(unused)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProgrammingLanguage {
    Shell(ShellType),
    Other(String),
}

impl ProgrammingLanguage {
    pub fn display_name(&self) -> String {
        match self {
            Self::Shell(shell_type) => shell_type.name().to_owned(),
            Self::Other(language) => language.to_lowercase(),
        }
    }

    /// Return whether this language is a shell language.
    /// This is used to determine whether to show the "execute in terminal" button.
    pub fn is_shell(&self) -> bool {
        matches!(self, Self::Shell(_))
    }
}

impl Display for ProgrammingLanguage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProgrammingLanguage::Shell(shell_type) => write!(f, "{}", shell_type.name()),
            ProgrammingLanguage::Other(language) => write!(f, "{}", language.to_lowercase()),
        }
    }
}

impl From<String> for ProgrammingLanguage {
    // Returns a programming language for a markdown language specifier
    fn from(value: String) -> Self {
        if let Some(shell_type) = ShellType::from_markdown_language_spec(value.as_str()) {
            ProgrammingLanguage::Shell(shell_type)
        } else {
            ProgrammingLanguage::Other(value)
        }
    }
}
