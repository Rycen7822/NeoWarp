#![allow(clippy::doc_lazy_continuation)]

mod alloc;
mod antivirus;
#[cfg(target_os = "macos")]
mod app_menus;
mod app_services;
mod app_state;
mod banner;
mod chip_configurator;
mod code;
mod code_review;
mod command_palette;
mod completer;
#[allow(dead_code)]
mod context_chips;
#[cfg(enable_crash_recovery)]
mod crash_recovery;
mod debug_dump;
mod default_terminal;
#[cfg(windows)]
mod dynamic_libraries;
#[cfg(feature = "local_fs")]
mod file_browser;
mod file_watcher;
mod global_resource_handles;
mod gpu_state;
mod interval_timer;
#[cfg(any(target_os = "macos", target_os = "windows"))]
mod login_item;
mod menu;
mod modal;
mod network;
mod notification;
mod palette;
mod persistence;
mod platform;
mod prefix;
mod profiling;
mod prompt;
mod quit_warning;
#[allow(dead_code)]
mod remote_server;
mod resource_limits;
mod safe_triangle;
mod search_bar;
mod session_management;
mod shell_indicator;
mod suggestions;
mod system;
mod tab;
#[cfg(test)]
mod test_util;
mod throttle;
mod tips;
mod ui_components;
mod undo_close;
mod uri;
mod user_config;
pub mod util;
mod view_components;
mod vim_registers;
mod warp_managed_paths_watcher;
mod window_settings;

// PLEASE DO NOT ADD MORE PUBLIC MODULES!
//
// Any modules which we make public outside of the `warp` crate lose dead code
// checking support, as the compiler cannot make any assumptions about whether
// or not the function/type is used by another crate that pulls in this one as
// a dependency.
//
// If you feel the need to export a module so that a type or function within it
// can be used by an integration test, you should define a new assertion function
// in the warp::integration_testing::assertions module (or a sub-module).  These
// functions will allow us to keep types internal to this crate and expose a
// simpler API for integration tests to consume.
pub mod appearance;
pub mod channel;
pub mod editor;
pub mod features;
pub mod input_suggestions;
#[cfg(feature = "integration_tests")]
pub mod integration_testing;
pub mod keyboard;
pub mod launch_configs;
pub mod pane_group;
pub mod resource_center;
pub mod root_view;
pub mod search;
pub mod settings;
pub mod settings_view;
pub mod tab_configs;
pub mod terminal;
pub mod themes;
use code::editor_management::CodeManager;
use code::opened_files::OpenedFilesModel;
use code::persisted_workspace::PersistedWorkspace;
use code_review::git_repo_model::GitRepoModels;
use quit_warning::UnsavedStateSummary;
#[cfg(feature = "local_fs")]
use repo_metadata::{
    RepoMetadataModel, repositories::DetectedRepositories, watcher::DirectoryWatcher,
};
#[cfg(feature = "local_fs")]
use settings::import::model::ImportedConfigModel;
use settings_view::pane_manager::SettingsPaneManager;
use terminal::general_settings::GeneralSettings;
use terminal::keys_settings::KeysSettings;
#[cfg(feature = "local_tty")]
use terminal::local_shell::LocalShellState;
pub use util::bindings::cmd_or_ctrl_shift;
#[cfg(feature = "local_fs")]
pub mod workspace;

use std::borrow::Cow;
use std::ops::Deref;

use ::settings::{Setting, ToggleableSetting};
#[cfg(feature = "local_tty")]
use anyhow::Context;
use anyhow::Result;
use appearance::{Appearance, AppearanceManager};
use channel::ChannelState;
use interval_timer::IntervalTimer;
use itertools::Itertools;
#[cfg(feature = "integration_tests")]
pub use persistence::testing as sqlite_testing;
use settings::{ExtraMetaKeys, PrivacySettings};
use terminal::input;
use terminal::session_settings::SessionSettings;
use url::Url;
// Re-export the debounce function to simplify imports.
pub use warp_core::r#async::debounce;
use warp_core::execution_mode::{AppExecutionMode, ExecutionMode};
// Re-export the safe logging macros at the crate root level for backwards compatibility
use warp_errors::{report_error, report_if_error};
#[cfg(feature = "local_fs")]
use warp_files::FileModel;
use warp_logging::{LogDestination, LogFrontend};
use warpui::integration::TestDriver;
use warpui::platform::TerminationMode;
use warpui::platform::app::{ApproveTerminateResult, TerminationRequestSource};
use warpui::windowing::state::ApplicationStage;
use warpui::{AppContext, Event, SingletonEntity, WindowId};
use window_settings::WindowSettings;
use workspace::sync_inputs::SyncedInputState;

use self::features::FeatureFlag;
use crate::antivirus::AntivirusInfo;
use crate::app_state::AppState;
use crate::code::global_buffer_model::GlobalBufferModel;
use crate::context_chips::prompt::Prompt;
use crate::default_terminal::DefaultTerminal;
pub use crate::global_resource_handles::{GlobalResourceHandles, GlobalResourceHandlesProvider};
use crate::gpu_state::GPUState;
use crate::network::NetworkStatus;
use crate::notification::NotificationContext;
use crate::palette::{PaletteMode, PaletteSource};
use crate::persistence::PersistenceWriter;
use crate::root_view::{
    OpenFromRestoredArg, OpenPath, quake_mode_window_id, quake_mode_window_is_open,
};
use crate::session_management::{RunningSessionSummary, SessionNavigationData};
use crate::settings::manager::SettingsManager;
use crate::settings::{AccessibilitySettings, ScrollSettings, SelectionSettings};
use crate::settings_view::DisplayCount;
use crate::settings_view::keybindings::KeybindingChangedNotifier;
use crate::system::SystemStats;
use crate::tab::TabShortcutModifierState;
use crate::terminal::resizable_data::ResizableData;
use crate::terminal::{AudibleBell, CustomSecretRegexUpdater, History};
use crate::undo_close::UndoCloseStack;
use crate::user_config::WarpConfig;
use crate::util::bindings::is_binding_cross_platform;
use crate::vim_registers::VimRegisters;
use crate::warp_managed_paths_watcher::{WarpManagedPathsWatcher, ensure_warp_watch_roots_exist};
use crate::workspace::{ActiveSession, PaneViewLocator, ToastStack, Workspace, WorkspaceAction};

/// Our embedded application assets.
pub static ASSETS: warp_assets::Assets = warp_assets::Assets;

