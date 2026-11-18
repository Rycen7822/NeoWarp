use std::collections::HashMap;
use std::path::{Path, PathBuf};

use pathfinder_color::ColorU;
use pathfinder_geometry::rect::RectF;
use pathfinder_geometry::vector::vec2f;
use warp_core::channel::{Channel, ChannelState};
use warp_core::ui::appearance::Appearance;
use warp_core::ui::icons::ICON_DIMENSIONS;
use warp_editor::render::element::VerticalExpansionBehavior;
use warp_util::path::LineAndColumnArg;
#[cfg(feature = "local_fs")]
use warpui::clipboard::ClipboardContent;
use warpui::elements::{
    AcceptedByDropTarget, Align, Border, ChildAnchor, ChildView, Clipped, ConstrainedBox,
    Container, CornerRadius, CrossAxisAlignment, Draggable, DraggableState, DropTarget, Empty,
    Expanded, Flex, Hoverable, MainAxisAlignment, MainAxisSize, MouseStateHandle,
    OffsetPositioning, Padding, ParentAnchor, ParentElement, ParentOffsetBounds, Radius, Rect,
    SavePosition, Shrinkable, Stack, Text,
};
use warpui::fonts::{Properties, Weight};
use warpui::keymap::EditableBinding;
use warpui::modals::{AlertDialogWithCallbacks, ModalButton};
use warpui::text_layout::ClipConfig;
use warpui::ui_components::button::Button;
use warpui::ui_components::components::UiComponent;
use warpui::{
    AppContext, Element, Entity, ModelHandle, SingletonEntity, TypedActionView, View, ViewContext,
    ViewHandle, WindowId, id,
};

use super::buffer_location::LocalOrRemotePath;
use super::editor::view::{CodeEditorEvent, CodeEditorView};
use super::editor_management::CodeSource;
#[cfg(feature = "local_fs")]
use super::image_preview::{ImagePreview, can_preview};
use super::local_code_editor::{LocalCodeEditorEvent, LocalCodeEditorView};
use crate::code::editor::view::CodeEditorRenderOptions;
use crate::code::global_buffer_model::GlobalBufferModel;
use crate::code::{
    DEFAULT_PREVIEW_ZOOM, EditorTabBarDropTargetData, ImmediateSaveError, SaveOutcome, SaveStatus,
};
use crate::input::Vector2F;
use crate::localization::tr;
use crate::menu::{MenuItem, MenuItemFields};
use crate::pane_group::focus_state::PaneFocusHandle;
use crate::pane_group::pane::view::header::components::{
    CenteredHeaderEdgeWidth, render_pane_header_buttons, render_pane_header_title_text,
    render_three_column_header,
};
use crate::pane_group::pane::view::header::render_pane_header_draggable;
use crate::pane_group::pane::{ActionOrigin, PaneHeaderAction, view};
use crate::pane_group::{
    BackingView, CodePane, PaneConfiguration, PaneConfigurationEvent, PaneDragDropLocation,
    PaneEvent, TabBarAxis,
};
use crate::search::ItemHighlightState;
use crate::search::files::icon::icon_from_file_path;
use crate::settings::CodeSettings;
use crate::tab::TAB_BAR_BORDER_HEIGHT;
use crate::ui_components::blended_colors;
use crate::ui_components::buttons::icon_button;
use crate::ui_components::icons::Icon;
use crate::util::path::{display_name_with_host, display_path_with_host};
use crate::view_components::DismissibleToast;
use crate::workspace::{ActiveSession, TabBarDropTargetData, ToastStack, WorkspaceRegistry};

type SaveCallback =
    Box<dyn FnOnce(SaveOutcome, &mut CodeView, &mut ViewContext<CodeView>) + Send + Sync + 'static>;

const CLOSE_BUTTON_WIDTH: f32 = 24.;
const LANGUAGE_ICON_WIDTH: f32 = 16.;
const TAB_INTERNAL_MARGIN: f32 = 4.;
const TAB_HORIZONTAL_MARGIN: f32 = 8.;
const TAB_PADDING: f32 = 2.;
const MIN_PREVIEW_ZOOM: i16 = 50;
const MAX_PREVIEW_ZOOM: i16 = 300;
const PREVIEW_ZOOM_STEP: i16 = 10;

// Keybinding constants shared by code views
pub const SAVE_FILE_BINDING_NAME: &str = "code_view:save";
pub const SAVE_FILE_BINDING_DESCRIPTION: &str = "Save file";

pub fn init(app: &mut AppContext) {
    super::editor::view::init(app);

    let text_entry = id!("CodeEditorView") & !id!("IMEOpen");
    app.register_editable_bindings([
        EditableBinding::new(
            SAVE_FILE_BINDING_NAME,
            SAVE_FILE_BINDING_DESCRIPTION,
            CodeViewAction::SaveFile,
        )
        .with_context_predicate(text_entry.clone())
        .with_key_binding("cmdorctrl-s"),
        EditableBinding::new(
            "code_view:save_as",
            "Save file as",
            CodeViewAction::SaveFileAs,
        )
        .with_context_predicate(text_entry.clone())
        .with_key_binding("cmdorctrl-shift-S"),
        EditableBinding::new(
            "code_view:close_all_tabs",
            "Close all tabs",
            CodeViewAction::CloseAll,
        )
        .with_context_predicate(id!("CodeEditorView"))
        .with_key_binding("cmdorctrl-r w"),
        EditableBinding::new(
            "code_view:close_saved_tabs",
            "Close saved tabs",
            CodeViewAction::CloseSaved,
        )
        .with_context_predicate(id!("CodeEditorView"))
        .with_key_binding("cmdorctrl-r u"),
    ]);
}

const PADDING: f32 = 4.;

pub use crate::util::openable_file_type::is_binary_file;
/// Determines the `SavePosition` ID for a draggable tab based on its index.
pub fn tab_position_id(index: usize) -> String {
    format!("file_tab_position_{index}")
}

#[derive(Debug, Clone)]
enum TabBarDragPosition {
    BeforeTab { index: usize },
    AfterTab { index: usize },
}

#[derive(Debug, Clone)]
pub enum CodeViewAction {
    SaveFile,
    SaveFileAs,
    SetCurrentTabIndex {
        index: usize,
    },
    RemoveTabAtIndex {
        index: usize,
    },
    CloseAll,
    CloseSaved,
    ToggleMaximized,
    TogglePreviewWrap,
    ChangePreviewZoom(i16),
    #[cfg(feature = "local_fs")]
    CopyFilePath,
    /// Open the active code tab's file in the platform's file manager
    /// (Finder on macOS, Explorer on Windows). No-op when the active tab has
    /// no resolvable local path.
    #[cfg(feature = "local_fs")]
    RevealInFinder,
    DragOverIndex {
        target: usize,
        drag_position: RectF,
    },
    DropAtIndex {
        origin: usize,
        target: usize,
        drag_position: RectF,
    },
    ClearEditorTabGroupDragPositions,
    ClearWorkspaceTabGroupDragPositions,
}

#[derive(Debug, Clone)]
pub enum CodeViewEvent {
    Pane(PaneEvent),
    FileOpened {
        location: LocalOrRemotePath,
        tab_index: usize,
    },
}

#[derive(Default, Clone)]
struct TabDataMouseStateHandles {
    tab_handle: MouseStateHandle,
    close_handle: MouseStateHandle,
    tab_draggable_state: DraggableState,
}

#[derive(Default)]
struct PreviewControls {
    zoom_out: MouseStateHandle,
    zoom_in: MouseStateHandle,
    wrap: MouseStateHandle,
}

#[derive(Clone)]
pub struct TabData {
    location: Option<LocalOrRemotePath>,
    content: TabContent,
    mouse_state_handles: TabDataMouseStateHandles,
}

#[derive(Clone)]
enum TabContent {
    Editor(ViewHandle<LocalCodeEditorView>),
    #[cfg(feature = "local_fs")]
    Image {
        zoom_percent: i16,
    },
}

#[derive(Debug, Clone)]
pub enum PendingSaveIntent {
    Save,
    Discard,
    Cancel,
}

impl TabData {
    fn editor(&self) -> Option<&ViewHandle<LocalCodeEditorView>> {
        match &self.content {
            TabContent::Editor(editor) => Some(editor),
            #[cfg(feature = "local_fs")]
            TabContent::Image { .. } => None,
        }
    }

    fn zoom_percent(&self, app: &AppContext) -> i16 {
        match &self.content {
            TabContent::Editor(local) => local.as_ref(app).editor().as_ref(app).zoom_percent(),
            #[cfg(feature = "local_fs")]
            TabContent::Image { zoom_percent } => *zoom_percent,
        }
    }

    /// Returns the file location (local or remote), if any.
    pub fn location(&self) -> Option<&LocalOrRemotePath> {
        self.location.as_ref()
    }

    /// Returns the local filesystem path, if this tab is backed by a local file.
    /// Returns `None` for remote files and untitled tabs.
    pub fn local_path(&self) -> Option<PathBuf> {
        self.location
            .as_ref()
            .and_then(|loc| loc.to_local_path().map(Path::to_path_buf))
    }
}

