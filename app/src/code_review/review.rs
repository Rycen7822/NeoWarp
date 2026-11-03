//! Local, read-only Git review with independently collapsible file cards.
//! Only visible, expanded cards load detail, one file at a time.
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use warp_editor::render::element::VerticalExpansionBehavior;
use warpui::r#async::SpawnedFutureHandle;
use warpui::clipboard::ClipboardContent;
use warpui::elements::new_scrollable::{NewScrollableElement, SingleAxisConfig};
use warpui::elements::{
    Border, ChildView, ConstrainedBox, Container, CrossAxisAlignment, Empty, Expanded, Flex, List,
    ListState, MouseStateHandle, NewScrollable, Padding, ParentElement, ScrollStateHandle,
    Shrinkable, Text,
};
use warpui::ui_components::components::UiComponent;
use warpui::units::Pixels;
use warpui::{
    AppContext, Element, Entity, EntityId, ModelHandle, SingletonEntity, TypedActionView, View,
    ViewContext, ViewHandle,
};

use super::diff_size_limits::{MAX_CHARACTERS_PER_LINE, MAX_DIFF_SIZE};
use super::diff_state::{DiffMode, GitFileStatus, LocalDiffStateModel};
use super::git_repo_model::{GitRepoStatusEvent, GitRepoStatusModel};
use crate::appearance::Appearance;
use crate::code::editor::view::{
    CodeEditorEvent, CodeEditorRenderOptions, CodeEditorView, ReviewEditorState,
};
use crate::code::editor::{add_color, remove_color};
use crate::localization::tr;
use crate::ui_components::buttons::{close_button, icon_button};
use crate::ui_components::icons::Icon;
use crate::util::git::FileChangeEntry;
use crate::workspace::WorkspaceAction;

struct FileCard {
    entry: FileChangeEntry,
    editor: Option<ViewHandle<CodeEditorView>>,
    editor_state: Option<ReviewEditorState>,
    base: Option<Arc<String>>,
    expanded: bool,
    dirty: bool,
    message: String,
    toggle_mouse: MouseStateHandle,
    copy_mouse: MouseStateHandle,
    open_mouse: MouseStateHandle,
}

impl FileCard {
    fn new(entry: FileChangeEntry) -> Self {
        Self {
            entry,
            editor: None,
            editor_state: None,
            base: None,
            expanded: true,
            dirty: true,
            message: tr("Loading changes…").into(),
            toggle_mouse: Default::default(),
            copy_mouse: Default::default(),
            open_mouse: Default::default(),
        }
    }
}

pub struct GitReviewView {
    root: PathBuf,
    status: ModelHandle<GitRepoStatusModel>,
    files: Vec<FileCard>,
    message: String,
    loading: Option<(String, SpawnedFutureHandle)>,
    list: ListState<()>,
    scroll: ScrollStateHandle,
    close_mouse: MouseStateHandle,
}

impl GitReviewView {
    pub fn new(
        root: PathBuf,
        status: ModelHandle<GitRepoStatusModel>,
        ctx: &mut ViewContext<Self>,
    ) -> Self {
        let weak = ctx.handle();
        let (visible_tx, visible_rx) = async_channel::unbounded();
        let list = ListState::new(move |index, _, app| {
            let Some(view) = weak.upgrade(app) else {
                return Empty::new().finish();
            };
            let me = view.as_ref(app);
            let Some(card) = me.files.get(index) else {
                return Empty::new().finish();
            };
            if card.expanded && card.dirty && me.loading.is_none() {
                // Layout only reports demand; all state changes and I/O belong to the view.
                let _ = visible_tx.try_send(index);
            }
            me.render_card(card, app)
        });
        ctx.spawn_stream_local(
            visible_rx,
            |me, index, ctx| me.load_visible(index, ctx),
            |_, _| (),
        );
        ctx.subscribe_to_model(&status, |me, _, event, ctx| match event {
            GitRepoStatusEvent::MetadataChanged => {
                if me.status.as_ref(ctx).metadata(ctx).is_none() {
                    me.message = tr("Unable to read repository changes").into();
                    ctx.notify();
                } else {
                    me.update_files(ctx);
                }
            }
            GitRepoStatusEvent::FilesInvalidated(changes) => {
                for card in &mut me.files {
                    if changes.git_changed {
                        card.base = None;
                    }
                    if changes.full
                        || changes.git_changed
                        || changes.paths.contains(&PathBuf::from(&card.entry.path))
                    {
                        card.dirty = true;
                    }
                }
                ctx.notify();
            }
        });
        let mut view = Self {
            root,
            status,
            files: Vec::new(),
            message: tr("Loading changes…").into(),
            loading: None,
            list,
            scroll: Default::default(),
            close_mouse: Default::default(),
        };
        view.update_files(ctx);
        view
    }

