//! Local counterpart of Warp's diff selector, using the shared searchable Menu.
use std::path::PathBuf;

use warp_editor::editor::NavigationKey;
use warpui::r#async::SpawnedFutureHandle;
use warpui::elements::{
    ChildAnchor, ChildView, ConstrainedBox, Container, CrossAxisAlignment, Flex, MouseStateHandle,
    OffsetPositioning, ParentAnchor, ParentElement, ParentOffsetBounds, Stack, Text,
};
use warpui::geometry::vector::vec2f;
use warpui::text_layout::ClipConfig;
use warpui::ui_components::button::ButtonVariant;
use warpui::ui_components::components::UiComponent;
use warpui::{
    AppContext, Element, Entity, ModelHandle, SingletonEntity, TypedActionView, View, ViewContext,
    ViewHandle,
};

use super::diff_state::DiffMode;
use super::git_repo_model::GitRepoStatusModel;
use crate::appearance::Appearance;
use crate::editor::{
    EditorView, Event as EditorEvent, PropagateAndNoOpNavigationKeys, SingleLineEditorOptions,
    TextOptions,
};
use crate::localization::tr;
use crate::menu::{Event as MenuEvent, Menu, MenuAction, MenuItemFields, MenuVariant};
use crate::ui_components::icons::Icon;
use crate::util::git::{
    get_all_branches, get_all_branches_with_known_main, sort_branches_main_first,
};

#[derive(Clone, Debug)]
pub(super) enum Action {
    Toggle,
    Select(DiffMode),
}

pub(super) struct DiffSelector {
    root: PathBuf,
    status: ModelHandle<GitRepoStatusModel>,
    mode: DiffMode,
    targets: Vec<(String, DiffMode)>,
    menu: ViewHandle<Menu<Action>>,
    search: ViewHandle<EditorView>,
    open: bool,
    loading: Option<SpawnedFutureHandle>,
    error: bool,
    mouse: MouseStateHandle,
}

impl DiffSelector {
    pub(super) fn new(
        root: PathBuf,
        status: ModelHandle<GitRepoStatusModel>,
        ctx: &mut ViewContext<Self>,
    ) -> Self {
        let search = ctx.add_typed_action_view(|ctx| {
            let mut editor = EditorView::single_line(
                SingleLineEditorOptions {
                    text: TextOptions::ui_text(None),
                    propagate_and_no_op_vertical_navigation_keys:
                        PropagateAndNoOpNavigationKeys::Always,
                    ..Default::default()
                },
                ctx,
            );
            editor.set_placeholder_text(tr("Search diff sets or branches to compare…"), ctx);
            editor
        });
        let search_header = search.clone();
        let menu = ctx.add_typed_action_view(|_| {
            let mut menu = Menu::new()
                .with_menu_variant(MenuVariant::scrollable())
                .prevent_interaction_with_other_elements();
            menu.set_width(280.);
            menu.set_height(220.);
            menu.set_pinned_header_builder(move |app| {
                Appearance::as_ref(app)
                    .ui_builder()
                    .text_input(search_header.clone())
                    .build()
                    .finish()
            });
            menu
        });
        ctx.subscribe_to_view(&menu, |me, _, event, ctx| {
            if matches!(event, MenuEvent::Close { .. }) {
                me.close(ctx);
            }
        });
        ctx.subscribe_to_view(&search, |me, _, event, ctx| match event {
            EditorEvent::Edited(_) => me.update_menu(ctx),
            EditorEvent::Escape => me.close(ctx),
            EditorEvent::Enter => me
                .menu
                .update(ctx, |menu, ctx| menu.handle_action(&MenuAction::Enter, ctx)),
            EditorEvent::Navigate(NavigationKey::Up) => {
                me.menu.update(ctx, |menu, ctx| menu.select_previous(ctx))
            }
            EditorEvent::Navigate(NavigationKey::Down) => {
                me.menu.update(ctx, |menu, ctx| menu.select_next(ctx))
            }
            _ => (),
        });
        Self {
            root,
            status,
            mode: DiffMode::Head,
            targets: Vec::new(),
            menu,
            search,
            open: false,
            loading: None,
            error: false,
            mouse: Default::default(),
        }
    }

    fn close(&mut self, ctx: &mut ViewContext<Self>) {
        self.open = false;
        if let Some(task) = self.loading.take() {
            task.abort();
        }
        ctx.focus_self();
        ctx.notify();
    }

    pub(super) fn set_mode(&mut self, mode: DiffMode, ctx: &mut ViewContext<Self>) {
        self.mode = mode;
        ctx.notify();
    }

