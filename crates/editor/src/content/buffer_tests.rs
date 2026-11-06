use std::collections::VecDeque;
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use markdown_parser::{FormattedTextFragment, FormattedTextStyles};
use rand::SeedableRng;
use rand::rngs::StdRng;
use string_offset::{ByteOffset, CharOffset};
use vec1::{Vec1, vec1};
use warp_util::content_version::ContentVersion;
use warpui_core::text::point::Point;
use warpui_core::{App, ModelContext, ModelHandle};

use super::{BufferEvent, EditResult, ToBufferCharOffset};
use crate::content::anchor::AnchorSide;
use crate::content::buffer::{
    ActionWithSelectionDelta, AutoScrollBehavior, Buffer, BufferEditAction, BufferSelectAction,
    EditOrigin, SelectionOffsets, StyledBlockBoundaryBehavior, StyledBufferBlock, StyledBufferRun,
    StyledTextBlock, TabIndentation, ToBufferByteOffset, ToBufferPoint,
};
use crate::content::core::{CoreEditorAction, CoreEditorActionType, EditorLine};
use crate::content::cursor::BufferSumTree;
use crate::content::edit::PreciseDelta;
use crate::content::selection::TextStyleBias;
use crate::content::selection_model::BufferSelectionModel;
use crate::content::text::{
    BufferBlockStyle, IndentBehavior, IndentUnit, TextStyles, TextStylesWithMetadata,
};
use crate::content::undo::{
    NonAtomicType, ReversibleEditorActions, ReversibleSelectionState, UndoActionType, UndoArg,
};
use crate::render::model::RenderedSelectionSet;

fn set_selections<T: Into<CharOffset>>(
    selection_model: &mut BufferSelectionModel,
    selections: Vec1<Range<T>>,
) {
    selection_model.set_selection_offsets(selections.mapped(|range| SelectionOffsets {
        head: range.end.into(),
        tail: range.start.into(),
    }));
}

impl Buffer {
    // Helper function to push a nonatomic action to the undo stack.
    // This is needed to skip the timer check in unit test since it could cause
    // flakiness.
    fn push_undo_item_nonatomic(
        &mut self,
        prev_selection_range: RenderedSelectionSet,
        arg: UndoArg,
        action: NonAtomicType,
        selection_model: ModelHandle<BufferSelectionModel>,
        ctx: &mut ModelContext<Self>,
    ) {
        self.undo_stack.push_non_atomic_edit(
            ReversibleEditorActions {
                actions: arg.actions,
                replacement_range: arg.replacement_range,
                selections: ReversibleSelectionState {
                    next: prev_selection_range,
                    reverse: self.to_rendered_selection_set(selection_model, ctx),
                },
            },
            action,
            self.content_version,
        );
    }

    // Helper function to set a single selection range.
    fn set_selection(
        &mut self,
        range: Range<CharOffset>,
        selection_model: ModelHandle<BufferSelectionModel>,
        ctx: &mut ModelContext<Self>,
    ) {
        self.add_cursor(range.start, true, selection_model.clone(), ctx);
        self.set_last_head(range.end, selection_model, ctx);
    }

    pub(in crate::content) fn apply_formatted_edit_fixture(
        &mut self,
        range: Range<CharOffset>,
        text: VecDeque<EditorLine>,
        origin: EditOrigin,
        selection_model: ModelHandle<BufferSelectionModel>,
        ctx: &mut ModelContext<Self>,
    ) -> EditResult {
        let actions = vec![
            CoreEditorAction::new(
                range.clone(),
                CoreEditorActionType::Insert {
                    text,
                    source: origin,

                    insert_on_selection: true,
                },
            ),
            self.update_buffer_end(range.end),
        ];
        let edit = ActionWithSelectionDelta::new_with_offsets(
            actions,
            &mut self.internal_anchors,
            range.end,
            range.end,
            0,
            AnchorSide::Right,
        );
        self.modify_first_selection(selection_model, edit, ctx)
    }

    pub(crate) fn initialize_editor_lines_fixture(
        &mut self,
        text: VecDeque<EditorLine>,
        selection: ModelHandle<BufferSelectionModel>,
        ctx: &mut ModelContext<Self>,
    ) {
        self.apply_formatted_edit_fixture(
            0.into()..1.into(),
            text,
            EditOrigin::UserInitiated,
            selection,
            ctx,
        );
    }

    pub fn mock_from_plain_text(
        text: &str,
        tab_indentation: TabIndentation,
        ctx: &mut App,
    ) -> (ModelHandle<Self>, ModelHandle<BufferSelectionModel>) {
        let buffer = ctx.add_model(|_| Buffer::new(Box::new(|_, _| IndentBehavior::Ignore)));
        let selection = ctx.add_model(|_| BufferSelectionModel::new(buffer.clone()));
        buffer.update(ctx, |buffer, ctx| {
            *buffer = Buffer::from_plain_text(text, tab_indentation, selection.clone(), ctx);
        });
        (buffer, selection)
    }
}

#[test]
fn test_edit_plain_text() {
    App::test((), |mut app| async move {
        let buffer = app.add_model(|_| Buffer::new(Box::new(|_, _| IndentBehavior::Ignore)));
        let selection = app.add_model(|_| BufferSelectionModel::new(buffer.clone()));

        buffer.update(&mut app, |buffer, ctx| {
            let _ = buffer.edit_internal("test", TextStyles::default(), selection.clone(), ctx);
            assert_eq!(buffer.content.debug(), "<text>test");

            buffer.set_selection(
                CharOffset::from(1)..CharOffset::from(4),
                selection.clone(),
                ctx,
            );
            let _ = buffer.edit_internal("be", TextStyles::default(), selection.clone(), ctx);
            assert_eq!(buffer.content.debug(), "<text>bet");

            buffer.set_selection(
                CharOffset::from(2)..CharOffset::from(3),
                selection.clone(),
                ctx,
            );
            let _ = buffer.edit_internal("", TextStyles::default(), selection.clone(), ctx);
            assert_eq!(buffer.content.debug(), "<text>bt");

            buffer.set_selection(
                CharOffset::from(1)..CharOffset::from(1),
                selection.clone(),
                ctx,
            );
            let _ = buffer.edit_internal("ke", TextStyles::default(), selection.clone(), ctx);
            assert_eq!(buffer.content.debug(), "<text>kebt");

            buffer.set_selection(
                CharOffset::from(2)..CharOffset::from(4),
                selection.clone(),
                ctx,
            );
            let _ = buffer.edit_internal("m", TextStyles::default(), selection.clone(), ctx);
            assert_eq!(buffer.content.debug(), "<text>kmt");
        });
    });
}

#[test]
fn test_edit_end_of_text() {
    App::test((), |mut app| async move {
        let buffer = app.add_model(|_| Buffer::new(Box::new(|_, _| IndentBehavior::Ignore)));
        let selection = app.add_model(|_| BufferSelectionModel::new(buffer.clone()));

        buffer.update(&mut app, |buffer, ctx| {
            let _ = buffer.edit_internal("test", TextStyles::default(), selection.clone(), ctx);
            assert_eq!(buffer.content.debug(), "<text>test");

            let _ = buffer.enter(false, TextStyles::default(), selection.clone(), ctx);
            assert_eq!(buffer.content.debug(), "<text>test\\n");

            let _ = buffer.enter(false, TextStyles::default(), selection.clone(), ctx);
            assert_eq!(buffer.content.debug(), "<text>test\\n\\n");
        });
    });
}

#[test]
fn test_edit_styled_text() {
    App::test((), |mut app| async move {
        let buffer = app.add_model(|_| Buffer::new(Box::new(|_, _| IndentBehavior::Ignore)));
        let selection = app.add_model(|_| BufferSelectionModel::new(buffer.clone()));

        buffer.update(&mut app, |buffer, ctx| {
            let _ = buffer.edit_internal("te", TextStyles::default(), selection.clone(), ctx);
            assert_eq!(buffer.text().as_str(), "te");
            assert_eq!(buffer.content.debug(), "<text>te");

            buffer.set_selection(
                CharOffset::from(3)..CharOffset::from(3),
                selection.clone(),
                ctx,
            );
            // Correct buffer state: te<b>st<b>.
            let _ =
                buffer.edit_internal("st", TextStyles::default().bold(), selection.clone(), ctx);
            assert_eq!(buffer.content.debug(), "<text>te<b_s>st<b_e>");

            // Correct buffer state: te<i>x<i><b>t<b>.
            buffer.set_selection(
                CharOffset::from(3)..CharOffset::from(4),
                selection.clone(),
                ctx,
            );
            let _ =
                buffer.edit_internal("x", TextStyles::default().italic(), selection.clone(), ctx);
            assert_eq!(buffer.content.debug(), "<text>te<i_s>x<b_s><i_e>t<b_e>");

            // Correct buffer state: t<b>i<b><i>x<i><b>t<b>.
            buffer.set_selection(
                CharOffset::from(2)..CharOffset::from(3),
                selection.clone(),
                ctx,
            );
            let _ = buffer.edit_internal("i", TextStyles::default().bold(), selection.clone(), ctx);
            assert_eq!(
                buffer.content.debug(),
                "<text>t<b_s>i<b_e><i_s>x<b_s><i_e>t<b_e>"
            );

            // Correct buffer state: t<b>i<b>a<b>t<b>.
            buffer.set_selection(
                CharOffset::from(3)..CharOffset::from(4),
                selection.clone(),
                ctx,
            );
            let _ = buffer.edit_internal("a", TextStyles::default(), selection.clone(), ctx);
            assert_eq!(buffer.content.debug(), "<text>t<b_s>i<b_e>a<b_s>t<b_e>");

            // Correct buffer state: t<b>at<b>.
            buffer.set_selection(
                CharOffset::from(2)..CharOffset::from(4),
                selection.clone(),
                ctx,
            );
            let _ = buffer.edit_internal("a", TextStyles::default().bold(), selection.clone(), ctx);
            assert_eq!(buffer.content.debug(), "<text>t<b_s>at<b_e>");

            // Correct buffer state: t<b>axt<b>.
            buffer.set_selection(
                CharOffset::from(3)..CharOffset::from(3),
                selection.clone(),
                ctx,
            );
            let _ = buffer.edit_internal("x", TextStyles::default().bold(), selection.clone(), ctx);
            assert_eq!(buffer.content.debug(), "<text>t<b_s>axt<b_e>");

            // Correct buffer state: t<b>axting<b>.
            buffer.set_selection(
                CharOffset::from(5)..CharOffset::from(5),
                selection.clone(),
                ctx,
            );
            let _ =
                buffer.edit_internal("ing", TextStyles::default().bold(), selection.clone(), ctx);
            assert_eq!(buffer.content.debug(), "<text>t<b_s>axting<b_e>");
        });
    });
}

#[test]
fn test_edit_with_anchors() {
    App::test((), |mut app| async move {
        let buffer = app.add_model(|_| Buffer::new(Box::new(|_, _| IndentBehavior::Ignore)));
        let selection = app.add_model(|_| BufferSelectionModel::new(buffer.clone()));

        buffer.update(&mut app, |buffer, ctx| {
            buffer.edit_internal("test", TextStyles::default(), selection.clone(), ctx);
            assert_eq!(buffer.text().as_str(), "test");
        });

        // Anchors are clamped to the end of the buffer
        let end = selection.update(&mut app, |selection, ctx| selection.anchor(500.into(), ctx));

        selection.read(&app, |selection, _| {
            assert_eq!(selection.resolve_anchor(&end), Some(5.into()));
        });

        buffer.update(&mut app, |buffer, ctx| {
            // When inserting text, the anchor is updated.
            buffer.set_selection(
                CharOffset::from(5)..CharOffset::from(5),
                selection.clone(),
                ctx,
            );
            buffer.edit_internal("ing", TextStyles::default(), selection.clone(), ctx);
            assert_eq!(buffer.text().as_str(), "testing");
        });

        selection.read(&app, |selection, _| {
            assert_eq!(selection.resolve_anchor(&end), Some(8.into()));
        });

        buffer.update(&mut app, |buffer, ctx| {
            // Likewise, it's updated when deleting, at the cursor or before it.
            buffer.set_selection(
                CharOffset::from(7)..CharOffset::from(8),
                selection.clone(),
                ctx,
            );
            buffer.edit_internal("", TextStyles::default(), selection.clone(), ctx);
            assert_eq!(buffer.text().as_str(), "testin");
        });

        selection.read(&app, |selection, _| {
            assert_eq!(selection.resolve_anchor(&end), Some(7.into()));
        });

        buffer.update(&mut app, |buffer, ctx| {
            buffer.set_selection(
                CharOffset::from(3)..CharOffset::from(5),
                selection.clone(),
                ctx,
            );
            buffer.edit_internal("", TextStyles::default(), selection.clone(), ctx);
            assert_eq!(buffer.text().as_str(), "tein");
        });

        selection.read(&app, |selection, _| {
            assert_eq!(selection.resolve_anchor(&end), Some(5.into()));
        });

        // If there are multiple anchors, all stay in sync.
        let middle = selection.update(&mut app, |selection, ctx| selection.anchor(2.into(), ctx));

        buffer.update(&mut app, |buffer, ctx| {
            buffer.set_selection(
                CharOffset::from(1)..CharOffset::from(1),
                selection.clone(),
                ctx,
            );
            buffer.edit_internal("pro", TextStyles::default(), selection.clone(), ctx);
            assert_eq!(buffer.text().as_str(), "protein");
        });

        selection.read(&app, |selection, _| {
            assert_eq!(selection.resolve_anchor(&middle), Some(5.into()));
            assert_eq!(selection.resolve_anchor(&end), Some(8.into()));
        });

        buffer.update(&mut app, |buffer, ctx| {
            // Anchors may be invalidated by deletion.
            buffer.set_selection(
                CharOffset::from(4)..CharOffset::from(6),
                selection.clone(),
                ctx,
            );
            buffer.edit_internal("", TextStyles::default(), selection.clone(), ctx);
            assert_eq!(buffer.text().as_str(), "proin");
        });

        selection.read(&app, |selection, _| {
            assert_eq!(selection.resolve_anchor(&middle), None);
            assert_eq!(selection.resolve_anchor(&end), Some(6.into()));
        });
    });
}

#[test]
fn test_replace_all_with_anchors() {
    App::test((), |mut app| async move {
        let buffer = app.add_model(|_| Buffer::new(Box::new(|_, _| IndentBehavior::Ignore)));
        let selection = app.add_model(|ctx| {
            ctx.subscribe_to_model(
                &buffer,
                |selection: &mut BufferSelectionModel, _, event, _| {
                    if let BufferEvent::AnchorUpdated { update, .. } = event {
                        selection.update_anchors(update.clone());
                    }
                },
            );
            BufferSelectionModel::new(buffer.clone())
        });

        buffer.update(&mut app, |buffer, ctx| {
            buffer.edit_internal(
                "Hello world.\nGoodbye world.",
                TextStyles::default(),
                selection.clone(),
                ctx,
            );
            assert_eq!(buffer.text().as_str(), "Hello world.\nGoodbye world.");
        });

        let anchor = selection.update(&mut app, |selection, ctx| selection.anchor(14.into(), ctx));

        selection.read(&app, |selection, _| {
            assert_eq!(selection.resolve_anchor(&anchor), Some(14.into()));
        });

        // Second anchor at the start of the buffer.
        let anchor_2 = selection.update(&mut app, |selection, ctx| selection.anchor(1.into(), ctx));

        selection.read(&app, |selection, _| {
            assert_eq!(selection.resolve_anchor(&anchor_2), Some(1.into()));
        });

        // Third anchor at the end of the buffer.
        let anchor_3 =
            selection.update(&mut app, |selection, ctx| selection.anchor(28.into(), ctx));

        selection.read(&app, |selection, _| {
            assert_eq!(selection.resolve_anchor(&anchor_3), Some(28.into()));
        });

        buffer.update(&mut app, |buffer, ctx| {
            buffer.replace_all("Howdy.\nGoodbye world.", ctx);
            assert_eq!(buffer.text().as_str(), "Howdy.\nGoodbye world.");
        });

        // Validate the buffer state.
        selection.read(&app, |selection, _| {
            buffer.read(&app, |buffer, _| {
                buffer.validate(&selection.anchors);
            });
        });
    });
}

#[test]
fn test_replace_with_anchors() {
    App::test((), |mut app| async move {
        let buffer = app.add_model(|_| Buffer::new(Box::new(|_, _| IndentBehavior::Ignore)));
        let selection = app.add_model(|_| BufferSelectionModel::new(buffer.clone()));

        buffer.update(&mut app, |buffer, ctx| {
            buffer.edit_internal("test", TextStyles::default(), selection.clone(), ctx);
        });

        let anchor = selection.update(&mut app, |selection, ctx| selection.anchor(3.into(), ctx));

        buffer.update(&mut app, |buffer, ctx| {
            // Replace on top of the anchor, with a net increase in character count.
            buffer.set_selection(
                CharOffset::from(2)..CharOffset::from(4),
                selection.clone(),
                ctx,
            );
            buffer.edit_internal("oas", TextStyles::default(), selection.clone(), ctx);
            assert_eq!(buffer.text().as_str(), "toast");
        });

        selection.read(&app, |selection, _| {
            assert_eq!(selection.resolve_anchor(&anchor), Some(3.into()));
        });

        buffer.update(&mut app, |buffer, ctx| {
            // Now, replace on top of the anchor with a net decrease in character count.
            buffer.set_selection(
                CharOffset::from(1)..CharOffset::from(4),
                selection.clone(),
                ctx,
            );
            buffer.edit_internal("p", TextStyles::default(), selection.clone(), ctx);
            assert_eq!(buffer.text().as_str(), "pst");
        });

        selection.read(&app, |selection, _| {
            assert_eq!(selection.resolve_anchor(&anchor), None);
        });
    });
}

