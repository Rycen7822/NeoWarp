use warp_core::settings::SupportedPlatforms;
use warp_core::settings::macros::define_settings_group;
use warpui::{AppContext, SingletonEntity};

use crate::banner::BannerState;

/// The default mode for new terminal sessions.
#[derive(
    Default,
    Debug,
    serde::Serialize,
    serde::Deserialize,
    PartialEq,
    Copy,
    Clone,
    schemars::JsonSchema,
    settings_value::SettingsValue,
)]
#[schemars(
    description = "Default mode for new sessions.",
    rename_all = "snake_case"
)]
pub enum DefaultSessionMode {
    /// New sessions start in the terminal mode (default).
    #[default]
    Terminal,
    /// New sessions open a user-defined tab config.
    /// The specific config is identified by the companion `default_tab_config_path` setting.
    TabConfig,
}

settings::macros::implement_setting_for_enum!(
    DefaultSessionMode,
    GeneralSettings,
    SupportedPlatforms::ALL,
    private: false,
    toml_path: "general.default_session_mode",
    description: "The default mode for new terminal sessions.",
);

impl DefaultSessionMode {
    /// Display name for the settings dropdown.
    pub fn display_name(&self) -> &'static str {
        match self {
            DefaultSessionMode::Terminal => "Terminal",
            DefaultSessionMode::TabConfig => "Tab Config",
        }
    }
}

define_settings_group!(GeneralSettings, settings: [
    accessibility_plugin: AccessibilityPlugin {
        type: bool,
        default: false,
        supported_platforms: SupportedPlatforms::WINDOWS,
        private: false,
        toml_path: "plugins.accessibility_windows.enabled",
        description: "Allow Windows accessibility tools to read and operate Projects. Restart NeoWarp to apply.",
    },
    default_session_mode: DefaultSessionMode,
    // Tab config paths are local to this machine.
    default_tab_config_path: DefaultTabConfigPath {
        type: String,
        default: String::new(),
        supported_platforms: SupportedPlatforms::ALL,
        private: false,
        toml_path: "general.default_tab_config_path",
    },
    show_warning_before_quitting: ShowWarningBeforeQuitting {
        type: bool,
        default: true,
        supported_platforms: SupportedPlatforms::DESKTOP,
        private: false,
        toml_path: "general.show_warning_before_quitting",
        description: "Whether to show a warning dialog before quitting Warp.",
    },
    quit_on_last_window_closed: QuitOnLastWindowClosed {
        type: bool,
        default: false,
        supported_platforms: SupportedPlatforms::MAC,
        private: false,
        toml_path: "general.quit_on_last_window_closed",
        description: "Whether to quit Warp when the last window is closed.",
    },
    restore_session: RestoreSession {
        type: bool,
        default: true,
        supported_platforms: SupportedPlatforms::DESKTOP,
        private: false,
        toml_path: "general.restore_session",
        description: "Whether to restore the previous session when Warp starts up.",
    },
    add_app_as_login_item: LoginItem {
        type: bool,
        default: true,
        supported_platforms: SupportedPlatforms::OR(
            Box::new(SupportedPlatforms::MAC),
            Box::new(SupportedPlatforms::WINDOWS),
        ),
        private: false,
        toml_path: "general.login_item",
        description: "Whether to launch Warp automatically when you log in.",
    },
    // Records whether the app has been added as a login item.
    // If it has, we don't try to add it again unless the user explicitly
    // retoggles the setting. This is to allow a user to remove the login item
    // directly from their OS's startup UI and not have it re-added when they
    // next start Warp.
    app_added_as_login_item: AppAddedAsLoginItem {
        type: bool,
        default: false,
        supported_platforms: SupportedPlatforms::OR(
            Box::new(SupportedPlatforms::MAC),
            Box::new(SupportedPlatforms::WINDOWS),
        ),
        private: true,
    },
    link_tooltip: LinkTooltip {
        type: bool,
        default: true,
        supported_platforms: SupportedPlatforms::ALL,
        private: false,
        toml_path: "general.link_tooltip",
        description: "Whether to show a tooltip when hovering over links.",
    },
    user_default_shell_unsupported_banner_state: UserDefaultShellUnsupportedBannerState {
        type: BannerState,
        default: BannerState::default(),
        supported_platforms: SupportedPlatforms::ALL,
        private: true,
    },
    open_in_warp_banner_dismissed_for_code_and_text: OpenInWarpBannerDismissedCode {
        type: bool,
        default: false,
        supported_platforms: SupportedPlatforms::ALL,
        private: true,
    },
]);

impl GeneralSettings {
    /// Looks up the `TabConfig` matching the stored `default_tab_config_path`.
    /// Returns `None` if the path is empty or no loaded config matches.
    pub fn resolved_default_tab_config(
        &self,
        app: &AppContext,
    ) -> Option<crate::tab_configs::TabConfig> {
        let path_str = self.default_tab_config_path.as_str();
        if path_str.is_empty() {
            return None;
        }
        let path = std::path::Path::new(path_str);
        crate::user_config::WarpConfig::as_ref(app)
            .tab_configs()
            .iter()
            .find(|config| config.source_path.as_deref().is_some_and(|p| p == path))
            .cloned()
    }
}