    fn update_files(&mut self, ctx: &mut ViewContext<Self>) {
        if self.status.as_ref(ctx).metadata(ctx).is_none() {
            return;
        }
        let old_len = self.files.len();
        let mut previous: HashMap<_, _> = self
            .files
            .drain(..)
            .map(|mut card| (std::mem::take(&mut card.entry.path), card))
            .collect();
        self.files = self
            .status
            .as_ref(ctx)
            .files(ctx)
            .unwrap_or_default()
            .iter()
            .cloned()
            .map(|entry| {
                if let Some(mut card) = previous.remove(&entry.path) {
                    card.entry = entry;
                    card
                } else {
                    FileCard::new(entry)
                }
            })
            .collect();
        for _ in old_len..self.files.len() {
            self.list.add_item();
        }
        for index in (self.files.len()..old_len).rev() {
            self.list.remove(index);
        }
        for index in 0..self.files.len() {
            self.list.invalidate_height_for_index(index);
        }
        if self
            .loading
            .as_ref()
            .is_some_and(|(path, _)| !self.files.iter().any(|card| card.entry.path == *path))
        {
            self.cancel_loading();
        }
        self.message = if self.files.is_empty() {
            tr("No uncommitted changes").into()
        } else {
            String::new()
        };
        ctx.notify();
    }

    fn cancel_loading(&mut self) {
        if let Some((_, task)) = self.loading.take() {
            task.abort();
        }
    }

