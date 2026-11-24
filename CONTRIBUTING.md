# Contributing to NeoWarp

Keep contributions within the [product scope](README.md#product-scope). Search
[existing issues](https://github.com/Rycen7822/NeoWarp/issues) before proposing work, and discuss substantial
behavior or architecture changes before implementing them.

## Issues and pull requests

Bug reports should include the NeoWarp version, OS, reproduction steps, expected and actual behavior, and relevant
diagnostics with secrets removed. Feature requests should explain the user problem and constraints.

Branch from `main` and keep each PR to one logical change. Use the [PR template](.github/pull_request_template.md).
Explain the problem and resulting behavior, link an issue when applicable, and record actual validation and anything
unverified. Include screenshots or a short recording when they demonstrate a visible change.

Use concise commit subjects such as `fix: preserve terminal selection` or `docs: clarify source builds`. Explain
non-obvious reasons in the commit body. Small fixes do not need a separate specification PR. For larger changes,
agree on a short design and acceptance criteria first.

## Development setup

Run development and build commands from the repository root. Use the Rust version pinned in
[rust-toolchain.toml](rust-toolchain.toml), currently `1.92.0`. Keep `Cargo.lock` unchanged unless a dependency
change requires it.

Native prerequisites include Git with Git LFS, Rust and rustup, CMake, Protobuf, and libclang. Windows also needs
the MSVC C++ build tools and Windows SDK. macOS needs Xcode. The platform scripts list additional dependencies:

- [Windows bootstrap](script/windows/bootstrap.ps1)
- [macOS bootstrap](script/macos/bootstrap)
- [Linux bootstrap](script/linux/bootstrap)

Bootstrap installs or updates system tools. It is not a routine build step. Read the relevant script before running
it. An already configured machine does not need to run it again. For a new checkout, fetch tracked assets with
`git lfs install --local` and `git lfs pull`.

### Local directories and resource limits

Keep temporary scripts, reports, runtime data, and reusable build caches under the ignored `.work/` directory.
On Windows, use a checkout on D: and configure these paths before downloading a toolchain or building:

```powershell
$work = Join-Path (Get-Location).Path '.work'
$env:CARGO_HOME = Join-Path $work 'cargo-home'
$env:RUSTUP_HOME = Join-Path $work 'rustup'
$env:CARGO_TARGET_DIR = Join-Path $work 'target'
$env:TEMP = Join-Path $work 'tmp'
$env:TMP = $env:TEMP
$env:SCCACHE_DIR = Join-Path $work 'sccache'
$env:NEOWARP_DATA_DIR = Join-Path $work 'runtime/dev'
$env:CARGO_BUILD_JOBS = '2'
$env:RUST_TEST_THREADS = '1'
$env:RAYON_NUM_THREADS = '2'
$directories = $env:CARGO_HOME, $env:RUSTUP_HOME, $env:CARGO_TARGET_DIR,
    $env:TEMP, $env:SCCACHE_DIR, $env:NEOWARP_DATA_DIR
New-Item -ItemType Directory -Force -Path $directories | Out-Null
```

In a macOS or Linux shell, set the equivalent environment before setup or builds:

```bash
export CARGO_HOME="$PWD/.work/cargo-home" RUSTUP_HOME="$PWD/.work/rustup"
export CARGO_TARGET_DIR="$PWD/.work/target" TMPDIR="$PWD/.work/tmp"
export SCCACHE_DIR="$PWD/.work/sccache" NEOWARP_DATA_DIR="$PWD/.work/runtime/dev"
export CARGO_BUILD_JOBS=2 RUST_TEST_THREADS=1 RAYON_NUM_THREADS=2
export GIT_RELEASE_TAG=v0.2.0
mkdir -p "$CARGO_HOME" "$RUSTUP_HOME" "$CARGO_TARGET_DIR" "$TMPDIR" "$SCCACHE_DIR" "$NEOWARP_DATA_DIR"
```

These environment blocks apply only to the current shell. Run the appropriate block in each new build shell.
Keep the Rust launchers on `PATH`. After selecting the configured homes, install the pinned toolchain with
`rustup toolchain install 1.92.0`. Download dependencies before using offline builds with a fresh cache.
Use fresh NeoWarp application data. Do not import an existing Warp profile during development or testing.

### Build and run

The Windows GUI build uses the existing binary name `warp-oss`.

Set `GIT_RELEASE_TAG` to the product version before building. This compile-time value supplies the application's
version string. It does not select a source revision or require a Git tag with the same name. The examples below use
`v0.2.0`. Replace that value when building another version.

```powershell
$env:GIT_RELEASE_TAG = 'v0.2.0'
cargo +1.92.0 build --locked -j 2 -p warp --bin warp-oss --features release_bundle,gui
& .\.work\target\debug\warp-oss.exe
```

On macOS and Linux, [`./script/run`](script/run) selects the native runner. macOS uses an app bundle. Linux runs
the `warp-oss` GUI process. Use the environment configured above. Report platform build success and runtime
acceptance separately.

Use `--locked` for Cargo commands. Add `--offline` only when the required dependencies are already cached.
Run one Cargo command at a time. The examples use two build jobs and one test thread. Do not exceed four build
jobs or four test threads.

## Testing and style

Read the affected source and existing tests first. Run the smallest existing checks that cover the change.
Add a focused test only when changed behavior is not already covered. Do not introduce a new framework or expand
unrelated coverage.

For an editor change, an example of an existing package test command is:

```text
cargo +1.92.0 test --locked -j 2 -p warp_editor --lib -- --test-threads=1
```

Follow nearby Rust style and [.rustfmt.toml](.rustfmt.toml). Keep code readable and lines within 150 characters.
Format only affected files while developing. [`script/format --check`](script/format) checks Rust formatting.
[`script/presubmit`](script/presubmit) runs broader formatting, lint, and test checks.
[CI](.github/workflows/ci.yml) defines platform checks and change filtering. Do not weaken gates to pass a change.

Prefer scripts and commands for verification. Use desktop interaction only when it resolves a remaining UI evidence
gap. Document commands, results, and untested platforms accurately. Documentation-only changes do not need unrelated
Rust builds or runtime tests.

## Development assistants and historical specs

External coding assistants are optional contributor tools. The checked-in [skills](.agents/skills/) describe
development workflows. [`specs/`](specs/) contains historical designs that may describe removed upstream behavior.
Verify instructions against current source and [product scope](README.md#product-scope).

Contributors remain responsible for reviewing generated changes and reporting actual validation results.
Use NeoWarp issues and pull requests to coordinate review with this repository's maintainers.

## Conduct, security, and help

Follow [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md). Read [SECURITY.md](SECURITY.md) before reporting a vulnerability.
Do not publish sensitive details in an issue or PR.

Use [NeoWarp issues](https://github.com/Rycen7822/NeoWarp/issues) for ordinary bugs, feature discussions, and
development questions.
