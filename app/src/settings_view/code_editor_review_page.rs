//! Basic editor settings, shown under the Code umbrella.

use warp_core::settings::{Setting as _, ToggleableSetting as _};
use warp_errors::report_if_error;
use warpui::elements::Element;
#[cfg(feature = "local_fs")]
use warpui::elements::{ChildView, Empty};
use warpui::ui_components::components::UiComponent;
use warpui::ui_components::switch::SwitchStateHandle;
use warpui::{AppContext, Entity, SingletonEntity, TypedActionView, View, ViewContext, ViewHandle};

#[cfg(feature = "local_fs")]
use super::features::external_editor::ExternalEditorView;
use super::settings_page::{
    MatchData, PageTitle, PageType, SettingsPageMeta, SettingsPageViewHandle, SettingsWidget,
    render_body_item, render_dropdown_item,
};
use super::{SettingsSection, ToggleState};
use crate::appearance::Appearance;
use crate::settings::{AppEditorSettings, CodeEditorLineNumberMode, CodeSettings};
use crate::view_components::{Dropdown, DropdownItem};

const PAGE_TITLE: &str = "Editor";

pub struct EditorAndCodeReviewPageView {
    page: PageType<Self>,
    #[cfg(feature = "local_fs")]
    external_editor_view: Option<ViewHandle<ExternalEditorView>>,
    code_editor_line_number_mode_dropdown: ViewHandle<Dropdown<EditorAndCodeReviewPageAction>>,
}

impl EditorAndCodeReviewPageView {
    pub fn new(ctx: &mut ViewContext<Self>) -> Self {
        #[cfg(feature = "local_fs")]
        let external_editor_view = Some(ctx.add_typed_action_view(ExternalEditorView::new));

        let code_editor_line_number_mode_dropdown = ctx.add_typed_action_view(Dropdown::new);
        Self::update_code_editor_line_number_mode_dropdown(
            code_editor_line_number_mode_dropdown.clone(),
            ctx,
        );

        ctx.subscribe_to_model(&AppEditorSettings::handle(ctx), |me, _, _, ctx| {
            Self::update_code_editor_line_number_mode_dropdown(
                me.code_editor_line_number_mode_dropdown.clone(),
                ctx,
            );
            ctx.notify();
        });

        Self {
            page: Self::build_page(ctx),
            #[cfg(feature = "local_fs")]
            external_editor_view,
            code_editor_line_number_mode_dropdown,
        }
    }

    fn build_page(ctx: &mut ViewContext<Self>) -> PageType<Self> {
        #[cfg(feature = "local_fs")]
        let mut widgets: Vec<Box<dyn SettingsWidget<View = Self>>> =
            vec![Box::new(ExternalEditorCodeWidget)];
        #[cfg(not(feature = "local_fs"))]
        let mut widgets: Vec<Box<dyn SettingsWidget<View = Self>>> = vec![];

        if AppEditorSettings::as_ref(ctx)
            .code_editor_line_number_mode
            .is_supported_on_current_platform()
        {
            widgets.push(Box::new(CodeEditorLineNumberModeWidget::default()));
        }

        widgets.push(Box::new(AutoSaveToggleWidget::default()));

        PageType::new_uncategorized(widgets, Some(PageTitle::new(PAGE_TITLE)))
    }

    fn update_code_editor_line_number_mode_dropdown(
        dropdown: ViewHandle<Dropdown<EditorAndCodeReviewPageAction>>,
        ctx: &mut ViewContext<Self>,
    ) {
        dropdown.update(ctx, |dropdown, ctx| {
            let values = [
                CodeEditorLineNumberMode::Absolute,
                CodeEditorLineNumberMode::Relative,
            ];

            let current_value = *AppEditorSettings::as_ref(ctx)
                .code_editor_line_number_mode
                .value();

            let selected_index = values
                .iter()
                .position(|val| *val == current_value)
                .unwrap_or(0);

            dropdown.set_items(
                values
                    .into_iter()
                    .map(|val| {
                        DropdownItem::localized(
                            val.dropdown_item_label(),
                            EditorAndCodeReviewPageAction::SetCodeEditorLineNumberMode(val),
                        )
                    })
                    .collect(),
                ctx,
            );
            dropdown.set_selected_by_index(selected_index, ctx);
        });
    }
}

impl Entity for EditorAndCodeReviewPageView {
    type Event = ();
}

impl View for EditorAndCodeReviewPageView {
    fn ui_name() -> &'static str {
        "EditorAndCodeReviewPage"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        self.page.render(self, app)
    }
}