#[test]
fn test_edit_delta_ranges() {
    // This tests that editing returns the correct modification ranges, which is
    // important for updating the rendering model.
    App::test((), |mut app| async move {
        let buffer = app.add_model(|_| Buffer::new(Box::new(|_, _| IndentBehavior::Ignore)));
        let selection = app.add_model(|_| BufferSelectionModel::new(buffer.clone()));

        buffer.update(&mut app, |buffer, ctx| {
            let delta = buffer
                .edit_internal("Test", Default::default(), selection.clone(), ctx)
                .delta
                .expect("Edit range should exist");

            // This is a fresh insertion, so the range in the original content is zero-length.
            assert_eq!(delta.old_offset, CharOffset::from(1)..CharOffset::from(1));

            // Inserting into the middle of a line modifies the whole line.
            buffer.set_selection(
                CharOffset::from(3)..CharOffset::from(3),
                selection.clone(),
                ctx,
            );
            let delta = buffer
                .edit_internal("x", Default::default(), selection.clone(), ctx)
                .delta
                .expect("Edit range should exist");
            assert_eq!(buffer.text().as_str(), "Texst");
            assert_eq!(delta.old_offset, CharOffset::from(1)..CharOffset::from(5));

            // A newline modifies both the line before and the line after it.
            buffer.set_selection(
                CharOffset::from(6)..CharOffset::from(6),
                selection.clone(),
                ctx,
            );
            let delta = buffer
                .edit_internal("\n", Default::default(), selection.clone(), ctx)
                .delta
                .expect("Edit range should exist");
            assert_eq!(buffer.text().as_str(), "Texst\n");
            assert_eq!(delta.old_offset, CharOffset::from(1)..CharOffset::from(6));

            // Typing at the start of a line, however, does not modify the previous line.
            // Note that in both this case and the newline one, the original edit range
            // is the very end of the buffer. However, the affected range for rendering is
            // quite different.
            buffer.set_selection(
                CharOffset::from(7)..CharOffset::from(7),
                selection.clone(),
                ctx,
            );
            let delta = buffer
                .edit_internal("Hi", Default::default(), selection.clone(), ctx)
                .delta
                .expect("Edit range should exist");
            assert_eq!(buffer.text().as_str(), "Texst\nHi");
            assert_eq!(delta.old_offset, CharOffset::from(7)..CharOffset::from(7));

            // A newline that splits an existing line modifies just that line, even though
            // it creates two new ones.
            buffer.set_selection(
                CharOffset::from(3)..CharOffset::from(3),
                selection.clone(),
                ctx,
            );
            let delta = buffer
                .edit_internal("\n", Default::default(), selection.clone(), ctx)
                .delta
                .expect("Edit range should exist");
            assert_eq!(buffer.text().as_str(), "Te\nxst\nHi");
            assert_eq!(delta.old_offset, CharOffset::from(1)..CharOffset::from(7));
            assert_eq!(delta.new_lines.len(), 2);

            // Deleting that same newline replaces the two lines with one.
            buffer.set_selection(
                CharOffset::from(3)..CharOffset::from(4),
                selection.clone(),
                ctx,
            );
            let delta = buffer
                .edit_internal("", Default::default(), selection.clone(), ctx)
                .delta
                .expect("Edit range should exist");
            assert_eq!(buffer.text().as_str(), "Texst\nHi");
            assert_eq!(delta.old_offset, CharOffset::from(1)..CharOffset::from(8));
            assert_eq!(delta.new_lines.len(), 1);
        });
    });
}

#[test]
fn test_buffer_line_metadata() {
    App::test((), |mut app| async move {
        let buffer = app.add_model(|_| Buffer::new(Box::new(|_, _| IndentBehavior::Ignore)));
        let selection = app.add_model(|_| BufferSelectionModel::new(buffer.clone()));

        buffer.update(&mut app, |buffer, ctx| {
            let target_text = "test\nhello\nword";
            let _ = buffer.edit_internal_first_selection(
                CharOffset::from(1)..CharOffset::from(1),
                target_text,
                TextStyles::default(),
                selection.clone(),
                ctx,
            );

            assert_eq!(
                buffer.len(),
                CharOffset::from(target_text.chars().count()) + 1
            );
            assert_eq!(buffer.max_point(), Point::new(3, 4));
            assert_eq!(buffer.line_len(1), 4);
            assert_eq!(buffer.line_len(2), 5);
            assert_eq!(buffer.line_len(3), 4);
        });
    });
}

#[test]
fn test_random() {
    // Use a fixed seed for stable output.
    let mut rng = StdRng::seed_from_u64(123456);
    let buffer = Buffer::random(&mut rng, 50);
    assert!(buffer.len().as_usize() <= 50);
    assert_eq!(
        buffer.text().as_str(),
        "d3\nSOf\ngZjvGHqkBxl2583x69F13\n\n8wlTivQFFQ9cY"
    );
    assert_eq!(
        buffer.content.debug(),
        "<text>d3\\nSOf\\ngZjvGHqkBxl25<c_s>83x69F<i_s>13\\n\\n8wlTivQFFQ9cY<i_e><c_e>"
    );
}

#[test]
fn test_edit_delta() {
    App::test((), |mut app| async move {
        let buffer = app.add_model(|_| Buffer::new(Box::new(|_, _| IndentBehavior::Ignore)));
        let selection = app.add_model(|_| BufferSelectionModel::new(buffer.clone()));

        buffer.update(&mut app, |buffer, ctx| {
            // Insert content into an empty buffer. EditDelta should return all the inserted
            // stylized text runs.
            let delta = buffer
                .edit_internal("te\nst", TextStyles::default(), selection.clone(), ctx)
                .delta
                .expect("Should exist");
            assert_eq!(buffer.content.debug(), "<text>te\\nst");
            assert_eq!(
                delta.precise_deltas,
                vec![PreciseDelta {
                    replaced_range: CharOffset::from(1)..CharOffset::from(1),
                    replaced_points: Point::new(1, 0)..Point::new(1, 0),
                    replaced_byte_range: ByteOffset::from(1)..ByteOffset::from(1),
                    new_byte_length: 5,
                    new_end_point: Point::new(2, 2),
                    resolved_range: CharOffset::from(1)..CharOffset::from(6),
                }]
            );
            assert_eq!(
                (*delta.new_lines),
                vec![
                    StyledBufferBlock::Text(StyledTextBlock {
                        block: vec![StyledBufferRun {
                            run: "te\n".to_string(),
                            text_styles: TextStylesWithMetadata::default(),
                            block_style: BufferBlockStyle::PlainText
                        },],
                        style: BufferBlockStyle::PlainText,
                        content_length: CharOffset::from(3)
                    }),
                    StyledBufferBlock::Text(StyledTextBlock {
                        block: vec![StyledBufferRun {
                            run: "st".to_string(),
                            text_styles: TextStylesWithMetadata::default(),
                            block_style: BufferBlockStyle::PlainText
                        },],
                        style: BufferBlockStyle::PlainText,
                        content_length: CharOffset::from(2)
                    })
                ]
            );

            // Remove a newline character to collapse two lines into one. EditDelta should return
            // the collapsed line.
            buffer.set_selection(
                CharOffset::from(2)..CharOffset::from(4),
                selection.clone(),
                ctx,
            );
            let delta = buffer
                .edit_internal("n", TextStyles::default(), selection.clone(), ctx)
                .delta
                .expect("Should exist");
            assert_eq!(buffer.content.debug(), "<text>tnst");
            assert_eq!(
                delta.precise_deltas,
                vec![PreciseDelta {
                    replaced_range: CharOffset::from(2)..CharOffset::from(4),
                    replaced_points: Point::new(1, 1)..Point::new(2, 0),
                    replaced_byte_range: ByteOffset::from(2)..ByteOffset::from(4),
                    new_byte_length: 1,
                    new_end_point: Point::new(1, 2),
                    resolved_range: CharOffset::from(2)..CharOffset::from(3),
                }]
            );
            assert_eq!(
                (*delta.new_lines),
                vec![StyledBufferBlock::Text(StyledTextBlock {
                    block: vec![StyledBufferRun {
                        run: "tnst".to_string(),
                        text_styles: TextStylesWithMetadata::default(),
                        block_style: BufferBlockStyle::PlainText
                    },],
                    style: BufferBlockStyle::PlainText,
                    content_length: CharOffset::from(4)
                }),]
            );

            // Insert multiple lines into the buffer. EditDelta should return all the inserted line.
            buffer.set_selection(
                CharOffset::from(2)..CharOffset::from(4),
                selection.clone(),
                ctx,
            );
            let delta = buffer
                .edit_internal("n\n\n", TextStyles::default(), selection.clone(), ctx)
                .delta
                .expect("Should exist");
            assert_eq!(buffer.content.debug(), "<text>tn\\n\\nt");
            assert_eq!(
                delta.precise_deltas,
                vec![PreciseDelta {
                    replaced_range: CharOffset::from(2)..CharOffset::from(4),
                    replaced_points: Point::new(1, 1)..Point::new(1, 3),
                    replaced_byte_range: ByteOffset::from(2)..ByteOffset::from(4),
                    new_byte_length: 3,
                    new_end_point: Point::new(3, 0),
                    resolved_range: CharOffset::from(2)..CharOffset::from(5),
                }]
            );
            assert_eq!(
                (*delta.new_lines),
                vec![
                    StyledBufferBlock::Text(StyledTextBlock {
                        block: vec![StyledBufferRun {
                            run: "tn\n".to_string(),
                            text_styles: TextStylesWithMetadata::default(),
                            block_style: BufferBlockStyle::PlainText
                        },],
                        style: BufferBlockStyle::PlainText,
                        content_length: CharOffset::from(3)
                    }),
                    StyledBufferBlock::Text(StyledTextBlock {
                        block: vec![StyledBufferRun {
                            run: "\n".to_string(),
                            text_styles: TextStylesWithMetadata::default(),
                            block_style: BufferBlockStyle::PlainText
                        },],
                        style: BufferBlockStyle::PlainText,
                        content_length: CharOffset::from(1)
                    }),
                    StyledBufferBlock::Text(StyledTextBlock {
                        block: vec![StyledBufferRun {
                            run: "t".to_string(),
                            text_styles: TextStylesWithMetadata::default(),
                            block_style: BufferBlockStyle::PlainText
                        },],
                        style: BufferBlockStyle::PlainText,
                        content_length: CharOffset::from(1)
                    }),
                ]
            );

            // Change the content of one line. EditDelta should only return the updated line.
            buffer.set_selection(
                CharOffset::from(2)..CharOffset::from(3),
                selection.clone(),
                ctx,
            );
            let delta = buffer
                .edit_internal("s", TextStyles::default(), selection.clone(), ctx)
                .delta
                .expect("Should exist");
            assert_eq!(buffer.content.debug(), "<text>ts\\n\\nt");
            assert_eq!(
                delta.precise_deltas,
                vec![PreciseDelta {
                    replaced_range: CharOffset::from(2)..CharOffset::from(3),
                    replaced_points: Point::new(1, 1)..Point::new(1, 2),
                    replaced_byte_range: ByteOffset::from(2)..ByteOffset::from(3),
                    new_byte_length: 1,
                    new_end_point: Point::new(1, 2),
                    resolved_range: CharOffset::from(2)..CharOffset::from(3),
                }]
            );
            assert_eq!(
                (*delta.new_lines),
                vec![StyledBufferBlock::Text(StyledTextBlock {
                    block: vec![StyledBufferRun {
                        run: "ts\n".to_string(),
                        text_styles: TextStylesWithMetadata::default(),
                        block_style: BufferBlockStyle::PlainText
                    },],
                    style: BufferBlockStyle::PlainText,
                    content_length: CharOffset::from(3)
                }),]
            );

            // Edit just the middle line.
            buffer.set_selection(
                CharOffset::from(4)..CharOffset::from(4),
                selection.clone(),
                ctx,
            );
            let delta = buffer
                .edit_internal("hi", TextStyles::default(), selection.clone(), ctx)
                .delta
                .expect("Should exist");
            assert_eq!(buffer.content.debug(), "<text>ts\\nhi\\nt");
            assert_eq!(
                delta.precise_deltas,
                vec![PreciseDelta {
                    replaced_range: CharOffset::from(4)..CharOffset::from(4),
                    replaced_points: Point::new(2, 0)..Point::new(2, 0),
                    replaced_byte_range: ByteOffset::from(4)..ByteOffset::from(4),
                    new_byte_length: 2,
                    new_end_point: Point::new(2, 2),
                    resolved_range: CharOffset::from(4)..CharOffset::from(6),
                }]
            );
            assert_eq!(
                (*delta.new_lines),
                vec![StyledBufferBlock::Text(StyledTextBlock {
                    block: vec![StyledBufferRun {
                        run: "hi\n".to_string(),
                        text_styles: TextStylesWithMetadata::default(),
                        block_style: BufferBlockStyle::PlainText
                    },],
                    style: BufferBlockStyle::PlainText,
                    content_length: CharOffset::from(3)
                }),]
            );
        });
    });
}

#[test]
fn test_multi_delta_stale_replaced_range() {
    // Demonstrates that when apply_core_edit_actions processes multiple actions where a
    // later action is at a lower buffer offset, the earlier action's replaced_range becomes
    // stale in the final buffer's coordinate system. This causes incorrect text when reading
    // replacement content from the final buffer (as notify_lsp_of_content_change does).
    App::test((), |mut app| async move {
        let buffer = app.add_model(|_| Buffer::new(Box::new(|_, _| IndentBehavior::Ignore)));
        let selection = app.add_model(|_| BufferSelectionModel::new(buffer.clone()));

        buffer.update(&mut app, |buffer, ctx| {
            // Set up buffer: "AABBCCEE" (8 chars at CharOffset 1..9)
            let _ = buffer.edit_internal("AABBCCEE", TextStyles::default(), selection.clone(), ctx);
            assert_eq!(buffer.text().as_str(), "AABBCCEE");

            // Apply two actions in reverse positional order (high offset first):
            //   Action 0: Replace "CC" (offset 5..7) with "DDDD" (+2 chars)
            //   Action 1: Replace "AA" (offset 1..3) with "X"  (-1 char)
            //
            // After action 0: buffer = "AABBDDDDEE"
            // After action 1: buffer = "XBBDDDDEE"  (final)
            let plain = |text: &str| {
                super::convert_text_with_style_to_formatted_text(text, TextStyles::default())
            };
            let actions = vec![
                CoreEditorAction::new(
                    CharOffset::from(5)..CharOffset::from(7),
                    CoreEditorActionType::Insert {
                        text: plain("DDDD"),
                        source: EditOrigin::UserInitiated,

                        insert_on_selection: true,
                    },
                ),
                CoreEditorAction::new(
                    CharOffset::from(1)..CharOffset::from(3),
                    CoreEditorActionType::Insert {
                        text: plain("X"),
                        source: EditOrigin::UserInitiated,

                        insert_on_selection: true,
                    },
                ),
            ];

            let result = buffer.apply_core_edit_actions(actions);
            let delta = result.delta.expect("Should have delta");

            assert_eq!(buffer.text().as_str(), "XBBDDDDEE");
            assert_eq!(delta.precise_deltas.len(), 2);

            // Delta 0 replaced "CC" at offset 5..7 with "DDDD" (4 chars).
            let delta0 = &delta.precise_deltas[0];
            assert_eq!(
                delta0.replaced_range,
                CharOffset::from(5)..CharOffset::from(7)
            );
            assert_eq!(delta0.resolved_length(), CharOffset::from(4));

            // Delta 1 replaced "AA" at offset 1..3 with "X" (1 char).
            let delta1 = &delta.precise_deltas[1];
            assert_eq!(
                delta1.replaced_range,
                CharOffset::from(1)..CharOffset::from(3)
            );
            assert_eq!(delta1.resolved_length(), CharOffset::from(1));

            // BUG: The stale replaced_range for delta 0 points to the wrong location
            // in the final buffer.
            //
            // Delta 0's replaced_range.start (5) is in the post-delta-0 coordinate system.
            // After delta 1 removed 1 char before it, "DDDD" shifted to offset 4..8 in the
            // final buffer. But the stale computation reads from 5..9:
            //
            //   Final buffer "XBBDDDDEE":
            //     offset 4: D, 5: D, 6: D, 7: D, 8: E, 9: E
            //   text_in_range(5..9) = "DDDE" -- WRONG, should be "DDDD"
            let stale_range =
                delta0.replaced_range.start..delta0.replaced_range.start + delta0.resolved_length();
            let stale_text = buffer.text_in_range(stale_range).into_string();
            assert_eq!(
                stale_text, "DDDE",
                "Stale replaced_range reads wrong text from the final buffer"
            );

            // resolved_range gives the correct range in the final buffer (4..8).
            assert_eq!(
                delta0.resolved_range,
                CharOffset::from(4)..CharOffset::from(8)
            );
            let correct_text = buffer
                .text_in_range(delta0.resolved_range.clone())
                .into_string();
            assert_eq!(
                correct_text, "DDDD",
                "resolved_range reads correct text from the final buffer"
            );

            // Delta 1 is the last action applied, so its resolved_range matches
            // the simple computation (replaced_range.start + resolved_length).
            assert_eq!(
                delta1.resolved_range,
                CharOffset::from(1)..CharOffset::from(2)
            );
            let delta1_text = buffer
                .text_in_range(delta1.resolved_range.clone())
                .into_string();
            assert_eq!(delta1_text, "X");
        });
    });
}

#[test]
fn test_selection_movement() {
    App::test((), |mut app| async move {
        let buffer = app.add_model(|_| Buffer::new(Box::new(|_, _| IndentBehavior::Ignore)));
        let selection = app.add_model(|_| BufferSelectionModel::new(buffer.clone()));

        buffer.update(&mut app, |buffer, ctx| {
            let _ = buffer.edit_internal_first_selection(
                CharOffset::from(1)..CharOffset::from(1),
                "te\nst",
                TextStyles::default(),
                selection.clone(),
                ctx,
            );
            assert_eq!(buffer.content.debug(), "<text>te\\nst");
            assert_eq!(
                selection.as_ref(ctx).selection_to_first_offset_range(),
                CharOffset::from(6)..CharOffset::from(6)
            );

            // This should be no-op.
            buffer.update_selection(
                selection.clone(),
                BufferSelectAction::MoveRight,
                AutoScrollBehavior::Selection,
                ctx,
            );
            assert_eq!(
                selection.as_ref(ctx).selection_to_first_offset_range(),
                CharOffset::from(6)..CharOffset::from(6)
            );

            buffer.update_selection(
                selection.clone(),
                BufferSelectAction::MoveLeft,
                AutoScrollBehavior::Selection,
                ctx,
            );
            assert_eq!(
                selection.as_ref(ctx).selection_to_first_offset_range(),
                CharOffset::from(5)..CharOffset::from(5)
            );

            buffer.extend_selection_left(2, selection.clone(), ctx);
            assert_eq!(
                selection.as_ref(ctx).selection_to_first_offset_range(),
                CharOffset::from(3)..CharOffset::from(5)
            );

            // The head of selection is 3. Extending it right by 3 should move the head to 6.
            buffer.extend_selection_right(3, selection.clone(), ctx);
            assert_eq!(
                selection.as_ref(ctx).selection_to_first_offset_range(),
                CharOffset::from(5)..CharOffset::from(6)
            );

            buffer.update_selection(
                selection.clone(),
                BufferSelectAction::MoveRight,
                AutoScrollBehavior::Selection,
                ctx,
            );
            assert_eq!(
                selection.as_ref(ctx).selection_to_first_offset_range(),
                CharOffset::from(6)..CharOffset::from(6)
            );

            buffer.set_selection(
                CharOffset::from(1)..CharOffset::from(3),
                selection.clone(),
                ctx,
            );
            buffer.update_selection(
                selection.clone(),
                BufferSelectAction::MoveLeft,
                AutoScrollBehavior::Selection,
                ctx,
            );
            assert_eq!(
                selection.as_ref(ctx).selection_to_first_offset_range(),
                CharOffset::from(1)..CharOffset::from(1)
            );

            selection.update(ctx, |selection, _| {
                set_selections(selection, vec1![1..1]);
            });
            // Should be no-op.
            buffer.update_selection(
                selection.clone(),
                BufferSelectAction::MoveLeft,
                AutoScrollBehavior::Selection,
                ctx,
            );
            assert_eq!(
                selection.as_ref(ctx).selection_to_first_offset_range(),
                CharOffset::from(1)..CharOffset::from(1)
            );

            buffer.update_selection(
                selection.clone(),
                BufferSelectAction::MoveRight,
                AutoScrollBehavior::Selection,
                ctx,
            );
            assert_eq!(
                selection.as_ref(ctx).selection_to_first_offset_range(),
                CharOffset::from(2)..CharOffset::from(2)
            );
        });
    });
}

