//! A lazy directory sidebar for the active local or WSL terminal.

pub(crate) mod operation;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use futures_lite::StreamExt;
use notify_debouncer_full::notify::{RecursiveMode, WatchFilter};
use pathfinder_geometry::vector::Vector2F;
use warp_core::ui::theme::color::internal_colors;
use warpui::r#async::{SpawnedFutureHandle, Timer};
use warpui::elements::new_scrollable::{NewScrollableElement, SingleAxisConfig};
use warpui::elements::{
    ChildAnchor, ChildView, ConstrainedBox, Container, CornerRadius, CrossAxisAlignment, Empty,
    Flex, Hoverable, List, ListState, MainAxisSize, MouseStateHandle, NewScrollable,
    OffsetPositioning, ParentAnchor, ParentElement, ParentOffsetBounds, Radius, ScrollStateHandle,
    Shrinkable, Stack, Text,
};
use warpui::fonts::{Properties, Weight};
use warpui::text_layout::ClipConfig;
use warpui::{
    AppContext, Element, Entity, ModelHandle, SingletonEntity, TypedActionView, View, ViewContext,
    ViewHandle,
};
use watcher::{BulkFilesystemWatcher, BulkFilesystemWatcherEvent};

use crate::appearance::Appearance;
#[cfg(windows)]
use crate::file_watcher::wsl;
use crate::localization::tr;
use crate::menu::{Event as MenuEvent, Menu, MenuItem, MenuItemFields};
use crate::ui_components::icons::Icon;
use crate::util::file::external_editor::{self, Editor, SUPPORTED_EDITORS};
use crate::workspace::ActiveSession;

struct Entry {
    path: PathBuf,
    is_dir: bool,
}

#[derive(Default)]
struct Directory {
    generation: u64,
    entries: Vec<Entry>,
    loading: Option<SpawnedFutureHandle>,
    dirty: bool,
    error: Option<String>,
    poll: bool,
    #[cfg(windows)]
    subscription: Option<wsl::Subscription>,
}

struct Row {
    path: PathBuf,
    depth: usize,
    is_dir: bool,
    mouse: MouseStateHandle,
}

pub(crate) struct FileBrowserView {
    root: Option<PathBuf>,
    generation: u64,
    directories: HashMap<PathBuf, Directory>,
    rows: Vec<Row>,
    selected: Option<PathBuf>,
    // Native roots share one watcher; changing the root resets it.
    watcher: Option<ModelHandle<BulkFilesystemWatcher>>,
    poll: Option<SpawnedFutureHandle>,
    list: ListState<()>,
    scroll: ScrollStateHandle,
    header: crate::workspace::sidebar::HeaderState,
    menu: ViewHandle<Menu<FileBrowserAction>>,
    menu_position: Option<Vector2F>,
}

pub(crate) enum FileBrowserEvent {
    Open(PathBuf),
    Preview(PathBuf),
    Edit(PathBuf, operation::Operation),
}

#[derive(Debug, Clone)]
pub(crate) enum FileBrowserAction {
    Select(PathBuf),
    ShowMenu(PathBuf, Vector2F),
    Preview(PathBuf),
    OpenExternal(PathBuf, Option<Editor>),
    Edit(PathBuf, operation::Operation),
    CopyPath(PathBuf),
    Refresh(PathBuf),
}

impl FileBrowserView {
    pub(crate) fn new(ctx: &mut ViewContext<Self>) -> Self {
        let weak = ctx.handle();
        let list = ListState::new(move |index, _, app| {
            let Some(view) = weak.upgrade(app) else {
                return Empty::new().finish();
            };
            view.as_ref(app).render_row(index, app)
        });
        ctx.observe(&ActiveSession::handle(ctx), |me, _, ctx| me.sync_root(ctx));
        let menu = ctx.add_typed_action_view(|_| Menu::new().with_drop_shadow());
        ctx.subscribe_to_view(&menu, |me, _, event, ctx| {
            if let MenuEvent::Close { .. } = event {
                me.menu_position = None;
                ctx.notify();
            }
        });
        let mut view = Self {
            root: None,
            generation: 0,
            directories: HashMap::new(),
            rows: Vec::new(),
            selected: None,
            watcher: None,
            poll: None,
            list,
            scroll: Default::default(),
            header: Default::default(),
            menu,
            menu_position: None,
        };
        view.sync_root(ctx);
        view
    }

