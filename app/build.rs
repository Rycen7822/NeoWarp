// We can use `std::process:Command` here because this is invoked within a build script,
// _not_ within the Warp binary (where it could cause a terminal to temporarily flash on
// Windows).
#![allow(clippy::disallowed_types)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs};

use anyhow::Result;
use cfg_aliases::cfg_aliases;
use warp_util::assets::{
    ASSETS_DIR, CONPTY_DLL_FILE, DXCOMPILER_DLL_FILE, DXIL_DLL_FILE, OPEN_CONSOLE_EXE_FILE,
    WINDOWS_ASSETS_DIR,
};

fn main() -> Result<()> {
    cfg_aliases! {
        linux_or_windows: { any(target_os = "linux", windows) },
        enable_crash_recovery: { linux_or_windows },
    }

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=CARGO_CFG_TARGET_OS");

    let target_os = env::var("CARGO_CFG_TARGET_OS")?;

    add_features(&target_os);

    if target_os == "macos" {
        println!("cargo:rustc-link-lib=framework=MetalKit");
        println!("cargo:rustc-link-lib=framework=UserNotifications");

        println!("cargo:rerun-if-changed=src/platform/mac/objc/services.h");
        println!("cargo:rerun-if-changed=src/platform/mac/objc/services.m");

        cc::Build::new()
            .file("src/platform/mac/objc/services.m")
            .compile("warp_objc");

        // Build the dock tile plugin
        println!("cargo:rerun-if-changed=DockTilePlugin/WarpDockTilePlugin.m");
        println!("cargo:rerun-if-changed=DockTilePlugin/WarpDockTilePlugin.h");
        println!("cargo:rerun-if-changed=DockTilePlugin/Info.plist");
        println!("cargo:rerun-if-changed=DockTilePlugin/Makefile");

        let min_macos_version = env::var("MACOSX_DEPLOYMENT_TARGET")
            .expect("MACOSX_DEPLOYMENT_TARGET must be set for macos builds");
        let target_dir = app_target_dir();
        let status = Command::new("make")
            .current_dir("DockTilePlugin")
            .arg(format!("OUTPUT_DIR={}", target_dir.display()))
            .env("MACOSX_DEPLOYMENT_TARGET", min_macos_version)
            .status()
            .expect("Failed to build dock tile plugin");
        if !status.success() {
            panic!("Dock tile plugin build failed");
        }
    }

    if target_os == "windows" {
        // These values change copied assets and embedded version metadata without changing sources.
        println!("cargo:rerun-if-env-changed=CARGO_FULL_PROFILE");
        println!("cargo:rerun-if-env-changed=CARGO_BIN_NAME");
        println!("cargo:rerun-if-env-changed=GIT_RELEASE_TAG");
        println!("cargo:rerun-if-env-changed=WARP_APP_NAME");
        let target_dir = app_target_dir();
        copy_windows_assets(&target_dir);

        #[cfg(windows)]
        embed_resource_file(&target_dir);
    }

    Ok(())
}

fn app_target_dir() -> PathBuf {
    // OUT_DIR retains the actual target root, target triple and custom profile selected by Cargo.
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR must be set"));
    out_dir
        .ancestors()
        .nth(3)
        .expect("OUT_DIR must be inside a Cargo profile directory")
        .to_path_buf()
}

fn add_features(target_os: &str) {
    println!("cargo:rustc-cfg=feature=\"local_fs\"");
    println!("cargo:rustc-cfg=feature=\"local_tty\"");

    if target_os != "windows" {
        println!("cargo:rustc-cfg=feature=\"iterm_images\"");
    }
}