#[test]
fn test_active_selection_style() {
    App::test((), |mut app| async move {
        let buffer = app.add_model(|_| Buffer::new(Box::new(|_, _| IndentBehavior::Ignore)));
        let selection = app.add_model(|_| BufferSelectionModel::new(buffer.clone()));

        buffer.update(&mut app, |buffer, ctx| {
            let _ = buffer.edit_internal_first_selection(
                CharOffset::from(1)..CharOffset::from(1),
                "te\nst",
                TextStyles::default(),
                selection.clone(),
                ctx,
            );
            assert_eq!(buffer.content.debug(), "<text>te\\nst");
            assert_eq!(
                selection.as_ref(ctx).selection_to_first_offset_range(),
                CharOffset::from(6)..CharOffset::from(6)
            );

            buffer.extend_selection_left(2, selection.clone(), ctx);
            assert_eq!(
                selection.as_ref(ctx).selection_to_first_offset_range(),
                CharOffset::from(4)..CharOffset::from(6)
            );
            let text_style: TextStyles = buffer
                .active_style_with_metadata_at_selection(selection.as_ref(ctx))
                .into();
            assert_eq!(text_style, TextStyles::default());

            // When selection is a range of characters, active style should be the style of the first character.
            buffer.apply_formatted_edit_fixture(
                CharOffset::zero()..buffer.max_charoffset(),
                VecDeque::from(vec![
                    EditorLine::Line(vec![FormattedTextFragment::plain_text("te")]),
                    EditorLine::Line(vec![FormattedTextFragment::bold("st")]),
                ]),
                EditOrigin::SystemEdit,
                selection.clone(),
                ctx,
            );
            selection.update(ctx, |selection, _| {
                selection.set_selection_offsets(vec1![SelectionOffsets {
                    head: 4.into(),
                    tail: 6.into(),
                }]);
            });
            let text_style: TextStyles = buffer
                .active_style_with_metadata_at_selection(selection.as_ref(ctx))
                .into();
            assert_eq!(text_style, TextStyles::default().bold());

            // When selection is a single cursor, active style should be the style of the character before the cursor.
            buffer.update_selection(
                selection.clone(),
                BufferSelectAction::MoveLeft,
                AutoScrollBehavior::Selection,
                ctx,
            );
            assert_eq!(
                selection.as_ref(ctx).selection_to_first_offset_range(),
                CharOffset::from(4)..CharOffset::from(4)
            );
            let text_style: TextStyles = buffer
                .active_style_with_metadata_at_selection(selection.as_ref(ctx))
                .into();
            assert_eq!(text_style, TextStyles::default());

            // When there are multiple selections, active style should be the common style of all selections, if any.
            assert_eq!(buffer.content.debug(), "<text>te\\n<b_s>st<b_e>");

            buffer.add_cursor(5.into(), true, selection.clone(), ctx);
            buffer.add_cursor(1.into(), false, selection.clone(), ctx);
            let text_style: TextStyles = buffer
                .active_style_with_metadata_at_selection(selection.as_ref(ctx))
                .into();
            assert_eq!(text_style, TextStyles::default());

            buffer.add_cursor(5.into(), true, selection.clone(), ctx);
            buffer.add_cursor(6.into(), false, selection.clone(), ctx);
            let text_style: TextStyles = buffer
                .active_style_with_metadata_at_selection(selection.as_ref(ctx))
                .into();
            assert_eq!(text_style, TextStyles::default().bold());
        });
    });
}

#[test]
fn test_containing_offset() {
    App::test((), |mut app| async move {
        let buffer = app.add_model(|_| Buffer::new(Box::new(|_, _| IndentBehavior::Ignore)));
        let selection = app.add_model(|_| BufferSelectionModel::new(buffer.clone()));

        buffer.update(&mut app, |buffer, ctx| {
            buffer.edit_internal_first_selection(
                CharOffset::from(1)..CharOffset::from(1),
                "first\nsecond\nthird",
                Default::default(),
                selection.clone(),
                ctx,
            );

            for (offset, expected_start, expected_end) in vec![
                // Extremes of the buffer should still behave as expected.
                (1, 1, 7),
                (19, 14, 20),
                // Likewise, we can seek from the start of each line and from its last character.
                (6, 1, 7),    // First newline.
                (7, 7, 14),   // First character of second line.
                (13, 7, 14),  // Second newline.
                (14, 14, 20), // First character of last line.
                // We can also seek to the boundaries from within a line.
                (4, 1, 7),
                (9, 7, 14),
                (16, 14, 20),
                // If out of bounds, we should clamp to the last line.
                (21, 14, 20),
            ] {
                let offset = CharOffset::from(offset);
                let expected_start = CharOffset::from(expected_start);
                let expected_end = CharOffset::from(expected_end);
                let start = buffer.containing_line_start(offset);
                assert_eq!(
            start, expected_start,
            "Expected line containing {offset} to start at {expected_start}, but got {start}"
        );

                let end = buffer.containing_line_end(offset);
                assert_eq!(
                    end, expected_end,
                    "Expected line containing {offset} to end at {expected_end}, but got {end}"
                );
            }
        });
    });
}

#[test]
fn test_containing_line_first_nonwhitespace_for_plain_text() {
    App::test((), |mut app| async move {
        let buffer = app.add_model(|_| Buffer::new(Box::new(|_, _| IndentBehavior::Ignore)));
        let selection = app.add_model(|_| BufferSelectionModel::new(buffer.clone()));

        buffer.update(&mut app, |buffer, ctx| {
            buffer.edit_internal_first_selection(
                CharOffset::from(1)..CharOffset::from(1),
                "  first\n\t second\n   ",
                Default::default(),
                selection,
                ctx,
            );

            assert_eq!(
                buffer.containing_line_first_nonwhitespace(CharOffset::from(1)),
                CharOffset::from(3)
            );
            assert_eq!(
                buffer.containing_line_first_nonwhitespace(CharOffset::from(9)),
                CharOffset::from(11)
            );
            assert_eq!(
                buffer.containing_line_first_nonwhitespace(CharOffset::from(18)),
                CharOffset::from(18)
            );
        });
    });
}

#[test]
fn test_content_from_block_start_to_selection_start() {
    App::test((), |mut app| async move {
        let buffer = app.add_model(|_| Buffer::new(Box::new(|_, _| IndentBehavior::Ignore)));
        let selection = app.add_model(|_| BufferSelectionModel::new(buffer.clone()));

        buffer.update(&mut app, |buffer, ctx| {
            let _ = buffer.edit_internal_first_selection(
                CharOffset::from(1)..CharOffset::from(1),
                "te\nst",
                TextStyles::default(),
                selection.clone(),
                ctx,
            );

            // Cursor is at the end of the buffer -- should return "st" from the second line.
            assert_eq!(
                buffer.content_from_block_start_to_selection_start(
                    selection
                        .as_ref(ctx)
                        .selection_to_first_offset_range()
                        .start
                ),
                "st".to_string()
            );

            // Cursor is at the end of the first line -- should return "te".
            selection.update(ctx, |selection, _| {
                set_selections(selection, vec1![3..3]);
            });
            assert_eq!(
                buffer.content_from_block_start_to_selection_start(
                    selection
                        .as_ref(ctx)
                        .selection_to_first_offset_range()
                        .start
                ),
                "te".to_string()
            );

            // Cursor is in the middle of the first line -- should return "t".
            selection.update(ctx, |selection, _| {
                set_selections(selection, vec1![2..2]);
            });
            assert_eq!(
                buffer.content_from_block_start_to_selection_start(
                    selection
                        .as_ref(ctx)
                        .selection_to_first_offset_range()
                        .start
                ),
                "t".to_string()
            );

            // Cursor is in the beginning of the second line -- should return empty string.
            selection.update(ctx, |selection, _| {
                set_selections(selection, vec1![4..4]);
            });
            assert_eq!(
                buffer.content_from_block_start_to_selection_start(
                    selection
                        .as_ref(ctx)
                        .selection_to_first_offset_range()
                        .start
                ),
                "".to_string()
            );

            // Cursor is in the beginning of the buffer -- should return empty string.
            selection.update(ctx, |selection, _| {
                set_selections(selection, vec1![1..1]);
            });
            assert_eq!(
                buffer.content_from_block_start_to_selection_start(
                    selection
                        .as_ref(ctx)
                        .selection_to_first_offset_range()
                        .start
                ),
                "".to_string()
            );
        });
    });
}

#[test]
fn test_styled_runs_multiple_styles() {
    // This tests that we split nested/overlapping style runs correctly.
    App::test((), |mut app| async move {
        let buffer = app.add_model(|_| Buffer::new(Box::new(|_, _| IndentBehavior::Ignore)));
        let selection = app.add_model(|_| BufferSelectionModel::new(buffer.clone()));

        buffer.update(&mut app, |buffer, ctx| {
            buffer.apply_formatted_edit_fixture(
                CharOffset::zero()..buffer.max_charoffset(),
                VecDeque::from(vec![EditorLine::Line(vec![
                    FormattedTextFragment::plain_text("he"),
                    FormattedTextFragment::bold("llo "),
                    FormattedTextFragment::bold_italic("wo"),
                    FormattedTextFragment::italic("rld"),
                ])]),
                EditOrigin::SystemEdit,
                selection.clone(),
                ctx,
            );
            selection.update(ctx, |selection, _| {
                set_selections(selection, vec1![7..12]);
            });

            assert_eq!(
                buffer.styled_blocks_in_range(
                    CharOffset::from(1)..buffer.max_charoffset(),
                    StyledBlockBoundaryBehavior::Exclusive
                ),
                vec![StyledBufferBlock::Text(StyledTextBlock {
                    block: vec![
                        StyledBufferRun {
                            run: "he".into(),
                            text_styles: Default::default(),
                            block_style: BufferBlockStyle::PlainText
                        },
                        StyledBufferRun {
                            run: "llo ".into(),
                            text_styles: TextStylesWithMetadata::default().bold(),
                            block_style: BufferBlockStyle::PlainText
                        },
                        StyledBufferRun {
                            run: "wo".into(),
                            text_styles: TextStylesWithMetadata::default().bold().italic(),
                            block_style: BufferBlockStyle::PlainText
                        },
                        StyledBufferRun {
                            run: "rld".into(),
                            text_styles: TextStylesWithMetadata::default().italic(),
                            block_style: BufferBlockStyle::PlainText
                        },
                    ],
                    style: BufferBlockStyle::PlainText,
                    content_length: CharOffset::from(11)
                })]
            );
        });
    });
}

#[test]
fn test_styled_runs_range() {
    // This tests that we apply the start and end offsets.
    App::test((), |mut app| async move {
        let buffer = app.add_model(|_| Buffer::new(Box::new(|_, _| IndentBehavior::Ignore)));
        let selection = app.add_model(|_| BufferSelectionModel::new(buffer.clone()));

        buffer.update(&mut app, |buffer, ctx| {
            buffer.apply_formatted_edit_fixture(
                CharOffset::zero()..buffer.max_charoffset(),
                VecDeque::from(vec![EditorLine::Line(vec![
                    FormattedTextFragment::plain_text("this "),
                    FormattedTextFragment::bold("is"),
                    FormattedTextFragment::plain_text(" a sentence"),
                ])]),
                EditOrigin::SystemEdit,
                selection.clone(),
                ctx,
            );
            selection.update(ctx, |selection, _| {
                set_selections(selection, vec1![6..8]);
            });

            assert_eq!(
                buffer.styled_blocks_in_range(
                    CharOffset::from(1)..CharOffset::from(7),
                    StyledBlockBoundaryBehavior::Exclusive
                ),
                vec![StyledBufferBlock::Text(StyledTextBlock {
                    block: vec![
                        StyledBufferRun {
                            run: "this ".into(),
                            text_styles: Default::default(),
                            block_style: BufferBlockStyle::PlainText
                        },
                        // This should be cut off by the upper limit.
                        StyledBufferRun {
                            run: "i".into(),
                            text_styles: TextStylesWithMetadata::default().bold(),
                            block_style: BufferBlockStyle::PlainText
                        }
                    ],
                    style: BufferBlockStyle::PlainText,
                    content_length: CharOffset::from(7)
                })]
            );

            assert_eq!(
                buffer.styled_blocks_in_range(
                    CharOffset::from(5)..buffer.max_charoffset(),
                    StyledBlockBoundaryBehavior::Exclusive
                ),
                vec![StyledBufferBlock::Text(StyledTextBlock {
                    block: vec![
                        // This should be cut off by the starting offset.
                        StyledBufferRun {
                            run: " ".into(),
                            text_styles: Default::default(),
                            block_style: BufferBlockStyle::PlainText
                        },
                        StyledBufferRun {
                            run: "is".into(),
                            text_styles: TextStylesWithMetadata::default().bold(),
                            block_style: BufferBlockStyle::PlainText
                        },
                        StyledBufferRun {
                            run: " a sentence".into(),
                            text_styles: Default::default(),
                            block_style: BufferBlockStyle::PlainText
                        },
                    ],
                    style: BufferBlockStyle::PlainText,
                    content_length: CharOffset::from(14)
                })]
            );
        });
    });
}

#[test]
fn test_styled_runs_splits_newlines() {
    // This tests that style runs are broken by newlines, even if the styling
    // itself continues.
    App::test((), |mut app| async move {
        let buffer = app.add_model(|_| Buffer::new(Box::new(|_, _| IndentBehavior::Ignore)));
        let selection = app.add_model(|_| BufferSelectionModel::new(buffer.clone()));

        buffer.update(&mut app, |buffer, ctx| {
            buffer.apply_formatted_edit_fixture(
                CharOffset::zero()..buffer.max_charoffset(),
                VecDeque::from(vec![EditorLine::Line(vec![
                    FormattedTextFragment::plain_text("he"),
                    FormattedTextFragment::italic("llo\nwo"),
                    FormattedTextFragment::plain_text("rld"),
                ])]),
                EditOrigin::SystemEdit,
                selection.clone(),
                ctx,
            );
            selection.update(ctx, |selection, _| {
                set_selections(selection, vec1![3..9]);
            });

            assert_eq!(
                buffer.styled_blocks_in_range(
                    CharOffset::from(2)..buffer.max_charoffset(),
                    StyledBlockBoundaryBehavior::Exclusive
                ),
                vec![
                    StyledBufferBlock::Text(StyledTextBlock {
                        block: vec![
                            StyledBufferRun {
                                run: "e".into(),
                                text_styles: Default::default(),
                                block_style: BufferBlockStyle::PlainText
                            },
                            StyledBufferRun {
                                run: "llo\n".into(),
                                text_styles: TextStylesWithMetadata::default().italic(),
                                block_style: BufferBlockStyle::PlainText
                            },
                        ],
                        style: BufferBlockStyle::PlainText,
                        content_length: CharOffset::from(5)
                    }),
                    StyledBufferBlock::Text(StyledTextBlock {
                        block: vec![
                            // This is italic, along with the previous run, but it split by the newline.
                            StyledBufferRun {
                                run: "wo".into(),
                                text_styles: TextStylesWithMetadata::default().italic(),
                                block_style: BufferBlockStyle::PlainText
                            },
                            StyledBufferRun {
                                run: "rld".into(),
                                text_styles: Default::default(),
                                block_style: BufferBlockStyle::PlainText
                            }
                        ],
                        style: BufferBlockStyle::PlainText,
                        content_length: CharOffset::from(5)
                    })
                ]
            );
        });
    });
}

#[test]
fn test_empty_plain_text_initialization_and_replace_all() {
    App::test((), |mut app| async move {
        let buffer = app.add_model(|_| Buffer::new(Box::new(|_, _| IndentBehavior::Ignore)));
        let selection = app.add_model(|ctx| {
            ctx.subscribe_to_model(
                &buffer,
                |selection: &mut BufferSelectionModel, _, event, _| {
                    if let BufferEvent::AnchorUpdated { update, .. } = event {
                        selection.update_anchors(update.clone());
                    }
                },
            );
            BufferSelectionModel::new(buffer.clone())
        });
        buffer.update(&mut app, |buffer, ctx| {
            *buffer = Buffer::from_plain_text(
                "",
                Box::new(|_, _| IndentBehavior::Ignore),
                selection.clone(),
                ctx,
            );
        });
        buffer.read(&app, |buffer, _| {
            assert_eq!(buffer.content.debug(), "<text>");
        });
        selection.read(&app, |selection, _| {
            assert_eq!(selection.first_selection_head(), CharOffset::from(1));
        });
        buffer.read(&app, |buffer, _| {
            selection.read(&app, |selection, _| {
                buffer.validate(&selection.anchors);
            });
        });

        let buffer = app.add_model(|_| Buffer::new(Box::new(|_, _| IndentBehavior::Ignore)));
        let selection = app.add_model(|ctx| {
            ctx.subscribe_to_model(
                &buffer,
                |selection: &mut BufferSelectionModel, _, event, _| {
                    if let BufferEvent::AnchorUpdated { update, .. } = event {
                        selection.update_anchors(update.clone());
                    }
                },
            );
            BufferSelectionModel::new(buffer.clone())
        });
        buffer.update(&mut app, |buffer, ctx| {
            *buffer = Buffer::from_plain_text(
                "has text",
                Box::new(|_, _| IndentBehavior::Ignore),
                selection.clone(),
                ctx,
            );
        });

        buffer.update(&mut app, |buffer, ctx| {
            buffer.replace_all("", ctx);

            assert_eq!(buffer.content.debug(), "<text>");
        });

        selection.read(&app, |selection, _| {
            assert_eq!(selection.first_selection_head(), CharOffset::from(1));
        });

        buffer.read(&app, |buffer, _| {
            selection.read(&app, |selection, _| {
                buffer.validate(&selection.anchors);
            });
        });
    });
}

