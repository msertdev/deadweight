mod analyze;
mod fix;
mod inventory;
mod paths;
mod report;
mod transcript;
mod util;

use std::io::{BufRead, IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Result, bail};
use clap::{Parser, Subcommand};
use serde_json::json;

use crate::fix::PlanOptions;
use crate::paths::Paths;

/// Find the dead weight in your Claude Code setup: what every request
/// carries before you type, and what you never use.
#[derive(Parser)]
#[command(name = "deadweight", version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// How many days of session history count as "used"
    #[arg(long, global = true, default_value_t = 30, value_name = "N")]
    days: u64,

    /// Claude Code config directory [default: $CLAUDE_CONFIG_DIR or ~/.claude]
    #[arg(long, global = true, value_name = "DIR")]
    config_dir: Option<PathBuf>,

    /// Print machine-readable JSON
    #[arg(long, global = true)]
    json: bool,

    /// List every source instead of the biggest ones
    #[arg(long, short)]
    all: bool,
}

#[derive(Subcommand)]
enum Command {
    /// Disable unused plugins and park unused skills (backed up, undoable)
    Fix {
        /// Apply without asking
        #[arg(long, short)]
        yes: bool,
        /// Show what would change and stop
        #[arg(long)]
        dry_run: bool,
        /// Also disable unused plugins that run hooks
        #[arg(long)]
        include_hooked: bool,
        /// Never touch this plugin or skill (repeatable)
        #[arg(long, value_name = "NAME")]
        keep: Vec<String>,
        /// Run even with fewer than 5 sessions of history
        #[arg(long)]
        force: bool,
    },
    /// Revert the most recent `fix`
    Undo {
        /// Apply without asking
        #[arg(long, short)]
        yes: bool,
    },
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(code) => code,
        Err(e) => {
            anstream::eprintln!("deadweight: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<ExitCode> {
    let paths = Paths::resolve(cli.config_dir.clone())?;
    match cli.command {
        None => audit(&paths, &cli),
        Some(Command::Fix {
            yes,
            dry_run,
            include_hooked,
            ref keep,
            force,
        }) => {
            let opts = PlanOptions {
                include_hooked,
                keep: keep.clone(),
            };
            run_fix(&paths, &cli, &opts, yes, dry_run, force)
        }
        Some(Command::Undo { yes }) => run_undo(&paths, cli.json, yes),
    }
}

fn load(paths: &Paths, days: u64) -> Result<(inventory::Inventory, analyze::Analysis)> {
    if !paths.claude_dir.is_dir() {
        bail!(
            "no Claude Code config found at {} (set --config-dir or CLAUDE_CONFIG_DIR)",
            paths.claude_dir.display()
        );
    }
    let inv = inventory::Inventory::load(paths);
    let history = transcript::load(&paths.projects(), days);
    match analyze::analyze(&inv, &history, days) {
        Some(a) => Ok((inv, a)),
        None => bail!(
            "no Claude Code session transcripts found under {}. Start a session, then run deadweight again.",
            paths.projects().display()
        ),
    }
}

fn audit(paths: &Paths, cli: &Cli) -> Result<ExitCode> {
    let (inv, a) = load(paths, cli.days)?;
    let plan = fix::plan(&a, &inv, &PlanOptions::default());
    if cli.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({ "analysis": a, "plan": plan }))?
        );
    } else {
        report::print(&a, &plan, cli.all);
    }
    Ok(ExitCode::SUCCESS)
}

fn run_fix(
    paths: &Paths,
    cli: &Cli,
    opts: &PlanOptions,
    yes: bool,
    dry_run: bool,
    force: bool,
) -> Result<ExitCode> {
    let (inv, a) = load(paths, cli.days)?;
    if a.sessions < 5 && !force {
        bail!(
            "only {} session(s) in the last {} days; too little history to call anything unused (use --force to override)",
            a.sessions,
            cli.days
        );
    }
    let plan = fix::plan(&a, &inv, opts);
    if plan.is_empty() {
        if cli.json {
            println!("{}", json!({ "applied": false, "plan": plan }));
        } else {
            report::note("Nothing to fix: everything listed was used recently.");
        }
        return Ok(ExitCode::SUCCESS);
    }
    if dry_run {
        if cli.json {
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({ "applied": false, "plan": plan }))?
            );
        } else {
            report::print_plan(&plan);
        }
        return Ok(ExitCode::SUCCESS);
    }
    if !cli.json {
        report::print_plan(&plan);
    }
    if !yes && !confirm("Apply these changes?")? {
        report::note("Nothing changed.");
        return Ok(ExitCode::SUCCESS);
    }
    let dir = fix::apply(&plan, paths)?;
    if cli.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({ "applied": true, "backup": dir, "plan": plan }))?
        );
    } else {
        report::success(&format!("Done. Backup: {}", dir.display()));
        report::note(
            "Restart Claude Code to load the slimmer setup. Revert with `deadweight undo`.",
        );
    }
    Ok(ExitCode::SUCCESS)
}

fn run_undo(paths: &Paths, json_out: bool, yes: bool) -> Result<ExitCode> {
    let Some((dir, manifest)) = fix::latest_backup(paths)? else {
        if json_out {
            println!("{}", json!({ "restored": false }));
        } else {
            report::note("No deadweight changes to undo.");
        }
        return Ok(ExitCode::SUCCESS);
    };
    if !json_out {
        report::note(&format!(
            "Backup {}: {} plugin(s), {} skill(s)",
            dir.display(),
            manifest.plugins.len(),
            manifest.skills.len()
        ));
    }
    if !yes && !confirm("Restore it?")? {
        report::note("Nothing changed.");
        return Ok(ExitCode::SUCCESS);
    }
    let m = fix::undo(paths, &dir, manifest)?;
    if json_out {
        println!(
            "{}",
            json!({ "restored": true, "backup": dir, "plugins": m.plugins.len(), "skills": m.skills.len() })
        );
    } else {
        report::success("Restored. Restart Claude Code to pick it up.");
    }
    Ok(ExitCode::SUCCESS)
}

fn confirm(question: &str) -> Result<bool> {
    if !std::io::stdin().is_terminal() {
        bail!("not running in a terminal; pass --yes to apply without asking");
    }
    print!("\n  {question} [y/N] ");
    std::io::stdout().flush()?;
    let mut answer = String::new();
    std::io::stdin().lock().read_line(&mut answer)?;
    Ok(matches!(answer.trim(), "y" | "Y" | "yes" | "Yes"))
}
