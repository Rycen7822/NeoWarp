use std::collections::HashSet;
use warpui::elements::Fill;

use uuid::Uuid;
use warpui::elements::new_scrollable::{NewScrollableElement, SingleAxisConfig};
use warpui::elements::{
    ConstrainedBox, Container, CrossAxisAlignment, DispatchEventResult, Empty, EventHandler, Flex,
    List, ListState, MouseStateHandle, NewScrollable, Padding, ParentElement, ScrollStateHandle,
    Shrinkable, Text,
};
use warpui::{AppContext, Element, Entity, SingletonEntity, TypedActionView, View, ViewContext};

use super::ProjectSessions;
use super::config::Host;
use crate::appearance::Appearance;
use crate::workspace::{SidebarTab, WorkspaceAction};

#[derive(Clone, Eq, PartialEq)]
enum Row {
    Project(Uuid),
    Session { project: Uuid, session: Uuid },
}

pub(crate) struct ProjectsView {
    rows: Vec<Row>,
    collapsed: HashSet<Uuid>,
    selected: Option<Uuid>,
    show_older: bool,
    list: ListState<()>,
    scroll: ScrollStateHandle,
    close: MouseStateHandle,
}

#[derive(Clone, Debug)]
pub(crate) enum ProjectsAction {
    Toggle(Uuid),
    Open { project: Uuid, session: Uuid },
    Older,
    New,
}

impl ProjectsView {
    pub(crate) fn new(ctx: &mut ViewContext<Self>) -> Self {
        let weak = ctx.handle();
        let list = ListState::new(move |index, _, app| {
            weak.upgrade(app).map_or_else(
                || Empty::new().finish(),
                |view| view.as_ref(app).row(index, app),
            )
        });
        ctx.observe(&ProjectSessions::handle(ctx), |me, _, ctx| me.sync(ctx));
        let mut view = Self {
            rows: Vec::new(),
            collapsed: HashSet::new(),
            selected: None,
            show_older: false,
            list,
            scroll: Default::default(),
            close: Default::default(),
        };
        view.sync(ctx);
        view
    }

    fn sync(&mut self, ctx: &mut ViewContext<Self>) {
        let store = ProjectSessions::as_ref(ctx);
        let cutoff = chrono::Utc::now().timestamp_millis() - 72 * 60 * 60 * 1000;
        let mut projects: Vec<_> = store.catalog.projects.iter().collect();
        projects.sort_by_key(|project| {
            (
                std::cmp::Reverse(project.pinned),
                std::cmp::Reverse(if store.catalog.manual_order {
                    0
                } else {
                    project.last_used
                }),
            )
        });
        let mut rows = Vec::new();
        for project in projects {
            let mut sessions: Vec<_> = project
                .sessions
                .iter()
                .filter(|session| {
                    self.show_older
                        || session.pinned
                        || session.last_used >= cutoff
                        || store.active_terminal(session.id, ctx).is_some()
                })
                .collect();
            if sessions.is_empty() && !project.pinned && !self.show_older {
                continue;
            }
            rows.push(Row::Project(project.id));
            if self.collapsed.contains(&project.id) {
                continue;
            }
            sessions.sort_by_key(|session| {
                (
                    std::cmp::Reverse(session.pinned),
                    std::cmp::Reverse(if project.manual_order {
                        0
                    } else {
                        session.last_used
                    }),
                )
            });
            rows.extend(sessions.into_iter().map(|session| Row::Session {
                project: project.id,
                session: session.id,
            }));
        }
        let previous = self.rows.len();
        self.rows = rows;
        for _ in previous..self.rows.len() {
            self.list.add_item();
        }
        for index in (self.rows.len()..previous).rev() {
            self.list.remove(index);
        }
        ctx.notify();
    }

