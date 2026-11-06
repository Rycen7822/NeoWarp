use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use warp_util::path::LineAndColumnArg;
use warpui::{Entity, EntityId, SingletonEntity, WindowId};

use super::buffer_location::LocalOrRemotePath;
use crate::pane_group::PaneId;
use crate::workspace::PaneViewLocator;

#[derive(Debug, Hash, Eq, PartialEq, Clone, Serialize, Deserialize)]
pub enum CodeSource {
    /// A new code pane not attached to an existing file.
    New {
        /// When the new file is saved, open the file picker to this directory.
        default_directory: Option<PathBuf>,
    },
    /// Opened from file links.
    Link {
        path: PathBuf,
        range_start: Option<LineAndColumnArg>,
        range_end: Option<LineAndColumnArg>,
    },
    /// Opened from command palette file search (local or remote).
    CommandPalette { location: LocalOrRemotePath },
    /// Opened from macOS Finder via "Open With".
    Finder { path: PathBuf },
}

impl CodeSource {
    pub fn default_directory(&self) -> Option<&PathBuf> {
        match self {
            Self::New {
                default_directory, ..
            } => default_directory.as_ref(),
            Self::Link { .. } | Self::CommandPalette { .. } | Self::Finder { .. } => None,
        }
    }

    /// Returns the `LocalOrRemotePath` for command palette sources.
    pub fn file_location(&self) -> Option<&LocalOrRemotePath> {
        match self {
            Self::CommandPalette { location } => Some(location),
            _ => None,
        }
    }

    /// Returns the `LocalOrRemotePath` for any source that has a backing file.
    ///
    /// Covers every variant that maps to a file — local or remote.
    pub fn location(&self) -> Option<LocalOrRemotePath> {
        match self {
            Self::New { .. } => None,
            Self::CommandPalette { location } => Some(location.clone()),
            Self::Link { path, .. } | Self::Finder { path } => {
                Some(LocalOrRemotePath::Local(path.clone()))
            }
        }
    }

    pub fn omit_line_col(&self) -> CodeSource {
        if let CodeSource::Link { path, .. } = self {
            CodeSource::Link {
                path: path.clone(),
                range_start: None,
                range_end: None,
            }
        } else {
            self.clone()
        }
    }

    /// Returns `true` if this source should be restored across app restarts.
    pub fn is_restorable(&self) -> bool {
        !matches!(
            self,
            Self::CommandPalette {
                location: LocalOrRemotePath::Remote(_),
            }
        )
    }
}

struct CodePaneData {
    #[allow(unused)]
    window_id: WindowId,
    #[allow(unused)]
    locator: PaneViewLocator,
}

/// Singleton model for managing the state of open code panes. It is responsible for
/// Allows callers to find an existing open code pane.
#[derive(Default)]
pub struct CodeManager {
    source_to_pane_data: HashMap<CodeSource, CodePaneData>,
}

impl CodeManager {
    /// Register a new pane in the code manager.
    pub fn register_pane(
        &mut self,
        pane_group_id: EntityId,
        window_id: WindowId,
        pane_id: PaneId,
        source: CodeSource,
    ) {
        let entry = self.source_to_pane_data.entry(source.omit_line_col());
        if let Entry::Vacant(entry) = entry {
            entry.insert(CodePaneData {
                window_id,
                locator: PaneViewLocator {
                    pane_group_id,
                    pane_id,
                },
            });
        } else {
            log::warn!("Ignoring duplicate code pane registration");
        }
    }

    /// De-register an open code pane when it's removed from a pane group.
    pub fn deregister_pane(&mut self, source: &CodeSource) {
        self.source_to_pane_data.remove(&source.omit_line_col());
    }

    /// Returns the locator for a code pane that already has the given `LocalOrRemotePath`
    /// open in the given pane group. Works for both local and remote files.
    pub fn get_locator_for_location_in_tab(
        &self,
        pane_group_id: EntityId,
        location: &LocalOrRemotePath,
    ) -> Option<PaneViewLocator> {
        self.source_to_pane_data
            .iter()
            .find(|(source, data)| {
                data.locator.pane_group_id == pane_group_id
                    && source.location().as_ref() == Some(location)
            })
            .map(|(_, data)| data.locator)
    }
}

impl Entity for CodeManager {
    type Event = ();
}

impl SingletonEntity for CodeManager {}
