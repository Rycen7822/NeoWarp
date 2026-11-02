use pathfinder_color::ColorU;
use pathfinder_geometry::vector::{Vector2F, vec2f};
use std::collections::{HashMap, HashSet};
use std::time::Duration;

use uuid::Uuid;
use warpui::accessibility::{SemanticId, Semantics, accessible};
use warpui::r#async::{SpawnedFutureHandle, Timer};
use warpui::elements::new_scrollable::{NewScrollableElement, SingleAxisConfig};
use warpui::elements::{
    Align, Border, ChildAnchor, ConstrainedBox, Container, CornerRadius, CrossAxisAlignment,
    DispatchEventResult, DragAxis, Draggable, DraggableState, Empty, EventHandler, Expanded, Fill,
    Flex, Hoverable, List, ListState, MouseStateHandle, NewScrollable, OffsetPositioning, Padding,
    ParentAnchor, ParentElement, ParentOffsetBounds, Radius, SavePosition, ScrollStateHandle,
    Shrinkable, Stack, Text,
};
use warpui::fonts::{Properties, Weight};
use warpui::platform::Cursor;
use warpui::text_layout::ClipConfig;
use warpui::ui_components::components::UiComponent;
use warpui::{
    AppContext, Element, Entity, SingletonEntity, TypedActionView, View, ViewContext, WindowId,
};

use super::ProjectSessions;
use super::config::{Entry as Row, Host};
use crate::appearance::Appearance;
use crate::ui_components::buttons::icon_button;
use crate::ui_components::icons::Icon;
use crate::workspace::{ActiveSession, SidebarTab, WorkspaceAction};

#[derive(Default)]
struct RowState {
    semantics: SemanticId,
    pin_semantics: SemanticId,
    hover: MouseStateHandle,
    menu: MouseStateHandle,
    drag: DraggableState,
}

pub(crate) struct ProjectsView {
    rows: Vec<Row>,
    states: HashMap<Row, RowState>,
    window: WindowId,
    collapsed: HashSet<Uuid>,
    selected: Option<Uuid>,
    show_older: bool,
    list: ListState<()>,
    scroll: ScrollStateHandle,
    header: crate::workspace::sidebar::HeaderState,
    new_button: MouseStateHandle,
    older_button: MouseStateHandle,
    new_semantics: SemanticId,
    older_semantics: SemanticId,
    expiry: Option<SpawnedFutureHandle>,
}

