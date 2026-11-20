use std::io::Write;

use serde_json::json;

use super::*;
use crate::project_sessions::command;
use crate::project_sessions::config::Host;

#[test]
fn pi_reader_keeps_progress_across_bad_lines_and_transcript_replacement() {
    let make_edit = |id: &str, path: &str| {
        json!({"message":{
            "role":"toolResult","nestedCalls":{"calls":[
                {"id":id,"name":"edit","status":"ok","arguments":{"path":path,"oldText":"before","newText":"after"}}
            ]}
        }})
    };
    let mut file = tempfile::NamedTempFile::new().unwrap();
    let path = file.path().to_owned();
    writeln!(file, "{}", make_edit("old/1", "old.txt")).unwrap();
    let (cursor, first) = read(&path, Provider::Pi, None).unwrap();
    assert!(matches!(&first[..], [Observation::Change(change)] if change.path == "old.txt"));

    writeln!(file, "{{bad record}}\n{}", make_edit("old/2", "next.txt")).unwrap();
    let (cursor, next) = read(&path, Provider::Pi, Some(cursor)).unwrap();
    assert!(
        matches!(&next[..], [Observation::Incomplete, Observation::Change(change)] if change.path == "next.txt")
    );

    std::fs::write(&path, format!("{}\n", make_edit("new/1", "new.txt"))).unwrap();
    let (_, replaced) = read(&path, Provider::Pi, Some(cursor)).unwrap();
    assert!(
        matches!(&replaced[..], [Observation::Reset, Observation::Change(change)] if change.path == "new.txt")
    );
}

#[test]
fn pi_session_selector_prefers_exact_id_and_rejects_ambiguous_prefix() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let store = root.join(".pi/agent/sessions");
    std::fs::create_dir_all(&store).unwrap();
    let transcript = |id: &str| {
        let path = store.join(format!("{id}.jsonl"));
        let record =
            json!({"type":"session", "cwd":root, "id":id, "timestamp":"2026-01-01T00:00:00Z"});
        std::fs::write(&path, format!("{record}\n")).unwrap();
        path
    };
    let first = transcript("a12");
    let locate = |selector: &str| {
        let parsed = command::parse(
            &format!("pi --session {selector}"),
            warp_terminal::shell::ShellType::Bash,
        )
        .unwrap();
        Source {
            block: BlockId::default(),
            query: Query {
                host: Host::Local,
                provider: parsed.provider,
                directory: root.to_string_lossy().into_owned(),
                home: root.to_string_lossy().into_owned(),
                environment: Default::default(),
                started_at: 0,
                resuming: parsed.resuming,
            },
            identity: parsed.identity,
            ambiguous: false,
        }
        .locate()
        .map(|path| path.map(|path| path.canonicalize().unwrap()))
    };
    assert_eq!(locate("a1").unwrap(), Some(first.canonicalize().unwrap()));
    transcript("a123");
    assert!(locate("a1").is_err());
    assert_eq!(locate("a12").unwrap(), Some(first.canonicalize().unwrap()));
}
