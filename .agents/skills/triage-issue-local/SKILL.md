---
name: triage-issue-local
description: Triage NeoWarp issue reports against current supported behavior and repository labels, using evidence and focused follow-up questions.
---

# Repo-specific triage guidance for NeoWarp

## Apply this guide

This guide is self-contained. Read the issue and available evidence, check the
current source, then propose the issue type, area, priority, reproduction state
and next action using the taxonomy below. Explain uncertainty and ask only for
information that changes the decision. Do not install an upstream parent skill.
Posting, labeling and closing follow the current task's authorization.

## Heuristics

- NeoWarp is a local desktop terminal with basic file editing and external document opening. Treat public issue reports as potentially incomplete and avoid asking for secrets, tokens, private workspace names, private repository names, or account identifiers in the public issue thread.
- Distinguish the user's observed behavior from their guesses about Rust modules, UI components, server behavior, feature flags, or product intent.
- For issue reports that mention another terminal, editor, shell, or CLI tool, identify whether the problem is NeoWarp-specific or generally reproducible outside NeoWarp before assigning ownership.
- When the issue includes screenshots, videos, logs, stack traces, or command output, use them as primary evidence and ask follow-up questions only for missing details that cannot be inferred from that evidence.
- Before asking any follow-up questions, check README.md, CONTRIBUTING.md, and the current source to determine whether the reported behavior is supported. Historical specs may describe removed upstream features.
- AI, accounts, billing, cloud services, internal Notebook previews, LSP, and the browser product have been removed. Explain the current scope for reports about those features; do not request account details or route reports to Warp's commercial support.
- Classify issues into at least one primary agent type (`agent:bug`, `agent:feature`, `agent:security`, `agent:documentation`). These are development-triage labels, not product agent features. Explicitly bias towards tagging regressions ("broke in recent version", "worked in previous build"), panics/crashes, and security reports with `agent:priority-high`.

## Follow-up question limit

Ask **at most 2 follow-up questions** per triage response. Each question must be high-value: it should meaningfully change the label assignment, owner routing, or reproduction confidence if answered. Do not ask questions whose answers can be inferred from existing evidence, and do not bundle multiple sub-questions into a single bullet. If more than 2 unknowns exist, prioritize the two that are most likely to unblock triage.

## Label taxonomy

The label taxonomy for this repository is managed in `.github/issue-triage/config.json`. Prefer labels from that configuration, especially the `area:*`, `os:*`, `repro:*`, `accessibility`, `needs-info`, `duplicate`, `agent:priority-high`, and primary agent issue-type labels (`agent:bug`, `agent:feature`, `agent:security`, `agent:documentation`). Do not invent new labels unless the prompt explicitly allows it.

### Primary issue types

Every issue should be classified with at least one primary agent type label:
- `agent:bug`: Bugs/regressions.
- `agent:feature`: Feature requests.
- `agent:security`: Security issues/vulnerabilities; follow SECURITY.md and avoid public sensitive details.
- `agent:documentation`: Missing or incorrect documentation.

### Priority

- `agent:priority-high`: Tag issues when there is evidence of a regression ("broke in recent version", "worked in previous build"), panic or crash, data loss, or a security vulnerability.

Evaluate `ready-to-implement` during triage instead of relying on issue-template defaults. For bug reports, apply `ready-to-implement` only when the issue is reproducible from the provided evidence or straightforward local verification and the likely fix appears narrow enough to implement without a product spec, design mocks, or substantial investigation. If the bug is not reproducible, lacks a clear fix path, requires product/design decisions, or needs deeper technical discovery, omit `ready-to-implement` and prefer `needs-info`, `ready-to-spec`, `needs-mocks`, or the appropriate `repro:*` label.

Use area labels based on the user's reported surface:

- `area:shell-terminal` for terminal output, block rendering, shell integration, prompt rendering, command execution display, and terminal-emulation behavior.
- `area:terminal-input` for command-line input editing, cursor movement, key handling, and typed text behavior.
- `area:window-tabs-panes` for window, tab, pane, split, layout, and focus behavior.
- `area:editor-notebooks` for basic file editing and external Notebook/Markdown opening. The existing label name does not imply an internal Notebook viewer or LSP.
- `area:code-review` for local Git diff and review views.
- `area:launch-configs` for local tab configurations.
- `area:settings-keybindings` for settings UI, preferences, keyboard shortcuts, and keybinding configuration.
- `area:performance:*` when the report includes CPU, memory, GPU, startup, rendering, latency, or responsiveness symptoms. Add the more specific CPU, memory, or GPU label when the evidence points to that resource.

## Information to check for before asking follow-up questions

Before asking the reporter for more information, check the issue body, comments, attachments, logs, labels, and repository context for:

- NeoWarp version/build number and whether it was built from source or installed from a package.
- OS and version, architecture, display setup, window manager or desktop environment on Linux, and whether the issue is platform-specific.
- Shell and terminal context: shell name/version, prompt framework, shell integration status, command being run, terminal mode, local vs SSH/remote/tmux, and whether the behavior reproduces in a fresh session.
- Clear reproduction steps, expected behavior, actual behavior, frequency, regression timing, and whether the user can reproduce outside NeoWarp.
- Visual evidence for UI, rendering, layout, font, cursor, focus, window, pane, tab, and accessibility issues. Prefer a screenshot or short recording when the symptom is visual.
- Logs and diagnostics for crashes, hangs, startup failures, shell integration, SSH, and external document opening. Ask for redacted logs only when the report lacks actionable evidence.
- For performance reports: approximate project/session size, command output size, CPU/memory/GPU observations, profile or diagnostics if provided, and whether the issue appears after long-running sessions.
- For keyboard or input reports: keyboard layout, custom keybindings, IME usage, conflicting OS shortcuts, focused surface, and whether the same keys work in other apps.

## Recurring follow-up patterns

- Visual UI/rendering issue with no media: ask for a screenshot or short screen recording first.
- Environment-sensitive terminal issue: ask for NeoWarp version, OS/version, shell, and whether it reproduces in a fresh local session.
- SSH/tmux/remote issue: ask for local OS, remote OS, shell, whether tmux is involved, and the minimal command or workflow that reproduces it.
- Performance issue: ask for approximate scale, how long NeoWarp has been running, what action triggers the spike or hang, and whether logs or a profile are available.

## Owner-inference hints

Use current source history and repository participation as evidence for likely owners. Do not route work to upstream Warp teams or assume an original author maintains this fork. If ownership is unclear, leave it unspecified rather than inventing an assignment.
