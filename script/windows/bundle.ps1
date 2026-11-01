#!/usr/bin/env powershell
#
# Bundle the application for release.

Param (
    # Build an unoptimized development bundle when explicitly requested.
    [Switch]$DEBUG_BUILD = $False,

    [Alias('check-only')]
    [Switch]$CHECK_ONLY,

    [Alias('release-tag')]
    [String]$RELEASE_TAG = '',
    [String]$FEATURES = 'release_bundle,gui',

    # Builds only the NeoWarp binary, skips the installer.
    [Switch]$SKIP_BUILD_INSTALLER = $False,
    # Builds only the installer, skips the NeoWarp binary. Use this if the NeoWarp
    # binary has already been built.
    [Switch]$SKIP_BUILD_BINARY = $False,

    [ValidateSet('x64', 'arm64')]
    [String]$ARCH = '',

    # A signtool command for Inno Setup to sign the setup engine and uninstaller.
    # Uses $f as the file placeholder, e.g.:
    #   'signtool.exe sign /fd SHA256 ... $f'
    # When empty, the installer is built without signing.
    [Alias('sign-tool-cmd')]
    [String]$SIGN_TOOL_CMD = ''
)

if ($RELEASE_TAG) {
    $env:GIT_RELEASE_TAG = $RELEASE_TAG
}

# Use provided ARCH parameter if set, otherwise detect from system
if (-not $ARCH) {
    if ($env:PROCESSOR_ARCHITECTURE -eq 'AMD64') {
        $ARCH = 'x64'
    } elseif ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64') {
        $ARCH = 'arm64'
    } else {
        throw "Unsupported processor architecture: $env:PROCESSOR_ARCHITECTURE"
    }
}

if ($ARCH -eq 'arm64') {
    $FILE_ENDING = 'Setup-arm64'
    $PLATFORM_TARGET = 'aarch64-pc-windows-msvc'
} else {
    # If x64, then we just use the filename "NeoWarpSetup.exe" for example
    $FILE_ENDING = 'Setup'
    $PLATFORM_TARGET = 'x86_64-pc-windows-msvc'
}

# Windows-on-ARM64 hosts can run x64 binaries via built-in emulation, but x64
# hosts cannot run arm64 binaries at all, so only that direction is unsafe.
# Resolve it once so the settings-schema default below (which assumes the
# just-built binary is executable) can fail clearly instead of the process
# simply failing to start.
#
# PROCESSOR_ARCHITECTURE reports the architecture of the running process, not
# the host: an x64 PowerShell process running under WOW64 on a Windows-on-ARM
# machine reports AMD64 even though the host is natively ARM64.
# PROCESSOR_ARCHITEW6432 carries the true native host architecture in that
# case, so prefer it when present.
$NATIVE_PROCESSOR_ARCHITECTURE = if ($env:PROCESSOR_ARCHITEW6432) {
    $env:PROCESSOR_ARCHITEW6432
} else {
    $env:PROCESSOR_ARCHITECTURE
}
$HOST_ARCH = if ($NATIVE_PROCESSOR_ARCHITECTURE -eq 'AMD64') {
    'x64'
} elseif ($NATIVE_PROCESSOR_ARCHITECTURE -eq 'ARM64') {
    'arm64'
} else {
    throw "Unsupported host architecture: $NATIVE_PROCESSOR_ARCHITECTURE"
}
$CAN_EXECUTE_ARCH = -not ($ARCH -eq 'arm64' -and $HOST_ARCH -eq 'x64')

$ErrorActionPreference = 'Stop'
$CHANNEL = 'oss'

$WORKSPACE_ROOT_DIR = (Resolve-Path "$PSScriptRoot\..\..").Path
if ($PWD.Path -ne $WORKSPACE_ROOT_DIR) {
    throw "Run this script from $WORKSPACE_ROOT_DIR."
}
$CARGO_TARGET_DIR = if ($env:CARGO_TARGET_DIR) {
    [System.IO.Path]::GetFullPath($env:CARGO_TARGET_DIR)
} else {
    Join-Path $WORKSPACE_ROOT_DIR '.work\target'
}
$env:CARGO_TARGET_DIR = $CARGO_TARGET_DIR
$WINDOWS_INSTALLER_DIR = $WORKSPACE_ROOT_DIR + '\script\windows'

