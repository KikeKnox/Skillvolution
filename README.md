# Skillvolution

Skillvolution is a shared, human-published procedural-memory vault for AI coding agents: a single
SQLite database exposed to Claude Code and OpenCode as a local MCP server, where agents search and
apply published skills, report whether they worked, and propose new lessons as drafts that only a
human can publish.

## Install

Prebuilt binaries (Linux x86_64 gnu/musl, aarch64 gnu) install to `~/.local/bin`:

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/KikeKnox/Skillvolution/releases/latest/download/skillvolution-installer.sh | sh
```

Rerunning the installer updates in place. For a look before you run it: download the script,
inspect it, then `sh skillvolution-installer.sh`. The script verifies each archive's SHA-256
before installing, adds `~/.local/bin` to `PATH` in your shell profile (skip with
`SKILLVOLUTION_NO_MODIFY_PATH=1`), and writes an install receipt to `~/.config/skillvolution/`.

**Build from source** (requires Rust):

```bash
cargo install --locked --path .
```

## Setup

Run once, after installing:

```bash
skillvolution setup
```

With no `--project`, this configures Claude Code and/or OpenCode (`--client both|opencode|claude-code`)
**globally, for the current user**: every project then works with no per-project step. Claude Code
gets the `evolution` skill, `SessionStart`/`Stop` hooks merged into `settings.json`, and the MCP
server registered at user scope via `claude mcp add-json` (setup prints the command to run
manually if `claude` isn't on `PATH`). OpenCode gets the skill, an `AGENTS.md` reminder, and a
plugin that injects the skill catalog into every session and prompts for review once a session
goes idle after doing work. Each project is then identified at runtime by its git repository's
directory name (outside a git repo, only global skills are visible).

Pass `--project PATH [--project-key KEY]` instead to configure a single project's own files rather
than the shared, global config. See `docs/integrations.md` for exactly what each mode writes.

Updating the binary later: rerun the installer. `skillvolution setup` doesn't need to run again —
the registered config already points at `~/.local/bin/skillvolution`.

## Human review

Agents can only create drafts; a human publishes or rejects them:

```bash
skillvolution drafts [--json]                  # list drafts awaiting review
skillvolution show my-skill --version 1 [--json]  # status, evidence, diff (or full JSON)
skillvolution publish my-skill --version 1
skillvolution reject my-skill --version 1 --note "too narrow"

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
drafts [--json]                      List drafts awaiting review
show ID --version N [--json]         Show a revision: status, evidence, diff (or full JSON)
publish ID --version N               Publish a draft revision
reject ID --version N [--note TEXT]  Reject a draft revision
deprecate ID / undeprecate ID        Toggle a skill's visibility in search
outcomes [ID] [--failing] [--json]   Summarize outcomes, or list one skill's outcome log
hook session-start / hook stop       Claude Code hook entry points
setup [OPTIONS]                      Configure Skillvolution for Claude Code and/or OpenCode
```

Run `skillvolution <command> --help` for exact flags. The database defaults to
`$XDG_DATA_HOME/skillvolution/skills.db`, falling back to `~/.local/share/skillvolution/skills.db`;
it is created automatically on first use, no separate init step.

See `docs/architecture.md` for modules and data model, and `docs/integrations.md` for what `setup`
writes and how the review hooks, the OpenCode plugin, and the evolution skill work.

## Limitations

- Evidence attached to a proposal is supplied by the proposing agent and is not independently
  verified by the vault; review it before publishing.
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