#[derive(Clone, Debug)]
pub(crate) enum ProjectsAction {
    Toggle(Uuid),
    Open { project: Uuid, session: Uuid },
    Older,
    New,
    Drop { source: Row, position: Vector2F },
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
        ctx.observe(&ActiveSession::handle(ctx), |_, _, ctx| ctx.notify());
        let mut view = Self {
            rows: Vec::new(),
            states: HashMap::new(),
            window: ctx.window_id(),
            collapsed: HashSet::new(),
            selected: None,
            show_older: false,
            list,
            scroll: Default::default(),
            header: Default::default(),
            new_button: Default::default(),
            older_button: Default::default(),
            new_semantics: Default::default(),
            older_semantics: Default::default(),
            expiry: None,
        };
        view.sync(ctx);
        view.schedule_expiry(ctx);
        view
    }

    fn schedule_expiry(&mut self, ctx: &mut ViewContext<Self>) {
        self.expiry = Some(
            ctx.spawn(Timer::after(Duration::from_secs(60)), |me, _, ctx| {
                me.expiry = None;
                me.sync(ctx);
                me.schedule_expiry(ctx);
            }),
        );
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
        let visible: HashSet<_> = self.rows.iter().copied().collect();
        self.states.retain(|row, _| visible.contains(row));
        for row in &self.rows {
            self.states.entry(*row).or_default();
        }
        for _ in previous..self.rows.len() {
            self.list.add_item();
        }
        for index in (self.rows.len()..previous).rev() {
            self.list.remove(index);
        }
        ctx.notify();
    }

    fn row(&self, index: usize, app: &AppContext) -> Box<dyn Element> {
        let Some(row) = self.rows.get(index).copied() else {
            return Empty::new().finish();
        };
        let (project, session_id) = match row {
            Row::Project(id) => (id, None),
            Row::Session { project, session } => (project, Some(session)),
        };
        let store = ProjectSessions::as_ref(app);
        let Some(folder) = store.project(project) else {
            return Empty::new().finish();
        };
        let session =
            session_id.and_then(|id| folder.sessions.iter().find(|session| session.id == id));
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let state = &self.states[&row];
        let terminal = session_id.and_then(|id| store.active_terminal(id, app));
        let selected = terminal.is_some()
            && terminal == ActiveSession::as_ref(app).terminal_view_id(self.window);
        let hovered = Hoverable::new(state.hover.clone(), |hover| {
            let show_controls = hover.is_hovered() || state.drag.is_dragging();
            let mut leading = Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(6.);
            if session.is_none() {
                leading.add_child(sized_icon(
                    if self.collapsed.contains(&project) {
                        Icon::ChevronRight
                    } else {
                        Icon::ChevronDown
                    },
                    12.,
                    theme.sub_text_color(theme.background()),
                ));
            }
            let icon = if terminal.is_some() {
                Align::new(sized_icon(Icon::CircleFilled, 7., theme.accent())).finish()
            } else {
                sized_icon(
                    if session.is_some() {
                        Icon::Terminal
                    } else {
                        Icon::FolderClosed
                    },
                    16.,
                    theme.sub_text_color(theme.background()),
                )
            };
            leading.add_child(
                ConstrainedBox::new(icon)
                    .with_width(16.)
                    .with_height(16.)
                    .finish(),
            );
            let title = session.map_or_else(
                || {
                    let name = folder
                        .directory
                        .trim_end_matches(['/', '\\'])
                        .rsplit(['/', '\\'])
                        .next()
                        .unwrap_or(&folder.directory);
                    match &folder.host {
                        Host::Local => name.to_owned(),
                        Host::Wsl(distro) => format!("{name} · {distro}"),
                    }
                },
                |session| session.title.clone(),
            );
            let pinned = session.map_or(folder.pinned, |session| session.pinned);
            let semantic_label = app.accessibility_plugin_enabled().then(|| title.clone());
            let title_text = Text::new_inline(title, appearance.ui_font_family(), 14.)
                .with_clip(ClipConfig::ellipsis())
                .with_color(theme.main_text_color(theme.background()).into())
                .with_style(Properties::default().weight(if session.is_none() {
                    Weight::Bold
                } else {
                    Weight::Normal
                }));
            let mut title_row = Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(6.)
                .with_child(Shrinkable::new(1., title_text.finish()).finish());
            if pinned {
                title_row.add_child(sized_icon(
                    Icon::PinFilledDiagonal,
                    12.,
                    theme.sub_text_color(theme.background()),
                ));
            }
            let text = if let Some(session) = session {
                let detail = if let Some((host, directory)) =
                    terminal.and_then(|id| store.running_directory(id))
                    && (host != &folder.host || !host.same_path(directory, &folder.directory))
                {
                    format!("Running in {directory}")
                } else if terminal.is_none()
                    && session.identity.is_none()
                    && session.resume_command.is_none()
                {
                    format!("{} · Needs session ID", session.provider.name())
                } else if session.title.eq_ignore_ascii_case(session.provider.name()) {
                    format!("{:?}", session.shell.kind)
                } else {
                    session.provider.name().to_owned()
                };
                let secondary = Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(8.)
                    .with_child(
                        Expanded::new(
                            1.,
                            Text::new_inline(detail, appearance.ui_font_family(), 11.)
                                .with_clip(ClipConfig::ellipsis())
                                .with_color(theme.sub_text_color(theme.background()).into())
                                .finish(),
                        )
                        .finish(),
                    )
                    .with_child(
                        Text::new_inline(
                            if terminal.is_some() {
                                "Running".to_owned()
                            } else {
                                relative_time(session.last_used)
                            },
                            appearance.ui_font_family(),
                            11.,
                        )
                        .with_color(theme.sub_text_color(theme.background()).into())
                        .finish(),
                    )
                    .finish();
                Flex::column()
                    .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                    .with_spacing(3.)
                    .with_child(title_row.finish())
                    .with_child(secondary)
                    .finish()
            } else {
                title_row.add_child(
                    Text::new_inline(
                        folder.sessions.len().to_string(),
                        appearance.ui_font_family(),
                        11.,
                    )
                    .with_color(theme.sub_text_color(theme.background()).into())
                    .finish(),
                );
                title_row.finish()
            };
            leading.add_child(Expanded::new(1., text).finish());
            let clickable = EventHandler::new(
                ConstrainedBox::new(leading.finish())
                    .with_height(if session.is_some() { 52. } else { 36. })
                    .finish(),
            )
            .on_left_mouse_down(move |ctx, _, _| {
                ctx.dispatch_typed_action(match session_id {
                    Some(session) => ProjectsAction::Open { project, session },
                    None => ProjectsAction::Toggle(project),
                });
                DispatchEventResult::StopPropagation
            })
            .finish();
            let semantics = Semantics::new(
                state.semantics,
                semantic_label.as_deref().unwrap_or_default(),
            );
            let semantics = if session.is_some() {
                semantics.selected(selected)
            } else {
                semantics.expanded(!self.collapsed.contains(&project))
            };
            let clickable = accessible(
                clickable,
                app,
                semantics,
                match session_id {
                    Some(session) => ProjectsAction::Open { project, session },
                    None => ProjectsAction::Toggle(project),
                },
            );
            let grip = ConstrainedBox::new(if show_controls {
                sized_icon(
                    Icon::DragIndicatorVertical,
                    14.,
                    theme.sub_text_color(theme.background()),
                )
            } else {
                Empty::new().finish()
            })
            .with_width(16.)
            .with_height(24.)
            .finish();
            let grip = Draggable::new(state.drag.clone(), grip)
                .with_drag_axis(DragAxis::VerticalOnly)
                .on_drop(move |ctx, _, position, _| {
                    ctx.dispatch_typed_action(ProjectsAction::Drop {
                        source: row,
                        position: position.center(),
                    });
                })
                .finish();
            let menu = if app.accessibility_plugin_enabled() {
                let action = match session_id {
                    Some(id) => WorkspaceAction::ToggleProjectSessionPin(id),
                    None => WorkspaceAction::ToggleProjectPin(project),
                };
                let click_action = action.clone();
                accessible(
                    icon_button(
                        appearance,
                        Icon::PinFilledDiagonal,
                        pinned,
                        state.menu.clone(),
                    )
                    .build()
                    .on_click(move |ctx, _, _| ctx.dispatch_typed_action(click_action.clone()))
                    .finish(),
                    app,
                    Semantics::new(
                        state.pin_semantics,
                        &format!("Pin {}", semantic_label.as_deref().unwrap_or_default()),
                    )
                    .checked(pinned),
                    action,
                )
            } else if show_controls {
                icon_button(appearance, Icon::DotsHorizontal, false, state.menu.clone())
                    .build()
                    .on_click(move |ctx, _, position| {
                        ctx.dispatch_typed_action(match session_id {
                            Some(id) => WorkspaceAction::ShowProjectSessionMenu { id, position },
                            None => WorkspaceAction::ShowProjectMenu {
                                id: project,
                                position,
                            },
                        });
                    })
                    .finish()
            } else {
                ConstrainedBox::new(Empty::new().finish())
                    .with_width(24.)
                    .with_height(24.)
                    .finish()
            };
            let content = Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(4.)
                .with_child(grip)
                .with_child(Expanded::new(1., clickable).finish())
                .with_child(menu)
                .finish();
            let mut container = Container::new(content)
                .with_horizontal_padding(4.)
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.)));
            if selected {
                container = container
                    .with_background(theme.surface_overlay_2())
                    .with_corner_radius(CornerRadius::default());
            } else if hover.is_hovered() {
                container = container.with_background(theme.surface_overlay_1());
            }
            let card = ConstrainedBox::new(container.finish())
                .with_height(if session.is_some() { 52. } else { 36. })
                .finish();
            Container::new(if selected {
                selection_corners(card)
            } else {
                card
            })
            .with_margin_left(if session.is_some() { 14. } else { 0. })
            .with_margin_right(1.)
            .with_margin_bottom(4.)
            .finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_right_click(move |ctx, _, position| {
            ctx.dispatch_typed_action(match session_id {
                Some(id) => WorkspaceAction::ShowProjectSessionMenu { id, position },
                None => WorkspaceAction::ShowProjectMenu {
                    id: project,
                    position,
                },
            });
        })
        .finish();
        SavePosition::new(hovered, &format!("project-entry-{}", row.id())).finish()
    }

    fn drop_entry(&mut self, source: Row, position: Vector2F, ctx: &mut ViewContext<Self>) {
        let target = self.rows.iter().find_map(|row| {
            let rect = ctx.element_position_by_id_at_last_frame(
                ctx.window_id(),
                format!("project-entry-{}", row.id()),
            )?;
            rect.contains_point(position)
                .then_some((*row, position.y() > rect.center().y()))
        });
        let Some((target, after)) = target else {
            return;
        };
        ProjectSessions::handle(ctx).update(ctx, |store, ctx| {
            if let Err(error) = store.reorder(source, target, after, ctx) {
                store.error = Some(format!("Cannot reorder: {error:#}"));
                ctx.notify();
            }
        });
    }
}

