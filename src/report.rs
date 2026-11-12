//! Human-readable terminal report.

use anstream::println;
use anstyle::{AnsiColor, Style};

use crate::analyze::{Analysis, Kind, Source};
use crate::fix::Plan;
use crate::util::{bar, fmt_tokens, plural, short_time};

const BOLD: Style = Style::new().bold();
const DIM: Style = Style::new().dimmed();
const RED: Style = AnsiColor::Red.on_default().bold();
const YELLOW: Style = AnsiColor::Yellow.on_default().bold();
const GREEN: Style = AnsiColor::Green.on_default().bold();
const CYAN: Style = AnsiColor::Cyan.on_default();

fn paint(style: Style, text: impl std::fmt::Display) -> String {
    format!("{style}{text}{style:#}")
}

pub fn print(a: &Analysis, plan: &Plan, show_all: bool) {
    println!();
    println!(
        "  {}  {}",
        paint(BOLD, "deadweight"),
        paint(DIM, "what your Claude Code setup costs on every request")
    );
    let when = a
        .session_started
        .as_deref()
        .map(short_time)
        .unwrap_or_else(|| "unknown time".into());
    println!(
        "  {}",
        paint(
            DIM,
            format!(
                "latest session {when} · {} {} in the last {} days",
                a.sessions,
                plural(a.sessions, "session", "sessions"),
                a.window_days
            )
        )
    );
    println!();

    headline(a);
    categories(a);
    sources(a, show_all);
    problems(a);
    footer(a, plan);
}

fn headline(a: &Analysis) {
    match a.measured_tokens {
        Some(m) => {
            println!(
                "  Before you type a word, every request carries {}",
                paint(RED, format!("{} tokens", fmt_tokens(m as f64)))
            );
            let mut note = "measured from your latest session".to_string();
            if let Some(med) = a.median_tokens {
                note.push_str(&format!(
                    "; median of recent sessions {}",
                    fmt_tokens(med as f64)
                ));
            }
            println!("  {}", paint(DIM, note));
        }
        None => println!(
            "  Your setup adds about {} tokens to every request",
            paint(RED, fmt_tokens(a.setup_tokens))
        ),
    }
    let builtin = match a.builtin_tools {
        Some(n) => format!("Claude Code itself (system prompt + {n} tools)"),
        None => "Claude Code itself".to_string(),
    };
    println!(
        "  {} from your setup · ~{} from {builtin}",
        paint(BOLD, format!("~{}", fmt_tokens(a.setup_tokens))),
        fmt_tokens(a.builtin_tokens)
    );
    println!();
}

fn categories(a: &Analysis) {
    println!("  {}", paint(BOLD, "WHAT YOUR SETUP ADDS"));
    let max = a.categories.first().map(|c| c.tokens).unwrap_or(0.0);
    for c in &a.categories {
        let label = format!("{} ({})", c.kind.label(), c.count);
        println!(
            "  {:<34} {:>7}  {}",
            label,
            format!("~{}", fmt_tokens(c.tokens)),
            paint(CYAN, bar(c.tokens, max, 24))
        );
    }
    println!();
}

