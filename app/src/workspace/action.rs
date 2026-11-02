use std::path::PathBuf;

use ui_components::lightbox;
use warpui::EntityId;
use warpui::accessibility::AccessibilityVerbosity;
use warpui::geometry::rect::RectF;
use warpui::geometry::vector::Vector2F;
use warpui::platform::Cursor;
use warpui::platform::keyboard::KeyCode;

use super::tab_settings::{
    VerticalTabsCompactSubtitle, VerticalTabsDisplayGranularity, VerticalTabsPrimaryInfo,
    VerticalTabsTabItemMode, VerticalTabsViewMode,
};
use super::view::WorkspaceBanner;
use crate::palette::{PaletteMode, PaletteSource};
use crate::search;
use crate::settings_view::{SettingsAction as SettingsTabAction, SettingsSection};
use crate::tab::{NewSessionMenuItem, SelectedTabColor};
use crate::tab_configs::TabConfig;
use crate::terminal::available_shells::AvailableShell;
use crate::themes::theme::AnsiColorIdentifier;
use crate::themes::theme_chooser::ThemeChooserMode;
use crate::workspace::PaneViewLocator;
use crate::workspace::tab_group::TabGroupId;

/// This enum determines how the search query is initialized when opening command search.
#[derive(Clone, Default, Debug)]
pub enum InitContent {
    /// Read the content of the active terminal input, and make that the initial search query.
    #[default]
    FromInputBuffer,
    /// Specify an exact string to initialize the query to.
    Custom(String),
}

/// To initialize command search, we may want to specify a search filter, or the content of the
/// query itself.
#[derive(Clone, Default, Debug)]
pub struct CommandSearchOptions {
    pub filter: Option<search::QueryFilter>,
    pub init_content: InitContent,
}

#[derive(Debug, Clone, Copy)]
pub enum TabContextMenuAnchor {
    Pointer(Vector2F),
    VerticalTabsKebab,
}

/// Describes how the new-session dropdown menu was opened so the renderer
/// can pick the right anchor strategy.
#[derive(Debug, Clone, Copy)]
pub enum NewSessionMenuAnchor {
    /// Menu was opened from the `+` add-tab button. When vertical tabs are
    /// active, the renderer anchors below the button's save position;
    /// otherwise the contained position is used directly.
    AddTabButton(Vector2F),
    /// Menu was opened by right-clicking the vertical tabs panel.
    /// Always anchored at the contained pointer position.
    Pointer(Vector2F),
}