if ($DEBUG_BUILD) {
    $CARGO_PROFILE = 'dev'
} else {
    $CARGO_PROFILE = 'rlto'
}

if ($CARGO_PROFILE -eq 'dev') {
    $CARGO_TARGET_OUTPUT_DIR = "$CARGO_TARGET_DIR" + '\' + $PLATFORM_TARGET + '\debug'
} else {
    $CARGO_TARGET_OUTPUT_DIR = "$CARGO_TARGET_DIR" + '\' + $PLATFORM_TARGET + '\' + "$CARGO_PROFILE"
}

# Update parameters based on the target release channel.
#
# APP_NAME here must match the value used in Rust as the
# application name; see crates/warp_core/src/channel/state.rs.
#
# WARP_BIN is the name of the binary produced by cargo;
# BINARY_NAME is the desired name of the binary in the final package.
$WARP_BIN = 'warp-oss'
$BINARY_NAME = 'neowarp.exe'
$APP_NAME = 'NeoWarp'

$BINARY_PATH = "$CARGO_TARGET_OUTPUT_DIR\$BINARY_NAME"
$BUNDLE_ID = "dev.neowarp.$APP_NAME"
$INSTALLER_OUTPUT_DIR = Join-Path $CARGO_TARGET_OUTPUT_DIR 'installer'
$INSTALLER_NAME = "$($APP_NAME)$($FILE_ENDING)"
$INSTALLER_PATH = "$($INSTALLER_OUTPUT_DIR)\$($INSTALLER_NAME).exe"
$PDB_PATH = "$CARGO_TARGET_OUTPUT_DIR\$WARP_BIN.pdb"
$CARGO_PACKAGE = 'warp'
$INSTALLER_SCRIPT = "$WINDOWS_INSTALLER_DIR\windows-installer.iss"

# The CARGO_FULL_PROFILE environment variable is read by the `cargo` build
# script (`app/build.rs`) to determine where to place `conpty.dll`.
if ($DEBUG_BUILD) {
    $env:CARGO_FULL_PROFILE = 'debug'
} else {
    $env:CARGO_FULL_PROFILE = $CARGO_PROFILE
}

# If we only want to check that compilation will succeed, perform the checks
# then exit.  We use this script to invoke `cargo check` to ensure that we are
# using the same feature flags and profile that we would be using in production.
if ($CHECK_ONLY) {
    cargo +1.92.0 check --manifest-path "$WORKSPACE_ROOT_DIR\Cargo.toml" -p $CARGO_PACKAGE --profile "$CARGO_PROFILE" --bin "$WARP_BIN" --features "$FEATURES" --target $PLATFORM_TARGET --locked -j 2
    if (-Not $?) {
        Write-Error "Failed to verify Warp $WARP_BIN compilation with profile $CARGO_PROFILE"
        exit 1
    }
    exit 0
}

if (-Not $SKIP_BUILD_BINARY) {
    Write-Output "Building Warp for channel $CHANNEL and bundle id $BUNDLE_ID"
    $env:CARGO_BIN_NAME = $CHANNEL
    $env:WARP_APP_NAME = $APP_NAME
    cargo +1.92.0 build --manifest-path "$WORKSPACE_ROOT_DIR\Cargo.toml" -p $CARGO_PACKAGE --profile "$CARGO_PROFILE" --bin "$WARP_BIN" --features "$FEATURES" --target $PLATFORM_TARGET --locked -j 2
    if (-Not $?) {
        Write-Error "Failed to build Warp $WARP_BIN binary with profile $CARGO_PROFILE"
        exit 1
    }

    # Keep the Cargo artifact available for incremental builds while staging the package name.
    if ("$WARP_BIN.exe" -ne $BINARY_NAME) {
        $binarySource = "$CARGO_TARGET_OUTPUT_DIR\$WARP_BIN.exe"
        Write-Output "Copying executable $WARP_BIN.exe to $BINARY_NAME"
        Copy-Item -LiteralPath "$binarySource" -Destination "$BINARY_PATH" -Force
    }
}

