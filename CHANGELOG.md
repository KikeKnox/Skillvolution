# Changelog

All notable changes to this project are documented here.
Format follows [Keep a Changelog](https://keepachangelog.com), except that
each version heading is `## [vX.Y.Z]` with the date on the next line: the
release tooling uses the heading as the GitHub release title.

## [v0.6.0]

_2026-11-06_

Subagents: the vault can now hold agent definitions next to skills and generate client
files from them. The vault migrates to schema v6 on first use: run
`skillvolution backup <path>` before upgrading (an older binary can't open it), then rerun
`skillvolution setup` (or `skillvolution doctor --fix`) to install the `evolution` v13 skill.

### Added

- A second kind of vault entry, `agent` (`AGENT.md`: `tier` high|mid|low, optional `effort`,
  `tools`, `skills`, `scope`), fixed on an id's first publish. Agents are excluded from
  search, the catalog and suggestions. `publish` accepts `AGENT.md` files and
  `PATH/*/AGENT.md`; `list --kind agent` shows the tier (`no agents` when empty); `get`
  prints an agent as `AGENT.md`, and also the built-ins; `show` displays kind, tier, tools
  and skills.
- `skillvolution agents models [set CLIENT TIER MODEL [--effort E] | unset CLIENT TIER]`,
  `add ID... [--global | --project KEY] [--force]`, `remove ID... [--global]`,
  `sync [--apply [--force]] [--global | --project KEY]` and `import PATH [-o DIR]` generate
  Claude Code and OpenCode agent files, with marker and hash ownership. Sync is a dry run
  without `--apply` and never automatic; `--force` overwrites foreign or hand-edited files
  after a backup. OpenCode files carry an explicit `permission:` map, and a same id at both
  project and user level is warned about.
- Built-in agents `sv-high`, `sv-mid` and `sv-low`, available without a vault row.
- Generated agent files pin each skill as `id vN (#hash)`, a content digest, so new text under
  the same version number rewrites the file; `agents sync` flags it (`! skill X vN content
  changed`) and says which pin changed instead of "rendering changed". Skills that are not
  available are left out of the file (`! skill X dropped (not available)`), and deprecated
  ones get a `[warning] agent X: skill Y is deprecated` from `add`, `sync` and `doctor`.
- `purge ID --version N` of a skill's current version is refused when agents list it, like a
  full purge (`--force` overrides).
- Claude Code agents that deny `edit` also deny `MultiEdit`.
- `agents import` of a generated project-scoped agent keeps `scope: project` and its tags;
  `import` of an export with unknown agent `meta` keys is rejected with a clear message.
- `purge` refuses a skill that an agent lists in `skills:` unless `--force`.
- Agent validation: `tier` is required, `tools: []` is rejected (omit it for all tools),
  `skills:` must be published skills visible from the agent's scope (at most 8, needs `mcp`
  in `tools`), and `sv-*` plus client built-in and managed-skill names are reserved ids.
- `doctor` agent checks (warn-only): hand-edited, outdated, missing, deprecated or non-agent
  files, same id at both levels, no model for a tier, unavailable `skills:`, catalog size.
- The Stop hook and the OpenCode plugin count a delegation to a managed vault agent as work
  that needs an outcome report (`report_skill_outcome` accepts the agent id with the
  version from the file's marker).

### Changed

- Agent skill availability is
  checked against the agent's own scope; importing an edited managed agent file notes the
  edits it does not import and an older marker version; `purge` warns about skills an agent's
  newly current revision lists that are unavailable, and its skill warning points to
  `agents sync --apply` when a published version remains; `export` keeps unknown agent meta
  keys verbatim; the agent-id conflict message mentions `skillvolution publish` only over MCP.
- The vault opens with `synchronous=NORMAL` when WAL actually took effect: commits no longer
  fsync one by one, so concurrent writers stop timing out on a busy disk. An OS crash or power
  loss can lose the last commits; the vault is never corrupted.
- `export` writes format version 2 when agents are present; older binaries refuse it.
- `get_skill` refuses an agent id and tells the model to delegate to it.
- `evolution` v13 covers reporting on delegated agents.
- The review reminder ends with `Delegated vault agents to report: <id> v<N>, ...` when the span
  delegated to managed vault agents (Claude Code hook and OpenCode plugin). The OpenCode plugin
  ignores a subagent session's own loads and reports (only its file edits count toward the
  parent) and finds project agents under the plugin's `worktree`.
- `agents add --force` backs up a foreign file containing the managed marker, takes backups
  only after validation and handles repeated ids once; `agents remove` exits 1 when only a
  foreign or edited file exists; `agents import DIR` reports `[rejected]` files and continues;
  symlinked `$HOME` is recognised; `purge --force` names every agent still listing the skill;
  `doctor` advice for user-level agent files says `--global`.
- Agent files deny delegation when restricted (Claude Code `Agent, Task`, OpenCode `task: deny`;
  `task: allow` otherwise), Claude's `shell` covers `Bash, PowerShell, Monitor`, and listed vault
  skills are pinned as `id vN` (`agents sync` flags `skill ID vA -> vB`; `add` notes the pinning
  for agents with shell or edit). The hash ignores only the marker line at its position.
  `agents import` notes widened or dropped settings, and sends `[rejected]` lines to stderr
  without `-o`. `import` (vault) lists imported agents and says to review with `agents sync`.
  Agent metadata rejects unknown fields.
- The Claude Code Stop hook gets `--project KEY` in project installs (re-run `setup` to refresh).

### Fixed

- `agents add --force` backs up a non-UTF-8 foreign file; `purge --force` warns about agents when
  the last published revision goes but superseded rows remain.
- Descriptions containing `#` or escaped quotes survive a `get` then `publish --replace`
  round trip, and a UTF-8 BOM at the start of a `SKILL.md`/`AGENT.md` is handled.

## [v0.5.0]

_2026-11-04_

Sharper prompt-time suggestions (Spanish prompts included), manual publishing and
editing of skills from the CLI, `doctor --fix`, and accountability for loaded skills.
The vault migrates to schema v5 on first use: run `skillvolution backup <path>` before
upgrading, then rerun `skillvolution setup` (or `skillvolution doctor --fix`) to install
the `evolution` v11 and `skill-acquisition` v2 skills.

### Added

- `skillvolution publish PATH` publishes a `SKILL.md` yourself (you are the evaluator,
  so no verdict is needed), or every `PATH/*/SKILL.md` of a directory such as
  `~/.claude/skills`, with one `[published]/[skipped]/[rejected]` line per file. The
  frontmatter supplies `name`, `description`, `tags` and `scope`; every problem is
  reported at once with a template; `--replace` publishes a new version. All other
  validation, including the credential scan, still applies. The `evolution` skill
  forbids agents from running it unless the user asks for a specific file.
- `skillvolution search`, `list` and `get`: `get` prints a skill as a `SKILL.md` that
  `publish --replace` accepts, so a skill can be fetched, edited and republished.
- `get_skill` loads are recorded (once per version, project and UTC day; schema v4).
  `skillvolution outcomes` shows `loaded` and `unreported` counts, and `--unreported`
  lists skills loaded without an outcome report. Loads are local telemetry: not
  exported or imported, and deleted by `purge`.
- `skillvolution doctor --fix` migrates the vault, rebuilds a broken search index,
  re-applies setup to clients that are configured but stale, and re-registers the
  Claude Code MCP server. It never configures a client you haven't set up and never
  touches a vault failing `quick_check`.
- A retrieval benchmark (`cargo test --test retrieval_eval -- --nocapture`): a fixed
  generic vault queried with English and Spanish prompts, with regression floors for
  search and prompt-time suggestions.

### Changed

- Prompt-time suggestions are much more precise: a hit needs two distinct non-generic
  keywords (fix, test, build, error and similar never count) matching its id, tags, or
  description in two different places (a compound tag like `docker-compose` or a
  hyphenated id is one place), the single whole-word id/tag path is gone, and hits
  scoring under half the top hit's score are dropped. Spanish developer words are
  translated to English through a built-in dictionary (inflections included), so
  Spanish prompts find English skills and stop prefix-matching unrelated English words.
- Two forms of one word in a prompt ("deploy deployment") count as one keyword toward
  the two-keyword suggestion gate.
- Search uses the porter stemmer (`restarts` finds `restarting`); the schema v5
  migration rebuilds the search index with it.
- Prompt keywords keep 2-letter technical terms (ci, go, js, db, ...) as whole words,
  and a skill whose current version has 2 or more net `not_applicable` reports is no
  longer suggested.
- Ranking: a republished skill carries half of its previous version's
  `helped - failed` record, so replacing a proven version no longer resets it to zero.
- Schema v5: a revision is only `published` or `superseded`. Legacy `draft` and
  `rejected` rows, and those in imported pre-v5 exports, become `superseded` with a
  `legacy <status>` note prefix.
- `setup` rewrites its own hook entries in their existing slot, never reorders foreign
  hooks, and keeps foreign hook groups that were already empty. It skips
  `claude mcp add-json` when the existing registration is identical, so a fully set-up
  client previews as `No changes.`. `doctor` counts a client as configured only when a
  file carries something setup owns, not a mere mention of `skillvolution`.
- `skill-acquisition` v2 searches the vault with English keywords even when the user
  writes another language.

### Fixed

- `publish` ignores YAML-style trailing comments in frontmatter values
  (`scope: project   # optional`), which used to be rejected or kept as part of the value.

## [v0.4.1]

_2026-10-10_

The first release that ships the v0.4.0 changes — per-prompt vault skill suggestions
(`hook prompt`), the `skill-evaluation` and `skill-acquisition` skills, and outcome
reports for every loaded skill; see v0.4.0 below. The `v0.4.0` tag's CI failed on
Windows and its GitHub release was published without binaries or installers. Rerun
`skillvolution setup` after upgrading to install the new skills and hooks.

### Fixed

- Releases are gated on CI for real: the release workflow's `host` job now requires
  the CI run to succeed. Before, a failing CI left the build jobs "skipped", which the
  generated condition accepted, so `v0.4.0` was published with only
  `dist-manifest.json` and `releases/latest/download/...` returned 404.
- `hook prompt` reads its whole stdin payload before it can fail open, so a client
  still writing the payload never gets a broken pipe.
- A `doctor` test compared a `/`-joined path against doctor's native output and
  failed on Windows.

## [v0.4.0]

_2026-10-04_

Tagged but never published with binaries; see v0.4.1.

### Added

- `setup` now installs three managed skills per client, each at
  `skills/<name>/SKILL.md` with its own `skillvolution-managed:<name>:vN` marker:
  `evolution` (now v11), `skill-evaluation` (the evaluator subagent's rubric: hard
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
