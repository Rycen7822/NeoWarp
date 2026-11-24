# WarpUI

## Overview

WarpUI connects application state, views, layout elements, and event handlers. This overview introduces those
relationships before the more detailed module documentation.

Rust ownership rules affect user interfaces because views and models often refer to each other. WarpUI uses a
global `App` object, entity handles, and short-lived contexts to express those references.

## The global App object, entities, and handles

`App` owns the application's views and models. Views and models are entities. An entity refers to another entity
through a handle.

For example, a workspace that contains multiple terminal views stores a vector of handles rather than owning the
views directly:

```rust
struct WorkspaceView {
    sessions: Vec<ViewHandle<TerminalView>>,
}
```

A handle retains the referenced view in `App`. To access the view, pass an app context to the handle. `App` supplies
that context during operations such as rendering.

`WorkspaceView::render` receives an `&AppContext`. Inside that method, the view can resolve each `ViewHandle` and
read the terminal titles:

```rust
let titles = self
    .sessions
    .iter()
    .map(|handle| handle.as_ref(ctx).title())
    .collect::<Vec<String>>();
```

The `&AppContext` is valid only for the current framework operation. Do not store the references returned by
`handle.as_ref(ctx)` after that operation ends.

Entities can also own state directly. Use a handle when an entity needs to refer to another entity owned by `App`.

## Elements

Every view implements `View`. Its `render` method computes an element tree from the current state. The framework
calls `render` when the view's state changes.

An element describes one version of a view's appearance. Elements can be recycled when their views do not change.
When a view changes, the framework can discard the old element tree and use the new one.

WarpUI includes elements for backgrounds, borders, padding, text, layout, and event handling. Their design is
loosely based on Flutter. Custom elements can control layout and paint pixels through the hardware-accelerated
`Scene` API.
