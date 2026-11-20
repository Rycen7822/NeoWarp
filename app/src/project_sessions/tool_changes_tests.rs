use serde_json::json;

use super::*;

#[test]
fn metis_tool_changes_use_mutation_receipts_and_deduplicate_early_edits() {
    let mut tools = Tools::default();
    let edit = json!({"path":"code.rs","edits":[{"oldText":"before","newText":"after"}]});
    let archive = json!({"type":"custom","customType":"context-prune-index","data":{"toolCalls":[{
        "toolCallId":"parent/1","parentToolCallId":"parent","toolName":"edit","isError":false,"archiveOnly":true,"args":edit
    }]}});
    let parent = json!({"message":{"role":"toolResult","toolCallId":"parent","nestedCalls":{"calls":[
        {"id":"parent/1","name":"edit","status":"ok","arguments":edit},
        {"id":"parent/2","name":"edit","status":"ok","arguments":edit}
    ]}}});
    assert_eq!(tools.observe(&archive, Provider::Pi).len(), 1);
    assert_eq!(tools.observe(&parent, Provider::Pi).len(), 1);
    assert!(tools.observe(&archive, Provider::Pi).is_empty());

    tools.observe(&json!({"message":{"role":"assistant","content":[
        {"type":"toolCall","id":"e","name":"edit","arguments":{"path":"code.rs","oldText":"x".repeat(70_000),"newText":"after"}},
        {"type":"toolCall","id":"w","name":"write","arguments":{"path":"doc.md","content":"after"}}
    ]}}), Provider::Pi);
    let edit = json!({"message":{"role":"toolResult","toolCallId":"e","isError":true,"details":{
        "patch":"--- a/code.rs\n+++ b/code.rs\n@@ -1 +1 @@\n-before\n+after\n",
        "metisActionFusion":{"version":1,"mutationStatus":"success","command":{"status":"failed"}}
    }}});
    let changes = tools.observe(&edit, Provider::Pi);
    assert_eq!(changes[0].diff, "@@ -1 +1 @@\n-before\n+after\n");
    let write = json!({"message":{"role":"toolResult","toolCallId":"w","isError":false,"details":{
        "metisWriteDiff":{"kind":"update","rows":[{"kind":"remove","content":"before"},{"kind":"add","content":"after"}]}
    }}});
    let changes = tools.observe(&write, Provider::Pi);
    assert_eq!(
        (
            &*changes[0].path,
            changes[0].additions,
            changes[0].deletions
        ),
        ("doc.md", 1, 1)
    );
}

#[test]
fn only_successful_tools_with_known_differences_are_shown() {
    let mut tools = Tools::default();
    tools.observe(&json!({"message":{"role":"assistant","content":[
        {"type":"toolCall","id":"failed","name":"edit","arguments":{"path":"x.rs","oldText":"old","newText":"new"}},
        {"type":"toolCall","id":"write","name":"write","arguments":{"path":"x.rs","content":"new"}}
    ]}}), Provider::Pi);
    assert!(
        tools
            .observe(
                &json!({"message":{"role":"toolResult","toolCallId":"failed","isError":true}}),
                Provider::Pi
            )
            .is_empty()
    );
    assert!(
        tools
            .observe(
                &json!({"message":{"role":"toolResult","toolCallId":"write","isError":false}}),
                Provider::Pi
            )
            .is_empty()
    );

    tools.observe(&json!({"message":{"role":"assistant","content":[
        {"type":"tool_use","id":"edit","name":"Edit","input":{"file_path":"x.rs","old_string":"old","new_string":"new"}}
    ]}}), Provider::Claude);
    let changes = tools.observe(
        &json!({"message":{"role":"user","content":[
            {"type":"tool_result","tool_use_id":"edit","is_error":false}
        ]}}),
        Provider::Claude,
    );
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].diff, "@@ replacement @@\n-old\n+new\n");

    tools.observe(
        &json!({"payload":{"type":"custom_tool_call","call_id":"patch","name":"apply_patch",
        "input":"*** Begin Patch\n*** Update File: x.rs\n@@\n-old\n+new\n*** End Patch"}}),
        Provider::Codex,
    );
    let changes = tools.observe(&json!({"payload":{"type":"custom_tool_call_output","call_id":"patch",
        "output":"{\"output\":\"Success. Updated the following files:\\nM x.rs\\n\",\"metadata\":{\"exit_code\":0}}"}}), Provider::Codex);
    assert_eq!(changes.len(), 1);
    assert_eq!(
        (
            &*changes[0].path,
            changes[0].additions,
            changes[0].deletions
        ),
        ("x.rs", 1, 1)
    );
    assert!(tools.observe(&json!({"payload":{"type":"custom_tool_call_output","call_id":"patch","output":"Success."}}), Provider::Codex).is_empty());
}
