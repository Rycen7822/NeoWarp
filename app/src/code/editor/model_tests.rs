use std::path::Path;

use vec1::vec1;
use warp_editor::content::buffer::SelectionOffsets;
use warp_editor::multiline::MultilineString;
use warp_util::content_version::ContentVersion;
use warpui::App;

use super::*;
use crate::code::editor::view::code_text_styles;
use crate::test_util::settings::initialize_settings_for_tests;

fn initialize_deps(app: &mut App) {
    app.add_singleton_model(|_| Appearance::mock());
    initialize_settings_for_tests(app);
}

fn mock_model(app: &mut App, text: &str, version: ContentVersion) -> ModelHandle<CodeEditorModel> {
    let editor = app.add_model(|ctx| {
        let styles = code_text_styles(Appearance::as_ref(ctx));
        let mut model = CodeEditorModel::new(styles, None, None, ctx);
        model.content().update(ctx, |buffer, ctx| {
            buffer.replace_all(text, ctx);
            buffer.set_version(version);
        });
        model
            .diff
            .update(ctx, |diff, _| diff.set_base(MultilineString::apply(text)));
        model.set_language_with_local_path(Path::new("/test.rs"), ctx);
        model
    });
    editor.update(app, |model, ctx| {
        let end = model.content().as_ref(ctx).max_charoffset();
        model.cursor_at(end, ctx);
    });
    editor
}

/// Builds a `CodeEditorModel` that shares an existing `Buffer` rather than creating its own.
/// Used to reproduce configurations where multiple editors point at the same underlying content
/// (e.g. two views of the same open file), such as via `CodeEditorView::new`'s optional shared
/// `Buffer` parameter.
fn mock_model_with_buffer(
    app: &mut App,
    buffer: ModelHandle<Buffer>,
) -> ModelHandle<CodeEditorModel> {
    app.add_model(|ctx| {
        let styles = code_text_styles(Appearance::as_ref(ctx));
        let mut model = CodeEditorModel::new(styles, None, Some(buffer), ctx);
        model.set_language_with_local_path(Path::new("/test.rs"), ctx);
        model
    })
}

async fn layout_model(app: &mut App, model: &ModelHandle<CodeEditorModel>) {
    app.read(|ctx| model.as_ref(ctx).render_state.as_ref(ctx).layout_complete())
        .await;
}

#[test]
fn test_two_editors_sharing_a_buffer_both_lay_out_a_large_content_replace() {
    // Regression test for APP-4844: `CodeEditorView::new` can pass an existing shared `Buffer`
    // into `CodeEditorModel::new`, so two editors can point at the same underlying content (for
    // example, two views of the same open file). Both editors independently subscribe to and
    // clone the `ContentChanged` event's `EditDelta` emitted by a full content replace, so more
    // than one clone of `new_lines` can be alive when either editor lays out. This exercises that
    // configuration end to end: it must not panic, and both editors must lay out the new content
    // correctly regardless of how many clones of the delta are alive when their layout runs.
    App::test((), |mut app| async move {
        initialize_deps(&mut app);

        let shared_buffer = app.add_model(|_| Buffer::default());
        let editor_a = mock_model_with_buffer(&mut app, shared_buffer.clone());
        let editor_b = mock_model_with_buffer(&mut app, shared_buffer.clone());

        let content = "line one\nline two\nline three\n".repeat(50);
        shared_buffer.update(&mut app, |buffer, ctx| {
            buffer.replace_all(content.clone(), ctx);
        });

        layout_model(&mut app, &editor_a).await;
        layout_model(&mut app, &editor_b).await;

        for editor in [&editor_a, &editor_b] {
            editor.read(&app, |editor, ctx| {
                let text = editor.content.as_ref(ctx).text().as_str().to_string();
                assert!(
                    text.contains("line three"),
                    "expected replaced content, got: {text}"
                );
                assert_eq!(text.matches("line one").count(), 50);
                assert!(editor.render_state.as_ref(ctx).blocks() > 1);
            });
        }
    })
}

