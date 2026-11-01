---
name: logging-and-error-reporting
description: Choose local log levels and typed error diagnostics in NeoWarp while keeping secrets out of logs.
---

# Local logging and error diagnostics

NeoWarp writes local diagnostics through `warp_logging`. `report_error!` and
`report_if_error!` in `crates/warp_errors/src/lib.rs` also write local logs;
there is no crash-upload or telemetry reporting path.

## Choose the level

- `error!`: an operation failed and needs attention.
- `warn!`: a handled failure or degraded condition where execution can continue.
- `info!`: infrequent lifecycle milestones. The native logger defaults to Info
  with module-specific filters in `crates/warp_logging/src/native.rs`.
- `debug!` / `trace!`: verbose, per-item or hot-path diagnostics. Avoid Info-level
  messages in rendering loops and per-message handlers.

Use a short, searchable prefix consistent with the surrounding module and
inline format arguments. Report an error once where it stops propagating;
propagate errors with `?` when the caller owns recovery or presentation.

## Preserve error classification

`report_error!(err)` uses `ErrorExt::is_actionable()` to select Error or Warn.
Keep a typed error and its source chain when that classification matters:

```rust
report_error!(anyhow::Error::new(err).context("Failed to read persisted data"));
```

The existing SQLite reader in `app/src/persistence/sqlite.rs` uses this form.
For a Result that only needs logging on failure, use
`report_if_error!(operation().context("Failed to complete operation"))`.
Use Result-level `.context()` when available rather than wrapping a Result
unnecessarily. Do not stringify an existing typed error into `anyhow!("{err}")`.
Add typed error variants only when callers or severity classification need them.

`extra: { "count" => %count }` adds local diagnostic context. It is not an
upload. `ReportErrorLogMode::OncePerRun` suppresses repeated messages at the
same macro invocation; use the existing option when repetition would flood logs.

## Keep diagnostics safe

Never log secrets, tokens, credentials, response bodies, file contents or
command text, including in debug builds. Prefer non-sensitive counts, durations,
error kinds and a redacted explanation. Check the entire source chain and
`extra:` values before logging; local storage does not make sensitive data safe.
