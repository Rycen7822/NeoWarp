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
use warpui::ui_components::components::{Coords, UiComponent, UiComponentStyles};
use warpui::units::Pixels;
use warpui::{
    AppContext, Element, Entity, EntityId, ModelHandle, SingletonEntity, TypedActionView, View,
    ViewContext, ViewHandle,
};

use super::diff_selector::DiffSelector;
use super::diff_size_limits::{MAX_CHARACTERS_PER_LINE, MAX_DIFF_SIZE};
use super::diff_state::{DiffMode, GitFileStatus, LocalDiffStateModel};
use super::git_repo_model::{GitRepoStatusEvent, GitRepoStatusModel};
use crate::appearance::Appearance;
use crate::code::editor::view::{
    CodeEditorEvent, CodeEditorRenderOptions, CodeEditorView, ReviewEditorState,
};
use crate::code::editor::{add_color, remove_color};
use crate::localization::tr;
use crate::settings::CodeSettings;
use crate::ui_components::buttons::{close_button, icon_button};
use crate::ui_components::icons::Icon;
use crate::util::git::FileChangeEntry;
use crate::workspace::WorkspaceAction;

#[cfg(test)]
#[path = "review_tests.rs"]
mod tests;

struct FileCard {
    entry: FileChangeEntry,
    editor: Option<ViewHandle<CodeEditorView>>,
    editor_state: Option<ReviewEditorState>,
    base: Option<Arc<String>>,
    expanded: bool,
    auto_collapse_pending: bool,
    dirty: bool,
    message: String,
    toggle_mouse: MouseStateHandle,
    copy_mouse: MouseStateHandle,
    open_mouse: MouseStateHandle,
}