/// Launch mode for how to start up Warp.
pub(crate) enum LaunchMode {
    /// Run the regular GUI application.
    App { args: warp_cli::AppArgs },

    /// Run a test - this may be an integration test or an eval.
    Test {
        driver: Box<Option<TestDriver>>,
        is_integration_test: bool,
    },

    /// Remote server proxy — bridges SSH stdio to the daemon's Unix socket.
    /// This is a short-lived process that runs for the lifetime of an SSH session.
    RemoteServerProxy,

    /// Remote server daemon — long-lived headless process serving remote
    /// connections via a Unix domain socket.
    #[cfg_attr(not(unix), allow(dead_code))]
    RemoteServerDaemon {
        /// Stable identity key used to partition the daemon's socket/PID
        /// directory on the remote host.
        identity_key: String,
    },
}

impl LaunchMode {
    fn args(&self) -> Cow<'_, warp_cli::AppArgs> {
        match self {
            Self::App { args, .. } => Cow::Borrowed(args),
            Self::Test { .. } | Self::RemoteServerProxy | Self::RemoteServerDaemon { .. } => {
                Cow::Owned(warp_cli::AppArgs::default())
            }
        }
    }

    fn is_integration_test(&self) -> bool {
        matches!(
            self,
            Self::Test {
                is_integration_test: true,
                ..
            }
        )
    }

    fn take_test_driver(&mut self) -> Option<TestDriver> {
        match self {
            Self::Test { driver, .. } => driver.take(),
            Self::App { .. } | Self::RemoteServerProxy | Self::RemoteServerDaemon { .. } => None,
        }
    }

    /// Add a URL to open. Only supported for [`LaunchMode::App`].
    #[allow(dead_code)]
    fn add_url(&mut self, url: Url) {
        if let Self::App { args, .. } = self {
            args.urls.push(url);
        }
    }

    fn execution_mode(&self) -> ExecutionMode {
        match self {
            Self::App { .. } | Self::Test { .. } => ExecutionMode::App,
            Self::RemoteServerProxy => ExecutionMode::RemoteServerProxy,
            Self::RemoteServerDaemon { .. } => ExecutionMode::RemoteServerDaemon,
        }
    }

    fn is_gui(&self) -> bool {
        matches!(self, Self::App { .. } | Self::Test { .. })
    }

    fn is_headless(&self) -> bool {
        matches!(
            self,
            Self::RemoteServerProxy | Self::RemoteServerDaemon { .. }
        )
    }

    /// Only GUI instances with heap profiling enabled need the local HTTP server.
    fn should_start_local_http_server(&self) -> bool {
        self.is_gui() && cfg!(feature = "jemalloc_pprof")
    }

    #[cfg(enable_crash_recovery)]
    pub(crate) fn crash_recovery_enabled(&self) -> bool {
        matches!(self, Self::App { .. })
    }

    fn log_destination(&self) -> Option<LogDestination> {
        match self {
            Self::RemoteServerProxy => Some(LogDestination::Stderr),
            Self::RemoteServerDaemon { .. } => Some(LogDestination::File),
            Self::App { .. } | Self::Test { .. } => None,
        }
    }

    fn log_frontend(&self) -> LogFrontend {
        match self {
            Self::App { .. } | Self::Test { .. } => LogFrontend::Gui,
            Self::RemoteServerProxy | Self::RemoteServerDaemon { .. } => LogFrontend::Cli,
        }
    }

    fn as_str_for_tracing(&self) -> &'static str {
        match self {
            Self::App { .. } => "app",
            Self::Test { .. } => "test",
            Self::RemoteServerProxy => "remote_server_proxy",
            Self::RemoteServerDaemon { .. } => "remote_server_daemon",
        }
    }
}

/// If the given event is a key down event containing alt modifiers, and those
/// alt modifiers should be treated as meta keys, then remove the alts and
/// prefix the keys with an escape. See WAR-472.
fn apply_extra_meta_keys(event: &mut Event, extra_metas: ExtraMetaKeys) {
    if let Event::KeyDown {
        keystroke, details, ..
    } = event
    {
        let left_as_meta = extra_metas.left_alt && details.left_alt;
        let right_as_meta = extra_metas.right_alt && details.right_alt;
        if left_as_meta || right_as_meta {
            let side = match (left_as_meta, right_as_meta) {
                (true, true) => "left+right alt",
                (true, false) => "left alt",
                (false, true) => "right alt",
                (false, false) => unreachable!(),
            };
            log::info!("Treating {side} as meta");
            keystroke.alt = false;
            keystroke.meta = true;
        }
    }
}

fn apply_scroll_multiplier(event: &mut Event, app: &AppContext) {
    if let Event::ScrollWheel { delta, precise, .. } = event
        && !*precise
    {
        let scroll_multiplier = *ScrollSettings::as_ref(app).mouse_scroll_multiplier.value();
        *delta *= scroll_multiplier;
    }
}

/// Runs the desktop application or one of its workers.
#[::tracing::instrument(skip_all)]
pub fn run() -> Result<()> {
    // Ensure feature flags are initialized before parsing command-line arguments.
    features::init_feature_flags();

    // Parse command-line arguments.
    let args = warp_cli::Args::from_env();

    if let Some(command) = args.command() {
        #[cfg(windows)]
        if command.prints_to_stdout() {
            // We attach a console to ensure that all standard output gets printed correctly.
            warp_util::windows::attach_to_parent_console();
        }
        match command {
            warp_cli::Command::Worker(worker) => return run_worker_command(worker),
            warp_cli::Command::DumpDebugInfo => {
                return debug_dump::run();
            }
            warp_cli::Command::DumpSettingsSchema { output_path } => {
                return settings::schema_generation::dump_settings_schema(output_path.as_deref());
            }
        }
    }

    run_internal(LaunchMode::App {
        args: args.into_app_args(),
    })
}

