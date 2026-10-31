// Adding this file level gate as some of the code around editability is not used in WASM yet.
use std::collections::{HashMap, HashSet};
use std::fmt::Debug;
use std::ops::Range;
use std::path::Path;

use lazy_static::lazy_static;
use num_traits::SaturatingSub;
use pathfinder_color::ColorU;
use rangemap::RangeSet;
use settings::Setting as _;
use string_offset::CharOffset;
use vec1::{Vec1, vec1};
use vim::vim::{Direction, InsertPosition, VimMode, VimModel, VimState, VimSubscriber};
use warp_core::platform::SessionPlatform;
use warp_editor::content::buffer::{
    AutoScrollBehavior, Buffer, BufferSelectAction, EditOrigin, SelectionOffsets, ToBufferPoint,
};
use warp_editor::content::text::LineCount as BufferLineCount;
use warp_editor::content::version::BufferVersion;
use warp_editor::model::CoreEditorModel;
use warp_editor::multiline::AnyMultilineString;
use warp_editor::render::element::{
    DisplayOptions, DisplayStateHandle, RichTextElement, VerticalExpansionBehavior,
};
use warp_editor::render::model::viewport::ScrollPositionSnapshot;
use warp_editor::render::model::{
    AutoScrollMode, BlockSpacing, BlockSpacings, CODE_EDITOR_HIDDEN_SECTION_EXPANSION_LINES,
    ExpansionType, InlineCodeStyle, LineCount, ParagraphStyles, RichTextStyles,
};
use warp_editor::search::{SearchEvent, Searcher};
use warp_util::content_version::ContentVersion;
use warp_util::standardized_path::StandardizedPath;
use warpui::elements::new_scrollable::{
    AxisConfiguration, DualAxisConfig, NewScrollableElement, ScrollableAppearance,
};
use warpui::elements::{
    ChildView, Dismiss, Fill, Flex, Margin, NewScrollable, Padding, ParentElement,
    ScrollStateHandle, Shrinkable, Stack,
};
use warpui::event::ModifiersState;
use warpui::keymap::Keystroke;
use warpui::units::Pixels;
use warpui::{
    AppContext, BlurContext, CursorInfo, Element, Entity, FocusContext, ModelHandle,
    SingletonEntity, View, ViewContext, ViewHandle, WeakViewHandle,
};

use crate::appearance::Appearance;
use crate::code::editor::element::{
    EditorWrapper, EditorWrapperStateHandle, GutterRange, LineNumberConfig,
};
use crate::code::editor::find::view::{CodeEditorFind as Find, Event as FindViewEvent};
use crate::code::editor::goto_line::view::{Event as GoToLineEvent, GoToLineView};
use crate::code::editor::model::{
    CodeEditorModel, CodeEditorModelEvent, DiffNavigationState, LineBound,
};
use crate::code::editor::nav_bar::{NavBar, NavBarEvent};
use crate::code::editor::scroll::{ScrollTrigger, ScrollWheelBehavior};
use crate::editor::InteractionState;
use crate::features::FeatureFlag;
use crate::settings::{AppEditorSettings, CodeEditorLineNumberMode, FontSettings};
use crate::util::color::{ContrastingColor, MinimumAllowedContrast};
use crate::view_components::find::FindDirection;

mod actions;
pub(super) use actions::CodeEditorViewAction;
pub use actions::init;

mod vim_handler;

/// Disable editing keybindings for read-only editors.
const NON_EDITABLE_KEYMAP_CONTEXT: &str = "NonEditableKeymapContext";

lazy_static! {
    static ref AUTOCOMPLETE_SYMBOLS: HashMap<char, char> =
        HashMap::from([('(', ')'), ('[', ']'), ('{', '}'), ('\'', '\''), ('"', '"'),]);
    static ref CLOSING_SYMBOLS: HashSet<char> = AUTOCOMPLETE_SYMBOLS.values().cloned().collect();
}

pub enum CodeEditorEvent {
    Focused,
    ContentChanged {
        origin: EditOrigin,
    },
    SelectionChanged,
    SelectionStart,
    SelectionEnd,
    /// Emitted when the editor is in Vim mode and a user Escape while already in Normal mode.
    VimEscapeInNormalMode,
    /// Emitted when escape key is pressed (regardless of Vim state).
    /// This allows parent views to handle escape for closing overlays.
    EscapePressed,
    /// Emitted when diff decorations are updated (line highlights, removed lines, etc.)
    DiffUpdated,
    /// Emitted when a diff hunk is reverted
    HiddenSectionExpanded,
    /// Emitted when the viewport is updated after layout
    ViewportUpdated,
    DelayedRenderingFlushed,
    /// Emitted when the render state layout has been updated.
    LayoutInvalidated,
}

/// Store all states related to displaying the editor content.
#[derive(Default)]
struct DisplayHandles {
    vertical_scroll_state: ScrollStateHandle,
    horizontal_scroll_state: ScrollStateHandle,
    display_state: DisplayStateHandle,
    wrapper_state_handle: EditorWrapperStateHandle,
}

#[derive(Debug, Clone, Copy)]
struct CodeEditorViewDisplayOptions {
    vertical_expansion_behavior: VerticalExpansionBehavior,
    scroll_wheel_behavior: ScrollWheelBehavior,
    horizontal_scrollbar_appearance: ScrollableAppearance,
    vertical_scrollbar_appearance: ScrollableAppearance,
    // Provides a starting line number for markdown code blocks.
    starting_line_number: Option<usize>,
}

#[derive(Debug)]
pub struct CodeEditorRenderOptions {
    vertical_expansion_behavior: VerticalExpansionBehavior,
}

impl CodeEditorRenderOptions {
    pub fn new(vertical_expansion_behavior: VerticalExpansionBehavior) -> Self {
        Self {
            vertical_expansion_behavior,
        }
    }
}

pub struct CodeEditorView {
    pub model: ModelHandle<CodeEditorModel>,
    pub searcher: ModelHandle<Searcher>,
    find_bar: ViewHandle<Find>,
    goto_line_dialog: ViewHandle<GoToLineView>,
    nav_bar: ViewHandle<NavBar>,
    display_states: DisplayHandles,
    is_selecting: bool,
    self_handle: WeakViewHandle<Self>,
    display_options: CodeEditorViewDisplayOptions,
    pending_scroll: Option<ScrollTrigger>,
    pending_review_state: Option<(BufferVersion, ReviewEditorState)>,
    supports_vim_mode: bool,
    vim_model: ModelHandle<VimModel>,
    // Track the most recent Vim search direction to determine how to cycle (n/N) thereafter.
    last_search_direction: Direction,
}

/// Interaction state retained while a collapsed review card releases its editor and text.
pub(crate) struct ReviewEditorState {
    selections: Vec1<SelectionOffsets>,
    visible_offsets: RangeSet<CharOffset>,
    scroll: ScrollPositionSnapshot,
    horizontal_scroll: Pixels,
}

impl CodeEditorView {
    pub(crate) fn review_state(&self, ctx: &AppContext) -> ReviewEditorState {
        let model = self.model.as_ref(ctx);
        let render = model.render_state().as_ref(ctx);
        ReviewEditorState {
            selections: model.selections(ctx),
            visible_offsets: render.viewport_charoffset_range(),
            scroll: render.snapshot_scroll_position(),
            horizontal_scroll: render.viewport().scroll_left(),
        }
    }