# The Linux sidecar is a separate Cargo invocation, never a nested build.rs
# build. Stage it beside the executable for both portable and installer builds.
& "$WINDOWS_INSTALLER_DIR\prepare_wsl_git_watcher.ps1" -OutputDirectory $CARGO_TARGET_OUTPUT_DIR -Arch $ARCH

if ($SKIP_BUILD_INSTALLER) {
    # If this is being run within a GitHub action, set an output variable with the
    # location of the binary so it can be referenced by subsequent actions.
    if ($env:GITHUB_ACTIONS -eq 'true') {
        Write-Output '::echo::on'
        "target_profile_dir=$CARGO_TARGET_OUTPUT_DIR" >> "$env:GITHUB_OUTPUT"
        "binary_path=$BINARY_PATH" >> "$env:GITHUB_OUTPUT"
        "pdb_file_path=$PDB_PATH" >> "$env:GITHUB_OUTPUT"
        Write-Output '::echo::off'
    }
    exit 0
}

Write-Output "Built for $ARCH with executable at $BINARY_PATH"

# Prepare bundled resources
if ($env:SKIP_SETTINGS_SCHEMA -ne '1' -and -not $env:SETTINGS_SCHEMA_EXECUTABLE -and -not $env:SETTINGS_SCHEMA_SOURCE) {
    if ($SKIP_BUILD_BINARY) {
        Write-Error '-skip_build_binary requires SETTINGS_SCHEMA_SOURCE or SETTINGS_SCHEMA_EXECUTABLE.'
        exit 1
    } elseif (-not $CAN_EXECUTE_ARCH) {
        Write-Error "Cannot execute the just-built $ARCH binary on this $HOST_ARCH host to generate a settings schema; pass SETTINGS_SCHEMA_SOURCE or SETTINGS_SCHEMA_EXECUTABLE."
        exit 1
    }
    $env:SETTINGS_SCHEMA_EXECUTABLE = $BINARY_PATH
}
$BUNDLED_RESOURCES_DIR = "$CARGO_TARGET_OUTPUT_DIR\resources"
Write-Output 'Preparing bundled resources...'
& "$WINDOWS_INSTALLER_DIR\prepare_bundled_resources.ps1" -DestinationDir "$BUNDLED_RESOURCES_DIR"
if (-Not $?) {
    Write-Error 'Failed to prepare bundled resources'
    exit 1
}
Write-Output 'Building NeoWarp installer'
$ISCC_ARGS = @(
    "$INSTALLER_SCRIPT",
    "/O$INSTALLER_OUTPUT_DIR",
    "/DReleaseChannel=$CHANNEL",
    "/DMyAppExeName=$BINARY_NAME",
    "/DTargetProfileDir=$CARGO_TARGET_OUTPUT_DIR",
    "/DMyAppName=$APP_NAME",
    "/DArch=$ARCH",
    "/DOutputName=$INSTALLER_NAME"
)
if ($env:GIT_RELEASE_TAG) {
    $ISCC_ARGS += "/DMyAppVersion=$env:GIT_RELEASE_TAG"
}
# Also accept the sign tool command via env var
if (-not $SIGN_TOOL_CMD -and $env:SIGN_TOOL_CMD) {
    $SIGN_TOOL_CMD = $env:SIGN_TOOL_CMD
}
if ($SIGN_TOOL_CMD) {
    $ISCC_ARGS += '/DSIGN_TOOL=1'
    $ISCC_ARGS += "/Scodesign=$SIGN_TOOL_CMD"
}
& ISCC @ISCC_ARGS
if (-Not $?) {
    Write-Error "Failed to build $APP_NAME installer"
    exit 1
}

# If this is being run within a GitHub action, set an output variable with the
# location of the installer so it can be referenced by subsequent actions.
if ($env:GITHUB_ACTIONS -eq 'true') {
    Write-Output '::echo::on'
    $INSTALLER_PATH = $INSTALLER_PATH -replace '\\', '/'
    "installer_path=$INSTALLER_PATH" >> "$env:GITHUB_OUTPUT"
    "pdb_file_path=$PDB_PATH" >> "$env:GITHUB_OUTPUT"
    Write-Output '::echo::off'
}
