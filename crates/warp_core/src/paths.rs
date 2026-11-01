//! Helper functions for retrieving base paths for storing config/data files.
//!
//! This file should not be directly exposed to or used in integration tests;
//! any paths computed using these functions should be exposed to integration
//! tests through use-case-specific helper functions.
//!
//! `_local_dir` variants of functions are for storing non-portable data, where
//! "portable" refers to the ability to copy that file to another machine.
//! Some examples of non-portable data include things that reference local
//! paths (which may not exist on a different machine), such as paths to shell
//! binaries or user-added theme files.
//!
//! TODO(vorporeal): In general, we should be returning Option<PathBuf> or
//! Result<PathBuf> when we can't compute the home directory instead of
//! returning a relative path.

use std::path::{Path, PathBuf};

use cfg_if::cfg_if;
use directories::BaseDirs;

use crate::AppId;
use crate::channel::{Channel, ChannelState};

/// The name of the directory in which to put non-global Warp-specific files.
///
/// This should be used, for example, as the base directory under which
/// repository configuration would be stored (in "./.neowarp").
pub const WARP_CONFIG_DIR: &str = ".neowarp";

/// The name of the folder that stores Warp execution logs and network logs.
/// This is currently only used on Windows to maintain backwards compatibility.
pub const WARP_LOGS_DIR: &str = "logs";

/// Overrides application-owned runtime directories without changing the user's shell home.
pub fn runtime_root() -> Option<PathBuf> {
    let root = PathBuf::from(std::env::var_os("NEOWARP_DATA_DIR")?);
    assert!(
        root.is_absolute(),
        "NEOWARP_DATA_DIR must be an absolute path"
    );
    Some(root)
}

fn base_warp_config_dir_name() -> String {
    match ChannelState::channel() {
        Channel::Oss => format!("{WARP_CONFIG_DIR}-oss"),
        Channel::Integration => format!("{WARP_CONFIG_DIR}-integration"),
    }
}

/// Returns the home-relative Warp config directory name for the current channel and data profile.
///
/// Channels and development profiles use separate `.neowarp*` directories.
pub fn warp_home_config_dir_name() -> String {
    let base_dir_name = base_warp_config_dir_name();

    if let Some(data_profile) = ChannelState::data_profile() {
        format!("{base_dir_name}-{data_profile}")
    } else {
        base_dir_name
    }
}

/// Returns the home-relative Warp config directory for the current channel and data profile.
///
/// Unlike [`data_dir`] and [`config_local_dir`] on non-macOS platforms, this intentionally keeps
/// user-facing config under a `.neowarp*` directory in the home directory instead of
/// using the platform XDG/AppData project directories.
pub fn warp_home_config_dir() -> Option<PathBuf> {
    if let Some(root) = runtime_root() {
        return Some(root.join("config"));
    }
    dirs::home_dir().map(|home_dir| home_dir.join(warp_home_config_dir_name()))
}

/// Returns the macOS config directory name for the current channel and data
/// profile.
///
/// OSS and integration retain their existing channel-specific directory suffixes.
///
/// Development data profiles append a further `-{profile}` suffix. Without it,
/// every profile of a channel would share this directory — and with it the
/// public settings in `settings.toml` — defeating the isolation that profiles
/// already provide for UserDefaults, Application Support, and the keychain.
///
/// These suffixes are persisted on disk as directory names.
#[cfg(target_os = "macos")]
fn macos_config_dir_name() -> String {
    macos_config_dir_name_for(
        ChannelState::channel(),
        ChannelState::data_profile().as_deref(),
    )
}

#[cfg(target_os = "macos")]
fn macos_config_dir_name_for(channel: Channel, data_profile: Option<&str>) -> String {
    let base_dir_name = match channel {
        Channel::Oss => format!("{WARP_CONFIG_DIR}-oss"),
        Channel::Integration => format!("{WARP_CONFIG_DIR}-integration"),
    };
    match data_profile {
        Some(profile) => format!("{base_dir_name}-{profile}"),
        None => base_dir_name,
    }
}

/// Returns the path to the directory where portable user data should be
/// stored.
///
/// This is the appropriate home for things like custom themes and workflows.
pub fn data_dir() -> PathBuf {
    if let Some(root) = runtime_root() {
        return root.join("data");
    }
    cfg_if! {
        if #[cfg(target_os = "macos")] {
            // TODO(vorporeal): We should do something better than return a
            // relative path.
            dirs::home_dir().unwrap_or_default().join(macos_config_dir_name())
        } else {
            project_dirs().map(|dirs| dirs.data_dir().to_owned()).unwrap_or_default()
        }
    }
}

/// Returns the path to the directory where non-portable configuration files
/// should be stored.
pub fn config_local_dir() -> PathBuf {
    if let Some(root) = runtime_root() {
        return root.join("config");
    }
    cfg_if! {
        if #[cfg(target_os = "macos")] {
            // TODO(vorporeal): We should do something better than return a
            // relative path.
            dirs::home_dir().unwrap_or_default().join(macos_config_dir_name())
        } else {
            project_dirs()
                .map(|dirs| dirs.config_local_dir().to_owned())
                .unwrap_or_default()
        }
    }
}

/// Returns the base directory for general config files. Useful for accessing the config files for
/// other programs.
pub fn base_config_dir() -> PathBuf {
    BaseDirs::new()
        .map(|dirs| dirs.config_dir().to_owned())
        .unwrap_or_default()
}

