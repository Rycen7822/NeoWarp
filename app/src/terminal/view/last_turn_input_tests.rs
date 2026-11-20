use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io::Write;
use std::rc::Rc;

use warpui::App;

use super::*;
use crate::project_sessions::config::{Host, SavedShell};
use crate::project_sessions::{Observation, command as cli_command};
use crate::test_util::assert_eventually;
use crate::test_util::terminal::{add_window_with_terminal, initialize_app_for_terminal_view};

#[test]
fn native_tool_diffs_never_hold_terminal_input_or_scan_workspace() {
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
        let append = |message: serde_json::Value| {
            let record = serde_json::json!({"type":"message", "timestamp":chrono::Utc::now().to_rfc3339(), "message": message});
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
            view.handle_action(
                &TerminalAction::SubmitControlSequence(b"\x1b[13u".to_vec()),
                ctx,
            );
            view.write_user_bytes_to_pty(b"x".to_vec(), ctx);
            view.write_user_bytes_to_pty(vec![3], ctx);
        });
        assert_eq!(
            *writes.borrow(),
            [b"\x1b[13u".to_vec(), b"x".to_vec(), vec![3]],
            "PTY input must be emitted before any background capture can complete"
        );
        let model = terminal
            .read(&app, |view, _| view.last_turn.clone())
            .unwrap();
        append(
            serde_json::json!({"role":"user", "timestamp":crate::code_review::last_turn::now(), "content":[{"type":"text","text":"request"}]}),
        );
        append(serde_json::json!({"role":"assistant", "content":[{
            "type":"toolCall", "id":"edit-1", "name":"edit",
            "arguments":{"path":"ignored.txt","oldText":"before","newText":"after"}
        }]}));
        append(
            serde_json::json!({"role":"toolResult", "toolCallId":"edit-1", "isError":false, "details":{"diff":"-1 before\n+1 after"}, "content":[]}),
        );
        append(serde_json::json!({
            "role":"toolResult", "toolName":"codemode", "toolCallId":"parent", "isError":true,
            "nestedCalls":{"calls":[
                {"name":"edit", "status":"ok", "arguments":{
                    "path":"nested.rs", "edits":[{"oldText":"old", "newText":"new"}]
                }},
                {"name":"edit", "status":"error", "arguments":{
                    "path":"failed.rs", "edits":[{"oldText":"old", "newText":"new"}]
                }}
            ]}
        }));
        std::fs::write(root.join("ignored.txt"), "external modification after edit").unwrap();
        std::fs::write(root.join("script-only.txt"), "not a captured tool diff").unwrap();
        assert_eventually!(3000 => model.read(&app, |model, _| model.changes().count() == 2), "tool diff did not arrive");
        let change = model
            .read(&app, |model, _| model.changes().next().cloned())
            .unwrap();
        assert_eq!(change.path, "ignored.txt");
        assert_eq!(change.diff, "-1 before\n+1 after");
        assert_eq!((change.additions, change.deletions), (1, 1));

        append(
            serde_json::json!({"role":"user", "content":[{"type":"text","text":"next request"}]}),
        );
        assert_eventually!(3000 => model.read(&app, |model, _| {
            model.changes().count() == 2 && model.label().contains(crate::localization::tr("Previous turn"))
        }), "starting a request erased the previous edits");
        append(
            serde_json::json!({"role":"toolResult", "nestedCalls":{"calls":[
                {"id":"last", "name":"edit", "status":"ok", "arguments":{
                    "path":"last.txt", "oldText":"old", "newText":"new"
                }}
            ]}}),
        );
        terminal.update(&mut app, |view, ctx| {
            view.handle_action(&TerminalAction::SubmitKeyDown("\r".into()), ctx);
            view.shutdown_pty(ctx);
        });
        assert_eq!(writes.borrow().last().unwrap(), b"\r");
        assert!(terminal.read(&app, |view, _| view.last_turn.is_none()));
        assert_eventually!(3000 => model.read(&app, |model, _| {
            model.changes().count() == 1
                && model.change("last.txt").is_some()
                && model.label() == crate::localization::tr("Session ended")
        }), "the final tool result was lost when the CLI exited");
    });
}
