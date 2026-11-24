---
name: gui-integration-test-video
description: >-
  Run and author NeoWarp GUI integration tests that capture screenshots and video through
  `TestStep::with_start_recording()` and `with_take_screenshot()`, including mouse and keyboard overlays.
  Use only for the `crates/integration` recording pipeline, a named test recording, or the artifacts it produced.
  Skip general requests to capture a live app or UI flow. Follow the root desktop-control rules for those requests.
---

# Integration test video recording

This skill applies only to NeoWarp's GUI desktop front end in the `app/` crate and its WarpUI framework.

Use this skill for NeoWarp's integration test recording pipeline.

## When not to use this skill

Use this skill only for screenshots and video captured by the Rust integration test harness in
`crates/integration`. For a general request to capture a live app or UI flow, follow the root desktop-control
instructions. If the project-local `agent-desktop` tool is available, read its installed instructions and use only
its permitted headless operations. Do not use integration test recording solely because a request mentions a
screenshot or recording.

The relevant implementation lives in:
- `crates/integration/src/bin/integration.rs`
- `crates/integration/src/test/video_recording.rs`
- `crates/integration/tests/integration/ui_tests.rs`
- `crates/warpui_core/src/integration/driver.rs`
- `crates/warpui_core/src/integration/step.rs`
- `crates/warpui_core/src/integration/video_recorder.rs`
- `crates/warpui_core/src/integration/artifacts.rs`
- `crates/warpui_core/src/integration/overlay.rs`

## Command to invoke a test

Run commands from the repository root with the configured Rust environment. Add `--offline` only when the required
dependencies are cached. Recording opens a real app window. Follow the root isolation and desktop-control rules
before running it. The current integration binary supports Unix. Its non-Unix branch is unimplemented.

For a single manually invoked recording test, prefer the integration binary:

```bash
WARPUI_USE_REAL_DISPLAY_IN_INTEGRATION_TESTS=1 \
cargo +1.92.0 run -p integration --bin integration --locked -j2 -- test_video_recording
```

This invokes the sample test in `crates/integration/src/test/video_recording.rs` with a real display.

If you want the driver to auto-record a test or set of tests, add `WARP_INTEGRATION_TEST_VIDEO`:

```bash
WARPUI_USE_REAL_DISPLAY_IN_INTEGRATION_TESTS=1 \
WARP_INTEGRATION_TEST_VIDEO=test_video_recording \
cargo +1.92.0 run -p integration --bin integration --locked -j2 -- test_video_recording
```

For broader integration test runs, the same environment variables work with nextest. Use a filter expression that
includes every selected test:

```bash
WARPUI_USE_REAL_DISPLAY_IN_INTEGRATION_TESTS=1 \
WARP_INTEGRATION_TEST_VIDEO=test_foo,test_bar \
cargo +1.92.0 nextest run -p integration --locked --no-fail-fast --build-jobs 2 --test-threads 1 \
  -E 'test(test_foo) or test(test_bar)'
```

## Environment variables

### `WARPUI_USE_REAL_DISPLAY_IN_INTEGRATION_TESTS`

Set this to `1` for real frame capture and manual visual verification. Recording requires a supported real display.

### `WARP_INTEGRATION_TEST_VIDEO`

This environment variable controls driver-managed video recording in
`crates/warpui_core/src/integration/driver.rs`.

- Unset or empty values disable automatic recording.
- `1` and `all` record every test in the run.
- A comma-separated list records only the named tests.

Examples:

```bash
# Record every test in the run
WARP_INTEGRATION_TEST_VIDEO=all
```

```bash
# Record only specific tests
WARP_INTEGRATION_TEST_VIDEO=test_foo,test_bar
```

A test that explicitly calls `with_start_recording()` and `with_stop_recording()` does not need
`WARP_INTEGRATION_TEST_VIDEO`. Use the environment variable for whole-test recording without changing test code.

### `WARP_INTEGRATION_TEST_ARTIFACTS_DIR`

This variable controls the root artifact directory used by `TestArtifacts` in
`crates/warpui_core/src/integration/artifacts.rs`. Set it to a directory under `.work/` for local runs.

If it is unset, artifacts go to `warp_integration_test_artifacts` under the system temporary directory returned by
`std::env::temp_dir()`.

Each run gets a timestamped directory:

```text
<artifacts_root>/<test_name>/<timestamp>/
```

Inspect this directory for screenshots, logs, and the final `recording.mp4`.

### `WARP_INTEGRATION_TEST_VIDEO_DIR`

This variable controls the lower-level recorder output root in
`crates/warpui_core/src/integration/video_recorder.rs`. Its default is `warp_integration_video_captures` under the
system temporary directory.

The integration driver writes the finalized video into the test artifacts directory. Inspect
`WARP_INTEGRATION_TEST_ARTIFACTS_DIR` for the test results.