pub struct CodeView {
    tab_group: Vec<TabData>,
    active_tab_index: usize,
    preview_controls: PreviewControls,
    #[cfg(feature = "local_fs")]
    active_image: Option<ViewHandle<ImagePreview>>,
    pane_configuration: ModelHandle<PaneConfiguration>,
    focus_handle: Option<PaneFocusHandle>,
    source: CodeSource,
    window_id: WindowId,
    drag_position: Option<TabBarDragPosition>,
}

impl CodeView {
    fn new_internal(source: CodeSource, ctx: &mut ViewContext<Self>) -> Self {
        let pane_configuration = ctx.add_model(|_ctx| PaneConfiguration::new(""));
        let window_id = ctx.window_id();

        Self {
            tab_group: Default::default(),
            active_tab_index: 0,
            preview_controls: Default::default(),
            #[cfg(feature = "local_fs")]
            active_image: None,
            pane_configuration,
            focus_handle: None,
            source,
            window_id,
            drag_position: None,
        }
    }

    pub fn new(
        source: CodeSource,
        line_col: Option<LineAndColumnArg>,
        ctx: &mut ViewContext<Self>,
    ) -> Self {
        let location = source.location();
        let mut view = Self::new_internal(source, ctx);
        view.open_or_focus_existing(location, line_col, ctx);
        view
    }

    /// Restore a code view from a persisted multi-tab snapshot.
    pub fn restore(
        tabs: &[crate::app_state::CodePaneTabSnapshot],
        active_tab_index: usize,
        source: CodeSource,
        ctx: &mut ViewContext<Self>,
    ) -> Self {
        let mut view = Self::new_internal(source, ctx);
        for tab_snapshot in tabs {
            let location = tab_snapshot.path.clone().map(LocalOrRemotePath::Local);
            let tab_data = view.build_tab_data(location, ctx);
            view.tab_group.push(tab_data);
        }
        let clamped_index = if view.tab_group.is_empty() {
            0
        } else {
            active_tab_index.min(view.tab_group.len() - 1)
        };
        view.active_tab_index = clamped_index;
        #[cfg(feature = "local_fs")]
        view.refresh_active_image(ctx);
        view.set_title(false, ctx);
        view
    }

    /// Construct an editor backed by the global shared buffer for the given location.
    ///
    /// Uses the same basic editor for local and remote file buffers.
    fn construct_editor_for_location(
        &mut self,
        location: LocalOrRemotePath,
        ctx: &mut ViewContext<Self>,
    ) -> ViewHandle<LocalCodeEditorView> {
        ctx.add_typed_action_view(|ctx| {
            LocalCodeEditorView::new_with_global_buffer(
                location,
                |buffer_state, ctx| {
                    ctx.add_typed_action_view(|ctx| {
                        CodeEditorView::new(
                            None,
                            Some(buffer_state.buffer),
                            CodeEditorRenderOptions::new(VerticalExpansionBehavior::FillMaxHeight),
                            ctx,
                        )
                        .with_horizontal_scrollbar_appearance(
                            warpui::elements::new_scrollable::ScrollableAppearance::new(
                                warpui::elements::ScrollbarWidth::Auto,
                                true,
                            ),
                        )
                    })
                },
                ctx,
            )
        })
    }

    /// Construct an editor for a new (unsaved) file with no file backing.
    fn construct_new_file_editor(
        &mut self,
        ctx: &mut ViewContext<Self>,
    ) -> ViewHandle<LocalCodeEditorView> {
        let editor = ctx.add_typed_action_view(|ctx| {
            CodeEditorView::new(
                None,
                None,
                CodeEditorRenderOptions::new(VerticalExpansionBehavior::FillMaxHeight),
                ctx,
            )
            .with_horizontal_scrollbar_appearance(
                warpui::elements::new_scrollable::ScrollableAppearance::new(
                    warpui::elements::ScrollbarWidth::Auto,
                    true,
                ),
            )
        });

        ctx.add_typed_action_view(|ctx| LocalCodeEditorView::new(editor, ctx))
    }

    fn build_tab_data(
        &mut self,
        location: Option<LocalOrRemotePath>,
        ctx: &mut ViewContext<Self>,
    ) -> TabData {
        #[cfg(feature = "local_fs")]
        if location
            .as_ref()
            .and_then(LocalOrRemotePath::to_local_path)
            .is_some_and(can_preview)
        {
            return TabData {
                location,
                content: TabContent::Image {
                    zoom_percent: DEFAULT_PREVIEW_ZOOM,
                },
                mouse_state_handles: Default::default(),
            };
        }
        let (code_editor, tab_location) = match location {
            Some(loc) => {
                let editor = self.construct_editor_for_location(loc.clone(), ctx);
                (editor, Some(loc))
            }
            None => (self.construct_new_file_editor(ctx), None),
        };

        let editor = code_editor.as_ref(ctx).editor().clone();

        ctx.subscribe_to_view(&editor, |me, _, event, ctx| match event {
            CodeEditorEvent::Focused => {
                ctx.emit(CodeViewEvent::Pane(PaneEvent::FocusSelf));
            }
            CodeEditorEvent::ContentChanged { .. } => {
                me.set_title_after_content_update(ctx);
            }
            _ => {}
        });

        // For new files (CodeSource::New), mark the editor as a new file and set default directory
        if tab_location.is_none() && matches!(self.source, CodeSource::New { .. }) {
            let default_directory = self.source.default_directory().cloned();
            code_editor.update(ctx, |local_editor, _ctx| {
                local_editor.set_new_file(true);
                local_editor.set_default_directory(default_directory);
            });
        }

        ctx.subscribe_to_view(&code_editor, |me, _, event, ctx| match event {
            LocalCodeEditorEvent::FileLoaded => {
                me.pane_configuration.update(ctx, |pane_config, ctx| {
                    pane_config.refresh_pane_header_overflow_menu_items(ctx);
                });
                ctx.emit(CodeViewEvent::Pane(PaneEvent::AppStateChanged));
            }
            LocalCodeEditorEvent::FailedToLoad { error: err } => {
                // A new file may not exist yet.
                if matches!(me.source, CodeSource::New { .. }) {
                    return;
                }
                log::warn!("Failed to load file. {err:?}");
                CodeView::display_load_failure(ctx.window_id(), ctx);
            }
            LocalCodeEditorEvent::FileSaved { auto_saved } => {
                me.sync_active_tab_location(ctx);
                me.set_title_after_content_update(ctx);
                // Only surface the success toast for manual (cmd-s) saves;
                // auto-saves persist silently.
                if !*auto_saved {
                    CodeView::display_save_success(ctx.window_id(), ctx);
                }
                ctx.notify();
            }
            LocalCodeEditorEvent::FailedToSave { error: err } => {
                log::warn!("Failed to load file. {err:?}");
                CodeView::display_save_failure(ctx.window_id(), ctx);
            }
            LocalCodeEditorEvent::DiffStatusUpdated => (),
            LocalCodeEditorEvent::ViewportUpdated => (),
            LocalCodeEditorEvent::LayoutInvalidated => (),
            LocalCodeEditorEvent::DiscardUnsavedChanges { path } => {
                #[cfg(feature = "local_fs")]
                GlobalBufferModel::handle(ctx).update(ctx, |global_buffer, ctx| {
                    global_buffer.discard_unsaved_changes(path, ctx);
                });
            }
            LocalCodeEditorEvent::DelayedRenderingFlushed => (),
        });

        TabData {
            location: tab_location,
            content: TabContent::Editor(code_editor),
            mouse_state_handles: Default::default(),
        }
    }

    fn clear_drag_position(&mut self) {
        self.drag_position = None;
    }

    pub fn tab_at(&self, index: usize) -> Option<&TabData> {
        self.tab_group.get(index)
    }

    pub fn active_tab_index(&self) -> usize {
        self.active_tab_index
    }

    pub fn source(&self) -> &CodeSource {
        &self.source
    }

    pub fn local_path(&self, ctx: &AppContext) -> Option<PathBuf> {
        let tab = self.tab_at(self.active_tab_index)?;
        let Some(editor) = tab.editor() else {
            return tab.local_path();
        };
        let file_id = editor.as_ref(ctx).file_id()?;
        GlobalBufferModel::as_ref(ctx)
            .file_path(file_id)
            .map(Path::to_path_buf)
    }

    pub fn pane_configuration(&self) -> ModelHandle<PaneConfiguration> {
        self.pane_configuration.clone()
    }

    pub fn focus(&self, ctx: &mut ViewContext<Self>) {
        if let Some(tab) = self.tab_at(self.active_tab_index) {
            if let Some(editor) = tab.editor() {
                ctx.focus(editor);
            } else {
                ctx.focus_self();
            }
        }
    }