#[test]
fn test_enter_at_starting_plain_text() {
    App::test((), |mut app| async move {
        let (buffer, selection) = Buffer::mock_from_plain_text(
            "example",
            Box::new(|_, _| IndentBehavior::Ignore),
            &mut app,
        );

        buffer.update(&mut app, |buffer, ctx| {
            // Enter at the start of the first line.
            selection.update(ctx, |selection, _| {
                set_selections(selection, vec1![1..1]);
            });
            let prev_selection = buffer.to_rendered_selection_set(selection.clone(), ctx);
            let result = buffer.enter(false, Default::default(), selection.clone(), ctx);
            let current_selection = buffer.to_rendered_selection_set(selection.clone(), ctx);
            buffer.push_undo_item(
                prev_selection,
                current_selection,
                result.undo_item.unwrap(),
                UndoActionType::Atomic,
            );

            // Because we're editing an existing line, it should be re-rendered along with the new one.
            let delta = result.delta.unwrap();
            assert_eq!(delta.old_offset, CharOffset::from(1)..CharOffset::from(8));
            assert_eq!(
                (*delta.new_lines),
                vec![
                    StyledBufferBlock::Text(StyledTextBlock {
                        block: vec![StyledBufferRun {
                            run: "\n".to_string(),
                            text_styles: Default::default(),
                            block_style: BufferBlockStyle::PlainText
                        }],
                        style: BufferBlockStyle::PlainText,
                        content_length: CharOffset::from(1)
                    }),
                    StyledBufferBlock::Text(StyledTextBlock {
                        block: vec![StyledBufferRun {
                            run: "example".to_string(),
                            text_styles: Default::default(),
                            block_style: BufferBlockStyle::PlainText
                        }],
                        style: BufferBlockStyle::PlainText,
                        content_length: CharOffset::from(7)
                    })
                ]
            );

            assert_eq!(buffer.debug(), "<text>\\nexample");

            buffer.undo(selection.clone(), ctx);
            assert_eq!(buffer.debug(), "<text>example");

            buffer.redo(selection.clone(), ctx);
            assert_eq!(buffer.debug(), "<text>\\nexample");
        });
    });
}

#[test]
fn test_enter_at_start_of_empty_text() {
    App::test((), |mut app| async move {
        let (buffer, selection) = Buffer::mock_from_plain_text(
            "list\n",
            Box::new(|_, _| IndentBehavior::Ignore),
            &mut app,
        );

        buffer.update(&mut app, |buffer, ctx| {
            assert_eq!(buffer.debug(), "<text>list\\n");

            // Enter at the start of the empty plain-text block.
            selection.update(ctx, |selection, _| {
                set_selections(selection, vec1![6..6]);
            });
            let prev_selection = buffer.to_rendered_selection_set(selection.clone(), ctx);
            let result = buffer.enter(false, Default::default(), selection.clone(), ctx);
            let current_selection = buffer.to_rendered_selection_set(selection.clone(), ctx);
            buffer.push_undo_item(
                prev_selection,
                current_selection,
                result.undo_item.unwrap(),
                UndoActionType::Atomic,
            );

            assert_eq!(buffer.debug(), "<text>list\\n\\n");

            buffer.undo(selection.clone(), ctx);
            assert_eq!(buffer.debug(), "<text>list\\n");

            buffer.redo(selection.clone(), ctx);
            assert_eq!(buffer.debug(), "<text>list\\n\\n");
        });
    });
}

#[test]
fn test_insert_formatted_text() {
    App::test((), |mut app| async move {
        let buffer = app.add_model(|_| Buffer::new(Box::new(|_, _| IndentBehavior::Ignore)));
        let selection = app.add_model(|_| BufferSelectionModel::new(buffer.clone()));

        buffer.update(&mut app, |buffer, ctx| {
            buffer.edit_internal_first_selection(
                CharOffset::from(1)..CharOffset::from(1),
                "Before\nBlock\nAfter",
                Default::default(),
                selection.clone(),
                ctx,
            );

            let formatted_text =
                VecDeque::from(vec![EditorLine::Line(vec![FormattedTextFragment::bold(
                    "test text",
                )])]);
            buffer.apply_formatted_edit_fixture(
                CharOffset::from(7)..CharOffset::from(7),
                formatted_text,
                EditOrigin::UserInitiated,
                selection.clone(),
                ctx,
            );
            assert_eq!(
                buffer.content.debug(),
                "<text>Before<b_s>test text<b_e>\\nBlock\\nAfter"
            );
        });
    });
}

#[test]
fn test_insert_formatted_text_empty_buffer() {
    App::test((), |mut app| async move {
        let buffer = app.add_model(|_| Buffer::new(Box::new(|_, _| IndentBehavior::Ignore)));
        let selection = app.add_model(|_| BufferSelectionModel::new(buffer.clone()));

        buffer.update(&mut app, |buffer, ctx| {
            let formatted_text = VecDeque::from([
                EditorLine::LineBreak,
                EditorLine::Line(vec![FormattedTextFragment::plain_text("block")]),
                EditorLine::LineBreak,
            ]);
            let delta = buffer
                .apply_formatted_edit_fixture(
                    CharOffset::from(0)..CharOffset::from(1),
                    formatted_text.clone(),
                    EditOrigin::UserInitiated,
                    selection.clone(),
                    ctx,
                )
                .delta
                .expect("Should exist");

            assert_eq!(buffer.content.debug(), "<text>\\nblock\\n");
            assert_eq!(delta.old_offset, CharOffset::from(0)..CharOffset::from(1));
            assert_eq!(
                (*delta.new_lines),
                vec![
                    StyledBufferBlock::Text(StyledTextBlock {
                        block: vec![StyledBufferRun {
                            run: "\n".into(),
                            text_styles: Default::default(),
                            block_style: BufferBlockStyle::PlainText
                        },],
                        style: BufferBlockStyle::PlainText,
                        content_length: CharOffset::from(1)
                    }),
                    StyledBufferBlock::Text(StyledTextBlock {
                        block: vec![StyledBufferRun {
                            run: "block\n".into(),
                            text_styles: Default::default(),
                            block_style: BufferBlockStyle::PlainText
                        },],
                        style: BufferBlockStyle::PlainText,
                        content_length: CharOffset::from(6)
                    }),
                ]
            );
        });
    });
}

#[test]
fn test_undo_redo_plain_text() {
    App::test((), |mut app| async move {
        let buffer = app.add_model(|_| Buffer::new(Box::new(|_, _| IndentBehavior::Ignore)));
        let selection = app.add_model(|_| BufferSelectionModel::new(buffer.clone()));

        buffer.update(&mut app, |buffer, ctx| {
            let prev_selection = buffer.to_rendered_selection_set(selection.clone(), ctx);
            let edit_result = buffer.edit_internal(
                "test\n\nline",
                TextStyles::default(),
                selection.clone(),
                ctx,
            );
            let curr_selection = buffer.to_rendered_selection_set(selection.clone(), ctx);
            buffer.push_undo_item(
                prev_selection,
                curr_selection,
                edit_result.undo_item.expect("Should exist"),
                UndoActionType::Atomic,
            );
            let delta = buffer
                .undo(selection.clone(), ctx)
                .delta
                .expect("Edit delta should exist");
            assert_eq!(buffer.content.debug(), "<text>");
            assert_eq!(delta.old_offset, CharOffset::from(1)..CharOffset::from(11));
            assert_eq!((*delta.new_lines), vec![]);

            let delta = buffer
                .redo(selection.clone(), ctx)
                .delta
                .expect("Edit delta should exist");
            assert_eq!(buffer.content.debug(), "<text>test\\n\\nline");
            assert_eq!(delta.old_offset, CharOffset::from(1)..CharOffset::from(1));
            assert_eq!(
                (*delta.new_lines),
                vec![
                    StyledBufferBlock::Text(StyledTextBlock {
                        block: vec![StyledBufferRun {
                            run: "test\n".into(),
                            text_styles: TextStylesWithMetadata::default(),
                            block_style: BufferBlockStyle::PlainText
                        }],
                        style: BufferBlockStyle::PlainText,
                        content_length: CharOffset::from(5)
                    }),
                    StyledBufferBlock::Text(StyledTextBlock {
                        block: vec![StyledBufferRun {
                            run: "\n".into(),
                            text_styles: TextStylesWithMetadata::default(),
                            block_style: BufferBlockStyle::PlainText
                        }],
                        style: BufferBlockStyle::PlainText,
                        content_length: CharOffset::from(1)
                    }),
                    StyledBufferBlock::Text(StyledTextBlock {
                        block: vec![StyledBufferRun {
                            run: "line".into(),
                            text_styles: TextStylesWithMetadata::default(),
                            block_style: BufferBlockStyle::PlainText
                        }],
                        style: BufferBlockStyle::PlainText,
                        content_length: CharOffset::from(4)
                    })
                ]
            );

            buffer.set_selection(
                CharOffset::from(3)..CharOffset::from(7),
                selection.clone(),
                ctx,
            );
            let prev_selection = buffer.to_rendered_selection_set(selection.clone(), ctx);
            let edit_result =
                buffer.edit_internal("", TextStyles::default(), selection.clone(), ctx);
            assert_eq!(buffer.content.debug(), "<text>teline");
            let curr_selection = buffer.to_rendered_selection_set(selection.clone(), ctx);
            buffer.push_undo_item(
                prev_selection,
                curr_selection,
                edit_result.undo_item.expect("Should exist"),
                UndoActionType::Atomic,
            );

            let delta = buffer
                .undo(selection.clone(), ctx)
                .delta
                .expect("Edit delta should exist");
            assert_eq!(buffer.content.debug(), "<text>test\\n\\nline");
            assert_eq!(delta.old_offset, CharOffset::from(1)..CharOffset::from(7));
            assert_eq!(
                (*delta.new_lines),
                vec![
                    StyledBufferBlock::Text(StyledTextBlock {
                        block: vec![StyledBufferRun {
                            run: "test\n".into(),
                            text_styles: TextStylesWithMetadata::default(),
                            block_style: BufferBlockStyle::PlainText
                        }],
                        style: BufferBlockStyle::PlainText,
                        content_length: CharOffset::from(5)
                    }),
                    StyledBufferBlock::Text(StyledTextBlock {
                        block: vec![StyledBufferRun {
                            run: "\n".into(),
                            text_styles: TextStylesWithMetadata::default(),
                            block_style: BufferBlockStyle::PlainText
                        }],
                        style: BufferBlockStyle::PlainText,
                        content_length: CharOffset::from(1)
                    }),
                    StyledBufferBlock::Text(StyledTextBlock {
                        block: vec![StyledBufferRun {
                            run: "line".into(),
                            text_styles: TextStylesWithMetadata::default(),
                            block_style: BufferBlockStyle::PlainText
                        }],
                        style: BufferBlockStyle::PlainText,
                        content_length: CharOffset::from(4)
                    })
                ]
            );

            let _ = buffer.undo(selection.clone(), ctx);
            assert_eq!(buffer.content.debug(), "<text>");

            let _ = buffer.redo(selection.clone(), ctx);
            assert_eq!(buffer.content.debug(), "<text>test\\n\\nline");

            let _ = buffer.redo(selection.clone(), ctx);
            assert_eq!(buffer.content.debug(), "<text>teline");
        });
    });
}

#[test]
fn test_undo_redo_block() {
    App::test((), |mut app| async move {
        let (buffer, selection) = Buffer::mock_from_plain_text(
            "test\n\nblock",
            Box::new(|_, _| IndentBehavior::Ignore),
            &mut app,
        );
        buffer.update(&mut app, |buffer, ctx| {
            let previous = buffer.to_rendered_selection_set(selection.clone(), ctx);
            let result = buffer.edit_internal_first_selection(
                6.into()..7.into(),
                "",
                TextStyles::default(),
                selection.clone(),
                ctx,
            );
            let current = buffer.to_rendered_selection_set(selection.clone(), ctx);
            buffer.push_undo_item(
                previous,
                current,
                result.undo_item.expect("Deletion should be reversible"),
                UndoActionType::Atomic,
            );
            assert_eq!(buffer.debug(), "<text>test\\nblock");
            buffer.undo(selection.clone(), ctx);
            assert_eq!(buffer.debug(), "<text>test\\n\\nblock");
            buffer.redo(selection.clone(), ctx);
            assert_eq!(buffer.debug(), "<text>test\\nblock");
        });
    });
}

#[test]
fn test_undo_redo_multi_block_deletion() {
    App::test((), |mut app| async move {
        let (buffer, selection) = Buffer::mock_from_plain_text(
            "test\nline\nblock\nafter",
            Box::new(|_, _| IndentBehavior::Ignore),
            &mut app,
        );
        buffer.update(&mut app, |buffer, ctx| {
            let previous = buffer.to_rendered_selection_set(selection.clone(), ctx);
            let result = buffer.edit_internal_first_selection(
                8.into()..13.into(),
                "",
                TextStyles::default(),
                selection.clone(),
                ctx,
            );
            let current = buffer.to_rendered_selection_set(selection.clone(), ctx);
            buffer.push_undo_item(
                previous,
                current,
                result.undo_item.expect("Deletion should be reversible"),
                UndoActionType::Atomic,
            );
            assert_eq!(buffer.debug(), "<text>test\\nliock\\nafter");
            buffer.undo(selection.clone(), ctx);
            assert_eq!(buffer.debug(), "<text>test\\nline\\nblock\\nafter");
            buffer.redo(selection.clone(), ctx);
            assert_eq!(buffer.debug(), "<text>test\\nliock\\nafter");
        });
    });
}

#[test]
fn test_delete_clamped() {
    // This tests that Delete actions can't remove the starting block marker.
    App::test((), |mut app| async move {
        let buffer = app.add_model(|_| Buffer::new(Box::new(|_, _| IndentBehavior::Ignore)));
        let selection = app.add_model(|_| BufferSelectionModel::new(buffer.clone()));

        buffer.update(&mut app, |buffer, ctx| {
            buffer.apply_formatted_edit_fixture(
                CharOffset::zero()..buffer.max_charoffset(),
                VecDeque::from(vec![EditorLine::Line(vec![
                    FormattedTextFragment::bold("some te"),
                    FormattedTextFragment::plain_text("xt"),
                ])]),
                EditOrigin::SystemEdit,
                selection.clone(),
                ctx,
            );
            selection.update(ctx, |selection, _| {
                set_selections(selection, vec1![1..8]);
            });

            // This would happen when, for example, deleting a word (alt-backspace) at the start of the buffer.
            let mut style = None;
            buffer.delete(
                &mut style,
                vec1![CharOffset::zero()..CharOffset::from(5)],
                selection.clone(),
                ctx,
            );

            assert_eq!(buffer.debug(), "<text><b_s> te<b_e>xt");
            assert_eq!(style, Some(TextStylesWithMetadata::default().bold()));
        });
    });
}

#[test]
fn test_nonatomic_undo_insertion() {
    App::test((), |mut app| async move {
        let buffer = app.add_model(|_| Buffer::new(Box::new(|_, _| IndentBehavior::Ignore)));
        let selection = app.add_model(|_| BufferSelectionModel::new(buffer.clone()));

        buffer.update(&mut app, |buffer, ctx| {
            buffer.edit_internal_first_selection(
                CharOffset::from(1)..CharOffset::from(1),
                "test\nline\nsecond",
                Default::default(),
                selection.clone(),
                ctx,
            );
            assert_eq!(buffer.content.debug(), "<text>test\\nline\\nsecond");

            let prev_selection = buffer.to_rendered_selection_set(selection.clone(), ctx);
            let edit_result = buffer.edit_internal_first_selection(
                CharOffset::from(1)..CharOffset::from(1),
                "so",
                TextStyles::default(),
                selection.clone(),
                ctx,
            );
            assert_eq!(buffer.content.debug(), "<text>sotest\\nline\\nsecond");

            buffer.push_undo_item_nonatomic(
                prev_selection,
                edit_result.undo_item.expect("Should exist"),
                NonAtomicType::Insert,
                selection.clone(),
                ctx,
            );

            let prev_selection = buffer.to_rendered_selection_set(selection.clone(), ctx);
            let edit_result = buffer.edit_internal_first_selection(
                CharOffset::from(3)..CharOffset::from(3),
                "\n",
                TextStyles::default(),
                selection.clone(),
                ctx,
            );
            assert_eq!(buffer.content.debug(), "<text>so\\ntest\\nline\\nsecond");

            buffer.push_undo_item_nonatomic(
                prev_selection,
                edit_result.undo_item.expect("Should exist"),
                NonAtomicType::Insert,
                selection.clone(),
                ctx,
            );

            let prev_selection = buffer.to_rendered_selection_set(selection.clone(), ctx);
            let edit_result = buffer.edit_internal_first_selection(
                CharOffset::from(4)..CharOffset::from(4),
                " ",
                TextStyles::default(),
                selection.clone(),
                ctx,
            );
            assert_eq!(buffer.content.debug(), "<text>so\\n test\\nline\\nsecond");

            buffer.push_undo_item_nonatomic(
                prev_selection,
                edit_result.undo_item.expect("Should exist"),
                NonAtomicType::Insert,
                selection.clone(),
                ctx,
            );

            let delta = buffer
                .undo(selection.clone(), ctx)
                .delta
                .expect("Should exist");
            assert_eq!(buffer.content.debug(), "<text>test\\nline\\nsecond");

            assert_eq!(delta.old_offset, CharOffset::from(1)..CharOffset::from(10));
            assert_eq!(
                (*delta.new_lines),
                vec![StyledBufferBlock::Text(StyledTextBlock {
                    block: vec![StyledBufferRun {
                        run: "test\n".to_string(),
                        text_styles: TextStylesWithMetadata::default(),
                        block_style: BufferBlockStyle::PlainText
                    },],
                    style: BufferBlockStyle::PlainText,
                    content_length: CharOffset::from(5)
                }),]
            );

            let delta = buffer
                .redo(selection.clone(), ctx)
                .delta
                .expect("Should exist");
            assert_eq!(buffer.content.debug(), "<text>so\\n test\\nline\\nsecond");

            assert_eq!(delta.old_offset, CharOffset::from(1)..CharOffset::from(6));
            assert_eq!(
                (*delta.new_lines),
                vec![
                    StyledBufferBlock::Text(StyledTextBlock {
                        block: vec![StyledBufferRun {
                            run: "so\n".to_string(),
                            text_styles: TextStylesWithMetadata::default(),
                            block_style: BufferBlockStyle::PlainText
                        },],
                        style: BufferBlockStyle::PlainText,
                        content_length: CharOffset::from(3)
                    }),
                    StyledBufferBlock::Text(StyledTextBlock {
                        block: vec![StyledBufferRun {
                            run: " test\n".to_string(),
                            text_styles: TextStylesWithMetadata::default(),
                            block_style: BufferBlockStyle::PlainText
                        },],
                        style: BufferBlockStyle::PlainText,
                        content_length: CharOffset::from(6)
                    })
                ]
            );
        });
    });
}

