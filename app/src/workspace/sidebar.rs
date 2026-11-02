use warpui::accessibility::{SemanticId, Semantics, accessible};
use warpui::elements::{
    Border, ChildView, ConstrainedBox, Container, CrossAxisAlignment, Expanded, Flex,
    ParentElement, Text,
};
use warpui::ui_components::components::UiComponent;
use warpui::{AppContext, Element, SingletonEntity, ViewHandle};

use super::WorkspaceAction;
use crate::appearance::Appearance;
use crate::file_browser::FileBrowserView;
use crate::project_sessions::view::ProjectsView;
use crate::ui_components::buttons::{close_button, icon_button};
use crate::ui_components::icons::Icon;

#[derive(Default)]
pub(crate) struct HeaderState {
    files_id: SemanticId,
    projects_id: SemanticId,
    close_id: SemanticId,
    files: warpui::elements::MouseStateHandle,
    projects: warpui::elements::MouseStateHandle,
    close: warpui::elements::MouseStateHandle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SidebarTab {
    Files,
    Projects,
}

pub(super) enum Sidebar {
    Closed(SidebarTab),
    Files(ViewHandle<FileBrowserView>),
    Projects(ViewHandle<ProjectsView>),
}

impl Sidebar {
    pub(super) fn tab(&self) -> SidebarTab {
        match self {
            Self::Closed(tab) => *tab,
            Self::Files(_) => SidebarTab::Files,
            Self::Projects(_) => SidebarTab::Projects,
        }
    }

    pub(super) fn is_open(&self) -> bool {
        !matches!(self, Self::Closed(_))
    }

    pub(super) fn render(&self) -> Option<Box<dyn Element>> {
        match self {
            Self::Closed(_) => None,
            Self::Files(view) => Some(ChildView::new(view).finish()),
            Self::Projects(view) => Some(ChildView::new(view).finish()),
        }
    }
}

pub(crate) fn header(tab: SidebarTab, state: &HeaderState, app: &AppContext) -> Box<dyn Element> {
    let appearance = Appearance::as_ref(app);
    let theme = appearance.theme();
    let mut tabs = Flex::row().with_cross_axis_alignment(CrossAxisAlignment::Center);
    for (target, label, icon, mouse, id) in [
        (
            SidebarTab::Files,
            "Files",
            Icon::Folder,
            &state.files,
            state.files_id,
        ),
        (
            SidebarTab::Projects,
            "Projects",
            Icon::Terminal,
            &state.projects,
            state.projects_id,
        ),
    ] {
        let tooltip = appearance
            .ui_builder()
            .tool_tip(label.to_owned())
            .build()
            .finish();
        tabs.add_child(accessible(
            icon_button(appearance, icon, tab == target, mouse.clone())
                .with_tooltip(move || tooltip)
                .build()
                .on_click(move |ctx, _, _| {
                    ctx.dispatch_typed_action(WorkspaceAction::SelectSidebarTab(target));
                })
                .finish(),
            app,
            Semantics::new(id, label).selected(tab == target),
            WorkspaceAction::SelectSidebarTab(target),
        ));
    }
    tabs.add_child(
        Container::new(
            Text::new_inline(
                match tab {
                    SidebarTab::Files => "Files",
                    SidebarTab::Projects => "Projects",
                },
                appearance.ui_font_family(),
                13.,
            )
            .with_color(theme.sub_text_color(theme.background()).into())
            .finish(),
        )
        .with_padding_left(8.)
        .finish(),
    );
    let row = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_child(Expanded::new(1., tabs.finish()).finish())
        .with_child(accessible(
            close_button(appearance, state.close.clone())
                .build()
                .on_click(|ctx, _, _| {
                    ctx.dispatch_typed_action(WorkspaceAction::ToggleFileBrowser);
                })
                .finish(),
            app,
            Semantics::new(state.close_id, "Close sidebar"),
            WorkspaceAction::ToggleFileBrowser,
        ))
        .finish();
    Container::new(ConstrainedBox::new(row).with_height(38.).finish())
        .with_border(Border::bottom(1.).with_border_fill(theme.outline()))
        .with_margin_bottom(8.)
        .finish()
}