## How to specify which tests to record

There are two modes:

### Record in test code

Call `TestStep::with_start_recording()` and `TestStep::with_stop_recording()` in the test to capture a specific
span.

### Record from the environment

Set `WARP_INTEGRATION_TEST_VIDEO` to `1`, `all`, or a comma-separated list such as `test_a,test_b`.
The driver starts recording at the beginning of each matching test and writes the video when the test completes.

## How overlays work

No separate overlay environment variable is required. The driver creates overlays from input events dispatched
while recording is active. `crates/warpui_core/src/integration/overlay.rs` implements the overlay pipeline.
`crates/warpui_core/src/integration/step.rs` captures the input events.

Use APIs that emit mouse and keyboard events:

- `with_event(...)`
- `with_event_fn(...)`
- `with_click_on_saved_position(...)`
- `with_keystrokes(...)`

The sample test exercises mouse click indicators, drag trails, and keyboard shortcut pills. Mouse down, drag, and
mouse up events create click and drag overlays. Key down events create keyboard overlay pills. A recording without
these input events has no corresponding annotations.

## How to write a test that takes screenshots

Use `TestStep::with_take_screenshot("filename.png")`.

Example pattern:

```rust
TestStep::new("Take screenshot after bootstrap")
    .with_take_screenshot("after_bootstrap.png")
```

The driver stores the screenshot request during the step and writes the PNG after rendering. The file goes into
the test's timestamped artifacts directory.

## How to write a test that records video

### Minimum pattern

1. Use `Builder::new().with_real_display()`.
2. Add a step with `with_start_recording()`.
3. Run the actions and events to capture.
4. Add a step with `with_stop_recording()`.

Example shape:

```rust
Builder::new()
    .with_real_display()
    .with_step(TestStep::new("Start recording").with_start_recording())
    .with_step(/* actions and events */)
    .with_step(TestStep::new("Stop recording").with_stop_recording())
```

### Record visible input overlays

Prefer steps that emit mouse and key events:

- Click with `with_click_on_saved_position(...)`.
- Dispatch mouse events with `with_event(...)` or `with_event_fn(...)`.
- Send keyboard shortcuts with `with_keystrokes(...)`.

For a drag overlay, dispatch `LeftMouseDown`, one or more `LeftMouseDragged` events, and `LeftMouseUp`.

### Validate the artifacts

Add a `with_on_finish(...)` hook that checks expected artifacts such as `recording.mp4`, `recording.log`, and
screenshot PNGs. The sample test checks these artifacts.

## Where the video assets go

The normal output location is:

```text
<artifacts_root>/<test_name>/<timestamp>/
```

The directory contains `recording.mp4`, `recording.log`, and screenshots requested with
`with_take_screenshot(...)`.

The `test_video_recording` sample checks `after_bootstrap.png`, `after_commands.png`, `recording.mp4`, and
`recording.log`.

If MP4 encoding fails during finalization, the recorder falls back to per-frame PNGs in a sibling directory like:

```text
recording_frames/
```

with files such as:

```text
recording_0000.png
```

## How to review the assets

1. Open the latest timestamped artifact directory for the test.
2. Review `recording.mp4` to check the UI state, recording boundaries, and overlay timing.
3. Review every PNG screenshot captured by the test.
4. Check `recording.log` if the output is incomplete or unexpected.
5. If `recording.mp4` is missing, inspect the fallback frame PNGs.

Include the exact artifact directory path in the result summary.

## Sample test for video recording

The manual sample test is `test_video_recording`.

- `crates/integration/src/bin/integration.rs` registers it.
- `crates/integration/tests/integration/ui_tests.rs` lists it.
- `crates/integration/src/test/video_recording.rs` implements it.

Run it with:

```bash
WARPUI_USE_REAL_DISPLAY_IN_INTEGRATION_TESTS=1 \
cargo +1.92.0 run -p integration --bin integration --locked -j2 -- test_video_recording
```

If you want full-test auto-recording from the environment as well, use:

```bash
WARPUI_USE_REAL_DISPLAY_IN_INTEGRATION_TESTS=1 \
WARP_INTEGRATION_TEST_VIDEO=test_video_recording \
cargo +1.92.0 run -p integration --bin integration --locked -j2 -- test_video_recording
```

## Working pattern for agents

When recording or debugging an integration test:

1. Identify the exact test name.
2. Choose explicit test recording or `WARP_INTEGRATION_TEST_VIDEO`.
3. Set `WARPUI_USE_REAL_DISPLAY_IN_INTEGRATION_TESTS=1`.
4. Dispatch mouse and keyboard events when the video needs interaction overlays.
5. Inspect the timestamped artifact directory.
6. Report the exact output paths and any missing artifacts.
