use std::cell::RefCell;
use std::rc::Rc;

use settings::Setting;
use warpui::{App, TypedActionView, View};

use super::*;
use crate::terminal::view::Event;
use crate::test_util::add_window_with_terminal;
use crate::test_util::terminal::initialize_app_for_terminal_view;

#[test]
fn cli_toolbelt_preserves_draft_and_submits_codex_text_without_running_a_shell_command() {
    App::test((), |mut app| async move {
        initialize_app_for_terminal_view(&mut app);
        app.add_singleton_model(crate::settings::import::model::ImportedConfigModel::new);
        let terminal = add_window_with_terminal(&mut app, None);
        let writes = Rc::new(RefCell::new(Vec::new()));
        let observed = writes.clone();
        app.update(|ctx| {
            ctx.subscribe_to_view(&terminal, move |_, event, _| {
                if let Event::WriteBytesToPty { bytes } = event {
                    observed.borrow_mut().push(bytes.to_vec());
                }
            })
        });
        let block = terminal.update(&mut app, |view, ctx| {
            assert!(view.cli_toolbelt.is_none());
            assert!(
                crate::project_sessions::command::parse(
                    "git status",
                    warp_terminal::shell::ShellType::Bash
                )
                .is_none()
            );
            let provider = crate::project_sessions::command::parse(
                "codex",
                warp_terminal::shell::ShellType::Bash,
            )
            .unwrap()
            .provider;
            assert_eq!(provider, Provider::Codex);
            let block = {
                let mut model = view.model.lock();
                model.simulate_long_running_block("codex", "");
                model.active_block_id().clone()
            };
            view.start_cli_toolbelt(block.clone(), provider, ctx);
            assert!(view.cli_toolbelt.as_ref().unwrap().editor.is_none());
            assert!(view.should_subscribe_to_git_status(ctx));
            assert!(view.keymap_context(ctx).set.contains("CliAgentToolbelt"));
            view.show_cli_toolbelt_menu(warpui::geometry::vector::vec2f(10., 10.), ctx);
            assert!(matches!(
                view.context_menu_state
                    .as_ref()
                    .map(|state| state.menu_type),
                Some(ContextMenuType::CliToolbelt { .. })
            ));
            assert!(matches!(
                view.context_menu
                    .as_ref(ctx)
                    .items()
                    .last()
                    .unwrap()
                    .item_on_select_action(),
                Some(TerminalAction::EditCliToolbelt)
            ));
            view.close_context_menu(ctx, false);
            view.handle_action(&TerminalAction::ToggleCliRichInput, ctx);
            let editor = view.cli_toolbelt.as_ref().unwrap().editor.clone().unwrap();
            editor.update(ctx, |editor, ctx| {
                editor.system_reset_buffer_text("first\nsecond", ctx)
            });
            view.toggle_cli_rich_input(ctx);
            assert!(view.cli_toolbelt.as_ref().unwrap().editor.is_none());
            view.toggle_cli_rich_input(ctx);
            let editor = view.cli_toolbelt.as_ref().unwrap().editor.as_ref().unwrap();
            assert_eq!(editor.as_ref(ctx).buffer_text(ctx), "first\nsecond");
            view.submit_cli_rich_input(ctx);
            block
        });
        SessionSettings::handle(&app).update(&mut app, |settings, ctx| {
            settings
                .cli_agent_footer_chip_selection
                .set_value(
                    crate::terminal::cli_toolbelt_config::CLIAgentToolbarChipSelection::Custom {
                        left: vec![Item::ContextChip(ContextChipKind::WorkingDirectory)],
                        right: vec![],
                    },
                    ctx,
                )
                .unwrap();
        });
        terminal.update(&mut app, |view, ctx| {
            assert!(view.cli_toolbelt.as_ref().unwrap().editor.is_none());
            assert!(!view.keymap_context(ctx).set.contains("CliAgentToolbelt"));
            view.end_cli_toolbelt(Some(&block), ctx);
            assert!(view.cli_toolbelt.is_none());
        });
        assert_eq!(
            *writes.borrow(),
            vec![b"\x1b[200~first\nsecond\x1b[201~".to_vec(), vec![b'\r']]
        );
    });
}

#[test]
fn cli_toolbelt_does_not_send_a_delayed_enter_or_draft_after_cli_exit() {
    App::test((), |mut app| async move {
        initialize_app_for_terminal_view(&mut app);
        let terminal = add_window_with_terminal(&mut app, None);
        let writes = Rc::new(RefCell::new(Vec::new()));
        let observed = writes.clone();
        app.update(|ctx| {
            ctx.subscribe_to_view(&terminal, move |_, event, _| {
                if let Event::WriteBytesToPty { bytes } = event {
                    observed.borrow_mut().push(bytes.to_vec());
                }
            })
        });
        terminal.update(&mut app, |view, ctx| {
            let block = {
                let mut model = view.model.lock();
                model.simulate_long_running_block("claude", "");
                model.active_block_id().clone()
            };
            view.start_cli_toolbelt(block.clone(), Provider::Claude, ctx);
            view.toggle_cli_rich_input(ctx);
            let editor = view.cli_toolbelt.as_ref().unwrap().editor.as_ref().unwrap();
            editor.update(ctx, |editor, ctx| {
                editor.system_reset_buffer_text("review only", ctx)
            });
            view.submit_cli_rich_input(ctx);
            view.end_cli_toolbelt(Some(&block), ctx);
            // Exercise the timer's real callback after its originating CLI has ended.
            view.finish_cli_submission(&block, ctx);
            view.submit_cli_rich_input(ctx);
        });
        assert_eq!(*writes.borrow(), vec![b"review only".to_vec()]);
    });
}