/// Runs a parsed Warp worker command.
fn run_worker_command(worker: &warp_cli::WorkerCommand) -> Result<()> {
    match worker {
        #[cfg(all(feature = "local_tty", unix))]
        warp_cli::WorkerCommand::TerminalServer(args) => {
            crate::terminal::local_tty::run_terminal_server(args);
            Ok(())
        }
        warp_cli::WorkerCommand::RemoteServerProxy(args) => {
            // Proxy is a thin byte bridge (stdin/stdout ↔ Unix socket).
            // It only needs logging to stderr since stdout is the protocol
            // channel. No crash reporting, no initialize_app.
            let launch_mode = LaunchMode::RemoteServerProxy;
            ::tracing::subscriber::set_global_default(::tracing::subscriber::NoSubscriber::new())?;
            warp_logging::init(warp_logging::LogConfig {
                frontend: launch_mode.log_frontend(),
                log_destination: launch_mode.log_destination(),
                ..Default::default()
            })?;
            crate::remote_server::run_proxy(args.identity_key.clone())
        }
        warp_cli::WorkerCommand::RemoteServerDaemon(args) => {
            // Daemon handles its own full initialization (including
            // initialize_app and crash reporting) inside run_daemon_app.
            crate::remote_server::run_daemon(args.identity_key.clone())
        }
    }
}

/// Runs an integration test using the provided test driver.
pub fn run_integration_test(driver: TestDriver) -> Result<()> {
    let is_integration_test = std::env::var("WARP_INTEGRATION").is_ok();
    let launch = LaunchMode::Test {
        driver: Box::new(Some(driver)),
        is_integration_test,
    };
    run_internal(launch)
}

