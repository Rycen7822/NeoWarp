use anyhow::{Context, Result, bail};
use uuid::Uuid;
use warp_editor::editor::NavigationKey;
use warp_terminal::shell::ShellType;
use warpui::elements::{
    Border, ChildView, ClippedScrollStateHandle, ClippedScrollable, ConstrainedBox, Container,
    CrossAxisAlignment, DispatchEventResult, EventHandler, Fill, Flex, Padding, ParentElement,
    SavePosition, ScrollTarget, ScrollToPositionMode, ScrollbarWidth, Text,
};
use warpui::{
    AppContext, Element, Entity, FocusContext, SingletonEntity, TypedActionView, View, ViewContext,
    ViewHandle, WindowId,
};

use super::config::{Activation, Edit, Host, Identity, Provider, SavedShell, Session};
use super::{ProjectSessions, command};
use crate::appearance::Appearance;
use crate::editor::{
    EditorView, Event as EditorEvent, PropagateAndNoOpNavigationKeys, SingleLineEditorOptions,
    TextOptions,
};
use crate::localization::tr;
use crate::workspace::ActiveSession;

#[derive(Clone, Copy)]
enum Field {
    Name,
    Directory,
    Distro,
    Shell,
    Variables,
    Activation,
    Environment,
    Conda,
    Command,
    Identity,
    Resume,
}

impl Field {
    const ALL: [Self; 11] = [
        Self::Name,
        Self::Directory,
        Self::Distro,
        Self::Shell,
        Self::Variables,
        Self::Activation,
        Self::Environment,
        Self::Conda,
        Self::Command,
        Self::Identity,
        Self::Resume,
    ];
    fn label(self) -> &'static str {
        tr(match self {
            Self::Name => "Name",
            Self::Directory => "Working directory",
            Self::Distro => "WSL distribution (blank for local)",
            Self::Shell => "Shell executable",
            Self::Variables => "Variables (JSON, e.g. {\"KEY\":\"value\"})",
            Self::Activation => "Environment type: none / uv / conda",
            Self::Environment => "Venv path or Conda prefix/name",
            Self::Conda => "Conda executable (optional)",
            Self::Command => "Startup command",
            Self::Identity => "Session ID (Pi: session file)",
            Self::Resume => "Explicit resume command (optional)",
        })
    }
}

pub(crate) fn draft(
    session: Option<Uuid>,
    project: Option<Uuid>,
    window: WindowId,
    app: &AppContext,
) -> Result<Edit> {
    let store = ProjectSessions::as_ref(app);
    if let Some(id) = session {
        let (project, session) = store.entry(id).context("Session no longer exists")?;
        return Ok(Edit {
            host: project.host.clone(),
            directory: project.directory.clone(),
            session: session.clone(),
        });
    }
    let project = project.and_then(|id| store.project(id));
    if let Some(project) = project
        && let Some(template) = project
            .sessions
            .iter()
            .max_by_key(|session| session.last_used)
    {
        let mut session = template.clone();
        session.id = Uuid::new_v4();
        session.title = "New CLI session".into();
        session.command.clear();
        session.identity = None;
        session.resume_command = None;
        session.pinned = false;
        session.last_used = chrono::Utc::now().timestamp_millis();
        return Ok(Edit {
            host: project.host.clone(),
            directory: project.directory.clone(),
            session,
        });
    }
    let active = ActiveSession::as_ref(app);
    let session = active
        .session(window)
        .context("Select a local or WSL terminal first")?;
    let path = active
        .path_if_local(window)
        .context("Select a local or WSL terminal folder first")?;
    let wsl = warp_util::path::parse_wsl_unc_path(path);
    let (host, mut directory) = match wsl {
        Some(path) if session.wsl_distro_name().is_some() => {
            (Host::Wsl(path.distro), path.linux_path)
        }
        _ => (Host::Local, path.to_string_lossy().into_owned()),
    };
    if let Some(project) = project {
        anyhow::ensure!(
            host == project.host,
            "Select a terminal on this project's host to choose its shell"
        );
        directory = project.directory.clone();
    }
    Ok(Edit {
        host,
        directory,
        session: Session {
            id: Uuid::new_v4(),
            title: "New CLI session".into(),
            provider: Provider::Custom,
            shell: SavedShell {
                program: session
                    .shell()
                    .shell_path()
                    .clone()
                    .context("Shell is still starting")?,
                kind: session.shell().shell_type(),
            },
            environment: Default::default(),
            activation: Activation::None,
            command: String::new(),
            identity: None,
            resume_command: None,
            pinned: false,
            last_used: chrono::Utc::now().timestamp_millis(),
        },
    })
}

