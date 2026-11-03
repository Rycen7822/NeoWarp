use settings::Setting;
use warpui::platform::WindowStyle;
use warpui::{App, ReadView, SingletonEntity};

use super::*;
use crate::test_util::settings::initialize_settings_for_tests;

#[test]
fn inline_editor_refreshes_when_vertical_tabs_are_enabled() {
    App::test((), |mut app| async move {
        let _vertical_tabs = crate::features::FeatureFlag::VerticalTabs.override_enabled(true);
        initialize_settings_for_tests(&mut app);
        app.add_singleton_model(|_| Appearance::mock());
        let (_, editor) =
            app.add_window(WindowStyle::NotStealFocus, HeaderToolbarInlineEditor::new);
        app.read_view(&editor, |editor, _| {
            assert_eq!(
                current_toolbar_items(&editor.chip_configurator),
                (
                    vec![HeaderToolbarItemKind::Files],
                    vec![HeaderToolbarItemKind::CodeReview]
                )
            );
        });

        TabSettings::handle(&app).update(&mut app, |settings, ctx| {
            settings.use_vertical_tabs.set_value(true, ctx).unwrap();
        });

        app.read_view(&editor, |editor, _| {
            assert_eq!(
                current_toolbar_items(&editor.chip_configurator),
                (
                    vec![
                        HeaderToolbarItemKind::TabsPanel,
                        HeaderToolbarItemKind::Files
                    ],
                    vec![HeaderToolbarItemKind::CodeReview]
                )
            );
        });
    });
}