#[test]
fn test_nonatomic_undo_deletion() {
    App::test((), |mut app| async move {
        let buffer = app.add_model(|_| Buffer::new(Box::new(|_, _| IndentBehavior::Ignore)));
        let selection = app.add_model(|_| BufferSelectionModel::new(buffer.clone()));

        buffer.update(&mut app, |buffer, ctx| {
            buffer.edit_internal_first_selection(
                CharOffset::from(1)..CharOffset::from(1),
                "test\nline\nsecond",
                Default::default(),
                selection.clone(),
                ctx,
            );
            assert_eq!(buffer.content.debug(), "<text>test\\nline\\nsecond");

            selection.update(ctx, |selection, _| {
                set_selections(selection, vec1![7..7]);
            });
            let prev_selection = buffer.to_rendered_selection_set(selection.clone(), ctx);
            let edit_result = buffer.backspace(&mut None, selection.clone(), ctx);
            assert_eq!(buffer.content.debug(), "<text>test\\nine\\nsecond");

            buffer.push_undo_item_nonatomic(
                prev_selection,
                edit_result.undo_item.expect("Should exist"),
                NonAtomicType::Backspace,
                selection.clone(),
                ctx,
            );

            let prev_selection = buffer.to_rendered_selection_set(selection.clone(), ctx);
            let edit_result = buffer.backspace(&mut None, selection.clone(), ctx);
            assert_eq!(buffer.content.debug(), "<text>testine\\nsecond");

            buffer.push_undo_item_nonatomic(
                prev_selection,
                edit_result.undo_item.expect("Should exist"),
                NonAtomicType::Backspace,
                selection.clone(),
                ctx,
            );

            let prev_selection = buffer.to_rendered_selection_set(selection.clone(), ctx);
            let edit_result = buffer.backspace(&mut None, selection.clone(), ctx);
            assert_eq!(buffer.content.debug(), "<text>tesine\\nsecond");

            buffer.push_undo_item_nonatomic(
                prev_selection,
                edit_result.undo_item.expect("Should exist"),
                NonAtomicType::Backspace,
                selection.clone(),
                ctx,
            );

            let delta = buffer
                .undo(selection.clone(), ctx)
                .delta
                .expect("Should exist");
            assert_eq!(buffer.content.debug(), "<text>test\\nline\\nsecond");

            assert_eq!(delta.old_offset, CharOffset::from(1)..CharOffset::from(8));
            assert_eq!(
                (*delta.new_lines),
                vec![
                    StyledBufferBlock::Text(StyledTextBlock {
                        block: vec![StyledBufferRun {
                            run: "test\n".to_string(),
                            text_styles: TextStylesWithMetadata::default(),
                            block_style: BufferBlockStyle::PlainText
                        },],
                        style: BufferBlockStyle::PlainText,
                        content_length: CharOffset::from(5)
                    }),
                    StyledBufferBlock::Text(StyledTextBlock {
                        block: vec![StyledBufferRun {
                            run: "line\n".to_string(),
                            text_styles: TextStylesWithMetadata::default(),
                            block_style: BufferBlockStyle::PlainText
                        },],
                        style: BufferBlockStyle::PlainText,
                        content_length: CharOffset::from(5)
                    })
                ]
            );

            let delta = buffer
                .redo(selection.clone(), ctx)
                .delta
                .expect("Should exist");
            assert_eq!(buffer.content.debug(), "<text>tesine\\nsecond");

            assert_eq!(delta.old_offset, CharOffset::from(1)..CharOffset::from(11));
            assert_eq!(
                (*delta.new_lines),
                vec![StyledBufferBlock::Text(StyledTextBlock {
                    block: vec![StyledBufferRun {
                        run: "tesine\n".to_string(),
                        text_styles: TextStylesWithMetadata::default(),
                        block_style: BufferBlockStyle::PlainText
                    },],
                    style: BufferBlockStyle::PlainText,
                    content_length: CharOffset::from(7)
                }),]
            );
        });
    });
}

#[test]
fn test_nonatomic_undo_mixed() {
    App::test((), |mut app| async move {
        let buffer = app.add_model(|_| Buffer::new(Box::new(|_, _| IndentBehavior::Ignore)));
        let selection = app.add_model(|_| BufferSelectionModel::new(buffer.clone()));

        buffer.update(&mut app, |buffer, ctx| {
            buffer.edit_internal_first_selection(
                CharOffset::from(1)..CharOffset::from(1),
                "test\nline\nsecond",
                Default::default(),
                selection.clone(),
                ctx,
            );
            assert_eq!(buffer.content.debug(), "<text>test\\nline\\nsecond");

            selection.update(ctx, |selection, _| {
                set_selections(selection, vec1![7..7]);
            });
            let prev_selection = buffer.to_rendered_selection_set(selection.clone(), ctx);
            let edit_result = buffer.backspace(&mut None, selection.clone(), ctx);
            assert_eq!(buffer.content.debug(), "<text>test\\nine\\nsecond");

            buffer.push_undo_item_nonatomic(
                prev_selection,
                edit_result.undo_item.expect("Should exist"),
                NonAtomicType::Backspace,
                selection.clone(),
                ctx,
            );

            let prev_selection = buffer.to_rendered_selection_set(selection.clone(), ctx);
            let edit_result = buffer.edit_internal_first_selection(
                CharOffset::from(6)..CharOffset::from(6),
                "d",
                Default::default(),
                selection.clone(),
                ctx,
            );
            assert_eq!(buffer.content.debug(), "<text>test\\ndine\\nsecond");

            buffer.push_undo_item_nonatomic(
                prev_selection,
                edit_result.undo_item.expect("Should exist"),
                NonAtomicType::Insert,
                selection.clone(),
                ctx,
            );

            let prev_selection = buffer.to_rendered_selection_set(selection.clone(), ctx);
            let edit_result = buffer.edit_internal_first_selection(
                CharOffset::from(7)..CharOffset::from(7),
                "e",
                Default::default(),
                selection.clone(),
                ctx,
            );
            assert_eq!(buffer.content.debug(), "<text>test\\ndeine\\nsecond");

            buffer.push_undo_item_nonatomic(
                prev_selection,
                edit_result.undo_item.expect("Should exist"),
                NonAtomicType::Insert,
                selection.clone(),
                ctx,
            );

            buffer.undo(selection.clone(), ctx);
            assert_eq!(buffer.content.debug(), "<text>test\\nine\\nsecond");

            buffer.undo(selection.clone(), ctx);
            assert_eq!(buffer.content.debug(), "<text>test\\nline\\nsecond");

            buffer.redo(selection.clone(), ctx);
            assert_eq!(buffer.content.debug(), "<text>test\\nine\\nsecond");

            buffer.redo(selection.clone(), ctx);
            assert_eq!(buffer.content.debug(), "<text>test\\ndeine\\nsecond");
        });
    });
}

// Regression test for CLD-655
#[test]
fn test_undo_with_invalidated_selection_range() {
    App::test((), |mut app| async move {
        let buffer = app.add_model(|_| Buffer::new(Box::new(|_, _| IndentBehavior::Ignore)));
        let selection = app.add_model(|_| BufferSelectionModel::new(buffer.clone()));

        buffer.update(&mut app, |buffer, ctx| {
            buffer.edit_internal_first_selection(
                CharOffset::from(1)..CharOffset::from(1),
                "abc\n",
                Default::default(),
                selection.clone(),
                ctx,
            );
            assert_eq!(buffer.content.debug(), "<text>abc\\n");

            let prev_selection = buffer.to_rendered_selection_set(selection.clone(), ctx);
            let edit_result = buffer.edit_internal_first_selection(
                CharOffset::from(5)..CharOffset::from(5),
                "abc",
                TextStyles::default(),
                selection.clone(),
                ctx,
            );
            let curr_selection = buffer.to_rendered_selection_set(selection.clone(), ctx);
            buffer.push_undo_item(
                prev_selection,
                curr_selection,
                edit_result.undo_item.expect("Should exist"),
                UndoActionType::Atomic,
            );
            assert_eq!(buffer.content.debug(), "<text>abc\\nabc");

            selection.update(ctx, |selection, _| {
                set_selections(selection, vec1![6..6]);
            });

            buffer.undo(selection.clone(), ctx);
            assert_eq!(buffer.content.debug(), "<text>abc\\n");
            assert_eq!(
                selection.as_ref(ctx).selection_to_first_offset_range(),
                CharOffset::from(5)..CharOffset::from(5)
            );
        });
    });
}

#[test]
fn test_edit_after_hyperlink() {
    App::test((), |mut app| async move {
        let buffer = app.add_model(|_| Buffer::new(Box::new(|_, _| IndentBehavior::Ignore)));
        let selection = app.add_model(|_| BufferSelectionModel::new(buffer.clone()));

        buffer.update(&mut app, |buffer, ctx| {
            buffer.edit_internal_first_selection(
                CharOffset::from(1)..CharOffset::from(1),
                "line\nblock",
                Default::default(),
                selection.clone(),
                ctx,
            );
            assert_eq!(buffer.content.debug(), "<text>line\\nblock");
            buffer.apply_formatted_edit_fixture(
                CharOffset::zero()..CharOffset::from(11),
                VecDeque::from(vec![
                    EditorLine::Line(vec![
                        FormattedTextFragment::hyperlink("lin", "www.google.com"),
                        FormattedTextFragment::plain_text("e"),
                    ]),
                    EditorLine::Line(vec![FormattedTextFragment::plain_text("block")]),
                ]),
                EditOrigin::SystemEdit,
                selection.clone(),
                ctx,
            );
            buffer.set_selection(1.into()..4.into(), selection.clone(), ctx);
            assert_eq!(
                buffer.content.debug(),
                "<text><a_www.google.com>lin<a>e\\nblock"
            );

            buffer.edit_internal_first_selection(
                CharOffset::from(4)..CharOffset::from(4),
                "ab",
                TextStyles::default(),
                selection.clone(),
                ctx,
            );
            assert_eq!(
                buffer.content.debug(),
                "<text><a_www.google.com>lin<a>abe\\nblock"
            );
        });
    });
}

#[test]
fn test_code_tab_indent() {
    App::test((), |mut app| async move {
        let buffer = app.add_model(|_| {
            Buffer::new(Box::new(|_, _| {
                IndentBehavior::TabIndent(IndentUnit::Space(4))
            }))
        });
        let selection = app.add_model(|_| BufferSelectionModel::new(buffer.clone()));

        buffer.update(&mut app, |buffer, ctx| {
            buffer.edit_internal_first_selection(
                CharOffset::from(1)..CharOffset::from(1),
                "first\n second",
                Default::default(),
                selection.clone(),
                ctx,
            );
            assert_eq!(buffer.debug(), "<text>first\\n second");

            // Partially-select each line and indent them. This adds 4 spaces to the first line, but only 3
            // to the second, resulting in an overall indent of 4 for each (one tab stop).
            buffer.set_selection(
                CharOffset::from(4)..CharOffset::from(10),
                selection.clone(),
                ctx,
            );
            let prev_selection = buffer.to_rendered_selection_set(selection.clone(), ctx);
            let edit_result = buffer.indent(1, selection.clone(), ctx);
            let curr_selection = buffer.to_rendered_selection_set(selection.clone(), ctx);
            buffer.push_undo_item(
                prev_selection,
                curr_selection,
                edit_result.undo_item.unwrap(),
                UndoActionType::Atomic,
            );

            assert_eq!(buffer.debug(), "<text>    first\\n    second");

            buffer.undo(selection.clone(), ctx);
            assert_eq!(buffer.debug(), "<text>first\\n second");
            buffer.redo(selection.clone(), ctx);

            // Indent the first line further. This should add 4 spaces, but not affect the second line.
            selection.update(ctx, |selection, _| {
                set_selections(selection, vec1![5..5]);
            });
            buffer.indent(1, selection.clone(), ctx);
            assert_eq!(buffer.debug(), "<text>        first\\n    second");

            // Indent the second line further, but starting 2 characters into the line. This results in
            // adding 2 more spaces, not a full tab.
            selection.update(ctx, |selection, _| {
                set_selections(selection, vec1![17..17]);
            });
            buffer.indent(1, selection.clone(), ctx);
            assert_eq!(buffer.debug(), "<text>        first\\n      second");
        });
    });
}

#[test]
fn test_indent_with_tab_unit() {
    App::test((), |mut app| async move {
        let buffer = app.add_model(|_| {
            Buffer::new(Box::new(|_, _| IndentBehavior::TabIndent(IndentUnit::Tab)))
        });
        let selection = app.add_model(|_| BufferSelectionModel::new(buffer.clone()));

        buffer.update(&mut app, |buffer, ctx| {
            buffer.edit_internal_first_selection(
                CharOffset::from(1)..CharOffset::from(1),
                "first\n second",
                Default::default(),
                selection.clone(),
                ctx,
            );
            assert_eq!(buffer.debug(), "<text>first\\n second");

            // Partially-select each line and indent them. This adds 4 spaces to the first line, but only 3
            // to the second, resulting in an overall indent of 4 for each (one tab stop).
            buffer.set_selection(
                CharOffset::from(4)..CharOffset::from(10),
                selection.clone(),
                ctx,
            );
            let prev_selection = buffer.to_rendered_selection_set(selection.clone(), ctx);
            let edit_result = buffer.indent(1, selection.clone(), ctx);
            let curr_selection = buffer.to_rendered_selection_set(selection.clone(), ctx);
            buffer.push_undo_item(
                prev_selection,
                curr_selection,
                edit_result.undo_item.unwrap(),
                UndoActionType::Atomic,
            );

            assert_eq!(buffer.debug(), "<text>\tfirst\\n \tsecond");

            buffer.undo(selection.clone(), ctx);
            assert_eq!(buffer.debug(), "<text>first\\n second");
            buffer.redo(selection.clone(), ctx);

            // Indent the first line further. This should add 4 spaces, but not affect the second line.
            selection.update(ctx, |selection, _| {
                set_selections(selection, vec1![1..1]);
            });
            buffer.indent(1, selection.clone(), ctx);
            assert_eq!(buffer.debug(), "<text>\t\tfirst\\n \tsecond");

            // Indent the second line further, but starting 2 characters into the line. This results in
            // adding 2 more spaces, not a full tab.
            selection.update(ctx, |selection, _| {
                set_selections(selection, vec1![10..10]);
            });
            buffer.indent(1, selection.clone(), ctx);
            assert_eq!(buffer.debug(), "<text>\t\tfirst\\n \t\tsecond");

            selection.update(ctx, |selection, _| {
                set_selections(selection, vec1![2..2]);
            });
            buffer.unindent(selection.clone(), ctx);
            assert_eq!(buffer.debug(), "<text>\tfirst\\n \t\tsecond");

            selection.update(ctx, |selection, _| {
                set_selections(selection, vec1![11..11]);
            });
            buffer.backspace(&mut None, selection.clone(), ctx);
            assert_eq!(buffer.debug(), "<text>\tfirst\\n \tsecond");
        });
    });
}

#[test]
fn test_code_tab_within_line() {
    App::test((), |mut app| async move {
        let buffer = app.add_model(|_| {
            Buffer::new(Box::new(|_, _| {
                IndentBehavior::TabIndent(IndentUnit::Space(4))
            }))
        });
        let selection = app.add_model(|_| BufferSelectionModel::new(buffer.clone()));

        buffer.update(&mut app, |buffer, ctx| {
            buffer.edit_internal_first_selection(
                CharOffset::from(1)..CharOffset::from(1),
                "git log",
                Default::default(),
                selection.clone(),
                ctx,
            );
            assert_eq!(buffer.debug(), "<text>git log");

            // Since the cursor is 1 character into the line, 3 more spaces are added.
            selection.update(ctx, |selection, _| {
                set_selections(selection, vec1![2..2]);
            });
            buffer.indent(1, selection.clone(), ctx);
            assert_eq!(buffer.debug(), "<text>g   it log");

            // The `l` is 8 characters in, so a full 4 spaces are added.
            selection.update(ctx, |selection, _| {
                set_selections(selection, vec1![9..9]);
            });
            buffer.indent(1, selection.clone(), ctx);
            assert_eq!(buffer.debug(), "<text>g   it l    og");

            // Tab at the start of the line also adds 4 spaces.
            selection.update(ctx, |selection, _| {
                set_selections(selection, vec1![1..1]);
            });
            buffer.indent(1, selection.clone(), ctx);
            assert_eq!(buffer.debug(), "<text>    g   it l    og");
            assert_eq!(
                selection.as_ref(ctx).selection_to_first_offset_range(),
                5.into()..5.into()
            );
        });
    });
}

#[test]
fn test_code_unindent() {
    App::test((), |mut app| async move {
        let buffer = app.add_model(|_| {
            Buffer::new(Box::new(|_, _| {
                IndentBehavior::TabIndent(IndentUnit::Space(4))
            }))
        });
        let selection = app.add_model(|_| BufferSelectionModel::new(buffer.clone()));

        buffer.update(&mut app, |buffer, ctx| {
            buffer.edit_internal_first_selection(
                1.into()..1.into(),
                "    git log\n  ls\n     cat file.txt",
                Default::default(),
                selection.clone(),
                ctx,
            );
            assert_eq!(
                buffer.debug(),
                "<text>    git log\\n  ls\\n     cat file.txt"
            );

            // Unindent all 3 lines.
            let max_charoffset = buffer.max_charoffset();
            buffer.set_selection(1.into()..max_charoffset, selection.clone(), ctx);
            buffer.unindent(selection.clone(), ctx);
            // `git log` was indented by 4 spaces, so all are removed.
            // `ls` was indented by 2 spaces, both of which are removed (but only the spaces).
            // `cat file.txt` was indented by 5 spaces, 1 of which is removed so that it's evenly indented.
            assert_eq!(buffer.debug(), "<text>git log\\nls\\n    cat file.txt");

            // Unindent the last line from a single cursor. Even though the character at the cursor is a
            // space, unindent only ever applies to leading indentation.
            selection.update(ctx, |selection, _| {
                set_selections(selection, vec1![19..19]);
            });
            buffer.unindent(selection.clone(), ctx);
            assert_eq!(buffer.debug(), "<text>git log\\nls\\ncat file.txt");
        });
    });
}