    pub(crate) fn restore_review_state(&mut self, state: ReviewEditorState, ctx: &AppContext) {
        // As with pending_scroll, wait until the replacement content has reached layout.
        self.pending_review_state = Some((self.model.as_ref(ctx).buffer_version(ctx), state));
    }

    fn apply_review_state(&mut self, state: ReviewEditorState, ctx: &mut ViewContext<Self>) {
        self.model.update(ctx, |model, ctx| {
            let buffer = model.buffer().as_ref(ctx);
            let max = buffer.max_charoffset();
            let clamp = |offset: CharOffset| offset.min(max).max(CharOffset::from(1));
            let selections = state.selections.mapped(|s| SelectionOffsets {
                head: clamp(s.head),
                tail: clamp(s.tail),
            });
            // Reopening recomputes collapsed context; keep the old viewport and selection visible.
            let visible_lines: Vec<_> = state
                .visible_offsets
                .into_iter()
                .chain(
                    selections
                        .iter()
                        .map(|s| s.head.min(s.tail)..s.head.max(s.tail)),
                )
                .map(|range| {
                    BufferLineCount::from(
                        clamp(range.start)
                            .to_buffer_point(buffer)
                            .row
                            .saturating_sub(1) as usize,
                    )
                        ..BufferLineCount::from(
                            clamp(range.end).to_buffer_point(buffer).row as usize,
                        )
                })
                .collect();
            let hidden = model
                .diff()
                .as_ref(ctx)
                .unchanged_line_ranges(model.line_count(ctx) + 1, 3);
            for range in visible_lines {
                // The reveal API handles one hidden section per call.
                for section in hidden.overlapping(&range) {
                    model.set_visible_line_range(
                        range.start.max(section.start)..range.end.min(section.end),
                        ctx,
                    );
                }
            }
            model.selection().update(ctx, |selection, ctx| {
                selection.update_selection(
                    BufferSelectAction::SetSelectionOffsets { selections },
                    AutoScrollBehavior::None,
                    ctx,
                );
            });
            model.render_state().update(ctx, |render, ctx| {
                render.scroll_to(state.scroll);
                render.scroll_horizontal(
                    render.viewport().scroll_left() - state.horizontal_scroll,
                    ctx,
                );
            });
        });
    }

    /// Replace a read-only review snapshot without touching a shared file buffer.
    pub(crate) fn set_review_content(
        &mut self,
        base: &str,
        current: &str,
        ctx: &mut ViewContext<Self>,
    ) {
        use warp_editor::multiline::{LF, MultilineString};
        self.model.update(ctx, |model, ctx| {
            model
                .buffer()
                .update(ctx, |buffer, ctx| buffer.replace_all(current, ctx));
            model.set_interaction_state(InteractionState::Selectable, ctx);
            let version = model.buffer_version(ctx);
            model.diff().update(ctx, |diff, ctx| {
                diff.set_base(MultilineString::<LF>::apply(base));
                diff.compute_diff(MultilineString::<LF>::apply(current), version, ctx);
            });
            model.show_review_diff(ctx);
        });
        ctx.notify();
    }

    /// A [`SessionPlatform`] is used to determine the default line ending for
    /// the code editor. It should be provided if the code editor can modify
    /// files on the user's system.
    pub fn new(
        session_platform: Option<SessionPlatform>,
        buffer: Option<ModelHandle<Buffer>>,
        render_options: CodeEditorRenderOptions,
        ctx: &mut ViewContext<Self>,
    ) -> Self {
        let appearance_handle = Appearance::handle(ctx);
        let font_settings_handle = FontSettings::handle(ctx);
        let initial_styles = code_text_styles(appearance_handle.as_ref(ctx));
        ctx.subscribe_to_model(&appearance_handle, |me, _, _, ctx| {
            me.handle_appearance_or_font_change(ctx);
        });
        ctx.subscribe_to_model(&font_settings_handle, |me, _, _, ctx| {
            me.handle_appearance_or_font_change(ctx);
        });
        let app_editor_settings_handle = AppEditorSettings::handle(ctx);
        ctx.subscribe_to_model(&app_editor_settings_handle, |_, _, _, ctx| {
            ctx.notify();
        });

        let model = ctx
            .add_model(|ctx| CodeEditorModel::new(initial_styles, session_platform, buffer, ctx));
        ctx.subscribe_to_model(&model, |me, _, event, ctx| {
            me.handle_editor_model_event(event, ctx);
        });

        // Creates a new model for searching the editor
        let buffer = model.as_ref(ctx).buffer().clone();
        let selection_model = model.as_ref(ctx).buffer_selection_model().clone();
        let searcher = ctx.add_model(|ctx| Searcher::new(buffer, selection_model, ctx));
        // Subscribes to events from Searcher model so that we can handle
        ctx.subscribe_to_model(&searcher, |me, _, event, ctx| {
            me.handle_searcher_event(event, ctx);
        });

        let find_bar = ctx.add_typed_action_view(|ctx| Find::new(searcher.clone(), ctx));
        ctx.subscribe_to_view(&find_bar, move |me, _, event, ctx| {
            me.handle_find_event(event, ctx);
        });

        let goto_line_dialog = ctx.add_typed_action_view(GoToLineView::new);
        ctx.subscribe_to_view(&goto_line_dialog, |me, _, event, ctx| {
            me.handle_goto_line_event(event, ctx);
        });

        let nav_bar = ctx.add_typed_action_view(|ctx| NavBar::new(model.clone(), ctx));
        ctx.subscribe_to_view(&nav_bar, |me, _, event, ctx| match event {
            NavBarEvent::Close => {
                me.toggle_diff_nav(None, ctx);
                ctx.notify();
            }
        });

        // If feature flag is enabled, enable vim mode.
        let supports_vim_mode = FeatureFlag::VimCodeEditor.is_enabled();

        let vim_model = ctx.add_model(|_| VimModel::new());
        ctx.subscribe_to_model(&vim_model, Self::handle_vim_event);

        // Ensure CodeEditorView starts in Normal mode when Vim keybindings are enabled.
        if supports_vim_mode && AppEditorSettings::as_ref(ctx).vim_mode_enabled() {
            vim_model.update(ctx, |vim_model, ctx| {
                if let Ok(escape) = Keystroke::parse("escape") {
                    vim_model.keypress(&escape, ctx);
                }
            });
        }
        // Ensure that we re-render when the rendering model changes.
        ctx.observe(&model.as_ref(ctx).render_state().clone(), |_, _, ctx| {
            ctx.notify();
        });

        Self {
            searcher,
            find_bar,
            goto_line_dialog,
            model,
            display_states: Default::default(),
            is_selecting: false,
            self_handle: ctx.handle(),
            nav_bar,
            display_options: CodeEditorViewDisplayOptions {
                vertical_expansion_behavior: render_options.vertical_expansion_behavior,
                starting_line_number: None,
                scroll_wheel_behavior: ScrollWheelBehavior::AlwaysHandle,
                // By default, we should render the horizontal scrollbar as overlay to prevent it
                // from truncating space for the code editor. We should not render it as an overlay
                // for small code editors.
                horizontal_scrollbar_appearance: ScrollableAppearance::new(
                    warpui::elements::ScrollbarWidth::Auto,
                    false,
                ),
                vertical_scrollbar_appearance: ScrollableAppearance::new(
                    warpui::elements::ScrollbarWidth::Auto,
                    false,
                ),
            },
            pending_scroll: None,
            pending_review_state: None,
            supports_vim_mode,
            vim_model,
            last_search_direction: Direction::Forward,
        }
    }

