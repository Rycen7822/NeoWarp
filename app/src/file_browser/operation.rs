use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use warpui::elements::{
    Border, ChildView, Container, CrossAxisAlignment, DispatchEventResult, EventHandler, Flex,
    Padding, ParentElement, Text,
};
use warpui::{
    AppContext, Element, Entity, FocusContext, SingletonEntity, TypedActionView, View, ViewContext,
    ViewHandle,
};

use crate::appearance::Appearance;
use crate::editor::{EditorView, Event as EditorEvent, SingleLineEditorOptions, TextOptions};
use crate::localization::tr;

#[cfg(test)]
#[path = "operation_tests.rs"]
mod tests;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Operation {
    NewFile,
    NewFolder,
    Rename,
    Delete,
}

impl Operation {
    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::NewFile => "New file",
            Self::NewFolder => "New folder",
            Self::Rename => "Rename",
            Self::Delete => "Delete",
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) enum Action {
    Apply,
    Cancel,
}

pub(crate) enum Event {
    Changed(PathBuf),
    Cancelled,
}

pub(crate) struct FileOperationView {
    path: PathBuf,
    operation: Operation,
    name: ViewHandle<EditorView>,
    error: Option<String>,
    busy: bool,
}

impl FileOperationView {
    pub(crate) fn new(path: PathBuf, operation: Operation, ctx: &mut ViewContext<Self>) -> Self {
        let name = ctx.add_typed_action_view(|ctx| {
            let mut field = EditorView::single_line(
                SingleLineEditorOptions {
                    text: TextOptions::ui_text(None),
                    ..Default::default()
                },
                ctx,
            );
            if operation == Operation::Rename {
                field.system_reset_buffer_text(
                    &path.file_name().unwrap_or_default().to_string_lossy(),
                    ctx,
                );
            }
            field
        });
        ctx.subscribe_to_view(&name, |me, _, event, ctx| match event {
            EditorEvent::Enter if me.operation != Operation::Delete => me.apply(ctx),
            EditorEvent::Escape if !me.busy => ctx.emit(Event::Cancelled),
            _ => (),
        });
        Self {
            path,
            operation,
            name,
            error: None,
            busy: false,
        }
    }

    fn apply(&mut self, ctx: &mut ViewContext<Self>) {
        if self.busy {
            return;
        }
        let name = self.name.as_ref(ctx).buffer_text(ctx);
        self.busy = true;
        self.error = None;
        ctx.spawn(
            apply(self.path.clone(), self.operation, name),
            |me, result, ctx| {
                me.busy = false;
                match result {
                    Ok(parent) => ctx.emit(Event::Changed(parent)),
                    Err(error) => me.error = Some(format!("{error:#}")),
                }
                ctx.notify();
            },
        );
        ctx.notify();
    }
}

// Native and WSL UNC entries use the same asynchronous filesystem operations, never a shell.
async fn apply(path: PathBuf, operation: Operation, name: String) -> Result<PathBuf> {
    let parent = match operation {
        Operation::NewFile | Operation::NewFolder => path.as_path(),
        _ => path.parent().context("Cannot change a filesystem root")?,
    };
    if operation == Operation::Delete {
        let metadata = async_fs::symlink_metadata(&path).await?;
        let is_dir = metadata.is_dir();
        #[cfg(windows)]
        let is_dir = {
            use std::os::windows::fs::FileTypeExt;
            is_dir || metadata.file_type().is_symlink_dir()
        };
        // remove_dir_all removes directory links themselves without traversing their targets.
        if is_dir {
            async_fs::remove_dir_all(&path).await?;
        } else {
            async_fs::remove_file(&path).await?;
        }
        return Ok(parent.into());
    }
    let mut components = Path::new(&name).components();
    ensure!(
        matches!(components.next(), Some(Component::Normal(_)))
            && components.next().is_none()
            && !name.contains(['/', '\\', '\0']),
        "{}",
        tr("Enter a single file or folder name"),
    );
    #[cfg(windows)]
    ensure!(
        !name.contains([':', '*', '?', '"', '<', '>', '|']) && !name.ends_with(['.', ' ']),
        "{}",
        tr("Enter a valid Windows file or folder name")
    );
    let destination = parent.join(name);
    match operation {
        Operation::NewFile => {
            async_fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(destination)
                .await?;
        }
        Operation::NewFolder => async_fs::create_dir(destination).await?,
        Operation::Rename => {
            if path == destination {
                return Ok(parent.into());
            }
            match async_fs::symlink_metadata(&destination).await {
                Ok(_) => bail!("{}", tr("A file or folder with that name already exists")),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                Err(error) => return Err(error.into()),
            }
            async_fs::rename(&path, destination).await?;
        }
        Operation::Delete => unreachable!(),
    }
    Ok(parent.into())
}

impl Entity for FileOperationView {
    type Event = Event;
}

impl TypedActionView for FileOperationView {
    type Action = Action;
    fn handle_action(&mut self, action: &Action, ctx: &mut ViewContext<Self>) {
        match action {
            Action::Apply => self.apply(ctx),
            Action::Cancel if !self.busy => ctx.emit(Event::Cancelled),
            _ => (),
        }
    }
}

impl View for FileOperationView {
    fn ui_name() -> &'static str {
        "FileOperation"
    }

    fn on_focus(&mut self, focus: &FocusContext, ctx: &mut ViewContext<Self>) {
        if focus.is_self_focused() && self.operation != Operation::Delete {
            ctx.focus(&self.name);
        }
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let text = |value: String| {
            Text::new(
                value,
                appearance.ui_font_family(),
                appearance.ui_font_size(),
            )
            .with_color(theme.main_text_color(theme.surface_2()).into())
            .finish()
        };
        let mut column = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(12.);
        column.add_child(text(crate::util::path::display_local_path(&self.path)));
        if self.operation == Operation::Delete {
            column.add_child(text(
                tr("Permanently delete this item and its contents? This cannot be undone.").into(),
            ));
        } else {
            column.add_child(
                Container::new(ChildView::new(&self.name).finish())
                    .with_padding(Padding::uniform(8.))
                    .with_border(Border::all(1.).with_border_fill(theme.outline()))
                    .finish(),
            );
        }
        if let Some(error) = &self.error {
            column.add_child(text(error.clone()));
        }
        let mut buttons = Flex::row().with_spacing(12.);
        for (action, label) in [
            (Action::Cancel, "Cancel"),
            (
                Action::Apply,
                if self.busy {
                    "Working…"
                } else if self.operation == Operation::Delete {
                    "Delete permanently"
                } else {
                    "Save"
                },
            ),
        ] {
            let busy = self.busy;
            buttons.add_child(
                EventHandler::new(
                    Container::new(text(tr(label).into()))
                        .with_padding(Padding::uniform(8.))
                        .finish(),
                )
                .on_left_mouse_down(move |ctx, _, _| {
                    if !busy {
                        ctx.dispatch_typed_action(action.clone());
                    }
                    DispatchEventResult::StopPropagation
                })
                .finish(),
            );
        }
        column.with_child(buttons.finish()).finish()
    }
}
