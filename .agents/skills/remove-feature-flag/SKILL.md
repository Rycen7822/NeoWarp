---
name: remove-feature-flag
description: Retire unused NeoWarp flags or simplify a feature whose final behavior is settled.
---

# Remove a Feature Flag

Determine the accepted behavior before editing. An unused definition, a permanently enabled feature, and a removed capability require different consumer changes.

1. Read the variant, producers and real consumers. Search `app/`, `crates/`, macros, scripts and development guides; use structural search where syntax matters and verify bindings. A name match alone is insufficient.
2. Preserve the accepted branch for continuing functionality, or remove the complete retired capability. Fold conditions structurally instead of leaving literal-true/false gates, empty providers or compatibility copies.
3. Remove the variant from `crates/warp_features/src/lib.rs`, its Cargo bridge in `app/src/features.rs`, the declaration/default entry in `app/Cargo.toml`, and any actual `RELEASE_FLAGS`/`DEBUG_FLAGS` entry. Do not enable another dormant flag as compensation.
4. Preserve persisted settings, shortcuts, shell/SSH/WSL behavior and platform guards that still have consumers. FeatureFlag indices are process-local implementation details; settings keep their own storage keys.
5. Inspect directly affected tests: keep or migrate protection for continuing behavior and retire tests owned solely by the deleted contract. Prefer existing tests; add nothing without a current coverage gap.

Run checks for affected packages with the repository environment, `--locked` and the configured job limit. Format only edited files. Temporary scripts, logs and comparisons belong in `.work/`; archive obsolete material there according to root instructions.

Finish with a scoped diff, actual verification results and current note. Do not run full workspace/presubmit, invent release infrastructure, or change unrelated feature defaults.
