# Changelog

All notable changes to this project are documented here.
Format follows [Keep a Changelog](https://keepachangelog.com), except that
each version heading is `## [vX.Y.Z]` with the date on the next line: the
release tooling uses the heading as the GitHub release title.

## [Unreleased]

## [v0.4.0]

_2026-10-04_

### Added

- `setup` now installs three managed skills per client, each at
  `skills/<name>/SKILL.md` with its own `skillvolution-managed:<name>:vN` marker:
  `evolution` (now v10), `skill-evaluation` (the evaluator subagent's rubric: hard
  gates, a 16-point rubric, and the verdict format `publish_skill` expects), and
  `skill-acquisition` (what an agent follows when it notices mid-task that it lacks
  the know-how). `doctor` checks each skill's version, `setup --remove` deletes each
  owned file and any directory left empty, and a user's own file at one of those paths
  is refused by `setup` ("unowned") and skipped by `--remove`. The `AGENTS.md` block
  and the Cursor rule mention the new skills.
- New `skillvolution hook prompt` subcommand: prompt-time retrieval. It extracts up to
  16 keywords from the submitted prompt (English and Spanish stopwords dropped,
  accents folded), searches the vault, and suggests at most 3 of the top 5 hits that
  match one keyword in their id or tags or two across id, description, and tags; skills
  with two or more net failures are never suggested and a prompt with fewer than two
  keywords is not searched. `setup` wires it as `UserPromptSubmit` for Claude Code,
  Codex, and Devin (Devin not yet verified against a live session) and `BeforeAgent`
  for Gemini CLI, and the OpenCode plugin calls it for each top-level user message
  through `chat.message`. Cursor is not wired: its `beforeSubmitPrompt` cannot inject
  context.
- `get_skill` results carry a `next` field asking the agent to verify the procedure and
  call `report_skill_outcome` with the loaded id and version.

### Changed

- Loading a skill is now work the review reminder holds the agent to: a successful
  `get_skill` counts like a file edit in the Claude Code transcript scan, the
  flag-tracked clients' tool-use hook, and the OpenCode plugin. The reminder
  (`STOP_REASON`, identical in the OpenCode plugin) now asks for an outcome report on
  every loaded skill and names the `skill-evaluation` skill for the evaluator subagent.
- MCP tool descriptions and `initialize` instructions mention per-request suggestions,
  saying which skill is being applied, and the `skill-acquisition` skill; the
  session-start catalog header points to `skill-acquisition` and says matching skills
  are also suggested with each request.
- Project-scope hook commands for `prompt` carry `--project 'KEY'`, like `session-start`.

### Fixed

- Claude Code sessions visibly check the vault again: `hook session-start`
  now sends the catalog as `hookSpecificOutput.additionalContext` JSON
  instead of plain stdout (which arrived as a "hook success" status line),
  and its header asks the agent to say in one line which vault skill it is
  applying, or that none matched, before non-trivial work.
  The OpenCode plugin unwraps that JSON and still accepts plain text.

## [v0.3.1]

_2026-09-27_

### Fixed

- A global OpenCode `opencode.jsonc` with trailing commas no longer makes
  `setup` (and `doctor`) fail: trailing commas in a `.jsonc` config are now
  accepted. Comments are still refused, since rewriting the file would drop them.
- The evolution skill (v9) gives the evaluator subagent the server's field
  limits (one-line reason and description within 280 bytes, tag format,
  `expected_version` 0 for new ids), so its verbatim output no longer gets
  rejected by `publish_skill`.

## [v0.3.0]

_2026-09-27_

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

## [v0.2.1]

_2026-09-22_

### Fixed

- Stop-hook feedback is delivered as `additionalContext` instead of
  stderr + exit 2, so Claude Code continues the same turn.

## [v0.2.0]

_2026-09-22_

### Added

- Publishing requires an evaluator verdict (`keep global` / `keep project`)
  bound to scope, with content validation and secret scanning enforced
  server-side.
- Search results are ranked by outcomes (helped vs. failed reports).

### Changed

- `CLAUDE.md` is untracked and covered by `.gitignore`.

## [v0.1.2]

_2026-09-21_

### Added

- Devin CLI client in `setup`.
- The evolution skill now has the agent dispatch a fresh evaluator subagent
  automatically instead of asking first.

## [v0.1.1]

_2026-09-15_

### Changed

- Install and configure in one command: the installer runs `setup`.
- Clients are configured once per user (global setup); the project is
  detected at runtime from the enclosing git repository.
- Project-key derivation extracted into a shared module.

### Fixed

- Assorted bugs found in pre-release review.

## [v0.1.0]

_2026-09-15_

Initial release: SQLite vault with FTS5 search and outcome tracking, MCP
server over stdio, Claude Code and OpenCode setup, client hooks, the
evolution skill, and a `curl | sh` installer built with cargo-dist.