impl Entity for ProjectsView {
    type Event = ();
}

impl Drop for ProjectsView {
    fn drop(&mut self) {
        if let Some(expiry) = self.expiry.take() {
            expiry.abort();
        }
    }
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
            ProjectsAction::Drop { source, position } => self.drop_entry(*source, *position, ctx),
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
                &self.header,
                app,
            ));
        let mut toolbar = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(
                Expanded::new(
                    1.,
                    Text::new_inline(
                        if self.show_older {
                            "All projects"
                        } else {
                            "Recent projects"
                        },
                        appearance.ui_font_family(),
                        12.,
                    )
                    .with_color(theme.sub_text_color(theme.background()).into())
                    .finish(),
                )
                .finish(),
            );
        for (action, icon, mouse, label, active, id) in [
            (
                ProjectsAction::Older,
                Icon::History,
                &self.older_button,
                if self.show_older {
                    "Show recent sessions only"
                } else {
                    "Show sessions older than 3 days"
                },
                self.show_older,
                self.older_semantics,
            ),
            (
                ProjectsAction::New,
                Icon::Plus,
                &self.new_button,
                "New CLI session",
                false,
                self.new_semantics,
            ),
        ] {
            let tooltip = appearance
                .ui_builder()
                .tool_tip(label.to_owned())
                .build()
                .finish();
            let semantic_action = action.clone();
            toolbar.add_child(accessible(
                icon_button(appearance, icon, active, mouse.clone())
                    .with_tooltip(move || tooltip)
                    .build()
                    .on_click(move |ctx, _, _| {
                        ctx.dispatch_typed_action(action.clone());
                    })
                    .finish(),
                app,
                Semantics::new(id, label),
                semantic_action,
            ));
        }
        column.add_child(
            Container::new(toolbar.finish())
                .with_margin_bottom(8.)
                .with_horizontal_padding(4.)
                .finish(),
        );
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
                Container::new(
                    Text::new(message.to_owned(), appearance.ui_font_family(), 12.)
                        .with_color(theme.sub_text_color(theme.background()).into())
                        .finish(),
                )
                .with_padding(Padding::uniform(12.))
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

