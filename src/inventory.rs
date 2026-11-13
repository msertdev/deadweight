//! What is installed: plugins, user skills and agents, MCP server names,
//! hook commands and CLAUDE.md files. Secrets are never read: for MCP servers
//! only the names are taken from `.claude.json`.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::paths::Paths;

#[derive(Debug, Clone)]
pub struct Plugin {
    /// `name@marketplace`, the key used in `enabledPlugins`.
    pub key: String,
    pub name: String,
    /// Enabled in the user-level `settings.json` (the only place `fix` edits).
    pub enabled_by_user: bool,
    /// Every hook command the plugin registers, verbatim.
    pub hook_commands: Vec<String>,
    /// Skill and command names the plugin ships, without the `plugin:` prefix.
    pub entry_names: BTreeSet<String>,
}

#[derive(Debug, Default)]
pub struct Inventory {
    pub plugins: Vec<Plugin>,
    /// User skills: name -> directory under `~/.claude/skills`.
    pub user_skills: BTreeMap<String, PathBuf>,
    pub project_skills: BTreeSet<String>,
    pub user_agents: BTreeSet<String>,
    /// MCP servers configured in `.claude.json` (user and current project).
    pub user_mcp: BTreeSet<String>,
    /// Of those, the ones in the top-level `mcpServers` (user scope), which
    /// load in every project.
    pub user_scope_mcp: BTreeSet<String>,
    pub settings_hook_commands: Vec<String>,
    /// `(path, characters)` of CLAUDE.md files loaded for the current directory.
    pub memory_files: Vec<(PathBuf, usize)>,
}

impl Inventory {
    pub fn load(paths: &Paths) -> Self {
        let mut inv = Inventory::default();
        let settings = read_json(&paths.settings()).unwrap_or(Value::Null);

        let enabled: BTreeMap<String, bool> = settings
            .get("enabledPlugins")
            .and_then(Value::as_object)
            .map(|m| {
                m.iter()
                    .map(|(k, v)| (k.clone(), v.as_bool().unwrap_or(false)))
                    .collect()
            })
            .unwrap_or_default();

        let installed = read_json(
            &paths
                .claude_dir
                .join("plugins")
                .join("installed_plugins.json"),
        )
        .and_then(|v| v.get("plugins").cloned())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();

        let mut keys: BTreeSet<String> = enabled.keys().cloned().collect();
        keys.extend(installed.keys().cloned());
        for key in keys {
            let install_path = installed
                .get(&key)
                .and_then(Value::as_array)
                .and_then(|a| a.first())
                .and_then(|e| e.get("installPath"))
                .and_then(Value::as_str)
                .map(PathBuf::from);
            let mut plugin = Plugin {
                name: key.split('@').next().unwrap_or(&key).to_owned(),
                enabled_by_user: enabled.get(&key).copied().unwrap_or(false),
                key,
                hook_commands: Vec::new(),
                entry_names: BTreeSet::new(),
            };
            if let Some(root) = install_path.filter(|p| p.is_dir()) {
                plugin.hook_commands = plugin_hook_commands(&root);
                plugin.entry_names = plugin_entry_names(&root);
            }
            inv.plugins.push(plugin);
        }

        inv.user_skills = skill_dirs(&paths.claude_dir.join("skills"));
        inv.project_skills = skill_dirs(&paths.cwd.join(".claude").join("skills"))
            .into_keys()
            .collect();
        inv.user_agents = md_stems(&paths.claude_dir.join("agents"));

        if let Some(cfg) = read_json(&paths.global_config) {
            if let Some(m) = cfg.get("mcpServers").and_then(Value::as_object) {
                inv.user_mcp.extend(m.keys().cloned());
                inv.user_scope_mcp.extend(m.keys().cloned());
            }
            let project_key = paths.cwd.to_string_lossy().replace('\\', "/");
            if let Some(m) = cfg
                .get("projects")
                .and_then(|p| p.get(&project_key))
                .and_then(|p| p.get("mcpServers"))
                .and_then(Value::as_object)
            {
                inv.user_mcp.extend(m.keys().cloned());
            }
        }
        if let Some(m) = read_json(&paths.cwd.join(".mcp.json"))
            .and_then(|v| v.get("mcpServers").cloned())
            .and_then(|v| v.as_object().cloned())
        {
            inv.user_mcp.extend(m.keys().cloned());
        }

        if let Some(h) = settings.get("hooks") {
            collect_commands(h, &mut inv.settings_hook_commands);
        }

        for p in [
            paths.claude_dir.join("CLAUDE.md"),
            paths.cwd.join("CLAUDE.md"),
            paths.cwd.join("CLAUDE.local.md"),
            paths.cwd.join(".claude").join("CLAUDE.md"),
        ] {
            if let Ok(text) = fs::read_to_string(&p) {
                inv.memory_files.push((p, text.chars().count()));
            }
        }
        inv
    }

    pub fn plugin_by_name(&self, name: &str) -> Option<&Plugin> {
        // Prefer an enabled plugin when two marketplaces ship the same name.
        self.plugins
            .iter()
            .filter(|p| p.name == name)
            .max_by_key(|p| p.enabled_by_user)
    }
}

pub fn read_json(path: &Path) -> Option<Value> {
    let text = fs::read_to_string(path).ok()?;
    serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()
}

fn plugin_hook_commands(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(v) = read_json(&root.join("hooks").join("hooks.json")) {
        collect_commands(&v, &mut out);
    }
    if let Some(manifest) = read_json(&root.join(".claude-plugin").join("plugin.json")) {
        match manifest.get("hooks") {
            Some(Value::String(rel)) => {
                if let Some(v) = read_json(&root.join(rel)) {
                    collect_commands(&v, &mut out);
                }
            }
            Some(Value::Array(list)) => {
                for item in list {
                    match item {
                        Value::String(rel) => {
                            if let Some(v) = read_json(&root.join(rel)) {
                                collect_commands(&v, &mut out);
                            }
                        }
                        other => collect_commands(other, &mut out),
                    }
                }
            }
            Some(obj @ Value::Object(_)) => collect_commands(obj, &mut out),
            _ => {}
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Every `"command": "..."` string anywhere inside a hooks definition.
fn collect_commands(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::Object(o) => {
            for (k, val) in o {
                if k == "command" {
                    if let Some(s) = val.as_str() {
                        out.push(s.to_owned());
                    }
                } else {
                    collect_commands(val, out);
                }
            }
        }
        Value::Array(a) => a.iter().for_each(|x| collect_commands(x, out)),
        _ => {}
    }
}

fn plugin_entry_names(root: &Path) -> BTreeSet<String> {
    let mut names: BTreeSet<String> = skill_dirs(&root.join("skills")).into_keys().collect();
    names.extend(md_stems(&root.join("commands")));
    names.extend(md_stems(&root.join("agents")));
    names
}

fn skill_dirs(dir: &Path) -> BTreeMap<String, PathBuf> {
    let mut out = BTreeMap::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return out;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.join("SKILL.md").is_file() {
            if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                out.insert(name.to_owned(), p);
            }
        }
    }
    out
}

fn md_stems(dir: &Path) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return out;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.extension().is_some_and(|x| x == "md") {
            if let Some(stem) = p.file_stem().and_then(|n| n.to_str()) {
                out.insert(stem.to_owned());
            }
        }
    }
    out
}