/// Runs the desktop application, tests, or remote daemon.
fn run_internal(mut launch_mode: LaunchMode) -> Result<()> {
    let mut timer = IntervalTimer::new();

    // ── Early initialization (pre-AppBuilder) ──────────────────────
    // These steps run before the platform event loop is started.
    // They must not depend on AppContext.

    #[cfg(windows)]
    dynamic_libraries::configure_library_loading();

    profiling::init();

    // The `run` function already initializes feature flags, but ensure they're initialized here
    // for other entrypoints.
    features::init_feature_flags();

    // Some environment predicates read feature flags, but none need a GUI.
    if let LaunchMode::Test { driver, .. } = &mut launch_mode
        && let Some(driver) = driver.as_mut()
        && !driver.should_run_test()
    {
        println!("Skipping integration test before GUI startup");
        return Ok(());
    }

    ::tracing::subscriber::set_global_default(::tracing::subscriber::NoSubscriber::new())?;

    // Install the subscriber before creating spans so tracing does not duplicate log output.
    let span = ::tracing::info_span!(
        "run_internal",
        launch_mode = launch_mode.as_str_for_tracing()
    );
    let _enter = span.enter();

    let log_destination = launch_mode.log_destination();

    cfg_if::cfg_if! {
        if #[cfg(enable_crash_recovery)] {
            if crash_recovery::is_crash_recovery_process(launch_mode.args().as_ref()) {
                warp_logging::init_for_crash_recovery_process()?;
            } else {
                warp_logging::init(warp_logging::LogConfig {
                    frontend: launch_mode.log_frontend(),
                    log_destination,
                    ..Default::default()
                })?;
            }
        } else {
            warp_logging::init(warp_logging::LogConfig {
                frontend: launch_mode.log_frontend(),
                log_destination,
                ..Default::default()
            })?;
        }
    }
    timer.mark_interval_end("LOG_FILE_SETUP_COMPLETE");

    // Claim a background-only process type before anything else can reach
    // AppKit, so a windowless launch never acquires a Dock tile. See APP-2946.
    #[cfg(target_os = "macos")]
    if !launch_mode.is_gui()
        && let Err(e) = platform::mac::mark_process_as_background_only()
    {
        log::warn!("Failed to mark process as background-only: {e:#}");
    }

    #[cfg(windows)]
    platform::windows::check_redirection_guard();

    // Adjust resource limits early, before doing other work, to ensure that
    // any children we spawn (like the terminal server) inherit our adjusted
    // rlimits.
    resource_limits::adjust_resource_limits();

    #[cfg(all(
        feature = "release_bundle",
        any(target_os = "linux", target_os = "freebsd")
    ))]
    if let LaunchMode::App { .. } = launch_mode {
        match app_services::linux::pass_startup_args_to_existing_instance(
            launch_mode.args().as_ref(),
        ) {
            // If we were able to contact an existing application instance, quit -
            // we only want to run a single instance of Warp at a time.
            Ok(_) => std::process::exit(0),
            // If Warp isn't already running, we're good to go.
            Err(app_services::linux::StartupArgsForwardingError::NoExistingInstance) => {}
            Err(
                app_services::linux::StartupArgsForwardingError::IgnoredForCrashRecoveryProcess,
            ) => {}
            // If we were unable to perform the forwarding for an unknown reason,
            // it's better to run a second instance than potentially end up in a
            // state where Warp refuses to run even a first instance.
            Err(err) => {
                let err = anyhow::Error::from(err).context("Failed to forward startup args");
                report_error!(&err);
            }
        }
    }

    #[cfg(all(feature = "release_bundle", windows))]
    if let LaunchMode::App { .. } = launch_mode {
        match app_services::windows::pass_startup_args_to_existing_instance(
            launch_mode.args().as_ref(),
        ) {
            // If we were able to contact an existing application instance, quit -
            // we only want to run a single instance of Warp at a time.
            Ok(_) => std::process::exit(0),
            // If Warp isn't already running, we're good to go.
            Err(app_services::windows::StartupArgsForwardingError::NoExistingInstance) => {}
            Err(
                app_services::windows::StartupArgsForwardingError::IgnoredForCrashRecoveryProcess,
            ) => {}
            // If we were unable to perform the forwarding for an unknown reason,
            // it's better to run a second instance than potentially end up in a
            // state where Warp refuses to run even a first instance.
            Err(err) => {
                let err = anyhow::Error::from(err).context("Failed to forward startup args");
                report_error!(&err);
            }
        }
    }

    // Sets up a Job Object that we associate with the Warp process to handle
    // shared fate with its child processes. This should be called before we
    // start spawning any child processes.
    #[cfg(windows)]
    command::windows::init();

    let private_preferences = settings::init_private_user_preferences();
    let (public_preferences, startup_toml_parse_error) = settings::init_public_user_preferences();

    // When the SettingsFile feature flag is enabled, public settings live in
    // the TOML-backed store. When disabled, they live in the platform-native
    // store (same backend as private). Use the correct one for pre-app reads.
    #[cfg_attr(
        not(any(
            enable_crash_recovery,
            target_os = "linux",
            target_os = "freebsd",
            target_os = "macos"
        )),
        expect(unused)
    )]
    let prefs_for_public_settings: &dyn warpui_extras::user_preferences::UserPreferences =
        if FeatureFlag::SettingsFile.is_enabled() {
            public_preferences.as_ref()
        } else {
            private_preferences.deref()
        };

    #[cfg(enable_crash_recovery)]
    let crash_recovery =
        crash_recovery::CrashRecovery::new(&launch_mode, prefs_for_public_settings);

    // Set up the pty spawner before doing any meaningful work. We want to
    // ensure that the process is in the cleanest possible state (minimal opened
    // files, modified signal handlers, etc.) to avoid unexpected effects on
    // spawned ptys.
    //
    #[cfg(feature = "local_tty")]
    let pty_spawner =
        terminal::local_tty::spawner::PtySpawner::new().context("Failed to create pty spawner")?;

    let callbacks = app_callbacks(launch_mode.is_integration_test());
    let mut app_builder = if launch_mode.is_gui() {
        warpui::platform::AppBuilder::new(
            callbacks,
            Box::new(ASSETS),
            launch_mode.take_test_driver(),
        )
    } else {
        warpui::platform::AppBuilder::new_windowless(
            callbacks,
            Box::new(ASSETS),
            launch_mode.take_test_driver(),
        )
    };

    // A user is present for any launch with a UI, so it may query microphone authorization.
    if !launch_mode.is_headless() {
        app_builder.enable_windowless_microphone_access_query();
    }

    // A windowless invocation has no Dock presence, so it performs no Dock-visible
    // setup at all (Dock icon, Dock menu, menu bar). See APP-2946.
    #[cfg(target_os = "macos")]
    if launch_mode.is_gui() {
        use warpui::AssetProvider as _;
        use warpui::platform::mac::AppExt;

        let activate_on_launch = !launch_mode.is_integration_test()
            || std::env::var("WARPUI_USE_REAL_DISPLAY_IN_INTEGRATION_TESTS").is_ok();
        app_builder.set_activate_on_launch(activate_on_launch);

        let dev_icon = ASSETS.get("bundled/png/local.png")?;
        app_builder.set_dev_icon(dev_icon);

        let show_dock_icon = crate::settings::app_icon::ShowDockIconState::read_from_preferences(
            prefs_for_public_settings,
        )
        .unwrap_or_else(crate::settings::app_icon::ShowDockIconState::default_value);
        app_builder.set_show_dock_icon_on_launch(show_dock_icon);
        app_builder.set_menu_bar_builder(app_menus::menu_bar);
        app_builder.set_dock_menu_builder(|_| app_menus::dock_menu());
    }

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    {
        use warpui::platform::linux::{self, AppBuilderExt};

        use crate::settings::ForceX11;

        app_builder.set_window_class(ChannelState::app_id().to_string());

        let force_x11 = ForceX11::read_from_preferences(prefs_for_public_settings)
            .unwrap_or(ForceX11::default_value());
        // Force use of wayland if the user has passed the `WARP_ENABLE_WAYLAND` env var.
        let allow_wayland = linux::is_wayland_env_var_set() || !force_x11;
        app_builder.force_x11(!allow_wayland);
    }

    #[cfg(target_os = "windows")]
    {
        use warpui::platform::windows::AppBuilderExt;
        app_builder.set_app_user_model_id(ChannelState::app_id().to_string());

        // Only use DXC for DirectX shader compilation if we're not running in a Parallels VM
        // Parallels VMs can have issues with DXC shader compilation
        let is_parallels_vm = crate::util::vm_detection::is_running_in_windows_parallels_vm();
        if !is_parallels_vm {
            log::info!("Using DXC for DirectX shader compilation");
            use warpui::platform::windows::DXCPath;

            app_builder.use_dxc_for_directx_shader_compilation(DXCPath {
                dxc_path: "dxcompiler.dll".to_string(),
                dxil_path: "dxil.dll".to_string(),
            });
        } else {
            log::info!("Skipping DXC for DirectX shader compilation; running in a Parallels VM");
        }
    }

    // Override any bindings that have a `Custom` trigger to a `Keystroke`-based trigger. In theory,
    // this should be a noop on Mac (since the keystrokes registered via the  Mac menus first
    // intercept the binding), but just to be safe we only enable this in cases where we don't
    // include mac menus.
    #[cfg(not(target_os = "macos"))]
    app_builder.convert_custom_triggers_to_keystroke_triggers(
        crate::util::bindings::custom_tag_to_keystroke,
    );

    #[cfg(target_os = "macos")]
    app_builder.register_default_keystroke_triggers_for_custom_actions(
        crate::util::bindings::custom_tag_to_keystroke,
    );

    app_builder.run(move |ctx| {
        // Rotate the log files in the background.
        ctx.background_executor()
            .spawn(warp_logging::rotate_log_files())
            .detach();

        ctx.add_singleton_model(|ctx| AppExecutionMode::new(launch_mode.execution_mode(), ctx));

        // Add the terminal server singleton to the application.
        #[cfg(feature = "local_tty")]
        ctx.add_singleton_model(move |_ctx| pty_spawner);

        // Register user preferences.  This must be done before initializing
        // feature flags, which check user preferences for overrides.
        ctx.add_singleton_model(move |_ctx| ::settings::PublicPreferences::new(public_preferences));
        ctx.add_singleton_model(move |_ctx| private_preferences);
        let startup_toml_parse_error = startup_toml_parse_error;

        #[cfg(enable_crash_recovery)]
        ctx.add_singleton_model(move |_ctx| crash_recovery);

        let app_state = initialize_app(&launch_mode, timer, startup_toml_parse_error, ctx);

        FeatureFlag::UseTantivySearch.set_enabled(true);

        launch(ctx, app_state, launch_mode)
    })
}

pub struct UpdateQuakeModeEventArg {
    active_window_id: Option<WindowId>,
}