fn sized_icon(icon: Icon, size: f32, color: warp_core::ui::theme::Fill) -> Box<dyn Element> {
    ConstrainedBox::new(icon.to_warpui_icon(color).finish())
        .with_width(size)
        .with_height(size)
        .finish()
}

fn selection_corners(card: Box<dyn Element>) -> Box<dyn Element> {
    let mut stack = Stack::new().with_child(card);
    for (x, y, parent, child) in [
        (-1., -1., ParentAnchor::TopLeft, ChildAnchor::TopLeft),
        (1., -1., ParentAnchor::TopRight, ChildAnchor::TopRight),
        (-1., 1., ParentAnchor::BottomLeft, ChildAnchor::BottomLeft),
        (1., 1., ParentAnchor::BottomRight, ChildAnchor::BottomRight),
    ] {
        let border = Border {
            top: y < 0.,
            bottom: y > 0.,
            left: x < 0.,
            right: x > 0.,
            ..Border::all(1.).with_border_color(ColorU::white())
        };
        stack.add_positioned_child(
            ConstrainedBox::new(
                Container::new(Empty::new().finish())
                    .with_border(border)
                    .finish(),
            )
            .with_width(12.)
            .with_height(12.)
            .finish(),
            OffsetPositioning::offset_from_parent(
                vec2f(x, y),
                ParentOffsetBounds::Unbounded,
                parent,
                child,
            ),
        );
    }
    stack.finish()
}

fn relative_time(last_used: i64) -> String {
    let minutes = (chrono::Utc::now().timestamp_millis() - last_used).max(0) / 60_000;
    match minutes {
        0 => "Just now".to_owned(),
        1..60 => format!("{minutes}m ago"),
        60..1440 => format!("{}h ago", minutes / 60),
        _ => format!("{}d ago", minutes / 1440),
    }
}
