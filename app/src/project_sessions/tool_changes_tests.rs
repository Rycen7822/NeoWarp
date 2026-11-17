use serde_json::json;

use super::*;

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
