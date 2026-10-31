use std::sync::atomic::{AtomicBool, Ordering};

use enum_iterator::{Sequence, cardinality};
#[cfg(feature = "test-util")]
pub use overrides::{get_overrides, set_overrides};

#[derive(Copy, Clone, Hash, PartialEq, Eq, Debug, Sequence)]
pub enum FeatureFlag {
    DebugMode,

    /// Does grid storage go forwards or backwards
    SequentialStorage,

    /// If set, generators are executed in-band in all SSH sessions.
    InBandGeneratorsForSSH,

    /// If set, generators are executed using cmd.exe on Windows.
    RunGeneratorsWithCmdExe,

    /// Gates a bindable keyboard action for accepting command corrections.
    CommandCorrectionKey,

    /// If `true`, the "Show Initialization Block" menu item is added to the Blocks menu in the Mac
    /// menu bar.
    ToggleBootstrapBlock,

    /// Ligature Support in the Editor and Grid
    Ligatures,

    /// When enabled, the `History` rule from the command_corrections crate
    /// will be enabled. When the `History` rule is enabled, the command_corrections
    /// lib will use the user's history as a last-ditch effort to find a reasonable correction.
    CommandCorrectionsHistoryRule,

    /// Enables a setting on Intel Dual-GPU Macs to enable use of the integrated GPU over the
    /// discrete GPU.
    IntegratedGPU,

    /// A setting to enable a traditional completions experience.
    ClassicCompletions,

    /// Force enable classic completions.
    ForceClassicCompletions,

    /// If enabled, autosuggestions are hidden when the tab completions
    /// menu is open (except when using completions-as-you-type).
    RemoveAutosuggestionDuringTabCompletions,

    /// Feature flag for cursor reflow fix (fixes part of the Alacritty resizing logic).
    ResizeFix,

    /// Enables Alt-modified multiple selection in the basic text editor.
    RichTextMultiselect,

    /// If enabled, the default input mode is set to waterfall for new users.
    DefaultWaterfallMode,

    /// Makes the input editor's prompt selectable.
    SelectablePrompt,

    /// Enables the settings file feature.
    SettingsFile,

    /// Enables rect selection.
    RectSelection,

    /// Adds Alacritty as a supported terminal to import settings from.
    AlacrittySettingsImport,

    /// Enables the shell selector, allowing us to open a new tab in
    /// a shell other than the default shell.
    ShellSelector,

    /// Lazily builds scenes at render time instead of eagerly when a view
    /// changes.
    LazySceneBuilding,

    /// Enables the full-screen "zen mode" setting, where we hide the tab bar if there's only one
    /// tab.
    FullScreenZenMode,

    /// Playground for reducing Warp UI clutter.
    MinimalistUI,

    /// Enables support for using native shell completions to supplement our
    /// completion specs.
    NativeShellCompletions,

    SshDragAndDrop,
    DragTabsToWindows,

    /// Enables multi-workspace selection.
    MultiWorkspace,

    /// Maximizes data in flat storage to reduce memory usage.

    /// Recognizes the OSC 8 hyperlink escape sequence and makes the
    /// linked text Cmd+click-able.
    OscHyperlinks,

    ImeMarkedText,

    /// Enables iTerm image rendering
    ITermImages,

    /// Enables validation of autosuggestions.
    ValidateAutosuggestions,

    /// Enables using `esc` to clear autosuggestions.
    ClearAutosuggestionOnEscape,

    /// Enables Kitty image rendering
    KittyImages,

    /// If enabled, command palette searches will use Tantivy search instead of the default fuzzy search.
    UseTantivySearch,

    /// UNIX shells running "natively" on Windows via MSYS2.
    MSYS2Shells,

    /// Enables the find/replace in code editor
    CodeFindReplace,

    /// Enables file search functionality in command palette
    CommandPaletteFileSearch,

    /// Enables close button on left side of tabs
    TabCloseButtonOnLeft,

    /// Enables the tabbed file viewer
    TabbedEditorView,

    /// Enables the prompt chip that displays the GitHub PR for the current branch.
    GithubPrPromptChip,

    /// Enables vim keybindings in the code editor.
    VimCodeEditor,