    fn find_bar_open(&self, ctx: &mut ViewContext<Self>) -> bool {
        self.find_bar.as_ref(ctx).is_open()
    }

    fn show_find_bar(&mut self, ctx: &mut ViewContext<Self>) {
        let find_bar = &self.find_bar;

        // Pre-populate the search field with the currently selected text if it's a single line
        let should_populate_query = self.model.as_ref(ctx).selection_is_single_range(ctx);

        if should_populate_query {
            let selection_offsets = *self.model.as_ref(ctx).selections(ctx).first();
            let selected_text = self
                .model
                .as_ref(ctx)
                .buffer()
                .as_ref(ctx)
                .text_in_range(selection_offsets.head..selection_offsets.tail)
                .into_string();
            // Only use the selected text if it's a single line (no newlines)
            if !selected_text.contains('\n') && !selected_text.trim().is_empty() {
                find_bar.update(ctx, |find_bar, ctx| {
                    find_bar.set_find_query(ctx, &selected_text);
                });
            }
        }

        find_bar.update(ctx, |find_bar, _ctx| {
            find_bar.set_open(true);
        });
        self.update_decorations_and_position(ctx);
        ctx.focus(find_bar);
        ctx.notify();
    }

    pub fn close_find_bar(&mut self, should_focus_editor: bool, ctx: &mut ViewContext<Self>) {
        {
            let find_bar = &self.find_bar;
            let should_update = find_bar.update(ctx, |find_bar, _ctx| {
                if find_bar.is_open() {
                    find_bar.set_open(false);
                    true
                } else {
                    false
                }
            });

            if should_update {
                self.update_decorations_and_position(ctx);
            }
        }

        if should_focus_editor {
            self.focus(ctx);
        }

        ctx.notify();
    }

    fn show_goto_line(&mut self, ctx: &mut ViewContext<Self>) {
        if self.find_bar_open(ctx) {
            self.close_find_bar(false, ctx);
        }
        self.goto_line_dialog.update(ctx, |dialog, ctx| {
            dialog.open(ctx);
        });
        ctx.focus(&self.goto_line_dialog);
        ctx.notify();
    }

    fn close_goto_line_dialog(&mut self, ctx: &mut ViewContext<Self>) {
        self.goto_line_dialog.update(ctx, |dialog, ctx| {
            dialog.close(ctx);
        });
        self.focus(ctx);
        ctx.notify();
    }

    fn handle_goto_line_event(&mut self, event: &GoToLineEvent, ctx: &mut ViewContext<Self>) {
        match event {
            GoToLineEvent::Close => {
                self.close_goto_line_dialog(ctx);
            }
            GoToLineEvent::Confirm { input } => {
                let trimmed = input.trim().to_string();
                if trimmed.is_empty() {
                    self.goto_line_dialog.update(ctx, |dialog, ctx| {
                        dialog.set_error("Please enter a line number".to_string(), ctx);
                    });
                    return;
                }
                let (line_str, col_str) = match trimmed.split_once(':') {
                    Some((l, c)) => (l, Some(c)),
                    None => (trimmed.as_str(), None),
                };
                let line_number = match line_str.parse::<usize>() {
                    Ok(n) if n >= 1 => n,
                    _ => {
                        self.goto_line_dialog.update(ctx, |dialog, ctx| {
                            dialog.set_error("Please enter a valid line number".to_string(), ctx);
                        });
                        return;
                    }
                };
                let column = match col_str {
                    Some(c) if !c.is_empty() => match c.parse::<usize>() {
                        Ok(n) => Some(n),
                        Err(_) => {
                            self.goto_line_dialog.update(ctx, |dialog, ctx| {
                                dialog.set_error(
                                    "Please enter a valid column number".to_string(),
                                    ctx,
                                );
                            });
                            return;
                        }
                    },
                    _ => None,
                };
                let line_count = self.model.as_ref(ctx).line_count(ctx);
                let clamped_line = line_number.min(line_count + 1);
                self.model.update(ctx, |model, ctx| {
                    model.jump_to_line_column(clamped_line, column, ctx);
                });
                self.close_goto_line_dialog(ctx);
            }
        }
    }

    fn escape(&mut self, ctx: &mut ViewContext<Self>) {
        if self.goto_line_dialog.as_ref(ctx).is_open() {
            self.close_goto_line_dialog(ctx);
            return;
        }

        // If find bar is open, close it (editor handles escape)
        if self.find_bar_open(ctx) {
            self.close_find_bar(true, ctx);
            return;
        }

        // If vim mode is enabled, let vim handle escape
        if self.vim_mode_enabled(ctx) {
            self.enter_vim_normal_mode(ctx);
            return;
        }

        // Editor didn't handle escape - emit event for parent views (e.g., close overlays)
        ctx.emit(CodeEditorEvent::EscapePressed);
    }

    fn update_decorations_and_position(&self, ctx: &mut ViewContext<Self>) {
        // Updating search decorations based on matches
        let search_decorations = match self.find_bar_open(ctx) {
            true => self.searcher.as_ref(ctx).result_decorations(),
            false => Vec::new(),
        };
        self.model.update(ctx, |model, ctx| {
            model.render_state().update(ctx, |render, ctx| {
                render.set_text_decorations(search_decorations, ctx);
            });
        });

        // Updating position on code editor view based on match location
        if let Some(autoscroll_match) = self.searcher.as_ref(ctx).selected_match_range() {
            self.model
                .as_ref(ctx)
                .render_state()
                .clone()
                .update(ctx, |render_state, _ctx| {
                    render_state.request_autoscroll_to(AutoScrollMode::ScrollOffsetsIntoViewport(
                        autoscroll_match.clone(),
                    ));
                });
        }
    }

    pub fn set_pending_scroll(&mut self, trigger: ScrollTrigger) {
        self.pending_scroll = Some(trigger);
    }

    #[allow(clippy::single_range_in_vec_init)]
    fn expand_hidden_section(
        &mut self,
        line_range: Range<LineCount>,
        expansion_type: &ExpansionType,
        ctx: &mut ViewContext<Self>,
    ) {
        let hidden_section_start = line_range.start.as_usize();
        let hidden_section_end = line_range.end.as_usize();
        let lines_to_unhide = match expansion_type {
            ExpansionType::Both => {
                warp_editor::content::text::LineCount::from(hidden_section_start)
                    ..warp_editor::content::text::LineCount::from(hidden_section_end)
            }
            ExpansionType::ExpandDown => {
                let end = hidden_section_end
                    .min(hidden_section_start + CODE_EDITOR_HIDDEN_SECTION_EXPANSION_LINES);
                warp_editor::content::text::LineCount::from(hidden_section_start)
                    ..warp_editor::content::text::LineCount::from(end)
            }
            ExpansionType::ExpandUp => {
                let start = hidden_section_start.max(
                    hidden_section_end.saturating_sub(CODE_EDITOR_HIDDEN_SECTION_EXPANSION_LINES),
                );
                warp_editor::content::text::LineCount::from(start)
                    ..warp_editor::content::text::LineCount::from(hidden_section_end)
            }
        };
        self.model.update(ctx, |model, ctx| {
            model.set_visible_line_range(lines_to_unhide, ctx);
        });
        ctx.emit(CodeEditorEvent::HiddenSectionExpanded);
    }

