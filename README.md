# NeoWarp

NeoWarp is an independent fork of [Warp](https://github.com/warpdotdev/warp), focused on native desktop
terminal sessions and local data.

## Product Scope

Open the upper-left sidebar and select **Files** to browse the active terminal's directory. Expand folders on demand
and select a file to open it beside the terminal, with basic editing and saving. On Windows, WSL
directory changes arrive through the bundled Linux filesystem watcher, including ignored folders.

- Terminal blocks, history, completion, tabs, splits, and basic text editing.
- Ordinary SSH/WSL and advanced SSH daemon support without Warp account or cloud-service dependencies.
- Git change statistics and a dedicated review panel with file lists and inline diffs.
- External Markdown and Notebook editing through VS Code or Zed, plus Typora for Markdown.
  When no application is selected, use the system file association.

The built-in AI agent, commercial workflows, cloud collaboration, full IDE/LSP, embedded document preview,
and Web/WASM product are outside NeoWarp's scope. External CLI tools run as ordinary terminal programs.

Development is ongoing. Cross-platform runtime and packaging acceptance is still in progress;
check validation results for the platform and scenario you intend to use.

## Project Shortcuts

Select **Projects** in the sidebar to group external CLI sessions by working directory. Literal
Codex, Pi, Claude Code and Hermes launches in NeoWarp are recorded automatically; **New session**
also supports other tools. A running entry focuses its terminal. After it ends, opening it resumes
that CLI's saved conversation ID or session file. Missing or ambiguous IDs require **Modify**;
NeoWarp does not substitute the latest conversation or silently start a new one.

Right-click a session and choose **Modify** to edit its name, directory, shell, environment variables
(a JSON object), existing uv virtual environment or Conda environment, startup command and resume
identity/command. Changes apply on the next launch. Only explicit command variables, selected CLI
configuration directories and Python activation state are captured; other variables come from the
new shell. Complex launch scripts can be configured manually. No conversations are imported.

Unused sessions hide after 72 hours; **Show older** reveals them without deleting records.
Right-click projects or sessions to **Pin** them, and drag their handles to reorder within the same
level, project and pin group. Unpinned entries use recent activity until manually reordered.
Launch records are stored locally in `project_sessions.json` inside NeoWarp's data directory.

## Local Data

NeoWarp uses its own application IDs, settings, history, caches, and `.neowarp*` configuration directories.
Existing NanoWarp and upstream Warp data are left in place and are not imported automatically.
For isolated development runs, set `NEOWARP_DATA_DIR` to a new absolute directory.

## Build from Source

Follow [Development Setup](CONTRIBUTING.md#development-setup) to configure prerequisites, local caches,
isolated application data, and resource limits before building. From the repository root on Windows:

```powershell
$env:GIT_RELEASE_TAG = 'v0.1.0'
cargo +1.92.0 build --locked -j 2 -p warp --bin warp-oss --features release_bundle,gui
```

The executable is `.work/target/debug/warp-oss.exe`; some internal package and binary names retain `warp`.
For macOS and Linux, see the platform runner in the contribution guide.

## Contributing and Support

Report bugs, propose features, and ask development questions through
[NeoWarp issues](https://github.com/Rycen7822/NeoWarp/issues). Include reproduction steps, version, OS,
expected behavior, and diagnostics with secrets removed.

Read [CONTRIBUTING.md](CONTRIBUTING.md) before opening a
[pull request](https://github.com/Rycen7822/NeoWarp/pulls). Follow [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md)
when participating, and use [SECURITY.md](SECURITY.md) for vulnerability reporting.

## Licensing and Attribution

NeoWarp derives from Warp. Preserve applicable upstream copyrights and third-party notices.

The `warpui_core` and `warpui` UI framework crates are licensed under the [MIT license](LICENSE-MIT).
The rest of this repository is licensed under the [AGPL v3](LICENSE-AGPL).
