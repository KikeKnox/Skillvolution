# Architecture

Skillvolution is a single Rust binary (`skillvolution`) that stores reusable
"skills" (verified lessons) in a SQLite vault, serves them to agent clients
over MCP (JSON-RPC on stdio), and wires each client up via `setup` and
client hooks.

## Module map

### CLI / entry

| Module | Role |
|---|---|
| `main.rs` | Arg parsing, command dispatch, `hook` subcommands, project detection |
| `lib.rs` | Re-exports the modules so integration tests and the binary share them |

### MCP

| Module | Role |
|---|---|
| `mcp.rs` | JSON-RPC server loop on stdio; `initialize`, `tools/list`, `tools/call`; tool argument validation |

### Hooks

| Module | Role |
|---|---|
| `hook.rs` | `session-start`, `tool-use`, `stop`, `session-end`, `approve` implementations; Claude transcript scan; per-session work/review flags |

### Vault

| Module | Role |
|---|---|
| `vault/mod.rs` | `Vault` connection setup, pragmas, migrations, validation helpers, deprecate/undeprecate |
| `vault/schema.sql` | Table/view definitions for the initial schema |
| `vault/search.rs` | FTS5 search, ranking, catalog listing |
| `vault/revisions.rs` | Publish logic: versioning, scope, optimistic concurrency, proven-version guard |
| `vault/outcomes.rs` | Outcome recording and listing (`helped`/`failed`/`not_applicable`) |
| `vault/transfer.rs` | `purge`, JSON export/import, `backup` (`VACUUM INTO`) |
| `vault/location.rs` | Default vault path; network-filesystem detection (advisory) |

### Setup / clients

| Module | Role |
|---|---|
| `setup/mod.rs` | `Scope`, `Change`/`Edit` model, atomic apply with rollback, `setup`/`--remove`/`--dry-run` orchestration |
| `setup/client.rs` | `ClientKind` enum, client sets, per-client `global_dir`/`changes`/`removals` dispatch |
| `setup/common.rs` | Shared pieces: config dirs from env, managed skill/`AGENTS.md` changes, MCP server argv |
| `setup/fs_safe.rs` | Atomic file writes with backup; strict-JSON load/merge/remove helpers |
| `setup/hooks.rs` | Builds hook entries for all clients; own entries are recognized by command text so reruns replace instead of duplicating |
| `setup/permissions.rs` | Permission grants merged into client settings (subagent dispatch + MCP tools) |
| `setup/plugin.rs` | Embeds `assets/opencode/skillvolution.js` with `--bin`/`--db` substitution |
| `setup/claude.rs` | Claude Code: settings, `.mcp.json`, skill install, legacy `CLAUDE.md` cleanup |
| `setup/claude_cli.rs` | Shelling out to `claude mcp add-json/remove/get` for user-scope MCP registration |
| `setup/opencode.rs` | OpenCode: `opencode.json`/`opencode.jsonc`, permissions, plugin |
| `setup/devin.rs` | Devin CLI: `mcp_config.json`, `config.json`, `AGENTS.md` |
| `setup/codex.rs` | Codex CLI: `config.toml`, `hooks.json`, `AGENTS.md` |
| `setup/gemini.rs` | Gemini CLI: `settings.json`, `GEMINI.md` |
| `setup/cursor.rs` | Cursor: `mcp.json`, `hooks.json`, `cli.json`, rules file |
| `setup/global.rs` | Global-scope orchestration: client detection, `claude mcp` registration |
| `setup/inspect.rs` | Read-only inspection `doctor` uses to compare files against what setup would write |
| `setup/remove.rs` | `setup --remove` planning and diff printing |
| `setup/prompt.rs` | Interactive client selection via `/dev/tty`; `SKILLVOLUTION_NO_INPUT` handling |

### Doctor / relocate

| Module | Role |
|---|---|
| `doctor.rs` | Read-only health checks (vault + client configuration) |
| `relocate.rs` | Move the vault file and repoint client configs |
| `project.rs` | Derive the project key from the enclosing git repository |

## Data model

From `src/vault/schema.sql` (current schema v3):