    pub(crate) fn with_horizontal_scrollbar_appearance(
        mut self,
        scrollbar_appearance: ScrollableAppearance,
    ) -> Self {
        self.display_options.horizontal_scrollbar_appearance = scrollbar_appearance;
        self
    }

    fn handle_searcher_event(&mut self, _event: &SearchEvent, ctx: &mut ViewContext<Self>) {
        // Only update decorations if the find bar is open.
        if self.find_bar_open(ctx) {
            // TODO: Add some indicator (maybe a border?) for invalid queries.
            self.update_decorations_and_position(ctx);
        }
    }

    /// Move the editor cursor to the start of the currently-selected search match, if any.
    fn move_cursor_to_selected_match(&mut self, ctx: &mut ViewContext<Self>) {
        if let Some(match_range) = self.searcher.as_ref(ctx).selected_match_range() {
            self.model.update(ctx, |model, ctx| {
                model.selection().update(ctx, |selection_model, ctx| {
                    selection_model.set_cursor(match_range.start, ctx);
                });
            });
        }
    }

    fn handle_find_event(&mut self, event: &FindViewEvent, ctx: &mut ViewContext<Self>) {
        let find_bar = &self.find_bar;

        match event {
            FindViewEvent::CloseFindBar => {
                // If vim mode is enabled and there is a selected search match, move the cursor
                // to the start of that match before closing the find bar.
                if self.vim_mode_enabled(ctx) {
                    self.move_cursor_to_selected_match(ctx);
                }
                self.close_find_bar(true, ctx);
            }
            FindViewEvent::Update { query } => {
                let query_str = query.clone().unwrap_or_default();
                self.run_find(&query_str, ctx);
            }
            FindViewEvent::NextMatch { direction } => {
                self.searcher.update(ctx, |searcher, ctx| match direction {
                    FindDirection::Up => searcher.select_previous_result(ctx),
                    FindDirection::Down => searcher.select_next_result(ctx),
                });

                self.move_cursor_to_selected_match(ctx);

                // After navigating via the find bar, only return focus to the editor in Vim mode.
                if self.vim_mode_enabled(ctx) {
                    self.focus(ctx);
                }
            }
            FindViewEvent::VimEnterAndFocusEditor => {
                // Shift focus back to the main editor when Enter is pressed in Vim mode in the find bar.
                // Select the nearest match from the cursor position and move to it.
                self.searcher
                    .update(ctx, |searcher, ctx| match self.last_search_direction {
                        Direction::Forward => searcher.select_next_from_cursor(ctx),
                        Direction::Backward => searcher.select_prev_from_cursor(ctx),
                    });
                self.move_cursor_to_selected_match(ctx);
                self.focus(ctx);
                ctx.notify();
            }
            FindViewEvent::SelectAll => {
                // Get all search results from the find model
                if let Some(results) = self.searcher.as_ref(ctx).results()
                    && !results.matches.is_empty()
                {
                    // Convert all match ranges to selection offsets
                    let selection_offsets: Vec<warp_editor::content::buffer::SelectionOffsets> =
                        results
                            .matches
                            .iter()
                            .map(
                                |match_result| warp_editor::content::buffer::SelectionOffsets {
                                    head: match_result.end,
                                    tail: match_result.start,
                                },
                            )
                            .collect();

                    // Set multiple selections on the editor to highlight all matches
                    if let Ok(selections) = vec1::Vec1::try_from_vec(selection_offsets) {
                        self.model.update(ctx, |model, ctx| {
                                model.selection().update(ctx, |selection_model, ctx| {
                                    selection_model.update_selection(
                                        warp_editor::content::buffer::BufferSelectAction::SetSelectionOffsets { selections },
                                        warp_editor::content::buffer::AutoScrollBehavior::Selection,
                                        ctx,
                                    );
                                });
                            });
                    }
                }
                self.close_find_bar(true, ctx);
            }
            FindViewEvent::ReplaceSelected => {
                let replace_query =
                    find_bar.update(ctx, |find_bar, ctx| find_bar.replace_query(ctx));
                if let Some(match_range) = self.searcher.as_ref(ctx).selected_match_range() {
                    let final_replace_text = if find_bar.as_ref(ctx).is_preserve_case_enabled() {
                        self.preserve_case(match_range.clone(), &replace_query, ctx)
                    } else {
                        replace_query.clone()
                    };

                    let edits = vec1![(final_replace_text, match_range)];
                    let selection_model = self.model.as_ref(ctx).buffer_selection_model().clone();
                    // Replace current selection in the editor with replace query
                    self.model.update(ctx, |model, ctx| {
                        model.update_content( |mut content_model, ctx| {
                            content_model.apply_edit(
                                warp_editor::content::buffer::BufferEditAction::InsertAtCharOffsetRanges { edits: &edits },
                                EditOrigin::UserInitiated,
                                selection_model,
                                ctx,
                            )
                        }, ctx);
                    });
                    // After selection is replaced, emit an accessibility announcement
                    find_bar.update(ctx, |find_bar, ctx| {
                        find_bar.emit_replace_a11y_content(ctx);
                    })
                }
            }
            FindViewEvent::ReplaceAll => {
                let (replace_query, preserve_case_enabled) =
                    find_bar.update(ctx, |find_bar, ctx| {
                        (
                            find_bar.replace_query(ctx),
                            find_bar.is_preserve_case_enabled(),
                        )
                    });
                if !replace_query.is_empty() {
                    // Get all search results from the find model
                    if let Some(results) = self.searcher.as_ref(ctx).results()
                        && !results.matches.is_empty()
                    {
                        // Convert all match ranges to selection offsets with case preservation
                        let edits: Vec<(String, Range<CharOffset>)> = results
                            .matches
                            .iter()
                            .map(|match_result| {
                                let match_range = Range {
                                    start: match_result.start,
                                    end: match_result.end,
                                };

                                let final_replace_text = if preserve_case_enabled {
                                    self.preserve_case(match_range.clone(), &replace_query, ctx)
                                } else {
                                    replace_query.clone()
                                };

                                (final_replace_text, match_range)
                            })
                            .collect();

                        if let Ok(edits) = vec1::Vec1::try_from_vec(edits) {
                            let selection_model =
                                self.model.as_ref(ctx).buffer_selection_model().clone();
                            // Replace selections in the editor with replace query
                            self.model.update(ctx, |model, ctx| {
                                    model.update_content(|mut content_model, ctx| {
                                        content_model.apply_edit(
                                            warp_editor::content::buffer::BufferEditAction::InsertAtCharOffsetRanges { edits: &edits },
                                            EditOrigin::UserInitiated,
                                            selection_model,
                                            ctx,
                                        )
                                    }, ctx);
                                });
                        }
                    }
                }
            }
        }
    }