    pub fn open_or_focus_existing(
        &mut self,
        location: Option<LocalOrRemotePath>,
        line_col: Option<LineAndColumnArg>,
        ctx: &mut ViewContext<Self>,
    ) {
        // If the tab already exists, focus it (and optionally jump) without re-opening from disk.
        if let Some(existing_index) = self.focus_existing_tab_if_present(location.as_ref(), ctx) {
            if let Some(line_col) = line_col {
                self.jump_to_line_col_in_tab(existing_index, line_col, ctx);
            }
            return;
        }

        self.open_new_tab(location, line_col, ctx);
    }

    fn focus_existing_tab_if_present(
        &mut self,
        location: Option<&LocalOrRemotePath>,
        ctx: &mut ViewContext<Self>,
    ) -> Option<usize> {
        let location = location?;
        let existing_index = self
            .tab_group
            .iter()
            .position(|tab| tab.location.as_ref() == Some(location))?;
        self.set_active_tab_index(existing_index, ctx);
        Some(existing_index)
    }

    fn jump_to_line_col_in_tab(
        &self,
        tab_index: usize,
        line_col: LineAndColumnArg,
        ctx: &mut ViewContext<Self>,
    ) {
        let Some(editor) = self.tab_at(tab_index).and_then(TabData::editor) else {
            return;
        };

        editor.update(ctx, |editor, ctx| {
            editor.set_pending_scroll(line_col, ctx);
        });
    }

    fn open_new_tab(
        &mut self,
        location: Option<LocalOrRemotePath>,
        line_col: Option<LineAndColumnArg>,
        ctx: &mut ViewContext<Self>,
    ) {
        let new_tab = self.build_tab_data(location.clone(), ctx);
        self.tab_group.push(new_tab);
        let active_tab_index = self.tab_group.len() - 1;

        if let Some(loc) = location {
            ctx.emit(CodeViewEvent::FileOpened {
                location: loc,
                tab_index: active_tab_index,
            });

            // By default scroll to the first line.
            let scroll_position = line_col.unwrap_or(LineAndColumnArg {
                line_num: 1,
                column_num: None,
            });

            // For GlobalBuffer path, set_pending_scroll handles the case where the file
            // hasn't finished loading yet by deferring the scroll until FileLoaded.
            self.jump_to_line_col_in_tab(active_tab_index, scroll_position, ctx);
        }

        self.set_active_tab_index(active_tab_index, ctx);
    }

    /// Set the title of the pane, which is the file path.
    fn set_title(&self, _unsaved_changes: bool, ctx: &mut ViewContext<Self>) {
        let tab = self.tab_at(self.active_tab_index);
        let editor = tab
            .and_then(TabData::editor)
            .map(|editor| editor.as_ref(ctx));
        let file_location = match editor {
            Some(editor) => editor.file_location(),
            None => tab.and_then(TabData::location),
        };
        let is_new = editor.is_some_and(LocalCodeEditorView::is_new_file);

        let title = match file_location {
            Some(location) => display_path_with_host(location, false, ctx),
            None => "Untitled".to_string(),
        };

        self.pane_configuration.update(ctx, |pane_config, ctx| {
            let mut secondary = String::new();
            if self.tab_group.len() > 1 {
                secondary.push_str(&format!(" (+{})", self.tab_group.len() - 1));
            } else if is_new {
                secondary.push_str(" (new)");
            }

            pane_config.set_title(title, ctx);
            pane_config.set_title_secondary(secondary, ctx);
            ctx.emit(PaneConfigurationEvent::TitleUpdated);
            ctx.emit(PaneConfigurationEvent::HeaderContentChanged);
        });
    }

    fn save_local(
        &mut self,
        index: usize,
        callback: Option<SaveCallback>,
        ctx: &mut ViewContext<Self>,
    ) -> SaveStatus {
        // This will only return an error immediately if there is a failure in the sync part of the call.
        // Other errors could be returned asynchronously via the FileModelEvent::FailedToSave event.
        let result = self
            .tab_at(index)
            .and_then(TabData::editor)
            .map(|editor| editor.update(ctx, |editor, ctx| editor.save_local(ctx)))
            .unwrap_or(Err(ImmediateSaveError::NoActiveFileTab));

        // This will only return an error immediately if there is a failure in the sync part of the call.
        // Other errors could be returned asynchronously via the FileModelEvent::FailedToSave event.
        match result {
            Err(ImmediateSaveError::NoFileId) => {
                // If there's no file ID, this is a new file - trigger Save As
                self.save_as(index, callback, ctx)
            }
            Err(ImmediateSaveError::RemoteDisconnected) => {
                log::warn!("Cannot save: remote session disconnected");
                CodeView::display_remote_disconnected_save_failure(ctx.window_id(), ctx);
                if let Some(callback) = callback {
                    callback(SaveOutcome::Failed, self, ctx);
                }
                SaveStatus::Failed(ImmediateSaveError::RemoteDisconnected)
            }
            Err(err) => {
                log::warn!("Failed to save file. {err:?}");
                CodeView::display_save_failure(ctx.window_id(), ctx);
                if let Some(callback) = callback {
                    callback(SaveOutcome::Failed, self, ctx);
                }
                SaveStatus::Failed(err)
            }
            Ok(()) => {
                if let Some(callback) = callback {
                    callback(SaveOutcome::Succeeded, self, ctx);
                }
                SaveStatus::SavedImmediately
            }
        }
    }

    fn save_as(
        &mut self,
        index: usize,
        callback: Option<SaveCallback>,
        ctx: &mut ViewContext<Self>,
    ) -> SaveStatus {
        if let Some(editor) = self.tab_at(index).and_then(TabData::editor) {
            let view_handle = ctx.handle().clone();
            editor.update(ctx, |editor, ctx| match callback {
                Some(cb) => {
                    editor.save_as(
                        Some(Box::new(move |outcome, ctx| {
                            if let Some(view) = view_handle.upgrade(ctx) {
                                view.update(ctx, |me, ctx| {
                                    cb(outcome, me, ctx);
                                });
                            }
                        })),
                        ctx,
                    );
                }
                None => {
                    editor.save_as(None, ctx);
                }
            });
            SaveStatus::AsyncSaveInProgress
        } else {
            SaveStatus::Failed(ImmediateSaveError::NoActiveFileTab)
        }
    }

    fn display_load_failure(window_id: WindowId, ctx: &mut ViewContext<Self>) {
        ToastStack::handle(ctx).update(ctx, |toast_stack, ctx| {
            let toast = DismissibleToast::error(String::from("Failed to load file."))
                .with_object_id("failed_to_load_file".to_string());
            toast_stack.add_ephemeral_toast(toast, window_id, ctx);
        });
    }

    fn display_save_failure(window_id: WindowId, ctx: &mut ViewContext<Self>) {
        ToastStack::handle(ctx).update(ctx, |toast_stack, ctx| {
            let toast = DismissibleToast::error(String::from("Failed to save file."))
                .with_object_id("failed_to_save_file".to_string());
            toast_stack.add_ephemeral_toast(toast, window_id, ctx);
        });
    }

    fn display_remote_disconnected_save_failure(window_id: WindowId, ctx: &mut ViewContext<Self>) {
        ToastStack::handle(ctx).update(ctx, |toast_stack, ctx| {
            let toast =
                DismissibleToast::error(String::from("Cannot save — remote session disconnected."))
                    .with_object_id("failed_to_save_file_remote_disconnected".to_string());
            toast_stack.add_ephemeral_toast(toast, window_id, ctx);
        });
    }

    fn display_save_success(window_id: WindowId, ctx: &mut ViewContext<Self>) {
        ToastStack::handle(ctx).update(ctx, |toast_stack, ctx| {
            let toast = DismissibleToast::success(String::from("File saved."))
                .with_object_id("file_saved".to_string());
            toast_stack.add_ephemeral_toast(toast, window_id, ctx);
        });
    }

    pub fn tab_count(&self) -> usize {
        self.tab_group.len()
    }

    // Check if there are unsaved changes in the buffer.
    // Implemented by comparing the ContentVersion in the FileModel with the ContentVersion in the Buffer.
    pub fn contains_unsaved_changes(&self, ctx: &AppContext) -> bool {
        self.tab_group
            .iter()
            .any(|tab| Self::has_unsaved_changes(tab, ctx))
    }

    // Returns the indices of tabs with unsaved changes as a vector.
    fn unsaved_indices(&self, ctx: &AppContext) -> Vec<usize> {
        self.tab_group
            .iter()
            .enumerate()
            .filter_map(|(index, tab)| Self::has_unsaved_changes(tab, ctx).then_some(index))
            .collect()
    }

    pub fn active_tab_has_unsaved_changes(&self, ctx: &AppContext) -> bool {
        let Some(tab) = self.tab_at(self.active_tab_index) else {
            return false;
        };
        Self::has_unsaved_changes(tab, ctx)
    }

    /// Whether the active tab should render the unsaved-changes indicator.
    /// Auto-save-aware: see [`Self::show_unsaved_indicator`].
    pub fn active_tab_shows_unsaved_indicator(&self, ctx: &AppContext) -> bool {
        self.tab_at(self.active_tab_index)
            .is_some_and(|tab| Self::show_unsaved_indicator(tab, ctx))
    }