#[::tracing::instrument(skip_all)]
pub(crate) fn initialize_app(
    launch_mode: &LaunchMode,
    mut timer: IntervalTimer,
    startup_toml_parse_error: Option<warpui_extras::user_preferences::Error>,
    ctx: &mut warpui::AppContext,
) -> Option<AppState> {
    ensure_warp_watch_roots_exist();
    ctx.add_singleton_model(WarpManagedPathsWatcher::new);

    ctx.add_singleton_model(WarpConfig::new);
    ctx.add_singleton_model(|_ctx| SettingsManager::default());

    let user_defaults_on_startup = settings::init(startup_toml_parse_error, ctx);
    window_settings::migrate_legacy_background_backdrop(ctx);
    timer.mark_interval_end("READ_USER_DEFAULTS_AND_INITIALIZE_SETTINGS");

    if FeatureFlag::UIZoom.is_enabled() {
        ctx.set_zoom_factor(WindowSettings::as_ref(ctx).zoom_level.as_zoom_factor());
    }

    ctx.add_singleton_model(|_ctx| GPUState::new());

    PrivacySettings::register_singleton(ctx);

    // If any part of sqlite initialization fails, we just don't do session restoration (i.e.
    // feature degradation).
    let persistence_scope = match launch_mode {
        LaunchMode::RemoteServerDaemon { identity_key } => {
            persistence::PersistenceScope::RemoteServerDaemon {
                identity_key: identity_key.clone(),
            }
        }
        LaunchMode::App { .. } | LaunchMode::RemoteServerProxy | LaunchMode::Test { .. } => {
            persistence::PersistenceScope::App
        }
    };
    let (sqlite_data, writer_handles) = persistence::initialize(persistence_scope);
    timer.mark_interval_end("SQLITE_INITIALIZED");

    let persistence_writer = PersistenceWriter::new(writer_handles);

    let model_event_sender = persistence_writer.sender();

    let tips_handle = ctx.add_model(|_| user_defaults_on_startup.tips_data);
    let user_default_shell_unsupported_banner_model_handle =
        ctx.add_model(|_| user_defaults_on_startup.user_default_shell_unsupported_banner_state);
    let settings_file_error = user_defaults_on_startup.settings_file_error;
    ctx.add_singleton_model(move |_ctx| {
        GlobalResourceHandlesProvider::new(GlobalResourceHandles {
            model_event_sender,
            tips_completed: tips_handle,
            user_default_shell_unsupported_banner_model_handle,
            settings_file_error,
        })
    });

    let (app_state, command_history, persisted_workspaces, persisted_ignored_suggestions) =
        sqlite_data
            .map(|sqlite_data| {
                (
                    sqlite_data.app_state,
                    sqlite_data.command_history,
                    sqlite_data.workspace_metadata,
                    sqlite_data.ignored_suggestions,
                )
            })
            .unwrap_or_default();

    ctx.add_singleton_model(AntivirusInfo::new);

    ctx.set_default_binding_validator(is_binding_cross_platform);

    // Initialize timestamp for session id and last active event

    ctx.add_singleton_model(|_| SettingsPaneManager::new());
    #[cfg(target_os = "macos")]
    if launch_mode.is_gui() {
        AppearanceManager::as_ref(ctx).set_app_icon(ctx);
    }

    #[cfg(feature = "local_tty")]
    terminal::available_shells::register(ctx);

    // Add truly global actions that don't depend on the existence of any view here
    ctx.add_global_action("app:toggle_user_ps1", move |_args: &(), ctx| {
        SessionSettings::handle(ctx).update(ctx, |session_settings, ctx| {
            report_if_error!(session_settings.honor_ps1.toggle_and_save_value(ctx));
        });
    });
    ctx.add_global_action("app:toggle_copy_on_select", move |_args: &(), ctx| {
        SelectionSettings::handle(ctx).update(ctx, |selection_settings, ctx| {
            report_if_error!(selection_settings.copy_on_select.toggle_and_save_value(ctx));
        });
    });

    ctx.add_singleton_model(|_ctx| SyncedInputState::new());

    ctx.add_singleton_model(remote_server::manager::RemoteServerManager::new);

    log::info!(
        "Starting warp with channel state {} and version {:?}",
        ChannelState::debug_str(),
        ChannelState::app_version()
    );

    // Teach our app that sometimes option means meta.
    ctx.set_event_munger(move |event, ctx| {
        let extra_meta_keys = *KeysSettings::as_ref(ctx).extra_meta_keys;
        apply_extra_meta_keys(event, extra_meta_keys);
        apply_scroll_multiplier(event, ctx);
    });

    ctx.set_a11y_verbosity(*AccessibilitySettings::as_ref(ctx).a11y_verbosity);

    #[cfg(enable_crash_recovery)]
    ctx.on_draw_frame_error(|ctx, window_id| {
        crash_recovery::CrashRecovery::handle(ctx).update(ctx, |crash_recovery, _ctx| {
            crash_recovery.on_draw_frame_error(window_id);
        });
    });

    ctx.on_first_frame_drawn(move |ctx| {
        GPUState::handle(ctx).update(ctx, |gpu_state, ctx| {
            gpu_state.set_has_lower_power_gpu(warpui::rendering::is_low_power_gpu_available(), ctx);
        });

        for window_id in ctx.window_ids().collect_vec() {
            for settings in ctx
                .views_of_type::<settings_view::SettingsView>(window_id)
                .into_iter()
                .flatten()
            {
                settings.update(ctx, |settings, ctx| {
                    settings.refresh_preferred_graphics_backend_dropdown(ctx);
                });
            }
        }

        IntervalTimer::handle(ctx).update(ctx, |timer, _| {
            timer.mark_interval_end("FIRST_FRAME_DRAWN");
        });
    });

    #[cfg(enable_crash_recovery)]
    ctx.on_frame_drawn(|ctx, window_id| {
        crash_recovery::CrashRecovery::handle(ctx).update(ctx, |crash_recovery, _| {
            crash_recovery.on_frame_drawn(window_id);
        });
    });
    {
        ctx.add_singleton_model(DirectoryWatcher::new);
        ctx.add_singleton_model(|_| DetectedRepositories::default());
    }

    #[cfg(feature = "local_fs")]
    {
        let imported_config_model = ctx.add_singleton_model(ImportedConfigModel::new);

        if ChannelState::channel() != warp_core::channel::Channel::Integration {
            imported_config_model.update(ctx, |model, ctx| {
                model.search_for_settings_to_import(ctx);
            });
        }

        let emit_incremental_updates = matches!(launch_mode, LaunchMode::RemoteServerDaemon { .. });
        ctx.add_singleton_model(|ctx| {
            let model = if emit_incremental_updates {
                RepoMetadataModel::new_with_incremental_updates(ctx)
            } else {
                RepoMetadataModel::new(ctx)
            };
            // Subscribe to RemoteServerManager push events so that remote repo
            // metadata snapshots and incremental updates populate the remote
            // sub-model and trigger RepoMetadataEvent emissions.
            {
                use remote_server::manager::{RemoteServerManager, RemoteServerManagerEvent};
                let mgr = RemoteServerManager::handle(ctx);
                ctx.subscribe_to_model(&mgr, |me, _, event, ctx| match event {
                    RemoteServerManagerEvent::RepoMetadataSnapshot { host_id, update } => {
                        me.insert_remote_snapshot(host_id.clone(), update, ctx);
                    }
                    RemoteServerManagerEvent::RepoMetadataUpdated { host_id, update }
                    | RemoteServerManagerEvent::RepoMetadataDirectoryLoaded { host_id, update } => {
                        me.apply_remote_incremental_update(host_id, update, ctx);
                    }
                    RemoteServerManagerEvent::HostDisconnected { host_id } => {
                        me.remove_remote_repositories_for_host(host_id, ctx);
                    }
                    _ => {}
                });
            }

            model
        });
    }

    ctx.add_singleton_model(|_| GitRepoModels::new());

    ctx.add_singleton_model(move |_| History::new(command_history));

    ctx.add_singleton_model(CustomSecretRegexUpdater::new);

    // Register initial keybindings prior to creating menus
    view_components::keyboard_navigable_buttons::init(ctx);
    app_services::init(ctx);
    // // TODO: Temporarily disabling keybindings for WASM builds. Will be implemented in future WASM support.
    code::editor::find::view::init(ctx);
    workspace::init(ctx);
    pane_group::init(ctx);
    terminal::init(ctx);
    input::init(ctx);
    editor::init(ctx);
    menu::init(ctx);
    tips::tip_view::init(ctx);
    launch_configs::init(ctx);
    themes::theme_chooser::init(ctx);
    themes::theme_creator_modal::init(ctx);
    themes::theme_deletion_modal::init(ctx);
    root_view::init(ctx);
    crate::view_components::find::init(ctx);
    prompt::editor_modal::init(ctx);
    undo_close::init(ctx);
    tab_configs::params_modal::init(ctx);
    context_chips::display_menu::init(ctx);
    context_chips::node_version_popup::init(ctx);

    let display_count = ctx.windows().display_count();
    ctx.add_singleton_model(|_| DisplayCount(display_count));

    ctx.add_singleton_model(|_| NetworkStatus::new());
    ctx.add_singleton_model(|_| SystemStats::new());
    ctx.add_singleton_model(|_| KeybindingChangedNotifier::new());
    ctx.add_singleton_model(|_| TabShortcutModifierState::new());
    ctx.add_singleton_model(|_| search::command_palette::SelectedItems::new());
    ctx.add_singleton_model(search::files::model::FileSearchModel::new);
    ctx.add_singleton_model(|_| VimRegisters::new());
    ctx.add_singleton_model(UndoCloseStack::new);
    ctx.add_singleton_model(|_| ToastStack);
    #[cfg(feature = "local_fs")]
    ctx.add_singleton_model(FileModel::new);
    ctx.add_singleton_model(GlobalBufferModel::new);
    #[cfg(windows)]
    ctx.add_singleton_model(util::traffic_lights::windows::RendererState::new);

    ctx.add_singleton_model(|_| AudibleBell::new());

    // LogManager must be registered before any subsystem that creates file-based loggers.
    ctx.add_singleton_model(|_| simple_logger::manager::LogManager::new());

    ctx.add_singleton_model(|_| CodeManager::default());
    ctx.add_singleton_model(|_| OpenedFilesModel::new());
    ctx.add_singleton_model(|_| ActiveSession::default());

    #[cfg(feature = "local_tty")]
    {
        ctx.add_singleton_model(LocalShellState::new);
        ctx.add_singleton_model(system::SystemInfo::new);
    }

    // Add a singleton model that holds the current prompt configuration.
    ctx.add_singleton_model(Prompt::new);

    // Add a singleton model for resizable modals whose size should be persisted through restarts.
    ctx.add_singleton_model(|_| ResizableData::default());

    timer.mark_interval_end("SINGLETON_MODELS_REGISTERED");

    ctx.add_singleton_model(move |_| timer);

    ctx.add_singleton_model(DefaultTerminal::new);

    ctx.add_singleton_model(|_| {
        PersistedWorkspace::new(persisted_workspaces, persistence_writer.sender())
    });
    ctx.add_singleton_model(move |_| persistence_writer);

    ctx.add_singleton_model(move |_| {
        crate::suggestions::ignored_suggestions_model::IgnoredSuggestionsModel::new(
            persisted_ignored_suggestions,
        )
    });
    if launch_mode.should_start_local_http_server() {
        ctx.add_singleton_model(move |ctx| {
            let routers = vec![profiling::make_router()];
            http_server::HttpServer::new(routers, ctx)
        });
    }
    app_state
}

