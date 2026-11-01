# Build the small Linux sidecar without a Linux compiler or starting WSL.
param (
    [Parameter(Mandatory = $true)]
    [string]$OutputDirectory,
    [ValidateSet('x64', 'arm64')]
    [string]$Arch = 'x64'
)

$ErrorActionPreference = 'Stop'
$workspaceRoot = (Resolve-Path "$PSScriptRoot\..\..").Path
if ($PWD.Path -ne $workspaceRoot) { throw "Run this script from $workspaceRoot." }
$target = if ($Arch -eq 'arm64') { 'aarch64-unknown-linux-musl' } else { 'x86_64-unknown-linux-musl' }
$linkerVariable = 'CARGO_TARGET_' + $target.Replace('-', '_').ToUpperInvariant() + '_LINKER'
$previousLinker = [Environment]::GetEnvironmentVariable($linkerVariable)
try {
    # rust-lld and the musl CRT are distributed with the Rust toolchain/target.
    $sysroot = rustc +1.92.0 --print sysroot
    $hostTriple = (rustc +1.92.0 -vV | Select-String '^host: ').ToString().Substring(6)
    [Environment]::SetEnvironmentVariable($linkerVariable, "$sysroot\lib\rustlib\$hostTriple\bin\rust-lld.exe")
    cargo +1.92.0 build --locked -p wsl_git_watcher --bin neowarp-wsl-watch --target $target --profile wsl-helper
    if ($LASTEXITCODE -ne 0) { throw 'WSL Git watcher build failed. Install the matching Rust musl target first.' }
    $targetDirectory = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { "$workspaceRoot\.work\target" }
    New-Item -ItemType Directory -Path $OutputDirectory -Force | Out-Null
    Copy-Item -LiteralPath "$targetDirectory\$target\wsl-helper\neowarp-wsl-watch" -Destination $OutputDirectory -Force
} finally {
    [Environment]::SetEnvironmentVariable($linkerVariable, $previousLinker)
}