    fn has_unsaved_changes(tab: &TabData, ctx: &AppContext) -> bool {
        tab.editor()
            .is_some_and(|editor| editor.as_ref(ctx).has_unsaved_changes(ctx))
    }

    /// Whether to render the transient "unsaved changes" indicator for a tab.
    ///
    /// When auto-save is enabled, edits are persisted automatically (debounced
    /// while typing and on focus loss), so showing the dot would just make it
    /// flicker on and off as the user types. It is only hidden for changes
    /// auto-save can actually persist, though: untitled buffers and
    /// disconnected remotes keep the indicator so unsaveable changes stay
    /// visible.
    fn show_unsaved_indicator(tab: &TabData, app: &AppContext) -> bool {
        let Some(editor) = tab.editor().map(|editor| editor.as_ref(app)) else {
            return false;
        };
        editor.has_unsaved_changes(app)
            && (!*CodeSettings::as_ref(app).auto_save || !editor.can_auto_save(app))
    }

    /// Flush-saves every unsaved tab that has a backing file, marking each save
    /// as an auto-save so it stays silent (no "File saved." toast).
    pub fn auto_save_all_unsaved_tabs(&mut self, ctx: &mut ViewContext<Self>) {
        if !*CodeSettings::as_ref(ctx).auto_save {
            return;
        }
        for index in self.unsaved_indices(ctx) {
            // A tab can only be auto-saved if it has a backing file *and*, for
            // remote files, its host is still connected. A disconnected remote
            // buffer has a `file_id` but `save_local` would fail silently, so
            // skip it instead of attempting a save that cannot succeed.
            if let Some(editor) = self.tab_at(index).and_then(TabData::editor)
                && editor.as_ref(ctx).can_auto_save(ctx)
            {
                editor.update(ctx, |editor, _| editor.mark_next_save_as_auto_save());
                self.save_local(index, None, ctx);
            }
        }
    }

    /// Check whether there are unsaved changes and reset the pane title accordingly.
    fn set_title_after_content_update(&self, ctx: &mut ViewContext<Self>) {
        self.set_title(self.contains_unsaved_changes(ctx), ctx);
    }

    /// Update the TabData location for the active tab to match the LocalCodeEditor metadata.
    /// This is needed after save operations to keep local and remote locations in sync.
    fn sync_active_tab_location(&mut self, ctx: &mut ViewContext<Self>) {
        if let Some(tab) = self.tab_group.get_mut(self.active_tab_index)
            && let Some(editor) = tab.editor()
        {
            tab.location = editor.as_ref(ctx).file_location().cloned();
        }
    }

    pub fn cleanup_all_tabs(&mut self, ctx: &mut ViewContext<Self>) {
        self.tab_group.clear();
        #[cfg(feature = "local_fs")]
        {
            self.active_image = None;
        }
        GlobalBufferModel::handle(ctx).update(ctx, |model, ctx| {
            model.remove_deallocated_buffers(ctx);
        });
    }

    pub fn close_overlays(&mut self, ctx: &mut ViewContext<Self>) {
        for editor in self.tab_group.iter().filter_map(TabData::editor) {
            editor.update(ctx, |editor, ctx| {
                editor.close_find_bar(false, ctx);
            })
        }
    }

    fn set_active_tab_index_after_remove(
        &mut self,
        remove_index: usize,
        ctx: &mut ViewContext<Self>,
    ) {
        if remove_index <= self.active_tab_index {
            self.set_active_tab_index(self.active_tab_index.saturating_sub(1), ctx);
        }
        self.update_tab_bar_state(ctx);
        ctx.notify();
    }

    fn remove_tab_with_confirmation(
        &mut self,
        index: usize,
        is_clearing_group: bool,
        ctx: &mut ViewContext<Self>,
    ) {
        if let Some(tab) = self.tab_at(index) {
            let file_name = tab
                .location
                .as_ref()
                .map(|loc| display_name_with_host(loc, ctx))
                .filter(|n| !n.is_empty());
            // If the tab being removed has unsaved changes, we attempt to show a modal before closing it.
            if Self::has_unsaved_changes(tab, ctx)
                && ChannelState::channel() != Channel::Integration
            {
                // With auto-save enabled, closing a file that has a backing path
                // shouldn't block on the "unsaved changes" dialog just because
                // the debounce hasn't fired yet — the edits will be persisted
                // anyway. Flush them and close (mirroring the dialog's "Save
                // changes" action), marking the saves as auto-saves so they stay
                // silent. Untitled files (no path) still prompt, since auto-save
                // can't persist them without a Save As.
                let tab_has_backing_file = self
                    .tab_at(index)
                    .and_then(TabData::editor)
                    .is_some_and(|editor| editor.as_ref(ctx).file_id().is_some());
                if *CodeSettings::as_ref(ctx).auto_save && tab_has_backing_file {
                    if is_clearing_group {
                        let unsaved_indices = self.unsaved_indices(ctx);
                        for &unsaved_index in &unsaved_indices {
                            if let Some(editor) =
                                self.tab_at(unsaved_index).and_then(TabData::editor)
                                && editor.as_ref(ctx).file_id().is_some()
                            {
                                editor
                                    .update(ctx, |editor, _| editor.mark_next_save_as_auto_save());
                            }
                        }
                        self.clear_tab_group_with_intent(
                            unsaved_indices,
                            0,
                            Some(PendingSaveIntent::Save),
                            ctx,
                        );
                    } else {
                        if let Some(editor) = self.tab_at(index).and_then(TabData::editor) {
                            editor.update(ctx, |editor, _| editor.mark_next_save_as_auto_save());
                        }
                        self.remove_tab_with_intent(index, Some(PendingSaveIntent::Save), ctx);
                    }
                    return;
                }

                let handle_save_intent = |intent: PendingSaveIntent| {
                    let handle = ctx.handle().clone();
                    move |ctx: &mut AppContext| {
                        if let Some(view) = handle.upgrade(ctx) {
                            view.update(ctx, |view, ctx| {
                                if is_clearing_group {
                                    let unsaved_indices = view.unsaved_indices(ctx);
                                    view.clear_tab_group_with_intent(
                                        unsaved_indices,
                                        0,
                                        Some(intent),
                                        ctx,
                                    );
                                } else {
                                    view.remove_tab_with_intent(index, Some(intent), ctx);
                                }
                            });
                        }
                    }
                };
                let dialog = AlertDialogWithCallbacks::for_app(
                    "Save changes?",
                    format!(
                        "Do you want to save the changes you made to {}?",
                        file_name.as_deref().unwrap_or("this file")
                    ),
                    vec![
                        ModalButton::for_app("Save", handle_save_intent(PendingSaveIntent::Save)),
                        ModalButton::for_app(
                            "Don't Save",
                            handle_save_intent(PendingSaveIntent::Discard),
                        ),
                        ModalButton::for_app(
                            "Cancel",
                            handle_save_intent(PendingSaveIntent::Cancel),
                        ),
                    ],
                    |_| {},
                );
                if cfg!(target_os = "macos") {
                    AppContext::show_native_platform_modal(ctx, dialog);
                } else if let Some(workspace) =
                    WorkspaceRegistry::as_ref(ctx).get(ctx.window_id(), ctx)
                {
                    workspace.update(ctx, |workspace, ctx| {
                        workspace.show_native_modal(dialog, ctx)
                    });
                }
            } else {
                self.remove_tab_data_index(index, ctx);
            }
        }
    }

    fn remove_tab_data_index(&mut self, index: usize, ctx: &mut ViewContext<Self>) {
        self.tab_group.remove(index);
        GlobalBufferModel::handle(ctx).update(ctx, |model, ctx| {
            model.remove_deallocated_buffers(ctx);
        });
        self.set_active_tab_index_after_remove(index, ctx);
    }

    pub fn remove_tab_for_move(
        &mut self,
        index: usize,
        ctx: &mut ViewContext<Self>,
    ) -> Option<CodePane> {
        self.tab_at(index).and_then(|t| t.local_path()).map(|path| {
            let source = CodeSource::Link {
                path,
                range_start: None,
                range_end: None,
            };
            self.remove_tab_data_index(index, ctx);
            CodePane::new(source, None, ctx)
        })
    }

    fn remove_tab_with_intent(
        &mut self,
        index: usize,
        intent: Option<PendingSaveIntent>,
        ctx: &mut ViewContext<Self>,
    ) {
        match intent {
            Some(PendingSaveIntent::Save) => {
                self.save_local(
                    index,
                    Some(Box::new(move |outcome, me, ctx| {
                        if outcome != SaveOutcome::Canceled {
                            me.remove_tab_data_index(index, ctx);
                        }
                    })),
                    ctx,
                );
            }
            Some(PendingSaveIntent::Discard) => {
                self.remove_tab_data_index(index, ctx);
            }
            _ => (),
        }
    }

