use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use parking_lot::Mutex;
use pathfinder_color::ColorU;
use pathfinder_geometry::rect::RectF;
use pathfinder_geometry::vector::{Vector2F, vec2f};
use warp_core::ui::appearance::Appearance;
use warp_core::ui::theme::Fill;
use warp_core::ui::theme::color::internal_colors;
use warp_editor::content::buffer::{Buffer, ToBufferPoint};
use warp_editor::editor::EditorView;
use warp_editor::render::element::{RichTextElement, VerticalExpansionBehavior};
use warp_editor::render::model::{
    BlockLocation, ExpansionType, LineCount, RenderState, WidthSetting,
    gutter_expansion_button_types,
};
use warpui::elements::new_scrollable::{NewScrollableElement, ScrollableAxis};
use warpui::elements::{
    Align, Axis, ConstrainedBox, Container, Empty, F32Ext, Point, ScrollData, Text, ZIndex,
};
use warpui::event::DispatchedEvent;
use warpui::fonts::FamilyId;
use warpui::units::Pixels;
use warpui::{
    AfterLayoutContext, AppContext, ClipBounds, Element, Event, EventContext, LayoutContext,
    ModelHandle, PaintContext, SingletonEntity, SizeConstraint,
};

use super::diff::{DiffHunkDisplay, DiffStatus};
use super::model::DiffNavigationState;
use crate::code::editor::line::EditorLineLocation;
use crate::settings::CodeEditorLineNumberMode;

pub const GUTTER_WIDTH: f32 = 94.;
const VERTICAL_DIFF_HUNK_INDICATOR_WIDTH: f32 = 3.;
const VERTICAL_DIFF_HUNK_INDICATOR_HOVERED_WIDTH: f32 = 8.;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeType {
    Add,
    Remove,
}

#[derive(Debug, Clone, Copy)]
pub enum GutterElementType {
    DiffHunk {
        hunk: Option<DiffHunkDisplay>,
        change_type: ChangeType,
    },
    HiddenSection {
        expansion_type: ExpansionType,
    },
}

struct GutterElement {
    element: Box<dyn Element>,
    offset: Pixels,
    height: f32,
    hovered: bool,
    line: EditorLineLocation,
    element_type: GutterElementType,
    /// Optional background fill for removed-line (temporary) blocks.
    overlay: Option<Fill>,
}

impl GutterElement {
    /// Checks if the given position falls within this gutter element's bounds.
    /// Returns a GutterRange if the position is contained, None otherwise.
    fn contains_position(
        &self,
        position: Vector2F,
        wrapper_origin: Vector2F,
    ) -> Option<GutterRange> {
        let gutter_origin = wrapper_origin + vec2f(0., self.offset.as_f32());

        match self.element_type {
            GutterElementType::HiddenSection { expansion_type } => {
                let does_contain = {
                    // Hidden sections use the full gutter width
                    let size = vec2f(GUTTER_WIDTH, self.height);
                    RectF::new(gutter_origin, size).contains_point(position)
                };
                if does_contain {
                    Some(GutterRange::HiddenSection {
                        line: self.line.clone(),
                        expansion_type,
                    })
                } else {
                    None
                }
            }
            GutterElementType::DiffHunk { .. } => {
                let does_contain = {
                    // For diff hunks, check if position is in the sliver or the full gutter
                    // Get the sliver size (always use expanded=true for hit testing)
                    let sliver_size = self.diff_hunk_size(true)?;
                    let sliver_rect = RectF::new(gutter_origin, sliver_size);
                    sliver_rect.contains_point(position)
                };

                if does_contain {
                    // Position is in the sliver
                    Some(GutterRange::DiffHunk {
                        line: self.line.clone(),
                        in_sliver: true,
                    })
                } else {
                    // Check if position is in the full gutter width (but not in sliver)
                    let full_gutter_rect =
                        RectF::new(gutter_origin, vec2f(GUTTER_WIDTH, self.height));
                    if full_gutter_rect.contains_point(position) {
                        Some(GutterRange::DiffHunk {
                            line: self.line.clone(),
                            in_sliver: false,
                        })
                    } else {
                        None
                    }
                }
            }
        }
    }