pub(crate) struct SessionEditor {
    base: Edit,
    is_new: bool,
    fields: Vec<ViewHandle<EditorView>>,
    scroll: ClippedScrollStateHandle,
    error: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) enum EditorAction {
    Save,
    Cancel,
}
pub(crate) enum EditorResult {
    Saved { id: Uuid, is_new: bool },
    Cancelled,
}

impl SessionEditor {
    pub(crate) fn new(base: Edit, is_new: bool, ctx: &mut ViewContext<Self>) -> Self {
        let (activation, environment, conda) = match &base.session.activation {
            Activation::None => ("none", String::new(), String::new()),
            Activation::Venv(path) => ("uv", path.clone(), String::new()),
            Activation::Conda {
                environment,
                executable,
            } => (
                "conda",
                environment.clone(),
                executable.clone().unwrap_or_default(),
            ),
        };
        let identity = base
            .session
            .identity
            .as_ref()
            .map(|identity| {
                identity
                    .transcript
                    .as_ref()
                    .filter(|_| base.session.provider == Provider::Pi)
                    .unwrap_or(&identity.id)
                    .clone()
            })
            .unwrap_or_default();
        let values = [
            base.session.title.clone(),
            base.directory.clone(),
            match &base.host {
                Host::Local => String::new(),
                Host::Wsl(name) => name.clone(),
            },
            base.session.shell.program.clone(),
            serde_json::to_string(&base.session.environment).expect("string map serializes"),
            activation.into(),
            environment,
            conda,
            base.session.command.clone(),
            identity,
            base.session.resume_command.clone().unwrap_or_default(),
        ];
        let mut fields = Vec::new();
        for (index, value) in values.into_iter().enumerate() {
            let field = ctx.add_typed_action_view(|ctx| {
                let mut editor = EditorView::single_line(
                    SingleLineEditorOptions {
                        text: TextOptions::ui_font_size(Appearance::as_ref(ctx)),
                        propagate_and_no_op_vertical_navigation_keys:
                            PropagateAndNoOpNavigationKeys::Always,
                        ..Default::default()
                    },
                    ctx,
                );
                editor.system_reset_buffer_text(&value, ctx);
                editor
            });
            ctx.subscribe_to_view(&field, move |me, _, event, ctx| match event {
                EditorEvent::Navigate(NavigationKey::Tab) => {
                    me.focus_field((index + 1) % me.fields.len(), ctx)
                }
                EditorEvent::Navigate(NavigationKey::ShiftTab) => {
                    me.focus_field((index + me.fields.len() - 1) % me.fields.len(), ctx)
                }
                EditorEvent::Enter => me.save(ctx),
                EditorEvent::Escape => ctx.emit(EditorResult::Cancelled),
                _ => (),
            });
            fields.push(field);
        }
        Self {
            base,
            is_new,
            fields,
            scroll: Default::default(),
            error: None,
        }
    }

    fn focus_field(&self, index: usize, ctx: &mut ViewContext<Self>) {
        ctx.focus(&self.fields[index]);
        self.scroll.scroll_to_position(ScrollTarget {
            position_id: format!("project-session-field-{index}"),
            mode: ScrollToPositionMode::FullyIntoView,
        });
    }

    fn text(&self, field: Field, app: &AppContext) -> String {
        self.fields[field as usize].as_ref(app).buffer_text(app)
    }