    fn clear_tab_group_with_intent(
        &mut self,
        unsaved_indices: Vec<usize>,
        current_index: usize,
        intent: Option<PendingSaveIntent>,
        ctx: &mut ViewContext<Self>,
    ) {
        match intent {
            Some(PendingSaveIntent::Save) => {
                self.save_local(
                    unsaved_indices[current_index],
                    Some(Box::new(move |outcome, me, ctx| {
                        if outcome != SaveOutcome::Canceled {
                            me.process_next_tab_for_clear(unsaved_indices, current_index + 1, ctx);
                        }
                    })),
                    ctx,
                );
            }
            Some(PendingSaveIntent::Discard) => {
                self.process_next_tab_for_clear(unsaved_indices, current_index + 1, ctx);
            }
            _ => (),
        }
    }

    fn process_next_tab_for_clear(
        &mut self,
        unsaved_indices: Vec<usize>,
        current_index: usize,
        ctx: &mut ViewContext<Self>,
    ) {
        // If we've processed all tabs with unsaved changes, we can clear the tab group immediately.
        if current_index >= unsaved_indices.len() {
            self.cleanup_all_tabs(ctx);
            self.set_active_tab_index(0, ctx);
            return;
        }

        // Otherwise, we need to prompt the user for a decision on the current tab with unsaved changes.
        let tab_index = unsaved_indices[current_index];
        self.remove_tab_with_confirmation(tab_index, true, ctx);
    }

    fn close_saved_tabs(&mut self, ctx: &mut ViewContext<Self>) {
        self.tab_group
            .retain(|tab| Self::has_unsaved_changes(tab, ctx));
        GlobalBufferModel::handle(ctx).update(ctx, |model, ctx| {
            model.remove_deallocated_buffers(ctx);
        });
        self.set_active_tab_index(0, ctx);
    }

    pub fn set_active_tab_index(&mut self, index: usize, ctx: &mut ViewContext<Self>) {
        self.active_tab_index = index;
        self.update_tab_bar_state(ctx);

        ctx.notify();
    }

    fn update_tab_bar_state(&mut self, ctx: &mut ViewContext<Self>) {
        #[cfg(feature = "local_fs")]
        self.refresh_active_image(ctx);
        if self.tab_group.is_empty() {
            ctx.emit(CodeViewEvent::Pane(PaneEvent::Close));
        } else {
            self.set_title_after_content_update(ctx);
            ctx.notify();
        }
    }

    #[cfg(feature = "local_fs")]
    fn refresh_active_image(&mut self, ctx: &mut ViewContext<Self>) {
        let image = self.preview_tab().and_then(|tab| match tab.content {
            TabContent::Image { zoom_percent } => {
                Some((tab.location()?.to_local_path()?, zoom_percent))
            }
            TabContent::Editor(_) => None,
        });
        let current = self
            .active_image
            .as_ref()
            .map(|view| view.as_ref(ctx).path.as_path());
        if image.map(|(path, _)| path) != current {
            self.active_image = image.map(|(path, zoom)| {
                ctx.add_view(|ctx| ImagePreview::new(path.to_path_buf(), zoom, ctx))
            });
        }
    }

    fn preview_tab(&self) -> Option<&TabData> {
        self.tab_at(self.active_tab_index)
            .filter(|tab| tab.location.is_some())
    }