    fn load_visible(&mut self, index: usize, ctx: &mut ViewContext<Self>) {
        // The list may measure preceding items to estimate height; those are not load requests.
        if self.loading.is_some()
            || index >= self.files.len()
            || (index != self.list.get_scroll_index()
                && !self
                    .list
                    .is_vertical_range_visible(index, Pixels::zero(), Pixels::zero()))
        {
            return;
        }
        let card = &mut self.files[index];
        if !card.expanded || !card.dirty {
            return;
        }
        card.dirty = false;
        card.message = tr("Updating…").into();
        let path = card.entry.path.clone();
        let root = self.root.clone();
        // Keep the cached snapshot available if an edit supersedes this cancellable read.
        let base = card.base.clone();
        let result_path = path.clone();
        self.loading = Some((
            path.clone(),
            ctx.spawn(
                async move { load_file(root, path, base).await },
                move |me, result, ctx| {
                    me.loading = None;
                    let Some(index) = me
                        .files
                        .iter()
                        .position(|card| card.entry.path == result_path)
                    else {
                        ctx.notify();
                        return;
                    };
                    // A save or Git change during this read supersedes its snapshot.
                    if me.files[index].dirty || !me.files[index].expanded {
                        ctx.notify();
                        return;
                    }
                    match result {
                        Ok(Some((base, current))) => {
                            let editor = if let Some(editor) = &me.files[index].editor {
                                editor.clone()
                            } else {
                                let editor = ctx.add_typed_action_view(|ctx| {
                                    CodeEditorView::new(
                                        None,
                                        None,
                                        CodeEditorRenderOptions::new(
                                            VerticalExpansionBehavior::GrowToMaxHeight,
                                        ),
                                        ctx,
                                    )
                                });
                                let path = result_path.clone();
                                ctx.subscribe_to_view(&editor, move |me, editor, event, ctx| {
                                    if matches!(
                                        event,
                                        CodeEditorEvent::DiffUpdated
                                            | CodeEditorEvent::HiddenSectionExpanded
                                            | CodeEditorEvent::LayoutInvalidated
                                    ) && let Some(index) =
                                        me.files.iter().position(|card| card.entry.path == path)
                                    {
                                        if matches!(event, CodeEditorEvent::DiffUpdated)
                                            && !me.files[index].dirty
                                            && let Some(state) = me.files[index].editor_state.take()
                                        {
                                            editor.update(ctx, |editor, ctx| {
                                                editor.restore_review_state(state, ctx);
                                            });
                                        }
                                        me.list.invalidate_height_for_index(index);
                                        ctx.notify();
                                    }
                                });
                                editor.update(ctx, |editor, ctx| {
                                    editor.set_language_with_local_path(
                                        &me.root.join(&result_path),
                                        ctx,
                                    );
                                });
                                me.files[index].editor = Some(editor.clone());
                                editor
                            };
                            editor.update(ctx, |editor, ctx| {
                                editor.set_review_content(&base, &current, ctx);
                            });
                            me.files[index].base = Some(base);
                            me.files[index].message.clear();
                        }
                        Ok(None) => {
                            me.files[index].editor = None;
                            me.files[index].message = tr("No text diff to display").into();
                        }
                        Err(error) => {
                            me.files[index].editor = None;
                            me.files[index].message =
                                format!("{}: {error:#}", tr("Unable to load diff"));
                        }
                    }
                    me.list.invalidate_height_for_index(index);
                    ctx.notify();
                },
            ),
        ));
        ctx.notify();
    }

    fn render_card(&self, card: &FileCard, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let path = card.entry.path.clone();
        let mut header = Flex::row().with_cross_axis_alignment(CrossAxisAlignment::Center);
        header.add_child(
            icon_button(
                appearance,
                if card.expanded {
                    Icon::ChevronDown
                } else {
                    Icon::ChevronRight
                },
                false,
                card.toggle_mouse.clone(),
            )
            .build()
            .on_click(move |ctx, _, _| {
                ctx.dispatch_typed_action(GitReviewAction::Toggle(path.clone()));
            })
            .finish(),
        );
        let path = card.entry.path.clone();
        header.add_child(
            Shrinkable::new(
                1.,
                Text::new_inline(path, appearance.ui_font_family(), 13.)
                    .with_color(theme.main_text_color(theme.surface_2()).into())
                    .finish(),
            )
            .finish(),
        );
        let copy_path = card.entry.path.clone();
        header.add_child(
            icon_button(appearance, Icon::Copy, false, card.copy_mouse.clone())
                .build()
                .on_click(move |ctx, _, _| {
                    ctx.dispatch_typed_action(GitReviewAction::CopyPath(copy_path.clone()));
                })
                .finish(),
        );
        header.add_child(diff_badge(
            card.entry.additions,
            card.entry.deletions,
            appearance,
        ));
        header.add_child(Expanded::new(1., Empty::new().finish()).finish());
        let path = self.root.join(&card.entry.path);
        header.add_child(
            icon_button(
                appearance,
                Icon::LinkExternal,
                false,
                card.open_mouse.clone(),
            )
            .build()
            .on_click(move |ctx, _, _| {
                ctx.dispatch_typed_action(WorkspaceAction::OpenFilePath { path: path.clone() });
            })
            .finish(),
        );
        let mut column = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(
                Container::new(header.finish())
                    .with_background(theme.surface_2())
                    .with_padding(Padding::uniform(8.))
                    .finish(),
            );
        if card.expanded {
            if let Some(editor) = &card.editor {
                column.add_child(
                    ConstrainedBox::new(ChildView::new(editor).finish())
                        .with_max_height(1000.)
                        .finish(),
                );
            } else {
                column.add_child(
                    Container::new(
                        Text::new_inline(card.message.clone(), appearance.ui_font_family(), 13.)
                            .with_color(theme.sub_text_color(theme.background()).into())
                            .finish(),
                    )
                    .with_padding(Padding::uniform(12.))
                    .finish(),
                );
            }
        }
        Container::new(column.finish())
            .with_background(theme.background())
            .with_border(Border::all(1.).with_border_fill(theme.outline()))
            .with_margin_bottom(12.)
            .finish()
    }
}