pub(crate) fn app_callbacks(is_integration_test: bool) -> warpui::platform::AppCallbacks {
    warpui::platform::AppCallbacks {
        on_internet_reachability_changed: Some(Box::new(move |reachable, ctx| {
            NetworkStatus::handle(ctx)
                .update(ctx, move |me, ctx| me.reachability_changed(reachable, ctx));
        })),
        on_become_active: None,
        on_screen_changed: Some(Box::new(move |ctx| {
            ctx.dispatch_global_action(
                "root_view:move_quake_mode_window_from_screen_change",
                &KeysSettings::as_ref(ctx)
                    .quake_mode_settings
                    .value()
                    .clone(),
            );

            let new_display_count = ctx.windows().display_count();
            DisplayCount::handle(ctx).update(ctx, |display_count, ctx| {
                display_count.0 = new_display_count;
                ctx.notify();
            });
        })),
        on_cpu_awakened: Some(Box::new(move |ctx| {
            SystemStats::handle(ctx).update(ctx, move |system, ctx| {
                log::info!("System has returned from sleep");
                system.dispatch_cpu_was_awakened(ctx);
            });
        })),
        on_cpu_will_sleep: Some(Box::new(move |ctx| {
            SystemStats::handle(ctx).update(ctx, move |system, ctx| {
                log::info!("System is going to sleep...");
                system.dispatch_cpu_will_sleep(ctx);
            });
        })),
        on_resigned_active: Some(Box::new(move |ctx| {
            let active_window_id = ctx.windows().active_window();
            let update_quake_mode_arg = UpdateQuakeModeEventArg { active_window_id };

            ctx.dispatch_global_action("root_view:update_quake_mode_state", &update_quake_mode_arg);
        })),
        on_will_terminate: Some(Box::new(move |ctx| {
            PersistenceWriter::handle(ctx).update(ctx, |writer, _ctx| {
                writer.terminate();
            });

            // Preserve session state before terminating the shell sessions.
            #[cfg(feature = "local_tty")]
            terminal::local_tty::spawner::PtySpawner::handle(ctx).update(ctx, |pty_spawner, _| {
                pty_spawner.prepare_for_app_termination();
            });

            #[cfg(all(feature = "local_tty", windows))]
            terminal::local_tty::shutdown_all_pty_event_loops(ctx);

            // Release application services and single-instance resources.
            app_services::teardown(ctx);

            // Tear down any application profilers that are running, writing
            // results to disk.
            profiling::teardown();

            #[cfg(enable_crash_recovery)]
            crash_recovery::CrashRecovery::handle(ctx).update(ctx, |crash_recovery, _ctx| {
                crash_recovery.teardown();
            });
        })),
        on_should_close_window: Some(Box::new(move |window_id, ctx| {
            let general_settings = GeneralSettings::as_ref(ctx);
            // On Linux or Windows, if we're about to close the final window, we should quit the app instead.
            // On Mac, we do this conditionally based on a user setting.
            let quit_on_last_window_closed =
                cfg!(any(target_os = "linux", target_os = "freebsd", windows))
                    || *general_settings.quit_on_last_window_closed;
            if ctx.window_ids().count() == 1 && quit_on_last_window_closed {
                log::info!("No windows left, terminating app");
                ctx.terminate_app(TerminationMode::Cancellable, None);
                return ApproveTerminateResult::Cancel;
            }

            let summary = UnsavedStateSummary::for_window(window_id, ctx);

            // Don't show dialog on integration test. Machine can't press buttons.
            if !is_integration_test && summary.save_unsaved_code_and_should_warn(ctx) {
                let shown = summary
                    .dialog()
                    .on_confirm(move |ctx| {
                        ctx.windows()
                            .close_window(window_id, TerminationMode::ForceTerminate);
                    })
                    .on_cancel(move |ctx| {
                        on_close_window_cancelled(window_id, false, ctx);
                    })
                    .on_show_processes(move |ctx| {
                        on_close_window_cancelled(window_id, true, ctx);
                    })
                    .show(ctx);
                if shown {
                    ApproveTerminateResult::Cancel
                } else {
                    ApproveTerminateResult::Terminate
                }
            } else {
                ApproveTerminateResult::Terminate
            }
        })),
        on_should_terminate_app: Some(Box::new(move |source, ctx| {
            // Never interrupt a system-initiated termination (logout / restart /
            // scheduled OS update): the quit-warning dialog can return
            // `ApproveTerminateResult::Cancel`, which macOS interprets as Warp
            // refusing to quit. That can abort a scheduled OS update while the
            // quit-warning modal has no visible window to attach to, leaving
            // Warp waiting on a prompt nobody can see (#12441).
            if source == TerminationRequestSource::System {
                return ApproveTerminateResult::Terminate;
            }

            let summary = UnsavedStateSummary::for_app(ctx);
            // Don't show dialog on integration test. Machine can't press buttons.
            if !is_integration_test && summary.save_unsaved_code_and_should_warn(ctx) {
                let shown = summary
                    .dialog()
                    .on_confirm(|ctx| ctx.terminate_app(TerminationMode::ForceTerminate, None))
                    .on_show_processes(|ctx| on_close_app_cancelled(true, ctx))
                    .on_cancel(|ctx| on_close_app_cancelled(false, ctx))
                    .show(ctx);
                if shown {
                    return ApproveTerminateResult::Cancel;
                }
            }

            ApproveTerminateResult::Terminate
        })),
        on_disable_warning_modal: Some(Box::new(move |ctx| {
            GeneralSettings::handle(ctx).update(ctx, |general_settings, ctx| {
                report_if_error!(
                    general_settings
                        .show_warning_before_quitting
                        .toggle_and_save_value(ctx)
                );
            });
        })),
        on_notification_clicked: Some(Box::new(move |notification_response, ctx| {
            if let Some(notification_data) = notification_response.data() {
                let context: serde_json::Result<NotificationContext> =
                    serde_json::from_str(notification_data);
                if let Ok(NotificationContext::BlockOrigin {
                    window_id,
                    pane_group_id,
                    pane_id,
                }) = context
                {
                    // Ensure the window ID exists, if so dispatch an action to focus
                    // the correct pane.
                    if ctx.window_ids().contains(&window_id)
                        && let Some(root_view_id) = ctx.root_view_id(window_id)
                    {
                        ctx.dispatch_action(
                            window_id,
                            &[root_view_id],
                            "root_view:handle_notification_click",
                            &PaneViewLocator {
                                pane_group_id,
                                pane_id,
                            },
                            log::Level::Info,
                        );
                    }
                }
            }
        })),
        on_new_window_requested: Some(Box::new(move |ctx| {
            // This one is called when the app is requested to open a new window,
            // e.g. clicking on the Dock icon. It is NOT called from the New Window
            // menu item.
            ctx.dispatch_global_action("root_view:open_new", &());
            ctx.dispatch_global_action("workspace:save_app", &());
        })),
        on_open_urls: Some(Box::new(move |urls, ctx| {
            for url in &urls {
                let parsed_url = Url::parse(url);
                match parsed_url {
                    Ok(url) => uri::handle_incoming_uri(&url, ctx),
                    Err(e) => log::warn!("Unable to parse received url: {e}"),
                }
            }
        })),
        on_os_appearance_changed: Some(Box::new(move |ctx| {
            AppearanceManager::handle(ctx).update(ctx, |appearance_manager, ctx| {
                appearance_manager.refresh_theme_state(ctx);
            });
        })),
        on_active_window_changed: Some(Box::new(move |ctx| {
            let windowing_model = ctx.windows();
            let active_window_id = windowing_model.active_window();
            let key_window_is_modal_panel = windowing_model.key_window_is_modal_panel();

            if !key_window_is_modal_panel {
                let update_quake_mode_arg = UpdateQuakeModeEventArg { active_window_id };
                ctx.dispatch_global_action(
                    "root_view:update_quake_mode_state",
                    &update_quake_mode_arg,
                );
            }

            ctx.dispatch_global_action("workspace:save_app", &());
        })),
        on_window_will_close: Some(Box::new(move |closed_window_data, ctx| {
            if ctx.windows().stage() == ApplicationStage::Terminating {
                return;
            }

            if let Some(window_data) = closed_window_data {
                UndoCloseStack::handle(ctx).update(ctx, |stack, ctx| {
                    stack.handle_window_closed(window_data, ctx);
                });
            }
            ctx.dispatch_global_action("workspace:save_app", &());
        })),
        on_window_moved: Some(Box::new(move |ctx| {
            ctx.dispatch_global_action("workspace:save_app", &());
        })),
        on_window_resized: Some(Box::new(move |ctx| {
            ctx.dispatch_global_action("workspace:save_app", &());
        })),
        ..Default::default()
    }
}

