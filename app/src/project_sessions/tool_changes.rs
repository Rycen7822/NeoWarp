//! Successful tool observations, not a reconstruction of the working tree.
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use serde_json::Value;

use super::config::Provider;

const MAX_DIFF_BYTES: usize = 64 * 1024;
const MAX_PENDING: usize = 128;
const MAX_NESTED_EDITS: usize = 1024;

#[derive(Clone)]
pub(crate) struct Change {
    pub(crate) path: String,
    pub(crate) diff: String,
    pub(crate) additions: usize,
    pub(crate) deletions: usize,
    pub(crate) truncated: bool,
}

impl Change {
    fn new(path: &str, mut diff: String) -> Option<Self> {
        if path.is_empty() {
            return None;
        }
        let additions = diff.lines().filter(|line| line.starts_with('+')).count();
        let deletions = diff.lines().filter(|line| line.starts_with('-')).count();
        let truncated = diff.len() > MAX_DIFF_BYTES;
        if truncated {
            let end = diff.as_bytes()[..MAX_DIFF_BYTES]
                .iter()
                .rposition(|byte| *byte == b'\n')
                .map_or(0, |index| index + 1);
            diff.truncate(end);
        }
        (additions + deletions > 0).then(|| Self {
            path: path.into(),
            diff,
            additions,
            deletions,
            truncated,
        })
    }
}

#[derive(Clone, Default)]
pub(crate) struct Tools {
    pending: HashMap<String, (String, Arc<Value>)>,
    nested: HashSet<Arc<str>>,
    nested_order: VecDeque<Arc<str>>,
    pub(crate) limited: bool,
}

impl Tools {
    pub(crate) fn new_request(&mut self) {
        self.pending.clear();
    }

    pub(crate) fn observe(&mut self, record: &Value, provider: Provider) -> Vec<Change> {
        let mut changes = Vec::new();
        match provider {
            Provider::Pi | Provider::Claude => {
                if provider == Provider::Pi && record["customType"] == "context-prune-index" {
                    self.observe_archive(record, &mut changes);
                    return changes;
                }
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
                {
                    let details = &message["details"];
                    if message["isError"] != true
                        || (provider == Provider::Pi
                            && details["metisActionFusion"]["version"] == 1
                            && details["metisActionFusion"]["mutationStatus"] == "success"
                            && (details["patch"].is_string()
                                || details["diff"].is_string()
                                || details["metisWriteDiff"].is_object()))
                    {
                        changes.extend(tool_change(&name, &args, details));
                    }
                }
                // Pi records nested ctx.executeTool calls (including codemode)
                // with their own outcome, even when the parent later fails.
                if provider == Provider::Pi && message["role"] == "toolResult" {
                    let nested = &message["nestedCalls"];
                    for call in nested["calls"].as_array().into_iter().flatten() {
                        if call["status"] == "ok"
                            && let Some(name) = call["name"].as_str()
                        {
                            changes.extend(self.nested_change(
                                &call["id"],
                                name,
                                &call["arguments"],
                            ));
                        }
                    }
                    self.limited |= nested["complete"] == false;
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

    fn observe_archive(&mut self, record: &Value, changes: &mut Vec<Change>) {
        for call in record["data"]["toolCalls"].as_array().into_iter().flatten() {
            if call["parentToolCallId"].is_string()
                && call["toolCallId"].is_string()
                && call["archiveOnly"] == true
                && call["toolName"] == "edit"
                && call["isError"] == false
            {
                changes.extend(self.nested_change(&call["toolCallId"], "edit", &call["args"]));
            }
        }
    }

    fn nested_change(&mut self, id: &Value, name: &str, args: &Value) -> Option<Change> {
        let id = id.as_str();
        if id.is_some_and(|id| self.nested.contains(id)) {
            return None;
        }
        let change = tool_change(name, args, &Value::Null)?;
        if let Some(id) = id {
            if self.nested_order.len() >= MAX_NESTED_EDITS
                && let Some(old) = self.nested_order.pop_front()
            {
                self.nested.remove(&old);
            }
            let id: Arc<str> = id.into();
            self.nested_order.push_back(id.clone());
            self.nested.insert(id);
        }
        Some(change)
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
        if self.pending.len() >= MAX_PENDING {
            self.pending.clear();
            self.limited = true;
        }
        let args = if matches!(name, "write" | "Write") || args.to_string().len() > MAX_DIFF_BYTES {
            let Some(path) = tool_path(args) else {
                self.limited = true;
                return;
            };
            serde_json::json!({"path": path})
        } else {
            args.clone()
        };
        self.pending
            .insert(id.into(), (name.into(), Arc::new(args)));
    }

    fn take(&mut self, id: &Value) -> Option<(String, Arc<Value>)> {
        self.pending.remove(id.as_str()?)
    }
}

fn tool_path(args: &Value) -> Option<&str> {
    args["path"].as_str().or_else(|| args["file_path"].as_str())
}

fn tool_change(name: &str, args: &Value, details: &Value) -> Option<Change> {
    if !matches!(name, "edit" | "Edit" | "MultiEdit" | "write" | "Write") {
        return None;
    }
    let path = tool_path(args)?;
    if let Some(patch) = details["patch"].as_str()
        && let Some(header) = patch.find("\n@@ ")
    {
        return Change::new(path, patch[header + 1..].into());
    }
    if let Some(diff) = details["diff"].as_str() {
        if details["firstChangedLine"].is_number() {
            return Change::new(path, strip_pi_line_numbers(diff));
        }
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
        let write = &details["metisWriteDiff"];
        if !matches!(write["kind"].as_str(), Some("add" | "update")) {
            return None;
        }
        let mut diff = String::new();
        for row in write["rows"].as_array()? {
            let marker = match row["kind"].as_str()? {
                "add" => '+',
                "remove" => '-',
                "context" => ' ',
                "separator" => {
                    diff.push_str("@@\n");
                    continue;
                }
                _ => continue,
            };
            diff.push(marker);
            diff.push_str(row["content"].as_str()?);
            diff.push('\n');
        }
        return Change::new(path, diff);
    }
    let replacements = args["edits"]
        .as_array()
        .map_or(std::slice::from_ref(args), Vec::as_slice);
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

// Older Pi edit results only include the display diff with a padded line number after each marker.
fn strip_pi_line_numbers(diff: &str) -> String {
    let mut result = String::with_capacity(diff.len());
    for line in diff.lines() {
        let Some(marker @ (b' ' | b'+' | b'-')) = line.as_bytes().first().copied() else {
            result.push_str(line);
            result.push('\n');
            continue;
        };
        let rest = line[1..].trim_start_matches(' ');
        let number = rest.bytes().take_while(u8::is_ascii_digit).count();
        if number > 0
            && let Some(text) = rest[number..].strip_prefix(' ')
        {
            result.push(char::from(marker));
            result.push_str(text);
        } else if marker == b' ' && rest == "..." {
            result.push_str("@@");
        } else {
            result.push_str(line);
        }
        result.push('\n');
    }
    result
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