fn diff_badge(additions: usize, deletions: usize, appearance: &Appearance) -> Box<dyn Element> {
    let mut row = Flex::row().with_cross_axis_alignment(CrossAxisAlignment::Center);
    row.add_child(
        Text::new_inline(format!("+{additions}"), appearance.ui_font_family(), 12.)
            .with_color(add_color(appearance))
            .finish(),
    );
    row.add_child(
        Container::new(
            Text::new_inline(format!("−{deletions}"), appearance.ui_font_family(), 12.)
                .with_color(remove_color(appearance))
                .finish(),
        )
        .with_margin_left(8.)
        .finish(),
    );
    Container::new(row.finish())
        .with_padding(Padding::uniform(3.).with_horizontal(8.))
        .with_margin_right(8.)
        .finish()
}

async fn load_file(
    root: PathBuf,
    path: String,
    cached_base: Option<Arc<String>>,
) -> Result<Option<(Arc<String>, String)>> {
    // The editor already computes and renders the diff. Querying a Git patch
    // and its binary numstat first repeats that work and adds WSL launches.
    let Some((_, status)) =
        LocalDiffStateModel::file_status_for_path(&root, &path, &DiffMode::Head, None).await?
    else {
        return Ok(None);
    };
    let base = match (cached_base, &status) {
        (Some(base), _) => base,
        (_, GitFileStatus::New | GitFileStatus::Untracked) => Arc::new(String::new()),
        (_, status) => {
            let base_path = match status {
                GitFileStatus::Renamed { old_path } | GitFileStatus::Copied { old_path } => {
                    old_path
                }
                _ => &path,
            };
            Arc::new(
                warp_util::git::read_head_file(&root, base_path, MAX_DIFF_SIZE)
                    .await
                    .context("cannot read HEAD content")?,
            )
        }
    };
    // Lossy UTF-8 decoding can expand the bounded raw output.
    if base.len() > MAX_DIFF_SIZE {
        anyhow::bail!("base content is too large to display");
    }
    let current = if matches!(status, GitFileStatus::Deleted) {
        String::new()
    } else {
        let file = root.join(path);
        if async_fs::metadata(&file).await?.len() > MAX_DIFF_SIZE as u64 {
            anyhow::bail!("file is too large to display");
        }
        async_fs::read_to_string(file).await?
    };
    if base.contains('\0') || current.contains('\0') {
        anyhow::bail!("binary file; open it in an external editor");
    }
    if base
        .lines()
        .chain(current.lines())
        .any(|line| line.chars().count() > MAX_CHARACTERS_PER_LINE)
    {
        anyhow::bail!("line is too long to display; open it in an external editor");
    }
    Ok(Some((base, current)))
}

#[derive(Debug, Clone)]
pub enum GitReviewAction {
    Toggle(String),
    CopyPath(String),
}