    fn sync_root(&mut self, ctx: &mut ViewContext<Self>) {
        let root = ActiveSession::as_ref(ctx)
            .path_if_local(ctx.window_id())
            .map(Path::to_path_buf);
        if self.root == root {
            return;
        }
        self.cancel_reads();
        self.directories.clear();
        self.watcher = None;
        self.root = root;
        self.selected = None;
        self.menu_position = None;
        self.list.scroll_to(0);
        self.scroll = Default::default();
        if let Some(root) = &self.root {
            self.expand(root.clone(), ctx);
        }
        self.update_rows(ctx);
    }

    fn expand(&mut self, path: PathBuf, ctx: &mut ViewContext<Self>) {
        self.generation += 1;
        self.directories.insert(
            path.clone(),
            Directory {
                generation: self.generation,
                ..Default::default()
            },
        );
        #[cfg(windows)]
        if let Some((subscription, events)) =
            wsl::subscribe(&path, wsl::WatchScope::Directory, ctx.background_executor())
        {
            let directory = self
                .directories
                .get_mut(&path)
                .expect("directory just inserted");
            directory.subscription = Some(subscription);
            directory.poll = true;
            let changed = path.clone();
            let closed = path.clone();
            let generation = self.generation;
            ctx.spawn_stream_local(
                events,
                move |me, _, ctx| {
                    let Some(directory) = me.directories.get_mut(&changed) else {
                        return;
                    };
                    if directory.generation != generation {
                        return;
                    }
                    directory.poll = false;
                    if let Some(subscription) = &directory.subscription {
                        subscription.take_changes();
                    }
                    me.refresh(&changed, ctx);
                },
                move |me, ctx| {
                    if let Some(directory) = me.directories.get_mut(&closed)
                        && directory.generation == generation
                    {
                        directory.subscription = None;
                        directory.poll = true;
                        me.schedule_poll(ctx);
                    }
                },
            );
            self.schedule_poll(ctx);
            self.refresh(&path, ctx);
            return;
        }

        if self.watcher.is_none() {
            let watcher =
                ctx.add_model(|ctx| BulkFilesystemWatcher::new(Duration::from_millis(150), ctx));
            ctx.subscribe_to_model(&watcher, |me, _, event, ctx| me.native_changes(event, ctx));
            self.watcher = Some(watcher);
        }
        let registration =
            self.watcher
                .as_ref()
                .expect("watcher initialized")
                .update(ctx, |watcher, _| {
                    watcher.register_path(
                        &path,
                        WatchFilter::accept_all(),
                        RecursiveMode::NonRecursive,
                    )
                });
        let watched = path.clone();
        let generation = self.generation;
        ctx.spawn(registration, move |me, result, ctx| {
            if let Err(error) = result {
                log::warn!("Directory sidebar watcher unavailable: {error:#}");
                if let Some(directory) = me.directories.get_mut(&watched)
                    && directory.generation == generation
                {
                    directory.poll = true;
                    me.schedule_poll(ctx);
                }
            }
        });
        self.refresh(&path, ctx);
    }

    fn native_changes(&mut self, event: &BulkFilesystemWatcherEvent, ctx: &mut ViewContext<Self>) {
        let affected: HashSet<_> = event
            .added
            .iter()
            .chain(&event.modified)
            .chain(&event.deleted)
            .chain(event.moved.keys())
            .chain(event.moved.values())
            .flat_map(|path| [Some(path.as_path()), path.parent()])
            .flatten()
            .filter(|path| self.directories.contains_key(*path))
            .map(Path::to_path_buf)
            .collect();
        for path in affected {
            self.refresh(&path, ctx);
        }
    }

    fn schedule_poll(&mut self, ctx: &mut ViewContext<Self>) {
        if self.poll.is_some() || !self.directories.values().any(|directory| directory.poll) {
            return;
        }
        self.poll = Some(
            ctx.spawn(Timer::after(Duration::from_secs(2)), |me, _, ctx| {
                me.poll = None;
                let paths: Vec<_> = me
                    .directories
                    .iter()
                    .filter(|(_, directory)| directory.poll)
                    .map(|(path, _)| path.clone())
                    .collect();
                for path in paths {
                    me.refresh(&path, ctx);
                }
                me.schedule_poll(ctx);
            }),
        );
    }