#[test]
fn test_code_backspace_unindent() {
    App::test((), |mut app| async move {
        let buffer = app.add_model(|_| {
            Buffer::new(Box::new(|_, _| {
                IndentBehavior::TabIndent(IndentUnit::Space(4))
            }))
        });
        let selection = app.add_model(|_| BufferSelectionModel::new(buffer.clone()));

        buffer.update(&mut app, |buffer, ctx| {
            buffer.edit_internal_first_selection(
                1.into()..1.into(),
                "fn main() {\n        panic!();\n }",
                Default::default(),
                selection.clone(),
                ctx,
            );
            assert_eq!(buffer.debug(), "<text>fn main() {\\n        panic!();\\n }");

            // At the start of the panic! line, backspace should remove a tab of indentation.
            selection.update(ctx, |selection, _| {
                set_selections(selection, vec1![21..21]);
            });
            buffer.backspace(&mut None, selection.clone(), ctx);
            assert_eq!(buffer.debug(), "<text>fn main() {\\n    panic!();\\n }");

            // Within the panic! line's whitespace, backspace should remove some of the indentation.
            selection.update(ctx, |selection, _| {
                set_selections(selection, vec1![15..15]);
            });
            buffer.backspace(&mut None, selection.clone(), ctx);
            assert_eq!(buffer.debug(), "<text>fn main() {\\n  panic!();\\n }");

            // At the start of the }, line, backspace should remove the single space.
            selection.update(ctx, |selection, _| {
                set_selections(selection, vec1![26..26]);
            });
            buffer.backspace(&mut None, selection.clone(), ctx);
            assert_eq!(buffer.debug(), "<text>fn main() {\\n  panic!();\\n}");

            // Within a line, backspace should remove the previous character.
            selection.update(ctx, |selection, _| {
                set_selections(selection, vec1![5..5]);
            });
            buffer.backspace(&mut None, selection.clone(), ctx);
            assert_eq!(buffer.debug(), "<text>fn ain() {\\n  panic!();\\n}");

            // At the start of an interior line, backspace should remove the newline.
            selection.update(ctx, |selection, _| {
                set_selections(selection, vec1![12..12]);
            });
            buffer.backspace(&mut None, selection.clone(), ctx);
            assert_eq!(buffer.debug(), "<text>fn ain() {  panic!();\\n}");
        });
    });
}

#[test]
fn test_undo_restores_inline_styles() {
    App::test((), |mut app| async move {
        let buffer = app.add_model(|_| Buffer::new(Box::new(|_, _| IndentBehavior::Ignore)));
        let selection = app.add_model(|_| BufferSelectionModel::new(buffer.clone()));
        buffer.update(&mut app, |buffer, ctx| {
            buffer.apply_formatted_edit_fixture(
                0.into()..1.into(),
                VecDeque::from([EditorLine::Line(vec![
                    FormattedTextFragment::bold("bl"),
                    FormattedTextFragment::bold_italic("o"),
                    FormattedTextFragment::italic("ck"),
                ])]),
                EditOrigin::SystemEdit,
                selection.clone(),
                ctx,
            );
            let previous = buffer.to_rendered_selection_set(selection.clone(), ctx);
            let result = buffer.edit_internal_first_selection(
                3.into()..4.into(),
                "",
                TextStyles::default(),
                selection.clone(),
                ctx,
            );
            let current = buffer.to_rendered_selection_set(selection.clone(), ctx);
            buffer.push_undo_item(
                previous,
                current,
                result.undo_item.expect("Deletion should be reversible"),
                UndoActionType::Atomic,
            );
            buffer.undo(selection.clone(), ctx);
            assert_eq!(buffer.debug(), "<text><b_s>bl<i_s>o<b_e>ck<i_e>");
            buffer.redo(selection.clone(), ctx);
            assert_eq!(buffer.text().as_str(), "blck");
        });
    });
}

// Regression test for CLD-771.
#[test]
fn test_invalidate_content() {
    App::test((), |mut app| async move {
        let (buffer, _selection) = Buffer::mock_from_plain_text(
            "\nabc",
            Box::new(|_, _| IndentBehavior::Ignore),
            &mut app,
        );
        buffer.update(&mut app, |buffer, _ctx| {
            assert_eq!(buffer.content.debug(), "<text>\\nabc");

            let edit_delta = buffer.invalidate_layout();
            assert_eq!(
                edit_delta.old_offset,
                CharOffset::from(1)..CharOffset::from(5)
            );
            assert_eq!(
                (*edit_delta.new_lines),
                vec![
                    StyledBufferBlock::Text(StyledTextBlock {
                        block: vec![StyledBufferRun {
                            run: "\n".to_string(),
                            text_styles: TextStylesWithMetadata::default(),
                            block_style: BufferBlockStyle::PlainText
                        },],
                        style: BufferBlockStyle::PlainText,
                        content_length: CharOffset::from(1)
                    }),
                    StyledBufferBlock::Text(StyledTextBlock {
                        block: vec![StyledBufferRun {
                            run: "abc".to_string(),
                            text_styles: TextStylesWithMetadata::default(),
                            block_style: BufferBlockStyle::PlainText
                        },],
                        style: BufferBlockStyle::PlainText,
                        content_length: CharOffset::from(3)
                    }),
                ]
            );
        });
    });
}

#[test]
fn test_multi_linebreak_paste() {
    App::test((), |mut app| async move {
        let buffer = app.add_model(|_| Buffer::new(Box::new(|_, _| IndentBehavior::Ignore)));
        let selection = app.add_model(|_| BufferSelectionModel::new(buffer.clone()));

        buffer.update(&mut app, |buffer, ctx| {
            buffer.edit_internal_first_selection(
                CharOffset::from(1)..CharOffset::from(1),
                "text",
                Default::default(),
                selection.clone(),
                ctx,
            );

            buffer.edit_internal_first_selection(
                CharOffset::from(3)..CharOffset::from(3),
                "\n\n",
                TextStyles::default(),
                selection.clone(),
                ctx,
            );

            assert_eq!(buffer.content.debug(), "<text>te\\n\\nxt");
        });
    });
}

#[test]
fn test_selection_bias_with_mouse_movement() {
    App::test((), |mut app| async move {
        let buffer = app.add_model(|_| Buffer::new(Box::new(|_, _| IndentBehavior::Ignore)));
        let selection = app.add_model(|_| BufferSelectionModel::new(buffer.clone()));

        buffer.update(&mut app, |buffer, ctx| {
            buffer.apply_formatted_edit_fixture(
                CharOffset::zero()..buffer.max_charoffset(),
                VecDeque::from(vec![EditorLine::Line(vec![
                    FormattedTextFragment::plain_text("text"),
                    FormattedTextFragment {
                        text: "inline".into(),
                        styles: FormattedTextStyles {
                            inline_code: true,
                            ..Default::default()
                        },
                    },
                    FormattedTextFragment::plain_text("text"),
                ])]),
                EditOrigin::SystemEdit,
                selection.clone(),
                ctx,
            );
            selection.update(ctx, |selection, _| {
                set_selections(selection, vec1![5..11]);
            });

            assert_eq!(buffer.content.debug(), "<text>text<c_s>inline<c_e>text");

            selection.update(ctx, |selection, _| {
                set_selections(selection, vec1![5..5]);
            });
            assert_eq!(
                selection.as_ref(ctx).selection().bias(),
                TextStyleBias::OutOfStyle
            );

            // Moving the cursor right from out of style should change bias to in style.
            buffer.update_selection(
                selection.clone(),
                BufferSelectAction::MoveRight,
                AutoScrollBehavior::Selection,
                ctx,
            );

            assert_eq!(
                selection.as_ref(ctx).selection().bias(),
                TextStyleBias::InStyle
            );
            assert_eq!(
                selection.as_ref(ctx).first_selection_head(),
                CharOffset::from(5)
            );

            // Moving the cursor right again from in style should move the cursor right by 1.
            buffer.update_selection(
                selection.clone(),
                BufferSelectAction::MoveRight,
                AutoScrollBehavior::Selection,
                ctx,
            );
            assert_eq!(
                selection.as_ref(ctx).first_selection_head(),
                CharOffset::from(6)
            );

            selection.update(ctx, |selection, _| {
                set_selections(selection, vec1![10..10]);
            });
            buffer.update_selection(
                selection.clone(),
                BufferSelectAction::MoveRight,
                AutoScrollBehavior::Selection,
                ctx,
            );

            // Moving the cursor right from within inline code to border should change bias to in style.
            assert_eq!(
                selection.as_ref(ctx).selection().bias(),
                TextStyleBias::InStyle
            );
            assert_eq!(
                selection.as_ref(ctx).first_selection_head(),
                CharOffset::from(11)
            );

            // Moving the cursor right again from in style should change bias to out of style.
            buffer.update_selection(
                selection.clone(),
                BufferSelectAction::MoveRight,
                AutoScrollBehavior::Selection,
                ctx,
            );
            assert_eq!(
                selection.as_ref(ctx).selection().bias(),
                TextStyleBias::OutOfStyle
            );
            assert_eq!(
                selection.as_ref(ctx).first_selection_head(),
                CharOffset::from(11)
            );

            selection.update(ctx, |selection, _| {
                set_selections(selection, vec1![6..6]);
            });

            // Moving the cursor left from within inline code to border should change bias to in style.
            buffer.update_selection(
                selection.clone(),
                BufferSelectAction::MoveLeft,
                AutoScrollBehavior::Selection,
                ctx,
            );

            assert_eq!(
                selection.as_ref(ctx).selection().bias(),
                TextStyleBias::InStyle
            );
            assert_eq!(
                selection.as_ref(ctx).first_selection_head(),
                CharOffset::from(5)
            );

            // Moving the cursor left again from in style should change bias to out of style.
            buffer.update_selection(
                selection.clone(),
                BufferSelectAction::MoveLeft,
                AutoScrollBehavior::Selection,
                ctx,
            );
            assert_eq!(
                selection.as_ref(ctx).selection().bias(),
                TextStyleBias::OutOfStyle
            );
            assert_eq!(
                selection.as_ref(ctx).first_selection_head(),
                CharOffset::from(5)
            );

            // Moving the cursor left again should move the cursor left by 1.
            buffer.update_selection(
                selection.clone(),
                BufferSelectAction::MoveLeft,
                AutoScrollBehavior::Selection,
                ctx,
            );
            assert_eq!(
                selection.as_ref(ctx).selection().bias(),
                TextStyleBias::OutOfStyle
            );
            assert_eq!(
                selection.as_ref(ctx).first_selection_head(),
                CharOffset::from(4)
            );

            // Moving the cursor left from outside inline code to border should keep bias to out of style.
            selection.update(ctx, |selection, _| {
                set_selections(selection, vec1![12..12]);
            });
            buffer.update_selection(
                selection.clone(),
                BufferSelectAction::MoveLeft,
                AutoScrollBehavior::Selection,
                ctx,
            );
            assert_eq!(
                selection.as_ref(ctx).selection().bias(),
                TextStyleBias::OutOfStyle
            );
            assert_eq!(
                selection.as_ref(ctx).first_selection_head(),
                CharOffset::from(11)
            );

            // Moving the cursor left again from out of style should change bias to in style.
            buffer.update_selection(
                selection.clone(),
                BufferSelectAction::MoveLeft,
                AutoScrollBehavior::Selection,
                ctx,
            );
            assert_eq!(
                selection.as_ref(ctx).selection().bias(),
                TextStyleBias::InStyle
            );
            assert_eq!(
                selection.as_ref(ctx).first_selection_head(),
                CharOffset::from(11)
            );

            // Moving the cursor left again should move the cursor left by 1.
            buffer.update_selection(
                selection.clone(),
                BufferSelectAction::MoveLeft,
                AutoScrollBehavior::Selection,
                ctx,
            );
            assert_eq!(
                selection.as_ref(ctx).selection().bias(),
                TextStyleBias::OutOfStyle
            );
            assert_eq!(
                selection.as_ref(ctx).first_selection_head(),
                CharOffset::from(10)
            );
        });
    });
}

#[test]
fn test_styled_blocks_from_buffer_start() {
    App::test((), |mut app| async move {
        let buffer = app.add_model(|_| Buffer::new(Box::new(|_, _| IndentBehavior::Ignore)));
        let selection = app.add_model(|_| BufferSelectionModel::new(buffer.clone()));

        buffer.update(&mut app, |buffer, ctx| {
            buffer.edit_internal_first_selection(
                CharOffset::from(1)..CharOffset::from(1),
                "A\nB",
                Default::default(),
                selection.clone(),
                ctx,
            );
            assert_eq!(buffer.debug(), "<text>A\\nB");
            assert_eq!(
                buffer.styled_blocks_in_range(
                    CharOffset::zero()..buffer.max_charoffset(),
                    StyledBlockBoundaryBehavior::Exclusive
                ),
                vec![
                    StyledBufferBlock::Text(StyledTextBlock {
                        block: vec![StyledBufferRun {
                            run: "A\n".to_string(),
                            text_styles: Default::default(),
                            block_style: BufferBlockStyle::PlainText
                        }],
                        style: BufferBlockStyle::PlainText,
                        content_length: CharOffset::from(2)
                    }),
                    StyledBufferBlock::Text(StyledTextBlock {
                        block: vec![StyledBufferRun {
                            run: "B".to_string(),
                            text_styles: Default::default(),
                            block_style: BufferBlockStyle::PlainText
                        }],
                        style: BufferBlockStyle::PlainText,
                        content_length: CharOffset::from(1)
                    })
                ]
            );
        });
    });
}

#[test]
fn test_deleting_range_with_trailing_block_marker() {
    App::test((), |mut app| async move {
        let buffer = app.add_model(|_| Buffer::new(Box::new(|_, _| IndentBehavior::Ignore)));
        let selection = app.add_model(|_| BufferSelectionModel::new(buffer.clone()));

        buffer.update(&mut app, |buffer, ctx| {
            buffer.edit_internal_first_selection(
                CharOffset::from(1)..CharOffset::from(1),
                "A\nB\nC",
                Default::default(),
                selection.clone(),
                ctx,
            );

            assert_eq!(buffer.content.debug(), "<text>A\\nB\\nC");

            let prev_selection = buffer.to_rendered_selection_set(selection.clone(), ctx);
            let edit_result = buffer.edit_internal_first_selection(
                CharOffset::from(1)..CharOffset::from(5),
                "",
                Default::default(),
                selection.clone(),
                ctx,
            );
            let delta = edit_result.delta.unwrap();
            assert_eq!(delta.old_offset, 1.into()..6.into());
            assert_eq!(
                (*delta.new_lines),
                vec![StyledBufferBlock::Text(StyledTextBlock {
                    block: vec![StyledBufferRun {
                        run: "C".to_string(),
                        text_styles: Default::default(),
                        block_style: BufferBlockStyle::PlainText
                    }],
                    style: BufferBlockStyle::PlainText,
                    content_length: CharOffset::from(1)
                })]
            );

            let undo_item = edit_result.undo_item.expect("Should exist");
            let curr_selection = buffer.to_rendered_selection_set(selection.clone(), ctx);
            buffer.push_undo_item(
                prev_selection,
                curr_selection,
                undo_item,
                UndoActionType::Atomic,
            );
            assert_eq!(buffer.content.debug(), "<text>C");

            buffer.undo(selection.clone(), ctx);
            assert_eq!(buffer.content.debug(), "<text>A\\nB\\nC");

            buffer.redo(selection.clone(), ctx);
            assert_eq!(buffer.content.debug(), "<text>C");
        });
    });
}

#[test]
fn test_multiple_selections() {
    App::test((), |mut app| async move {
        let buffer = app.add_model(|_| Buffer::new(Box::new(|_, _| IndentBehavior::Ignore)));
        let selection = app.add_model(|_| BufferSelectionModel::new(buffer.clone()));

        buffer.update(&mut app, |buffer, ctx| {
            buffer.edit_internal_first_selection(
                CharOffset::from(1)..CharOffset::from(1),
                "This is some text!",
                Default::default(),
                selection.clone(),
                ctx,
            );

            assert_eq!(selection.as_ref(ctx).selection_offsets().len(), 1);

            buffer.add_cursor(4.into(), false, selection.clone(), ctx);

            assert_eq!(selection.as_ref(ctx).selection_offsets().len(), 2);
            assert_eq!(
                selection.as_ref(ctx).selections_to_offset_ranges()[0],
                19.into()..19.into()
            );
            assert_eq!(
                selection.as_ref(ctx).selections_to_offset_ranges()[1],
                4.into()..4.into()
            );
        });
    });
}

#[test]
fn test_multiselect_insert() {
    App::test((), |mut app| async move {
        let buffer = app.add_model(|_| Buffer::new(Box::new(|_, _| IndentBehavior::Ignore)));
        let selection = app.add_model(|_| BufferSelectionModel::new(buffer.clone()));

        buffer.update(&mut app, |buffer, ctx| {
            buffer.edit_internal_first_selection(
                CharOffset::from(1)..CharOffset::from(1),
                "two three four five",
                Default::default(),
                selection.clone(),
                ctx,
            );

            // Two cursors
            buffer.add_cursor(1.into(), true, selection.clone(), ctx);
            buffer.add_cursor(5.into(), false, selection.clone(), ctx);

            // Insert "one"
            let prev_selection = buffer.to_rendered_selection_set(selection.clone(), ctx);
            let edit_result =
                buffer.edit_internal("one ", Default::default(), selection.clone(), ctx);

            buffer.push_undo_item_nonatomic(
                prev_selection,
                edit_result.undo_item.expect("Should exist"),
                NonAtomicType::Insert,
                selection.clone(),
                ctx,
            );

            assert_eq!(buffer.content.debug(), "<text>one two one three four five");

            // Undo
            buffer.undo(selection.clone(), ctx);
            assert_eq!(buffer.content.debug(), "<text>two three four five");

            // Insert "one " again and then "six "
            buffer.edit_internal("one ", Default::default(), selection.clone(), ctx);
            buffer.edit_internal("six ", Default::default(), selection.clone(), ctx);
            assert_eq!(
                buffer.content.debug(),
                "<text>one six two one six three four five"
            );

            // Two selections
            buffer.add_cursor(1.into(), true, selection.clone(), ctx);
            buffer.set_last_head(9.into(), selection.clone(), ctx);
            buffer.add_cursor(13.into(), false, selection.clone(), ctx);
            buffer.set_last_head(17.into(), selection.clone(), ctx);
            let prev_selection = buffer.to_rendered_selection_set(selection.clone(), ctx);

            // Replace two selections with "hi"
            let edit_result =
                buffer.edit_internal("hi ", Default::default(), selection.clone(), ctx);
            assert_eq!(
                buffer.content.debug(),
                "<text>hi two hi six three four five"
            );

            buffer.push_undo_item_nonatomic(
                prev_selection,
                edit_result.undo_item.expect("Should exist"),
                NonAtomicType::Insert,
                selection.clone(),
                ctx,
            );

            buffer.undo(selection.clone(), ctx);
            assert_eq!(
                buffer.content.debug(),
                "<text>one six two one six three four five"
            );
            assert_eq!(
                selection.as_ref(ctx).selections_to_offset_ranges(),
                vec1![1.into()..9.into(), 13.into()..17.into()]
            );

            buffer.redo(selection.clone(), ctx);
            assert_eq!(
                buffer.content.debug(),
                "<text>hi two hi six three four five"
            );
        });
    });
}