    fn preview_editor<'a>(&self, app: &'a AppContext) -> Option<&'a ViewHandle<CodeEditorView>> {
        self.preview_tab()?
            .editor()
            .map(|editor| editor.as_ref(app).editor())
    }

    fn change_preview_zoom(&mut self, delta: i16, ctx: &mut ViewContext<Self>) {
        let Some(tab) = self
            .tab_group
            .get_mut(self.active_tab_index)
            .filter(|tab| tab.location.is_some())
        else {
            return;
        };
        let current = tab.zoom_percent(ctx);
        let zoom = current
            .saturating_add(delta)
            .clamp(MIN_PREVIEW_ZOOM, MAX_PREVIEW_ZOOM);
        if zoom == current {
            return;
        }
        match &mut tab.content {
            TabContent::Editor(local) => {
                let editor = local.as_ref(ctx).editor().clone();
                editor.update(ctx, |editor, ctx| editor.set_zoom_percent(zoom, ctx));
            }
            #[cfg(feature = "local_fs")]
            TabContent::Image { zoom_percent } => {
                *zoom_percent = zoom;
                if let Some(image) = &self.active_image {
                    image.update(ctx, |image, ctx| image.set_zoom_percent(zoom, ctx));
                }
            }
        }
        self.notify_preview_controls(ctx);
    }

    fn notify_preview_controls(&self, ctx: &mut ViewContext<Self>) {
        self.pane_configuration
            .update(ctx, |config, ctx| config.notify_header_content_changed(ctx));
    }

    #[cfg(all(test, feature = "local_fs"))]
    pub(crate) fn decoded_image_for_test(
        &self,
        ctx: &AppContext,
    ) -> Option<std::sync::Arc<warpui::image_cache::StaticImage>> {
        self.active_image.as_ref()?.as_ref(ctx).decoded().cloned()
    }

    fn relative_path(path: PathBuf, window_id: WindowId, app: &AppContext) -> String {
        let maybe_relative_path = ActiveSession::as_ref(app)
            .path_if_local(window_id)
            .and_then(|cwd| {
                path.strip_prefix(cwd)
                    .ok()
                    .map(|p| p.to_string_lossy().to_string())
            });

        maybe_relative_path.unwrap_or(path.to_string_lossy().to_string())
    }

    fn render_close_button(
        appearance: &Appearance,
        close_handle: MouseStateHandle,
        index: usize,
    ) -> Box<dyn Element> {
        icon_button(
            appearance,
            crate::ui_components::icons::Icon::X,
            false,
            close_handle,
        )
        .build()
        .on_click(move |ctx, _app, _pos| {
            ctx.dispatch_typed_action(
                PaneHeaderAction::<CodeViewAction, CodeViewAction>::CustomAction(
                    CodeViewAction::RemoveTabAtIndex { index },
                ),
            );
        })
        .finish()
    }

    fn calculate_tab_bar_dragged_position(
        drag_position: &RectF,
        index: usize,
        ctx: &ViewContext<Self>,
    ) -> TabBarDragPosition {
        if let Some(tab_rect) = ctx.element_position_by_id(tab_position_id(index)) {
            let tab_center_x = tab_rect.center().x();
            if drag_position.center().x() < tab_center_x {
                TabBarDragPosition::BeforeTab { index }
            } else {
                TabBarDragPosition::AfterTab { index }
            }
        } else {
            // If for some reason we can't retrieve the drag position, assume that the drag occurred before the tab.
            TabBarDragPosition::BeforeTab { index }
        }
    }

    fn render_tab_drag_element(file_name: String, appearance: &Appearance) -> Box<dyn Element> {
        let mut row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_main_axis_alignment(MainAxisAlignment::SpaceBetween);

        let language_icon =
            icon_from_file_path(&file_name, appearance, ItemHighlightState::Default);
        row.add_child(
            Container::new(
                ConstrainedBox::new(language_icon)
                    .with_width(LANGUAGE_ICON_WIDTH)
                    .with_height(LANGUAGE_ICON_WIDTH)
                    .finish(),
            )
            .with_margin_right(TAB_INTERNAL_MARGIN)
            .finish(),
        );

        let file_name_text = Text::new_inline(
            file_name,
            appearance.ui_font_family(),
            appearance.ui_font_size(),
        )
        .finish();
        row.add_child(Shrinkable::new(1., Container::new(file_name_text).finish()).finish());

        Container::new(
            ConstrainedBox::new(row.finish())
                .with_height(34.)
                .with_max_width(200.)
                .finish(),
        )
        .with_vertical_padding(TAB_PADDING)
        .with_horizontal_padding(TAB_PADDING + TAB_HORIZONTAL_MARGIN)
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.)))
        .with_background_color(appearance.theme().surface_1().with_opacity(70).into())
        .finish()
    }

    fn render_tab_internal(
        tab_data: &TabData,
        index: usize,
        is_active: bool,
        is_hovered: bool,
        has_unsaved_changes: bool,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let theme = appearance.theme();
        let text_color = if is_active {
            blended_colors::text_main(theme, theme.surface_1())
        } else {
            blended_colors::text_sub(theme, theme.surface_1())
        };

        let mut row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_main_axis_alignment(MainAxisAlignment::SpaceBetween);

        let file_name = tab_data
            .location
            .as_ref()
            .map(|loc| display_name_with_host(loc, app))
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "Untitled".to_string());
        let language_icon =
            icon_from_file_path(&file_name, appearance, ItemHighlightState::Default);
        row.add_child(
            Container::new(
                ConstrainedBox::new(language_icon)
                    .with_width(LANGUAGE_ICON_WIDTH)
                    .with_height(LANGUAGE_ICON_WIDTH)
                    .finish(),
            )
            .with_margin_right(TAB_INTERNAL_MARGIN)
            .finish(),
        );

        if has_unsaved_changes {
            row.add_child(
                Container::new(render_unsaved_changes_icon(text_color))
                    .with_margin_right(TAB_INTERNAL_MARGIN)
                    .finish(),
            )
        }

        let style = Properties::default().weight(Weight::Semibold);
        let file_name_text = Text::new_inline(
            file_name.clone(),
            appearance.ui_font_family(),
            appearance.ui_font_size(),
        )
        .with_color(text_color)
        .with_style(style)
        .with_clip(ClipConfig::start())
        .finish();
        row.add_child(
            Shrinkable::new(
                1.,
                Container::new(file_name_text)
                    .with_margin_right(TAB_INTERNAL_MARGIN)
                    .finish(),
            )
            .finish(),
        );

        let show_close = is_active || is_hovered;
        row.add_child(
            Shrinkable::new(
                1.,
                if show_close {
                    Self::render_close_button(
                        appearance,
                        tab_data.mouse_state_handles.close_handle.clone(),
                        index,
                    )
                } else {
                    Container::new(
                        ConstrainedBox::new(Empty::new().finish())
                            .with_width(CLOSE_BUTTON_WIDTH)
                            .with_height(CLOSE_BUTTON_WIDTH)
                            .finish(),
                    )
                    .finish()
                },
            )
            .finish(),
        );

        let draggable = Draggable::new(
            tab_data.mouse_state_handles.tab_draggable_state.clone(),
            row.finish(),
        )
        .with_drag_bounds_callback(|_, window_size| Some(RectF::new(Vector2F::zero(), window_size)))
        .with_accepted_by_drop_target_fn(move |_, _| AcceptedByDropTarget::Yes)
        .with_keep_original_visible(true)
        .on_drag(move |ctx, _, drag_position, data| {
            if let Some(tab_group_index) =
                data.and_then(|data| data.as_any().downcast_ref::<EditorTabBarDropTargetData>())
            {
                // If an editor tab is dragged over the editor tab bar, we should clear all drag indicators on the workspace tab group.
                ctx.dispatch_typed_action(
                    PaneHeaderAction::<CodeViewAction, CodeViewAction>::CustomAction(
                        CodeViewAction::ClearWorkspaceTabGroupDragPositions,
                    ),
                );

                ctx.dispatch_typed_action(
                    PaneHeaderAction::<CodeViewAction, CodeViewAction>::CustomAction(
                        CodeViewAction::DragOverIndex {
                            target: tab_group_index.index,
                            drag_position,
                        },
                    ),
                );
            } else if let Some(data) =
                data.and_then(|data| data.as_any().downcast_ref::<TabBarDropTargetData>())
            {
                // If an editor tab is dragged over the workspace tab bar, we should clear all drag indicators on the editor tab group.
                ctx.dispatch_typed_action(
                    PaneHeaderAction::<CodeViewAction, CodeViewAction>::CustomAction(
                        CodeViewAction::ClearEditorTabGroupDragPositions,
                    ),
                );

                ctx.dispatch_typed_action(
                    PaneHeaderAction::<CodeViewAction, CodeViewAction>::PaneHeaderDragged {
                        origin: ActionOrigin::EditorTab(index),
                        drag_location: PaneDragDropLocation::TabBar(data.tab_bar_location),
                        drag_position,
                        tab_bar_axis: Some(TabBarAxis::Horizontal),
                    },
                );
            } else {
                // If an editor tab is dragged anywhere else, we should clear all drag indicators on the editor and workspace tab groups.
                ctx.dispatch_typed_action(
                    PaneHeaderAction::<CodeViewAction, CodeViewAction>::CustomAction(
                        CodeViewAction::ClearWorkspaceTabGroupDragPositions,
                    ),
                );
                ctx.dispatch_typed_action(
                    PaneHeaderAction::<CodeViewAction, CodeViewAction>::CustomAction(
                        CodeViewAction::ClearEditorTabGroupDragPositions,
                    ),
                );
            }
        })
        .on_drop(move |ctx, _, drag_position, data| {
            if let Some(tab_group_index) =
                data.and_then(|data| data.as_any().downcast_ref::<EditorTabBarDropTargetData>())
            {
                ctx.dispatch_typed_action(
                    PaneHeaderAction::<CodeViewAction, CodeViewAction>::CustomAction(
                        CodeViewAction::DropAtIndex {
                            origin: index,
                            target: tab_group_index.index,
                            drag_position,
                        },
                    ),
                );
            } else if let Some(data) =
                data.and_then(|data| data.as_any().downcast_ref::<TabBarDropTargetData>())
            {
                ctx.dispatch_typed_action(
                    PaneHeaderAction::<CodeViewAction, CodeViewAction>::PaneHeaderDropped {
                        origin: ActionOrigin::EditorTab(index),
                        drop_location: PaneDragDropLocation::TabBar(data.tab_bar_location),
                    },
                );
            }
        })
        .with_alternate_drag_element(Self::render_tab_drag_element(file_name, appearance));

        SavePosition::new(
            if !tab_data
                .mouse_state_handles
                .tab_draggable_state
                .is_dragging()
            {
                DropTarget::new(draggable.finish(), EditorTabBarDropTargetData { index }).finish()
            } else {
                draggable.finish()
            },
            &tab_position_id(index),
        )
        .finish()
    }

    fn render_preview_zoom(&self, app: &AppContext) -> Option<Box<dyn Element>> {
        let zoom = self.preview_tab()?.zoom_percent(app);
        let appearance = Appearance::as_ref(app);
        let mut row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_main_axis_size(MainAxisSize::Min);
        for (icon, mouse, enabled, delta, label) in [
            (
                Icon::Minus,
                &self.preview_controls.zoom_out,
                zoom > MIN_PREVIEW_ZOOM,
                -PREVIEW_ZOOM_STEP,
                "Zoom out preview",
            ),
            (
                Icon::Plus,
                &self.preview_controls.zoom_in,
                zoom < MAX_PREVIEW_ZOOM,
                PREVIEW_ZOOM_STEP,
                "Zoom in preview",
            ),
        ] {
            let button = icon_button(appearance, icon, false, mouse.clone());
            row.add_child(Self::preview_button(
                appearance,
                if enabled { button } else { button.disabled() },
                CodeViewAction::ChangePreviewZoom(delta),
                label,
            ));
        }
        Some(row.finish())
    }

    fn render_preview_wrap(&self, app: &AppContext) -> Option<Box<dyn Element>> {
        let wrapping = self.preview_editor(app)?.as_ref(app).word_wrap(app);
        let appearance = Appearance::as_ref(app);
        Some(Self::preview_button(
            appearance,
            icon_button(
                appearance,
                Icon::TextWrap,
                wrapping,
                self.preview_controls.wrap.clone(),
            ),
            CodeViewAction::TogglePreviewWrap,
            if wrapping {
                "Disable word wrap"
            } else {
                "Enable word wrap"
            },
        ))
    }

    fn render_trailing_controls(
        &self,
        header_ctx: &view::HeaderRenderContext,
        app: &AppContext,
    ) -> (Flex, u32) {
        let show_close = self
            .focus_handle
            .as_ref()
            .is_some_and(|handle| handle.is_in_split_pane(app));
        let wrap = self.render_preview_wrap(app);
        let count = u32::from(wrap.is_some())
            + u32::from(header_ctx.has_overflow_items)
            + u32::from(show_close);
        let mut buttons = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_main_axis_size(MainAxisSize::Min);
        buttons.add_children(wrap);
        buttons.add_child(
            render_pane_header_buttons::<CodeViewAction, CodeViewAction>(
                header_ctx,
                Appearance::as_ref(app),
                show_close,
                None,
                None,
            ),
        );
        (buttons, count)
    }

    fn preview_button(
        appearance: &Appearance,
        button: Button,
        action: CodeViewAction,
        label: &'static str,
    ) -> Box<dyn Element> {
        let builder = appearance.ui_builder().clone();
        button
            .with_tooltip(move || builder.tool_tip(tr(label).to_owned()).build().finish())
            .build()
            .on_click(move |ctx, _, _| {
                ctx.dispatch_typed_action(
                    PaneHeaderAction::<CodeViewAction, CodeViewAction>::CustomAction(
                        action.clone(),
                    ),
                );
            })
            .finish()
    }

    /// Renders the tab bar with explicit draggable handling for multi-tab case.
    fn render_tab_bar_with_draggable(
        &self,
        header_ctx: &view::HeaderRenderContext,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let is_pane_dragging = header_ctx.draggable_state.is_dragging();

        let mut header_row = Flex::row()
            .with_main_axis_alignment(MainAxisAlignment::Start)
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Min);

        if header_ctx.header_left_inset > 0. {
            header_row.add_child(
                Container::new(Empty::new().finish())
                    .with_padding_left(header_ctx.header_left_inset)
                    .finish(),
            );
        }
        if let Some(zoom) = self.render_preview_zoom(app) {
            header_row.add_child(
                Container::new(Align::new(zoom).finish())
                    .with_padding_left(PADDING)
                    .with_border(
                        Border::bottom(TAB_BAR_BORDER_HEIGHT).with_border_fill(theme.outline()),
                    )
                    .finish(),
            );
        }

        let mut tabs_row = Flex::row()
            .with_main_axis_alignment(MainAxisAlignment::Start)
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(if is_pane_dragging {
                MainAxisSize::Min
            } else {
                MainAxisSize::Max
            });

        for (index, tab_data) in self.tab_group.iter().enumerate() {
            let is_active = index == self.active_tab_index;
            let close_handle = tab_data.mouse_state_handles.close_handle.clone();
            let tab = Hoverable::new(
                tab_data.mouse_state_handles.tab_handle.clone(),
                |tab_handle| {
                    let mut stack = Stack::new();
                    let container = Container::new(
                        Container::new(Self::render_tab_internal(
                            tab_data,
                            index,
                            is_active,
                            tab_handle.is_hovered(),
                            Self::show_unsaved_indicator(tab_data, app),
                            appearance,
                            app,
                        ))
                        .with_horizontal_margin(TAB_HORIZONTAL_MARGIN)
                        .with_padding(Padding::uniform(TAB_PADDING))
                        .finish(),
                    )
                    .with_border(
                        Border::new(TAB_BAR_BORDER_HEIGHT)
                            .with_border_fill(theme.outline())
                            .with_sides(false, false, !is_active, true),
                    );

                    // Renders a border to the left/right of a tab being dragged over to display the intended drop position.
                    let border = match &self.drag_position {
                        Some(TabBarDragPosition::BeforeTab { index: drag_index })
                            if *drag_index == index =>
                        {
                            Some(Border::left(2.).with_border_fill(theme.foreground()))
                        }
                        Some(TabBarDragPosition::AfterTab { index: drag_index })
                            if *drag_index == index =>
                        {
                            Some(Border::right(2.).with_border_fill(theme.foreground()))
                        }
                        _ => None,
                    };

                    if let Some(border) = border {
                        stack.add_child(
                            Container::new(container.finish())
                                .with_border(border)
                                .finish(),
                        );
                    } else {
                        stack.add_child(container.finish());
                    }

                    if tab_handle.is_hovered()
                        && !tab_data
                            .mouse_state_handles
                            .tab_draggable_state
                            .is_dragging()
                        && let Some(path) = tab_data.local_path()
                    {
                        let tooltip = appearance
                            .ui_builder()
                            .tool_tip(Self::relative_path(path, self.window_id, app))
                            .build()
                            .finish();
                        stack.add_positioned_overlay_child(
                            tooltip,
                            OffsetPositioning::offset_from_parent(
                                vec2f(10., -1.),
                                ParentOffsetBounds::Unbounded,
                                ParentAnchor::BottomLeft,
                                ChildAnchor::TopLeft,
                            ),
                        );
                    }

                    stack.finish()
                },
            )
            .on_click(move |ctx, _app, _pos| {
                let is_close_button_hovered = close_handle
                    .lock()
                    .map(|handle| handle.is_hovered())
                    .unwrap_or(false);

                if !is_close_button_hovered {
                    ctx.dispatch_typed_action(
                        PaneHeaderAction::<CodeViewAction, CodeViewAction>::CustomAction(
                            CodeViewAction::SetCurrentTabIndex { index },
                        ),
                    );
                }
            })
            .on_middle_click(move |ctx, _app, _pos| {
                ctx.dispatch_typed_action(
                    PaneHeaderAction::<CodeViewAction, CodeViewAction>::CustomAction(
                        CodeViewAction::RemoveTabAtIndex { index },
                    ),
                );
            });

            let tab_element = tab.finish();
            if is_pane_dragging {
                // ConstrainedBox gives the tab finite horizontal constraints so
                // its internal Shrinkable children don't hit the infinite-flex
                // assertion.
                tabs_row.add_child(
                    ConstrainedBox::new(tab_element)
                        .with_max_width(300.)
                        .finish(),
                );
            } else {
                tabs_row.add_child(Shrinkable::new(1., tab_element).finish());
            }
        }

        // Draggable spacer fills the gap between tabs and buttons and supports header dragging.
        let spacer = Container::new(Empty::new().finish())
            .with_border(Border::bottom(TAB_BAR_BORDER_HEIGHT).with_border_fill(theme.outline()))
            .finish();
        let draggable_spacer = render_pane_header_draggable::<CodeView>(
            self.pane_configuration.clone(),
            spacer,
            header_ctx.draggable_state.clone(),
            app,
        );
        tabs_row.add_child(if is_pane_dragging {
            draggable_spacer
        } else {
            Expanded::new(1., draggable_spacer).finish()
        });

        // Clip tabs so overflow doesn't push the buttons off-screen.
        let clipped_tabs = Clipped::new(tabs_row.finish()).finish();
        header_row.add_child(if is_pane_dragging {
            clipped_tabs
        } else {
            Expanded::new(1., clipped_tabs).finish()
        });

        let (buttons, _) = self.render_trailing_controls(header_ctx, app);
        header_row.add_child(
            Container::new(Align::new(buttons.finish()).finish())
                .with_padding_right(4.)
                .with_border(
                    Border::bottom(TAB_BAR_BORDER_HEIGHT).with_border_fill(theme.outline()),
                )
                .finish(),
        );

        header_row.finish()
    }

    /// Renders the header for the single-tab (or empty) case with a centered title.
    fn render_single_tab_header(
        &self,
        header_ctx: &view::HeaderRenderContext,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let tab = self.tab_group.first();
        let title = tab
            .and_then(TabData::location)
            .map(|location| display_name_with_host(location, app))
            .filter(|title| !title.is_empty())
            .unwrap_or_else(|| "Untitled".to_string());

        let appearance = Appearance::as_ref(app);
        let is_pane_dragging = header_ctx.draggable_state.is_dragging();
        let zoom = self.render_preview_zoom(app);
        let (right_row, right_count) = self.render_trailing_controls(header_ctx, app);
        let button_count = right_count.max(if zoom.is_some() { 2 } else { 0 });

        let buttons_width = button_count as f32 * ICON_DIMENSIONS;
        let edge_width = view::StandardHeaderOptions::DEFAULT_CONTROL_CONTAINER_WIDTH;

        // Build the center title element, with a hover tooltip showing the full path.
        let title_element = if let Some(tab) = tab {
            let has_unsaved = Self::show_unsaved_indicator(tab, app);
            Hoverable::new(tab.mouse_state_handles.tab_handle.clone(), |hover_state| {
                let title_text =
                    render_pane_header_title_text(title.clone(), appearance, ClipConfig::start());

                let mut title_row = Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_main_axis_size(MainAxisSize::Min);
                if has_unsaved {
                    let dot_color = appearance
                        .theme()
                        .sub_text_color(appearance.theme().background());
                    title_row.add_child(
                        Container::new(render_unsaved_changes_icon(dot_color.into()))
                            .with_margin_right(4.)
                            .finish(),
                    );
                }
                title_row.add_child(title_text);

                let mut stack = Stack::new();
                stack.add_child(title_row.finish());
                if hover_state.is_hovered()
                    && let Some(location) = tab.location()
                {
                    let tooltip = appearance
                        .ui_builder()
                        .tool_tip(location.display_path())
                        .build()
                        .finish();
                    stack.add_positioned_overlay_child(
                        tooltip,
                        OffsetPositioning::offset_from_parent(
                            vec2f(0., 4.),
                            ParentOffsetBounds::Unbounded,
                            ParentAnchor::BottomMiddle,
                            ChildAnchor::TopMiddle,
                        ),
                    );
                }
                stack.finish()
            })
            .finish()
        } else {
            render_pane_header_title_text(title, appearance, ClipConfig::start())
        };

        render_three_column_header(
            zoom.unwrap_or_else(|| Empty::new().finish()),
            title_element,
            right_row
                .with_main_axis_alignment(MainAxisAlignment::End)
                .finish(),
            CenteredHeaderEdgeWidth {
                min: buttons_width,
                max: edge_width,
            },
            header_ctx.header_left_inset,
            is_pane_dragging,
        )
    }

    fn overflow_menu_items(&self, ctx: &AppContext) -> Vec<MenuItem<CodeViewAction>> {
        let is_maximized = self
            .focus_handle
            .as_ref()
            .is_some_and(|h| h.is_maximized(ctx));
        let modifier_keys = if cfg!(target_os = "macos") {
            "⌘R"
        } else {
            "Ctrl-R"
        };

        let mut items = vec![
            MenuItemFields::new_with_label("Close saved", &format!("{modifier_keys} U"))
                .with_on_select_action(CodeViewAction::CloseSaved)
                .into_item(),
            MenuItemFields::toggle_pane_action(is_maximized)
                .with_on_select_action(CodeViewAction::ToggleMaximized)
                .into_item(),
        ];

        #[cfg(feature = "local_fs")]
        {
            let active_location = self
                .tab_at(self.active_tab_index)
                .and_then(|t| t.location.as_ref());
            let local_path = self.local_path(ctx);

            if active_location.is_some() {
                items.push(MenuItem::Separator);
                items.push(
                    MenuItemFields::new("Copy file path")
                        .with_on_select_action(CodeViewAction::CopyFilePath)
                        .into_item(),
                );
            }

            if local_path.is_some() {
                let reveal_label = if cfg!(target_os = "macos") {
                    "Reveal in Finder"
                } else if cfg!(target_os = "windows") {
                    "Reveal in Explorer"
                } else {
                    "Reveal in file manager"
                };
                items.push(
                    MenuItemFields::new(reveal_label)
                        .with_on_select_action(CodeViewAction::RevealInFinder)
                        .into_item(),
                );
            }
        }

        items
    }

    /// Merges tabs from another `CodeView`, avoiding duplicates and updating the active tab index.
    pub fn merge_tabs(&mut self, source_code_view: &CodeView, ctx: &mut ViewContext<Self>) {
        let existing_locations_to_idx: HashMap<&LocalOrRemotePath, usize> = self
            .tab_group
            .iter()
            .enumerate()
            .filter_map(|(idx, tab)| tab.location.as_ref().map(|loc| (loc, idx)))
            .collect();
        let mut active_tab_index = self.active_tab_index();
        let mut to_extend: Vec<TabData> = Vec::new();

        for (i, tab_data) in source_code_view.tab_group.iter().enumerate() {
            if let Some(loc) = tab_data.location.as_ref() {
                if let Some(&index) = existing_locations_to_idx.get(loc) {
                    // If the tab already exists in the tab group and is the active tab in the source CodeView,
                    // update the active tab index to point to it.
                    if i == source_code_view.active_tab_index() {
                        active_tab_index = index;
                    }
                } else {
                    to_extend.push(tab_data.clone());
                    // If the newly added tab is the active tab in the source CodeView, update the active tab index to point to it.
                    if i == source_code_view.active_tab_index() {
                        active_tab_index = self.tab_group.len() + to_extend.len() - 1;
                    }
                }
            }
        }

        self.tab_group.extend(to_extend);
        self.set_active_tab_index(active_tab_index, ctx);
    }
}

