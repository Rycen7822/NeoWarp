---
name: review-pr-local
description: Review NeoWarp changes against local Rust, GUI, logging and validation conventions.
---

# NeoWarp review guidance

## Repo-specific style and recurring review patterns

- Review directly affected tests against the accepted behavior and root test-necessity limits. Consult `rust-unit-tests` or `gui-integration-test` only for the relevant test level; do not demand new tests or load both guides for every PR. Report a missing check when it leaves a concrete regression risk uncovered.
- When a PR is clearly a V0 or initial implementation, frame robustness suggestions such as timeouts, retries, and lifecycle management as optional future work rather than blocking concerns, unless they risk correctness, security, data loss, or a persistent UI hang.
- For Rust changes, apply the repository conventions from `AGENTS.md`: avoid unnecessary type annotations, prefer imports over long path qualifiers, name context parameters `ctx` and place them last, remove unused parameters instead of prefixing them with `_`, and prefer inline format arguments in macros.
- Keep comments focused on non-obvious reasons and API contracts. Do not restate code, enumerate callers or describe the edit history; preserve useful unrelated comments.
- Review changed local diagnostics against `.agents/skills/logging-and-error-reporting/SKILL.md`: log levels, hot-path volume, typed error classification, duplicate reports and sensitive data at every level.
- Avoid wildcard `_` match arms when an enum can reasonably be matched exhaustively; exhaustive matches are preferred so future variants are surfaced during review.
- For new or changed feature flags, prefer high-level runtime checks with `FeatureFlag::YourFlag.is_enabled()` over `#[cfg(...)]` unless the code cannot compile without a compile-time gate.
- Flag nested or redundant `TerminalModel` locking when the call stack may already hold the model lock. Prefer passing locked references down the stack and keeping lock scopes short.
- In WarpUI code, flag inline `MouseStateHandle::default()` usage during render or event handling. Mouse state handles should be created during construction and then cloned/referenced where needed.
- For user-facing UI changes, mention missing validation only when it is tied to a concrete risk or when the PR changes behavior that should be verified visually.

## Changed error diagnostics

For changed `report_error!` / `report_if_error!` calls, inspect the actual error
and its source chain. Check that typed errors retain classification, repeated
failures do not flood logs, and messages or `extra:` fields contain no secrets.
These macros write local logs; review them without assuming an upload service.

## Behavioral or UI-impacting changes and visual evidence

- If the PR changes anything user-visible (UI components, layout, styling, copy in surfaces users see, terminal/Warp app visuals, or other behavior a user can perceive), analyze both `pr_description.txt` and any PR comments available in the workflow context for attached screenshots, GIFs, or videos demonstrating the change end to end.
  - Treat markdown image/video embeds (`![...](...)`, `<img ...>`, `<video ...>`), GitHub user-attachment links (e.g. `https://github.com/user-attachments/...`, `https://user-images.githubusercontent.com/...`), Loom links, and similar hosted media as valid evidence.
  - The `Screenshots / Videos` section from `.github/pull_request_template.md` being present but empty does not count as evidence.
  - Unit tests, integration tests, `git diff --check`, code-path descriptions, and other textual explanations may supplement visual evidence but do not replace it when visual proof is required.
- If the change is behavioral or UI-impacting and no screenshots or videos are attached in the description or comments, you may add an inline or summary-level comment requesting them. Use wording such as: "For this user-facing change, please include screenshots or a screen recording demonstrating it working end to end."
- Missing visual evidence is blocking only when the request, brief, or spec explicitly required visual proof. In that case, when the change can be manually tested, state that the explicitly requested evidence is still missing, even if no code defect was found. Otherwise, missing visual evidence is not a finding — ordinary tests and checks are sufficient.
- If visual proof was explicitly required, an unavailable desktop remains an evidence gap. Record the exact limitation and obtain evidence from the current NeoWarp build; an upstream Warp recording does not verify this fork.
- Exempt visual evidence only when the user-visible behavior truly cannot be meaningfully shown visually (for example, changes affecting only screen readers or non-visual side effects). If so, briefly state why screenshots or recordings would not be meaningful. When visual proof is required, never exempt based on limitations of the author's environment.
- If the PR is not user-visible at all (e.g. pure refactor, internal tools, build scripts, backend-only code, tests, or documentation), do not request screenshots or videos.

## User-facing strings

- Flag interpolated text that would read unnaturally at runtime or combine sentence fragments with the wrong casing.
- Link text should be descriptive rather than bare URLs or generic "click here" labels.
- Verify that product terminology is consistent across related UI, comments, workflow messages, and errors in the same PR.

## Graceful degradation and observability

- When optional dynamic data such as URLs, session links, workflow links, issue numbers, or metadata may be absent, prefer omitting the element or showing a short fallback over rendering empty or broken output.
- Preserve useful, non-sensitive local diagnostics when changing error paths.
- Prefer generic, user-safe error text in user-visible surfaces, but keep enough structured logging or diagnostic context for maintainers to investigate failures.
