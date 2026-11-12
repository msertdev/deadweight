//! End-to-end tests against a synthetic Claude Code config directory.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{Value, json};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(sessions: usize) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!("deadweight-test-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let claude = root.join("claude");
        let work = root.join("work");
        fs::create_dir_all(&work).unwrap();

        write(
            &claude.join("settings.json"),
            &json!({
                "theme": "dark",
                "enabledPlugins": { "bloat@mkt": true, "handy@mkt": true, "hooky@mkt": true }
            }),
        );

        let cache = claude.join("plugins").join("cache").join("mkt");
        let mut installed = serde_json::Map::new();
        for name in ["bloat", "handy", "hooky"] {
            let dir = cache.join(name);
            fs::create_dir_all(dir.join(".claude-plugin")).unwrap();
            write(
                &dir.join(".claude-plugin").join("plugin.json"),
                &json!({ "name": name }),
            );
            installed.insert(
                format!("{name}@mkt"),
                json!([{ "scope": "user", "installPath": dir }]),
            );
        }
        write(
            &cache.join("hooky").join("hooks").join("hooks.json"),
            &json!({ "hooks": { "SessionStart": [ { "hooks": [ { "type": "command", "command": "hooky-start" } ] } ] } }),
        );
        write(
            &claude.join("plugins").join("installed_plugins.json"),
            &json!({ "version": 2, "plugins": installed }),
        );

        for skill in ["old-skill", "used-skill"] {
            let dir = claude.join("skills").join(skill);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("SKILL.md"), format!("---\nname: {skill}\n---\n")).unwrap();
        }

        write(
            &claude.join(".claude.json"),
            &json!({ "mcpServers": { "idle-srv": { "command": "x", "env": { "TOKEN": "s3cr3t-value" } } } }),
        );

        let project = claude.join("projects").join("demo");
        fs::create_dir_all(&project).unwrap();
        let start = [
            attachment(json!({
                "type": "hook_success", "hookEvent": "SessionStart",
                "command": "hooky-start", "content": "x".repeat(720), "stdout": ""
            })),
            attachment(json!({
                "type": "deferred_tools_delta",
                "addedNames": ["CronCreate", "mcp__plugin_bloat_api__fetch", "mcp__plugin_bloat_api__push", "mcp__idle-srv__query"],
                "failedMcpServers": [{ "name": "plugin:bloat:broken", "errorCode": "ENOTFOUND", "error": "getaddrinfo ENOTFOUND" }]
            })),
            attachment(json!({
                "type": "skill_listing", "isInitial": true, "skillCount": 5,
                "names": ["bloat:report", "handy:helper", "old-skill", "used-skill", "loop"],
                "content": format!(
                    "- bloat:report: {}\n- handy:helper: helps\n- old-skill: an old skill\n- used-skill\n- loop: built in",
                    "y".repeat(1800)
                )
            })),
            json!({ "type": "assistant", "message": { "role": "assistant", "content": [],
                "usage": { "input_tokens": 3, "cache_creation_input_tokens": 4000, "cache_read_input_tokens": 997 } } }),
        ];
        let usage = [
            tool_use("Skill", json!({ "skill": "handy:helper" })),
            tool_use("Skill", json!({ "skill": "used-skill" })),
        ];
        for i in 0..sessions {
            let mut lines: Vec<String> = start.iter().map(Value::to_string).collect();
            if i == 0 {
                lines.extend(usage.iter().map(Value::to_string));
            }
            fs::write(
                project.join(format!("session-{i}.jsonl")),
                lines.join("\n") + "\n",
            )
            .unwrap();
        }
        Fixture { root }
    }

    fn claude(&self) -> PathBuf {
        self.root.join("claude")
    }

    fn run(&self, args: &[&str]) -> (bool, String) {
        let out = Command::new(env!("CARGO_BIN_EXE_deadweight"))
            .args(args)
            .arg("--config-dir")
            .arg(self.claude())
            .current_dir(self.root.join("work"))
            .env("NO_COLOR", "1")
            .env_remove("CLAUDE_CONFIG_DIR")
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout).to_string()
            + &String::from_utf8_lossy(&out.stderr);
        (out.status.success(), text)
    }

    fn json(&self, args: &[&str]) -> Value {
        let mut all = args.to_vec();
        all.push("--json");
        let (ok, text) = self.run(&all);
        assert!(ok, "deadweight failed: {text}");
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("bad json ({e}): {text}"))
    }

    fn settings(&self) -> Value {
        serde_json::from_str(&fs::read_to_string(self.claude().join("settings.json")).unwrap())
            .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn write(path: &Path, v: &Value) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, serde_json::to_string_pretty(v).unwrap()).unwrap();
}

