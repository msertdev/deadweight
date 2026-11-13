# Changelog

## 0.2.0

- New: user-scope MCP servers that are only used in one or two projects are listed with those projects, so you can move them out of the global config. Usage is counted per project from the session transcripts. Suggested by a reader on dev.to.

## 0.1.0

First release.

- Measures what every Claude Code request carries before you type, from the API usage recorded in your latest session.
- Attributes skill listings, deferred tool names, MCP instructions, agent listings and hook injections to the plugin, skill, MCP server or connector that added them.
- Counts real usage of skills, MCP tools, agents and slash commands across all sessions in the window, including subagents.
- Detects skills listed without a description because the skill listing is full, failed and never-authenticated MCP servers, duplicate skill names, and very large skill descriptions.
- `fix` disables unused plugins and parks unused user skills, with a backup and manifest; `undo` reverts it.
- `--json` output and a Claude Code plugin with a `/deadweight` command.