    fn line_number_config(&self, ctx: &AppContext) -> LineNumberConfig {
        let appearance = Appearance::as_ref(ctx);
        let theme = appearance.theme();
        let editor_settings = AppEditorSettings::as_ref(ctx);
        LineNumberConfig {
            font_family: appearance.monospace_font_family(),
            font_size: appearance.monospace_font_size(),
            text_color: theme.sub_text_color(theme.background()).into(),
            highlight_text_color: theme.main_text_color(theme.background()).into(),
            starting_line_number: self.display_options.starting_line_number,
            mode: *editor_settings.code_editor_line_number_mode.value(),
            active_line_number: self.active_cursor_line_for_line_numbers(ctx),
            active_cursor_is_visible: self.is_focused(ctx) && self.is_editable(ctx),
        }
    }

    fn active_cursor_line_for_line_numbers(&self, ctx: &AppContext) -> Option<LineCount> {
        let model = self.model.as_ref(ctx);
        let selection = *model.selections(ctx).first();
        let buffer = model.content().as_ref(ctx);
        let point = selection.head.to_buffer_point(buffer);
        // `LineCount`s used by render blocks are zero-based, while buffer points report rows using
        // the editor's one-based convention.
        Some(LineCount::from(point.row.saturating_sub(1) as usize))
    }

    fn run_find(&mut self, query: &str, ctx: &mut ViewContext<Self>) {
        self.searcher.update(ctx, |searcher, ctx| {
            searcher.set_query(query.to_string(), ctx);
        });
    }

    fn handle_editor_model_event(
        &mut self,
        event: &CodeEditorModelEvent,
        ctx: &mut ViewContext<Self>,
    ) {
        match event {
            CodeEditorModelEvent::DiffUpdated => {
                self.nav_bar.update(ctx, |_, ctx| ctx.notify());
                ctx.emit(CodeEditorEvent::DiffUpdated);
                ctx.notify()
            }
            CodeEditorModelEvent::SyntaxHighlightingUpdated => {
                self.nav_bar.update(ctx, |_, ctx| ctx.notify());
                ctx.notify()
            }
            CodeEditorModelEvent::SelectionChanged => {
                self.reset_for_editing_change();
                self.vim_maybe_enforce_cursor_line_cap(ctx);
                ctx.emit(CodeEditorEvent::SelectionChanged);
                if *AppEditorSettings::as_ref(ctx)
                    .code_editor_line_number_mode
                    .value()
                    == CodeEditorLineNumberMode::Relative
                {
                    // Repaint relative line-number gutters when the cursor origin changes.
                    ctx.notify();
                }
            }
            CodeEditorModelEvent::ContentChanged { origin } => {
                if origin.from_user() {
                    self.reset_for_editing_change();
                    self.vim_maybe_enforce_cursor_line_cap(ctx);
                }
                ctx.emit(CodeEditorEvent::ContentChanged { origin: *origin });
            }
            CodeEditorModelEvent::ViewportUpdated(version) => {
                if let Some((_, state)) = self
                    .pending_review_state
                    .take_if(|(minimum_version, _)| *minimum_version <= *version)
                {
                    self.apply_review_state(state, ctx);
                }
                if let Some(trigger) = self
                    .pending_scroll
                    .take_if(|trigger| trigger.minimum_applicable_version <= *version)
                {
                    self.jump_to_line_column(
                        trigger.position.line_num,
                        trigger.position.column_num,
                        ctx,
                    );
                }
                ctx.emit(CodeEditorEvent::ViewportUpdated);
            }
            CodeEditorModelEvent::InteractionStateChanged => (),
            CodeEditorModelEvent::DelayedRenderingFlushed => {
                ctx.emit(CodeEditorEvent::DelayedRenderingFlushed);
            }
            CodeEditorModelEvent::LayoutInvalidated => {
                ctx.emit(CodeEditorEvent::LayoutInvalidated);
            }
        }
    }

    pub fn toggle_diff_nav(
        &self,
        line_range: Option<Range<LineCount>>,
        ctx: &mut ViewContext<Self>,
    ) {
        if *self.model.as_ref(ctx).diff_navigation_state() == DiffNavigationState::Review {
            return;
        }
        let toggle_to_on = self
            .model
            .update(ctx, |model, ctx| model.toggle_diff_nav(line_range, ctx));

        if toggle_to_on {
            self.nav_bar.update(ctx, |nav_bar, ctx| {
                nav_bar.autoscroll(ctx);
            });
        }
    }

    /// Handles [`Appearance`] changes by updating the render model.
    fn handle_appearance_or_font_change(&mut self, ctx: &mut ViewContext<Self>) {
        let new_styles = code_text_styles(Appearance::as_ref(ctx));
        self.model.update(ctx, move |model, ctx| {
            model.handle_appearance_or_font_change(new_styles, ctx);
        });
    }

    pub fn is_focused(&self, app: &AppContext) -> bool {
        let Some(handle) = self.self_handle.upgrade(app) else {
            return false;
        };

        // If our window is not active, we don't have user focus, even if we're focused within the app.
        if app.windows().state().active_window != Some(handle.window_id(app)) {
            return false;
        }

        //  Check if the editor is focused directly.
        if handle.is_focused(app) {
            return true;
        }

        false
    }

    pub fn focus(&self, ctx: &mut ViewContext<Self>) {
        ctx.focus_self();
        ctx.emit(CodeEditorEvent::Focused);
    }

    fn selection_start(
        &mut self,
        offset: CharOffset,
        modifiers: ModifiersState,
        ctx: &mut ViewContext<Self>,
    ) {
        // Clicking into the editor should restore focus.
        self.focus(ctx);

        let multiselect = modifiers.alt && FeatureFlag::RichTextMultiselect.is_enabled();
        self.model.update(ctx, |model, ctx| {
            model.select_at(offset, multiselect, ctx);
        });
        self.is_selecting = true;
        ctx.emit(CodeEditorEvent::SelectionStart);
    }

    /// Extend the selection that is currently being dragged.  This should be called after
    /// `selection_start` and before `selection_end`.
    fn selection_update(&mut self, offset: CharOffset, ctx: &mut ViewContext<Self>) {
        self.model.update(ctx, |model, ctx| {
            model.update_pending_selection(offset, ctx);
        });
    }

    fn selection_end(&mut self, ctx: &mut ViewContext<Self>) {
        self.is_selecting = false;
        self.model.update(ctx, |model, ctx| {
            model.end_selection(ctx);
        });
        ctx.emit(CodeEditorEvent::SelectionEnd);
        ctx.notify();
    }

    /// Extend the selection to the given offset.  This is used for shift-clicking to extend the
    /// selection, and not for dragging the selection.
    fn selection_extend(&mut self, offset: CharOffset, ctx: &mut ViewContext<Self>) {
        self.model.update(ctx, |model, ctx| {
            model.set_last_selection_head(offset, ctx);
        });
    }

