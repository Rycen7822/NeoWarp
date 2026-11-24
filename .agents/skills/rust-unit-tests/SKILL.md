---
name: rust-unit-tests
description: Run and maintain focused Rust unit tests for accepted NeoWarp behavior, using existing helpers and the repository build environment.
---

# Rust unit tests in NeoWarp

## Scope
- This skill focuses on crate-level unit tests.
- Favor incremental, well-scoped tests that exercise a single function or behavior per case.
- For *how* to structure and run tests, read on. For *whether* a change needs a test and at what level, start with "What to unit test".

## Test necessity

Start with related existing tests and the root instructions. Add a test only
for an uncovered behavior changed by this task or an explicit test request;
keep its scope small. Temporary probes belong in `.work/`. The guidance below
chooses the test level, not additional product scope or a coverage target.

## What to unit test

Default to a unit test when the logic is deterministic and reachable without booting the app:

- Parsing, encoding, escaping, and pure data transformations.
- Domain logic and state machines such as block lifecycle, selection, ranges, and diffs.
- Boundary cases such as empty input, range ends, invalid UTF-8, and wide grapheme clusters.
- Error paths and fallbacks alongside successful paths.
- Bug fixes that are reachable at this level. Reuse existing protection first. Add a reproducing case only when it
  covers an accepted behavior and an actual gap.

Rendering artifacts, PTY timing races, and crashes that require a real window need a higher-level test. Do not
reshape production code only to force these defects into a unit test.

## When a unit test is the wrong level

NeoWarp has a GPU renderer, PTY and shell integration, and IPC. Defects at those boundaries can disappear when a
unit test replaces them with mocks. A fixed unit-test coverage ratio does not account for that risk.

Use a higher-level test when:

- The behavior needs a real PTY, shell, window, or display.
- The risk is component wiring, such as applying a setting, dispatching a key binding, or moving focus.
- Reproduction requires real I/O, spawned processes, or cross-process timing.
- The only unit-test seam would be a trait or indirection layer added solely for the test.
- The test stubs so much behavior that it no longer exercises the real path.

Use `gui-integration-test` for GUI behavior, terminal and shell integration, settings, and key binding wiring.
Use `gui-integration-test-video` when a test must capture visual artifacts. For interactive desktop inspection,
follow the root instructions and use the project-local `agent-desktop` workflow only when it is available and
permitted.

First check whether an existing unit or integration test exposes the problem. Extract a pure helper only when the
extraction improves the implementation independently of testing.

## When not to write a unit test

Tests have a maintenance cost. Avoid these cases:

- **Change-detector tests.** A test that restates internal calls fails on equivalent refactors without catching a
  contract defect. Assert the observable state instead.
- **Trivial code without logic.** Getters, `From` and `Into` passthroughs, `Default` implementations, and plain
  struct construction rarely have independent behavior to test.
- **Third-party implementations.** Test NeoWarp's use of the standard library, `tokio`, and `wgpu`, rather than
  testing those projects.
- **Redundant cases.** A second test of the same behavior adds maintenance without adding evidence.

Coverage shows that code executed. It does not show that the test checked the result.

## Where unit tests live
- Put unit tests in separate files named `${filename}_tests.rs` or `mod_tests.rs`.
- Include the test module at the end of the corresponding source file:

```rust
#[cfg(test)]
#[path = "filename_tests.rs"] // or "mod_tests.rs"
mod tests;
```

## Writing good tests

Tests should change when the behavior contract changes, not when an equivalent implementation changes. Update an
existing expectation when the new contract requires it. If an otherwise equivalent refactor breaks a test, check
whether the test depends on an internal detail.

### Test behavior through the public API

Exercise the unit as its callers do. Private-state assertions can fail on refactors that callers cannot observe.
When a helper exists only to serve one or two callers, prefer testing it through those callers.

### Assert on state, not on interactions

Assert what the system *is* after the action, not which functions it called to get there.

```rust
// Brittle: still passes if the entry is dropped right after insertion, and
// fails on an equivalent refactor that calls a different internal method.
assert!(recorder.saw_call_to_insert(id));

// Better: asserts the outcome the caller actually cares about.
store.insert(id, entry.clone());
assert_eq!(store.get(id), Some(&entry));
```

### One behavior per test, named after that behavior

The test name is often the only thing visible in a failure report, so make it a sentence about behavior rather than about the method:

```rust
#[test]
fn parses_utf8_sequence_when_valid() { /* ... */ }

#[test]
fn returns_replacement_char_for_invalid_utf8() { /* ... */ }
```

If the name joins independent outcomes with "and", split them into separate tests. Separate the arrangement,
action, and assertion with blank lines.

### Keep the test complete and concise

Keep the inputs and setup needed to understand the result visible in the test. Remove unrelated setup. Prefer a
builder that accepts only the relevant fields over one large shared fixture. Set every asserted input in the test
instead of inheriting it from shared setup.

### Prefer duplication over indirection

Test code must be correct on inspection. Some repetition can make a test easier to read from top to bottom.
Extract a helper when it removes noise and preserves the inputs relevant to each case.

### Keep expected values independent

Do not compute an expected value with the algorithm under test. Use literal expected values or an independent
reference. Table-driven tests and loops are appropriate when each case keeps concrete inputs and expected outputs.

### Make failures diagnosable

- Prefer `assert_eq!` and `assert_ne!` over `assert!` when the compared values explain the failure.
- Add a message when the values lack context. For example,
  `assert_eq!(got, want, "cursor should clamp to line end for {input:?}")`.
- Use `#[should_panic]` only when a panic is the intended API. Pin the message with `expected = "..."`.

### Repo-specific

