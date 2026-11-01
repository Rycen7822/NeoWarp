# Windows installer

`windows-installer.iss` builds the NeoWarp installer with Inno Setup 6.7 or
newer. It installs `neowarp.exe` under NeoWarp's own directory, registry and
shortcut identity. Cargo's development target remains `warp-oss`.

## Build

Run from the repository root after loading the local Rust environment:

```powershell
. .\.work\rust-env.ps1
$env:CARGO_BUILD_JOBS = '2'
$env:RUST_TEST_THREADS = '1'
rustup target add x86_64-unknown-linux-musl --toolchain 1.92.0
.\script\windows\bundle.ps1 -RELEASE_TAG 0.1.0
```

The default bundle uses the optimized `rlto` profile (release with ThinLTO).
For a development bundle with debug assertions, add `-DEBUG_BUILD`.
The `release_bundle` feature selects packaged resources; it does not enable
compiler optimizations by itself. Use the same profile for performance comparisons.

Put `ISCC.exe` on the current process's PATH. Keep tool downloads, TEMP/TMP,
Cargo output and installer output under the D-drive `.work` directory.
The [official Inno Setup notes](https://jrsoftware.org/ishelp/topic_technotes.htm)
describe portable installation of the compiler itself.

The default installer is unsigned. To use your own Authenticode signing setup,
pass `-SIGN_TOOL_CMD`; this is independent of Git commit signing.

Use `-SKIP_BUILD_BINARY` only with an existing package-named executable and
`SETTINGS_SCHEMA_SOURCE` or `SETTINGS_SCHEMA_EXECUTABLE`. Cross-compiled binaries
also require a runnable schema source; see the script's existing guards.
`-CHECK_ONLY` checks the Cargo target without building an installer.

Windows packages include `neowarp-wsl-watch`, a small static Linux executable
used only for Git invalidations in WSL repositories. The bundler cross-compiles
it using Rust's bundled linker; it does not start or install anything in WSL.
For ARM64 packages install `aarch64-unknown-linux-musl` instead. For a plain
`cargo run` build, prepare the adjacent helper first:

```powershell
.\script\windows\prepare_wsl_git_watcher.ps1 -OutputDirectory .work/target/debug
```

The app shares one helper per active distribution, communicates through pipes,
and closes it when subscriptions end. Missing or failed helpers use periodic
background Git queries; terminal input and ordinary SSH are independent.

The bundler supplies `TargetProfileDir`, `MyAppName`, `MyAppExeName`,
`MyAppVersion`, `ReleaseChannel`, `Arch` and `OutputName` to ISCC. It writes to
`.work/target/<target>/<profile>/installer/` (or the selected CARGO_TARGET_DIR).
The x64 output is `NeoWarpSetup.exe`; arm64 uses `NeoWarpSetup-arm64.exe`.

## Verification

Compile the installer and inspect its inputs before installing it. Compilation
does not verify registry writes, uninstall behavior or a running GUI.
The installer uses the NeoWarp mutex for ordinary running-app checks; it has
no background-update parameter, wait loop or image-name process kill.

Icons come from `app/channels/oss/icon/no-padding/icon.ico`; shared ConPTY,
OpenConsole, VC runtime and graphics DLL filenames remain unchanged.

The checked-in `vcruntime140.dll`, `vcruntime140_1.dll` and `msvcp140.dll`
retain Microsoft's copyright. Redistribution is subject to the applicable
Visual Studio license terms; see Microsoft's [redistribution guidance](https://learn.microsoft.com/en-us/cpp/windows/redistributing-visual-cpp-files)
and [redistributable list](https://learn.microsoft.com/en-us/visualstudio/releases/2022/redistribution).
When refreshing them, use the package architecture's release CRT files under
`VC\Redist\MSVC\<version>\<arch>\Microsoft.VC143.CRT` from a Visual Studio
installation covered by those terms. Keep the files unmodified and exclude
`debug_nonredist`.