impl Entity for CodeView {
    type Event = CodeViewEvent;
}

impl View for CodeView {
    fn ui_name() -> &'static str {
        "CodeView"
    }

    fn render(&self, _app: &AppContext) -> Box<dyn Element> {
        #[cfg(feature = "local_fs")]
        if let Some(image) = &self.active_image {
            return Container::new(ChildView::new(image).finish())
                .with_padding_top(PADDING)
                .finish();
        }
        let body =
            if let Some(editor) = self.tab_at(self.active_tab_index).and_then(TabData::editor) {
                ChildView::new(editor).finish()
            } else {
                Empty::new().finish()
            };

        Container::new(body).with_padding_top(PADDING).finish()
    }
}

impl TypedActionView for CodeView {
    type Action = CodeViewAction;

    fn handle_action(&mut self, action: &Self::Action, ctx: &mut ViewContext<Self>) {
        match action {
            CodeViewAction::SaveFile => {
                self.save_local(self.active_tab_index, None, ctx);
            }
            CodeViewAction::SaveFileAs => {
                self.save_as(self.active_tab_index, None, ctx);
            }
            CodeViewAction::SetCurrentTabIndex { index } => {
                self.set_active_tab_index(*index, ctx);
            }

            CodeViewAction::RemoveTabAtIndex { index } => {
                self.remove_tab_with_confirmation(*index, false, ctx);
            }

            CodeViewAction::CloseAll => {
                let unsaved_indices = self.unsaved_indices(ctx);
                self.process_next_tab_for_clear(unsaved_indices, 0, ctx);
            }

            CodeViewAction::CloseSaved => {
                self.close_saved_tabs(ctx);
            }

            CodeViewAction::ToggleMaximized => {
                ctx.emit(CodeViewEvent::Pane(PaneEvent::ToggleMaximized));
                self.pane_configuration.update(ctx, |pane_config, ctx| {
                    pane_config.refresh_pane_header_overflow_menu_items(ctx);
                });
            }

            CodeViewAction::TogglePreviewWrap => {
                if let Some(editor) = self.preview_editor(ctx).cloned() {
                    let enable = !editor.as_ref(ctx).word_wrap(ctx);
                    editor.update(ctx, |editor, ctx| editor.set_word_wrap(enable, ctx));
                    self.notify_preview_controls(ctx);
                }
            }
            CodeViewAction::ChangePreviewZoom(delta) => self.change_preview_zoom(*delta, ctx),

            #[cfg(feature = "local_fs")]
            CodeViewAction::CopyFilePath => {
                if let Some(location) = self
                    .tab_at(self.active_tab_index)
                    .and_then(|t| t.location.as_ref())
                {
                    let path = crate::util::path::clipboard_file_path(location);
                    ctx.clipboard().write(ClipboardContent::plain_text(path));
                }
            }
            #[cfg(feature = "local_fs")]
            CodeViewAction::RevealInFinder => {
                if let Some(path) = self.local_path(ctx) {
                    ctx.open_file_path_in_explorer(&path);
                } else {
                    log::warn!(
                        "Reveal in Finder requested, but the active code tab has no local file path"
                    );
                }
            }

            CodeViewAction::DragOverIndex {
                target,
                drag_position,
            } => {
                self.drag_position = Some(Self::calculate_tab_bar_dragged_position(
                    drag_position,
                    *target,
                    ctx,
                ));
                self.update_tab_bar_state(ctx);
            }

            CodeViewAction::DropAtIndex {
                origin,
                target,
                drag_position,
            } => {
                self.clear_drag_position();

                let calculated_drag_position =
                    Self::calculate_tab_bar_dragged_position(drag_position, *target, ctx);
                let mut target_index = *target;
                if *origin <= *target
                    && matches!(
                        calculated_drag_position,
                        TabBarDragPosition::BeforeTab { .. }
                    )
                {
                    target_index = target.saturating_sub(1);
                } else if *origin > *target
                    && matches!(
                        calculated_drag_position,
                        TabBarDragPosition::AfterTab { .. }
                    )
                {
                    target_index = (*target + 1).min(self.tab_group.len().saturating_sub(1));
                }

                if *origin != target_index {
                    let tab = self.tab_group.remove(*origin);
                    self.tab_group.insert(target_index, tab);
                    self.active_tab_index = target_index;
                }
                self.update_tab_bar_state(ctx);
            }

            CodeViewAction::ClearEditorTabGroupDragPositions => {
                self.clear_drag_position();
                self.update_tab_bar_state(ctx);
            }

            CodeViewAction::ClearWorkspaceTabGroupDragPositions => {
                ctx.emit(CodeViewEvent::Pane(PaneEvent::ClearHoveredTabIndex));
            }
        }
    }
}

