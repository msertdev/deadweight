//! Reads Claude Code session transcripts (`~/.claude/projects/**/*.jsonl`).
//!
//! Two things come out of them:
//! * a [`Snapshot`]: what the first request of a session carried before the
//!   user's prompt (skill listing, deferred tool names, MCP instructions, agent
//!   listing, hook injections) plus the token count the API actually billed;
//! * [`Usage`]: which skills, MCP tools, agents and slash commands were really
//!   used across all sessions inside the time window.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use rayon::prelude::*;
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, Serialize)]
pub struct HookInjection {
    pub event: String,
    pub command: String,
    pub chars: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct FailedMcp {
    pub name: String,
    pub code: String,
    pub error: String,
}

#[derive(Debug, Default, Clone)]
pub struct Snapshot {
    pub file: PathBuf,
    pub started_at: Option<String>,
    pub cwd: Option<String>,
    /// `(skill name, characters of its listing entry)`
    pub skills: Vec<(String, usize)>,
    /// Skills listed by name only: the listing ran out of room for them.
    pub skills_without_description: Vec<String>,
    /// Size of the whole skill listing text.
    pub skill_listing_chars: usize,
    /// Names of deferred tools listed to the model.
    pub tools: Vec<String>,
    /// `(server name, characters of its instruction block)`
    pub mcp_instructions: Vec<(String, usize)>,
    /// `(agent type, characters of its listing line)`
    pub agents: Vec<(String, usize)>,
    pub hooks: Vec<HookInjection>,
    pub failed_mcp: Vec<FailedMcp>,
    /// Claude Code's own system prompt + inline tool schemas, when recorded.
    pub builtin_chars: Option<usize>,
    /// Number of tool schemas sent inline with the request.
    pub builtin_tools: Option<usize>,
    /// input + cache creation + cache read tokens of the first API call.
    pub first_request_tokens: Option<u64>,
}

impl Snapshot {
    fn has_setup_data(&self) -> bool {
        !self.skills.is_empty() || !self.tools.is_empty() || !self.hooks.is_empty()
    }
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct Usage {
    /// Top-level sessions (not subagents) touched inside the window.
    pub sessions: usize,
    /// Age in days of the oldest session inside the window.
    pub history_days: u64,
    pub skills: HashMap<String, u32>,
    pub mcp_tools: HashMap<String, u32>,
    pub agents: HashMap<String, u32>,
    pub commands: HashMap<String, u32>,
}

impl Usage {
    fn merge(mut self, other: Usage) -> Usage {
        self.sessions += other.sessions;
        self.history_days = self.history_days.max(other.history_days);
        for (dst, src) in [
            (&mut self.skills, other.skills),
            (&mut self.mcp_tools, other.mcp_tools),
            (&mut self.agents, other.agents),
            (&mut self.commands, other.commands),
        ] {
            for (k, v) in src {
                *dst.entry(k).or_default() += v;
            }
        }
        self
    }
}

struct SessionFile {
    path: PathBuf,
    modified: SystemTime,
    top_level: bool,
}

/// All transcript files under `projects/`, newest first.
fn session_files(projects: &Path) -> Vec<SessionFile> {
    let mut out = Vec::new();
    let Ok(dirs) = fs::read_dir(projects) else {
        return out;
    };
    for dir in dirs.flatten() {
        collect_jsonl(&dir.path(), 0, &mut out);
    }
    out.sort_by_key(|f| std::cmp::Reverse(f.modified));
    out
}

fn collect_jsonl(dir: &Path, depth: usize, out: &mut Vec<SessionFile>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_dir() {
            // project/<session>/subagents/*.jsonl
            if depth < 3 {
                collect_jsonl(&path, depth + 1, out);
            }
        } else if path.extension().is_some_and(|e| e == "jsonl") {
            let modified = entry
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            out.push(SessionFile {
                path,
                modified,
                top_level: depth == 0,
            });
        }
    }
}

pub struct History {
    /// The newest session that recorded what was loaded at start.
    pub latest: Option<Snapshot>,
    /// First-request token counts of the most recent sessions.
    pub recent_first_requests: Vec<u64>,
    pub usage: Usage,
}

pub fn load(projects: &Path, days: u64) -> History {
    let files = session_files(projects);
    let now = SystemTime::now();
    let cutoff = now
        .checked_sub(Duration::from_secs(days.saturating_mul(86_400)))
        .unwrap_or(SystemTime::UNIX_EPOCH);

    let top: Vec<&SessionFile> = files.iter().filter(|f| f.top_level).collect();

    // Newest snapshots first; we only need a handful.
    let snapshots: Vec<Snapshot> = top
        .iter()
        .take(12)
        .collect::<Vec<_>>()
        .par_iter()
        .filter_map(|f| parse_snapshot(&f.path).ok())
        .filter(|s| s.has_setup_data())
        .collect();
    // par_iter keeps order, so the first one is the newest.
    let recent_first_requests = snapshots
        .iter()
        .filter_map(|s| s.first_request_tokens)
        .take(10)
        .collect();
    let latest = snapshots.into_iter().next();

    let usage = files
        .par_iter()
        .filter(|f| f.modified >= cutoff)
        .map(|f| {
            let mut u = scan_usage(&f.path).unwrap_or_default();
            if f.top_level {
                u.sessions = 1;
                u.history_days = now
                    .duration_since(f.modified)
                    .map(|d| d.as_secs() / 86_400)
                    .unwrap_or(0);
            }
            u
        })
        .reduce(Usage::default, Usage::merge);

    History {
        latest,
        recent_first_requests,
        usage,
    }
}

/// Read the start of one session up to (and including) its first API response.
pub fn parse_snapshot(path: &Path) -> std::io::Result<Snapshot> {
    let reader = BufReader::with_capacity(256 * 1024, File::open(path)?);
    let mut snap = Snapshot {
        file: path.to_path_buf(),
        ..Default::default()
    };
    let mut aggregated_hook_chars = 0usize;
    let mut seen_skill_listing = false;

    for line in reader.lines() {
        let Ok(line) = line else { break };
        let is_attachment = line.contains("\"type\":\"attachment\"");
        let is_assistant = line.contains("\"type\":\"assistant\"");
        let is_shape = line.contains("\"type\":\"api-request-shape\"");
        if !(is_attachment || is_assistant || is_shape) {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if snap.cwd.is_none() {
            snap.cwd = v.get("cwd").and_then(Value::as_str).map(str::to_owned);
        }
        if snap.started_at.is_none() {
            snap.started_at = v
                .get("timestamp")
                .and_then(Value::as_str)
                .map(str::to_owned);
        }
        match v.get("type").and_then(Value::as_str) {
            Some("assistant") => {
                if let Some(u) = v.pointer("/message/usage") {
                    let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
                    let total = n("input_tokens")
                        + n("cache_creation_input_tokens")
                        + n("cache_read_input_tokens");
                    if total > 0 {
                        snap.first_request_tokens = Some(total);
                    }
                }
                break;
            }
            Some("api-request-shape") => {
                // The exact system prompt + tools of the first request.
                let shape = match v.get("shape") {
                    Some(Value::String(s)) => serde_json::from_str::<Value>(s).ok(),
                    Some(other) => Some(other.clone()),
                    None => None,
                };
                if let Some(shape) = shape {
                    let system = shape.get("system").map(text_len).unwrap_or(0);
                    let tools = shape.get("tools").and_then(Value::as_array);
                    let tool_chars: usize = tools
                        .map(|t| t.iter().map(|x| x.to_string().len()).sum())
                        .unwrap_or(0);
                    snap.builtin_chars = Some(system + tool_chars);
                    snap.builtin_tools = tools.map(Vec::len);
                }
            }
            Some("attachment") => {
                let Some(a) = v.get("attachment") else {
                    continue;
                };
                match a.get("type").and_then(Value::as_str).unwrap_or("") {
                    "skill_listing" if !seen_skill_listing => {
                        seen_skill_listing = true;
                        let names = str_array(a.get("names"));
                        let content = a.get("content").and_then(Value::as_str).unwrap_or("");
                        snap.skills = split_listing(content, &names);
                        snap.skill_listing_chars = content.chars().count();
                        let known: HashSet<&str> = names.iter().map(String::as_str).collect();
                        snap.skills_without_description = content
                            .lines()
                            .filter_map(|l| l.strip_prefix("- ").map(str::trim))
                            .filter(|n| known.contains(n))
                            .map(str::to_owned)
                            .collect();
                    }
                    "deferred_tools_delta" => {
                        snap.tools.extend(str_array(a.get("addedNames")));
                        if let Some(failed) = a.get("failedMcpServers").and_then(Value::as_array) {
                            for f in failed {
                                let s = |k: &str| {
                                    f.get(k).and_then(Value::as_str).unwrap_or("").to_owned()
                                };
                                snap.failed_mcp.push(FailedMcp {
                                    name: s("name"),
                                    code: s("errorCode"),
                                    error: s("error"),
                                });
                            }
                        }
                    }
                    "mcp_instructions_delta" => {
                        let names = str_array(a.get("addedNames"));
                        let blocks = str_array(a.get("addedBlocks"));
                        for (n, b) in names.into_iter().zip(blocks) {
                            snap.mcp_instructions.push((n, b.chars().count()));
                        }
                    }
                    "agent_listing_delta" => {
                        let types = str_array(a.get("addedTypes"));
                        let lines = str_array(a.get("addedLines"));
                        if types.len() == lines.len() {
                            for (t, l) in types.into_iter().zip(lines) {
                                snap.agents.push((t, l.chars().count() + 1));
                            }
                        } else {
                            let joined = lines.join("\n");
                            snap.agents.extend(split_listing(&joined, &types));
                        }
                    }
                    "hook_success" => {
                        let chars = hook_injected_chars(a);
                        if chars > 0 {
                            let s =
                                |k: &str| a.get(k).and_then(Value::as_str).unwrap_or("").to_owned();
                            snap.hooks.push(HookInjection {
                                event: s("hookEvent"),
                                command: s("command"),
                                chars,
                            });
                        }
                    }
                    "hook_additional_context" | "hook_system_message" => {
                        aggregated_hook_chars += a.get("content").map(text_len).unwrap_or(0);
                    }
                    "prompt_snapshot" => {
                        let system = a.get("systemPrompt").map(text_len).unwrap_or(0);
                        let tools = a
                            .get("tools")
                            .and_then(Value::as_array)
                            .map(|t| t.iter().map(|x| x.to_string().len()).sum())
                            .unwrap_or(0);
                        if tools > 0 && snap.builtin_chars.is_none() {
                            snap.builtin_chars = Some(system + tools);
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    // Older/newer formats may only record the merged hook context.
    if snap.hooks.is_empty() && aggregated_hook_chars > 0 {
        snap.hooks.push(HookInjection {
            event: "SessionStart".into(),
            command: String::new(),
            chars: aggregated_hook_chars,
        });
    }
    Ok(snap)
}

/// Characters a successful hook added to the model's context.
fn hook_injected_chars(a: &Value) -> usize {
    let content = a.get("content").and_then(Value::as_str).unwrap_or("");
    if !content.trim().is_empty() {
        return content.chars().count();
    }
    let stdout = a.get("stdout").and_then(Value::as_str).unwrap_or("").trim();
    if !stdout.starts_with('{') {
        return 0;
    }
    let Ok(out) = serde_json::from_str::<Value>(stdout) else {
        return 0;
    };
    let ctx = out
        .pointer("/hookSpecificOutput/additionalContext")
        .map(text_len)
        .unwrap_or(0);
    let msg = out.get("systemMessage").map(text_len).unwrap_or(0);
    ctx + msg
}

/// Split a `- name: description` listing into per-entry sizes. Descriptions
/// may span several lines, so an entry runs until the next known name.
pub fn split_listing(content: &str, names: &[String]) -> Vec<(String, usize)> {
    let known: HashSet<&str> = names.iter().map(String::as_str).collect();
    let mut out: Vec<(String, usize)> = Vec::new();
    for line in content.lines() {
        let len = line.chars().count() + 1;
        let starts_entry = line
            .strip_prefix("- ")
            .and_then(|rest| entry_name(rest, &known));
        match (starts_entry, out.last_mut()) {
            (Some(name), _) => out.push((name.to_owned(), len)),
            (None, Some(last)) => last.1 += len,
            (None, None) => {}
        }
    }
    // Names that never appeared in the text still cost their own line.
    let present: HashSet<String> = out.iter().map(|(n, _)| n.clone()).collect();
    for n in names {
        if !present.contains(n) {
            out.push((n.clone(), n.len() + 3));
        }
    }
    out
}

/// The longest known name that `rest` starts with, followed by `:` or the end.
/// Names contain colons themselves (`plugin:skill`), so try every split point.
fn entry_name<'a>(rest: &'a str, known: &HashSet<&str>) -> Option<&'a str> {
    let whole = rest.trim();
    if known.contains(whole) {
        return Some(whole);
    }
    // Walk split points from the right so the first hit is the longest name.
    rest.rmatch_indices(':')
        .map(|(i, _)| rest[..i].trim())
        .find(|n| known.contains(n))
}

fn str_array(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// Length of the text inside a string / array of strings / content blocks.
fn text_len(v: &Value) -> usize {
    match v {
        Value::String(s) => s.chars().count(),
        Value::Array(a) => a.iter().map(text_len).sum(),
        Value::Object(o) => o.get("text").map(text_len).unwrap_or(0),
        _ => 0,
    }
}

/// Count every skill, MCP tool, agent and slash command used in one transcript.
fn scan_usage(path: &Path) -> std::io::Result<Usage> {
    let reader = BufReader::with_capacity(256 * 1024, File::open(path)?);
    let mut u = Usage::default();
    for line in reader.lines() {
        let Ok(line) = line else { break };
        let tool_use = line.contains("\"tool_use\"");
        let command = line.contains("<command-name>");
        if !tool_use && !command {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let role = v.get("type").and_then(Value::as_str);
        if tool_use && role == Some("assistant") {
            let Some(blocks) = v.pointer("/message/content").and_then(Value::as_array) else {
                continue;
            };
            for b in blocks {
                if b.get("type").and_then(Value::as_str) != Some("tool_use") {
                    continue;
                }
                let name = b.get("name").and_then(Value::as_str).unwrap_or("");
                let input = b.get("input");
                let field = |k: &str| {
                    input
                        .and_then(|i| i.get(k))
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                };
                match name {
                    "Skill" => {
                        if let Some(s) = field("skill") {
                            *u.skills
                                .entry(s.trim_start_matches('/').to_owned())
                                .or_default() += 1;
                        }
                    }
                    "Agent" | "Task" => {
                        if let Some(s) = field("subagent_type") {
                            *u.agents.entry(s).or_default() += 1;
                        }
                    }
                    n if n.starts_with("mcp__") => {
                        *u.mcp_tools.entry(n.to_owned()).or_default() += 1;
                    }
                    _ => {}
                }
            }
        }
        if command && role == Some("user") {
            let text = v
                .pointer("/message/content")
                .map(Value::to_string)
                .unwrap_or_default();
            for cmd in extract_commands(&text) {
                *u.commands.entry(cmd).or_default() += 1;
            }
        }
    }
    Ok(u)
}

fn extract_commands(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("<command-name>") {
        rest = &rest[start + "<command-name>".len()..];
        let Some(end) = rest.find("</command-name>") else {
            break;
        };
        let name = rest[..end].trim().trim_start_matches('/');
        if !name.is_empty() {
            out.push(name.to_owned());
        }
        rest = &rest[end..];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listing_entries_absorb_multiline_descriptions() {
        let names = vec!["a".to_string(), "plugin:b".to_string(), "c".to_string()];
        let content = "- a: first\n- plugin:b: second\nTRIGGER — more text\n- c: third";
        let got = split_listing(content, &names);
        assert_eq!(got.len(), 3);
        assert_eq!(got[0], ("a".into(), "- a: first".len() + 1));
        assert_eq!(got[1].0, "plugin:b");
        assert_eq!(
            got[1].1,
            "- plugin:b: second".len() + 1 + "TRIGGER — more text".chars().count() + 1
        );
    }

    #[test]
    fn listing_without_description() {
        let names = vec!["bare".to_string(), "missing".to_string()];
        let got = split_listing("- bare", &names);
        assert_eq!(got[0], ("bare".into(), 7));
        assert_eq!(got[1].0, "missing");
    }

    #[test]
    fn commands_are_extracted() {
        let t =
            r#"<command-name>/model</command-name> x <command-name>/sales:forecast</command-name>"#;
        assert_eq!(extract_commands(t), vec!["model", "sales:forecast"]);
    }

    #[test]
    fn hook_json_output_is_measured() {
        let a: Value = serde_json::json!({
            "content": "",
            "stdout": "{\"hookSpecificOutput\":{\"additionalContext\":\"hello\"},\"systemMessage\":\"hi\"}"
        });
        assert_eq!(hook_injected_chars(&a), 7);
        let plain: Value = serde_json::json!({"content": "plain text", "stdout": "plain text"});
        assert_eq!(hook_injected_chars(&plain), 10);
    }
}