    pub(crate) fn refresh(&mut self, path: &Path, ctx: &mut ViewContext<Self>) {
        let Some(directory) = self.directories.get_mut(path) else {
            return;
        };
        if directory.loading.is_some() {
            directory.dirty = true;
            return;
        }
        let generation = directory.generation;
        let path = path.to_path_buf();
        let read_path = path.clone();
        directory.loading = Some(
            ctx.spawn(read_directory(read_path), move |me, result, ctx| {
                let Some(directory) = me.directories.get_mut(&path) else {
                    return;
                };
                if directory.generation != generation {
                    return;
                }
                directory.loading = None;
                let again = std::mem::take(&mut directory.dirty);
                match result {
                    Ok(entries) => {
                        directory.entries = entries;
                        directory.error = None;
                    }
                    Err(error) => {
                        directory.entries.clear();
                        directory.error = Some(error.to_string());
                        directory.poll = true;
                    }
                }
                // Removed or renamed descendants must release their subscriptions too.
                let stale: Vec<_> = me
                    .directories
                    .keys()
                    .filter(|child| {
                        **child != path && child.starts_with(&path) && !me.is_reachable(child)
                    })
                    .cloned()
                    .collect();
                for child in stale {
                    me.collapse(&child, ctx);
                }
                me.update_rows(ctx);
                me.schedule_poll(ctx);
                if again {
                    me.refresh(&path, ctx);
                }
            }),
        );
    }

    fn is_reachable(&self, path: &Path) -> bool {
        if self.root.as_deref() == Some(path) {
            return true;
        }
        path.parent()
            .and_then(|parent| self.directories.get(parent))
            .is_some_and(|parent| {
                parent
                    .entries
                    .iter()
                    .any(|entry| entry.is_dir && entry.path == path)
            })
    }

    fn collapse(&mut self, path: &Path, ctx: &mut ViewContext<Self>) {
        let removed: Vec<_> = self
            .directories
            .keys()
            .filter(|p| p.starts_with(path))
            .cloned()
            .collect();
        for path in removed {
            if let Some(mut directory) = self.directories.remove(&path) {
                if let Some(loading) = directory.loading.take() {
                    loading.abort();
                }
                if let Some(watcher) = &self.watcher {
                    std::mem::drop(
                        watcher.update(ctx, |watcher, _| watcher.unregister_path(&path)),
                    );
                }
            }
        }
    }

    fn update_rows(&mut self, ctx: &mut ViewContext<Self>) {
        fn collect(
            path: &Path,
            depth: usize,
            directories: &HashMap<PathBuf, Directory>,
            rows: &mut Vec<Row>,
        ) {
            if let Some(directory) = directories.get(path) {
                for entry in &directory.entries {
                    rows.push(Row {
                        path: entry.path.clone(),
                        depth,
                        is_dir: entry.is_dir,
                        mouse: Default::default(),
                    });
                    if entry.is_dir {
                        collect(&entry.path, depth + 1, directories, rows);
                    }
                }
            }
        }
        let old_count = self.rows.len();
        self.rows.clear();
        if let Some(root) = &self.root {
            self.rows.push(Row {
                path: root.clone(),
                depth: 0,
                is_dir: true,
                mouse: Default::default(),
            });
            collect(root, 1, &self.directories, &mut self.rows);
        }
        for _ in old_count..self.rows.len() {
            self.list.add_item();
        }
        for index in (self.rows.len()..old_count).rev() {
            self.list.remove(index);
        }
        // Heights stay fixed; List rebuilds visible elements on every layout.
        ctx.notify();
    }

    fn cancel_reads(&mut self) {
        if let Some(poll) = self.poll.take() {
            poll.abort();
        }
        for directory in self.directories.values_mut() {
            if let Some(loading) = directory.loading.take() {
                loading.abort();
            }
        }
    }

