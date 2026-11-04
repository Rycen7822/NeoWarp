use warpui::platform::WindowStyle;
use warpui::{App, ReadView, UpdateView};

use super::*;
use crate::chip_configurator::ChipLocation;
use crate::test_util::settings::initialize_settings_for_tests;

#[test]
fn inline_cli_toolbelt_saves_changes_and_tracks_settings() {
    App::test((), |mut app| async move {
        initialize_settings_for_tests(&mut app);
        app.add_singleton_model(|_| Appearance::mock());
        let (_, editor) = app.add_window(WindowStyle::NotStealFocus, CliToolbeltInlineEditor::new);

        app.update_view(&editor, |editor, ctx| {
            editor.handle_action(
                &InlineAction::Chip(ChipConfiguratorAction::RemoveFromUsed {
                    location: ChipLocation::Left { index: 0 },
                }),
                ctx,
            );
        });
        let (mut left, right) = Selection::Default.items();
        left.remove(0);
        app.read(|ctx| {
            assert_eq!(
                *SessionSettings::as_ref(ctx).cli_agent_footer_chip_selection,
                Selection::Custom { left, right },
            );
        });

        let empty = Selection::Custom {
            left: vec![],
            right: vec![],
        };
        SessionSettings::handle(&app).update(&mut app, |settings, ctx| {
            settings
                .cli_agent_footer_chip_selection
                .set_value(empty.clone(), ctx)
                .unwrap();
        });
        app.read_view(&editor, |editor, _| {
            assert_eq!(selection(&editor.chips), empty)
        });

        app.update_view(&editor, |editor, ctx| {
            editor.handle_action(&InlineAction::Reset, ctx)
        });
        app.read(|ctx| {
            assert_eq!(
                *SessionSettings::as_ref(ctx).cli_agent_footer_chip_selection,
                Selection::Default,
            );
        });
        app.read_view(&editor, |editor, _| {
            assert_eq!(selection(&editor.chips), Selection::Default)
        });
    });
}
