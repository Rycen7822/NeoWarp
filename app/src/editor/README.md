# Terminal input editor

## View ownership and communication

`TerminalView` owns a `ViewHandle<Input>` for the terminal input area. `Input` owns the `ViewHandle<EditorView>`
that edits the input text.

`EditorView` communicates with `Input` through editor actions. `Input` updates the editor through its view handle.
For example, `Input::input_enter` checks whether Enter should insert a newline. In that branch, it calls
`EditorView::user_initiated_insert` with `"\n"` and `PlainTextEditorViewAction::NewLine`. Other Enter branches can
accept a completion suggestion or execute the command.

## Buffer storage

The editor's `Buffer` stores text and edit history in a `SumTree<Fragment>`. Each fragment contains `Text`.
`Text` stores its contents as `Rc<str>` and indexes newline and character runs with a `SumTree<Run>`.
These runs summarize character counts, byte counts, and line positions.

Use `Buffer::chars_at` to iterate over characters from a buffer position. Use `Buffer::line_len` to get a row's
length in Rust `char` values, excluding the terminating newline. Both APIs return a `Result` when the input position
can be invalid.

## Buffer positions

`Point` identifies a row and column in buffer coordinates. Within `Buffer`, the column counts Rust `char` values,
not UTF-8 bytes or display cells. `CharOffset` counts Rust `char` values from the start of the buffer.
`ByteOffset` counts UTF-8 bytes. These offset types are distinct newtypes rather than bare `usize` values.

The `ToCharOffset` trait converts positions to `CharOffset`. The `ToBufferOffset` trait converts positions to
`ByteOffset`. Implementations exist for position types such as `Point` and `Anchor`. Conversion methods take the
buffer and return a `Result` because a position can be out of range.

## Display positions

`DisplayPoint` identifies a row and column in the `DisplayMap`, which describes the visual coordinate system.
The buffer does not store display layout. Multiple views can display the same buffer differently.

Display points and buffer points differ when code folding or soft wrapping changes the visible layout.
Use the `DisplayMap` to convert between these coordinate systems.

## Selections and cursors

`EditorView` holds a `ModelHandle<EditorModel>`. The model accesses local selections through its buffer.
`LocalSelections` stores established selections in `Vec1<LocalSelection>`, so at least one selection is present.
It also stores pending selection and marked text state.

Each `LocalSelection` contains a `Selection` and the display state needed to extend it. `Selection::start` and
`Selection::end` are anchors. The start anchor never sorts after the end anchor. When `reversed` is `false`, `end`
is the selection head. When `reversed` is `true`, `start` is the head. The head positions the cursor and is the end
extended by selection changes. An empty selection has equal start and end anchors.

## Anchors and insertion bias

An `Anchor` identifies a logical text position that remains resolvable after edits. Buffer and display map
conversions can resolve an anchor to a `Point`, an offset, or a `DisplayPoint`.

For a cursor between `l` and `E` in `PartialEq`:

```text
Partial|Eq
```

After inserting ten characters with a right-biased anchor, the cursor follows the original text:

```text
Partial1234567890|Eq
```

A left-biased anchor stays before the inserted text:

```text
Partial|1234567890Eq
```

Pass `AnchorBias::Left` or `AnchorBias::Right` to `Buffer::anchor_at` to select this behavior when creating an
anchor.