    fn update_menu(&mut self, ctx: &mut ViewContext<Self>) {
        let query = self.search.as_ref(ctx).buffer_text(ctx).to_lowercase();
        let mut items: Vec<_> = self
            .targets
            .iter()
            .filter(|(label, _)| label.to_lowercase().contains(&query))
            .map(|(label, mode)| {
                let mut item = MenuItemFields::new(label.clone())
                    .with_on_select_action(Action::Select(mode.clone()));
                if *mode == self.mode {
                    item = item.with_icon(Icon::Check);
                }
                item.into_item()
            })
            .collect();
        if items.is_empty() || self.error {
            items.push(
                MenuItemFields::new(tr(if self.error {
                    "Unable to load branches"
                } else {
                    "No matches"
                }))
                .with_disabled(true)
                .into_item(),
            );
        }
        self.menu.update(ctx, |menu, ctx| {
            menu.set_items(items, ctx);
            menu.select_first(ctx);
        });
        ctx.notify();
    }

    fn open(&mut self, ctx: &mut ViewContext<Self>) {
        self.open = true;
        self.error = false;
        self.search
            .update(ctx, |search, ctx| search.clear_buffer(ctx));
        self.targets = vec![(tr("Uncommitted changes").into(), DiffMode::Head)];
        self.update_menu(ctx);
        ctx.focus(&self.search);
        let root = self.root.clone();
        let metadata = self.status.as_ref(ctx).metadata(ctx);
        let main = metadata
            .map(|m| m.main_branch_name.clone())
            .unwrap_or_default();
        let current = metadata
            .map(|m| m.current_branch_name.clone())
            .unwrap_or_default();
        self.loading = Some(ctx.spawn(
            async move {
                if main.is_empty() {
                    get_all_branches(&root, None, false).await
                } else {
                    get_all_branches_with_known_main(&root, &main, None, false).await
                }
            },
            move |me, result, ctx| {
                me.loading = None;
                match result {
                    Ok(branches) => {
                        for branch in sort_branches_main_first(&branches) {
                            if branch.name != current || branch.is_main {
                                me.targets.push((
                                    branch.name.clone(),
                                    if branch.is_main {
                                        DiffMode::MainBranch
                                    } else {
                                        DiffMode::OtherBranch(branch.name.clone())
                                    },
                                ));
                            }
                        }
                    }
                    Err(error) => {
                        me.error = true;
                        log::warn!("Could not list Review comparison branches: {error:#}");
                    }
                }
                if let DiffMode::OtherBranch(branch) = &me.mode
                    && !me.targets.iter().any(|(_, mode)| *mode == me.mode)
                {
                    me.targets.insert(1, (branch.clone(), me.mode.clone()));
                }
                me.update_menu(ctx);
            },
        ));
    }
}

impl Entity for DiffSelector {
    type Event = DiffMode;
}

impl TypedActionView for DiffSelector {
    type Action = Action;
    fn handle_action(&mut self, action: &Action, ctx: &mut ViewContext<Self>) {
        match action {
            Action::Toggle if self.open => self.close(ctx),
            Action::Toggle => self.open(ctx),
            Action::Select(mode) => {
                self.close(ctx);
                ctx.emit(mode.clone());
            }
        }
    }
}

impl View for DiffSelector {
    fn ui_name() -> &'static str {
        "CodeReviewDiffSelector"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let color = appearance
            .theme()
            .main_text_color(appearance.theme().background());
        let label = match &self.mode {
            DiffMode::Head => tr("Uncommitted changes"),
            DiffMode::MainBranch => self
                .status
                .as_ref(app)
                .metadata(app)
                .map(|metadata| metadata.main_branch_name.as_str())
                .unwrap_or("main"),
            DiffMode::OtherBranch(branch) => branch,
        };
        let button = appearance
            .ui_builder()
            .button(ButtonVariant::Text, self.mouse.clone())
            .with_custom_label(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(6.)
                    .with_child(
                        ConstrainedBox::new(
                            Icon::SwitchHorizontal01.to_warpui_icon(color).finish(),
                        )
                        .with_width(15.)
                        .with_height(15.)
                        .finish(),
                    )
                    .with_child(
                        ConstrainedBox::new(
                            Text::new_inline(
                                label.to_owned(),
                                appearance.ui_font_family(),
                                appearance.ui_font_size(),
                            )
                            .with_color(color.into())
                            .with_clip(ClipConfig::ellipsis())
                            .finish(),
                        )
                        .with_max_width(240.)
                        .finish(),
                    )
                    .finish(),
            )
            .build()
            .on_click(|ctx, _, _| ctx.dispatch_typed_action(Action::Toggle))
            .finish();
        let mut stack = Stack::new().with_child(button);
        if self.open {
            stack.add_positioned_overlay_child(
                ChildView::new(&self.menu).finish(),
                OffsetPositioning::offset_from_parent(
                    vec2f(0., 4.),
                    ParentOffsetBounds::WindowByPosition,
                    ParentAnchor::BottomLeft,
                    ChildAnchor::TopLeft,
                ),
            );
        }
        Container::new(stack.finish()).finish()
    }
}

impl Drop for DiffSelector {
    fn drop(&mut self) {
        if let Some(task) = self.loading.take() {
            task.abort();
        }
    }
}