#[test]
fn test_multiselect_movement() {
    App::test((), |mut app| async move {
        let buffer = app.add_model(|_| Buffer::new(Box::new(|_, _| IndentBehavior::Ignore)));
        let selection = app.add_model(|_| BufferSelectionModel::new(buffer.clone()));

        buffer.update(&mut app, |buffer, ctx| {
            let _ = buffer.edit_internal_first_selection(
                CharOffset::from(1)..CharOffset::from(1),
                "te\nst\nhey",
                TextStyles::default(),
                selection.clone(),
                ctx,
            );
            assert_eq!(buffer.content.debug(), "<text>te\\nst\\nhey");
            assert_eq!(
                selection.as_ref(ctx).selections_to_offset_ranges(),
                vec1![CharOffset::from(10)..CharOffset::from(10)]
            );

            // Add another cursor.
            buffer.add_cursor(3.into(), false, selection.clone(), ctx);

            // This should be no-op for one cursor.
            buffer.update_selection(
                selection.clone(),
                BufferSelectAction::MoveRight,
                AutoScrollBehavior::Selection,
                ctx,
            );
            assert_eq!(
                selection.as_ref(ctx).selections_to_offset_ranges(),
                vec1![
                    CharOffset::from(10)..CharOffset::from(10),
                    CharOffset::from(4)..CharOffset::from(4)
                ]
            );

            buffer.update_selection(
                selection.clone(),
                BufferSelectAction::MoveLeft,
                AutoScrollBehavior::Selection,
                ctx,
            );
            assert_eq!(
                selection.as_ref(ctx).selections_to_offset_ranges(),
                vec1![
                    CharOffset::from(9)..CharOffset::from(9),
                    CharOffset::from(3)..CharOffset::from(3)
                ]
            );

            buffer.extend_selection_left(2, selection.clone(), ctx);
            assert_eq!(
                selection.as_ref(ctx).selections_to_offset_ranges(),
                vec1![
                    CharOffset::from(7)..CharOffset::from(9),
                    CharOffset::from(1)..CharOffset::from(3)
                ]
            );

            buffer.extend_selection_right(3, selection.clone(), ctx);
            assert_eq!(
                selection.as_ref(ctx).selections_to_offset_ranges(),
                vec1![
                    CharOffset::from(9)..CharOffset::from(10),
                    CharOffset::from(3)..CharOffset::from(4)
                ]
            );

            buffer.update_selection(
                selection.clone(),
                BufferSelectAction::MoveRight,
                AutoScrollBehavior::Selection,
                ctx,
            );
            assert_eq!(
                selection.as_ref(ctx).selections_to_offset_ranges(),
                vec1![
                    CharOffset::from(10)..CharOffset::from(10),
                    CharOffset::from(4)..CharOffset::from(4)
                ]
            );

            buffer.add_cursor(1.into(), false, selection.clone(), ctx);
            // Should be no-op for the last cursor.
            buffer.update_selection(
                selection.clone(),
                BufferSelectAction::MoveLeft,
                AutoScrollBehavior::Selection,
                ctx,
            );
            assert_eq!(
                selection.as_ref(ctx).selections_to_offset_ranges(),
                vec1![
                    CharOffset::from(9)..CharOffset::from(9),
                    CharOffset::from(3)..CharOffset::from(3),
                    CharOffset::from(1)..CharOffset::from(1)
                ]
            );

            buffer.update_selection(
                selection.clone(),
                BufferSelectAction::MoveRight,
                AutoScrollBehavior::Selection,
                ctx,
            );
            assert_eq!(
                selection.as_ref(ctx).selections_to_offset_ranges(),
                vec1![
                    CharOffset::from(10)..CharOffset::from(10),
                    CharOffset::from(4)..CharOffset::from(4),
                    CharOffset::from(2)..CharOffset::from(2)
                ]
            );
        });
    });
}

#[test]
fn test_multiselect_backspace_in_plain_text() {
    App::test((), |mut app| async move {
        let (buffer, selection) = Buffer::mock_from_plain_text(
            "test\nline\nsecond",
            Box::new(|_, _| IndentBehavior::Ignore),
            &mut app,
        );
        buffer.update(&mut app, |buffer, ctx| {
            selection.update(ctx, |selection, _| {
                set_selections(selection, vec1![14..14, 7..7]);
            });
            buffer.backspace(&mut None, selection.clone(), ctx);
            assert_eq!(buffer.content.debug(), "<text>test\\nine\\nseond");
        });
    });
}

#[test]
fn test_multiselect_copy() {
    App::test((), |mut app| async move {
        let buffer = app.add_model(|_| Buffer::new(Box::new(|_, _| IndentBehavior::Ignore)));
        let selection = app.add_model(|_| BufferSelectionModel::new(buffer.clone()));

        buffer.update(&mut app, |buffer, ctx| {
            buffer.edit_internal(
                "test\nline\nsecond",
                Default::default(),
                selection.clone(),
                ctx,
            );

            selection.update(ctx, |selection, _| {
                selection.set_selection_offsets(vec1![
                    SelectionOffsets {
                        tail: 1.into(),
                        head: 7.into(),
                    },
                    SelectionOffsets {
                        tail: 11.into(),
                        head: 14.into(),
                    },
                ]);
            });

            assert_eq!(
                buffer
                    .selected_text_as_plain_text(selection.clone(), ctx)
                    .as_str(),
                "test\nl\nsec"
            );
        })
    });
}

#[test]
fn test_multiselect_copy_blocks() {
    App::test((), |mut app| async move {
        let (buffer, selection) = Buffer::mock_from_plain_text(
            "Hey\nYou\nGuys\nText\nThis is code\n",
            Box::new(|_, _| IndentBehavior::Ignore),
            &mut app,
        );
        buffer.update(&mut app, |buffer, ctx| {
            assert_eq!(
                buffer.content.debug(),
                "<text>Hey\\nYou\\nGuys\\nText\\nThis is code\\n"
            );

            selection.update(ctx, |selection, _| {
                selection.set_selection_offsets(vec1![
                    SelectionOffsets {
                        tail: 1.into(),
                        head: 7.into(),
                    },
                    SelectionOffsets {
                        tail: 15.into(),
                        head: 23.into(),
                    },
                ]);
            });

            assert_eq!(
                buffer
                    .selected_text_as_plain_text(selection.clone(), ctx)
                    .as_str(),
                "Hey\nYo\next\nThis"
            );
        })
    });
}

#[test]
fn test_multiselect_indent_unindent() {
    App::test((), |mut app| async move {
        let (buffer, selection) = Buffer::mock_from_plain_text(
            "hey\nthere\nhey\nthere\nhey\nthere",
            Box::new(|_, _| IndentBehavior::TabIndent(IndentUnit::Space(4))),
            &mut app,
        );
        buffer.update(&mut app, |buffer, ctx| {
            selection.update(ctx, |selection, _| {
                set_selections(selection, vec1![1..1, 11..11, 21..21]);
            });
            buffer.indent(1, selection.clone(), ctx);
            assert_eq!(
                buffer.debug(),
                "<text>    hey\\nthere\\n    hey\\nthere\\n    hey\\nthere"
            );
            buffer.unindent(selection.clone(), ctx);
            assert_eq!(
                buffer.debug(),
                "<text>hey\\nthere\\nhey\\nthere\\nhey\\nthere"
            );
            assert_eq!(selection.as_ref(ctx).selection_offsets().len(), 3);
        });
    });
}

#[test]
fn test_selection_collapsing() {
    // To test
    // - Extend selection into each other.
    // - Backspace merges selections.
    // - Make sure it works with multiple selections overlapping at once.
    // - Make sure we call it again inside of selections.rs, like when extending selection by line.
    App::test((), |mut app| async move {
        let (buffer, selection) = Buffer::mock_from_plain_text(
            "hey\nthere\nyou",
            Box::new(|_, _| IndentBehavior::Ignore),
            &mut app,
        );

        selection.update(&mut app, |selection, _| {
            selection.set_selection_offsets(vec1![
                SelectionOffsets {
                    tail: 4.into(),
                    head: 1.into(),
                },
                SelectionOffsets {
                    tail: 7.into(),
                    head: 5.into(),
                },
            ]);
        });

        buffer.update(&mut app, |buffer, ctx| {
            // Moving left once would make the selections touch.  They should not merge yet.
            assert_eq!(selection.as_ref(ctx).selection_offsets().len(), 2);
            buffer.update_selection(
                selection.clone(),
                BufferSelectAction::ExtendLeft,
                AutoScrollBehavior::Selection,
                ctx,
            );
            assert_eq!(selection.as_ref(ctx).selection_offsets().len(), 2);

            // Moving left another time makes them overlap.  They should merge.
            buffer.update_selection(
                selection.clone(),
                BufferSelectAction::ExtendLeft,
                AutoScrollBehavior::Selection,
                ctx,
            );
            assert_eq!(
                selection.as_ref(ctx).selections_to_offset_ranges(),
                vec1![1.into()..7.into()]
            );

            selection.update(ctx, |selection, _| {
                set_selections(selection, vec1![4..4]);
            });
            buffer.add_cursor(5.into(), false, selection.clone(), ctx);

            // Backspace will place the cursors at the same position, but they should not be merged
            // until the next backspace.
            assert_eq!(selection.as_ref(ctx).selection_offsets().len(), 2);
            buffer.update_content(
                BufferEditAction::Backspace,
                EditOrigin::UserTyped,
                selection.clone(),
                ctx,
            );
            assert_eq!(selection.as_ref(ctx).selection_offsets().len(), 2);

            // Backspace again and they should merge.
            buffer.update_content(
                BufferEditAction::Backspace,
                EditOrigin::UserTyped,
                selection.clone(),
                ctx,
            );
            assert_eq!(
                selection.as_ref(ctx).selection_offsets(),
                vec1![SelectionOffsets {
                    tail: 2.into(),
                    head: 2.into(),
                }]
            );

            // Place two selections near each other.
            selection.update(ctx, |selection, _| {
                selection.set_selection_offsets(vec1![
                    SelectionOffsets {
                        tail: 1.into(),
                        head: 4.into(),
                    },
                    SelectionOffsets {
                        tail: 5.into(),
                        head: 7.into(),
                    },
                ]);
            });

            // If we extend to the right, the head of the new selection should be to the right.
            buffer.update_selection(
                selection.clone(),
                BufferSelectAction::ExtendRight,
                AutoScrollBehavior::Selection,
                ctx,
            );
            buffer.update_selection(
                selection.clone(),
                BufferSelectAction::ExtendRight,
                AutoScrollBehavior::Selection,
                ctx,
            );
            assert_eq!(
                selection.as_ref(ctx).selection_offsets(),
                vec1![SelectionOffsets {
                    tail: 1.into(),
                    head: 9.into(),
                }]
            );

            // Opposite of the previous test.
            // Place two selections near each other.
            selection.update(ctx, |selection, _| {
                selection.set_selection_offsets(vec1![
                    SelectionOffsets {
                        tail: 4.into(),
                        head: 1.into(),
                    },
                    SelectionOffsets {
                        tail: 7.into(),
                        head: 5.into(),
                    },
                ]);
            });

            // If we extend to the right, the head of the new selection should be to the right.
            buffer.update_selection(
                selection.clone(),
                BufferSelectAction::ExtendLeft,
                AutoScrollBehavior::Selection,
                ctx,
            );
            buffer.update_selection(
                selection.clone(),
                BufferSelectAction::ExtendLeft,
                AutoScrollBehavior::Selection,
                ctx,
            );
            assert_eq!(
                selection.as_ref(ctx).selection_offsets(),
                vec1![SelectionOffsets {
                    tail: 7.into(),
                    head: 1.into(),
                }]
            );
        });
    });
}

#[test]
fn test_update_selection_same_range() {
    App::test((), |mut app| async move {
        let (buffer, selection) = Buffer::mock_from_plain_text(
            "hello world",
            Box::new(|_, _| IndentBehavior::Ignore),
            &mut app,
        );

        selection.update(&mut app, |selection, _| {
            selection.set_selection_offsets(vec1![SelectionOffsets {
                tail: 1.into(),
                head: 5.into()
            }]);
        });

        let selection_changes = Arc::new(AtomicU8::new(0));
        app.update(|ctx| {
            let selection_changes = selection_changes.clone();
            ctx.subscribe_to_model(&buffer, move |_, event, _| {
                if let BufferEvent::SelectionChanged { .. } = event {
                    selection_changes.fetch_add(1, Ordering::Relaxed);
                }
            });
        });

        // Modify the selection.
        buffer.update(&mut app, |buffer, ctx| {
            buffer.update_selection(
                selection.clone(),
                BufferSelectAction::ExtendRight,
                AutoScrollBehavior::Selection,
                ctx,
            );
        });
        assert_eq!(selection_changes.load(Ordering::Relaxed), 1);

        // Make a no-op selection change.
        buffer.update(&mut app, |buffer, ctx| {
            buffer.update_selection(
                selection.clone(),
                BufferSelectAction::AddSelection {
                    head: 6.into(),
                    tail: 1.into(),
                    clear_selections: true,
                },
                AutoScrollBehavior::Selection,
                ctx,
            );
        });
        // No event should be emitted.
        assert_eq!(selection_changes.load(Ordering::Relaxed), 1);

        // Make another selection change.
        buffer.update(&mut app, |buffer, ctx| {
            buffer.update_selection(
                selection.clone(),
                BufferSelectAction::SelectAll,
                AutoScrollBehavior::Selection,
                ctx,
            );
        });
        assert_eq!(selection_changes.load(Ordering::Relaxed), 2);
    });
}

#[test]
#[allow(clippy::single_range_in_vec_init)]
#[allow(clippy::reversed_empty_ranges)]
fn test_overlapping_ranges() {
    assert_eq!(
        Buffer::overlapping_ranges(vec![1..5, 3..7, 8..10]),
        (vec![0, 1], vec![1..7])
    );
    assert_eq!(
        Buffer::overlapping_ranges(vec![1..5, 8..10, 3..7]),
        (vec![0, 1], vec![1..7])
    );
    assert_eq!(
        Buffer::overlapping_ranges(vec![1..5, 5..7]),
        (vec![0, 1], vec![1..7])
    );
    assert_eq!(
        Buffer::overlapping_ranges(vec![1..5, 5..7, 4..10, 11..15, 14..17, 20..27]),
        (vec![0, 1, 2, 3, 4], vec![1..10, 11..17])
    );
}

#[test]
fn test_insert_for_each_selection() {
    App::test((), |mut app| async move {
        let buffer = app.add_model(|_| Buffer::new(Box::new(|_, _| IndentBehavior::Ignore)));
        let selection = app.add_model(|_| BufferSelectionModel::new(buffer.clone()));

        buffer.update(&mut app, |buffer, ctx| {
            let _ = buffer.edit_internal_first_selection(
                CharOffset::from(1)..CharOffset::from(1),
                "start\nline\nend",
                Default::default(),
                selection.clone(),
                ctx,
            );

            selection.update(ctx, |selection, _| {
                set_selections(selection, vec1![6..6, 11..11, 15..15]);
            });
            let _ = buffer.edit_for_each_selection(
                &vec1![
                    ("ab".to_string(), 2),
                    ("def".to_string(), 2),
                    ("next".to_string(), 1)
                ],
                selection.clone(),
                ctx,
            );

            assert_eq!(buffer.content.debug(), "<text>startab\\nlinedef\\nendnext");
            // The cursor position should be (1) after "ab", (2) between "de" and "f", (3) between "n" and "ext".
            assert_eq!(
                selection.as_ref(ctx).selections_to_offset_ranges(),
                vec1![
                    CharOffset::from(8)..CharOffset::from(8),
                    CharOffset::from(15)..CharOffset::from(15),
                    CharOffset::from(21)..CharOffset::from(21)
                ]
            );
        });
    });
}

#[test]
fn test_dimension_conversions() {
    App::test((), |mut app| async move {
        let buffer = app.add_model(|_| Buffer::new(Box::new(|_, _| IndentBehavior::Ignore)));
        let selection = app.add_model(|_| BufferSelectionModel::new(buffer.clone()));

        buffer.update(&mut app, |buffer, ctx| {
            let _ = buffer.edit_internal_first_selection(
                CharOffset::from(1)..CharOffset::from(1),
                "  start\n    line\n  end",
                Default::default(),
                selection.clone(),
                ctx,
            );

            assert_eq!(
                Point::new(1, 3).to_buffer_char_offset(buffer),
                CharOffset::from(4)
            );

            assert_eq!(
                CharOffset::from(4).to_buffer_point(buffer),
                Point::new(1, 3)
            );

            assert_eq!(
                Point::new(1, 7).to_buffer_char_offset(buffer),
                CharOffset::from(8)
            );

            assert_eq!(
                CharOffset::from(8).to_buffer_point(buffer),
                Point::new(1, 7)
            );

            assert_eq!(
                Point::new(2, 0).to_buffer_char_offset(buffer),
                CharOffset::from(9)
            );

            assert_eq!(
                CharOffset::from(9).to_buffer_point(buffer),
                Point::new(2, 0)
            );
        });

        let buffer2 = app.add_model(|_| Buffer::new(Box::new(|_, _| IndentBehavior::Ignore)));
        let selection2 = app.add_model(|_| BufferSelectionModel::new(buffer2.clone()));

        buffer2.update(&mut app, |buffer, ctx| {
            let _ = buffer.edit_internal_first_selection(
                CharOffset::from(1)..CharOffset::from(1),
                "国际符号",
                Default::default(),
                selection2.clone(),
                ctx,
            );

            // Each CJK character should take 3 bytes + linebreak at start of buffer takes 1 byte.
            assert_eq!(
                CharOffset::from(2).to_buffer_byte_offset(buffer),
                ByteOffset::from(4)
            );

            assert_eq!(
                ByteOffset::from(4).to_buffer_char_offset(buffer),
                CharOffset::from(2)
            );

            assert_eq!(
                CharOffset::from(4).to_buffer_byte_offset(buffer),
                ByteOffset::from(10)
            );

            assert_eq!(
                ByteOffset::from(10).to_buffer_char_offset(buffer),
                CharOffset::from(4)
            );
        });
    });
}