fn sources(a: &Analysis, show_all: bool) {
    let list: Vec<_> = a
        .sources
        .iter()
        .filter(|s| !s.source.is_builtin() && s.tokens >= 1.0)
        .collect();
    let limit = if show_all { list.len() } else { 12 };
    println!("  {}", paint(BOLD, "BIGGEST SOURCES"));
    println!(
        "  {}",
        paint(
            DIM,
            format!(
                "{:<30} {:<13} {:>7}  {:>6}  {}",
                "name", "type", "tokens", "used", "contents"
            )
        )
    );
    for s in list.iter().take(limit) {
        let mut parts = Vec::new();
        if s.hook_tokens >= 1.0 {
            parts.push(format!("hooks {}", fmt_tokens(s.hook_tokens)));
        }
        if s.tools > 0 {
            parts.push(format!("{} {}", s.tools, plural(s.tools, "tool", "tools")));
        }
        if s.skills > 0 {
            parts.push(format!(
                "{} {}",
                s.skills,
                plural(s.skills, "skill", "skills")
            ));
        }
        if s.agents > 0 {
            parts.push(format!(
                "{} {}",
                s.agents,
                plural(s.agents, "agent", "agents")
            ));
        }
        let used = if matches!(s.source, Source::Memory(_)) || s.hook_tokens >= s.tokens {
            paint(DIM, format!("{:>6}", "n/a"))
        } else if s.uses == 0 {
            paint(RED, format!("{:>6}", "never"))
        } else {
            paint(GREEN, format!("{:>6}", format!("{}×", s.uses)))
        };
        println!(
            "  {:<30} {:<13} {:>7}  {}  {}",
            truncate(&s.label, 30),
            s.source.kind_label(),
            format!("~{}", fmt_tokens(s.tokens)),
            used,
            paint(DIM, parts.join(" · "))
        );
    }
    if list.len() > limit {
        println!(
            "  {}",
            paint(DIM, format!("… {} more (use --all)", list.len() - limit))
        );
    }
    println!();
}

fn problems(a: &Analysis) {
    let mut lines: Vec<String> = Vec::new();
    let bare = a.skills_without_description.len();
    if bare > 0 {
        lines.push(format!(
            "{} {} of {} skills are listed {}: the skill listing is full",
            paint(RED, "✗"),
            bare,
            a.skills_listed,
            paint(BOLD, "without a description"),
        ));
        lines.push(paint(
            DIM,
            format!(
                "    (~{} characters). Claude sees only their names, so it can't tell when to use them.",
                fmt_tokens(a.skill_listing_chars as f64)
            ),
        ));
        if !a.used_skills_without_description.is_empty() {
            lines.push(paint(
                DIM,
                format!(
                    "    Including {} you actually use: {}",
                    a.used_skills_without_description.len(),
                    preview(&a.used_skills_without_description, 5)
                ),
            ));
        }
    }
    if !a.failed_mcp.is_empty() {
        lines.push(format!(
            "{} {} MCP {} failed to connect at startup",
            paint(RED, "✗"),
            a.failed_mcp.len(),
            plural(a.failed_mcp.len(), "server", "servers")
        ));
        for f in &a.failed_mcp {
            lines.push(paint(DIM, format!("    {} ({})", f.name, f.code)));
        }
    }
    if !a.unauthenticated.is_empty() {
        let tools: usize = a.unauthenticated.iter().map(|u| u.tools).sum();
        lines.push(format!(
            "{} {} MCP {} never signed in, still listing {} auth-only tools",
            paint(YELLOW, "!"),
            a.unauthenticated.len(),
            plural(a.unauthenticated.len(), "server", "servers"),
            tools
        ));
        let names: Vec<String> = a.unauthenticated.iter().map(|u| u.server.clone()).collect();
        lines.push(paint(DIM, format!("    {}", preview(&names, 6))));
    }
    if !a.duplicates.is_empty() {
        lines.push(format!(
            "{} {} skill {} listed more than once",
            paint(YELLOW, "!"),
            a.duplicates.len(),
            plural(a.duplicates.len(), "name is", "names are")
        ));
        for d in a.duplicates.iter().take(6) {
            lines.push(paint(
                DIM,
                format!("    {}: {}", d.name, d.entries.join(", ")),
            ));
        }
    }
    if a.skills_listed > 0 {
        let unused_tokens: f64 = a
            .items
            .iter()
            .filter(|i| i.kind == Kind::Skill && i.uses == 0 && !i.source.is_builtin())
            .map(|i| i.tokens)
            .sum();
        lines.push(format!(
            "{} {} of {} listed skills were not used in {} days (~{} per request)",
            paint(YELLOW, "!"),
            a.skills_unused,
            a.skills_listed,
            a.window_days,
            fmt_tokens(unused_tokens)
        ));
    }
    for h in a.heavy_skills.iter().take(3) {
        lines.push(format!(
            "{} {} alone costs ~{} tokens per request (used {}× in {} days)",
            paint(YELLOW, "!"),
            paint(BOLD, &h.name),
            fmt_tokens(h.tokens),
            h.uses,
            a.window_days
        ));
    }
    if lines.is_empty() {
        return;
    }
    println!("  {}", paint(BOLD, "PROBLEMS"));
    for l in lines {
        println!("  {l}");
    }
    println!();
}