    fn show_menu(&mut self, path: &Path, position: Vector2F, ctx: &mut ViewContext<Self>) {
        let Some(row) = self.rows.iter().find(|row| row.path == path) else {
            return;
        };
        let is_dir = row.is_dir;
        self.selected = Some(path.to_path_buf());
        let item = |label: &str, action| {
            MenuItemFields::new(label)
                .with_on_select_action(action)
                .into_item()
        };
        let mut items = Vec::new();
        if !is_dir {
            items.push(item(
                tr("Open in right pane"),
                FileBrowserAction::Preview(path.into()),
            ));
        }
        items.push(item(
            tr("Open with system default app"),
            FileBrowserAction::OpenExternal(path.into(), None),
        ));
        if !is_dir {
            for &editor in SUPPORTED_EDITORS {
                if editor.is_installed(ctx) {
                    items.push(item(
                        &format!("{} {editor}", tr("Open in")),
                        FileBrowserAction::OpenExternal(path.into(), Some(editor)),
                    ));
                }
            }
        }
        items.push(MenuItem::Separator);
        if is_dir {
            for operation in [
                operation::Operation::NewFile,
                operation::Operation::NewFolder,
            ] {
                items.push(item(
                    tr(operation.title()),
                    FileBrowserAction::Edit(path.into(), operation),
                ));
            }
        }
        if self.root.as_deref() != Some(path) {
            for operation in [operation::Operation::Rename, operation::Operation::Delete] {
                items.push(item(
                    tr(operation.title()),
                    FileBrowserAction::Edit(path.into(), operation),
                ));
            }
        }
        items.push(item(
            tr("Copy path"),
            FileBrowserAction::CopyPath(path.into()),
        ));
        items.push(item(
            tr("Refresh"),
            FileBrowserAction::Refresh(if is_dir {
                path.into()
            } else {
                path.parent().unwrap_or(path).into()
            }),
        ));
        self.menu
            .update(ctx, |menu, ctx| menu.set_items(items, ctx));
        self.menu_position = Some(position);
        ctx.focus(&self.menu);
        ctx.notify();
    }

    fn render_row(&self, index: usize, app: &AppContext) -> Box<dyn Element> {
        let Some(row) = self.rows.get(index) else {
            return Empty::new().finish();
        };
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let expanded = self.directories.contains_key(&row.path);
        let is_root = self.root.as_ref() == Some(&row.path);
        let selected =
            self.selected.as_ref() == Some(&row.path) || (is_root && self.selected.is_none());
        let content = Hoverable::new(row.mouse.clone(), |mouse| {
            let color = if selected {
                theme.foreground()
            } else if mouse.is_hovered() {
                theme.main_text_color(theme.background())
            } else {
                theme.sub_text_color(theme.background())
            };
            let chevron = if row.is_dir {
                if expanded {
                    Icon::ChevronDown
                } else {
                    Icon::ChevronRight
                }
                .to_warpui_icon(color)
                .finish()
            } else {
                Empty::new().finish()
            };
            let icon = if row.is_dir {
                Icon::Folder.to_warpui_icon(color).finish()
            } else {
                crate::code::icon_from_file_path(&row.path.to_string_lossy(), appearance)
                    .unwrap_or_else(|| Icon::File.to_warpui_icon(color).finish())
            };
            let name = row
                .path
                .file_name()
                .unwrap_or(row.path.as_os_str())
                .to_string_lossy();
            let content = Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(
                    Container::new(
                        ConstrainedBox::new(chevron)
                            .with_width(16.)
                            .with_height(16.)
                            .finish(),
                    )
                    .with_margin_right(4.)
                    .finish(),
                )
                .with_child(
                    Container::new(
                        ConstrainedBox::new(icon)
                            .with_width(16.)
                            .with_height(16.)
                            .finish(),
                    )
                    .with_margin_right(8.)
                    .finish(),
                )
                .with_child(
                    Shrinkable::new(
                        1.,
                        Text::new_inline(
                            name.into_owned(),
                            appearance.ui_font_family(),
                            appearance.scaled_ui_size(14.),
                        )
                        .with_clip(ClipConfig::ellipsis())
                        .with_color(color.into())
                        .with_style(Properties::default().weight(if is_root {
                            Weight::Bold
                        } else {
                            Weight::Normal
                        }))
                        .finish(),
                    )
                    .finish(),
                );
            let mut container = Container::new(content.finish())
                .with_padding_left(8. + row.depth as f32 * 16.)
                .with_padding_right(8.)
                .with_vertical_padding(4.);
            if selected || mouse.is_hovered() {
                container = container
                    .with_background(if selected {
                        internal_colors::fg_overlay_4(theme)
                    } else {
                        internal_colors::fg_overlay_2(theme)
                    })
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.)));
            }
            ConstrainedBox::new(container.finish())
                .with_height(appearance.scaled_ui_size(26.))
                .finish()
        });
        let path = row.path.clone();
        let menu_path = path.clone();
        content
            .on_mouse_down(move |ctx, _, _| {
                ctx.dispatch_typed_action(FileBrowserAction::Select(path.clone()));
            })
            .on_right_click(move |ctx, _, position| {
                ctx.dispatch_typed_action(FileBrowserAction::ShowMenu(menu_path.clone(), position));
            })
            .finish()
    }
}

