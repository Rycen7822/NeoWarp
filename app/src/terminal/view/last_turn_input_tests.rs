use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;
use std::rc::Rc;

use warpui::App;
use wsl_git_watcher::snapshot::ChangeKind;

use super::*;
use crate::project_sessions::{
    Observation, command as cli_command,
    config::{Host, SavedShell},
};
use crate::test_util::{
    assert_eventually,
    terminal::{add_window_with_terminal, initialize_app_for_terminal_view},
};

#[test]
fn native_capture_keeps_approvals_out_of_the_baseline_and_cancels_pending_input() {
    App::test((), |mut app| async move {
        initialize_app_for_terminal_view(&mut app);
        let sessions = app.add_singleton_model(ProjectSessions::new);
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repo");
        std::fs::create_dir(&root).unwrap();
        let mut git = command::blocking::Command::new("git");
        #[cfg(windows)]
        git.creation_flags(0);
        assert!(
            git.current_dir(&root)
                .args(["init", "--quiet", "--template="])
                .status()
                .unwrap()
                .success()
        );
        std::fs::write(root.join(".gitignore"), "ignored.txt\n").unwrap();
        std::fs::write(root.join("ignored.txt"), "before").unwrap();
        let transcript = directory.path().join("session.jsonl");
        std::fs::write(&transcript, "").unwrap();
        let cli = format!("pi --session \"{}\"", transcript.display());
        let terminal = add_window_with_terminal(&mut app, None);
        let observe = |app: &mut App| {
            terminal.update(app, |view, ctx| {
                {
                    let mut model = view.model.lock();
                    if model.block_list().active_block().is_executing() {
                        model.finish_block();
                    }
                    model.simulate_long_running_block(cli.as_str(), "");
                }
                view.current_repo_path = Some(LocalOrRemotePath::Local(root.clone()));
                let observation = Observation {
                    terminal: ctx.handle(),
                    block: view.model.lock().active_block_id().clone(),
                    host: Host::Local,
                    directory: root.to_string_lossy().into_owned(),
                    home: directory.path().to_string_lossy().into_owned(),
                    shell: SavedShell {
                        program: "powershell".into(),
                        kind: ShellType::PowerShell,
                    },
                    command: cli.clone(),
                    environment: BTreeMap::new(),
                    parsed: cli_command::parse(&cli, ShellType::PowerShell),
                    launched: false,
                    started_at: crate::code_review::last_turn::now() as i64,
                };
                ProjectSessions::handle(ctx)
                    .update(ctx, |sessions, ctx| sessions.observe(observation, ctx));
            });
        };
        observe(&mut app);
        assert_eventually!(2000 => sessions.read(&app, |sessions, _| sessions.turn_source(terminal.id()).is_some()), "CLI source did not bind");
        let writes = Rc::new(RefCell::new(Vec::<Vec<u8>>::new()));
        let sink = writes.clone();
        app.update(|ctx| {
            ctx.subscribe_to_view(&terminal, move |_, event, _| {
                if let Event::WriteBytesToPty { bytes } = event {
                    sink.borrow_mut().push(bytes.to_vec());
                }
            })
        });
        let append_user = || {
            let record = serde_json::json!({"type":"message", "id":"user", "message":{
                "role":"user", "timestamp":crate::code_review::last_turn::now(), "content":[{"type":"text","text":"request"}]}});
            writeln!(
                std::fs::OpenOptions::new()
                    .append(true)
                    .open(&transcript)
                    .unwrap(),
                "{record}"
            )
            .unwrap();
        };
        terminal.update(&mut app, |view, ctx| {
            view.current_repo_path = Some(LocalOrRemotePath::Local(root.clone()));
            view.handle_action(
                &TerminalAction::SubmitControlSequence(b"\x1b[13u".to_vec()),
                ctx,
            );
            view.write_user_bytes_to_pty(b"x".to_vec(), ctx);
            assert!(
                writes.borrow().is_empty(),
                "input escaped before the baseline completed"
            );
        });
        assert_eventually!(4000 => terminal.read(&app, |view, _| view.last_turn_input.is_none()), "baseline capture did not finish");
        assert_eq!(*writes.borrow(), [b"\x1b[13u".to_vec(), b"x".to_vec()]);
        append_user();
        let model = terminal
            .read(&app, |view, _| view.last_turn.clone())
            .unwrap();
        assert_eventually!(2000 => model.read(&app, |model, _| model.confirmed()), "user request was not confirmed");
        std::fs::write(root.join("ignored.txt"), "after").unwrap();
        let preview = model
            .read(&app, |model, _| model.preview())
            .unwrap()
            .await
            .unwrap();
        assert_eq!(preview.files.len(), 1);
        let body = preview
            .read(Path::new("ignored.txt"), ChangeKind::Modified, 1024)
            .await
            .unwrap();
        assert_eq!((&**body.0, body.1.as_str()), ("before", "after"));
        drop(preview);

        // A real read-only request replaces the baseline even without a new diff.
        terminal.update(&mut app, |view, ctx| {
            view.current_repo_path = Some(LocalOrRemotePath::Local(root.clone()));
            view.handle_action(&TerminalAction::SubmitKeyDown("\r".into()), ctx);
        });
        assert_eventually!(4000 => terminal.read(&app, |view, _| view.last_turn_input.is_none()), "second capture did not finish");
        append_user();
        assert_eventually!(2000 => model.read(&app, |model, _| model.confirmed()), "second request was not confirmed");
        assert!(
            model
                .read(&app, |model, _| model.preview())
                .unwrap()
                .await
                .unwrap()
                .files
                .is_empty()
        );
        std::fs::write(root.join("ignored.txt"), "after approval").unwrap();
        for _ in 0..2 {
            terminal.update(&mut app, |view, ctx| {
                view.current_repo_path = Some(LocalOrRemotePath::Local(root.clone()));
                view.handle_action(&TerminalAction::SubmitKeyDown("\r".into()), ctx);
            });
            assert_eventually!(4000 => terminal.read(&app, |view, _| view.last_turn_input.is_none()), "approval capture did not finish");
        }
        assert!(!model.read(&app, |model, _| model.confirmed()));
        let preview = model
            .read(&app, |model, _| model.preview())
            .unwrap()
            .await
            .unwrap();
        let body = preview
            .read(Path::new("ignored.txt"), ChangeKind::Modified, 1024)
            .await
            .unwrap();
        assert_eq!((&**body.0, body.1.as_str()), ("after", "after approval"));
        drop(preview);

        // A new block cannot receive the previous block's delayed Enter.
        observe(&mut app);
        let before_cancel = writes.borrow().len();
        terminal.update(&mut app, |view, ctx| {
            view.current_repo_path = Some(LocalOrRemotePath::Local(root.clone()));
            view.handle_action(&TerminalAction::SubmitKeyDown("\r".into()), ctx);
            assert!(view.last_turn_input.is_some());
            view.write_user_bytes_to_pty(vec![3], ctx);
            assert!(view.last_turn_input.is_none());
        });
        assert_eq!(&writes.borrow()[before_cancel..], &[vec![3]]);
        terminal.update(&mut app, |view, ctx| view.shutdown_pty(ctx));
        assert!(terminal.read(&app, |view, _| view.last_turn.is_none()));
    });
}