    fn row(&self, index: usize, app: &AppContext) -> Box<dyn Element> {
        let Some(row) = self.rows.get(index) else {
            return Empty::new().finish();
        };
        let store = ProjectSessions::as_ref(app);
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let (project, session) = match row {
            Row::Project(id) => (*id, None),
            Row::Session { project, session } => (*project, Some(*session)),
        };
        let Some(folder) = store
            .catalog
            .projects
            .iter()
            .find(|candidate| candidate.id == project)
        else {
            return Empty::new().finish();
        };
        let label = match session {
            None => {
                let name = folder
                    .directory
                    .trim_end_matches(['/', '\\'])
                    .rsplit(['/', '\\'])
                    .next()
                    .unwrap_or(&folder.directory);
                let host = match &folder.host {
                    Host::Local => String::new(),
                    Host::Wsl(name) => format!(" [{name}]"),
                };
                format!(
                    "{} {}{name}{host}",
                    if self.collapsed.contains(&project) {
                        "▸"
                    } else {
                        "▾"
                    },
                    if folder.pinned { "★ " } else { "" }
                )
            }
            Some(id) => {
                let Some(session) = folder.sessions.iter().find(|session| session.id == id) else {
                    return Empty::new().finish();
                };
                let state = if store.active_terminal(id, app).is_some() {
                    "Running"
                } else if session.identity.is_some() || session.resume_command.is_some() {
                    "Resume"
                } else {
                    "Needs ID"
                };
                format!(
                    "{}{} · {} · {state}",
                    if session.pinned { "★ " } else { "" },
                    session.title,
                    session.provider.name()
                )
            }
        };
        let child = ConstrainedBox::new(
            Container::new(
                Text::new_inline(label, appearance.ui_font_family(), 12.)
                    .with_color(theme.main_text_color(theme.background()).into())
                    .finish(),
            )
            .with_margin_left(if session.is_some() { 16. } else { 0. })
            .with_padding(Padding::uniform(4.))
            .finish(),
        )
        .with_height(28.)
        .finish();
        EventHandler::new(child)
            .on_left_mouse_down(move |ctx, _, _| {
                if let Some(id) = session {
                    ctx.dispatch_typed_action(ProjectsAction::Open {
                        project,
                        session: id,
                    });
                } else {
                    ctx.dispatch_typed_action(ProjectsAction::Toggle(project));
                }
                DispatchEventResult::StopPropagation
            })
            .on_right_mouse_down(move |ctx, _, position, _| {
                if let Some(id) = session {
                    ctx.dispatch_typed_action(WorkspaceAction::ShowProjectSessionMenu {
                        id,
                        position,
                    });
                } else {
                    ctx.dispatch_typed_action(WorkspaceAction::ShowProjectMenu {
                        id: project,
                        position,
                    });
                }
                DispatchEventResult::StopPropagation
            })
            .finish()
    }
}

impl Entity for ProjectsView {
    type Event = ();
}
impl TypedActionView for ProjectsView {
    type Action = ProjectsAction;
    fn handle_action(&mut self, action: &Self::Action, ctx: &mut ViewContext<Self>) {
        match action {
            ProjectsAction::Open { project, session } => {
                self.selected = Some(*project);
                ctx.dispatch_typed_action(&WorkspaceAction::OpenProjectSession(*session));
            }
            ProjectsAction::Toggle(id) => {
                self.selected = Some(*id);
                if !self.collapsed.remove(id) {
                    self.collapsed.insert(*id);
                }
                self.sync(ctx);
            }
            ProjectsAction::Older => {
                self.show_older = !self.show_older;
                self.sync(ctx);
            }
            ProjectsAction::New => {
                ctx.dispatch_typed_action(&WorkspaceAction::CreateProjectSession(self.selected))
            }
        }
    }
}

impl View for ProjectsView {
    fn ui_name() -> &'static str {
        "Projects"
    }
    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let store = ProjectSessions::as_ref(app);
        let mut column = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(crate::workspace::sidebar::header(
                SidebarTab::Projects,
                self.close.clone(),
                app,
            ));
        let mut toolbar = Flex::row().with_spacing(8.);
        for (action, text) in [
            (ProjectsAction::New, "+ New session"),
            (
                ProjectsAction::Older,
                if self.show_older {
                    "Recent only"
                } else {
                    "Show older"
                },
            ),
        ] {
            toolbar.add_child(
                EventHandler::new(
                    Container::new(
                        Text::new_inline(text, appearance.ui_font_family(), 12.)
                            .with_color(theme.main_text_color(theme.background()).into())
                            .finish(),
                    )
                    .with_padding(Padding::uniform(5.))
                    .finish(),
                )
                .on_left_mouse_down(move |ctx, _, _| {
                    ctx.dispatch_typed_action(action.clone());
                    DispatchEventResult::StopPropagation
                })
                .finish(),
            );
        }
        column.add_child(toolbar.finish());
        let message = store.error.as_deref().or({
            if !store.ready {
                Some("Loading sessions…")
            } else if self.rows.is_empty() {
                Some("Start a CLI here or add a session. Unpinned entries hide after 3 days.")
            } else {
                None
            }
        });
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
            Fill::None,
        )
        .finish();
        column.add_child(Shrinkable::new(1., list).finish());
        Container::new(column.finish())
            .with_padding(Padding::uniform(8.))
            .finish()
    }
}