    /// The size of the diff hunk element in gutter (if exists).
    fn diff_hunk_size(&self, gutter_element_is_hovered: bool) -> Option<Vector2F> {
        match self.element_type {
            GutterElementType::HiddenSection { .. } => {
                // Hidden section elements are always horizontal.
                Some(vec2f(GUTTER_WIDTH, self.height))
            }
            GutterElementType::DiffHunk { hunk: ref diff, .. } => {
                let vertical_indicator_width = if gutter_element_is_hovered {
                    VERTICAL_DIFF_HUNK_INDICATOR_HOVERED_WIDTH
                } else {
                    VERTICAL_DIFF_HUNK_INDICATOR_WIDTH
                };

                diff.as_ref()
                    .map(|_| vec2f(vertical_indicator_width, self.height))
            }
        }
    }

    // The color of the diff hunk indicator element in gutter (if it exists).
    fn diff_indicator_color(&self, diff_hunks_are_expanded: bool) -> Option<ColorU> {
        if let GutterElementType::DiffHunk {
            hunk: Some(diff_hunk),
            change_type,
        } = self.element_type
        {
            match &diff_hunk {
                DiffHunkDisplay::Add(color) | DiffHunkDisplay::Remove(color) => Some(*color),
                DiffHunkDisplay::Replacement {
                    collapsed_color: change_color,
                    add_color,
                    remove_color,
                } => {
                    if diff_hunks_are_expanded {
                        match change_type {
                            ChangeType::Add => Some(*add_color),
                            ChangeType::Remove => Some(*remove_color),
                        }
                    } else {
                        Some(*change_color)
                    }
                }
            }
        } else {
            None
        }
    }
}

/// States that need to live in between frames.
#[derive(Default)]
pub struct EditorWrapperState {
    /// The line range of the hovered diff hunk.
    hovered_diff_hunk: Mutex<Option<EditorLineLocation>>,
    /// Whether there is an active click.
    in_click: AtomicBool,
}

pub type EditorWrapperStateHandle = Arc<EditorWrapperState>;

pub enum GutterRange {
    DiffHunk {
        line: EditorLineLocation,
        // Whether the position is in the sliver (the colored part) of the diff hunk element.
        in_sliver: bool,
    },
    HiddenSection {
        line: EditorLineLocation,
        expansion_type: ExpansionType,
    },
}

impl GutterRange {
    pub fn line(&self) -> &EditorLineLocation {
        match self {
            GutterRange::DiffHunk { line, .. } => line,
            GutterRange::HiddenSection { line, .. } => line,
        }
    }
}

/// The caller defined handler for a click event.
type EditorWrapperClickHandler = Box<dyn FnMut(GutterRange, &mut EventContext)>;

/// UI Config for rendering line number.
pub struct LineNumberConfig {
    pub font_family: FamilyId,
    pub font_size: f32,
    pub text_color: ColorU,
    pub highlight_text_color: ColorU,
    pub starting_line_number: Option<usize>,
    pub show_numbers: bool,
    pub mode: CodeEditorLineNumberMode,
    pub active_line_number: Option<LineCount>,
    pub active_cursor_is_visible: bool,
}
impl LineNumberConfig {
    pub fn absolute_line_number(&self, line_count: LineCount) -> usize {
        line_count.as_usize() + self.starting_line_number.unwrap_or(1)
    }

    pub fn display_line_number(&self, line_count: LineCount) -> usize {
        if self.mode == CodeEditorLineNumberMode::Relative
            && let Some(active_line_number) = self.active_line_number
            && active_line_number != line_count
        {
            return active_line_number
                .as_usize()
                .abs_diff(line_count.as_usize());
        }

        self.absolute_line_number(line_count)
    }
}