fn attachment(a: Value) -> Value {
    json!({ "type": "attachment", "timestamp": "2026-11-12T10:00:00.000Z", "attachment": a })
}

fn tool_use(name: &str, input: Value) -> Value {
    json!({ "type": "assistant", "message": { "role": "assistant",
        "content": [ { "type": "tool_use", "id": "t", "name": name, "input": input } ] } })
}

fn labels(v: &Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|p| p["key"].as_str().or(p["name"].as_str()).unwrap().to_owned())
        .collect()
}

#[test]
fn audit_measures_and_attributes() {
    let f = Fixture::new(6);
    let v = f.json(&[]);
    let a = &v["analysis"];
    assert_eq!(a["measured_tokens"], 5000);
    assert_eq!(a["sessions"], 6);
    assert_eq!(a["failed_mcp"][0]["name"], "plugin:bloat:broken");
    assert_eq!(a["skills_without_description"], json!(["used-skill"]));

    let bloat = a["sources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["label"] == "bloat")
        .expect("bloat source");
    assert_eq!(bloat["uses"], 0);
    assert_eq!(bloat["skills"], 1);
    assert_eq!(bloat["tools"], 2);
    // The 1800-character description belongs to bloat, not to the line before.
    assert!(bloat["tokens"].as_f64().unwrap() > 450.0);

    let plan = &v["plan"];
    assert_eq!(labels(&plan["plugins"]), vec!["bloat@mkt"]);
    assert_eq!(labels(&plan["hooked"]), vec!["hooky@mkt"]);
    assert_eq!(labels(&plan["skills"]), vec!["old-skill"]);
}

#[test]
fn human_report_runs_and_never_prints_secrets() {
    let f = Fixture::new(6);
    let (ok, text) = f.run(&[]);
    assert!(ok, "{text}");
    assert!(text.contains("every request carries 5.0k tokens"), "{text}");
    assert!(text.contains("deadweight fix"), "{text}");
    assert!(!text.contains("s3cr3t-value"));
    let (_, json_text) = f.run(&["--json"]);
    assert!(!json_text.contains("s3cr3t-value"));
}

#[test]
fn fix_then_undo_round_trips() {
    let f = Fixture::new(6);
    let skill = f.claude().join("skills").join("old-skill");

    let v = f.json(&["fix", "--yes"]);
    assert_eq!(v["applied"], true);
    let s = f.settings();
    assert_eq!(s["enabledPlugins"]["bloat@mkt"], false);
    assert_eq!(s["enabledPlugins"]["handy@mkt"], true);
    assert_eq!(s["enabledPlugins"]["hooky@mkt"], true);
    assert_eq!(s["theme"], "dark");
    assert!(!skill.exists());
    assert!(f.claude().join("skills").join("used-skill").exists());

    let v = f.json(&["undo", "--yes"]);
    assert_eq!(v["restored"], true);
    let s = f.settings();
    assert_eq!(s["enabledPlugins"]["bloat@mkt"], true);
    assert!(skill.join("SKILL.md").exists());

    let v = f.json(&["undo", "--yes"]);
    assert_eq!(v["restored"], false);
}

#[test]
fn fix_respects_keep_and_include_hooked() {
    let f = Fixture::new(6);
    let v = f.json(&["fix", "--dry-run", "--include-hooked", "--keep", "bloat"]);
    assert_eq!(labels(&v["plan"]["plugins"]), vec!["hooky@mkt"]);
    assert_eq!(f.settings()["enabledPlugins"]["bloat@mkt"], true);
}

#[test]
fn fix_refuses_with_too_little_history() {
    let f = Fixture::new(2);
    let (ok, text) = f.run(&["fix", "--yes"]);
    assert!(!ok);
    assert!(text.contains("too little history"), "{text}");
    assert_eq!(f.settings()["enabledPlugins"]["bloat@mkt"], true);
}

#[test]
fn missing_config_dir_is_a_clear_error() {
    let f = Fixture::new(1);
    let out = Command::new(env!("CARGO_BIN_EXE_deadweight"))
        .args(["--config-dir"])
        .arg(f.root.join("nope"))
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("no Claude Code config found"));
}