#[allow(clippy::enum_variant_names)]
#[derive(Debug, Clone, PartialEq)]
pub enum EditorAndCodeReviewPageAction {
    ToggleAutoSave,
    SetCodeEditorLineNumberMode(CodeEditorLineNumberMode),
}

impl TypedActionView for EditorAndCodeReviewPageView {
    type Action = EditorAndCodeReviewPageAction;

    fn handle_action(&mut self, action: &Self::Action, ctx: &mut ViewContext<Self>) {
        match action {
            EditorAndCodeReviewPageAction::ToggleAutoSave => {
                CodeSettings::handle(ctx).update(ctx, |settings, ctx| {
                    report_if_error!(settings.auto_save.toggle_and_save_value(ctx));
                });
                ctx.notify();
            }
            EditorAndCodeReviewPageAction::SetCodeEditorLineNumberMode(mode) => {
                AppEditorSettings::handle(ctx).update(ctx, |editor_settings, ctx| {
                    report_if_error!(
                        editor_settings
                            .code_editor_line_number_mode
                            .set_value(*mode, ctx)
                    );
                    ctx.notify();
                });
            }
        }
    }
}

impl SettingsPageMeta for EditorAndCodeReviewPageView {
    fn section() -> SettingsSection {
        SettingsSection::EditorAndCodeReview
    }

    fn update_filter(&mut self, query: &str, ctx: &mut ViewContext<Self>) -> MatchData {
        self.page.update_filter(query, ctx)
    }

    fn should_render(&self, _ctx: &AppContext) -> bool {
        true
    }

    fn scroll_to_widget(&mut self, widget_id: &'static str) {
        self.page.scroll_to_widget(widget_id)
    }

    fn clear_highlighted_widget(&mut self) {
        self.page.clear_highlighted_widget();
    }
}

impl From<ViewHandle<EditorAndCodeReviewPageView>> for SettingsPageViewHandle {
    fn from(view_handle: ViewHandle<EditorAndCodeReviewPageView>) -> Self {
        SettingsPageViewHandle::EditorAndCodeReview(view_handle)
    }
}

#[cfg(feature = "local_fs")]
struct ExternalEditorCodeWidget;

#[cfg(feature = "local_fs")]
impl SettingsWidget for ExternalEditorCodeWidget {
    type View = EditorAndCodeReviewPageView;

    fn search_terms(&self) -> &str {
        "code editor open files markdown notebook configuration layout pane tab sidebar system default application preview right"
    }

    fn render(
        &self,
        view: &Self::View,
        _appearance: &Appearance,
        _app: &AppContext,
    ) -> Box<dyn Element> {
        if let Some(editor_view) = &view.external_editor_view {
            ChildView::new(editor_view).finish()
        } else {
            Empty::new().finish()
        }
    }
}

#[derive(Default)]
struct AutoSaveToggleWidget {
    switch_state: SwitchStateHandle,
}

impl SettingsWidget for AutoSaveToggleWidget {
    type View = EditorAndCodeReviewPageView;

    fn search_terms(&self) -> &str {
        "auto save autosave automatically save editor files on type focus"
    }

    fn render(
        &self,
        _view: &Self::View,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let code_settings = CodeSettings::as_ref(app);

        render_body_item::<EditorAndCodeReviewPageAction>(
            "Auto save".into(),
            None,
            ToggleState::Enabled,
            appearance,
            appearance
                .ui_builder()
                .switch(self.switch_state.clone())
                .check(*code_settings.auto_save)
                .build()
                .on_click(move |ctx, _, _| {
                    ctx.dispatch_typed_action(EditorAndCodeReviewPageAction::ToggleAutoSave);
                })
                .finish(),
            Some(
                "Automatically saves changes in the Warp text editor as you type and when the editor loses focus."
                    .into(),
            ),
        )
    }
}

#[derive(Default)]
struct CodeEditorLineNumberModeWidget {}

impl SettingsWidget for CodeEditorLineNumberModeWidget {
    type View = EditorAndCodeReviewPageView;

    fn search_terms(&self) -> &str {
        "line number numbers relative line vim gutter code editor"
    }

    fn render(
        &self,
        view: &Self::View,
        appearance: &Appearance,
        _app: &AppContext,
    ) -> Box<dyn Element> {
        render_dropdown_item(
            appearance,
            "Code editor line numbers:",
            None,
            None,
            None,
            &view.code_editor_line_number_mode_dropdown,
        )
    }
}