- Minimize global state. Use existing dependency boundaries rather than adding traits solely for tests.
- Use exhaustive matches when adding an enum variant. Add cases for the behaviors that the new variant affects.
- Avoid acquiring multiple `model.lock()` guards in one call stack. Pass an existing locked reference when the API
  supports it.

## Test doubles: prefer real code, then fakes

Work down this list and stop at the first option that is fast and deterministic:

1. **The real implementation.** Use it for value types and pure logic. Real collaborators can expose integration
   defects that a replacement hides.
2. **An existing repository fake.** These fakes follow the real boundary and receive maintenance with it. Existing
   helpers include `warpui::App::test`, `VirtualFS`, `TerminalModel::mock(..)`, `TestBlockListBuilder`,
   `TestBlockBuilder`, `Appearance::mock()`, and `FeatureFlag::X.override_enabled(..)`.
3. **A stubbed return value.** Use this only to reach a state that the real boundary cannot produce deterministically.
   Give each stub a related assertion in the same test. Many stubs can indicate that the unit has too many
   responsibilities.
4. **An interaction assertion.** Use this last for a write, send, or spawn whose result has no other observable
   state. Do not assert calls to pure getters. Assert the returned value instead.

## Keeping tests deterministic

Investigate intermittent failures instead of rerunning until they pass.

- **Time.** Inject a clock or timestamp when the assertion depends on time.
- **Async.** Await the future, use a callback, or poll for a state transition with a bounded timeout. Do not use an
  arbitrary sleep as a completion signal.
- **Ordering.** Make tests independent of execution order and parallelism. Check statics, singletons, `OnceCell`,
  and environment variables.
- **Shared state.** Use `serial_test`'s `#[serial]` when shared state cannot be isolated. Otherwise, keep the state
  local to the test.

Do not leave an ignored flaky test without a linked issue and a concrete reason. Ignoring a test does not resolve
the underlying failure.

## Async and feature-gated code

- For async logic, use `#[tokio::test]` when the code requires a runtime.
- Prefer runtime feature checks such as `FeatureFlag::X.is_enabled()` over `#[cfg(...)]` when tests should toggle
  behavior without recompilation.

## Quickstart harness (UI/model tests)
- Prefer `warpui::App::test` for deterministic unit tests around views/models.
- Initialize app models once, then mutate via `update` and assert via `read`.

```rust
// These helpers are private to the app crate's own unit tests.
use crate::test_util::{add_window_with_terminal, terminal::initialize_app_for_terminal_view};
use warpui::App;

#[test]
fn example() {
    App::test((), |mut app| async move {
        // One-time app setup for terminal/view tests
        initialize_app_for_terminal_view(&mut app); // includes settings init
        let term = add_window_with_terminal(&mut app, None);

        // Act
        term.update(&mut app, |view, _ctx| {
            view.model.lock().simulate_block("ls", "out");
        });

        // Assert
        term.read(&app, |view, _ctx| {
            assert!(view.model.lock().block_list().len() > 0);
        });
    })
}
```

`app/src/test_util` is private and compiled only for the app crate's own tests. Other crates cannot import it.
There is no public equivalent of `initialize_app_for_terminal_view` or `add_window_with_terminal`. Build integration
tests with `integration::Builder` and the public `warp::integration_testing` helpers. Follow `gui-integration-test`
for the isolated app setup.

## Common helpers to use
- Terminal model shortcuts: `TerminalModel::mock(..)`, `.simulate_block(..)`, `.finish_block()`, `.simulate_cmd(..)`.
- Builders for focused tests: `terminal::model::test_utils::{TestBlockListBuilder, TestBlockBuilder}`.
- Virtual filesystem for IO-heavy code:
```rust
use virtual_fs::{VirtualFS, Stub};
VirtualFS::test("case", |_dirs, mut fs| {
    fs.with_files(vec![Stub::FileWithContent("path/file.txt", "contents")]);
    // run logic and assert
});
```
- Feature flags (scoped):
```rust
use warp::features::FeatureFlag; // or `use crate::features::FeatureFlag;` inside the app crate
let _flag = FeatureFlag::SelectablePrompt.override_enabled(true);
```
- UI numeric assertions (lines):
```rust
assert_lines_approx_eq!(actual_lines, INLINE_BANNER_HEIGHT);
```
- Concurrency: keep `model.lock()` scopes minimal; avoid nested/re-entrant locks in the same call chain.
- Don't call `initialize_settings_for_tests` directly when using `initialize_app_for_terminal_view` because the
  latter already calls it.
- Async needs: use `#[tokio::test]` when a real runtime is required; otherwise prefer `App::test`.
- Tests touching global/external state: consider `serial_test`'s `#[serial]` or local mocking instead of parallelism.

## Running unit tests

Run from the repository root after loading `.work/rust-env.ps1` (Windows) or
`.work/rust-env-unix.sh` (Linux). Use the pinned toolchain and current root/Goal
job and test-thread limits. Filter the affected package and test first:

```bash
cargo +1.92.0 test -p <package> --lib --locked --offline -j2 <test_filter> -- --test-threads=1
```

For suites that already use nextest, use the installed runner with a package
and name or expression filter; do not install another runner or default to the
whole workspace. Confirm that the intended test was collected and executed.
Review OS clipboard and other shared-state tests before running on a user PC;
use an isolated environment when their real side effects are required.

Run relevant doctests or scoped Clippy only when the change needs them.
Format/check edited Rust files with `skip_children=true` as appropriate;
do not invoke a whole-repository format/presubmit for a small patch. Rerun a
check only when later edits invalidate its evidence or an unresolved failure
requires it. Preserve results and limitations in the current `.work` note.