    pub fn set_language_with_path(&mut self, path: &StandardizedPath, ctx: &mut ViewContext<Self>) {
        self.model.update(ctx, |model, ctx| {
            model.set_language_with_path(path, ctx);
        });
    }

    pub fn set_language_with_local_path(&mut self, path: &Path, ctx: &mut ViewContext<Self>) {
        self.model.update(ctx, |model, ctx| {
            model.set_language_with_local_path(path, ctx);
        });
    }

    fn jump_to_line_column(&self, line: usize, column: Option<usize>, ctx: &mut ViewContext<Self>) {
        self.model.update(ctx, |model, ctx| {
            model.jump_to_line_column(line, column, ctx)
        })
    }

    fn reset_for_editing_change(&mut self) {
        self.display_states.display_state.reset_cursor_blink_timer();
    }

    pub fn text(&self, ctx: &AppContext) -> AnyMultilineString {
        self.model.as_ref(ctx).content_string(ctx)
    }

    /// Check whether the given version matches the version of the underlying buffer.
    pub fn version_match(&self, version: &ContentVersion, ctx: &AppContext) -> bool {
        self.model
            .as_ref(ctx)
            .content()
            .as_ref(ctx)
            .version_match(version)
    }

    /// Return the ContentVersion of the underlying `Buffer`.
    pub fn version(&self, ctx: &AppContext) -> ContentVersion {
        self.model.as_ref(ctx).content().as_ref(ctx).version()
    }

    pub fn buffer_version(&self, ctx: &AppContext) -> BufferVersion {
        self.model.as_ref(ctx).buffer_version(ctx)
    }

    pub fn interaction_state(&self, ctx: &AppContext) -> InteractionState {
        self.model.as_ref(ctx).interaction_state()
    }

    pub fn is_editable(&self, app: &AppContext) -> bool {
        self.model.as_ref(app).interaction_state() == InteractionState::Editable
    }

    /// Splits text based on pascal case, camel case, hyphens, or underscores.
    /// Returns a tuple of (parts, delimiter) where delimiter is Some(char) if a delimiter was used.
    fn split_text(text: &str) -> (Vec<String>, Option<char>) {
        if text.is_empty() {
            return (vec![], None);
        }

        // Check for hyphen or underscore delimiters first
        if text.contains('-') {
            let parts = text.split('-').map(|s| s.to_string()).collect();
            return (parts, Some('-'));
        }
        if text.contains('_') {
            let parts = text.split('_').map(|s| s.to_string()).collect();
            return (parts, Some('_'));
        }

        // Split on camelCase/PascalCase boundaries
        let mut parts = Vec::new();
        let mut current_part = String::new();
        let mut chars = text.chars().peekable();

        while let Some(ch) = chars.next() {
            if ch.is_uppercase() && !current_part.is_empty() {
                // Check if this is a sequence of uppercase letters (like "HTML" in "HTMLParser")
                if let Some(&next_ch) = chars.peek() {
                    if next_ch.is_lowercase() {
                        // This is the start of a new word (like "P" in "HTMLParser")
                        parts.push(current_part);
                        current_part = ch.to_string();
                    } else {
                        // Continue the current part for sequences like "HTML"
                        current_part.push(ch);
                    }
                } else {
                    // Last character - check if it should be part of the previous uppercase sequence
                    if current_part
                        .chars()
                        .last()
                        .is_some_and(|c| c.is_uppercase())
                    {
                        // Previous char was uppercase, append to current part
                        current_part.push(ch);
                    } else {
                        // Previous char was lowercase, start new part
                        parts.push(current_part);
                        current_part = ch.to_string();
                    }
                }
            } else {
                current_part.push(ch);
            }
        }

        if !current_part.is_empty() {
            parts.push(current_part);
        }

        // No delimiter used for camelCase/PascalCase
        (parts, None)
    }

    /// Preserves the case of a single part based on the original part's casing.
    fn preserve_case_for_part(original_part: &str, replace_part: &str) -> String {
        if original_part.chars().all(|c| c.is_lowercase()) {
            replace_part.to_lowercase()
        } else if original_part.chars().all(|c| c.is_uppercase()) {
            replace_part.to_uppercase()
        } else if let Some(first_original) = original_part.chars().next() {
            if first_original.is_uppercase() {
                // Preserve uppercase first character
                let mut result = String::new();
                let mut replace_chars = replace_part.chars();
                if let Some(mut first_replace) = replace_chars.next() {
                    first_replace = if first_replace.is_uppercase() {
                        first_replace
                    } else {
                        first_replace.to_uppercase().next().unwrap_or(first_replace)
                    };
                    result.push(first_replace);
                    result.push_str(&replace_chars.collect::<String>().to_lowercase());
                }
                result
            } else {
                // Mixed case but starts with lowercase - keep replace_part as-is
                replace_part.to_string()
            }
        } else {
            // Empty original part - keep replace_part as-is
            replace_part.to_string()
        }
    }

    /// Whether the editor needs a vertical constraint when rendering. This is when the editor
    /// has a vertical expansion behavior that is not infinite height.
    pub fn needs_vertical_constraint(&self) -> bool {
        !matches!(
            self.display_options.vertical_expansion_behavior,
            VerticalExpansionBehavior::InfiniteHeight
        )
    }

    /// Preserves the case pattern of the original text when replacing it with new text.
    /// The case pattern is preserved in several ways:
    /// - For simple text: "begin" -> "end" will preserve uppercase ("Begin" -> "End")
    ///   and all-caps ("BEGIN" -> "END")
    /// - For camelCase/PascalCase: The replacement text's casing pattern is used
    ///   ("oneTwoThree" -> "fourFiveSix", "OneTwoThree" -> "FourFiveSix")
    /// - Delimiters like hyphens and underscores are preserved
    fn preserve_case(
        &self,
        original_range: Range<CharOffset>,
        replace_text: &str,
        ctx: &AppContext,
    ) -> String {
        // Get the original text from the buffer
        let original_text = self
            .model
            .as_ref(ctx)
            .buffer()
            .as_ref(ctx)
            .text_in_range(original_range.start..original_range.end)
            .into_string();

        // Split both original and replacement text
        let (mut original_parts, mut original_delimiter) = Self::split_text(&original_text);
        let (mut replace_parts, replace_delimiter) = Self::split_text(replace_text);
        // If the replaced text is delimited differently than incoming text, we shouldn't split either of them.
        if original_delimiter != replace_delimiter {
            (original_parts, original_delimiter) = (vec![original_text.to_string()], None);
            replace_parts = vec![replace_text.to_string()];
        }

        // Apply case preservation to each part
        let mut result_parts = Vec::new();
        for (i, replace_part) in replace_parts.iter().enumerate() {
            if let Some(original_part) = original_parts.get(i) {
                result_parts.push(Self::preserve_case_for_part(original_part, replace_part));
            } else {
                // If we have more replacement parts than original parts, keep them as-is
                result_parts.push(replace_part.clone());
            }
        }

        // Join the parts back together using the original delimiter if it exists
        let delimiter = original_delimiter
            .map(|d| d.to_string())
            .unwrap_or_default();
        result_parts.join(&delimiter)
    }

    /// If vim keybindings are enabled, return the [`VimMode`]. Otherwise, return None.
    pub fn vim_mode(&self, ctx: &AppContext) -> Option<VimMode> {
        self.vim_state(ctx).map(|state| state.mode)
    }

