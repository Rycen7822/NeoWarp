# Contributing to NanoWarp

Keep contributions within the [product scope](README.md#product-scope). Search
[existing issues](https://github.com/Rycen7822/NanoWarp/issues) before proposing work, and discuss substantial
behavior or architecture changes before implementing them.

## Issues and Pull Requests

Bug reports should include the NanoWarp version, OS, reproduction steps, expected and actual behavior, and relevant diagnostics with secrets removed. Feature requests should explain the user problem and constraints.

Branch from `main` and keep each PR to one logical change. Use the [PR template](.github/pull_request_template.md): explain the problem and resulting behavior, link an issue when applicable, and record actual validation and anything unverified. Screenshots or a short recording are useful when they demonstrate a visible change.

Use concise, descriptive commit subjects such as `fix: preserve terminal selection` or `docs: clarify source builds`. Explain non-obvious reasons in the commit body. Small fixes do not need a separate specification PR; for larger changes, agree on a short design and acceptance criteria first.

## Development Setup

Run development and build commands from the repository root. Use the Rust version pinned in [rust-toolchain.toml](rust-toolchain.toml), currently `1.92.0`, and keep `Cargo.lock` unchanged unless a dependency change requires it.

Native prerequisites include Git with Git LFS, Rust/rustup, CMake, Protobuf, and libclang. Windows also needs the MSVC C++ build tools and Windows SDK; macOS needs Xcode. The platform scripts list additional dependencies:

- [Windows bootstrap](script/windows/bootstrap.ps1)
- [macOS bootstrap](script/macos/bootstrap)
- [Linux bootstrap](script/linux/bootstrap)

Bootstrap installs or updates system tools and is not a routine build step. Read the relevant script before running it; an already configured machine does not need to rerun it. Fetch tracked assets with `git lfs install --local` and `git lfs pull` when setting up a new checkout.

### Local Directories and Resource Limits

Keep temporary scripts, reports, runtime data, and reusable build caches under the ignored `.work/` directory. On Windows, use a checkout on D: and configure these paths before downloading a toolchain or building:

```powershell
$work = Join-Path (Get-Location).Path '.work'
$env:CARGO_HOME = Join-Path $work 'cargo-home'
$env:RUSTUP_HOME = Join-Path $work 'rustup'
$env:CARGO_TARGET_DIR = Join-Path $work 'target'
$env:TEMP = Join-Path $work 'tmp'
$env:TMP = $env:TEMP
$env:SCCACHE_DIR = Join-Path $work 'sccache'
$env:NANOWARP_DATA_DIR = Join-Path $work 'runtime/dev'
$env:CARGO_BUILD_JOBS = '2'
$env:RUST_TEST_THREADS = '1'
$env:RAYON_NUM_THREADS = '2'
$directories = $env:CARGO_HOME, $env:RUSTUP_HOME, $env:CARGO_TARGET_DIR,
    $env:TEMP, $env:SCCACHE_DIR, $env:NANOWARP_DATA_DIR
New-Item -ItemType Directory -Force -Path $directories | Out-Null
```

Keep the Rust launchers on `PATH` and install the pinned toolchain into the configured Rust home.
A fresh local cache needs dependencies downloaded before offline builds can work.
Use fresh NanoWarp application data; do not import an existing Warp profile during development or testing.

In a macOS/Linux shell, set the equivalent environment before setup or builds:

```bash
export CARGO_HOME="$PWD/.work/cargo-home" RUSTUP_HOME="$PWD/.work/rustup"
export CARGO_TARGET_DIR="$PWD/.work/target" TMPDIR="$PWD/.work/tmp"
export SCCACHE_DIR="$PWD/.work/sccache" NANOWARP_DATA_DIR="$PWD/.work/runtime/dev"
export CARGO_BUILD_JOBS=2 RUST_TEST_THREADS=1 RAYON_NUM_THREADS=2
export GIT_RELEASE_TAG=v0.1.0
mkdir -p "$CARGO_HOME" "$RUSTUP_HOME" "$CARGO_TARGET_DIR" "$TMPDIR" "$SCCACHE_DIR" "$NANOWARP_DATA_DIR"
```

### Build and Run

The Windows GUI build uses the existing binary name `warp-oss`:

Set `GIT_RELEASE_TAG` to the product version before building (`v0.1.0` for the current checkout). This compile-time value supplies the application's version string.

```powershell
$env:GIT_RELEASE_TAG = 'v0.1.0'
cargo +1.92.0 build --locked -j 2 -p warp --bin warp-oss --features release_bundle,gui
& .\.work\target\debug\warp-oss.exe
```

For macOS/Linux, [`./script/run`](script/run) selects the native runner: a macOS app bundle or a Linux `warp-oss` GUI process. Run it in the configured environment above. Platform build success and actual runtime acceptance must be reported separately.

Use `--locked` for Cargo commands, and add `--offline` only when the required dependencies are already cached.
Run one Cargo command at a time. The examples use two build jobs and one test thread;
do not exceed four build jobs or four test threads.

## Testing and Style

Read the affected source and existing tests first. Run the smallest existing checks that cover the change. Add a focused test only when changed behavior is not already covered; do not introduce a new framework or expand unrelated coverage.

For an editor change, an example of an existing package test command is:

```text
cargo +1.92.0 test --locked -j 2 -p warp_editor --lib -- --test-threads=1
```

Follow nearby Rust style and [.rustfmt.toml](.rustfmt.toml). Keep code readable and lines within 150 characters.
Format only affected files while developing. [`script/format --check`](script/format) checks Rust formatting;
[`script/presubmit`](script/presubmit) runs broader formatting, lint, and test checks.
[CI](.github/workflows/ci.yml) defines platform checks and change filtering; do not weaken gates to pass a change.

Prefer scripts and commands for verification. Use desktop interaction only when it resolves a remaining UI evidence gap. Document commands, results, and untested platforms accurately. Documentation-only changes do not need unrelated Rust builds or runtime tests.

## Development Assistants and Historical Specs

External coding assistants are optional contributor tools. The checked-in [skills](.agents/skills/)
describe development workflows. [`specs/`](specs/) contains historical designs that may describe removed
upstream behavior; verify instructions against current source and [product scope](README.md#product-scope).

Contributors remain responsible for reviewing generated changes and reporting actual validation results.
Use NanoWarp issues and pull requests to coordinate review with this repository's maintainers.

## Conduct, Security, and Help

Follow [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md). Read [SECURITY.md](SECURITY.md) before reporting a vulnerability; do not publish sensitive details in an issue or PR.

Use [NanoWarp issues](https://github.com/Rycen7822/NanoWarp/issues) for ordinary bugs, feature discussions, and development questions.
