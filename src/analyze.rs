//! Turns a session snapshot, usage history and the inventory into an
//! attributed, token-weighted picture of the setup.

use std::collections::{BTreeMap, HashMap};

use serde::Serialize;

use crate::inventory::Inventory;
use crate::transcript::History;

/// Characters per token for prose such as skill descriptions and hook text.
const PROSE_CHARS_PER_TOKEN: f64 = 3.6;
/// Identifiers like `mcp__plugin_x_y__get_thing` split into many short tokens.
const IDENT_CHARS_PER_TOKEN: f64 = 2.8;
/// JSON tool schemas, used only when nothing was measured.
const SCHEMA_CHARS_PER_TOKEN: f64 = 3.0;
/// A single skill description above this many tokens is worth pointing out.
const HEAVY_SKILL_TOKENS: f64 = 400.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Skill,
    Tool,
    McpInstructions,
    Agent,
    Hook,
    Memory,
}

impl Kind {
    fn chars_per_token(self) -> f64 {
        match self {
            Kind::Tool => IDENT_CHARS_PER_TOKEN,
            _ => PROSE_CHARS_PER_TOKEN,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::Skill => "Skill listings",
            Kind::Tool => "Deferred tool names",
            Kind::McpInstructions => "MCP server instructions",
            Kind::Agent => "Agent listings",
            Kind::Hook => "Hook injections",
            Kind::Memory => "CLAUDE.md files",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(tag = "type", content = "name", rename_all = "snake_case")]
pub enum Source {
    /// Plugin key `name@marketplace`.
    Plugin(String),
    UserSkill(String),
    ProjectSkill(String),
    UserAgent(String),
    UserMcp(String),
    /// claude.ai connector.
    Connector(String),
    /// MCP server configured somewhere deadweight does not edit.
    OtherMcp(String),
    /// A `prefix:` skill group that is not an installed plugin.
    Bundle(String),
    SettingsHook,
    Memory(String),
    UnknownHook,
    BuiltIn,
}

impl Source {
    pub fn label(&self) -> String {
        match self {
            Source::Plugin(k) => k.split('@').next().unwrap_or(k).to_owned(),
            Source::UserSkill(n) | Source::ProjectSkill(n) => n.clone(),
            Source::UserAgent(n) => n.clone(),
            Source::UserMcp(n) | Source::OtherMcp(n) => n.clone(),
            Source::Connector(n) => format!("claude.ai {n}"),
            Source::Bundle(n) => n.clone(),
            Source::SettingsHook => "settings.json hooks".into(),
            Source::Memory(p) => p.clone(),
            Source::UnknownHook => "unattributed hooks".into(),
            Source::BuiltIn => "Claude Code built-ins".into(),
        }
    }

    pub fn kind_label(&self) -> &'static str {
        match self {
            Source::Plugin(_) => "plugin",
            Source::UserSkill(_) => "user skill",
            Source::ProjectSkill(_) => "project skill",
            Source::UserAgent(_) => "user agent",
            Source::UserMcp(_) => "mcp server",
            Source::Connector(_) => "connector",
            Source::OtherMcp(_) => "mcp server",
            Source::Bundle(_) => "skill bundle",
            Source::SettingsHook => "hooks",
            Source::Memory(_) => "memory",
            Source::UnknownHook => "hooks",
            Source::BuiltIn => "built-in",
        }
    }

    pub fn is_builtin(&self) -> bool {
        matches!(self, Source::BuiltIn)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Item {
    pub kind: Kind,
    pub name: String,
    pub source: Source,
    pub tokens: f64,
    pub uses: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct CategoryTotal {
    pub kind: Kind,
    pub count: usize,
    pub tokens: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct SourceSummary {
    pub source: Source,
    pub label: String,
    pub tokens: f64,
    pub skills: usize,
    pub tools: usize,
    pub agents: usize,
    pub mcp_blocks: usize,
    pub hook_tokens: f64,
    pub uses: u32,
    /// The plugin registers hooks, so it may do useful work deadweight cannot see.
    pub runs_hooks: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct FailedServer {
    pub name: String,
    pub source: String,
    pub code: String,
    pub error: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Unauthenticated {
    pub server: String,
    pub source: String,
    pub tools: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct Duplicate {
    pub name: String,
    pub entries: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct HeavySkill {
    pub name: String,
    pub source: String,
    pub tokens: f64,
    pub uses: u32,
}

/// Something deadweight will not edit for you, with the command that does it.
#[derive(Debug, Clone, Serialize)]
pub struct ManualStep {
    pub what: String,
    pub how: String,
    pub tokens: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Analysis {
    pub session_file: String,
    pub session_started: Option<String>,
    /// Tokens the API billed for the first request of the latest session.
    pub measured_tokens: Option<u64>,
    pub median_tokens: Option<u64>,
    /// Claude Code's own system prompt, tools and listed built-ins
    /// (the measured total minus your setup when a measurement exists).
    pub builtin_tokens: f64,
    pub builtin_tools: Option<usize>,
    /// Everything your setup adds on top.
    pub setup_tokens: f64,
    pub categories: Vec<CategoryTotal>,
    pub sources: Vec<SourceSummary>,
    pub items: Vec<Item>,
    pub failed_mcp: Vec<FailedServer>,
    pub unauthenticated: Vec<Unauthenticated>,
    pub duplicates: Vec<Duplicate>,
    /// Skills shown to the model by name only because the listing is full.
    pub skills_without_description: Vec<String>,
    /// Of those, the ones you actually used in the window.
    pub used_skills_without_description: Vec<String>,
    pub skill_listing_chars: usize,
    pub heavy_skills: Vec<HeavySkill>,
    pub manual: Vec<ManualStep>,
    pub skills_listed: usize,
    pub skills_unused: usize,
    pub window_days: u64,
    pub sessions: usize,
    pub history_days: u64,
}

/// Replace characters Claude Code strips from MCP server names in tool names.
pub fn normalize_server(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

struct Attributor<'a> {
    inv: &'a Inventory,
    user_mcp_norm: HashMap<String, String>,
}

impl<'a> Attributor<'a> {
    fn new(inv: &'a Inventory) -> Self {
        let user_mcp_norm = inv
            .user_mcp
            .iter()
            .map(|n| (normalize_server(n), n.clone()))
            .collect();
        Self { inv, user_mcp_norm }
    }

    fn prefixed(&self, name: &str) -> Option<Source> {
        let (prefix, _) = name.split_once(':')?;
        Some(match self.inv.plugin_by_name(prefix) {
            Some(p) => Source::Plugin(p.key.clone()),
            None => Source::Bundle(prefix.to_owned()),
        })
    }

    fn skill(&self, name: &str) -> Source {
        if let Some(s) = self.prefixed(name) {
            return s;
        }
        if self.inv.user_skills.contains_key(name) {
            Source::UserSkill(name.to_owned())
        } else if self.inv.project_skills.contains(name) {
            Source::ProjectSkill(name.to_owned())
        } else {
            Source::BuiltIn
        }
    }

    fn agent(&self, name: &str) -> Source {
        if let Some(s) = self.prefixed(name) {
            return s;
        }
        if self.inv.user_agents.contains(name) {
            Source::UserAgent(name.to_owned())
        } else {
            Source::BuiltIn
        }
    }

    /// Source of a normalized MCP server id as it appears in `mcp__<id>__tool`.
    fn server(&self, id: &str) -> Source {
        if let Some(rest) = id.strip_prefix("plugin_") {
            let best = self
                .inv
                .plugins
                .iter()
                .filter(|p| rest == p.name || rest.starts_with(&format!("{}_", p.name)))
                .max_by_key(|p| (p.name.len(), p.enabled_by_user));
            if let Some(p) = best {
                return Source::Plugin(p.key.clone());
            }
        }
        if let Some(rest) = id.strip_prefix("claude_ai_") {
            return Source::Connector(rest.replace('_', " "));
        }
        match self.user_mcp_norm.get(id) {
            Some(n) => Source::UserMcp(n.clone()),
            None => Source::OtherMcp(id.to_owned()),
        }
    }

    /// Source of an MCP server by its display name (`plugin:x:y`, `claude.ai X`, `name`).
    fn server_display(&self, name: &str) -> Source {
        self.server(&normalize_server(name))
    }

    fn tool(&self, name: &str) -> (Source, Option<String>) {
        match name.strip_prefix("mcp__").and_then(|r| r.split_once("__")) {
            Some((server, _)) => (self.server(server), Some(server.to_owned())),
            None => (Source::BuiltIn, None),
        }
    }

    fn hook(&self, command: &str) -> Source {
        if command.is_empty() {
            return Source::UnknownHook;
        }
        let owner = self
            .inv
            .plugins
            .iter()
            .filter(|p| p.hook_commands.iter().any(|c| c == command))
            .max_by_key(|p| p.enabled_by_user);
        if let Some(p) = owner {
            Source::Plugin(p.key.clone())
        } else if self.inv.settings_hook_commands.iter().any(|c| c == command) {
            Source::SettingsHook
        } else {
            Source::UnknownHook
        }
    }
}

pub fn analyze(inv: &Inventory, hist: &History, window_days: u64) -> Option<Analysis> {
    let snap = hist.latest.as_ref()?;
    let usage = &hist.usage;
    let attr = Attributor::new(inv);

    let tok = |kind: Kind, chars: usize| chars as f64 / kind.chars_per_token();

    let count = |m: &HashMap<String, u32>, k: &str| m.get(k).copied().unwrap_or(0);

    let mut items = Vec::new();
    for (name, chars) in &snap.skills {
        let source = attr.skill(name);
        let mut uses = count(&usage.skills, name) + count(&usage.commands, name);
        if matches!(source, Source::Plugin(_)) {
            if let Some((_, base)) = name.split_once(':') {
                uses += count(&usage.commands, base);
            }
        }
        items.push(Item {
            kind: Kind::Skill,
            name: name.clone(),
            source,
            tokens: tok(Kind::Skill, *chars),
            uses,
        });
    }

    let mut server_uses: HashMap<String, u32> = HashMap::new();
    for (tool, n) in &usage.mcp_tools {
        if let Some((server, _)) = tool.strip_prefix("mcp__").and_then(|r| r.split_once("__")) {
            *server_uses.entry(server.to_owned()).or_default() += n;
        }
    }

    let mut tools_by_server: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    for name in &snap.tools {
        let (source, server) = attr.tool(name);
        if let Some(s) = &server {
            tools_by_server.entry(s.clone()).or_default().push(name);
        }
        items.push(Item {
            kind: Kind::Tool,
            name: name.clone(),
            source,
            tokens: tok(Kind::Tool, name.len() + 1),
            uses: count(&usage.mcp_tools, name),
        });
    }

    for (name, chars) in &snap.mcp_instructions {
        items.push(Item {
            kind: Kind::McpInstructions,
            name: name.clone(),
            source: attr.server_display(name),
            tokens: tok(Kind::McpInstructions, *chars),
            uses: count(&server_uses, &normalize_server(name)),
        });
    }

    for (name, chars) in &snap.agents {
        items.push(Item {
            kind: Kind::Agent,
            name: name.clone(),
            source: attr.agent(name),
            tokens: tok(Kind::Agent, *chars),
            uses: count(&usage.agents, name),
        });
    }

    for h in &snap.hooks {
        let source = attr.hook(&h.command);
        let name = if h.command.is_empty() {
            h.event.clone()
        } else {
            format!("{}: {}", h.event, h.command)
        };
        items.push(Item {
            kind: Kind::Hook,
            name,
            source,
            tokens: tok(Kind::Hook, h.chars),
            uses: 0,
        });
    }

    for (path, chars) in &inv.memory_files {
        let p = path.display().to_string();
        items.push(Item {
            kind: Kind::Memory,
            name: p.clone(),
            source: Source::Memory(p),
            tokens: tok(Kind::Memory, *chars),
            uses: 0,
        });
    }

    // Plugin usage also counts things that were not in the latest listing.
    let mut plugin_uses: HashMap<String, u32> = HashMap::new();
    let mut credit = |source: Source, n: u32| {
        if let Source::Plugin(k) = source {
            *plugin_uses.entry(k).or_default() += n;
        }
    };
    for (name, n) in usage.skills.iter().chain(usage.agents.iter()) {
        if let Some(s) = attr.prefixed(name) {
            credit(s, *n);
        }
    }
    for (name, n) in &usage.commands {
        if let Some(s) = attr.prefixed(name) {
            credit(s, *n);
        } else {
            for p in inv.plugins.iter().filter(|p| p.entry_names.contains(name)) {
                credit(Source::Plugin(p.key.clone()), *n);
            }
        }
    }
    for (server, n) in &server_uses {
        credit(attr.server(server), *n);
    }

    // Per-source totals.
    let mut by_source: BTreeMap<Source, SourceSummary> = BTreeMap::new();
    for it in &items {
        let s = by_source
            .entry(it.source.clone())
            .or_insert_with(|| SourceSummary {
                source: it.source.clone(),
                label: it.source.label(),
                tokens: 0.0,
                skills: 0,
                tools: 0,
                agents: 0,
                mcp_blocks: 0,
                hook_tokens: 0.0,
                uses: 0,
                runs_hooks: false,
            });
        s.tokens += it.tokens;
        match it.kind {
            Kind::Skill => s.skills += 1,
            Kind::Tool => s.tools += 1,
            Kind::Agent => s.agents += 1,
            Kind::McpInstructions => s.mcp_blocks += 1,
            Kind::Hook => s.hook_tokens += it.tokens,
            Kind::Memory => {}
        }
        // Instruction blocks repeat their server's tool uses; don't double count.
        if it.kind != Kind::McpInstructions {
            s.uses += it.uses;
        }
    }
    for s in by_source.values_mut() {
        if let Source::Plugin(key) = &s.source {
            s.uses = s.uses.max(plugin_uses.get(key).copied().unwrap_or(0));
            s.runs_hooks = inv
                .plugins
                .iter()
                .any(|p| &p.key == key && !p.hook_commands.is_empty());
        }
    }
    let mut sources: Vec<SourceSummary> = by_source.into_values().collect();
    sources.sort_by(|a, b| b.tokens.total_cmp(&a.tokens));

    let mut categories: BTreeMap<Kind, CategoryTotal> = BTreeMap::new();
    for it in items.iter().filter(|i| !i.source.is_builtin()) {
        let c = categories.entry(it.kind).or_insert(CategoryTotal {
            kind: it.kind,
            count: 0,
            tokens: 0.0,
        });
        c.count += 1;
        c.tokens += it.tokens;
    }
    let mut categories: Vec<CategoryTotal> = categories.into_values().collect();
    categories.sort_by(|a, b| b.tokens.total_cmp(&a.tokens));

    let setup_tokens: f64 = categories.iter().map(|c| c.tokens).sum();
    let listed_builtin: f64 = items
        .iter()
        .filter(|i| i.source.is_builtin())
        .map(|i| i.tokens)
        .sum();
    // The measured total is ground truth; Claude Code gets whatever your setup
    // does not explain.
    let builtin_tokens = match (snap.first_request_tokens, snap.builtin_chars) {
        (Some(m), _) => (m as f64 - setup_tokens).max(0.0),
        (None, Some(c)) => c as f64 / SCHEMA_CHARS_PER_TOKEN + listed_builtin,
        (None, None) => listed_builtin,
    };

    let failed_mcp = snap
        .failed_mcp
        .iter()
        .map(|f| FailedServer {
            source: attr.server_display(&f.name).label(),
            name: f.name.clone(),
            code: f.code.clone(),
            error: f.error.clone(),
        })
        .collect();

    let unauthenticated = tools_by_server
        .iter()
        .filter(|(_, tools)| {
            tools
                .iter()
                .all(|t| t.ends_with("__authenticate") || t.ends_with("__complete_authentication"))
        })
        .map(|(server, tools)| {
            let source = attr.server(server);
            let label = source.label();
            let pretty = match &source {
                Source::Plugin(_) => server
                    .strip_prefix(&format!("plugin_{label}_"))
                    .map(|rest| format!("{label}/{rest}"))
                    .unwrap_or_else(|| server.clone()),
                _ => server.clone(),
            };
            Unauthenticated {
                server: pretty,
                source: label,
                tools: tools.len(),
            }
        })
        .collect();

    let mut by_base: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for (name, _) in &snap.skills {
        let base = name.rsplit(':').next().unwrap_or(name);
        by_base.entry(base).or_default().push(name.clone());
    }
    let duplicates = by_base
        .into_iter()
        .filter(|(_, v)| v.len() > 1)
        .map(|(k, entries)| Duplicate {
            name: k.to_owned(),
            entries,
        })
        .collect();

    let skill_items: Vec<&Item> = items.iter().filter(|i| i.kind == Kind::Skill).collect();
    let skills_unused = skill_items.iter().filter(|i| i.uses == 0).count();
    let skill_uses: HashMap<&str, u32> = skill_items
        .iter()
        .map(|i| (i.name.as_str(), i.uses))
        .collect();
    let used_skills_without_description = snap
        .skills_without_description
        .iter()
        .filter(|n| skill_uses.get(n.as_str()).copied().unwrap_or(0) > 0)
        .cloned()
        .collect();

    let mut heavy_skills: Vec<HeavySkill> = skill_items
        .iter()
        .filter(|i| i.tokens >= HEAVY_SKILL_TOKENS && !i.source.is_builtin())
        .map(|i| HeavySkill {
            name: i.name.clone(),
            source: i.source.label(),
            tokens: i.tokens,
            uses: i.uses,
        })
        .collect();
    heavy_skills.sort_by(|a, b| b.tokens.total_cmp(&a.tokens));

    // Unused things deadweight leaves to you, with what removes them.
    let manual = sources
        .iter()
        .filter(|s| s.uses == 0 && s.tokens >= 1.0)
        .filter_map(|s| {
            let how = match &s.source {
                Source::UserMcp(n) => format!("claude mcp remove \"{n}\""),
                Source::Connector(n) => format!("claude.ai > Settings > Connectors > {n}"),
                _ => return None,
            };
            Some(ManualStep {
                what: format!("{} {}", s.source.kind_label(), s.label),
                how,
                tokens: s.tokens,
            })
        })
        .collect();

    let mut recent = hist.recent_first_requests.clone();
    recent.sort_unstable();
    let median_tokens = (recent.len() >= 3).then(|| recent[recent.len() / 2]);

    Some(Analysis {
        session_file: snap.file.display().to_string(),
        session_started: snap.started_at.clone(),
        measured_tokens: snap.first_request_tokens,
        median_tokens,
        builtin_tokens,
        builtin_tools: snap.builtin_tools,
        setup_tokens,
        categories,
        sources,
        skills_listed: skill_items.len(),
        skills_unused,
        items,
        failed_mcp,
        unauthenticated,
        duplicates,
        skills_without_description: snap.skills_without_description.clone(),
        used_skills_without_description,
        skill_listing_chars: snap.skill_listing_chars,
        heavy_skills,
        manual,
        window_days,
        sessions: usage.sessions,
        history_days: usage.history_days,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_names_normalize_like_tool_names() {
        assert_eq!(
            normalize_server("plugin:vercel:vercel"),
            "plugin_vercel_vercel"
        );
        assert_eq!(
            normalize_server("claude.ai Claude Docs"),
            "claude_ai_Claude_Docs"
        );
        assert_eq!(normalize_server("google calendar"), "google_calendar");
        assert_eq!(normalize_server("bio-research"), "bio-research");
    }
}