impl BackingView for CodeView {
    type PaneHeaderOverflowMenuAction = CodeViewAction;
    type CustomAction = CodeViewAction;
    type AssociatedData = ();

    fn pane_header_overflow_menu_items(&self, ctx: &AppContext) -> Vec<MenuItem<CodeViewAction>> {
        self.overflow_menu_items(ctx)
    }

    fn handle_pane_header_overflow_menu_action(
        &mut self,
        action: &Self::PaneHeaderOverflowMenuAction,
        ctx: &mut ViewContext<Self>,
    ) {
        self.handle_action(action, ctx);
    }

    fn close(&mut self, ctx: &mut ViewContext<Self>) {
        self.handle_action(&CodeViewAction::CloseAll, ctx);
    }

    fn focus_contents(&mut self, ctx: &mut ViewContext<Self>) {
        self.focus(ctx);
    }

    fn handle_custom_action(
        &mut self,
        custom_action: &Self::CustomAction,
        ctx: &mut ViewContext<Self>,
    ) {
        self.handle_action(custom_action, ctx);
    }

    fn render_header_content(
        &self,
        ctx: &view::HeaderRenderContext,
        app: &AppContext,
    ) -> view::HeaderContent {
        if self.tab_group.len() >= 2 {
            // Multi-tab case: render custom tab bar with explicit draggable handling
            view::HeaderContent::Custom {
                element: self.render_tab_bar_with_draggable(ctx, app),
                has_custom_draggable_behavior: true,
            }
        } else {
            view::HeaderContent::Custom {
                element: self.render_single_tab_header(ctx, app),
                has_custom_draggable_behavior: false,
            }
        }
    }

    fn set_focus_handle(&mut self, focus_handle: PaneFocusHandle, _ctx: &mut ViewContext<Self>) {
        self.focus_handle = Some(focus_handle);
    }
}

fn render_unsaved_changes_icon(color: ColorU) -> Box<dyn Element> {
    ConstrainedBox::new(
        Rect::new()
            .with_background_color(color)
            .with_corner_radius(CornerRadius::with_all(Radius::Percentage(50.)))
            .finish(),
    )
    .with_width(8.)
    .with_height(8.)
    .finish()
}