impl TypedActionView for GitReviewView {
    type Action = GitReviewAction;
    fn handle_action(&mut self, action: &Self::Action, ctx: &mut ViewContext<Self>) {
        match action {
            GitReviewAction::CopyPath(path) => {
                ctx.clipboard()
                    .write(ClipboardContent::plain_text(path.clone()));
            }
            GitReviewAction::Toggle(path) => {
                if let Some(index) = self.files.iter().position(|card| card.entry.path == *path) {
                    let card = &mut self.files[index];
                    card.expanded = !card.expanded;
                    card.dirty = true;
                    card.base = None;
                    if !card.expanded
                        && let Some(editor) = card.editor.take()
                    {
                        card.editor_state = Some(editor.as_ref(ctx).review_state(ctx));
                    }
                    if !self.files[index].expanded
                        && self
                            .loading
                            .as_ref()
                            .is_some_and(|(loading, _)| loading == path)
                    {
                        self.cancel_loading();
                        self.files[index].dirty = true;
                    }
                    self.list.invalidate_height_for_index(index);
                    ctx.notify();
                }
            }
        }
    }
}

impl Entity for GitReviewView {
    type Event = ();
}
impl View for GitReviewView {
    fn ui_name() -> &'static str {
        "GitReview"
    }

    fn child_view_ids(&self, _app: &AppContext) -> Vec<EntityId> {
        self.files
            .iter()
            .filter_map(|card| card.editor.as_ref().map(ViewHandle::id))
            .collect()
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let font = appearance.ui_font_family();
        let branch = self
            .status
            .as_ref(app)
            .metadata(app)
            .map(|metadata| metadata.current_branch_name.as_str())
            .unwrap_or("");
        let mut header = Flex::row().with_cross_axis_alignment(CrossAxisAlignment::Center);
        header.add_child(
            Expanded::new(
                1.,
                Text::new_inline(self.root.display().to_string(), font, 12.)
                    .with_color(theme.sub_text_color(theme.background()).into())
                    .finish(),
            )
            .finish(),
        );
        header.add_child(
            Container::new(
                Text::new_inline(format!("{branch}  ·  {}", self.files.len()), font, 12.)
                    .with_color(theme.sub_text_color(theme.background()).into())
                    .finish(),
            )
            .with_margin_left(8.)
            .with_margin_right(8.)
            .finish(),
        );
        header.add_child(diff_badge(
            self.files.iter().map(|card| card.entry.additions).sum(),
            self.files.iter().map(|card| card.entry.deletions).sum(),
            appearance,
        ));
        header.add_child(
            close_button(appearance, self.close_mouse.clone())
                .build()
                .on_click(|ctx, _, _| {
                    ctx.dispatch_typed_action(WorkspaceAction::CloseGitReview);
                })
                .finish(),
        );
        let mut summary = Flex::row().with_cross_axis_alignment(CrossAxisAlignment::Center);
        summary.add_child(
            Shrinkable::new(
                1.,
                Text::new_inline(tr("⇄  Uncommitted changes"), font, 13.)
                    .with_color(theme.main_text_color(theme.background()).into())
                    .finish(),
            )
            .finish(),
        );
        let mut column = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(
                Container::new(header.finish())
                    .with_margin_bottom(8.)
                    .finish(),
            )
            .with_child(
                Container::new(summary.finish())
                    .with_margin_bottom(12.)
                    .finish(),
            );
        if !self.message.is_empty() {
            column.add_child(
                Text::new_inline(self.message.clone(), font, 13.)
                    .with_color(theme.sub_text_color(theme.background()).into())
                    .finish(),
            );
        }
        let list = NewScrollable::vertical(
            SingleAxisConfig::Manual {
                handle: self.scroll.clone(),
                child: NewScrollableElement::finish_scrollable(List::new(self.list.clone())),
            },
            theme.disabled_text_color(theme.background()).into(),
            theme.main_text_color(theme.background()).into(),
            warpui::elements::Fill::None,
        )
        .finish();
        column.add_child(Shrinkable::new(1., list).finish());
        Container::new(column.finish())
            .with_padding(Padding::uniform(16.))
            .finish()
    }
}

impl Drop for GitReviewView {
    fn drop(&mut self) {
        self.cancel_loading();
    }
}
