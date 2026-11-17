//! Successful tool observations, not a reconstruction of the working tree.
use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value;

use super::config::Provider;

const MAX_DIFF_BYTES: usize = 64 * 1024;
const MAX_PENDING: usize = 128;

#[derive(Clone)]
pub(crate) struct Change {
    pub(crate) path: String,
    pub(crate) diff: String,
    pub(crate) additions: usize,
    pub(crate) deletions: usize,
}

impl Change {
    fn new(path: &str, diff: String) -> Option<Self> {
        if path.is_empty() || diff.len() > MAX_DIFF_BYTES {
            return None;
        }
        let additions = diff.lines().filter(|line| line.starts_with('+')).count();
        let deletions = diff.lines().filter(|line| line.starts_with('-')).count();
        (additions + deletions > 0).then(|| Self {
            path: path.into(),
            diff,
            additions,
            deletions,
        })
    }
}

#[derive(Clone, Default)]
pub(crate) struct Tools {
    pending: HashMap<String, (String, Arc<Value>)>,
}

impl Tools {
    pub(crate) fn observe(&mut self, record: &Value, provider: Provider) -> Vec<Change> {
        let mut changes = Vec::new();
        match provider {
            Provider::Pi | Provider::Claude => {
                let message = &record["message"];
                if let Some(parts) = message["content"].as_array() {
                    for part in parts {
                        match part["type"].as_str() {
                            Some("toolCall" | "tool_use") if message["role"] == "assistant" => {
                                let args = part.get("arguments").unwrap_or(&part["input"]);
                                self.remember(&part["id"], &part["name"], args);
                            }
                            Some("tool_result") if message["role"] == "user" => {
                                if let Some((name, args)) = self.take(&part["tool_use_id"])
                                    && part["is_error"] != true
                                {
                                    changes.extend(tool_change(
                                        &name,
                                        &args,
                                        &record["toolUseResult"],
                                    ));
                                }
                            }
                            _ => (),
                        }
                    }
                }
                if message["role"] == "toolResult"
                    && let Some((name, args)) = self.take(&message["toolCallId"])
                    && message["isError"] != true
                {
                    changes.extend(tool_change(&name, &args, &message["details"]));
                }
                // Pi records nested ctx.executeTool calls (including codemode)
                // with their own outcome, even when the parent later fails.
                if message["role"] == "toolResult"
                    && let Some(calls) = message["nestedCalls"]["calls"].as_array()
                {
                    for call in calls {
                        if call["status"] == "ok"
                            && let Some(name) = call["name"].as_str()
                        {
                            changes.extend(tool_change(name, &call["arguments"], &Value::Null));
                        }
                    }
                }
            }
            Provider::Codex => {
                let payload = &record["payload"];
                match payload["type"].as_str() {
                    Some("custom_tool_call") if payload["name"] == "apply_patch" => {
                        self.remember(&payload["call_id"], &payload["name"], &payload["input"]);
                    }
                    Some("custom_tool_call_output") => {
                        if let Some((_, input)) = self.take(&payload["call_id"]) {
                            let output = payload["output"].as_str().unwrap_or_default();
                            let parsed = serde_json::from_str::<Value>(output).ok();
                            let text = parsed
                                .as_ref()
                                .and_then(|v| v["output"].as_str())
                                .unwrap_or(output);
                            if text.starts_with("Success.") {
                                changes.extend(patch_changes(input.as_str().unwrap_or_default()));
                            }
                        }
                    }
                    _ => (),
                }
            }
            _ => (),
        }
        changes
    }

    fn remember(&mut self, id: &Value, name: &Value, args: &Value) {
        let (Some(id), Some(name)) = (id.as_str(), name.as_str()) else {
            return;
        };
        if !matches!(
            name,
            "edit" | "Edit" | "MultiEdit" | "write" | "Write" | "apply_patch"
        ) {
            return;
        }
        if args.to_string().len() > MAX_DIFF_BYTES {
            return;
        }
        if self.pending.len() >= MAX_PENDING {
            self.pending.clear();
        }
        self.pending
            .insert(id.into(), (name.into(), Arc::new(args.clone())));
    }

    fn take(&mut self, id: &Value) -> Option<(String, Arc<Value>)> {
        self.pending.remove(id.as_str()?)
    }
}

fn tool_change(name: &str, args: &Value, details: &Value) -> Option<Change> {
    if !matches!(name, "edit" | "Edit" | "MultiEdit" | "write" | "Write") {
        return None;
    }
    let path = args["path"]
        .as_str()
        .or_else(|| args["file_path"].as_str())?;
    if let Some(diff) = details["diff"].as_str() {
        return Change::new(path, diff.into());
    }
    if let Some(hunks) = details["structuredPatch"].as_array() {
        let mut diff = String::new();
        for hunk in hunks {
            diff.push_str(&format!(
                "@@ -{},{} +{},{} @@\n",
                hunk["oldStart"], hunk["oldLines"], hunk["newStart"], hunk["newLines"]
            ));
            for line in hunk["lines"].as_array()? {
                diff.push_str(line.as_str()?);
                diff.push('\n');
            }
        }
        if !diff.is_empty() {
            return Change::new(path, diff);
        }
    }
    if matches!(name, "write" | "Write") {
        // New content alone cannot prove a creation or reconstruct an overwrite.
        return None;
    }
    let replacements: Vec<_> = args["edits"]
        .as_array()
        .map(|edits| edits.iter().collect())
        .unwrap_or_else(|| vec![args]);
    let mut diff = String::new();
    for edit in replacements {
        let before = edit["oldText"]
            .as_str()
            .or_else(|| edit["old_string"].as_str())?;
        let after = edit["newText"]
            .as_str()
            .or_else(|| edit["new_string"].as_str())?;
        if before == after {
            continue;
        }
        diff.push_str("@@ replacement @@\n");
        for (prefix, text) in [('-', before), ('+', after)] {
            for line in text.lines() {
                diff.push(prefix);
                diff.push_str(line);
                diff.push('\n');
            }
        }
    }
    Change::new(path, diff)
}

fn patch_changes(patch: &str) -> Vec<Change> {
    let mut changes = Vec::new();
    let mut path = None;
    let mut diff = String::new();
    for line in patch.lines() {
        if line.starts_with("*** ") {
            if line.starts_with("*** Move to: ") {
                path = line.strip_prefix("*** Move to: ");
                continue;
            }
            if let Some(path) = path.take()
                && let Some(change) = Change::new(path, std::mem::take(&mut diff))
            {
                changes.push(change);
            }
            path = line
                .strip_prefix("*** Update File: ")
                .or_else(|| line.strip_prefix("*** Add File: "));
        } else if path.is_some() {
            diff.push_str(line);
            diff.push('\n');
        }
    }
    if let Some(path) = path
        && let Some(change) = Change::new(path, diff)
    {
        changes.push(change);
    }
    changes
}

#[cfg(test)]
#[path = "tool_changes_tests.rs"]
mod tests;