    /// Allows opening file links using the $EDITOR environment variable.
    AllowOpeningFileLinksUsingEditorEnv,

    /// Enables the ability to undo closed panes.
    UndoClosedPanes,

    /// Enables ignoring input suggestions.
    AllowIgnoringInputSuggestions,

    /// Enables UI zoom support (scaling the entire UI by a given percentage).
    UIZoom,

    /// Enables asynchronous find in terminal, running search on a background thread.
    AsyncFind,

    /// Enables configuring header toolbar item order, side placement, and visibility.
    ConfigurableToolbar,

    /// Enables pluggable notifications via OSC 9 and OSC 777 escape sequences.
    /// External programs can trigger system and in-app notifications.
    PluggableNotifications,

    /// Updated tab styling (background colors, border, close button positioning, margins).
    NewTabStyling,

    /// Enables incremental (diff-based) buffer updates for auto-reload instead of full replace.
    IncrementalAutoReload,

    /// Enables Kitty keyboard protocol support (CSI u encoding, progressive enhancement).
    KittyKeyboardProtocol,

    /// Enables associating a tab color with a directory so tabs automatically
    /// adopt the configured color when their working directory matches.
    DirectoryTabColors,

    /// Enables vertical tab layout as an alternative to the horizontal tab bar.
    VerticalTabs,

    /// Enables tab configs — user-definable TOML templates for launching custom tab layouts.
    TabConfigs,

    /// Replaces the in-block warpification banner with a warpify footer.
    WarpifyFooter,

    /// Gates the new SSH remote server flow that installs and connects to a
    /// persistent binary on the remote machine instead of using ControlMaster
    /// for command execution.
    SshRemoteServer,

    /// Enables summary mode in vertical tabs, showing condensed tab summaries
    /// instead of individual pane rows.
    VerticalTabsSummaryMode,

    /// Gates the Grouped Tabs feature.
    GroupedTabs,

    /// Gates the Pinned Tabs feature, which lets users pin individual tabs
    /// and whole tab groups so they stay at the front of the tab list and
    /// are protected from reordering.
    PinnedTabs,

    /// Enables state-mutating recovery for abnormal terminal lifecycle sequences.
    TerminalLifecycleRecovery,

    /// Renders supported solid box-drawing characters (`U+2500..=U+257F`)
    /// procedurally as cell-filling rectangles instead of from the font,
    /// eliminating seams between adjacent box-drawing cells in the terminal.
    BoxDrawingGlyphs,

    /// Uses fzf or atuin for history search instead of Warp's command search.
    ShellWidgetHandoff,

    /// Gates Ctrl+R / Command Search history ranking on match quality and usage priors (recency,
    /// session, exit status) plus whitespace space-AND tokenization, instead of Skim's raw
    /// fuzzy-match score against the whole query as a single pattern. Disabling this is a full
    /// return to the pre-APP-5650 history search behavior, not an approximation of it.
    HistorySearchRankingV2,
}

static FLAG_STATES: [AtomicBool; cardinality::<FeatureFlag>()] =
    [const { AtomicBool::new(false) }; { cardinality::<FeatureFlag>() }];

/// Flag for whether or not feature flags have been globally initialized. Outside
/// of tests, this ensures that feature flags are only used after they're set
/// up by the app's `run_internal` function.
#[cfg(debug_assertions)]
static FEATURES_INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Features used in debugging.
pub const DEBUG_FLAGS: &[FeatureFlag] = &[FeatureFlag::DebugMode];

/// Platform capabilities enabled for release bundles.
/// Other default features are configured in app/Cargo.toml.
pub const RELEASE_FLAGS: &[FeatureFlag] = &[
    FeatureFlag::ImeMarkedText,
    // Remote server binary is not yet supported on Windows.
    #[cfg(not(windows))]
    FeatureFlag::SshRemoteServer,
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    FeatureFlag::DragTabsToWindows,
];

impl FeatureFlag {
    pub fn is_enabled(&self) -> bool {
        #[cfg(all(debug_assertions, not(feature = "test-util")))]
        {
            use std::sync::atomic::Ordering;
            assert!(
                FEATURES_INITIALIZED.load(Ordering::Relaxed),
                "Tried to check FeatureFlag::{self:?} before feature flags were initialized"
            );
        }

        #[cfg(feature = "test-util")]
        if let Some(enabled) = overrides::get_override(*self) {
            return enabled;
        }

        FLAG_STATES[*self as usize].load(Ordering::Relaxed)
    }