async fn read_directory(path: PathBuf) -> std::io::Result<Vec<Entry>> {
    let mut reader = async_fs::read_dir(path).await?;
    let mut entries = Vec::new();
    while let Some(entry) = reader.next().await {
        let entry = entry?;
        let kind = match entry.file_type().await {
            Ok(kind) => kind,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        let is_dir = kind.is_dir()
            || (kind.is_symlink()
                && async_fs::metadata(entry.path())
                    .await
                    .is_ok_and(|m| m.is_dir()));
        entries.push(Entry {
            path: entry.path(),
            is_dir,
        });
    }
    entries.sort_by_cached_key(|entry| {
        (
            !entry.is_dir,
            entry
                .path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_lowercase(),
        )
    });
    Ok(entries)
}

impl Entity for FileBrowserView {
    type Event = FileBrowserEvent;
}

impl TypedActionView for FileBrowserView {
    type Action = FileBrowserAction;
    fn handle_action(&mut self, action: &Self::Action, ctx: &mut ViewContext<Self>) {
        let path = match action {
            FileBrowserAction::Select(path) => path,
            FileBrowserAction::ShowMenu(path, position) => {
                self.show_menu(path, *position, ctx);
                return;
            }
            FileBrowserAction::Preview(path) => {
                ctx.emit(FileBrowserEvent::Preview(path.clone()));
                return;
            }
            FileBrowserAction::OpenExternal(path, editor) => {
                if let Some(editor) = editor {
                    external_editor::open_file_path_with_editor(
                        None,
                        path.clone(),
                        Some(*editor),
                        ctx,
                    );
                } else {
                    ctx.open_file_path(path);
                }
                return;
            }
            FileBrowserAction::Edit(path, operation) => {
                if self.root.as_ref() != Some(path)
                    || matches!(
                        operation,
                        operation::Operation::NewFile | operation::Operation::NewFolder
                    )
                {
                    ctx.emit(FileBrowserEvent::Edit(path.clone(), *operation));
                }
                return;
            }
            FileBrowserAction::CopyPath(path) => {
                ctx.clipboard()
                    .write(warpui::clipboard::ClipboardContent::plain_text(
                        path.to_string_lossy().into_owned(),
                    ));
                return;
            }
            FileBrowserAction::Refresh(path) => {
                self.refresh(path, ctx);
                return;
            }
        };
        let is_dir = self.rows.iter().any(|row| row.path == *path && row.is_dir);
        if is_dir {
            self.selected = Some(path.clone());
            if self.directories.contains_key(path) {
                self.collapse(path, ctx);
            } else {
                self.expand(path.clone(), ctx);
            }
            self.update_rows(ctx);
        } else {
            self.selected = Some(path.clone());
            ctx.emit(FileBrowserEvent::Open(path.clone()));
            ctx.notify();
        }
    }
}

impl View for FileBrowserView {
    fn ui_name() -> &'static str {
        "FileBrowser"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let header = crate::workspace::sidebar::header(
            crate::workspace::SidebarTab::Files,
            &self.header,
            app,
        );
        let mut column = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(header);
        let message = match &self.root {
            None => Some("Select a local or WSL terminal folder"),
            Some(root) => self
                .directories
                .get(root)
                .and_then(|directory| directory.error.as_deref()),
        };
        if let Some(message) = message {
            column.add_child(
                Text::new_inline(
                    tr(message).to_owned(),
                    appearance.ui_font_family(),
                    appearance.scaled_ui_size(12.),
                )
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
        let content = Container::new(column.finish())
            .with_uniform_padding(8.)
            .finish();
        let mut stack = Stack::new().with_child(content);
        if let Some(position) = self.menu_position {
            stack.add_positioned_overlay_child(
                ChildView::new(&self.menu).finish(),
                OffsetPositioning::offset_from_parent(
                    position,
                    ParentOffsetBounds::WindowByPosition,
                    ParentAnchor::TopLeft,
                    ChildAnchor::TopLeft,
                ),
            );
        }
        stack.finish()
    }
}

impl Drop for FileBrowserView {
    fn drop(&mut self) {
        self.cancel_reads();
    }
}
