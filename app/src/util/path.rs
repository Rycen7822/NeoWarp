use warp_util::local_or_remote_path::LocalOrRemotePath;
pub use warp_util::path::*;
use warpui::{AppContext, SingletonEntity};

use crate::remote_server::manager::RemoteServerManager;

/// Fallback label used when a `RemotePath`'s host is not currently tracked.
/// Matches the fallback in `terminal::writeable_pty::remote_server_controller::connection_label_from_user_and_host`.
const UNKNOWN_HOST_LABEL: &str = "Remote host";

/// Formats a local I/O path without exposing the Windows WSL provider prefix.
pub fn display_local_path(path: &std::path::Path) -> String {
    match parse_wsl_unc_path(path) {
        Some(wsl) => format!("{}:{}", wsl.distro, wsl.linux_path),
        None => path.display().to_string(),
    }
}

/// Returns the display name of a local or remote path, prefixed with the
/// host label for remote paths.
pub fn display_name_with_host(path: &LocalOrRemotePath, ctx: &AppContext) -> String {
    let name = path.display_name();
    match path {
        LocalOrRemotePath::Local(_) => name.to_string(),
        LocalOrRemotePath::Remote(remote) => {
            let host_label = RemoteServerManager::as_ref(ctx)
                .host_label(&remote.host_id)
                .unwrap_or(UNKNOWN_HOST_LABEL);
            format!("{host_label}:{name}")
        }
    }
}

/// Returns the display path of a local or remote path,
/// prefixed with the host label for remote paths.
///
/// When `abbreviate_home` is true, local paths under the user's home directory
/// are abbreviated with a `~/` prefix. The flag is ignored for remote paths,
/// whose home directory lives on a different machine.
pub fn display_path_with_host(
    path: &LocalOrRemotePath,
    abbreviate_home: bool,
    ctx: &AppContext,
) -> String {
    match path {
        LocalOrRemotePath::Local(local_path) => {
            if abbreviate_home {
                dirs::home_dir()
                    .and_then(|home| local_path.strip_prefix(&home).ok())
                    .map(|relative| format!("~/{}", relative.display()))
                    .unwrap_or_else(|| display_local_path(local_path))
            } else {
                display_local_path(local_path)
            }
        }
        LocalOrRemotePath::Remote(remote) => {
            let host_label = RemoteServerManager::as_ref(ctx)
                .host_label(&remote.host_id)
                .unwrap_or(UNKNOWN_HOST_LABEL);
            format!("{host_label}:{}", path.display_path())
        }
    }
}

use typed_path::{TypedPath, TypedPathBuf, WindowsPath};
use warp_errors::report_error;
use warp_terminal::shell::ShellLaunchData;
use warpui_core::platform::OperatingSystem;

fn use_unix_paths(shell: Option<&ShellLaunchData>) -> bool {
    OperatingSystem::get().is_linux()
        || OperatingSystem::get().is_mac()
        || shell.is_some_and(|shell| {
            matches!(
                shell,
                ShellLaunchData::WSL { .. } | ShellLaunchData::MSYS2 { .. }
            )
        })
}

fn shell_native_absolute_path_internal(
    file_path: &str,
    shell: Option<&ShellLaunchData>,
    current_working_directory: &str,
) -> TypedPathBuf {
    let expanded_path = shellexpand::tilde(file_path).into_owned();

    let use_unix_paths = use_unix_paths(shell);
    let (cwd, file_path) = if use_unix_paths {
        (
            TypedPathBuf::from_unix(current_working_directory),
            TypedPath::unix(&expanded_path),
        )
    } else {
        (
            TypedPathBuf::from_windows(current_working_directory),
            TypedPath::windows(&expanded_path),
        )
    };
    cwd.join(file_path).normalize()
}

/// Returns the absolute path of the path in the host's native format.
///
/// The result uses the user's OS format, regardless of the shell format.
/// For example, WSL paths are converted to Windows host paths.
pub fn host_native_absolute_path(
    file_path: &str,
    shell: &Option<ShellLaunchData>,
    current_working_directory: &Option<String>,
) -> String {
    let Some(cwd) = current_working_directory.as_ref() else {
        return shellexpand::tilde(file_path).into_owned();
    };
    let normalized_path = shell_native_absolute_path_internal(file_path, shell.as_ref(), cwd);

    match shell {
        Some(ShellLaunchData::WSL { distro }) => {
            match convert_wsl_to_windows_host_path(&normalized_path.to_path(), distro) {
                Ok(path) => path.to_string_lossy().into_owned(),
                Err(err) => {
                    report_error!(
                        anyhow::Error::new(err).context("Could not convert WSL to Windows host path"),
                        extra: { "path" => ?normalized_path }
                    );
                    normalized_path.to_string_lossy().into_owned()
                }
            }
        }
        Some(ShellLaunchData::MSYS2 {
            executable_path, ..
        }) => {
            match convert_msys2_to_windows_native_path(
                &normalized_path.to_path(),
                &msys2_exe_to_root(WindowsPath::new(
                    executable_path.as_os_str().as_encoded_bytes(),
                )),
            ) {
                Ok(path) => path.to_string_lossy().into_owned(),
                Err(err) => {
                    report_error!(
                        anyhow::Error::new(err)
                            .context("Could not convert MSYS2 to Windows host path"),
                        extra: { "path" => ?normalized_path }
                    );
                    normalized_path.to_string_lossy().into_owned()
                }
            }
        }
        _ => normalized_path.to_string_lossy().into_owned(),
    }
}

#[cfg(test)]
#[path = "path_tests.rs"]
mod tests;
