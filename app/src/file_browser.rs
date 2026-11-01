//! A lazy directory sidebar for the active local or WSL terminal.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use futures_lite::StreamExt;
use notify_debouncer_full::notify::{RecursiveMode, WatchFilter};
#[cfg(windows)]
use warp_files::FileModel;
use warpui::r#async::{SpawnedFutureHandle, Timer};
use warpui::elements::new_scrollable::{NewScrollableElement, SingleAxisConfig};
use warpui::elements::{
    ConstrainedBox, Container, CrossAxisAlignment, DispatchEventResult, Empty, EventHandler,
    Expanded, Flex, List, ListState, MouseStateHandle, NewScrollable, Padding, ParentElement,
    ScrollStateHandle, Shrinkable, Text,
};
use warpui::ui_components::components::UiComponent;
use warpui::{
    AppContext, Element, Entity, ModelHandle, SingletonEntity, TypedActionView, View, ViewContext,
};
use watcher::{BulkFilesystemWatcher, BulkFilesystemWatcherEvent};

use crate::appearance::Appearance;
#[cfg(windows)]
use crate::file_watcher::wsl;
use crate::ui_components::buttons::close_button;
use crate::ui_components::icons::Icon;
use crate::workspace::{ActiveSession, WorkspaceAction};

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
    native: bool,
    poll: bool,
    #[cfg(windows)]
    subscription: Option<wsl::Subscription>,
}

struct Row {
    path: PathBuf,
    depth: usize,
    is_dir: bool,
}

pub(crate) struct FileBrowserView {
    root: Option<PathBuf>,
    generation: u64,
    directories: HashMap<PathBuf, Directory>,
    rows: Vec<Row>,
    selected: Option<PathBuf>,
    watcher: Option<ModelHandle<BulkFilesystemWatcher>>,
    poll: Option<SpawnedFutureHandle>,
    list: ListState<()>,
    scroll: ScrollStateHandle,
    close_mouse: MouseStateHandle,
}

#[derive(Debug, Clone)]
pub(crate) enum FileBrowserAction {
    Select(PathBuf),
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
            close_mouse: Default::default(),
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
                        let changes = subscription.take_changes();
                        let files = if changes.full {
                            directory
                                .entries
                                .iter()
                                .filter(|e| !e.is_dir)
                                .map(|e| e.path.clone())
                                .collect()
                        } else {
                            changes.paths.into_iter().map(|p| changed.join(p)).collect()
                        };
                        FileModel::handle(ctx)
                            .update(ctx, |model, ctx| model.reload_file_paths(files, ctx));
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
        self.directories
            .get_mut(&path)
            .expect("directory just inserted")
            .native = true;
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

    fn refresh(&mut self, path: &Path, ctx: &mut ViewContext<Self>) {
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
                if directory.native
                    && let Some(watcher) = &self.watcher
                {
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
            collect(root, 0, &self.directories, &mut self.rows);
        }
        for _ in old_count..self.rows.len() {
            self.list.add_item();
        }
        for index in (self.rows.len()..old_count).rev() {
            self.list.remove(index);
        }
        for index in 0..self.rows.len() {
            self.list.invalidate_height_for_index(index);
        }
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

    fn render_row(&self, index: usize, app: &AppContext) -> Box<dyn Element> {
        let Some(row) = self.rows.get(index) else {
            return Empty::new().finish();
        };
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let expanded = self.directories.contains_key(&row.path);
        let name = row.path.file_name().unwrap_or_default().to_string_lossy();
        let label = if row.is_dir {
            format!("{} {name}", if expanded { "▾" } else { "▸" })
        } else {
            name.into_owned()
        };
        let icon = if row.is_dir { Icon::Folder } else { Icon::File };
        let content = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(icon.to_warpui_icon(theme.foreground()).finish())
            .with_child(
                Shrinkable::new(
                    1.,
                    Text::new_inline(label, appearance.ui_font_family(), 13.)
                        .with_color(theme.main_text_color(theme.background()).into())
                        .finish(),
                )
                .finish(),
            );
        let mut container = Container::new(content.finish()).with_padding(Padding::uniform(2.));
        if self.selected.as_ref() == Some(&row.path) {
            container = container.with_background(theme.surface_2());
        }
        let path = row.path.clone();
        EventHandler::new(
            ConstrainedBox::new(
                container
                    .with_margin_left(8. + row.depth as f32 * 14.)
                    .finish(),
            )
            .with_height(28.)
            .finish(),
        )
        .on_left_mouse_down(move |ctx, _, _| {
            ctx.dispatch_typed_action(FileBrowserAction::Select(path.clone()));
            DispatchEventResult::StopPropagation
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
    type Event = PathBuf;
}

impl TypedActionView for FileBrowserView {
    type Action = FileBrowserAction;
    fn handle_action(&mut self, action: &Self::Action, ctx: &mut ViewContext<Self>) {
        match action {
            FileBrowserAction::Select(path) => {
                let is_dir = self.rows.iter().any(|row| row.path == *path && row.is_dir);
                if is_dir {
                    if self.directories.contains_key(path) {
                        self.collapse(path, ctx);
                    } else {
                        self.expand(path.clone(), ctx);
                    }
                    self.update_rows(ctx);
                } else {
                    self.selected = Some(path.clone());
                    ctx.emit(path.clone());
                    ctx.notify();
                }
            }
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
        let title = self
            .root
            .as_ref()
            .map(|p| {
                p.file_name()
                    .unwrap_or_else(|| p.as_os_str())
                    .to_string_lossy()
                    .into_owned()
            })
            .unwrap_or_else(|| "Files".into());
        let header = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(
                Expanded::new(
                    1.,
                    Text::new_inline(title, appearance.ui_font_family(), 13.)
                        .with_color(theme.main_text_color(theme.background()).into())
                        .finish(),
                )
                .finish(),
            )
            .with_child(
                close_button(appearance, self.close_mouse.clone())
                    .build()
                    .on_click(|ctx, _, _| {
                        ctx.dispatch_typed_action(WorkspaceAction::ToggleFileBrowser);
                    })
                    .finish(),
            );
        let mut column = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(header.finish());
        let message = match &self.root {
            None => Some("Select a local or WSL terminal folder"),
            Some(root) => self
                .directories
                .get(root)
                .and_then(|directory| directory.error.as_deref()),
        };
        if let Some(message) = message {
            column.add_child(
                Text::new_inline(message.to_owned(), appearance.ui_font_family(), 12.)
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
            .with_padding(Padding::uniform(8.))
            .finish()
    }
}

impl Drop for FileBrowserView {
    fn drop(&mut self) {
        self.cancel_reads();
    }
}