pub struct EditorWrapper<V: EditorView> {
    editor: RichTextElement<V>,
    buffer: ModelHandle<Buffer>,
    element_size: Option<Vector2F>,
    element_origin: Option<Point>,
    /// Whether the editor should expand vertically to fill the available space.
    vertical_expansion_behavior: VerticalExpansionBehavior,
    line_number_config: LineNumberConfig,
    gutter_elements: Option<Vec<GutterElement>>,
    diff_status: DiffStatus,
    state_handle: EditorWrapperStateHandle,
    click_handler: EditorWrapperClickHandler,
    /// The current state of diff navigation in the editor
    diff_navigation_state: DiffNavigationState,
    /// The line range of the focused diff hunk (if any)
    focused_diff_line_range: Option<Range<LineCount>>,
    should_handle_scroll_wheel: bool,
    /// This helps us handle events properly on stacks. A stack will always
    /// put its children on higher z-indexes than its origin, so a hit test using the standard
    /// `z_index` method would always result in the event being covered (by the children of the
    /// stack). Instead, we track the upper-bound of z-indexes _contained by_ the child element.
    /// Then we use that upper bound to do the hit testing, which means a parent will always get
    /// events from its children, regardless of whether they are stacks or not.
    child_max_z_index: Option<ZIndex>,
}