#[test]
fn test_decorate_lines_with_prefix() {
    App::test((), |mut app| async move {
        let buffer = app.add_model(|_| {
            Buffer::new(Box::new(|_, _| {
                IndentBehavior::TabIndent(IndentUnit::Space(4))
            }))
        });
        let selection = app.add_model(|_| BufferSelectionModel::new(buffer.clone()));

        buffer.update(&mut app, |buffer, ctx| {
            let _ = buffer.edit_internal_first_selection(
                CharOffset::from(1)..CharOffset::from(1),
                "  start\n    line\n  end",
                Default::default(),
                selection.clone(),
                ctx,
            );

            selection.update(ctx, |selection, _| {
                set_selections(selection, vec1![5..21]);
            });
            let prev_selection = buffer.to_rendered_selection_set(selection.clone(), ctx);
            let edit_result =
                buffer.decorate_lines_with_prefix(vec1![1, 2, 3], "// ", selection.clone(), ctx);
            let curr_selection = buffer.to_rendered_selection_set(selection.clone(), ctx);
            buffer.push_undo_item(
                prev_selection,
                curr_selection,
                edit_result.undo_item.expect("Should have undo item"),
                UndoActionType::Atomic,
            );

            assert_eq!(
                buffer.content.debug(),
                "<text>  // start\\n  //   line\\n  // end"
            );

            // Should not affect the selections.
            assert_eq!(
                selection.as_ref(ctx).selections_to_offset_ranges(),
                vec1![CharOffset::from(8)..CharOffset::from(30)]
            );

            let _ = buffer.undo(selection.clone(), ctx);
            assert_eq!(buffer.content.debug(), "<text>  start\\n    line\\n  end");
            assert_eq!(
                selection.as_ref(ctx).selections_to_offset_ranges(),
                vec1![CharOffset::from(5)..CharOffset::from(21)]
            );
        });
    });
}

#[test]
fn test_remove_prefix_from_lines() {
    App::test((), |mut app| async move {
        let buffer = app.add_model(|_| {
            Buffer::new(Box::new(|_, _| {
                IndentBehavior::TabIndent(IndentUnit::Space(4))
            }))
        });
        let selection = app.add_model(|_| BufferSelectionModel::new(buffer.clone()));

        buffer.update(&mut app, |buffer, ctx| {
            let _ = buffer.edit_internal_first_selection(
                CharOffset::from(1)..CharOffset::from(1),
                "  // start\n    //line\n  //     end",
                Default::default(),
                selection.clone(),
                ctx,
            );

            selection.update(ctx, |selection, _| {
                set_selections(selection, vec1![5..28]);
            });
            let prev_selection = buffer.to_rendered_selection_set(selection.clone(), ctx);
            let edit_result =
                buffer.remove_prefix_from_lines(vec1![1, 2, 3], "//", selection.clone(), ctx);
            let curr_selection = buffer.to_rendered_selection_set(selection.clone(), ctx);
            buffer.push_undo_item(
                prev_selection,
                curr_selection,
                edit_result.undo_item.expect("Should have undo item"),
                UndoActionType::Atomic,
            );

            assert_eq!(
                buffer.content.debug(),
                "<text>  start\\n    line\\n      end",
            );

            // Should not affect the selections.
            assert_eq!(
                selection.as_ref(ctx).selections_to_offset_ranges(),
                vec1![CharOffset::from(3)..CharOffset::from(20)]
            );

            let _ = buffer.undo(selection.clone(), ctx);
            assert_eq!(
                buffer.content.debug(),
                "<text>  // start\\n    //line\\n  //     end"
            );
            assert_eq!(
                selection.as_ref(ctx).selections_to_offset_ranges(),
                vec1![CharOffset::from(5)..CharOffset::from(28)]
            );
        });
    });
}

#[test]
fn test_selected_lines() {
    App::test((), |mut app| async move {
        let buffer = app.add_model(|_| Buffer::new(Box::new(|_, _| IndentBehavior::Ignore)));
        let selection = app.add_model(|_| BufferSelectionModel::new(buffer.clone()));

        buffer.update(&mut app, |buffer, ctx| {
            let _ = buffer.edit_internal_first_selection(
                CharOffset::from(1)..CharOffset::from(1),
                "abc\nd\nend\n\nnew",
                Default::default(),
                selection.clone(),
                ctx,
            );
        });

        selection.update(&mut app, |selection, ctx| {
            set_selections(selection, vec1![1..2, 5..5, 10..12]);
            assert_eq!(selection.selected_lines(ctx), vec1![1, 2, 3, 4]);

            // There should be no duplicates.
            set_selections(selection, vec1![1..2, 3..4]);
            assert_eq!(selection.selected_lines(ctx), vec1![1]);
        });
    });
}

#[test]
fn test_line_prefix() {
    App::test((), |mut app| async move {
        let buffer = app.add_model(|_| {
            Buffer::new(Box::new(|_, _| {
                IndentBehavior::TabIndent(IndentUnit::Space(4))
            }))
        });
        let selection = app.add_model(|_| BufferSelectionModel::new(buffer.clone()));

        buffer.update(&mut app, |buffer, ctx| {
            let _ = buffer.edit_internal_first_selection(
                CharOffset::from(1)..CharOffset::from(1),
                "    // abc\ndef\n//line",
                Default::default(),
                selection.clone(),
                ctx,
            );

            assert!(buffer.line_decorated_with_prefix(1, "//"));
            assert!(!buffer.line_decorated_with_prefix(2, "//"));
            assert!(buffer.line_decorated_with_prefix(3, "//"));
        });
    });
}

#[test]
fn test_indent_unit() {
    App::test((), |mut app| async move {
        let buffer = app.add_model(|_| {
            Buffer::new(Box::new(|_, _| {
                IndentBehavior::TabIndent(IndentUnit::Space(4))
            }))
        });
        let selection = app.add_model(|_| BufferSelectionModel::new(buffer.clone()));

        buffer.update(&mut app, |buffer, ctx| {
            let _ = buffer.edit_internal_first_selection(
                CharOffset::from(1)..CharOffset::from(1),
                "    line\n        test",
                Default::default(),
                selection.clone(),
                ctx,
            );

            assert_eq!(
                buffer.indented_units_at_offset(CharOffset::from(5)),
                Some(1)
            );

            assert_eq!(
                buffer.indented_units_at_offset(CharOffset::from(1)),
                Some(0)
            );
        });
    });
}

#[test]
fn test_indent_with_multiple_units() {
    App::test((), |mut app| async move {
        let buffer = app.add_model(|_| {
            Buffer::new(Box::new(|_, _| {
                IndentBehavior::TabIndent(IndentUnit::Space(4))
            }))
        });
        let selection = app.add_model(|_| BufferSelectionModel::new(buffer.clone()));

        buffer.update(&mut app, |buffer, ctx| {
            let _ = buffer.edit_internal_first_selection(
                CharOffset::from(1)..CharOffset::from(1),
                "text\n line\n     test",
                Default::default(),
                selection.clone(),
                ctx,
            );

            selection.update(ctx, |selection, _| {
                set_selections(selection, vec1![1..1, 7..7, 17..17]);
            });
            let _ = buffer.indent(2, selection.clone(), ctx);
            assert_eq!(
                buffer.content.debug(),
                "<text>        text\\n        line\\n            test"
            );
        });
    });
}

#[test]
fn test_get_multibyte_selected_text() {
    App::test((), |mut app| async move {
        let buffer = app.add_model(|_| Buffer::new(Box::new(|_, _| IndentBehavior::Ignore)));
        let selection = app.add_model(|_| BufferSelectionModel::new(buffer.clone()));

        buffer.update(&mut app, |buffer, ctx| {
            buffer.edit_internal_first_selection(
                CharOffset::from(1)..CharOffset::from(1),
                "你好, my name is 文档",
                Default::default(),
                selection.clone(),
                ctx,
            );

            selection.update(ctx, |selection, _| {
                set_selections(selection, vec1![1..3]);
            });
            assert_eq!(
                buffer
                    .selected_text_as_plain_text(selection.clone(), ctx)
                    .as_str(),
                "你好"
            );

            selection.update(ctx, |selection, _| {
                set_selections(selection, vec1![2..7]);
            });
            assert_eq!(
                buffer
                    .selected_text_as_plain_text(selection.clone(), ctx)
                    .as_str(),
                "好, my"
            );

            selection.update(ctx, |selection, _| {
                set_selections(selection, vec1![15..18]);
            });
            assert_eq!(
                buffer
                    .selected_text_as_plain_text(selection.clone(), ctx)
                    .as_str(),
                " 文档"
            );
        });
    });
}

#[test]
fn test_version_match() {
    App::test((), |mut app| async move {
        let buffer = app.add_model(|_ctx| Buffer::new(Box::new(|_, _| IndentBehavior::Ignore)));
        let selection = app.add_model(|_| BufferSelectionModel::new(buffer.clone()));

        buffer.update(&mut app, |buffer, ctx| {
            let version = buffer.content_version;
            assert!(buffer.version_match(&version));

            buffer.update_content(
                BufferEditAction::Insert {
                    text: "hey there",
                    style: TextStyles::default(),
                    override_text_style: None,
                },
                EditOrigin::UserTyped,
                selection.clone(),
                ctx,
            );

            assert!(!buffer.version_match(&version));
        });
    });
}

#[test]
fn test_undo_redo_versions() {
    App::test((), |mut app| async move {
        let buffer = app.add_model(|_ctx| Buffer::new(Box::new(|_, _| IndentBehavior::Ignore)));
        let selection = app.add_model(|_| BufferSelectionModel::new(buffer.clone()));

        buffer.update(&mut app, |buffer, ctx| {
            let version1 = buffer.content_version;

            assert!(buffer.version_match(&version1));

            buffer.update_content(
                BufferEditAction::Insert {
                    text: "hey there",
                    style: TextStyles::default(),
                    override_text_style: None,
                },
                EditOrigin::UserTyped,
                selection.clone(),
                ctx,
            );

            let version2 = buffer.content_version;

            assert!(!buffer.version_match(&version1));
            assert!(buffer.version_match(&version2));

            buffer.update_content(
                BufferEditAction::Undo,
                EditOrigin::UserTyped,
                selection.clone(),
                ctx,
            );

            assert!(buffer.version_match(&version1));
            assert!(!buffer.version_match(&version2));
            assert_ne!(buffer.content_version, version1);

            buffer.update_content(
                BufferEditAction::Redo,
                EditOrigin::UserTyped,
                selection.clone(),
                ctx,
            );

            assert!(!buffer.version_match(&version1));
            assert!(buffer.version_match(&version2));
        });
    });
}

/// Regression test: after a remote file save, the server's file-watcher sends a
/// `BufferUpdatedPush` with the same content the client just saved.
/// `insert_at_char_offset_ranges` is a no-op (content unchanged) and returns early
/// without calling `set_version(new_version)`. But the caller updates
/// `base_content_version` to `new_version` anyway, creating a mismatch that makes
/// `has_unsaved_changes` return true — causing a spurious "Save changes?" dialog.
#[test]
fn test_insert_at_char_offset_ranges_noop_skips_set_version() {
    App::test((), |mut app| async move {
        let buffer = app.add_model(|_| Buffer::new(Box::new(|_, _| IndentBehavior::Ignore)));
        let selection = app.add_model(|_| BufferSelectionModel::new(buffer.clone()));

        buffer.update(&mut app, |buffer, ctx| {
            // Step 1: Simulate initial file load — populate buffer and set version.
            buffer.replace_all("hello world", ctx);
            let load_version = ContentVersion::new();
            buffer.set_version(load_version);
            assert!(buffer.version_match(&load_version));

            // Step 2: User edits the buffer.
            buffer.update_content(
                BufferEditAction::Insert {
                    text: "!",
                    style: TextStyles::default(),
                    override_text_style: None,
                },
                EditOrigin::UserTyped,
                selection.clone(),
                ctx,
            );
            assert!(!buffer.version_match(&load_version));

            // Step 3: Simulate save — capture the current buffer version as the base.
            let save_version = buffer.version();
            assert!(buffer.version_match(&save_version));

            // Step 4: Simulate post-save server push with NO edits.
            // The server file-watcher detected the save, computed a diff against
            // the buffer content, and found zero changes — so the push carries
            // an empty edit list.
            let push_version = ContentVersion::new();
            buffer.insert_at_char_offset_ranges(vec![], push_version, ctx);

            // Step 5: The caller (GlobalBufferModel) would set base_content_version = push_version.
            // version_match(push_version) should be true — the content hasn't changed.
            // BUG: insert_at_char_offset_ranges returned early without calling set_version,
            // so the buffer version is still save_version, not push_version.
            assert!(
                buffer.version_match(&push_version),
                "version_match should return true after a no-op insert_at_char_offset_ranges, \
                 but the buffer version was not updated because the no-op early return \
                 skipped set_version. Buffer version is {:?}, expected to match {:?}",
                buffer.version(),
                push_version,
            );
        });
    });
}

#[test]
fn test_insert_at_offsets() {
    App::test((), |mut app| async move {
        let buffer = app.add_model(|_| Buffer::new(Box::new(|_, _| IndentBehavior::Ignore)));
        let selection = app.add_model(|_| BufferSelectionModel::new(buffer.clone()));

        buffer.update(&mut app, |buffer, ctx| {
            buffer.edit_internal_first_selection(
                CharOffset::from(1)..CharOffset::from(1),
                "Start\nLine\nSecond",
                Default::default(),
                selection.clone(),
                ctx,
            );

            let edits = Vec1::try_from_vec(vec![
                ("abc".to_string(), CharOffset::from(2)..CharOffset::from(5)),
                ("def".to_string(), CharOffset::from(7)..CharOffset::from(11)),
            ])
            .unwrap();
            buffer.insert_at_offsets(&edits, selection.clone(), ctx);
            assert_eq!(buffer.content.debug(), "<text>Sabct\\ndef\\nSecond");

            // Edits overlapping each other should be applied in sequence.
            let edits = Vec1::try_from_vec(vec![
                ("def".to_string(), CharOffset::from(2)..CharOffset::from(5)),
                ("abc".to_string(), CharOffset::from(1)..CharOffset::from(3)),
            ])
            .unwrap();
            buffer.insert_at_offsets(&edits, selection.clone(), ctx);
            assert_eq!(buffer.content.debug(), "<text>abceft\\ndef\\nSecond");

            // Edit should not affect the selection offsets.
            selection.update(ctx, |selection, _| {
                set_selections(selection, vec1![2..5]);
            });
            let edits = Vec1::try_from_vec(vec![(
                "def".to_string(),
                CharOffset::from(1)..CharOffset::from(4),
            )])
            .unwrap();
            buffer.insert_at_offsets(&edits, selection.clone(), ctx);
            assert_eq!(buffer.content.debug(), "<text>defeft\\ndef\\nSecond");
            assert_eq!(
                selection.as_ref(ctx).selections_to_offset_ranges(),
                vec1![CharOffset::from(2)..CharOffset::from(5)]
            );
        });
    });
}

/// Regression test for WARP-CLIENT-DEV-NYY: panic "Invalid edit range 4042..3982".
///
/// Root cause: `fuzzy_match_v4a_diffs` produces `DiffDelta`s with overlapping
/// `replacement_line_range` values when multiple V4A hunks target the same
/// region of a file (confirmed by the companion test
/// `test_v4a_maa_crash_d71bf84b_no_overlapping_deltas` in the `ai` crate).
///
/// `CodeEditorModel::apply_diffs` converts those line ranges to char offsets
/// and passes them to `insert_at_offsets`, which feeds them into
/// `apply_core_edit_actions` without validating.  The invalid range reaches
/// `Buffer::edit`, which panics on the `debug_assert!`.
///
/// This test passes the exact Sentry crash values (`4042..3982`) to
/// `insert_at_offsets` to confirm the editor does not defend against bad
/// input from the diff layer.
#[test]
fn test_insert_at_offsets_overlapping_ranges_skipped() {
    App::test((), |mut app| async move {
        let buffer = app.add_model(|_| Buffer::new(Box::new(|_, _| IndentBehavior::Ignore)));
        let selection = app.add_model(|_| BufferSelectionModel::new(buffer.clone()));

        buffer.update(&mut app, |buffer, ctx| {
            // Populate the buffer with enough content.
            let content = (0..200)
                .map(|i| format!("line_{:03}_content_padding", i))
                .collect::<Vec<_>>()
                .join("\n");
            buffer.edit_internal_first_selection(
                CharOffset::from(1)..CharOffset::from(1),
                &content,
                Default::default(),
                selection.clone(),
                ctx,
            );

            let original_text = buffer.text().into_string();

            // Pass a range with start > end (the exact Sentry crash values).
            // After the fix in apply_core_edit_actions, the inverted range
            // should be skipped gracefully instead of panicking.
            let edits = Vec1::try_from_vec(vec![(
                "replacement\n".to_string(),
                CharOffset::from(4042)..CharOffset::from(3982),
            )])
            .unwrap();

            buffer.insert_at_offsets(&edits, selection.clone(), ctx);

            // Buffer should be unchanged — the invalid edit was skipped.
            assert_eq!(buffer.text().into_string(), original_text);
        });
    });
}

#[test]
fn test_from_plain_text() {
    App::test((), |mut app| async move {
        let buffer = app.add_model(|_| Buffer::new(Box::new(|_, _| IndentBehavior::Ignore)));
        let selection = app.add_model(|_| BufferSelectionModel::new(buffer.clone()));
        buffer.update(&mut app, |buffer, ctx| {
            *buffer = Buffer::from_plain_text(
                "Hello\nWorld",
                Box::new(|_, _| IndentBehavior::Ignore),
                selection.clone(),
                ctx,
            );
        });

        buffer.update(&mut app, |buffer, _ctx| {
            // Ensure that there is a text marker at the start of the buffer.
            assert_eq!(buffer.content.debug(), "<text>Hello\\nWorld");
        });

        selection.read(&app, |selection, _| {
            // Ensure that the cursor is at the end of the buffer.
            assert_eq!(
                selection.selections_to_offset_ranges(),
                vec1![CharOffset::from(12)..CharOffset::from(12)]
            );
        });
    });
}

#[test]
fn test_unindent_panic_simpler_case() {
    // This verifies unindent does not invalidate selection anchors.
    App::test((), |mut app| async move {
        let buffer = app.add_model(|_| {
            Buffer::new(Box::new(|_, _| {
                IndentBehavior::TabIndent(IndentUnit::Space(4))
            }))
        });
        let selection = app.add_model(|_| BufferSelectionModel::new(buffer.clone()));

        buffer.update(&mut app, |buffer, ctx| {
            // Insert simple indented text
            buffer.edit_internal_first_selection(
                CharOffset::from(1)..CharOffset::from(1),
                "    let x = 42;",
                TextStyles::default(),
                selection.clone(),
                ctx,
            );

            // Position cursor in middle of the indentation
            selection.update(ctx, |selection, _| {
                set_selections(selection, vec1![3..3]);
            });

            // Should not panic; anchors within the removed indentation are clamped
            buffer.unindent(selection.clone(), ctx);

            // Indentation should be removed
            assert_eq!(buffer.debug(), "<text>let x = 42;");

            // Ensure rendering selections does not panic
            let _ = buffer.to_rendered_selection_set(selection.clone(), ctx);
        });
    });
}