    fn edited(&self, app: &AppContext) -> Result<Edit> {
        let mut session = self.base.session.clone();
        session.title = self.text(Field::Name, app);
        let program = self.text(Field::Shell, app);
        let kind = if program == session.shell.program {
            session.shell.kind
        } else {
            ShellType::from_name(
                &program
                    .rsplit(['/', '\\'])
                    .next()
                    .unwrap_or(&program)
                    .to_ascii_lowercase(),
            )
            .context("Choose Bash, Zsh, Fish or PowerShell")?
        };
        session.shell = SavedShell { program, kind };
        session.command = self.text(Field::Command, app);
        session.provider = command::parse(&session.command, kind)
            .map_or(Provider::Custom, |parsed| parsed.provider);
        session.environment = serde_json::from_str(&self.text(Field::Variables, app))
            .context("Variables must be a JSON object of string values")?;
        let environment = self.text(Field::Environment, app);
        session.activation = match self
            .text(Field::Activation, app)
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "none" | "" => Activation::None,
            "uv" | "venv" if !environment.trim().is_empty() => Activation::Venv(environment),
            "conda" if !environment.trim().is_empty() => Activation::Conda {
                environment,
                executable: Some(self.text(Field::Conda, app))
                    .filter(|value| !value.trim().is_empty()),
            },
            _ => bail!("Choose none, uv or conda and supply its environment path/name"),
        };
        let id = self.text(Field::Identity, app);
        session.identity = (!id.trim().is_empty()).then(|| Identity {
            transcript: (session.provider == Provider::Pi).then(|| id.clone()),
            id,
        });
        session.resume_command =
            Some(self.text(Field::Resume, app)).filter(|value| !value.trim().is_empty());
        let distro = self.text(Field::Distro, app);
        Ok(Edit {
            host: if distro.trim().is_empty() {
                Host::Local
            } else {
                Host::Wsl(distro)
            },
            directory: self.text(Field::Directory, app),
            session,
        })
    }

    fn save(&mut self, ctx: &mut ViewContext<Self>) {
        let result = self.edited(ctx).and_then(|edit| {
            ProjectSessions::handle(ctx).update(ctx, |store, ctx| store.edit(edit, ctx))
        });
        match result {
            Ok(id) => ctx.emit(EditorResult::Saved {
                id,
                is_new: self.is_new,
            }),
            Err(error) => {
                self.error = Some(format!("{error:#}"));
                ctx.notify();
            }
        }
    }
}

impl Entity for SessionEditor {
    type Event = EditorResult;
}
impl TypedActionView for SessionEditor {
    type Action = EditorAction;
    fn handle_action(&mut self, action: &Self::Action, ctx: &mut ViewContext<Self>) {
        match action {
            EditorAction::Save => self.save(ctx),
            EditorAction::Cancel => ctx.emit(EditorResult::Cancelled),
        }
    }
}

impl View for SessionEditor {
    fn ui_name() -> &'static str {
        "ProjectSessionEditor"
    }
    fn on_focus(&mut self, focus: &FocusContext, ctx: &mut ViewContext<Self>) {
        if focus.is_self_focused() {
            self.focus_field(0, ctx);
        }
    }
    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let mut form = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch);
        form.add_child(
            Text::new_inline(
                tr("Changes apply on the next launch; the active terminal stays unchanged."),
                appearance.ui_font_family(),
                12.,
            )
            .with_color(theme.sub_text_color(theme.background()).into())
            .finish(),
        );
        for (index, field) in Field::ALL.into_iter().enumerate() {
            form.add_child(
                Text::new_inline(field.label(), appearance.ui_font_family(), 12.)
                    .with_color(theme.main_text_color(theme.background()).into())
                    .finish(),
            );
            form.add_child(
                Container::new(
                    SavePosition::new(
                        ChildView::new(&self.fields[index]).finish(),
                        &format!("project-session-field-{index}"),
                    )
                    .finish(),
                )
                .with_padding(Padding::uniform(4.))
                .with_margin_bottom(8.)
                .with_border(Border::all(1.).with_border_fill(theme.outline()))
                .finish(),
            );
        }
        let scroll = ClippedScrollable::vertical(
            self.scroll.clone(),
            form.finish(),
            ScrollbarWidth::Auto,
            theme.nonactive_ui_text_color().into(),
            theme.active_ui_text_color().into(),
            Fill::None,
        )
        .finish();
        let mut column = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(ConstrainedBox::new(scroll).with_max_height(300.).finish());
        if let Some(error) = &self.error {
            column.add_child(
                Text::new_inline(error.clone(), appearance.ui_font_family(), 12.)
                    .with_color(theme.main_text_color(theme.background()).into())
                    .finish(),
            );
        }
        let mut buttons = Flex::row().with_spacing(12.);
        for (action, label) in [
            (EditorAction::Cancel, "Cancel"),
            (
                EditorAction::Save,
                if self.is_new {
                    "Save and start"
                } else {
                    "Save"
                },
            ),
        ] {
            buttons.add_child(
                EventHandler::new(
                    Container::new(
                        Text::new_inline(tr(label), appearance.ui_font_family(), 13.)
                            .with_color(theme.main_text_color(theme.background()).into())
                            .finish(),
                    )
                    .with_padding(Padding::uniform(8.))
                    .finish(),
                )
                .on_left_mouse_down(move |ctx, _, _| {
                    ctx.dispatch_typed_action(action.clone());
                    DispatchEventResult::StopPropagation
                })
                .finish(),
            );
        }
        column.with_child(buttons.finish()).finish()
    }
}