impl FileCard {
    fn new(entry: FileChangeEntry, collapse_limit: u16) -> Self {
        Self {
            entry,
            editor: None,
            editor_state: None,
            base: None,
            expanded: collapse_limit != 0,
            auto_collapse_pending: true,
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
    selector: ViewHandle<DiffSelector>,
    mode: DiffMode,
    merge_base: Option<String>,
    refreshing: Option<SpawnedFutureHandle>,
    refresh_pending: bool,
    files: Vec<FileCard>,
    message: String,
    loading: Option<(String, SpawnedFutureHandle)>,
    list: ListState<()>,
    scroll: ScrollStateHandle,
    close_mouse: MouseStateHandle,
    wrap_lines: bool,
    refresh_mouse: MouseStateHandle,
    wrap_mouse: MouseStateHandle,
    fold_mouse: MouseStateHandle,
}

impl GitReviewView {
    pub fn new(
        root: PathBuf,
        status: ModelHandle<GitRepoStatusModel>,
        ctx: &mut ViewContext<Self>,
    ) -> Self {
        let selector =
            ctx.add_typed_action_view(|ctx| DiffSelector::new(root.clone(), status.clone(), ctx));
        ctx.subscribe_to_view(&selector, |me, _, mode, ctx| {
            me.select_mode(mode.clone(), ctx);
        });
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
                if me.refreshing.is_some() {
                    me.refresh_pending = true;
                }
                if changes.git_changed {
                    me.merge_base = None;
                }
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
            selector,
            mode: DiffMode::Head,
            merge_base: None,
            refreshing: None,
            refresh_pending: false,
            files: Vec::new(),
            message: tr("Loading changes…").into(),
            loading: None,
            list,
            scroll: Default::default(),
            close_mouse: Default::default(),
            wrap_lines: false,
            refresh_mouse: Default::default(),
            wrap_mouse: Default::default(),
            fold_mouse: Default::default(),
        };
        view.update_files(ctx);
        view
    }

    fn update_files(&mut self, ctx: &mut ViewContext<Self>) {
        if self.mode != DiffMode::Head {
            self.refresh_comparison(ctx);
            return;
        }
        if self.status.as_ref(ctx).metadata(ctx).is_none() {
            return;
        }
        let files = self
            .status
            .as_ref(ctx)
            .files(ctx)
            .unwrap_or_default()
            .to_vec();
        self.set_files(files, ctx);
    }

    fn set_files(&mut self, files: Vec<FileChangeEntry>, ctx: &mut ViewContext<Self>) {
        let collapse_limit = *CodeSettings::as_ref(ctx).diff_auto_collapse_lines;
        let old_len = self.files.len();
        let mut previous: HashMap<_, _> = self
            .files
            .drain(..)
            .map(|mut card| (std::mem::take(&mut card.entry.path), card))
            .collect();
        self.files = files
            .into_iter()
            .map(|entry| {
                if let Some(mut card) = previous.remove(&entry.path) {
                    card.entry = entry;
                    card
                } else {
                    FileCard::new(entry, collapse_limit)
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
            tr(if self.mode == DiffMode::Head {
                "No uncommitted changes"
            } else {
                "No changes against the selected branch"
            })
            .into()
        } else {
            String::new()
        };
        ctx.notify();
    }

    fn select_mode(&mut self, mode: DiffMode, ctx: &mut ViewContext<Self>) {
        if self.mode == mode {
            return;
        }
        if let Some(task) = self.refreshing.take() {
            task.abort();
        }
        self.refresh_pending = false;
        self.cancel_loading();
        self.mode = mode;
        self.selector.update(ctx, |selector, ctx| {
            selector.set_mode(self.mode.clone(), ctx)
        });
        self.merge_base = None;
        self.set_files(Vec::new(), ctx);
        self.message = tr("Loading changes…").into();
        self.update_files(ctx);
    }

    fn refresh_comparison(&mut self, ctx: &mut ViewContext<Self>) {
        if self.refreshing.is_some() {
            self.refresh_pending = true;
            return;
        }
        let root = self.root.clone();
        let mode = self.mode.clone();
        let base = self.merge_base.clone();
        self.refreshing = Some(ctx.spawn(
            async move { load_comparison(root, mode, base).await },
            |me, result, ctx| {
                me.refreshing = None;
                // An invalidation during the read supersedes this summary too.
                if std::mem::take(&mut me.refresh_pending) {
                    me.refresh_comparison(ctx);
                    return;
                }
                match result {
                    Ok((base, files)) => {
                        if me.merge_base.as_ref() != Some(&base) {
                            me.cancel_loading();
                            for card in &mut me.files {
                                card.base = None;
                                card.dirty = true;
                            }
                        }
                        me.merge_base = Some(base);
                        me.set_files(files, ctx);
                    }
                    Err(error) => {
                        me.merge_base = None;
                        me.set_files(Vec::new(), ctx);
                        me.message = format!("{}: {error:#}", tr("Unable to compare branches"));
                        ctx.notify();
                    }
                }
            },
        ));
    }

    fn cancel_loading(&mut self) {
        if let Some((_, task)) = self.loading.take() {
            task.abort();
        }
    }

    fn load_visible(&mut self, index: usize, ctx: &mut ViewContext<Self>) {
        // The list may measure preceding items to estimate height; those are not load requests.
        if self.loading.is_some()
            || (self.mode != DiffMode::Head && self.merge_base.is_none())
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
        let mode = self.mode.clone();
        let revision = self.merge_base.clone();
        self.loading = Some((
            path.clone(),
            ctx.spawn(
                async move { load_file(root, path, mode, revision, base).await },
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
                                            && std::mem::take(
                                                &mut me.files[index].auto_collapse_pending,
                                            )
                                        {
                                            let model = editor.as_ref(ctx).model.as_ref(ctx);
                                            let lines = model
                                                .diff()
                                                .as_ref(ctx)
                                                .review_line_count(model.line_count(ctx) + 1);
                                            let limit =
                                                *CodeSettings::as_ref(ctx).diff_auto_collapse_lines;
                                            if limit == 0 || lines > usize::from(limit) {
                                                me.set_expanded(index, false, ctx);
                                            }
                                        }
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
                                        &crate::util::path::local_path_for_io(&me.root)
                                            .join(&result_path),
                                        ctx,
                                    );
                                });
                                me.files[index].editor = Some(editor.clone());
                                editor
                            };
                            editor.update(ctx, |editor, ctx| {
                                editor.set_review_content(&base, &current, ctx);
                                editor.set_review_wrapping(me.wrap_lines, ctx);
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

    fn set_expanded(&mut self, index: usize, expanded: bool, ctx: &mut ViewContext<Self>) {
        let card = &mut self.files[index];
        card.auto_collapse_pending = false;
        if card.expanded == expanded {
            return;
        }
        card.expanded = expanded;
        card.dirty = true;
        card.base = None;
        if !expanded && let Some(editor) = card.editor.take() {
            card.editor_state = Some(editor.as_ref(ctx).review_state(ctx));
        }
        if !expanded
            && self
                .loading
                .as_ref()
                .is_some_and(|(path, _)| *path == card.entry.path)
        {
            self.cancel_loading();
        }
        self.list.invalidate_height_for_index(index);
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
        let path = crate::util::path::local_path_for_io(&self.root).join(&card.entry.path);
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
            if let Some(editor) = &card.editor
                && !card.auto_collapse_pending
            {
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

pub(crate) fn diff_badge(
    additions: usize,
    deletions: usize,
    appearance: &Appearance,
) -> Box<dyn Element> {
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

async fn load_comparison(
    root: PathBuf,
    mode: DiffMode,
    cached_base: Option<String>,
) -> Result<(String, Vec<FileChangeEntry>)> {
    let base = match cached_base {
        Some(base) => base,
        None => LocalDiffStateModel::compute_merge_base(&root, &mode).await?,
    };
    let metadata = LocalDiffStateModel::diff_metadata_against_base(&root, &base).await?;
    Ok((base, metadata.files))
}

async fn load_file(
    root: PathBuf,
    path: String,
    mode: DiffMode,
    revision: Option<String>,
    cached_base: Option<Arc<String>>,
) -> Result<Option<(Arc<String>, String)>> {
    // The editor already computes and renders the diff. Querying a Git patch
    // and its binary numstat first repeats that work and adds WSL launches.
    let Some((_, status)) =
        LocalDiffStateModel::file_status_for_path(&root, &path, &mode, revision.as_deref()).await?
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
                warp_util::git::read_file_at_revision(
                    &root,
                    revision.as_deref().unwrap_or("HEAD"),
                    base_path,
                    MAX_DIFF_SIZE,
                )
                .await
                .context("cannot read comparison base content")?,
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
        let file = crate::util::path::local_path_for_io(&root).join(path);
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
    Refresh,
    ToggleWrapping,
    ToggleAll,
}

impl TypedActionView for GitReviewView {
    type Action = GitReviewAction;
    fn handle_action(&mut self, action: &Self::Action, ctx: &mut ViewContext<Self>) {
        match action {
            GitReviewAction::Refresh => {
                self.status
                    .update(ctx, |status, ctx| status.refresh_metadata(ctx));
            }
            GitReviewAction::ToggleWrapping => {
                self.wrap_lines = !self.wrap_lines;
                for (index, card) in self.files.iter().enumerate() {
                    if let Some(editor) = &card.editor {
                        editor.update(ctx, |editor, ctx| {
                            editor.set_review_wrapping(self.wrap_lines, ctx)
                        });
                    }
                    self.list.invalidate_height_for_index(index);
                }
                ctx.notify();
            }
            GitReviewAction::ToggleAll => {
                let expanded = !self.files.iter().any(|card| card.expanded);
                for index in 0..self.files.len() {
                    self.set_expanded(index, expanded, ctx);
                }
                ctx.notify();
            }
            GitReviewAction::CopyPath(path) => {
                ctx.clipboard()
                    .write(ClipboardContent::plain_text(path.clone()));
            }
            GitReviewAction::Toggle(path) => {
                if let Some(index) = self.files.iter().position(|card| card.entry.path == *path) {
                    self.set_expanded(index, !self.files[index].expanded, ctx);
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
            .chain(std::iter::once(self.selector.id()))
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
        let ready = self.mode == DiffMode::Head || self.merge_base.is_some();
        let mut header = Flex::row().with_cross_axis_alignment(CrossAxisAlignment::Center);
        header.add_child(
            Expanded::new(
                1.,
                Text::new_inline(crate::util::path::display_local_path(&self.root), font, 12.)
                    .with_color(theme.sub_text_color(theme.background()).into())
                    .finish(),
            )
            .finish(),
        );
        header.add_child(
            Container::new(
                Text::new_inline(
                    if ready {
                        format!("{branch}  ·  {}", self.files.len())
                    } else {
                        branch.to_owned()
                    },
                    font,
                    12.,
                )
                .with_color(theme.sub_text_color(theme.background()).into())
                .finish(),
            )
            .with_margin_left(8.)
            .with_margin_right(8.)
            .finish(),
        );
        if ready {
            header.add_child(diff_badge(
                self.files.iter().map(|card| card.entry.additions).sum(),
                self.files.iter().map(|card| card.entry.deletions).sum(),
                appearance,
            ));
        }
        header.add_child(
            close_button(appearance, self.close_mouse.clone())
                .build()
                .on_click(|ctx, _, _| {
                    ctx.dispatch_typed_action(WorkspaceAction::CloseGitReview);
                })
                .finish(),
        );
        let mut summary = Flex::row().with_cross_axis_alignment(CrossAxisAlignment::Center);
        summary.add_child(ChildView::new(&self.selector).finish());
        summary.add_child(Expanded::new(1., Empty::new().finish()).finish());
        let any_expanded = self.files.iter().any(|card| card.expanded);
        for (icon, active, mouse, action, label) in [
            (
                Icon::Refresh,
                false,
                &self.refresh_mouse,
                GitReviewAction::Refresh,
                "Refresh changes",
            ),
            (
                Icon::TextWrap,
                self.wrap_lines,
                &self.wrap_mouse,
                GitReviewAction::ToggleWrapping,
                if self.wrap_lines {
                    "Disable word wrap"
                } else {
                    "Enable word wrap"
                },
            ),
            (
                if any_expanded {
                    Icon::CollapseAll
                } else {
                    Icon::ExpandAll
                },
                false,
                &self.fold_mouse,
                GitReviewAction::ToggleAll,
                if any_expanded {
                    "Collapse all diffs"
                } else {
                    "Expand all diffs"
                },
            ),
        ] {
            let builder = appearance.ui_builder().clone();
            summary.add_child(
                icon_button(appearance, icon, active, mouse.clone())
                    .with_style(
                        UiComponentStyles::default()
                            .set_width(28.)
                            .set_height(28.)
                            .set_padding(Coords::uniform(7.))
                            .set_border_width(0.)
                            .set_font_color(theme.sub_text_color(theme.background()).into()),
                    )
                    .with_hovered_styles(
                        UiComponentStyles::default().set_background(theme.surface_2().into()),
                    )
                    .with_active_styles(
                        UiComponentStyles::default().set_background(theme.surface_2().into()),
                    )
                    .with_clicked_styles(
                        UiComponentStyles::default().set_background(theme.surface_3().into()),
                    )
                    .with_tooltip(Box::new(move || {
                        builder.tool_tip(tr(label).to_owned()).build().finish()
                    }))
                    .build()
                    .on_click(move |ctx, _, _| ctx.dispatch_typed_action(action.clone()))
                    .finish(),
            );
        }
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
        if let Some(task) = self.refreshing.take() {
            task.abort();
        }
    }
}