/// Returns the path to the directory where non-portable application state data
/// should be stored.
///
/// This is the appropriate home for files like our sqlite database, which
/// contains durable but non-critical and non-portable data such as window,
/// tab, and terminal state.
pub fn state_dir() -> PathBuf {
    if let Some(root) = runtime_root() {
        return root.join("state");
    }
    let Some(project_dirs) = project_dirs() else {
        return PathBuf::new();
    };
    // For platforms that don't have a notion of a "state" directory (e.g.:
    // macOS and Windows), fall back to using the data directory.
    project_dirs
        .state_dir()
        .unwrap_or_else(|| project_dirs.data_local_dir())
        .to_owned()
}

/// Returns the path to the directory containing the user's custom themes.
pub fn themes_dir() -> PathBuf {
    data_dir().join("themes")
}

/// Returns the path to the directory where files can be stored for caching
/// purposes.
///
/// This is a good place to store things like user profile pictures, which
/// we don't want to fetch on every launch of the app but can be safely
/// deleted by the OS.
pub fn cache_dir() -> PathBuf {
    if let Some(root) = runtime_root() {
        return root.join("cache");
    }
    let Some(project_dirs) = project_dirs() else {
        return PathBuf::new();
    };
    cfg_if! {
        if #[cfg(target_os = "macos")] {
            // TODO(vorporeal): Given that this is just cache data; do we want
            // change the path we use on macOS?
            project_dirs.data_dir().to_owned()
        } else {
            project_dirs.cache_dir().to_owned()
        }
    }
}

/// Returns a display-ready version of the path that is formatted in a
/// home-dir-relative manner, if appropriate.
pub fn home_relative_path(path: &Path) -> String {
    #[cfg(unix)]
    if let Some(base_dirs) = directories::BaseDirs::new()
        && let Ok(relative_path) = path.strip_prefix(base_dirs.home_dir())
    {
        return format!("~/{}", relative_path.display());
    };

    path.display().to_string()
}

/// Returns a [`directories::ProjectDirs`] configured based on the current app ID
/// and the current data profile, if one is set.
///
/// This returns [`None`] if the user's home directory could not be determined.
fn project_dirs() -> Option<directories::ProjectDirs> {
    project_dirs_for_app_id(
        ChannelState::app_id(),
        ChannelState::data_profile().as_deref(),
    )
}

/// Returns a [`directories::ProjectDirs`] configured based on the given app ID
/// and data profile.
///
/// This returns [`None`] if the user's home directory could not be determined.
fn project_dirs_for_app_id(
    app_id: AppId,
    data_profile: Option<&str>,
) -> Option<directories::ProjectDirs> {
    let base_app_name = app_id.application_name().to_owned();
    let app_name = if let Some(data_profile) = data_profile {
        format!("{base_app_name}-{data_profile}")
    } else {
        base_app_name
    };
    directories::ProjectDirs::from(app_id.qualifier(), app_id.organization(), &app_name)
}

/// Returns the path to resources included in the Warp distribution.
///
/// Unlike [`warpui_core::AssetProvider`] assets, which are generally embedded in the binary, these are
/// stored on the filesystem alongside the rest of Warp.
///
/// ## macOS
/// For the `.app` bundle, the resources directory is `$APP_DIR/Contents/Resources`
/// (e.g. `/Applications/Warp.app/Contents/Resources`). For the standalone CLI build
/// (compiled with the `standalone` feature) the binary is not inside a `.app` bundle,
/// and its resources live in a sibling `resources` directory next to the binary
/// (e.g. `$INSTALL_DIR/resources`), matching the Linux/Windows layout.
///
/// ## Linux
/// The resources directory is `$INSTALL_DIR/resources`, where `$INSTALL_DIR` depends on the
/// specific package manager. For example, on Ubuntu this might be `/opt/warpdotdev/warp-terminal/resources`.
///
/// ## Windows
/// The resources directory is `$INSTALL_DIR/resources`, where `$INSTALL_DIR` is the directory
/// containing the Warp executable (e.g. `C:\Program Files\WarpDev\resources`).
pub fn bundled_resources_dir() -> Option<PathBuf> {
    cfg_if::cfg_if! {
        if #[cfg(target_os = "macos")] {
            if cfg!(feature = "standalone") {
                // Standalone CLI build: the binary is not inside a `.app` bundle, so
                // its resources live in a sibling `resources` directory next to the
                // binary (matching the Linux/Windows layout).
                std::env::current_exe()
                    .ok()
                    .and_then(|executable| std::fs::canonicalize(executable).ok())
                    .and_then(|executable| executable.parent().map(|parent| parent.join("resources")))
            } else {
                // Regular `.app` bundle: resources live in `Contents/Resources`.
                crate::macos::get_bundle_path()
                    .ok()
                    .map(|bundle_path| PathBuf::from(bundle_path).join("Contents").join("Resources"))
            }
        } else if #[cfg(any(target_os = "linux", target_os = "freebsd"))] {
            std::env::current_exe()
                .ok()
                .and_then(|executable| std::fs::canonicalize(executable).ok())
                .and_then(|executable| executable.parent().map(|parent| parent.join("resources")))
        } else if #[cfg(target_os = "windows")] {
            std::env::current_exe()
                .ok()
                .and_then(|executable| std::fs::canonicalize(executable).ok())
                .and_then(|executable| executable.parent().map(|parent| parent.join("resources")))
        } else {
            None
        }
    }
}

#[cfg(all(test, feature = "local_fs"))]
#[path = "paths_tests.rs"]
mod tests;