impl NewSessionMenuAnchor {
    pub fn position(&self) -> Vector2F {
        match self {
            Self::AddTabButton(position) | Self::Pointer(position) => *position,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum VerticalTabsPaneContextMenuTarget {
    ClickedPane(PaneViewLocator),
    ActivePane(PaneViewLocator),
}

impl VerticalTabsPaneContextMenuTarget {
    pub fn locator(self) -> PaneViewLocator {
        match self {
            Self::ClickedPane(locator) | Self::ActivePane(locator) => locator,
        }
    }
}

#[derive(Debug, Clone)]
pub enum WorkspaceAction {
    OpenGitReview(Option<crate::code::buffer_location::LocalOrRemotePath>),
    CloseGitReview,
    ActivateTab(usize),
    ActivatePrevTab,
    ActivateNextTab,
    ActivateLastTab,
    CyclePrevSession,
    CycleNextSession,
    MoveActiveTabLeft,
    MoveActiveTabRight,
    MoveTabLeft(usize),
    MoveTabRight(usize),
    RenameTab(usize),
    ResetTabName(usize),
    RenamePane(PaneViewLocator),
    ResetPaneName(PaneViewLocator),
    RenameActiveTab,
    RenameActivePane,
    SetActiveTabName(String),
    CycleActiveTabColor,
    SetActiveTabColor(SelectedTabColor),
    ToggleTabRightClickMenu {
        tab_index: usize,
        anchor: TabContextMenuAnchor,
    },
    ToggleTabSelectionRightClickMenu {
        tab_index: usize,
        anchor: TabContextMenuAnchor,
    },
    ToggleVerticalTabsPaneContextMenu {
        tab_index: usize,
        target: VerticalTabsPaneContextMenuTarget,
        position: Vector2F,
    },
    TabHoverWidthStart {
        width: f32,
    },
    TabHoverWidthEnd,
    ToggleWelcomeTips,
    CloseTab(usize),
    CloseActiveTab,
    CloseOtherTabs(usize),
    CloseNonActiveTabs,
    CloseTabsRight(usize),
    CloseTabsRightActiveTab,
    /// Close every tab that belongs to the given tab group.
    CloseTabGroup(TabGroupId),
    /// Toggle collapsed state for the given tab group.
    ToggleTabGroupCollapsed(TabGroupId),
    /// Opens an inline editor over the given group's header for renaming.
    RenameTabGroup(TabGroupId),
    CancelActiveRename,
    /// Creates a new tab group containing the tab at the given index.
    NewTabGroupFromTab(usize),
    /// Moves the tab at `tab_index` into `group_id`, appending it to the
    /// end of the group's contiguous run.
    MoveTabToGroup {
        tab_index: usize,
        group_id: TabGroupId,
    },
    /// Removes the tab at the given index from its current group.
    RemoveTabFromGroup(usize),
    /// Selects every tab between the active tab and the shift-clicked row (inclusive).
    ShiftSelectTabRange {
        locator: PaneViewLocator,
    },
    /// Toggles whether the tab at `locator` is part of the active multi-selection.
    /// Dispatched on cmd-click of a vertical tab row.
    ToggleTabMultiSelection {
        locator: PaneViewLocator,
    },
    /// Clears the tab multi-selection.
    ClearTabMultiSelection,
    /// Creates a new tab group from the current tab multi-selection.
    NewTabGroupFromSelectedTabs,
    /// Context-aware "create group" entry point for the keybinding: groups
    /// the multi-selection when 2+ tabs are selected, otherwise groups the
    /// active tab.
    NewTabGroupFromActiveOrSelectedTabs,
    /// Moves every selected tab into `group_id`.
    MoveSelectedTabsToGroup {
        group_id: TabGroupId,
    },
    /// Removes every selected tab from its group (requires a single shared group).
    RemoveSelectedTabsFromGroup,
    /// Context-aware "remove from group" entry point for the keybinding:
    /// removes the multi-selection from its shared group when 2+ tabs are
    /// selected, otherwise removes the active tab.
    RemoveActiveOrSelectedTabsFromGroup,
    ToggleTabGroupRightClickMenu {
        group_id: TabGroupId,
        anchor: TabContextMenuAnchor,
    },
    UngroupTabs(TabGroupId),
    NewTabInGroup(TabGroupId),
    MoveTabGroupUp(TabGroupId),
    MoveTabGroupDown(TabGroupId),
    CloseTabsOutsideGroup(TabGroupId),
    CloseTabsAboveGroup(TabGroupId),
    CloseTabsBelowGroup(TabGroupId),
    /// Pins the tab at the given index. If the tab is part of a group, it
    /// is first extracted from the group and then pinned as ungrouped.
    PinTab(usize),
    /// Unpins the tab at the given index.
    UnpinTab(usize),
    /// Pins the active tab.
    PinActiveTab,
    /// Unpins the active tab.
    UnpinActiveTab,
    /// Pins the entire tab group: sets the group as pinned
    /// and moves the group block to the end of the pinned region.
    PinTabGroup(TabGroupId),
    /// Unpins the entire tab group: clears the pinned flag on the group
    /// and moves the group block to the start of the unpinned region.
    UnpinTabGroup(TabGroupId),
    /// Pins the active tab's group.
    PinActiveTabGroup,
    /// Unpins the active tab's group.
    UnpinActiveTabGroup,
    AddDefaultTab,
    AddTerminalTab {
        hide_homepage: bool,
    },
    AddTabWithShell {
        shell: AvailableShell,
    },
    OpenNewSessionMenu {
        anchor: NewSessionMenuAnchor,
    },
    ToggleTabConfigsMenu,
    ToggleNewSessionMenu {
        anchor: NewSessionMenuAnchor,
    },
    SelectNewSessionMenuItem(NewSessionMenuItem),
    CopyVersion(&'static str),
    ConfigureKeybindingSettings {
        keybinding_name: Option<String>,
    },
    ShowSettings,
    ShowSettingsPage(SettingsSection),
    ShowSettingsPageWithSearch {
        search_query: String,
        section: Option<SettingsSection>,
    },
    ShowThemeChooser(ThemeChooserMode),
    ShowThemeChooserForActiveTheme,
    IncreaseFontSize,
    DecreaseFontSize,
    ResetFontSize,
    IncreaseZoom,
    DecreaseZoom,
    ResetZoom,
    ActivateTabByNumber(usize),
    SetTabShortcutModifierKey {
        key_code: KeyCode,
        pressed: bool,
    },
    OpenPalette {
        mode: PaletteMode,
        source: PaletteSource,
        query: Option<String>,
    },
    TogglePalette {
        mode: PaletteMode,
        source: PaletteSource,
    },
    /// Open the log directory in the system file explorer with the current log file selected.
    ViewLogs,
    ChangeCursor(Cursor),
    ToggleBlockSnackbar,
    ToggleErrorUnderlining,
    ToggleSyntaxHighlighting,
    SetA11yVerbosityLevel(AccessibilityVerbosity),
    ToggleNotifications,
    ToggleTabColor {
        color: AnsiColorIdentifier,
        tab_index: usize,
    },
    ToggleTabGroupColor {
        color: AnsiColorIdentifier,
        group_id: TabGroupId,
    },
    OpenLaunchConfigSaveModal,
    SelectTabConfig(TabConfig),
    DispatchToSettingsTab(SettingsTabAction),
    ToggleResourceCenter,
    ToggleKeybindingsPage,
    ShowCommandSearch(CommandSearchOptions),
    TriggerExternalCtrlTFileSearch,
    TriggerExternalAltCDirectorySearch,
    ToggleMouseReporting,
    ToggleScrollReporting,
    ToggleFocusReporting,
    StartTabDrag,
    DragTab {
        tab_index: usize,
        tab_position: RectF,
    },
    DropTab,
    StartGroupDrag(TabGroupId),
    DragGroup {
        group_id: TabGroupId,
        /// The dragged group's painted rect.
        position: RectF,
        /// The position of the cursor while dragging a group.
        cursor_position: Vector2F,
    },
    DropGroup,
    /// Toggles the vertical tabs panel. This happens as an explicit action from the user.
    ToggleVerticalTabsPanel,
    ToggleFileBrowser,
    OpenVerticalTabsPanel,
    ToggleVerticalTabsSettingsPopup,
    SetVerticalTabsDisplayGranularity(VerticalTabsDisplayGranularity),
    SetVerticalTabsTabItemMode(VerticalTabsTabItemMode),
    SetVerticalTabsViewMode(VerticalTabsViewMode),
    SetVerticalTabsPrimaryInfo(VerticalTabsPrimaryInfo),
    SetVerticalTabsCompactSubtitle(VerticalTabsCompactSubtitle),
    ToggleVerticalTabsShowPrLink,
    ToggleVerticalTabsShowDiffStats,
    ToggleVerticalTabsShowDetailsOnHover,
    CopyTextToClipboard(String),
    /// Copies the focused code pane's file path or the focused terminal's working directory.
    /// No-op if neither yields a path.
    CopyCurrentPath,
    DismissWorkspaceBanner(WorkspaceBanner),
    /// An action only registered in dev and local builds, which triggers a
    /// panic immediately when called.
    Panic,
    /// Writes a heap profile to disk.
    DumpHeapProfile,
    /// An action to open a new window with a view hierarchy debugger.
    OpenViewTreeDebugWindow,
    /// An action to either upgrade syncing status from none or just in one tab
    /// to syncing all tabs, or downgrade from syncing all tabs to no syncing
    ToggleSyncAllTerminalInputsInAllTabs,
    /// An action to either cancel syncing
    /// or switch from no syncing/syncing all tabs to syncing within one tab
    ToggleSyncTerminalInputsInTab,
    /// An action to force terminal input syncing off
    DisableTerminalInputSync,
    OpenPromptEditor,
    OpenHeaderToolbarEditor,
    ShowHeaderToolbarContextMenu {
        position: Vector2F,
    },
    OpenLink(String),
    ReopenClosedSession,
    AddWindow,
    AddWindowWithShell {
        shell: AvailableShell,
    },
    /// Moves focus to the panel on the left
    FocusLeftPanel,
    /// Moves focus to the panel on the right
    FocusRightPanel,
    /// Open a local path in the file explorer.
    OpenInExplorer {
        path: PathBuf,
    },
    /// Open a local file with the system's default application.
    OpenFilePath {
        path: PathBuf,
    },
    TerminateApp,
    CloseWindow,
    /// Help the user call the Warp executable with the [`crate::args::DEBUG_DUMP_FLAG`].
    DumpDebugInfo,
    ToggleRecordingMode,
    ToggleInBandGenerators,
    ToggleDebugNetworkStatus,
    ToggleShowMemoryStats,
    RunCommand(String),
    #[cfg(feature = "local_fs")]
    OpenProjectSession(uuid::Uuid),
    #[cfg(feature = "local_fs")]
    StartNewProjectSession(uuid::Uuid),
    #[cfg(feature = "local_fs")]
    SelectSidebarTab(super::SidebarTab),
    #[cfg(feature = "local_fs")]
    CreateProjectSession(Option<uuid::Uuid>),
    #[cfg(feature = "local_fs")]
    EditProjectSession(uuid::Uuid),
    #[cfg(feature = "local_fs")]
    ToggleProjectPin(uuid::Uuid),
    #[cfg(feature = "local_fs")]
    ToggleProjectSessionPin(uuid::Uuid),
    #[cfg(feature = "local_fs")]
    ShowProjectSessionMenu {
        id: uuid::Uuid,
        position: Vector2F,
    },
    #[cfg(feature = "local_fs")]
    ShowProjectMenu {
        id: uuid::Uuid,
        position: Vector2F,
    },
    /// Acknowledges and dismisses the local Wayland crash recovery warning.
    #[cfg(target_os = "linux")]
    DismissWaylandCrashRecoveryBanner,
    FocusTerminalViewInWorkspace {
        terminal_view_id: EntityId,
    },
    /// Focus a specific pane by its locator (pane_group_id and pane_id).
    FocusPane(PaneViewLocator),
    ScrollToSettingsWidget {
        page: SettingsSection,
        widget_id: &'static str,
    },
    /// Open a repository directory, selecting a folder when the path is absent.
    OpenRepository {
        path: Option<String>,
    },
    /// Open the native folder picker for a repo param in the tab-config modal after the
    /// current interaction cycle finishes.
    OpenTabConfigRepoPicker {
        param_index: usize,
    },
    /// Open a new blank code file in the current tab
    NewCodeFile,
    NavigatePrevPaneOrPanel,
    NavigateNextPaneOrPanel,
    /// Take a process sample of the app (equivalent to Activity Monitor > Sample Process).
    #[cfg(target_os = "macos")]
    SampleProcess,
    /// Open a full-window lightbox displaying the given images.
    OpenLightbox {
        images: Vec<lightbox::LightboxImage>,
        /// The index of the image to display initially.
        initial_index: usize,
    },
    /// Update a single image in the currently open lightbox.
    UpdateLightboxImage {
        index: usize,
        image: lightbox::LightboxImage,
    },
    ShowSessionConfigModal,
    SaveCurrentTabAsNewConfig(usize),
    SyncTrafficLights,
    /// Opens a tab config file in the editor and dismisses the associated error toast.
    OpenTabConfigErrorFile {
        path: PathBuf,
        toast_object_id: String,
    },
    /// Sidecar action: set the hovered item as the Cmd+T default.
    TabConfigSidecarMakeDefault {
        mode: crate::terminal::general_settings::DefaultSessionMode,
        tab_config_path: Option<PathBuf>,
        shell: Option<AvailableShell>,
    },
    /// Sidecar action: open the tab config TOML in the user's editor.
    TabConfigSidecarEditConfig {
        path: PathBuf,
    },
    /// Sidecar action: show the remove confirmation dialog for a tab config.
    TabConfigSidecarRemoveConfig {
        name: String,
        path: PathBuf,
    },
    /// Opens the settings.toml file in a code editor pane.
    OpenSettingsFile,
}

impl WorkspaceAction {
    /// Matches what actions require the app state to be saved, and which don't. We match all
    /// actions directly, rather than using _, so we're forced to make a conscious decision for each
    /// of them, rather than following some default.
    pub fn should_save_app_state_on_action(&self) -> bool {
        use WorkspaceAction::*;
        match self {
            #[cfg(feature = "local_fs")]
            OpenProjectSession(_) | StartNewProjectSession(_) => true,
            #[cfg(feature = "local_fs")]
            SelectSidebarTab(_) => false,
            #[cfg(feature = "local_fs")]
            CreateProjectSession(_)
            | EditProjectSession(_)
            | ToggleProjectPin(_)
            | ToggleProjectSessionPin(_)
            | ShowProjectSessionMenu { .. }
            | ShowProjectMenu { .. } => false,
            ActivateTab(_)
            | ActivateTabByNumber(_)
            | SetTabShortcutModifierKey { .. }
            | ActivatePrevTab
            | ActivateNextTab
            | ActivateLastTab
            | CyclePrevSession
            | CycleNextSession
            | MoveActiveTabLeft
            | MoveActiveTabRight
            | MoveTabLeft(_)
            | MoveTabRight(_)
            | DropTab
            | DropGroup
            | RenameTab(_)
            | ResetTabName(_)
            | RenamePane(_)
            | ResetPaneName(_)
            | RenameActiveTab
            | RenameActivePane
            | SetActiveTabName(_)
            | CycleActiveTabColor
            | SetActiveTabColor(_)
            | CloseTab(_)
            | CloseActiveTab
            | CloseOtherTabs(_)
            | CloseNonActiveTabs
            | CloseTabsRight(_)
            | CloseTabsRightActiveTab
            | CloseTabGroup(_)
            | ToggleTabGroupCollapsed(_)
            | RenameTabGroup(_)
            | NewTabGroupFromTab(_)
            | MoveTabToGroup { .. }
            | RemoveTabFromGroup(_)
            | NewTabGroupFromSelectedTabs
            | NewTabGroupFromActiveOrSelectedTabs
            | MoveSelectedTabsToGroup { .. }
            | RemoveSelectedTabsFromGroup
            | RemoveActiveOrSelectedTabsFromGroup
            | UngroupTabs(_)
            | NewTabInGroup(_)
            | MoveTabGroupUp(_)
            | MoveTabGroupDown(_)
            | CloseTabsOutsideGroup(_)
            | CloseTabsAboveGroup(_)
            | CloseTabsBelowGroup(_)
            | PinTab(_)
            | UnpinTab(_)
            | PinActiveTab
            | UnpinActiveTab
            | PinTabGroup(_)
            | UnpinTabGroup(_)
            | PinActiveTabGroup
            | UnpinActiveTabGroup
            | ToggleTabColor { .. }
            | ToggleTabGroupColor { .. }
            | AddDefaultTab
            | AddTerminalTab { .. }
            | AddTabWithShell { .. }
            | AddWindow
            | AddWindowWithShell { .. }
            | CloseWindow
            | ScrollToSettingsWidget { .. }
            | NewCodeFile
            | OpenRepository { .. }
            | SelectTabConfig(_)
            | ToggleVerticalTabsPanel
            | OpenVerticalTabsPanel => true, // actions that actually change a state of the state of user's
            // workspace would most likely require a save, so that if the app gets
            // restarted, the user can continue working
            CopyVersion(_)
            | ConfigureKeybindingSettings { .. }
            | ShowSettings
            | ShowSettingsPage(_)
            | ShowSettingsPageWithSearch { .. }
            | ShowThemeChooser(_)
            | ShowThemeChooserForActiveTheme
            | IncreaseFontSize
            | DecreaseFontSize
            | ResetFontSize
            | IncreaseZoom
            | DecreaseZoom
            | ResetZoom
            | OpenPalette { .. }
            | TogglePalette { mode: _, source: _ }
            | ChangeCursor(_)
            | ToggleBlockSnackbar
            | ToggleErrorUnderlining
            | ToggleSyntaxHighlighting
            | OpenLaunchConfigSaveModal
            | ToggleTabRightClickMenu { .. }
            | ToggleTabSelectionRightClickMenu { .. }
            | ToggleTabGroupRightClickMenu { .. }
            | ToggleVerticalTabsPaneContextMenu { .. }
            | OpenNewSessionMenu { .. }
            | ToggleTabConfigsMenu
            | ToggleNewSessionMenu { .. }
            | SelectNewSessionMenuItem(_)
            | SetA11yVerbosityLevel(_)
            | ToggleNotifications
            | DispatchToSettingsTab { .. }
            | ToggleResourceCenter
            | ToggleFileBrowser
            | ToggleKeybindingsPage
            | ShowCommandSearch(_)
            | TriggerExternalCtrlTFileSearch
            | TriggerExternalAltCDirectorySearch
            | ToggleMouseReporting
            | ToggleScrollReporting
            | ToggleFocusReporting
            | OpenInExplorer { .. }
            | DragTab { .. }
            | StartTabDrag
            | DragGroup { .. }
            | StartGroupDrag(_)
            | ToggleVerticalTabsSettingsPopup
            | SetVerticalTabsDisplayGranularity(_)
            | SetVerticalTabsTabItemMode(_)
            | SetVerticalTabsViewMode(_)
            | SetVerticalTabsPrimaryInfo(_)
            | SetVerticalTabsCompactSubtitle(_)
            | ToggleVerticalTabsShowPrLink
            | ToggleVerticalTabsShowDiffStats
            | ToggleVerticalTabsShowDetailsOnHover
            | ToggleWelcomeTips
            | CopyTextToClipboard(_)
            | CopyCurrentPath
            | OpenTabConfigRepoPicker { .. }
            | Panic
            | DumpHeapProfile
            | OpenViewTreeDebugWindow
            | DismissWorkspaceBanner(..)
            | ToggleSyncAllTerminalInputsInAllTabs
            | ToggleSyncTerminalInputsInTab
            | DisableTerminalInputSync
            | OpenPromptEditor
            | OpenHeaderToolbarEditor
            | ShowHeaderToolbarContextMenu { .. }
            | OpenLink(_)
            | ReopenClosedSession
            | FocusLeftPanel
            | FocusRightPanel
            | OpenGitReview(_)
            | CloseGitReview
            | DumpDebugInfo
            | ToggleRecordingMode
            | ToggleInBandGenerators
            | ToggleDebugNetworkStatus
            | ToggleShowMemoryStats
            | RunCommand { .. }
            | OpenFilePath { .. }
            | TerminateApp
            | TabHoverWidthStart { .. }
            | TabHoverWidthEnd
            | FocusTerminalViewInWorkspace { .. }
            | FocusPane(..)
            | ShiftSelectTabRange { .. }
            | ToggleTabMultiSelection { .. }
            | ClearTabMultiSelection
            | CancelActiveRename
            | NavigatePrevPaneOrPanel
            | NavigateNextPaneOrPanel
            | OpenLightbox { .. }
            | UpdateLightboxImage { .. }
            | ShowSessionConfigModal
            | SaveCurrentTabAsNewConfig(_)
            | SyncTrafficLights
            | OpenTabConfigErrorFile { .. }
            | TabConfigSidecarMakeDefault { .. }
            | TabConfigSidecarEditConfig { .. }
            | TabConfigSidecarRemoveConfig { .. }
            | OpenSettingsFile => false,
            ViewLogs => false,
            #[cfg(target_os = "macos")]
            SampleProcess => false,
            #[cfg(target_os = "linux")]
            DismissWaylandCrashRecoveryBanner => false,
            // actions that are related to updating user settings or
            // managing some ui elements (like closing/opening modals)
            // that don't reflect on actual workspace and don't need to
            // be preserved between restarts.
        }
    }
}

#[cfg(test)]
#[path = "action_tests.rs"]
mod tests;