    #[allow(dead_code)]
    pub fn set_enabled(self, enabled: bool) {
        // Allow calling this in integration tests because we sometimes use it in the app
        // during flows that integration tests cover.
        if cfg!(test) && cfg!(not(feature = "integration_tests")) {
            panic!(
                "Tried to globally enable {self:?} in a test. Use FeatureFlag::{self:?}.override_enabled instead"
            );
        }
        FLAG_STATES[self as usize].store(enabled, Ordering::Relaxed);
    }

    /// Sets a thread-local test override for this flag. The override lasts
    /// until the returned guard is dropped.
    ///
    /// **Warning**: overrides do not work for tests of multi-threaded code. If
    /// you need to test multi-threaded code that's behind a feature flag, you'll
    /// need to set an override in _each_ thread.
    ///
    /// Tests should create overrides early on and allow them to be
    /// dropped automatically when they finish. This keeps overrides scoped to
    /// the duration of the test, since Rust doesn't have test lifecycle hooks.
    #[cfg(feature = "test-util")]
    pub fn override_enabled(self, enabled: bool) -> overrides::OverrideGuard {
        overrides::override_flag(self, enabled)
    }
}

/// Marks that feature flags have been globally initialized.
pub fn mark_initialized() {
    #[cfg(debug_assertions)]
    FEATURES_INITIALIZED.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Thread-local feature flag overrides for unit tests. For isolation, tests
/// should use overrides instead of globally modifying flags with [`super::FeatureFlag::set_enabled`].
#[cfg(feature = "test-util")]
mod overrides {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use super::FeatureFlag;

    thread_local! {
        static FLAG_OVERRIDES: RefCell<HashMap<FeatureFlag,bool>> = RefCell::new(HashMap::new());
    }

    /// RAII guard to set feature flag overrides in tests. When the guard is
    /// dropped, it reverts to the global flag state.
    #[must_use = "if unused the override will be immediately cleared"]
    pub struct OverrideGuard {
        flag: FeatureFlag,
    }

    /// Gets the overridden state for a flag, if set.
    pub fn get_override(flag: FeatureFlag) -> Option<bool> {
        FLAG_OVERRIDES.with(|overrides| overrides.borrow().get(&flag).copied())
    }

    /// Gets the set of overridden flags.
    pub fn get_overrides() -> HashMap<FeatureFlag, bool> {
        FLAG_OVERRIDES.with(|overrides| overrides.borrow().clone())
    }

    /// Applies a set of overrides.
    ///
    /// This is intended to be used with [`get_overrides`] to apply a set of
    /// existing overrides to a newly-spawned thread.  If you are trying to
    /// override a single feature flag, use [`FeatureFlag::override_enabled`]
    /// instead.
    pub fn set_overrides(new_overrides: HashMap<FeatureFlag, bool>) {
        FLAG_OVERRIDES.with(|overrides| *overrides.borrow_mut() = new_overrides);
    }

    /// Set a thread-local override for a flag.
    pub fn override_flag(flag: FeatureFlag, enabled: bool) -> OverrideGuard {
        set_override(flag, enabled);
        OverrideGuard { flag }
    }

    fn set_override(flag: FeatureFlag, enabled: bool) {
        FLAG_OVERRIDES.with(|overrides| {
            let previous = overrides.borrow_mut().insert(flag, enabled);
            // We could support nested overrides, but it requires some care around
            // out-of-order drops - if overrides are set and then cleared out of
            // order, what should the state after each drop be?
            if previous.is_some() {
                panic!("Multiple overrides set for {flag:?}");
            }
        });
    }

    fn clear_override(flag: FeatureFlag) {
        FLAG_OVERRIDES.with(|overrides| {
            let previous = overrides.borrow_mut().remove(&flag);
            if previous.is_none() {
                panic!("Cleared override for {flag:?}, but none was set");
            }
        });
    }

    impl Drop for OverrideGuard {
        fn drop(&mut self) {
            clear_override(self.flag);
        }
    }
}
