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

## Set up a project

```bash
cd /path/to/project
skillvolution setup
```

This configures Claude Code and/or OpenCode (`--client both|opencode|claude-code`) to use the
vault. See `docs/integrations.md` for exactly what it writes.

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
setup [OPTIONS]                      Configure a project to use the vault
```

Run `skillvolution <command> --help` for exact flags. The database defaults to
`$XDG_DATA_HOME/skillvolution/skills.db`, falling back to `~/.local/share/skillvolution/skills.db`;
it is created automatically on first use, no separate init step.

See `docs/architecture.md` for modules and data model, and `docs/integrations.md` for what `setup`
writes and how the review hooks and evolution skill work.

## Limitations

- Evidence attached to a proposal is supplied by the proposing agent and is not independently
  verified by the vault; review it before publishing.
- Automatic review hooks (SessionStart catalog, Stop review reminder) exist for Claude Code only;
  OpenCode gets the `evolution` skill and an `AGENTS.md` reminder but no lifecycle enforcement.
- Live, end-to-end sessions against real Claude Code / OpenCode clients have not been verified in
  this repository; tests cover the CLI, the MCP protocol over a real subprocess, and the files
  `setup` writes, not an authenticated client actually reading them.

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