fn on_close_app_cancelled(open_navigation_palette: bool, ctx: &mut AppContext) {
    let sessions = SessionNavigationData::all_sessions(ctx).collect_vec();
    let sessions_summary = RunningSessionSummary::new(&sessions);

    // If open_navigation_palette is false, return early. Otherwise, we honor the open_navigation_palette
    // param which is true if the user clicked the modal button for that. However, if the running
    // processes in this window have finished since the modal popped, there is nothing to do now and we
    // can return early
    if !open_navigation_palette || sessions_summary.long_running_cmds.is_empty() {
        return;
    }

    let windowing_model = ctx.windows();
    let active_window_id = windowing_model.active_window();
    // show the nav palette in the active window. if there is no active window,
    // arbitrarily pick one of the windows having a running process
    let window_id_to_focus = active_window_id.unwrap_or_else(|| {
        *sessions_summary
            .windows_running()
            .iter()
            .next()
            .expect("already checked len > 0")
    });

    windowing_model.show_window_and_focus_app(window_id_to_focus);

    // open the nav palette in the selected window
    if let Some(workspaces) = ctx.views_of_type::<Workspace>(window_id_to_focus)
        && let Some(handle) = workspaces.first()
    {
        ctx.dispatch_typed_action_for_view(
            window_id_to_focus,
            handle.id(),
            &WorkspaceAction::OpenPalette {
                mode: PaletteMode::Navigation,
                source: PaletteSource::QuitModal,
                query: Some("running".to_owned()),
            },
        );
    }
}