| Table / view | Purpose |
|---|---|
| `skills` | One row per skill `id`; `scope` (`global`/`project`, fixed at first publish), `deprecated` flag, `fts_rowid` |
| `revisions` | Every published version: `id`, `version`, `description`, `tags` (JSON), `content`, `evidence`, `expected_version`, `status`, timestamps and review fields |
| `outcomes` | Agent reports: `id`, `version`, `result` (`helped`/`failed`/`not_applicable`), `note`, `project`, `created_at` |
| `hook_state` | Claude Code transcript offsets (incremental Stop scan) |
| `devin_hook_state` | Per-session `worked`/`reviewed` flags; used by every flag-tracked client (Devin, Codex, Gemini, Cursor) |
| `skills_fts` | FTS5 index over id/description/tags/content, `unicode61 remove_diacritics 2` |
| `current_skills` | View: each skill's latest published revision plus helped/failed outcome counts |

## Schema versioning and migrations

- Current version: **v3**. Ordered `MIGRATIONS` chain in `src/vault/mod.rs`:
  - v1 → v2: add `devin_hook_state`.
  - v2 → v3: FTS rows addressed by `rowid`; one outcome per skill/version/project/UTC day; legacy drafts retired; hook-state timestamp columns and pruning support.
- `PRAGMA application_id` is `SKV1`; a non-empty database with a different `application_id` is refused as foreign.
- A database with a newer schema version is refused.
- When the schema is already current the vault opens without taking a write lock, so concurrent read-only opens don't serialize.
- `hook_state`/`devin_hook_state` rows older than 30 days are pruned.

## Search ranking

From `src/vault/search.rs`:

- FTS5 `bm25` column weights: **id 4.0, description 3.0, tags 2.0, content 1.0**. The raw `bm25` score is sign-inverted and clamped at zero so higher is better.
- Outcome multiplier applied to the text score:

  ```text
  1 + (helped - failed) / (ABS(helped - failed) + 2.0)
  ```

  The factor is in `(0, 2)`; `not_applicable` outcomes do not affect it.
- Ranked ordering: score after multiplier, then `helped - failed` descending, then `id` ascending.
- Empty query (catalog): `helped - failed` descending, then `id` ascending.
- Limits: `limit` 1–100, `offset` >= 0, query at most 512 UTF-8 bytes with no NUL.

## Publishing rules

Enforced in `src/vault/revisions.rs` (and `mcp.rs` for tool-level checks):

- `id` and tags: lowercase ASCII letters/digits in hyphen-separated groups. `id` 1–64 bytes; at most 8 tags of 32 bytes each.
- `description` and `verdict_reason`: single line, at most 280 bytes.
- `content` must contain these headings, each on its own `## ` line, in this order (case-insensitive): `## When to use`, `## Procedure`, `## Pitfalls`, `## Verification`. Preamble and extra sections are allowed.
- Secret scanning over `description`, `content`, `evidence`, `verdict_reason` and outcome `note`: token prefixes (AWS/GitHub/OpenAI/Google/Slack/Stripe/GitLab/npm/Hugging Face/PyPI/OAuth shapes), full JWTs, quoted bearer tokens, PEM private-key blocks, credential URLs with passwords, Slack webhook URLs. Local-dev defaults like `root:secret@localhost` are allowed.
- Verdict: exactly `keep global` or `keep project` (compared trimmed/lowercased, stored verbatim); everything else is refused.
- Verdict/scope pairing: `keep project` requires `scope: project` and a project key; `keep global` requires global scope.
- `scope` is fixed per skill id at first publication.
- `expected_version` provides optimistic concurrency; stale versions are rejected.
- Replacing a proven version (more `helped` than `failed`) requires `replaces_proven: true`.
- Publication is immediate; there is no draft or approval stage.

## Outcomes

- Unique key: `(id, version, project, UTC day)`. A second report on the same day replaces the first — last write wins.
- `helped`/`failed` feed the search multiplier and the proven-version check; `not_applicable` only records a note.

## Concurrency

- SQLite WAL journal with a busy timeout, so readers and the occasional writer tolerate each other.
- Mutations run in `IMMEDIATE` transactions.
- Flag-tracked hook state updates are atomic upserts on `devin_hook_state` keyed by session.

## Hooks

`src/hook.rs`. All hook entry points **fail open**: errors print
`skillvolution hook: <err>` to stderr and the process exits 0, so a broken
vault never stalls the agent.

Two tracking designs:

- **Claude Code** — transcript scan. `Stop` reads the session transcript JSONL from the stored byte offset (`hook_state`). Work tools: `Edit`, `Write`, `MultiEdit`, `NotebookEdit` (Bash does not count). A review counts only when a `publish_skill` or `report_skill_outcome` tool result succeeds (`is_error` results don't). The hook blocks when the latest relevant edit is newer than the latest successful review. Each transcript span is judged once; `stop_hook_active=true` prevents repeated blocking.
- **Flag-tracked clients** (Devin, Codex, Gemini, Cursor) — `tool-use` sets `worked` on matching tools and `reviewed` on vault review tools; `stop` emits the reminder if `worked` is set after `reviewed`. Work after a review re-arms the reminder; every judged stop consumes the span. `session-end` deletes the session state. Work tools per client: Devin `write`/`edit`/`apply_patch`/`notebook_edit`, Codex `apply_patch`, Gemini `write_file`/`replace`, Cursor `Write`.

`hook approve` (wired to PermissionRequest for Devin and Codex) emits
`{"decision":"approve","reason":"Skillvolution-managed tool"}` for
`run_subagent`, `read_subagent` and every `mcp__skillvolution__*` tool.

`session-start` prints the catalog: plain text for Claude, a JSON
`additional_context` payload for flag-tracked clients.

## Setup design

- `ClientKind`: `ClaudeCode`, `OpenCode`, `Devin`, `Codex`, `Gemini`, `Cursor`. `--client` also accepts `all` and `both` (Claude Code + OpenCode).
- `Scope`: global (per-user config directories) or `--project` (files inside the project directory).
- Each client module produces a list of `FileChange`s and `removals`; the applier writes files atomically with rollback on failure.
- Backups: overwritten user files get `.skillvolution.bak` rotation (up to three). Fully managed files get none.
- Ownership markers decide what `setup --remove` may delete: it removes only Skillvolution-owned entries and preserves user keys and values (global install merges and preserves user keys like `env` and `enabled`; project install replaces the MCP server entry whole as a security measure; surgical removal on uninstall).
- Global setup detects clients by executable on `PATH` or an existing config dir; non-detected clients are skipped. Prompts default to yes; `SKILLVOLUTION_NO_INPUT` (non-empty) or no TTY configures every detected client.
- Project setup defaults to all clients and derives the project key from the directory name (`setup --project-key` overrides).
- `setup --dry-run` prints unified diffs and writes nothing.
- A vault on a network filesystem triggers an advisory warning (SQLite WAL should live on local storage); setup proceeds.

## Transfer

- `purge ID [--version N]` — permanently deletes a skill or one revision plus its outcomes in an `IMMEDIATE` transaction with `secure_delete` on. Deletes the row(s) immediately, then compacts the file (secure_delete, VACUUM, WAL checkpoint) so purged content doesn't linger on disk. Compaction can't complete while another connection (an AI client's MCP server) holds the vault open; then it prints a warning and the content may linger until a later purge compacts it.
- `export [-o FILE]` — dumps all skills, revisions and outcomes as JSON, format `skillvolution-export` version 1.
- `import FILE` — merges an export: identical revisions are skipped, same-id/version content conflicts are errors, outcomes insert-or-skip on the uniqueness key, and aborts if a revision's or outcome's id differs from the skill it is listed under.
- `backup PATH` — `VACUUM INTO` a path that must not exist; creates parent directories.

## Doctor

`skillvolution doctor` is read-only. It opens the vault read-only (schema
version, `PRAGMA quick_check`, FTS orphan check, stats) and diffs each
detected client's files against what `setup` would write — stale skill
versions, missing configured binaries, Claude MCP registration status, and
legacy installs. `--json` emits the same checks as data; the command exits
non-zero on failures.

## Relocate

`skillvolution relocate PATH` moves the vault:

1. Validates every global client config it would repoint first (a config that can't be read aborts with nothing written).
2. Prints a `NOTE: close running AI client sessions: …` warning.
3. Copies the database via the same `VACUUM INTO` path as `backup`.
4. Verifies the copy (integrity and contents) before touching clients.
5. Writes repointed configs for global clients that referenced the old path (re-registering Claude Code's MCP server); project-level installs are left untouched.
6. Best-effort WAL checkpoint.
7. Renames the old database to `<name>.relocated.bak` and any `-wal`/`-shm` sidecars to `.relocated.bak-wal`/`.relocated.bak-shm` (nothing is deleted).
8. Prints a `SKILLVOLUTION_DB=<new>` hint when the CLI's default vault was relocated. Path matching handles both the displayable path and escaped variants (Windows canonical paths with doubled backslashes).

## Project key

`src/project.rs`: at runtime the key is the enclosing git repository's
directory name — ASCII alphanumerics lowercased, runs of anything else
collapsed to one hyphen, trimmed, capped at 64 bytes. Submodules key on
their own folder name; worktrees resolve to the main repository name.
`setup --project DIR` instead defaults the key from `DIR` itself, which can
differ when `DIR` is a subdirectory of a larger repo.
