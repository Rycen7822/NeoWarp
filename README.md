# NeoWarp

NeoWarp is an independent fork of [Warp](https://github.com/warpdotdev/warp) for native desktop terminal
sessions and local data.

## Install on Windows

The [latest NeoWarp release](https://github.com/Rycen7822/NeoWarp/releases/latest) provides an unsigned
Windows x64 installer. Download `NeoWarpSetup.exe` from that release, then run it to install NeoWarp.
For macOS and Linux, [build from source](#build-from-source).

## Product scope

Select **Files** in the upper-left sidebar to browse the active terminal's directory. Expand folders on demand.
Select a file to open it beside the terminal, then edit and save text in the basic editor. On Windows, the bundled
Linux filesystem watcher reports WSL directory changes, including changes in ignored folders.

Right-click a file to open it in the right pane, in the system default app, or in an installed editor such as
VS Code or Zed. Use the menu to create files and folders, rename items, copy paths, and refresh the listing.
Deletion requires confirmation and permanently removes the selected item and its contents.

Click a terminal file link to open text, including Markdown source, in the right pane. Ctrl+click, or Cmd+click
on macOS, opens the system default app. Relative paths and `file://` links resolve against the source session's
working directory and WSL distribution. NeoWarp opens folders and binary files with the system app.

- Use terminal blocks, history, completion, tabs, splits, and basic text editing.
- Run standard SSH and WSL sessions without a Warp account or cloud-service dependencies. Advanced SSH daemon
  support is also available.
- Inspect Git change statistics and a dedicated review panel with file lists and inline diffs.
  Use the searchable comparison selector for uncommitted changes or changes against a branch's merge base.
  Counts and file cards follow the selected comparison without checking out a branch or fetching remotes.
- Use English or Simplified Chinese. Choose **Appearance → Interface language** in settings to apply the change
  immediately. Separate **Interface font** and size controls update menus, settings, and sidebars immediately.
  Terminal font settings remain independent, and missing glyphs use system fallback fonts.
- Use Windows 11 Snap Layouts by hovering over the maximize or restore button.
- Edit Markdown and Notebook files externally through VS Code or Zed. Markdown also supports Typora.
  When no application is selected, NeoWarp uses the system file association.

The built-in AI agent, commercial workflows, cloud collaboration, full IDE and LSP support, embedded document
preview, and the web and WASM product are outside NeoWarp's scope. External CLI tools run as ordinary terminal
programs.

Development is ongoing. Cross-platform runtime and packaging acceptance is still in progress. Check the release
notes and [CI results](https://github.com/Rycen7822/NeoWarp/actions) for your platform and scenario.

## Project shortcuts

Select **Projects** in the sidebar to group external CLI sessions by working directory. NeoWarp records direct
Codex, Pi, Claude Code, and Hermes launches after confirming their session identities. Resume pickers do not
create placeholder entries. **New session** also supports other tools.

Select a running entry to focus its terminal. After the command exits, select the entry to resume the CLI's saved
conversation ID or session file. Missing or ambiguous IDs require **Modify**. NeoWarp does not substitute the
latest conversation or silently start a new one.

Right-click a session and choose **Modify** to edit its name, directory, shell, environment variables, Python
environment, startup command, and resume settings. Enter environment variables as a JSON object. Changes apply
on the next launch. NeoWarp captures explicit command variables, selected CLI configuration directories, and
Python activation state. Other variables come from the new shell. You can configure complex launch scripts
manually. NeoWarp does not import conversations.

NeoWarp hides an unpinned session 72 hours after its last recorded use unless its command is still running.
Launching or focusing a session updates its recorded use time. Pinned and running sessions remain visible. Projects
without visible sessions also hide unless they are pinned. Select the history button in **Projects** to show all
projects and sessions, including entries hidden by age. Select it again to show recent sessions only. These actions
do not delete records. Right-click projects or sessions to **Pin** them. Drag their handles to reorder entries within
the same level, project, and pin group. Unpinned entries use recent activity until you reorder them manually.
NeoWarp stores launch records locally in `project_sessions.json` inside its data directory.

While a recognized Codex, Pi, Claude Code, or Hermes command runs, a local toolbelt provides **Files**,
**Rich Input** (`Ctrl+G`), the working directory, and live Git statistics linked to **Review**.
Rich Input sends text directly to the CLI. `Enter` submits, `Shift+Enter` inserts a newline, and `Escape` hides
the editor. Drafts survive toggling within that command. The toolbelt closes when the command exits.

Right-click the toolbelt to copy prompt details or choose **Edit CLI agent toolbelt**. Drag local information
chips and controls between the available bank and the left or right zones. **Save changes** applies the layout
immediately. **Cancel** discards edits. **Restore default** restores the original arrangement before saving.

Review's **Captured tool changes** view shows successful edits, patches, and structured write diffs available in
Codex, Pi, and Claude Code session records. The previous turn stays visible with a label until the current turn
has its first captured change. Scripts, external edits, and writes without a recorded diff may be absent.
Counts accumulate individual tool operations. NeoWarp sends input immediately. This view does not scan or
snapshot the repository or install global hooks.

## Local data

NeoWarp uses its own application IDs, settings, history, caches, and `.neowarp*` configuration directories.
Existing NanoWarp and upstream Warp data are left in place and are not imported automatically.
For isolated development runs, set `NEOWARP_DATA_DIR` to a new absolute directory.

## Build from source

Follow [Development setup](CONTRIBUTING.md#development-setup) to configure prerequisites, local caches, isolated
application data, and resource limits. From the repository root on Windows:

```powershell
$env:GIT_RELEASE_TAG = 'v0.2.0'
cargo +1.92.0 build --locked -j 2 -p warp --bin warp-oss --features release_bundle,gui
```

Replace the example release tag with the version you are building. The executable is
`.work/target/debug/warp-oss.exe`. Some internal package and binary names retain `warp`.
For macOS and Linux, use the [platform runner](CONTRIBUTING.md#build-and-run).

## Contributing and support

Report bugs, propose features, and ask development questions through
[NeoWarp issues](https://github.com/Rycen7822/NeoWarp/issues). Include reproduction steps, version, OS,
expected behavior, and diagnostics with secrets removed.

Read [CONTRIBUTING.md](CONTRIBUTING.md) before opening a
[pull request](https://github.com/Rycen7822/NeoWarp/pulls). Follow [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md)
when participating, and use [SECURITY.md](SECURITY.md) for vulnerability reporting.

## Licensing and attribution

NeoWarp derives from Warp. Preserve applicable upstream copyrights and third-party notices.

The `warpui_core` and `warpui` UI framework crates are licensed under the [MIT license](LICENSE-MIT).
The rest of this repository is licensed under the [AGPL v3](LICENSE-AGPL).
