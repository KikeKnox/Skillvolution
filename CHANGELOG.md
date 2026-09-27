# Changelog

All notable changes to this project are documented here.
Format follows [Keep a Changelog](https://keepachangelog.com).

## [0.3.0] - Unreleased

### Added

- New clients: **Codex CLI**, **Gemini CLI**, and **Cursor** — full `setup`
  support (MCP entry, hooks, skill file, permissions) in both global and
  `--project` modes.
- `skillvolution doctor` — read-only health checks for the vault and every
  configured client (`--json` for machine-readable output).
- `skillvolution relocate PATH` — moves the vault to a new path, verifies
  the copy, repoints global client configs (keeping each configured binary
  path), and renames the old file to `.relocated.bak`.
- `skillvolution purge`, `export`, `import`, and `backup` — hard deletion
  with `secure_delete` + `VACUUM`, JSON export/import (format
  `skillvolution-export` v1) with merge semantics, and `VACUUM INTO`
  backups.
- `setup --remove` and `setup --dry-run` — surgical uninstall that keeps
  every entry setup doesn't own, and a unified-diff preview that writes
  nothing.
- `SKILLVOLUTION_DB` environment variable as the default vault path, plus a
  vault-location prompt and a warning when the database sits on a network
  filesystem.
- macOS and Windows release targets and a PowerShell installer
  (`install.ps1`).
- Windows legacy vault path fallback: if `%LOCALAPPDATA%\skillvolution\skills.db`
  doesn't exist but a legacy `%USERPROFILE%\.local\share\skillvolution\skills.db`
  vault does, the legacy vault keeps being used.

### Changed

- Project-scope MCP entry replaced whole (discarding any pre-existing `env`,
  `cwd`, or `url` on that entry), a security hardening against pre-seeded keys
  in untrusted repos; global entries continue to merge and preserve user keys.
- `relocate` validates every global client config it would repoint first (a
  config that can't be read aborts with nothing written), keeps `-wal`/`-shm`
  sidecars renamed alongside the backup, prints a hint when the CLI's default
  vault was relocated, and correctly matches Windows canonical paths.
- `purge` warns when it cannot compact while another connection (an AI client's
  MCP server) holds the vault open, so purged content may linger until a later
  purge compacts it.
- `import` rejects revisions and outcomes whose id differs from the skill they
  are listed under, preventing incorrect vault state.
- MCP `tools/call` no longer answers invalid notifications (missing `name`
  parameter).
- Vault schema v3 with an ordered migration chain: FTS entries addressed by
  `rowid`, one outcome per skill/version/project/UTC day (last report wins),
  legacy drafts retired, and hook-state pruning.
- Devin-style hook state is updated atomically and shared by all
  flag-tracked clients (Devin, Codex, Gemini, Cursor).
- All hook entry points fail open: errors go to stderr and the process
  exits 0.
- `show` defaults to the latest revision when `--version` is omitted.
- Secret scanning widened (credential URLs, webhooks, more token shapes);
  local-development connection strings like `user:pass@localhost` stay
  publishable.
- OpenCode plugin hardened: correct project-key baking, robust session
  handling, and JSON-literal rendering checked by `node --check` in CI.
- Setup writes and JSON merges hardened; clients modeled as a single enum
  with per-scope `changes`/`removals`; the interactive prompt re-asks once
  on an invalid answer.
- Releases are gated on CI (`plan-jobs = ["./ci"]`); installers gained
  version pinning and a fixed TTY probe.
- `install.sh` exits on INT/TERM instead of resuming after cleanup;
  `install.ps1` no longer closes the caller's session under `irm | iex`.

### Fixed

- `doctor` per-client failures are now isolated: a client whose config can't be
  read or parsed gets its own `[fail]` check and the report continues. Adds a
  `vault: relocated` warning when the vault file is missing but a
  `<path>.relocated.bak` backup exists. Deprecated skills with a published
  revision no longer count as search-index orphans.
- Hook review spans: a failed review tool call no longer counts as a
  review, and each transcript span is judged once.
- Doubled slashes in dry-run diff headers for absolute paths.
- `doctor` compares a client against its configured binary when that binary
  still exists.
- The evolution skill (v8) now spells out every server-enforced publish
  rule so agents stop hitting avoidable rejections.

## [0.2.1] - 2026-09-22

### Fixed

- Stop-hook feedback is delivered as `additionalContext` instead of
  stderr + exit 2, so Claude Code continues the same turn.

## [0.2.0] - 2026-09-22

### Added

- Publishing requires an evaluator verdict (`keep global` / `keep project`)
  bound to scope, with content validation and secret scanning enforced
  server-side.
- Search results are ranked by outcomes (helped vs. failed reports).

### Changed

- `CLAUDE.md` is untracked and covered by `.gitignore`.

## [0.1.2] - 2026-09-21

### Added

- Devin CLI client in `setup`.
- The evolution skill now has the agent dispatch a fresh evaluator subagent
  automatically instead of asking first.

## [0.1.1] - 2026-09-15

### Changed

- Install and configure in one command: the installer runs `setup`.
- Clients are configured once per user (global setup); the project is
  detected at runtime from the enclosing git repository.
- Project-key derivation extracted into a shared module.

### Fixed

- Assorted bugs found in pre-release review.

## [0.1.0] - 2026-09-15

Initial release: SQLite vault with FTS5 search and outcome tracking, MCP
server over stdio, Claude Code and OpenCode setup, client hooks, the
evolution skill, and a `curl | sh` installer built with cargo-dist.
