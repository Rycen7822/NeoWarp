---
name: gui-settings-ui
description: Build or edit NeoWarp SettingsWidget and PageType pages with correct headings, per-widget search, runtime visibility and stable widget links.
---

# NeoWarp settings UI

Read the affected page and `app/src/settings_view/settings_page.rs` before
editing. Current pages are represented by `SettingsPageViewHandle`; historical
AI, Drive, account and code-indexing examples are not supported settings.

## Page shape and headings

- `Uncategorized` contains a flat widget list; `Categorized` groups widgets
  under `Category` headings. Their optional page title is `PageTitle<V>`:

  ```rust
  PageType::new_uncategorized(widgets, Some(PageTitle::new("Editor")))
  PageType::new_categorized(categories, None)
  ```

- `Monolith` contains a single inseparable widget. Its `new_monolith` title
  argument remains `Option<&'static str>`, followed by `is_dual_scrollable`.
- Put a whole-page heading in the title slot and a section heading with its
  owning widget/category. Widget-list titles survive a partial filter; a
  non-matching monolith disappears as a whole, including its title.
- Use the current basic Editor page (`code_editor_review_page.rs`) or Privacy
  page as concrete examples. A header-only widget is unnecessary when the
  heading belongs to the page; retain actual controls and section headings.

## Search and visibility

- A widget is the smallest search unit. Keep `search_terms` specific to its
  row or tightly coupled group; do not give unrelated rows one large term set.
- `search_terms_match` requires every whitespace-delimited query word to occur
  case-insensitively in the widget's terms; an empty query matches all.
- Use page-construction guards for static platform/feature availability. Use
  `should_render` for values that change while the page is open. Filtering and
  rendering consult it; hiding advertised rows only inside `render` creates
  misleading search results.
- If a change rebuilds a `PageType` while search is active, reapply the current
  query through the existing filtering path. Do not add obsolete AI-subpage
  state or assume an old reapply helper still exists.
- Keep control state handles on their owning view/widget rather than creating
  them while rendering; reuse the existing UI components and themes.

## Links and verification

`widget_id` defaults to the Rust type name. Public widget slugs are allowlisted
by `settings_widget_deeplink_target` in `settings_view/mod.rs`; read the current
mapping before renaming a linked widget. Internal type names are not public
URI contracts.

Use related existing checks in `settings_view/mod_tests.rs` before adding
tests. Its `StubWidget`, `stub_widgets_page` and `visible_widget_count` cover
filter behavior. Add a case only for an uncovered accepted behavior, following
root limits. For a visible search change, inspect the relevant query, title
and clearing behavior using the existing script/harness where possible;
desktop control follows root rules. Do not boot the GUI for a documentation
change or add a full settings matrix.
