---
name: add-feature-flag
description: Add an opt-in flag for an accepted local NeoWarp feature.
---

# Add a Feature Flag

Use a flag only when the accepted change needs optional activation. Read its actual consumers and preserve ordinary terminal behavior.

1. Add the variant in `crates/warp_features/src/lib.rs`.
2. Add an opt-in Cargo feature under `[features]` in `app/Cargo.toml`.
3. Map it inside `enabled_features()` in `app/src/features.rs`:

```rust
#[cfg(feature = "your_feature")]
FeatureFlag::YourFeature,
```

4. Gate the real consumer with `FeatureFlag::YourFeature.is_enabled()`. Include settings schema/widget availability and keybinding predicates when those belong to the feature; keep persisted setting keys independent of flag indices.
5. Enable defaults only when the accepted requirement calls for them. Cargo defaults, target-conditional `RELEASE_FLAGS`, and developer-only `DEBUG_FLAGS` are the current activation sources. Preserve their platform boundaries.

From the root, with the configured build environment and cache locations:

```powershell
cargo +1.92.0 check --locked -j2 -p warp --features release_bundle,gui,your_feature
```

Run the nearest relevant existing checks. Put temporary probes in `.work/`; do not add a framework, permanent test matrix, private channel, or remote flag provider. Use the existing thread-local override API for tests that need a flag value.