impl<V: EditorView> EditorWrapper<V> {
    fn paint_removed_line_overlays(
        &self,
        origin: Vector2F,
        wrapper_size: Vector2F,
        ctx: &mut PaintContext,
    ) {
        // Removed lines: group consecutive Remove-type gutter elements by line_range and
        // draw one full-width rect per group.
        let Some(gutter_elements) = &self.gutter_elements else {
            return;
        };

        struct Group {
            start_y: f32,
            end_y: f32,
            line_range: Range<LineCount>,
            overlay: Fill,
        }

        let mut group: Option<Group> = None;

        let mut flush = |group: &mut Option<Group>| {
            let Some(group) = group.take() else {
                return;
            };
            ctx.scene
                .draw_rect_without_hit_recording(RectF::new(
                    origin + vec2f(0., group.start_y),
                    vec2f(wrapper_size.x(), group.end_y - group.start_y),
                ))
                .with_background(group.overlay);
        };

        for element in gutter_elements.iter() {
            let is_remove = matches!(
                element.element_type,
                GutterElementType::DiffHunk {
                    change_type: ChangeType::Remove,
                    ..
                }
            );

            if !is_remove {
                flush(&mut group);
                continue;
            }

            let Some(overlay) = element.overlay else {
                flush(&mut group);
                continue;
            };

            let current_range = element.line.line_range().clone();
            let start_y = element.offset.as_f32();
            let end_y = start_y + element.height;

            match &mut group {
                Some(group) if group.line_range == current_range && group.overlay == overlay => {
                    group.end_y = end_y;
                }
                _ => {
                    flush(&mut group);
                    group = Some(Group {
                        start_y,
                        end_y,
                        line_range: current_range,
                        overlay,
                    });
                }
            }
        }

        flush(&mut group);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new(
        editor: RichTextElement<V>,
        buffer: ModelHandle<Buffer>,
        vertical_expansion_behavior: VerticalExpansionBehavior,
        line_number_config: LineNumberConfig,
        diff_status: DiffStatus,
        state_handle: EditorWrapperStateHandle,
        click_handler: EditorWrapperClickHandler,
        should_handle_scroll_wheel: bool,
        diff_navigation_state: DiffNavigationState,
        focused_diff_line_range: Option<Range<LineCount>>,
    ) -> Self {
        Self {
            editor,
            buffer,
            vertical_expansion_behavior,
            element_size: None,
            element_origin: None,
            line_number_config,
            gutter_elements: None,
            diff_status,
            state_handle,
            diff_navigation_state,
            focused_diff_line_range,
            click_handler,
            should_handle_scroll_wheel,
            child_max_z_index: None,
        }
    }

    /// True iff the diff hunks are expanded in the underlying editor model.
    fn diff_hunks_are_expanded(&self) -> bool {
        !matches!(self.diff_navigation_state, DiffNavigationState::Collapsed)
    }

    fn should_display_relative_line_number(&self) -> bool {
        let line_number_config = &self.line_number_config;
        if line_number_config.mode != CodeEditorLineNumberMode::Relative
            || line_number_config.active_line_number.is_none()
        {
            return false;
        }

        // Relative numbers follow the cursor: only show them when a cursor is
        // actually drawn (editor focused and editable).
        line_number_config.active_cursor_is_visible
    }

    fn gutter_elements(&self, app: &AppContext) -> Vec<GutterElement> {
        let appearance = Appearance::as_ref(app);
        let line_number_config = &self.line_number_config;
        let Some(blocks) = self.editor.blocks() else {
            return Vec::new();
        };

        let mut elements = Vec::new();
        let model = self.model().as_ref(app);
        let wrapped_buffer =
            (model.width_setting() == &WidthSetting::FitViewport).then(|| self.buffer.as_ref(app));
        let hovered_range = self.state_handle.hovered_diff_hunk.lock();
        let line_decorations = model.decorations().line_decoration_ranges();
        let last_block_idx = blocks.len().saturating_sub(1);
        // Track the index of removal blocks with the same line_count
        let mut removed_hunk_line_number: Option<LineCount> = None;
        let mut removed_hunk_line_index: usize = 0;
        for (block_idx, block) in blocks.iter().enumerate() {
            let Some(mut line_count) = model.start_line_index(&**block) else {
                continue;
            };
            if let Some(buffer) = wrapped_buffer
                && !block.is_hidden_section()
            {
                // Render offsets omit the buffer's leading newline marker.
                let point = (block.viewport_item().block_offset() + 1).to_buffer_point(buffer);
                line_count = LineCount::from(point.row.saturating_sub(1) as usize);
            }

            let offset = block.viewport_item().viewport_offset;
            let diff_hunk = self.diff_status.diff_hunk(line_count, appearance);
            let is_removal = matches!(diff_hunk, Some(DiffHunkDisplay::Remove(_)));

            let current_line = if self.should_display_relative_line_number() {
                line_number_config.display_line_number(line_count)
            } else {
                line_number_config.absolute_line_number(line_count)
            };

            // If the block is temporary, don't render line number.
            // Currently, all temporary blocks are removal hunks, either from a deleted section,
            // or the old lines from a replacement hunk.
            if block.is_temporary() {
                let diff_range = self.diff_status.removed_diff_range(line_count);

                // If we are expanding diff hunks and the current block is a removal hunk, render
                // the gutter element with the line decoration.
                if self.diff_hunks_are_expanded()
                    && (is_removal
                        || matches!(diff_hunk, Some(DiffHunkDisplay::Replacement { .. })))
                {
                    // Track the index for this removal line
                    if removed_hunk_line_number == Some(line_count) {
                        removed_hunk_line_index += 1;
                    } else {
                        removed_hunk_line_index = 0;
                        removed_hunk_line_number = Some(line_count);
                    }

                    let height = block.viewport_item().content_size.y();

                    let line = EditorLineLocation::Removed {
                        line_number: line_count,
                        line_range: diff_range.unwrap_or(line_count..line_count),
                        index: removed_hunk_line_index,
                    };
                    let range_hovered = hovered_range
                        .as_ref()
                        .map(|hovered_line| hovered_line.line_range() == line.line_range())
                        .unwrap_or(false);

                    let element = self.render_gutter_element(
                        None,
                        line_number_config,
                        height,
                        block.overlay_decoration(),
                        true, // is diff line
                    );

                    elements.push(GutterElement {
                        element,
                        height,
                        offset,
                        hovered: range_hovered,
                        line,
                        element_type: GutterElementType::DiffHunk {
                            hunk: diff_hunk,
                            change_type: ChangeType::Remove,
                        },
                        overlay: block.overlay_decoration(),
                    });
                }

                continue;
            }

            if block.is_hidden_section() {
                let line_range = self
                    .model()
                    .as_ref(app)
                    .line_range(&**block)
                    .unwrap_or_default();

                let range_hovered = hovered_range
                    .as_ref()
                    .map(|line| line.line_range())
                    .is_some_and(|hovered_line_range| *hovered_line_range == line_range);
                let range_length = line_range.end - line_range.start;
                let height = block.viewport_item().content_size.y();

                let block_location = if block_idx == 0 {
                    BlockLocation::Start
                } else if block_idx >= last_block_idx {
                    BlockLocation::End
                } else {
                    BlockLocation::Middle
                };

                let expand_button_types =
                    gutter_expansion_button_types(&block_location, range_length.as_usize());
                let mut new_gutter_elements = if expand_button_types.len() == 1 {
                    vec![self.construct_expand_hidden_section_gutter_element(
                        height,
                        expand_button_types[0],
                        range_hovered,
                        appearance,
                        line_number_config,
                        line_range,
                        offset,
                    )]
                } else {
                    let half_len = range_length.as_usize() / 2;
                    let midpoint = line_range.start + half_len;

                    let (first_half_range, second_half_range) =
                        (line_range.start..midpoint, midpoint..line_range.end);

                    let first_range_hovered = hovered_range
                        .as_ref()
                        .map(|line| line.line_range().contains(&line_count))
                        .unwrap_or(false);

                    let second_range_hovered = hovered_range
                        .as_ref()
                        .map(|line| line.line_range().contains(&midpoint))
                        .unwrap_or(false);

                    // The editor will render a double-height line,
                    // so the buttons should split the available vertical space.
                    let height = height / 2.0;
                    vec![
                        self.construct_expand_hidden_section_gutter_element(
                            height,
                            expand_button_types[0],
                            first_range_hovered,
                            appearance,
                            line_number_config,
                            first_half_range,
                            offset,
                        ),
                        self.construct_expand_hidden_section_gutter_element(
                            height,
                            expand_button_types[1],
                            second_range_hovered,
                            appearance,
                            line_number_config,
                            second_half_range,
                            offset + Pixels::new(height),
                        ),
                    ]
                };

                elements.append(&mut new_gutter_elements);

                continue;
            }
            let diff_range = self.diff_status.added_diff_range(line_count);

            // If the corresponding line in the editor element has a line decoration, we should apply the decoration
            // in the wrapper as well. This does assume the line could only have a single decoration. I think it's fine
            // given this is a wrapper specific to our code pane implementation.
            let overlay = line_decorations
                .iter()
                .filter(|decoration| decoration.start <= line_count && decoration.end > line_count)
                .map(|decoration| decoration.overlay)
                .next();

            let Some(height) = model.first_line_height(&**block) else {
                continue;
            };

            // Check if this line is part of any diff hunk when diff hunks are expanded and hovered
            let is_diff_line = self.diff_hunks_are_expanded() && diff_hunk.is_some();

            let line = EditorLineLocation::Current {
                line_number: line_count,
                line_range: diff_range.unwrap_or(line_count..line_count + 1),
            };

            let range_hovered = hovered_range
                .as_ref()
                .is_some_and(|hovered_line| hovered_line.line_range() == line.line_range());

            let element = self.render_gutter_element(
                line_number_config.show_numbers.then_some(current_line),
                line_number_config,
                height,
                overlay,
                is_diff_line,
            );

            let diff_hunk = if is_removal && self.diff_hunks_are_expanded() {
                None
            } else {
                diff_hunk
            };
            elements.push(GutterElement {
                element,
                height,
                offset,
                hovered: range_hovered,
                // We can skip rendering this removal gutter element if its hunk is expanded since
                // the gutter is rendered on the temporary block.
                line,
                element_type: GutterElementType::DiffHunk {
                    hunk: diff_hunk,
                    change_type: ChangeType::Add,
                },
                overlay: None,
            });
        }
        elements
    }

    #[allow(clippy::too_many_arguments)]
    fn construct_expand_hidden_section_gutter_element(
        &self,
        height: f32,
        expansion_type: ExpansionType,
        range_hovered: bool,
        appearance: &Appearance,
        line_number_config: &LineNumberConfig,
        line_range: Range<LineCount>,
        offset: Pixels,
    ) -> GutterElement {
        let theme = appearance.theme();
        // Shift the button background when its row (or half-row, for split
        // up/down buttons) is hovered, so the expand affordance reads as
        // interactive.
        let gutter_background_color = if range_hovered {
            internal_colors::fg_overlay_3(theme)
        } else {
            internal_colors::fg_overlay_1(theme)
        };

        let icon_color = if range_hovered {
            line_number_config.highlight_text_color
        } else {
            line_number_config.text_color
        };
        let icon = ConstrainedBox::new(
            warpui::elements::Icon::new(expansion_type.icon().into(), icon_color).finish(),
        )
        .with_width(16.)
        .with_height(16.)
        .finish();

        let element = Container::new(
            ConstrainedBox::new(Align::new(icon).finish())
                .with_height(height)
                .with_width(GUTTER_WIDTH)
                .finish(),
        )
        .with_background_color(gutter_background_color.into())
        .finish();

        GutterElement {
            element,
            offset,
            height,
            hovered: range_hovered,
            line: EditorLineLocation::Collapsed { line_range },
            element_type: GutterElementType::HiddenSection { expansion_type },
            overlay: None,
        }
    }

    fn render_gutter_element(
        &self,
        current_line: Option<usize>,
        line_number_config: &LineNumberConfig,
        gutter_element_height: f32,
        overlay: Option<Fill>,
        highlight_text: bool,
    ) -> Box<dyn Element> {
        let base_content = match current_line {
            Some(line) => {
                // Highlight the line number if it's a diff line or has an overlay
                let text_color = if highlight_text || overlay.is_some() {
                    line_number_config.highlight_text_color
                } else {
                    line_number_config.text_color
                };
                Align::new(
                    Text::new_inline(
                        line.to_string(),
                        line_number_config.font_family,
                        line_number_config.font_size,
                    )
                    .with_selectable(true)
                    .with_color(text_color)
                    .finish(),
                )
                .finish()
            }
            None => {
                // If no current line, render empty element
                Empty::new().finish()
            }
        };

        ConstrainedBox::new(base_content)
            .with_height(gutter_element_height)
            .with_width(GUTTER_WIDTH)
            .finish()
    }

    /// Returns whether a diff hunk range contains the given mouse position on the screen.
    fn gutter_element_range_containing_position(&self, position: Vector2F) -> Option<GutterRange> {
        let wrapper_origin = self.element_origin?.xy();

        if let Some(gutter_elements) = &self.gutter_elements {
            for gutter_element in gutter_elements {
                let gutter_range = gutter_element.contains_position(position, wrapper_origin);
                if let Some(gutter_range) = gutter_range {
                    return Some(gutter_range);
                }
            }
        }

        None
    }

    fn model(&self) -> &ModelHandle<RenderState> {
        &self.editor.model
    }

    fn size_buffer(&self) -> Vector2F {
        GUTTER_WIDTH.along(Axis::Horizontal)
    }
}

impl<V: EditorView> Element for EditorWrapper<V> {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        ctx: &mut LayoutContext,
        app: &AppContext,
    ) -> Vector2F {
        let size_buffer = self.size_buffer();
        let content_constraint = SizeConstraint::new(
            (constraint.min - size_buffer).max(Vector2F::zero()),
            (constraint.max - size_buffer).max(Vector2F::zero()),
        );

        // Layout the editor element first so we can read the laid out visible blocks.
        let editor_size = self.editor.layout(content_constraint, ctx, app);

        let size = match self.vertical_expansion_behavior {
            VerticalExpansionBehavior::GrowToMaxHeight
            | VerticalExpansionBehavior::InfiniteHeight => {
                Vector2F::new(constraint.max.x(), constraint.max.y().min(editor_size.y()))
            }
            VerticalExpansionBehavior::FillMaxHeight => constraint.max,
        };

        let mut gutter_elements = self.gutter_elements(app);
        for gutter_element in &mut gutter_elements {
            gutter_element.element.layout(constraint, ctx, app);
        }

        self.gutter_elements = Some(gutter_elements);
        self.element_size = Some(size);
        size
    }