/// Copies the DLLs needed to run Warp on Windows.
///
/// They are organized as follows:
/// - `conpty.dll`
/// - `{platform}/OpenConsole.exe` (ex: `x64/OpenConsole.exe`)
/// - `dxcompiler.dll` (ex: `dxcompiler.dll`)
/// - `dxil.dll` (ex: `dxil.dll`)
fn copy_windows_assets(target_dir: &Path) {
    println!("cargo:rerun-if-changed=assets/windows");

    let target_arch = match std::env::var("CARGO_CFG_TARGET_ARCH")
        .expect("Target arhcitecture expected")
        .as_str()
    {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        _ => {
            panic!("Unsupported architecture");
        }
    };

    // This directory is architecture-specific.
    let windows_asset_dir = Path::new(ASSETS_DIR)
        .join(WINDOWS_ASSETS_DIR)
        .join(target_arch);

    let new_platform_dir = target_dir.join(target_arch);
    fs::create_dir_all(&new_platform_dir).expect("Could not create new platform directory");

    for asset in [
        CONPTY_DLL_FILE,
        DXCOMPILER_DLL_FILE,
        DXIL_DLL_FILE,
        OPEN_CONSOLE_EXE_FILE,
    ] {
        let source = windows_asset_dir.join(asset);
        let destination = if asset == OPEN_CONSOLE_EXE_FILE {
            new_platform_dir.join(asset)
        } else {
            target_dir.join(asset)
        };

        // Loaded Windows assets cannot be overwritten, and identical bytes need no update.
        if let (Ok(source_bytes), Ok(existing_bytes)) = (fs::read(&source), fs::read(&destination))
            && source_bytes == existing_bytes
        {
            continue;
        }

        fs::copy(&source, &destination).unwrap_or_else(|err| {
            panic!("Could not copy {asset} from {windows_asset_dir:?} to {destination:?}: {err:#}")
        });
    }
}

#[cfg(windows)]
fn embed_resource_file(target_dir: &Path) {
    use std::io::Write;

    let version = env::var("GIT_RELEASE_TAG").unwrap_or("v0".to_owned());
    let app_name = env::var("WARP_APP_NAME").unwrap_or("NeoWarp".to_owned());
    let bin_name = env::var("CARGO_BIN_NAME").unwrap_or("oss".to_owned());

    let icon_path = Path::new("channels")
        .join(bin_name)
        .join("icon")
        .join("no-padding")
        .join("icon.ico");

    println!("cargo:rerun-if-changed={}", icon_path.display());
    fs::copy(icon_path, target_dir.join("icon.ico"))
        .unwrap_or_else(|err| panic!("Could not copy icon: {err:#}"));

    let resource_file_path = target_dir.join("resource.rc");
    let mut rcfile = fs::File::create(&resource_file_path).unwrap();
    write!(
        rcfile,
        r#"
#pragma code_page(65001)
#include <winres.h>
#define IDI_ICON 0x101

IDI_ICON ICON "icon.ico"
VS_VERSION_INFO VERSIONINFO
FILEVERSION     1,0,0,0
PRODUCTVERSION  1,0,0,0
FILEFLAGSMASK   VS_FFI_FILEFLAGSMASK
FILEFLAGS       0
FILEOS          VOS__WINDOWS32
FILETYPE        VFT_APP
FILESUBTYPE     VFT2_UNKNOWN
BEGIN
    BLOCK "StringFileInfo"
    BEGIN
        BLOCK "040904E4"
        BEGIN
            VALUE "CompanyName",      "NeoWarp contributors\0"
            VALUE "FileDescription",  "{app_name}\0"
            VALUE "FileVersion",      "{version}\0"
            VALUE "LegalCopyright",   "© 2025, Denver Technologies, Inc\0"
            VALUE "InternalName",     "\0"
            VALUE "OriginalFilename", "\0"
            VALUE "ProductName",      "NeoWarp\0"
            VALUE "ProductVersion",   "{version}\0"
        END
    END
    BLOCK "VarFileInfo"
    BEGIN
        VALUE "Translation", 0x409, 1252
    END
END
"#,
    )
    .unwrap();
    drop(rcfile);

    // Obtain MSVC environment so that the rc compiler can find the right headers.
    // https://github.com/nabijaczleweli/rust-embed-resource/issues/11#issuecomment-603655972
    let target = env::var("TARGET").unwrap();
    if let Some(tool) = cc::windows_registry::find_tool(target.as_str(), "cl.exe") {
        for (key, value) in tool.env() {
            unsafe {
                env::set_var(key, value);
            }
        }
    }
    embed_resource::compile(resource_file_path, embed_resource::NONE)
        .manifest_required()
        .unwrap_or_else(|err| panic!("Unable to embed resource file: {err:#}"));
}
