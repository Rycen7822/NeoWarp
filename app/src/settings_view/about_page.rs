use warpui::assets::asset_cache::AssetSource;
use warpui::elements::{
    Align, CacheOption, ConstrainedBox, Container, CrossAxisAlignment, Element, Flex, Image,
    MainAxisAlignment, MouseStateHandle, ParentElement, Text, Wrap,
};
use warpui::fonts::{Properties, Weight};
use warpui::ui_components::components::UiComponent;
use warpui::{AppContext, Entity, TypedActionView, View, ViewContext, ViewHandle};

use super::SettingsSection;
use super::settings_page::{
    MatchData, PageType, SettingsPageEvent, SettingsPageMeta, SettingsPageViewHandle,
    SettingsWidget,
};
use crate::appearance::Appearance;
use crate::channel::ChannelState;
use crate::localization::tr;
use crate::workspace::WorkspaceAction;

pub struct AboutPageView {
    page: PageType<Self>,
}

impl AboutPageView {
    pub fn new(_ctx: &mut ViewContext<AboutPageView>) -> Self {
        AboutPageView {
            page: PageType::new_monolith(AboutPageWidget::default(), None, false),
        }
    }
}

impl Entity for AboutPageView {
    type Event = SettingsPageEvent;
}
impl TypedActionView for AboutPageView {
    type Action = ();
}

impl View for AboutPageView {
    fn ui_name() -> &'static str {
        "AboutPage"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        self.page.render(self, app)
    }
}

#[derive(Default)]
struct AboutPageWidget {
    copy_version_button_mouse_state: MouseStateHandle,
    neowarp_source_mouse_state: MouseStateHandle,
    warp_source_mouse_state: MouseStateHandle,
}

impl SettingsWidget for AboutPageWidget {
    type View = AboutPageView;

    fn search_terms(&self) -> &str {
        "about neowarp warp version license source 关于 版本 开源 许可证"
    }

    fn render(
        &self,
        _view: &AboutPageView,
        appearance: &Appearance,
        _app: &AppContext,
    ) -> Box<dyn Element> {
        let theme = appearance.theme();
        let ui_builder = appearance.ui_builder();

        let version = ChannelState::app_version().unwrap_or("v#.##.###");

        let version_text = ui_builder
            .span(version.to_string())
            .with_soft_wrap()
            .build()
            .with_margin_top(16.)
            .finish();

        let copy_version_icon = appearance
            .ui_builder()
            .copy_button(16., self.copy_version_button_mouse_state.clone())
            .build()
            .on_click(move |ctx, _, _| {
                ctx.dispatch_typed_action(WorkspaceAction::CopyVersion(version));
            })
            .finish();

        let version_row = Wrap::row()
            .with_main_axis_alignment(MainAxisAlignment::Center)
            .with_children([
                version_text,
                Container::new(copy_version_icon)
                    .with_margin_top(16.)
                    .with_padding_left(6.)
                    .finish(),
            ]);

        Align::new(
            ConstrainedBox::new(Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(
                    Flex::row()
                        .with_main_axis_alignment(MainAxisAlignment::Center)
                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                        .with_spacing(16.)
                        .with_child(
                            ConstrainedBox::new(
                                Image::new(
                                    AssetSource::Bundled { path: "bundled/svg/neowarp-icon.svg" },
                                    CacheOption::BySize,
                                ).finish(),
                            )
                            .with_width(72.)
                            .with_height(72.)
                            .finish(),
                        )
                        .with_child(
                            Text::new_inline("NeoWarp", appearance.ui_font_family(), appearance.scaled_ui_size(36.))
                                .with_style(Properties::default().weight(Weight::Semibold))
                                .with_color(theme.active_ui_text_color().into())
                                .finish(),
                        )
                        .finish(),
                )
                .with_child(version_row.finish())
                .with_child(
                    ui_builder
                        .span(tr("NeoWarp is based on the open-source Warp terminal."))
                        .with_soft_wrap()
                        .build()
                        .with_margin_top(16.)
                        .finish(),
                )
                .with_child(
                    ui_builder
                        .span(tr("License: AGPL-3.0-only. UI frameworks warpui and warpui_core: MIT."))
                        .with_soft_wrap()
                        .build()
                        .with_margin_top(8.)
                        .finish(),
                )
                .with_child(
                    Flex::row()
                        .with_main_axis_alignment(MainAxisAlignment::Center)
                        .with_spacing(20.)
                        .with_child(
                            ui_builder.link(
                                tr("NeoWarp source").to_owned(),
                                Some("https://github.com/Rycen7822/NeoWarp".to_owned()),
                                None,
                                self.neowarp_source_mouse_state.clone(),
                            ).build().finish(),
                        )
                        .with_child(
                            ui_builder.link(
                                tr("Warp upstream source").to_owned(),
                                Some("https://github.com/warpdotdev/warp".to_owned()),
                                None,
                                self.warp_source_mouse_state.clone(),
                            ).build().finish(),
                        )
                        .finish(),
                )
                .with_child(
                    ui_builder
                        .span(tr("Original work © 2020–2026 Denver Technologies, Inc."))
                        .with_soft_wrap()
                        .build()
                        .with_margin_top(16.)
                        .finish(),
                )
                .finish())
                .with_max_width(680.)
                .finish(),
        )
        .finish()
    }
}

impl SettingsPageMeta for AboutPageView {
    fn section() -> SettingsSection {
        SettingsSection::About
    }

    fn should_render(&self, _ctx: &AppContext) -> bool {
        true
    }

    fn update_filter(&mut self, query: &str, ctx: &mut ViewContext<Self>) -> MatchData {
        self.page.update_filter(query, ctx)
    }

    fn scroll_to_widget(&mut self, widget_id: &'static str) {
        self.page.scroll_to_widget(widget_id)
    }

    fn clear_highlighted_widget(&mut self) {
        self.page.clear_highlighted_widget();
    }
}

impl From<ViewHandle<AboutPageView>> for SettingsPageViewHandle {
    fn from(view_handle: ViewHandle<AboutPageView>) -> Self {
        SettingsPageViewHandle::About(view_handle)
    }
}