    /// If vim keybindings are enabled, return the [`VimState`]. Otherwise, return None.
    pub fn vim_state<'a>(&self, ctx: &'a AppContext) -> Option<VimState<'a>> {
        self.vim_mode_enabled(ctx)
            .then(|| self.vim_model.as_ref(ctx).state())
    }

    pub fn vim_mode_enabled(&self, ctx: &AppContext) -> bool {
        self.supports_vim_mode && AppEditorSettings::as_ref(ctx).vim_mode_enabled()
    }

    pub fn enter_vim_normal_mode(&mut self, ctx: &mut ViewContext<Self>) {
        if self.vim_mode_enabled(ctx) {
            self.vim_escape(ctx);
        }
    }

    /// Send the 'escape' keystroke to the VimFSA.
    fn vim_escape(&mut self, ctx: &mut ViewContext<Self>) {
        self.vim_keystroke(&Keystroke::parse("escape").expect("escape parses"), ctx)
    }

    fn vim_apply_insert_position(
        &mut self,
        position: &InsertPosition,
        ctx: &mut ViewContext<Self>,
    ) {
        match position {
            InsertPosition::AtCursor => {}
            InsertPosition::AfterCursor => {
                self.model.update(ctx, |model, ctx| {
                    model.vim_move_horizontal_by_offset(
                        1,
                        &Direction::Forward,
                        false, // keep_selection
                        true,  // stop_at_line_boundary
                        ctx,
                    );
                });
            }
            InsertPosition::LineFirstNonWhitespace => self.model.update(ctx, |model, ctx| {
                model.vim_move_to_first_nonwhitespace(false, ctx);
            }),
            InsertPosition::LineEnd => self.model.update(ctx, |model, ctx| {
                model.vim_move_to_line_bound(LineBound::End, false, ctx);
            }),
            InsertPosition::LineAbove => {
                self.model.update(ctx, |model, ctx| {
                    model.vim_newline(true, ctx);
                });
            }
            InsertPosition::LineBelow => {
                self.model.update(ctx, |model, ctx| {
                    model.vim_newline(false, ctx);
                });
            }
        }
    }

    /// When in Vim mode, specifically normal mode, the block cursor cannot go past the last
    /// character on the line as the beam cursor can. We call this "line capping." This helper
    /// method determines if line capping needs to be enforced, and if so, enforces it.
    fn vim_maybe_enforce_cursor_line_cap(&mut self, ctx: &mut ViewContext<Self>) {
        if let Some(VimMode::Normal) = self.vim_mode(ctx)
            && self.model.as_ref(ctx).vim_needs_line_capping(ctx)
        {
            self.model.update(ctx, |model, ctx| {
                model.vim_enforce_cursor_line_cap(ctx);
            });
        }
    }

    fn user_insert(&mut self, typed: &str, ctx: &mut ViewContext<Self>) {
        if ctx.is_self_focused() {
            if let Some(first_char) = typed.chars().next()
                && typed.chars().count() == 1
            {
                let all_cursors_next_character_matches_char =
                    self.model.update(ctx, |model, ctx| {
                        model.all_cursors_next_character_matches_char(first_char, ctx)
                    });

                // If the character is a closing symbol, we want to potentially step over it incase it's already a closing symbol.
                if CLOSING_SYMBOLS.contains(&first_char) && all_cursors_next_character_matches_char
                {
                    let buffer = self.model.as_ref(ctx).buffer().as_ref(ctx);
                    let selection_model =
                        self.model.as_ref(ctx).buffer_selection_model().as_ref(ctx);
                    if selection_model.all_single_cursors() {
                        let selections = selection_model.selection_offsets();
                        let should_step_over = selections
                            .iter()
                            .all(|sel| buffer.char_at(sel.head).is_some_and(|c| c == first_char));
                        if should_step_over {
                            self.model.update(ctx, |model, ctx| {
                                model.selection_model().update(ctx, |selection, ctx| {
                                    selection.update_selection(
                                        warp_editor::content::buffer::BufferSelectAction::MoveRight,
                                        warp_editor::content::buffer::AutoScrollBehavior::Selection,
                                        ctx,
                                    );
                                });
                            });
                            return;
                        }
                    }
                }

                // If the character is opening autocomplete symbol, we want to autocomplete it with a closing symbol.
                if let Some(close) = AUTOCOMPLETE_SYMBOLS.get(&first_char) {
                    self.model.update(ctx, |model, ctx| {
                        model.autocomplete_symbol(first_char, *close, ctx);
                    });
                    return;
                }
            }

            self.model.update(ctx, |model, ctx| {
                model.user_insert(typed, ctx);
            });
        }
    }

    fn delete_line_left(&self, ctx: &mut ViewContext<Self>) {
        self.model.update(ctx, |model, ctx| {
            model.delete_all_left(ctx);
        })
    }

    fn vim_user_insert(&mut self, text: &str, ctx: &mut ViewContext<Self>) {
        // Only handle Vim input when this editor is focused.
        if !ctx.is_self_focused() {
            return;
        }
        self.vim_model.update(ctx, |vim_model, ctx| {
            for c in text.chars() {
                vim_model.typed_character(c, ctx);
            }
        });
    }

    /// Similar to Self::vim_user_insert, but for keystrokes which aren't represented by a char.
    pub fn vim_keystroke(&mut self, keystroke: &Keystroke, ctx: &mut ViewContext<Self>) {
        // Only handle Vim keystrokes when this editor is focused.
        if !ctx.is_self_focused() {
            return;
        }
        self.vim_model.update(ctx, |vim_model, ctx| {
            vim_model.keypress(keystroke, ctx);
        });
    }
}

impl Entity for CodeEditorView {
    type Event = CodeEditorEvent;
}

impl View for CodeEditorView {
    fn ui_name() -> &'static str {
        "CodeEditorView"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let editable = self.is_editable(app);

        let focused = self.is_focused(app);
        let blink_cursors = AppEditorSettings::as_ref(app).cursor_blink_enabled();

        let display_options = DisplayOptions {
            debug_bounds: false,
            focused,
            blink_cursors,
            vertical_expansion_behavior: self.display_options.vertical_expansion_behavior,
            editable,
            ..Default::default()
        };

        let line_number_config = self.line_number_config(app);

        let diff_status = self.model.as_ref(app).diff_status(app);

        let render_state = self.model.as_ref(app).render_state();

        // Align Vim visual tails with the render state's selection coordinate system.
        // The render model stores selection head/tail as (buffer_offset - 1), so apply the same
        // transformation to the visual tails to avoid off-by-one mismatches when highlighting.
        let vim_visual_tails = self
            .model
            .as_ref(app)
            .vim_visual_tails()
            .iter()
            .map(|t| t.saturating_sub(&CharOffset::from(1)))
            .collect::<Vec<_>>();
        let editor_rich_content = RichTextElement::<Self>::new(
            render_state.clone(),
            self.self_handle.clone(),
            display_options,
            self.display_states.display_state.clone(),
            self.vim_mode(app),
            vim_visual_tails,
        );

