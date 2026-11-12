use std::path::PathBuf;

use anyhow::{Context, Result};

/// Locations of the Claude Code files deadweight reads and writes.
#[derive(Debug, Clone)]
pub struct Paths {
    /// `~/.claude` (or `$CLAUDE_CONFIG_DIR`).
    pub claude_dir: PathBuf,
    /// `~/.claude.json` (or `$CLAUDE_CONFIG_DIR/.claude.json`).
    pub global_config: PathBuf,
    /// The directory deadweight was run from, used for project-level files.
    pub cwd: PathBuf,
}

impl Paths {
    pub fn resolve(config_dir: Option<PathBuf>) -> Result<Self> {
        let cwd = std::env::current_dir().context("cannot read the current directory")?;
        let explicit = config_dir.or_else(|| {
            std::env::var_os("CLAUDE_CONFIG_DIR")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        });
        let (claude_dir, global_config) = match explicit {
            Some(dir) => {
                let cfg = dir.join(".claude.json");
                (dir, cfg)
            }
            None => {
                let home = home_dir().context("cannot find your home directory")?;
                (home.join(".claude"), home.join(".claude.json"))
            }
        };
        Ok(Self {
            claude_dir,
            global_config,
            cwd,
        })
    }

    pub fn settings(&self) -> PathBuf {
        self.claude_dir.join("settings.json")
    }

    pub fn projects(&self) -> PathBuf {
        self.claude_dir.join("projects")
    }

    pub fn backups(&self) -> PathBuf {
        self.claude_dir.join("deadweight").join("backups")
    }
}

fn home_dir() -> Option<PathBuf> {
    #[allow(deprecated)]
    std::env::home_dir().filter(|p| !p.as_os_str().is_empty())
}
