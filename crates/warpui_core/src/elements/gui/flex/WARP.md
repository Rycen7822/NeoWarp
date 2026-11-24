# Flex layout debugging

Use this guide to diagnose unbounded constraints and flexible child sizing in WarpUI.

## Fix unbounded main-axis constraints

### Error: `flex contains flexible children but has an infinite constraint along the flex axis`

A `Flex` reports this error when it has an `Expanded` or `Shrinkable` child and its maximum main-axis constraint is
infinite. This check applies with either `MainAxisSize::Min` or `MainAxisSize::Max`.

Choose the fix that matches the intended layout:

- Add a finite maximum constraint to the `Flex` or an ancestor. For `Flex::row()`, constrain the maximum width.
  For `Flex::column()`, constrain the maximum height. A `ConstrainedBox` can supply either constraint.
- Remove `Expanded` or `Shrinkable` when no child needs a share of the remaining space.
- For a nested `Flex`, trace the incoming constraint through its ancestors to find where the maximum becomes
  infinite.

### Error: `A flex that should expand to a max space can't be rendered in an infinite max constraint`

A `Flex` with `MainAxisSize::Max` reports this error when its maximum main-axis constraint is infinite.

Choose one of these fixes:

- Add a finite maximum constraint to the `Flex` or an ancestor.
- Remove `.with_main_axis_size(MainAxisSize::Max)` when the `Flex` does not need to fill its parent and no
  `Expanded` or `Shrinkable` child remains. `MainAxisSize::Min` is the default. If flexible children remain, add a
  finite maximum constraint instead.

Debug builds use `debug_assert!` for both errors. Debug messages include the `Flex` construction location when it
is available. The implementation also reports the error through `report_error!`.

## Understand child sizing

- `Expanded` and `Shrinkable` divide the remaining main-axis space according to their flex values.
- `Expanded` uses a tight fit. Its child receives a minimum and maximum main-axis size equal to the allocated share.
- `Shrinkable` uses a loose fit. Its child can use less than the allocated share.
- `Flex` lays out non-flexible children first with an unbounded main-axis constraint. It preserves the incoming
  cross-axis constraint, or applies a tight cross-axis constraint when using `CrossAxisAlignment::Stretch`.
- `Flex` reads flex metadata from direct children. Wrapping `Expanded` or `Shrinkable` in another element prevents
  the parent `Flex` from reading that metadata.
- `MainAxisSize::Max` requires a finite maximum constraint. This rule applies to nested and top-level `Flex`
  elements.

## Use common layout patterns

Center children on the main axis when the parent has bounded constraints:

```rust
Flex::row()
    .with_main_axis_size(MainAxisSize::Max)
    .with_main_axis_alignment(MainAxisAlignment::Center)
    .with_child(element)
    .finish()
```

Center children on the cross axis of a row:

```rust
Flex::row()
    .with_cross_axis_alignment(CrossAxisAlignment::Center)
    .with_child(element)
    .finish()
```

Place groups at opposite ends of a row with a flexible spacer:

```rust
Flex::row()
    .with_child(left_element)
    .with_child(Expanded::new(1.0, Empty::new()))
    .with_child(right_element)
    .finish()
```

## Debug a layout failure

1. Run the application with `RUST_BACKTRACE=full` to capture the failing layout stack.
2. In a debug build, use the construction location in the error message to identify the `Flex`.
3. Trace constraints from the window root to that element. Find the ancestor that supplies an infinite maximum.
4. Check that `Expanded` and `Shrinkable` remain direct children of the intended `Flex`.
5. Use `MainAxisSize::Max` only when the element must fill the available main-axis space.
