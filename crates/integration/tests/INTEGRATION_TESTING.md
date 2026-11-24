# NeoWarp integration tests

Use integration tests for behavior that requires a real app, PTY, shell, window, or cross-component wiring. Keep
pure parsing and state transitions in unit tests. An integration test sets up isolated state, drives user actions,
and checks the resulting app state.

The [GUI integration test guide](../../../.agents/skills/gui-integration-test/SKILL.md) describes the framework,
helper APIs, artifact capture, and debugging options in more detail.

## Register a test

Each test needs all of these entries:

1. Implement `pub fn test_name() -> Builder` in a module under `crates/integration/src/test/`.
2. Add the module to `crates/integration/src/test.rs` and re-export its test functions.
3. Add `register_test!(test_name);` to `register_tests()` in
   `crates/integration/src/bin/integration.rs`.
4. Add `test_name` to `integration_tests!` in either
   `crates/integration/tests/integration/ui_tests.rs` or
   `crates/integration/tests/integration/shell_integration_tests.rs`.

Use the shell integration list when the test needs every shell or a specific shell matrix. Use the UI list for
other app behavior. Registration in the binary does not add the test to nextest. The outer
`crates/integration/tests/integration.rs` file only declares the test modules.

Test functions return `Builder`. The integration binary builds the `TestDriver`. Do not call `.build()` in the test
function or add `#[test]` to it.

## Write a test

Prefer existing helpers from `warp::integration_testing` before creating raw input events. This example waits for
terminal bootstrap, runs a command, and checks the window count:

```rust
use warp::integration_testing::step::new_step_with_default_assertions;
use warp::integration_testing::terminal::{
    clear_blocklist_to_remove_bootstrapped_blocks, execute_echo_str,
    wait_until_bootstrapped_single_pane_for_tab,
};
use warpui_core::async_assert_eq;

use crate::Builder;

pub fn test_example() -> Builder {
    Builder::new()
        .with_step(wait_until_bootstrapped_single_pane_for_tab(0))
        .with_step(clear_blocklist_to_remove_bootstrapped_blocks())
        .with_step(execute_echo_str(0, "echo hello"))
        .with_step(
            new_step_with_default_assertions("Check app state").add_named_assertion(
                "one app window",
                |app, _window_id| async_assert_eq!(app.window_ids().len(), 1, "expected one app window"),
            ),
        )
}
```

Use `with_keystrokes(&["ctrl-c"])` for key input and `with_input_string("text", None)` for text input. Both APIs
accept string-like values. Do not parse `Keystroke` objects before calling them.

## Write assertions

A normal `assert!` or `assert_eq!` panics on its first failed evaluation. Use `async_assert!` or `async_assert_eq!`
when the app can reach the expected state before the step timeout. Return the macro's `AssertionOutcome` from the
assertion callback so the driver can poll it.

Prefer `add_named_assertion` over `add_assertion`. A specific name makes timeout output actionable. Assertions can
run more than once, so keep them idempotent and avoid changing app state.

Use `AssertionOutcome::ImmediateFailure` only when a failure must stop polling immediately. Use
`PreconditionFailed` only for a genuine environmental precondition. Do not use retries or repeated runs to hide a
product or test defect.

## Run a test

Run commands from the repository root with the configured Rust environment. Add `--offline` only when all required
dependencies are cached.

Run the integration binary directly while authoring a test:

```bash
cargo +1.92.0 run -p integration --bin integration --locked -j2 -- test_example
```

After registering the test for nextest, run:

```bash
cargo +1.92.0 nextest run -p integration --locked --build-jobs 2 --test-threads 1 test_example
```

Set `WARP_SHELL_PATH` to test a specific installed shell. Fish does not require a `fish_shell` Cargo feature.
For a real display, set `WARPUI_USE_REAL_DISPLAY_IN_INTEGRATION_TESTS=1` in the same command. Real-display runs
must follow the repository's desktop-control and isolation rules.

The current integration binary supports Unix. Its non-Unix branch is unimplemented. Do not report a native Windows
run from a build or command that did not execute the test.

## Add a SQLite snapshot

Store sanitized test snapshots in `crates/integration/tests/data/`. Do not copy a personal profile database without
removing private paths, history, credentials, and other user data.

`TEST_ONLY_ASSETS` embeds files from that directory. During `Builder::with_setup`, copy the selected snapshot to
the isolated app database path with `create_file_from_assets` and `database_file_path_for_scope`. Follow the existing
pattern in `crates/integration/src/test/session_restoration.rs`:

```rust
Builder::new().with_setup(|_utils| {
    warp::integration_testing::create_file_from_assets(
        TEST_ONLY_ASSETS,
        "three_tabs.sqlite",
        &warp::integration_testing::persistence::database_file_path_for_scope(
            &warp::integration_testing::persistence::PersistenceScope::App,
        ),
    );
})
```

Adding a database file without this setup does not load it into the isolated test app.

## Register integration-only key bindings

Synthetic keypresses do not trigger every macOS menu binding. Before adding an integration-only binding, check for
an existing binding in the feature's source. Current registrations use `ctx.register_fixed_bindings` and
`FixedBinding::new` under `ChannelState::channel() == Channel::Integration`. See
`app/src/editor/view/mod.rs` for the editor's integration-only bindings. Use the `id!` macro for binding contexts.
