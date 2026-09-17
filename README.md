# Skillvolution

Skillvolution is a shared procedural-memory vault for AI coding agents: a single
SQLite database exposed to Claude Code, OpenCode, and Devin CLI as a local MCP server, where agents search and
apply published skills, report whether they worked, and publish new lessons once a fresh-context
subagent has judged them worth keeping (global or project scope — or discarded). Humans curate the
result afterwards with `deprecate`.

## Install

One command installs the prebuilt binary (Linux x86_64 gnu/musl, aarch64 gnu) to `~/.local/bin`,
detects the installed AI clients (Claude Code, OpenCode, Devin CLI), and asks which ones to
configure:

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://raw.githubusercontent.com/KikeKnox/Skillvolution/main/install.sh | sh
```

[`install.sh`](install.sh) runs the release's cargo-dist installer, which verifies each archive's
SHA-256, adds `~/.local/bin` to `PATH` in your shell profile (skip with
`SKILLVOLUTION_NO_MODIFY_PATH=1`) and writes an install receipt to `~/.config/skillvolution/`;
then it runs `skillvolution setup` (skip with `SKILLVOLUTION_NO_SETUP=1`). The per-client questions
default to yes — just press Enter — and are skipped entirely when there is no terminal or when
`SKILLVOLUTION_NO_INPUT=1`, in which case every detected client is configured. Rerunning the same
command updates the binary; setup is idempotent. To install without configuring anything, use the
release installer directly:
`https://github.com/KikeKnox/Skillvolution/releases/latest/download/skillvolution-installer.sh`.

**Build from source** (requires Rust):

```bash
cargo install --locked --path .
```

## Setup

The install command already runs this. Run it again yourself after installing a new AI client:

```bash
skillvolution setup
```

With no `--project`, this detects the installed clients (a `claude`/`opencode`/`devin` executable
on `PATH` or an existing config directory), asks which of them to configure when a terminal is
attached, and configures the selection **globally, for the current user**: every project then
works with no per-project step. To choose explicitly — and skip the questions — pass
`--client` with a comma-separated list (`--client claude-code,devin`, `--client all`; `both` still
means claude-code + opencode). Claude Code
gets the `evolution` skill, `SessionStart`/`Stop` hooks merged into `settings.json`, and the MCP
server registered at user scope via `claude mcp add-json` (setup prints the command to run
manually if `claude` isn't on `PATH`). OpenCode gets the skill, an `AGENTS.md` reminder, and a
plugin that injects the skill catalog into every session and adds a review reminder to the system
prompt once a session goes idle after doing unreviewed work. Devin CLI gets the skill, the MCP
server in `mcp_config.json`, lifecycle hooks in `config.json` (catalog on `SessionStart`,
unreviewed-work tracking on `PostToolUse`, a blocking `Stop` reminder, `PermissionRequest`
auto-approval for the subagent and vault tools), plus the permissions and `AGENTS.md` rule that
pre-authorize the evaluator subagent. Each project is then identified at runtime by its git repository's
directory name (outside a git repo, only global skills are visible).

Pass `--project PATH [--project-key KEY]` instead to configure a single project's own files rather
than the shared, global config. See `docs/integrations.md` for exactly what each mode writes.

Updating later: rerun the install command; the registered config keeps pointing at
`~/.local/bin/skillvolution`.

## Curation

Publication is automatic: when a session reflects on finished work, a fresh subagent evaluates the
candidate with clean context and decides `global`, `project`, or `discard`; kept lessons go live
immediately. Humans curate the vault afterwards:

```bash
skillvolution show my-skill --version 1 [--json]  # status, evidence, diff (or full JSON)
skillvolution outcomes [--json]                # helped/failed/not_applicable per skill
skillvolution outcomes my-skill [--json]       # this skill's outcome log
skillvolution outcomes --failing [--json]      # skills with failures in current version

skillvolution deprecate my-skill               # hide from search, keep history
skillvolution undeprecate my-skill
```

## CLI

```
skillvolution [--db PATH] <command>

serve [--project KEY]                Start the MCP server on stdio
show ID --version N [--json]         Show a revision: status, evidence, diff (or full JSON)
deprecate ID / undeprecate ID        Toggle a skill's visibility in search
outcomes [ID] [--failing] [--json]   Summarize outcomes, or list one skill's outcome log
hook <event> [--client devin]        Client hook entry points (see docs/integrations.md)
setup [OPTIONS]                      Configure Skillvolution for Claude Code, OpenCode, and/or Devin CLI
```

Run `skillvolution <command> --help` for exact flags. The database defaults to
`$XDG_DATA_HOME/skillvolution/skills.db`, falling back to `~/.local/share/skillvolution/skills.db`;
it is created automatically on first use, no separate init step.

See `docs/architecture.md` for modules and data model, and `docs/integrations.md` for what `setup`
writes and how the review hooks, the OpenCode plugin, and the evolution skill work.

## Limitations

- Evidence attached to a publication is supplied by the publishing agent and is not independently
  verified by the vault; the evaluator subagent judges it from the text alone, so deprecate
  publications whose evidence looks invented.
- The project key is the sanitized name of the enclosing git repository's root directory, so two
  unrelated repositories that happen to share a directory name share project scope.
- Live, end-to-end sessions against real Claude Code / OpenCode clients have not been fully
  verified in this repository — in particular, Claude Code hooks actually firing and blocking a
  live session. Tests cover the CLI, the MCP protocol over a real subprocess, the OpenCode
  plugin's logic, and the files `setup` writes, not an authenticated client actually reading them.

## Development

```bash
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
```

## Releasing

1. Bump the version in `Cargo.toml` and commit.
2. Tag the commit: `git tag vX.Y.Z && git push origin vX.Y.Z`
3. The Release workflow (cargo-dist) builds the shell installer and binaries for each target
   platform and publishes them to GitHub Releases with checksums.