#[test]
fn replace_first_n_characters_handles_incremental_unicode_prefix() {
    App::test((), |mut app| async move {
        initialize_deps(&mut app);
        let editor = mock_model(&mut app, "suffix", ContentVersion::new());

        editor.update(&mut app, |editor, ctx| {
            editor.replace_first_n_characters(CharOffset::from(0), "é", ctx);
            editor.replace_first_n_characters(CharOffset::from(1), "élan", ctx);
        });

        editor.read(&app, |editor, ctx| {
            assert_eq!(editor.content().as_ref(ctx).text().as_str(), "élansuffix");
        });
    });
}

#[test]
fn test_no_trailing_newline() {
    App::test((), |mut app| async move {
        initialize_deps(&mut app);
        let editor = mock_model(&mut app, "", ContentVersion::new());
        // We need to layout the model to be able to select by line boundaries.
        layout_model(&mut app, &editor).await;

        editor.update(&mut app, |editor, ctx| {
            editor.cursor_at(CharOffset::from(1), ctx);
        });

        // At the start we should have a trailing newline (the same height as an empty line).
        let old_height = app.read(|ctx| editor.as_ref(ctx).render_state().as_ref(ctx).height());

        editor.update(&mut app, |editor, ctx| {
            editor.insert("a", EditOrigin::UserTyped, ctx);
        });
        layout_model(&mut app, &editor).await;

        // After insertion, we should replace the trailing newline with an actual text line.
        let height_after_insertion =
            app.read(|ctx| editor.as_ref(ctx).render_state().as_ref(ctx).height());
        assert_eq!(old_height, height_after_insertion);

        editor.update(&mut app, |editor, ctx| {
            editor.enter(ctx);
        });
        layout_model(&mut app, &editor).await;

        // Enter should create a newline, which adds to the height.
        let height_after_newline =
            app.read(|ctx| editor.as_ref(ctx).render_state().as_ref(ctx).height());
        assert_ne!(old_height, height_after_newline);
    })
}

#[test]
fn test_toggle_comment() {
    App::test((), |mut app| async move {
        initialize_deps(&mut app);
        let editor = mock_model(&mut app, "    First\n    Second", ContentVersion::new());
        // We need to layout the model to be able to select by line boundaries.
        layout_model(&mut app, &editor).await;

        editor.update(&mut app, |editor, ctx| {
            editor.cursor_at(CharOffset::from(5), ctx);
            editor.select_to_line_end(ctx);
        });

        editor.update(&mut app, |editor, ctx| {
            assert_eq!(
                editor.selections(ctx),
                vec1![SelectionOffsets {
                    head: CharOffset::from(10),
                    tail: CharOffset::from(5)
                }]
            );
            editor.toggle_comments(ctx);
        });

        // Toggling comment for the first line.
        editor.read(&app, |editor, ctx| {
            assert_eq!(
                editor.content.as_ref(ctx).text().as_str(),
                "    // First\n    Second"
            );
        });

        editor.update(&mut app, |editor, ctx| {
            editor.select_all(ctx);
        });

        // Not all selected lines have comment prefix. This should add the prefix again.
        editor.update(&mut app, |editor, ctx| {
            editor.toggle_comments(ctx);
        });

        editor.read(&app, |editor, ctx| {
            assert_eq!(
                editor.content.as_ref(ctx).text().as_str(),
                "    // // First\n    // Second"
            );
        });

        editor.update(&mut app, |editor, ctx| {
            editor.cursor_at(CharOffset::from(20), ctx);
            editor.select_to_line_end(ctx);
        });

        editor.update(&mut app, |editor, ctx| {
            editor.toggle_comments(ctx);
        });

        // Toggling comment on a line with comment should uncomment it.
        editor.read(&app, |editor, ctx| {
            assert_eq!(
                editor.content.as_ref(ctx).text().as_str(),
                "    // // First\n    Second"
            );
        });
    })
}