    fn after_layout(&mut self, ctx: &mut AfterLayoutContext, app: &AppContext) {
        self.editor.after_layout(ctx, app);
    }

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, app: &AppContext) {
        // Save the element origin for hit testing.
        let element_origin = Point::from_vec2f(origin, ctx.scene.z_index());
        self.element_origin = Some(element_origin);

        let size_buffer = self.size_buffer();
        let wrapper_size = self.size().unwrap_or_default();

        // Pre-pass: Draw full-width overlay rects for diff highlighting.
        // Drawing before the inner editor and gutter elements so they appear behind text.
        // Clip to the wrapper bounds so overlays don't bleed outside the element
        // (important for Lens mode where LineDecoration ranges may exceed the visible range).
        let overlay_clip = RectF::new(origin, wrapper_size);
        ctx.scene
            .start_layer(ClipBounds::BoundedByActiveLayerAnd(overlay_clip));

        // Added/replaced lines: one rect per LineDecoration range.
        {
            let model = self.model().as_ref(app);
            let content = model.content();
            let y_adjustment = model.viewport().scroll_top();
            for decoration in model.decorations().line_decoration_ranges() {
                let start_y = content.y_offset_at_line(decoration.start);
                let end_y = content.y_offset_at_line(decoration.end);
                ctx.scene
                    .draw_rect_without_hit_recording(RectF::new(
                        origin + vec2f(0., (start_y - y_adjustment).as_f32()),
                        vec2f(wrapper_size.x(), (end_y - start_y).as_f32()),
                    ))
                    .with_background(decoration.overlay);
            }
        }

        self.paint_removed_line_overlays(origin, wrapper_size, ctx);

        ctx.scene.stop_layer();

        self.editor.paint(origin + size_buffer, ctx, app);

        let diff_hunks_are_expanded = self.diff_hunks_are_expanded();
        let gutter_width = self.size_buffer().x();
        let gutter_bounds = RectF::new(origin, vec2f(gutter_width, wrapper_size.y()));

        if let Some(gutter_elements) = &mut self.gutter_elements {
            // Start clipping layer for gutter elements to prevent overflow
            // at the bottom of the editor.
            // Use BoundedByActiveLayerAnd to intersect with parent scrollable's clipping.
            ctx.scene
                .start_layer(ClipBounds::BoundedByActiveLayerAnd(gutter_bounds));

            // Compute grouped diff indicator sliver rects.
            // Adjacent indicators in the same diff range are grouped into one tall rect.
            struct SliverGroup<'a> {
                start_y: f32,
                end_y: f32,
                range: &'a Range<LineCount>,
                color: ColorU,
                width: f32,
            }

            let mut sliver_rects: Vec<(RectF, ColorU)> = Vec::new();
            let mut group: Option<SliverGroup<'_>> = None;

            for element in gutter_elements.iter() {
                let gutter_y = element.offset.as_f32();

                // For rendering, only expand the gutter element size if it is
                // hovered or it is the active diff hunk.
                let is_hovered = element.hovered
                    || self
                        .focused_diff_line_range
                        .as_ref()
                        .is_some_and(|r| r == element.line.line_range());

                if let Some(size) = element.diff_hunk_size(is_hovered)
                    && let Some(color) = element.diff_indicator_color(diff_hunks_are_expanded)
                {
                    let current_range = element.line.line_range();
                    let same_group = group.as_ref().is_some_and(|g| {
                        g.range == current_range
                            && g.color == color
                            && (g.width - size.x()).abs() < f32::EPSILON
                    });
                    if same_group {
                        group.as_mut().unwrap().end_y = gutter_y + element.height;
                    } else {
                        // Flush previous group.
                        if let Some(g) = group.take() {
                            sliver_rects.push((
                                RectF::new(
                                    origin + vec2f(0., g.start_y),
                                    vec2f(g.width, g.end_y - g.start_y),
                                ),
                                g.color,
                            ));
                        }
                        // Start new group.
                        group = Some(SliverGroup {
                            start_y: gutter_y,
                            end_y: gutter_y + element.height,
                            range: current_range,
                            color,
                            width: size.x(),
                        });
                    }
                }
            }
            // Flush last group.
            if let Some(g) = group.take() {
                sliver_rects.push((
                    RectF::new(
                        origin + vec2f(0., g.start_y),
                        vec2f(g.width, g.end_y - g.start_y),
                    ),
                    g.color,
                ));
            }

            // Draw all buffered sliver rects (behind gutter elements).
            for (rect, color) in &sliver_rects {
                ctx.scene
                    .draw_rect_with_hit_recording(*rect)
                    .with_background(*color);
            }

            // Paint gutter elements.
            for gutter_element in gutter_elements.iter_mut() {
                let gutter_origin = origin + vec2f(0., gutter_element.offset.as_f32());
                gutter_element.element.paint(gutter_origin, ctx, app);
            }

            ctx.scene.stop_layer();
        }

        self.child_max_z_index = Some(ctx.scene.max_active_z_index());
    }

    fn size(&self) -> Option<Vector2F> {
        self.element_size
    }

    fn origin(&self) -> Option<Point> {
        self.element_origin
    }

    fn dispatch_event(
        &mut self,
        event: &DispatchedEvent,
        ctx: &mut EventContext,
        app: &AppContext,
    ) -> bool {
        let Some(z_index) = self
            .child_max_z_index
            .or_else(|| self.element_origin.map(|origin| origin.z_index()))
        else {
            return false;
        };

        // Then, dispatch events to gutter elements that are in the hovered range
        // This is important for Hoverable elements like the plus button
        let mut gutter_handled = false;
        if let Some(gutter_elements) = &mut self.gutter_elements {
            let hovered_range = self.state_handle.hovered_diff_hunk.lock().clone();

            for gutter_element in gutter_elements {
                let should_dispatch = hovered_range
                    .as_ref()
                    .is_some_and(|hovered_line| hovered_line.is_same_line(&gutter_element.line));

                if should_dispatch && gutter_element.element.dispatch_event(event, ctx, app) {
                    gutter_handled = true;
                }
            }
        }

        // Handle mouse events for hover state and clicks
        match event.at_z_index(z_index, ctx) {
            Some(Event::MouseMoved { position, .. }) => {
                let broad_hovered_range = self.gutter_element_range_containing_position(*position);

                let hovered_range = match broad_hovered_range {
                    Some(GutterRange::HiddenSection { .. }) => {
                        self.gutter_element_range_containing_position(*position)
                    }
                    other => other,
                };

                let hovered_line = hovered_range.map(|gutter_range| gutter_range.line().clone());
                let mut hovered_diff_hunk = self.state_handle.hovered_diff_hunk.lock();
                if hovered_diff_hunk.as_ref() != hovered_line.as_ref() {
                    *hovered_diff_hunk = hovered_line;
                    ctx.notify();
                }
            }
            Some(Event::LeftMouseDown { position, .. }) => {
                if !gutter_handled {
                    let in_bound = self
                        .gutter_element_range_containing_position(*position)
                        .is_some();
                    self.state_handle
                        .in_click
                        .store(in_bound, Ordering::Relaxed);
                }
            }
            Some(Event::LeftMouseUp { position, .. }) => {
                if !gutter_handled {
                    let was_clicking = self.state_handle.in_click.swap(false, Ordering::Relaxed);

                    if was_clicking
                        && let Some(gutter_range) =
                            self.gutter_element_range_containing_position(*position)
                    {
                        (self.click_handler)(gutter_range, ctx);
                    }
                }
            }
            _ => (),
        };

        let dispatch_to_editor = self.editor.dispatch_event(event, ctx, app);

        // Always dispatch to both the gutter elements and the editor.
        dispatch_to_editor || gutter_handled
    }
}

impl<V: EditorView> NewScrollableElement for EditorWrapper<V> {
    fn scroll_data(&self, axis: Axis, app: &AppContext) -> Option<ScrollData> {
        self.editor.scroll_data(axis, app)
    }

    fn scroll(&mut self, delta: Pixels, axis: Axis, ctx: &mut EventContext) {
        self.editor.scroll(delta, axis, ctx)
    }

    fn axis_should_handle_scroll_wheel(&self, _axis: Axis) -> bool {
        self.should_handle_scroll_wheel
    }

    fn axis(&self) -> ScrollableAxis {
        ScrollableAxis::Both
    }
}

#[cfg(test)]
#[path = "element_tests.rs"]
mod tests;
