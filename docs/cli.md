# CLI reference

```
skillvolution [--db PATH] <command>
```

`--db PATH` is a global flag accepted by every command: it sets the vault database
file, ahead of `SKILLVOLUTION_DB` and the default platform location (see
[configuration.md](configuration.md)). `-h`/`--help` prints a command's help;
`-V`/`--version` prints the binary's version. The database is created and migrated on
first open — there is no init step.

Exit codes: commands exit `0` on success and `1` on any error (printed as
`Error: …` on stderr). `doctor` additionally exits `1` when a check fails. `hook`
subcommands always exit `0`, even on error — see [hook](#hook).

## serve

```
skillvolution serve [--project KEY]
```

Starts the MCP server on stdio (newline-delimited JSON-RPC 2.0) and serves it until
stdin closes. This is the process every client's MCP entry launches; you don't run it
yourself except to debug.

`--project KEY` sets the project scope for project-scoped skills and outcome records.
When omitted, the project is auto-detected: the sanitized name of the enclosing git
repository root of the working directory, falling back to `CLAUDE_PROJECT_DIR` when the
working directory is not inside a repo (Claude Code can start a user-scope server with
its own config dir as cwd). A worktree resolves to the main repository's name; a
submodule resolves to its own directory name. Outside a git repository only global
skills are visible and `scope: "project"` publishes are refused.

On startup the resolved key is printed on stderr:

```
skillvolution: project = myrepo      # or "none" outside a git repo
```

See [MCP tools](#mcp-tools) for the protocol and tool surface.

## show

```
skillvolution show ID [--version N] [--json]
```

Prints one revision of a skill: a header line (`id vN (status, base vM, scope)`),
`reviewed:`/`note:` lines when present, the evidence, and a unified diff against the
revision's `expected_version` base. Without `--version` it shows the skill's latest
revision in any status (not just published); a base that has been purged diffs against
empty with a `(purged)` label.

```bash
skillvolution show rust-sqlite-busy-timeout            # latest revision
skillvolution show rust-sqlite-busy-timeout --version 1
skillvolution show rust-sqlite-busy-timeout --json     # full revision as JSON
```

Errors: `unknown skill: ID`, `no revision ID version N` (exit 1).

## deprecate / undeprecate

```
skillvolution deprecate ID
skillvolution undeprecate ID
```

`deprecate` hides a skill from `search_skills` without deleting its history; the body is
still returned by `get_skill` with `deprecated: true`. `undeprecate` makes it searchable
again. Both print nothing on success and fail with `unknown skill: ID` otherwise.

## outcomes

```
skillvolution outcomes [ID] [--failing] [--json]
```

Without `ID`, prints one summary line per skill, at its current published version,
ordered by failures then successes:

```
rust-sqlite-busy-timeout v2: helped 3, failed 0, not applicable 1
```

With `ID`, prints that skill's outcome log, newest first:

```
2026-09-26T14:03:11Z rust-sqlite-busy-timeout v2 helped: replaced a sleep loop
```

`--failing` (conflicts with `ID`) restricts the summary to skills whose current version
has at least one `failed` report. `--json` prints the raw summary objects or records.

The vault keeps at most one report per skill, version, project, and UTC day — a repeat
report that day replaces the earlier one — so counts can't be inflated by a chatty
session.

Errors: `unknown skill: ID` (exit 1).

## hook

Entry points the clients' lifecycle hooks call. Every `hook` subcommand reads the
client's hook payload on stdin (except `session-start`, which needs none), writes its
response on stdout, and **always exits 0** — a hook error is reported on stderr as
`skillvolution hook: …` and must never block or break the client session. These are
wired by `setup`; the exact matcher and event each client uses is in
[integrations.md](integrations.md).

### hook session-start

```
skillvolution hook session-start [--client CLIENT] [--project KEY]
```

Prints the skill catalog as session context: a "follow the `evolution` skill" header,
then up to 30 published skills visible to the project with id, version, description,
and helped/failed counts (or `No published skills are visible to this project yet.`).

`--client` selects the output envelope (default `claude-code`): `claude-code` prints the
raw text; `devin`, `codex`, and `gemini` print
`{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":…}}`;
`cursor` prints `{"additional_context":…}`. `--project` resolves like `serve`'s.

### hook tool-use

```
skillvolution hook tool-use [--client CLIENT]
```

PostToolUse-style entry point for the flag-tracked clients (every client but Claude
Code). Flags the session as having done work when `tool_name` is one of the client's
file-editing tools — `write`, `edit`, `apply_patch`, `notebook_edit` (Devin);
`apply_patch` (Codex); `write_file`, `replace` (Gemini CLI); `Write` (Cursor) — or as
having reviewed when the tool name ends with `publish_skill`/`report_skill_outcome`.
Any other tool is ignored. Work after a review re-arms the reminder. Prints nothing.
`--client` defaults to `devin`.

### hook stop

```
skillvolution hook stop [--client CLIENT]
```

Asks for an evolution review when the session has unreviewed work, at most once per
span. For `--client claude-code` (the default) the payload's `transcript_path` is
scanned from the byte offset recorded for that session: if the last `Edit`/`Write`/
`MultiEdit`/`NotebookEdit` call postdates the last *successful* review call, it prints
`{"hookSpecificOutput":{"hookEventName":"Stop","additionalContext":…}}`, which makes
Claude Code continue the turn (a `stop_hook_active` stop is never blocked; the reviewed
offset always advances to the last complete line). For the flag-tracked clients it
consumes the session's worked/reviewed flags and prints the client's continue format:
`{"decision":"block","reason":…}` (`devin`, `codex`), `{"decision":"deny","reason":…}`
(`gemini`), `{"followup_message":…}` (`cursor`). Prints nothing when there is nothing
unreviewed.

### hook session-end

```
skillvolution hook session-end
```

Drops the session's per-session flag row so the table doesn't grow with dead sessions.
Used by the flag-tracked clients' session-end events (`SessionEnd`/`sessionEnd`).
Prints nothing.

### hook approve

```
skillvolution hook approve
```

PermissionRequest-style entry point. Prints
`{"decision":"approve","reason":"Skillvolution-managed tool"}` when the payload's
`tool_name` is `run_subagent`, `read_subagent`, or starts with `mcp__skillvolution__`;
prints nothing otherwise, so the client's normal permission prompt still runs. Note the
grant covers `publish_skill`: anything an evaluator subagent keeps is published without
a further prompt.

## setup

```
skillvolution setup [--project DIR] [--client LIST] [--bin PATH] [--db PATH]
                    [--project-key KEY] [--remove] [--dry-run]
```

Configures AI clients to use the vault, or removes that configuration. See
[README](../README.md#setup) for the walkthrough and
[configuration.md](configuration.md) for every file written.

- Without `--project`: global, per-user setup. Without `--client` it detects installed
  clients and asks which to configure when a terminal is attached; every detected client
  is configured when there's no terminal or `SKILLVOLUTION_NO_INPUT` is set. When
  neither `--db` nor `SKILLVOLUTION_DB` picks a vault path, a terminal is also asked for
  it.
- `--client LIST`: comma-separated `claude-code`, `opencode`, `devin`, `codex`,
  `gemini`, `cursor`, `all`, or `both` (claude-code + opencode). Skips the prompt.
- `--project DIR`: write one project's own files instead; `--client` defaults to `all`
  here. `--project-key KEY` (requires `--project`) overrides the scope key, which
  defaults to `DIR`'s sanitized directory name.
- `--bin PATH`: binary path recorded in the written configs; defaults to the currently
  running executable. The path is made absolute without resolving symlinks, so a
  package-manager symlink stays stable across upgrades.
- `--db PATH`: database path recorded in the written configs.
- `--remove`: undo the configuration, keeping every entry setup doesn't own (see
  [README](../README.md#uninstall) for what is kept). With `--client`, only those
  clients are touched; without it, every client that has something to remove.
- `--dry-run`: print a unified diff of every file setup would write (or, with
  `--remove`, remove) plus the `claude mcp …` command a global Claude Code install
  would run — without writing anything or creating the database. A global dry run with
  no `--client` uses detection alone and never prompts.

If `claude` isn't on `PATH` for a global Claude Code setup, the file changes still
happen and setup prints the `claude mcp add-json` command to run manually (and the
`claude mcp remove` equivalent for `--remove`).

## purge

```
skillvolution purge ID [--version N]
```

Permanently deletes a skill — or, with `--version N`, just that revision — together
with its outcomes and search-index row. Deletes the row(s) immediately, then compacts
the file (secure_delete, VACUUM, WAL checkpoint) so purged content doesn't linger on
disk. Compaction can't complete while another connection (an AI client's MCP server)
holds the vault open; then it prints a warning and the content may linger until a later
purge compacts it. Prints `Purged ID: N revision(s), M outcome(s)[, skill removed].`

Prefer `deprecate` for content you merely don't want surfaced; `purge` is for data that
must be gone (a leaked secret that predates the scanner, etc.).

Errors: `unknown skill: ID`, `unknown version: ID vN` (exit 1).

## export / import

```
skillvolution export [-o|--output FILE]
skillvolution import FILE
```

`export` writes every skill, revision, and outcome as a versioned JSON document
(`format: "skillvolution-export"`, `version: 1`) to `FILE` or stdout. Per-session hook
state is not exported. `import` merges such a document into the vault in one
transaction: new revisions are validated like fresh publishes (headings, secrets),
identical existing revisions and duplicate outcomes are skipped, and any conflicting
revision — a skill that exists under a different scope — or a revision/outcome whose
id differs from the skill it's listed under — aborts the whole import with no partial
writes. `import` prints a JSON report of imported/skipped counts.

```bash
skillvolution export -o vault.json
skillvolution --db other.db import vault.json
```

## backup

```
skillvolution backup PATH
```

Writes a consistent copy of the live database to `PATH` via `VACUUM INTO` (WAL content
included, no exclusive lock needed). `PATH` must not exist. Prints `Backed up to PATH.`

## doctor

```
skillvolution doctor [--json]
```

Read-only health report on the vault and on what `setup` wrote. Never writes — the
database is opened read-only and client files are only compared against what `setup`
would write. Exits `0` when nothing fails (warnings are fine), `1` when any check
fails.

```
[warn] vault: database: will be created on first use
[ok] vault: schema: up to date (v3)
[ok] vault: integrity: PRAGMA quick_check: ok
[ok] vault: search index: 12 skill(s) indexed, no orphans
[ok] vault: stats: 12 skill(s), 15 published revision(s), 34 outcome(s), 1 deprecated
[ok] Claude Code: configured, up to date
[warn] OpenCode: differs from what `skillvolution setup --client opencode` would write: …
[warn] OpenCode skill: rerun setup to update the evolution skill (installed v7, current v8)
[fail] Devin CLI binary: configured binary /old/path/skillvolution does not exist; rerun setup
[warn] Claude Code MCP registration: `claude mcp get skillvolution` reports no registration; rerun setup
[warn] legacy install: project-level install found (…) alongside the global one; hooks will run twice
```

Checks, in order: database presence, a warning if the vault was relocated (old copy
at `PATH.relocated.bak` exists but the current path is missing), `application_id`/
`user_version` (a foreign or newer-schema database fails; an older one warns it will
migrate on next use), `PRAGMA quick_check`, search-index completeness (deprecated skills
with a published revision still count toward the index), row counts; then per client:
installed-but-not-configured vs. configured and up to date vs. differing, staleness of
the installed evolution skill version, a configured binary path that no longer exists
(fail), Claude Code's user-scope MCP registration, and a project-level install left in
the current repo next to the global one. A client whose config can't be read or parsed
gets its own `[fail]` check and the report continues. `--json` prints
`{"healthy": bool, "checks": [{"name", "status", "detail"}]}`.

## relocate

```
skillvolution relocate PATH
```

Moves the vault database to `PATH` (which must not exist; parent directories are
created): validates every global client config it would repoint first (a config that
can't be read or parsed aborts with nothing written), prints a note to close running
AI client sessions, copies the database with `VACUUM INTO` and verifies the copy against
the source, writes repointed configs for every *global* client that referenced the old
path (re-registering Claude Code's MCP server via the `claude` CLI), performs a
best-effort WAL checkpoint, and renames the old file to `<path>.relocated.bak` along
with any `-wal`/`-shm` sidecars (renamed to `.relocated.bak-wal`/`.relocated.bak-shm`
so they're found if the backup is opened later; nothing is ever deleted). Project-level
installs are untouched — rerun `setup --project DIR` for them. When the CLI's default
vault was relocated, a hint is printed so you can update `SKILLVOLUTION_DB` in your
shell profile.

```bash
NOTE: close running AI client sessions: their MCP servers and hooks keep the old
vault open, and anything they write to it from now on is not carried over.
skillvolution relocate ~/vaults/skills.db
# Relocated the vault to /home/you/vaults/skills.db.
# Repointed OpenCode to /home/you/vaults/skills.db.
# The old vault was kept as a backup at /home/you/.local/share/skillvolution/skills.db.relocated.bak.
# Note: CLI commands without --db still resolve to the old path. Set SKILLVOLUTION_DB=/home/you/vaults/skills.db in your shell profile (or pass --db) so they use the new vault; AI clients are already repointed.
```

## MCP tools

`serve` speaks newline-delimited JSON-RPC 2.0: one request per line, one response per
line on stdout. `initialize` negotiates the protocol version (`2025-06-18`,
`2025-03-26`, `2024-11-05` supported; an unrecognized version falls back to the newest)
and returns `capabilities.tools` plus `instructions` for the agent. `ping` returns `{}`,
`tools/list` returns the four tools below, `tools/call` dispatches by `name`.

Protocol errors are JSON-RPC errors on the same `id`:

| Code | When |
|---|---|
| `-32700` | the line isn't valid UTF-8 JSON |
| `-32600` | a request line over 4 MiB, a batch (array) request, a missing/non-`"2.0"` `jsonrpc`, an object/array `id`, or a missing `method` |
| `-32601` | unknown `method` |
| `-32602` | `tools/call` with a missing `name` or an unknown tool name |

Notifications (no `id`) and response-shaped messages are ignored. A tool *argument*
problem or a tool-side failure is not a protocol error: it comes back as a normal
`tools/call` result with `"isError": true` and the message in `content[0].text`, so the
model can read and react to it.

### search_skills

| Param | Type | Constraint |
|---|---|---|
| `query` | string | ≤ 512 bytes, no NUL; empty lists the catalog |
| `limit` | integer | 1–100, default 20 |
| `offset` | integer | ≥ 0, default 0 |

Returns `skills` (metadata only: `id`, `version`, `description`, `tags`, `scope`,
`helped`, `failed`), `total`, `has_more`. Full-text matching over id, description, tags,
and content, ranked by relevance scaled by the current version's outcome counts — a
skill with more failed than helped reports is demoted, a proven one promoted. Bodies are
never returned; use `get_skill`.

### get_skill

| Param | Type | Constraint |
|---|---|---|
| `id` | string | required; `^[a-z0-9]+(-[a-z0-9]+)*$`, ≤ 64 bytes |
| `version` | integer | ≥ 1; defaults to the latest published version |

Returns `id`, `version`, `scope`, `deprecated`, `description`, `tags`, `content` for a
published revision visible to the server's project (global skills plus this project's
own). A deprecated skill still returns its body with `deprecated: true`. Fails with
`no published skill ID is visible from this project`.

### report_skill_outcome

| Param | Type | Constraint |
|---|---|---|
| `id` | string | required; id format above |
| `version` | integer | required, ≥ 1; the version you loaded |
| `result` | string | required; `helped`, `failed`, or `not_applicable` |
| `note` | string | required; ≤ 2,048 bytes, no NUL; scanned for credential-shaped values |

Returns `{"recorded": true}`. Only recordable against a published revision visible to
this project (`no published revision ID version N visible to this project`). One report
per revision, project, and UTC day; a repeat that day replaces it.

### publish_skill

| Param | Type | Constraint |
|---|---|---|
| `id` | string | required; id format above — the skill's stable identity |
| `description` | string | required; single line, ≤ 280 bytes, starts with "Use when" |
| `tags` | string[] | ≤ 8 tags, each ≤ 32 bytes in the id format |
| `content` | string | required; ≤ 65,536 bytes; must contain `## When to use`, `## Procedure`, `## Pitfalls`, `## Verification` as `## ` headings in that order (case-insensitive; preamble and extra `##` sections allowed) |
| `evidence` | string | required; ≤ 16,384 bytes |
| `expected_version` | integer | required, ≥ 0; the current published version, `0` for a new id |
| `scope` | string | `global` (default) or `project`; fixed on the id's first publish |
| `verdict` | string | required; exactly `keep global` or `keep project`, compared trimmed and lowercased, and must match `scope` — `discard` is refused outright |
| `verdict_reason` | string | required; single line, ≤ 280 bytes |
| `replaces_proven` | boolean | required to be `true` when the base version has more `helped` than `failed` reports |

Publishes immediately and returns the new revision summary plus a `next` hint. On
success the revision's `review_note` stores `"<verdict>: <verdict_reason>"`. Notable
rejections (all returned as `isError` tool results):

- id/tag/description/format limits, and `content` missing a required heading;
- credential-shaped values in `description`, `content`, `evidence`, or
  `verdict_reason`;
- `verdict`/`scope` mismatch (`keep project` requires `scope: "project"`);
- `scope: "project"` on a server with no project key;
- republishing an id under a different scope (`skill ID already exists with scope …;
  choose a different id`);
- a stale base (`stale base: expected N, current published version is M`) — `get_skill`
  the new version, merge, and re-evaluate;
- replacing a proven version without `replaces_proven: true` — merge the base's content
  rather than rewriting it, or publish under a new id.