fn footer(a: &Analysis, plan: &Plan) {
    if a.sessions < 5 {
        println!(
            "  {}",
            paint(
                YELLOW,
                format!(
                    "Only {} {} of history: usage numbers are thin. Come back after a few more sessions.",
                    a.sessions,
                    plural(a.sessions, "session", "sessions")
                )
            )
        );
    }
    if !plan.is_empty() {
        let mut what = Vec::new();
        if !plan.plugins.is_empty() {
            what.push(format!(
                "disable {} unused {}",
                plan.plugins.len(),
                plural(plan.plugins.len(), "plugin", "plugins")
            ));
        }
        if !plan.skills.is_empty() {
            what.push(format!(
                "park {} unused {}",
                plan.skills.len(),
                plural(plan.skills.len(), "skill", "skills")
            ));
        }
        println!(
            "  Run {} to {} and save {} per request.",
            paint(BOLD, "deadweight fix"),
            what.join(" and "),
            paint(GREEN, format!("~{} tokens", fmt_tokens(plan.tokens)))
        );
    }
    if !plan.hooked.is_empty() {
        let t: f64 = plan.hooked.iter().map(|p| p.tokens).sum();
        let names: Vec<String> = plan.hooked.iter().map(|p| p.label.clone()).collect();
        println!(
            "  {}",
            paint(
                DIM,
                format!(
                    "{} more unused {} {} hooks ({}, ~{}); review, or add --include-hooked.",
                    names.len(),
                    plural(names.len(), "plugin", "plugins"),
                    plural(names.len(), "runs", "run"),
                    preview(&names, 4),
                    fmt_tokens(t)
                )
            )
        );
    }
    if !a.manual.is_empty() {
        println!();
        println!("  {}", paint(BOLD, "UNUSED, BUT YOURS TO REMOVE"));
        for m in a.manual.iter().take(6) {
            println!(
                "  {:<34} ~{:<6} {}",
                truncate(&m.what, 34),
                fmt_tokens(m.tokens),
                paint(CYAN, &m.how)
            );
        }
    }
    println!();
    let basis = if a.measured_tokens.is_some() {
        "The total is measured; the breakdown is estimated from text size."
    } else {
        "Figures are estimated from text size."
    };
    println!("  {}", paint(DIM, basis));
    println!();
}

pub fn print_plan(plan: &Plan) {
    println!();
    if !plan.plugins.is_empty() {
        println!(
            "  {}",
            paint(
                BOLD,
                "Disable these plugins (settings.json → enabledPlugins: false)"
            )
        );
        for p in &plan.plugins {
            println!("    {:<56} ~{}", p.key, fmt_tokens(p.tokens));
        }
    }
    if !plan.skills.is_empty() {
        println!(
            "  {}",
            paint(BOLD, "Move these skills into the backup folder")
        );
        for s in &plan.skills {
            println!("    {:<56} ~{}", s.name, fmt_tokens(s.tokens));
        }
    }
    println!(
        "\n  Saves {} per request. Undo any time with {}.",
        paint(GREEN, format!("~{} tokens", fmt_tokens(plan.tokens))),
        paint(BOLD, "deadweight undo")
    );
}

pub fn success(msg: &str) {
    println!("  {} {msg}", paint(GREEN, "✓"));
}

pub fn note(msg: &str) {
    println!("  {}", paint(DIM, msg));
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_owned()
    } else {
        let mut t: String = s.chars().take(max - 1).collect();
        t.push('…');
        t
    }
}

fn preview(names: &[String], n: usize) -> String {
    let mut s = names.iter().take(n).cloned().collect::<Vec<_>>().join(", ");
    if names.len() > n {
        s.push_str(&format!(", +{} more", names.len() - n));
    }
    s
}