#[test]
fn test_move_by_word() {
    App::test((), |mut app| async move {
        initialize_deps(&mut app);
        let editor = mock_model(
            &mut app,
            "fn test() {}\nfn test2() {}",
            ContentVersion::new(),
        );
        layout_model(&mut app, &editor).await;

        editor.update(&mut app, |editor, ctx| {
            editor.cursor_at(CharOffset::from(1), ctx);
            editor.add_cursor_at(14.into(), ctx);

            editor.forward_word_with_unit(true, word_unit(ctx), ctx);
        });

        editor.read(&app, |editor, ctx| {
            assert_eq!(
                editor.selections(ctx),
                vec1![
                    SelectionOffsets {
                        head: CharOffset::from(3),
                        tail: CharOffset::from(1)
                    },
                    SelectionOffsets {
                        head: CharOffset::from(16),
                        tail: CharOffset::from(14)
                    }
                ]
            );
        });

        editor.update(&mut app, |editor, ctx| {
            editor.backward_word_with_unit(false, word_unit(ctx), ctx);
        });

        editor.read(&app, |editor, ctx| {
            assert_eq!(
                editor.selections(ctx),
                vec1![
                    SelectionOffsets {
                        head: CharOffset::from(1),
                        tail: CharOffset::from(1)
                    },
                    SelectionOffsets {
                        head: CharOffset::from(14),
                        tail: CharOffset::from(14)
                    }
                ]
            );
        });
    })
}

#[test]
fn test_version_match() {
    App::test((), |mut app| async move {
        let initial_version = ContentVersion::new();

        initialize_deps(&mut app);
        let editor = mock_model(&mut app, "fn test() {}\nfn test2() {}", initial_version);
        layout_model(&mut app, &editor).await;

        editor.update(&mut app, |editor, ctx| {
            assert!(editor.content().as_ref(ctx).version_match(&initial_version));

            editor.insert("hey", EditOrigin::UserTyped, ctx);

            // The version should no longer match.
            assert!(!editor.content().as_ref(ctx).version_match(&initial_version));
        });
    });
}

#[test]
fn test_loaded_buffer_version() {
    App::test((), |mut app| async move {
        let initial_version = ContentVersion::new();

        initialize_deps(&mut app);
        let editor = mock_model(&mut app, "fn test() {}\nfn test2() {}", initial_version);
        layout_model(&mut app, &editor).await;

        editor.update(&mut app, |editor, ctx| {
            let version2 = ContentVersion::new();

            editor.content().update(ctx, |buffer, ctx| {
                buffer.replace_all("heyo", ctx);
                buffer.set_version(version2);
            });

            assert!(!editor.content().as_ref(ctx).version_match(&initial_version));
            assert!(editor.content().as_ref(ctx).version_match(&version2));
        });
    });
}

#[test]
fn test_edit_preserves_windows_line_endings() {
    App::test((), |mut app| async move {
        initialize_deps(&mut app);

        let editor = mock_model(&mut app, "One\r\nTwo\r\nThree\r\n", ContentVersion::new());
        layout_model(&mut app, &editor).await;

        // Modify the middle line.
        editor.update(&mut app, |editor, ctx| {
            editor.cursor_at(CharOffset::from(8), ctx);
            editor.insert("!!", EditOrigin::UserTyped, ctx);
        });

        // Verify that the content uses the original line endings.
        editor.read(&app, |editor, ctx| {
            assert_eq!(
                editor.content_string(ctx).as_str(),
                "One\r\nTwo!!\r\nThree\r\n"
            );
        });
    });
}

#[test]
fn test_edit_normalizes_mixed_line_endings() {
    App::test((), |mut app| async move {
        initialize_deps(&mut app);

        let editor = mock_model(&mut app, "One\nTwo\nThree\r\n", ContentVersion::new());
        layout_model(&mut app, &editor).await;

        // Modify the middle line.
        editor.update(&mut app, |editor, ctx| {
            editor.cursor_at(CharOffset::from(8), ctx);
            editor.insert("!!", EditOrigin::UserTyped, ctx);
        });

        // Verify that the content uses the original, inferred line endings.
        editor.read(&app, |editor, ctx| {
            assert_eq!(editor.content_string(ctx).as_str(), "One\nTwo!!\nThree\n");
        });
    });
}