fn on_close_window_cancelled(
    window_id: WindowId,
    open_navigation_palette: bool,
    ctx: &mut AppContext,
) {
    let sessions = SessionNavigationData::all_sessions(ctx).collect_vec();
    let sessions_summary = RunningSessionSummary::new(&sessions);
    let num_processes_in_window = sessions_summary.processes_in_window(&window_id).len();

    // If open_navigation_palette is false, return early. Otherwise, we honor the
    // open_navigation_palette param which is true if the user clicked the modal
    // button for that. However, if the running processes in this window have finished
    // since the modal popped, there is nothing to do now and we can return early
    if !open_navigation_palette || num_processes_in_window == 0 {
        return;
    }

    ctx.windows().show_window_and_focus_app(window_id);

    // if we haven't returned early, it means open_navigation_palette is true as the
    // user pressed the modal button for opening the navigation palette to show their
    // running processes
    if let Some(workspaces) = ctx.views_of_type::<Workspace>(window_id)
        && let Some(handle) = workspaces.first()
    {
        ctx.dispatch_typed_action_for_view(
            window_id,
            handle.id(),
            &WorkspaceAction::OpenPalette {
                mode: PaletteMode::Navigation,
                source: PaletteSource::QuitModal,
                query: Some("running".to_owned()),
            },
        );
    }
}

#[::tracing::instrument(skip_all)]
fn launch(ctx: &mut warpui::AppContext, app_state: Option<AppState>, launch_mode: LaunchMode) {
    IntervalTimer::handle(ctx).update(ctx, |timer, _ctx| {
        timer.mark_interval_end("APP_LAUNCHED");
    });

    keyboard::load_custom_keybindings(ctx);

    IntervalTimer::handle(ctx).update(ctx, |timer, _ctx| {
        timer.mark_interval_end("KEYBINDINGS_LOADED");
    });

    match launch_mode {
        LaunchMode::App { .. } | LaunchMode::Test { .. } => {
            // Attempt to restore windows from the persisted application state.
            let arg = OpenFromRestoredArg { app_state };
            ctx.dispatch_global_action("root_view:open_from_restored", &arg);

            // Process any URLs that were provided on the command line (which may be
            // file:// URLs or ones using our custom URL scheme).
            for url in launch_mode.args().urls.iter() {
                uri::handle_incoming_uri(url, ctx);
            }

            // If, after session restoration and command-line argument handling, we
            // haven't opened any windows, open a new window.
            if ctx.window_ids().count() == 0 {
                ctx.dispatch_global_action("root_view:open_new", &());
            }

            IntervalTimer::handle(ctx).update(ctx, |timer, _| {
                timer.mark_interval_end("WINDOWS_CREATED");
            });

            // TODO(ben): We should skip this for LaunchMode::Test.
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            {
                use crate::login_item::maybe_register_app_as_login_item;
                use crate::terminal::general_settings::GeneralSettingsChangedEvent;
                // Note that we put this here because it depends on settings already having been initialized.
                ctx.subscribe_to_model(&GeneralSettings::handle(ctx), |_, event, ctx| {
                    if matches!(event, GeneralSettingsChangedEvent::LoginItem { .. }) {
                        maybe_register_app_as_login_item(ctx);
                    }
                });
                maybe_register_app_as_login_item(ctx);
            }
        }
        // Proxy should never reach launch() — it's a thin byte bridge.
        LaunchMode::RemoteServerProxy => {
            report_error!("Proxy mode should not use the launch() path");
            std::process::exit(1);
        }
        // Daemon: bind the Unix socket and register the ServerModel.
        // initialize_app already set up everything else including crash
        // reporting.
        #[cfg(unix)]
        LaunchMode::RemoteServerDaemon { identity_key } => {
            remote_server::unix::launch_daemon(&identity_key, ctx);
        }
        #[cfg(not(unix))]
        LaunchMode::RemoteServerDaemon { .. } => {
            report_error!("RemoteServerDaemon is not supported on this platform");
            std::process::exit(1);
        }
    }
}

/// Initializes the logger before running tests.
///
/// The `ctor` attribute here means that this runs BEFORE main(), whenever the
/// binary is executed. For this reason, we need to ensure that this function
/// only exists within unit test code. Production bundles and integration tests
/// also initialize the logging system, and initializing it twice causes a panic.
///
/// Additionally, we must not write anything to stdout in this function, as it
/// can interfere with test harnesses collecting the set of tests to run. (This
/// is why we're not simply calling the init() function above.)
#[ctor::ctor]
#[cfg(test)]
fn init_logging_for_unit_tests_glue() {
    // Initialize terminal-friendly logging for tests from the shared logger crate.
    warp_logging::init_logging_for_unit_tests();
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
