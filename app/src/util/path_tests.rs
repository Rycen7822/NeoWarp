#[cfg(windows)]
use std::path::PathBuf;

#[cfg(windows)]
use warp_terminal::shell::{ShellLaunchData, ShellType};

use super::*;

#[cfg(all(windows, feature = "local_fs"))]
#[test]
fn clipboard_file_path_uses_the_linux_absolute_path_for_wsl() {
    let path = LocalOrRemotePath::Local(PathBuf::from(
        r"\\WSL$\Ubuntu-24.04-F\home\xu\project\harness\metis-pi\\.work\performance\pi-quiet-resident\IMPLEMENTATION-PLAN.md",
    ));
    assert_eq!(
        clipboard_file_path(&path),
        "/home/xu/project/harness/metis-pi/.work/performance/pi-quiet-resident/IMPLEMENTATION-PLAN.md",
    );
    let native = LocalOrRemotePath::Local(PathBuf::from(r"D:\repo\file.md"));
    assert_eq!(clipboard_file_path(&native), r"D:\repo\file.md");
}

#[cfg(unix)]
#[test]
fn test_host_native_absolute_path() {
    // Test with absolute path
    assert_eq!(
        host_native_absolute_path(
            "/home/user/file.txt",
            &None,
            &Some("/current/dir".to_string())
        ),
        "/home/user/file.txt"
    );

    // Test with relative path
    assert_eq!(
        host_native_absolute_path("file.txt", &None, &Some("/current/dir".to_string())),
        "/current/dir/file.txt"
    );

    // Test with tilde expansion
    assert_eq!(
        host_native_absolute_path("~/file.txt", &None, &Some("/current/dir".to_string())),
        shellexpand::tilde("~/file.txt").into_owned()
    );

    // Test with ..
    assert_eq!(
        host_native_absolute_path("../user/file.txt", &None, &Some("/current/dir".to_string())),
        "/current/user/file.txt"
    );

    // Test with .
    assert_eq!(
        host_native_absolute_path("./user/file.txt", &None, &Some("/current/dir".to_string())),
        "/current/dir/user/file.txt"
    );

    // Test with no current working directory
    assert_eq!(
        host_native_absolute_path("file.txt", &None, &None),
        "file.txt"
    );

    // Test with empty current working directory
    assert_eq!(
        host_native_absolute_path("file.txt", &None, &Some("".to_string())),
        "file.txt"
    );
}

#[cfg(windows)]
#[test]
fn test_host_native_absolute_path() {
    // Test with absolute path
    assert_eq!(
        host_native_absolute_path(
            r"C:\home\user\file.txt",
            &None,
            &Some(r"C:\current\dir".to_string())
        ),
        r"C:\home\user\file.txt"
    );

    // Test with relative path
    assert_eq!(
        host_native_absolute_path("file.txt", &None, &Some(r"C:\current\dir".to_string())),
        r"C:\current\dir\file.txt"
    );

    // Test with tilde expansion
    assert_eq!(
        host_native_absolute_path(r"~\file.txt", &None, &Some(r"C:\current\dir".to_string())),
        shellexpand::tilde(r"~\file.txt").into_owned()
    );

    // Test with ..
    assert_eq!(
        host_native_absolute_path(
            r"..\user\file.txt",
            &None,
            &Some(r"C:\current\dir".to_string())
        ),
        r"C:\current\user\file.txt"
    );

    // Test with .
    assert_eq!(
        host_native_absolute_path(
            r".\user\file.txt",
            &None,
            &Some(r"C:\current\dir".to_string())
        ),
        r"C:\current\dir\user\file.txt"
    );

    // Test with no current working directory
    assert_eq!(
        host_native_absolute_path("file.txt", &None, &None),
        "file.txt"
    );

    // Test with empty current working directory
    assert_eq!(
        host_native_absolute_path("file.txt", &None, &Some("".to_string())),
        "file.txt"
    );
}

#[cfg(windows)]
#[test]
fn test_git_bash_paths() {
    let executable_path = PathBuf::from(r"C:\Program Files\Git\usr\bin\bash.exe");
    let git_bash_shell = Some(ShellLaunchData::MSYS2 {
        executable_path,
        shell_type: ShellType::Bash,
    });

    assert_eq!(
        host_native_absolute_path(
            "/c/Users/username/project/file.txt",
            &git_bash_shell,
            &Some("/c/Users/username".to_string())
        ),
        r"c:\Users\username\project\file.txt"
    );

    assert_eq!(
        host_native_absolute_path(
            "project/file.txt",
            &git_bash_shell,
            &Some("/c/Users/username".to_string())
        ),
        r"c:\Users\username\project\file.txt"
    );

    assert_eq!(
        host_native_absolute_path(
            "../project/file.txt",
            &git_bash_shell,
            &Some("/c/Users/username/docs".to_string())
        ),
        r"c:\Users\username\project\file.txt"
    );
}

#[cfg(windows)]
#[test]
fn test_wsl_paths() {
    let wsl_shell = Some(ShellLaunchData::WSL {
        distro: "Ubuntu".to_string(),
    });

    assert_eq!(
        host_native_absolute_path(
            "/mnt/c/Users/username/project/file.txt",
            &wsl_shell,
            &Some("/mnt/c/Users/username".to_string())
        ),
        r"c:\Users\username\project\file.txt"
    );

    assert_eq!(
        host_native_absolute_path(
            "project/file.txt",
            &wsl_shell,
            &Some("/mnt/c/Users/username".to_string())
        ),
        r"c:\Users\username\project\file.txt"
    );

    assert_eq!(
        host_native_absolute_path(
            "../project/file.txt",
            &wsl_shell,
            &Some("/mnt/c/Users/username/docs".to_string())
        ),
        r"c:\Users\username\project\file.txt"
    );

    assert_eq!(
        host_native_absolute_path(
            "/home/user/file.txt",
            &wsl_shell,
            &Some("/mnt/c/Users/username".to_string())
        ),
        r"\\WSL$\Ubuntu\home\user\file.txt"
    );
}
