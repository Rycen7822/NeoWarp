use warpui::elements::{
    ChildView, Container, CrossAxisAlignment, DispatchEventResult, EventHandler, Expanded, Flex,
    Padding, ParentElement, Text,
};
use warpui::ui_components::components::UiComponent;
use warpui::{AppContext, Element, SingletonEntity, ViewHandle};

use super::WorkspaceAction;
use crate::appearance::Appearance;
use crate::file_browser::FileBrowserView;
use crate::project_sessions::view::ProjectsView;
use crate::ui_components::buttons::close_button;

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

pub(crate) fn header(
    tab: SidebarTab,
    close: warpui::elements::MouseStateHandle,
    app: &AppContext,
) -> Box<dyn Element> {
    let appearance = Appearance::as_ref(app);
    let theme = appearance.theme();
    let mut tabs = Flex::row().with_cross_axis_alignment(CrossAxisAlignment::Center);
    for (target, label) in [
        (SidebarTab::Files, "Files"),
        (SidebarTab::Projects, "Projects"),
    ] {
        let mut item = Container::new(
            Text::new_inline(label, appearance.ui_font_family(), 13.)
                .with_color(theme.main_text_color(theme.background()).into())
                .finish(),
        )
        .with_padding(Padding::uniform(6.));
        if tab == target {
            item = item.with_background(theme.surface_2());
        }
        tabs.add_child(
            EventHandler::new(item.finish())
                .on_left_mouse_down(move |ctx, _, _| {
                    ctx.dispatch_typed_action(WorkspaceAction::SelectSidebarTab(target));
                    DispatchEventResult::StopPropagation
                })
                .finish(),
        );
    }
    Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_child(Expanded::new(1., tabs.finish()).finish())
        .with_child(
            close_button(appearance, close)
                .build()
                .on_click(|ctx, _, _| {
                    ctx.dispatch_typed_action(WorkspaceAction::ToggleFileBrowser);
                })
                .finish(),
        )
        .finish()
}