        let code_editor = EditorWrapper::new(
            editor_rich_content,
            self.display_options.vertical_expansion_behavior,
            line_number_config,
            diff_status,
            self.display_states.wrapper_state_handle.clone(),
            Box::new(move |click, ctx| match click {
                GutterRange::DiffHunk {
                    line,
                    in_sliver: true,
                } => ctx.dispatch_typed_action(CodeEditorViewAction::ToggleDiffNav(Some(
                    line.line_range().clone(),
                ))),
                GutterRange::HiddenSection {
                    line: line_info,
                    expansion_type,
                } => ctx.dispatch_typed_action(CodeEditorViewAction::HiddenSectionExpansion {
                    line_range: line_info.line_range().clone(),
                    expansion_type,
                }),
                _ => {}
            }),
            self.display_options
                .scroll_wheel_behavior
                .should_handle(focused),
            self.model.as_ref(app).diff_navigation_state().clone(),
            if let Some(index) = self.model.as_ref(app).focused_diff_index() {
                self.model
                    .as_ref(app)
                    .diff()
                    .as_ref(app)
                    .line_range_by_diff_hunk_index(index)
                    .map(|line_count| {
                        LineCount::from(line_count.start)..LineCount::from(line_count.end)
                    })
            } else {
                None
            },
        );

        let config = DualAxisConfig::Manual {
            horizontal: AxisConfiguration::Manual(
                self.display_states.horizontal_scroll_state.clone(),
            ),
            vertical: AxisConfiguration::Manual(self.display_states.vertical_scroll_state.clone()),
            child: NewScrollableElement::finish_scrollable(code_editor),
        };

        let scrollable = NewScrollable::horizontal_and_vertical(
            config,
            theme.disabled_text_color(theme.background()).into(),
            theme.main_text_color(theme.background()).into(),
            Fill::None,
        )
        .with_horizontal_scrollbar(self.display_options.horizontal_scrollbar_appearance)
        .with_vertical_scrollbar(self.display_options.vertical_scrollbar_appearance)
        .with_propagate_mousewheel_if_not_handled(true)
        .finish();

        let inner = match self.display_options.vertical_expansion_behavior {
            VerticalExpansionBehavior::InfiniteHeight => scrollable,
            VerticalExpansionBehavior::FillMaxHeight
            | VerticalExpansionBehavior::GrowToMaxHeight => {
                Shrinkable::new(1., scrollable).finish()
            }
        };
        let mut col = Flex::column().with_child(inner);
        if self.model.as_ref(app).diff_nav_is_active()
            && *self.model.as_ref(app).diff_navigation_state() != DiffNavigationState::Review
        {
            col.add_child(ChildView::new(&self.nav_bar).finish());
        }

        let mut stack = Stack::new()
            .with_constrain_absolute_children()
            .with_child(col.finish());
        if self.find_bar.as_ref(app).is_open() {
            stack.add_overlay_child(ChildView::new(&self.find_bar).finish());
        }
        if self.goto_line_dialog.as_ref(app).is_open() {
            let dialog = Dismiss::new(ChildView::new(&self.goto_line_dialog).finish())
                .on_dismiss(|ctx, _app| {
                    ctx.dispatch_typed_action(CodeEditorViewAction::Escape);
                })
                .finish();
            stack.add_overlay_child(dialog);
        }

        stack.finish()
    }

    fn on_focus(&mut self, focus_ctx: &FocusContext, ctx: &mut ViewContext<Self>) {
        if focus_ctx.is_self_focused() && self.goto_line_dialog.as_ref(ctx).is_open() {
            ctx.focus(&self.goto_line_dialog);
        }
    }

    fn active_cursor_position(&self, ctx: &ViewContext<Self>) -> Option<CursorInfo> {
        let render_state = self.model.as_ref(ctx).render_state().as_ref(ctx);
        let cursor_id = render_state.saved_positions().cursor_id();
        let font_size = render_state.styles().base_text.font_size;

        ctx.element_position_by_id(cursor_id.as_str())
            .map(|position| CursorInfo {
                position,
                font_size,
            })
    }

    fn on_blur(&mut self, blur_ctx: &BlurContext, ctx: &mut ViewContext<Self>) {
        if blur_ctx.is_self_blurred() {
            ctx.notify();
        }
    }

    fn keymap_context(&self, app: &AppContext) -> warpui::keymap::Context {
        let mut context = Self::default_keymap_context();

        if self.interaction_state(app) != InteractionState::Editable {
            context.set.insert(NON_EDITABLE_KEYMAP_CONTEXT);
        }
        if let Some(vim_mode) = self.vim_mode(app) {
            context.set.insert("Vim");
            match vim_mode {
                VimMode::Normal => {
                    context.set.insert("VimNormalMode");
                }
                VimMode::Visual(_) => {
                    context.set.insert("VimVisualMode");
                }
                _ => {}
            }
        }
        context.set.insert("FindBarAvailable");
        context
    }
}

pub fn code_text_styles(appearance: &Appearance) -> RichTextStyles {
    let theme = appearance.theme();
    let base_text = ParagraphStyles {
        font_size: appearance.monospace_font_size(),
        line_height_ratio: appearance.line_height_ratio(),
        font_family: appearance.monospace_font_family(),
        font_weight: Default::default(),
        text_color: theme.main_text_color(theme.background()).into_solid(),
        baseline_ratio: 0.8,
        fixed_width_tab_size: Some(4),
    };
    let block_spacings = BlockSpacings {
        text: BlockSpacing {
            margin: Margin::uniform(0.).with_left(1.),
            padding: Padding::uniform(0.),
        },
    };
    RichTextStyles {
        base_text,
        placeholder_color: theme.hint_text_color(theme.background()).into_solid(),
        selection_fill: theme.text_selection_color().into(),
        cursor_fill: theme
            .cursor()
            .on_background(theme.background(), MinimumAllowedContrast::Text)
            .into(),
        inline_code_style: InlineCodeStyle {
            font_family: appearance.monospace_font_family(),
            background: theme.surface_3().into(),
            font_color: ColorU::from(theme.terminal_colors().normal.red)
                .on_background(theme.surface_3().into(), MinimumAllowedContrast::Text),
        },
        block_spacings,
        show_placeholder_text_on_empty_block: false,
        minimum_paragraph_height: None,
        cursor_width: 2.,
        highlight_urls: false,
    }
}

#[cfg(feature = "integration_tests")]
impl CodeEditorView {
    pub fn open_goto_line_for_test(&mut self, ctx: &mut ViewContext<Self>) {
        self.show_goto_line(ctx);
    }

    pub fn goto_line_confirm_for_test(&mut self, input: &str, ctx: &mut ViewContext<Self>) {
        self.show_goto_line(ctx);
        let event = GoToLineEvent::Confirm {
            input: input.to_string(),
        };
        self.handle_goto_line_event(&event, ctx);
    }

    pub fn displayed_line_number_for_test(
        &self,
        one_based_line_number: usize,
        ctx: &AppContext,
    ) -> Option<usize> {
        let line_number_config = self.line_number_config(ctx);
        let line_count = LineCount::from(one_based_line_number.checked_sub(1)?);

        Some(line_number_config.display_line_number(line_count))
    }
}

#[cfg(test)]
mod view_tests;
