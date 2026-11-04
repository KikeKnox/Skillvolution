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

## publish

```
skillvolution publish PATH [--id ID] [--scope global|project] [--project KEY]
                           [--evidence TEXT] [--replace]
```

Publishes a `SKILL.md` file (or `PATH/SKILL.md` when PATH is a directory) yourself: you
are the evaluator, so no verdict is needed. A file named `SKILL.md` without a `name` takes
its directory's name as the id.

If PATH is a directory with no `SKILL.md` of its own, every `PATH/*/SKILL.md` (one level
deep, sorted) is published independently, e.g. `skillvolution publish ~/.claude/skills`
to import existing Claude Code skills. `--id` is an error there; `--scope`, `--project`,
`--evidence`, and `--replace` apply to every file. It prints one line per file
(`[published] ID vN (scope)`, `[skipped] ID: already at vN; pass --replace` for an existing
id, or `[rejected] PATH:` with its problems), the SKILL.md template once if any were
rejected (an error raised while publishing a file, such as a credential-shaped value, an over-size field, a scope conflict, or a non-UTF-8 file, is a per-file rejection too, not an abort), and a summary `N published, N skipped, N rejected`; the exit code is 1 if any
were rejected. A directory with no SKILL.md at all is an error. The optional frontmatter (`---` lines, simple
`key: value` pairs) supplies `name` (the id; `--id` overrides it), `description`, and
`tags` (`[a, b]` or comma-separated), and `scope` (`global` or `project`; `--scope` wins,
default global; `get` writes `scope: project` for project skills); a YAML-style trailing
comment (`# ...` after whitespace, outside a leading quoted value) is ignored; multi-line
YAML values are rejected; other keys are ignored. The rest of the file is
the content, minus any `<!-- skillvolution-managed:... -->` line.

Before touching the vault the file is checked and every problem is reported at once with
a template: missing id, invalid id, missing or multi-line description (at most 280
bytes), invalid tags (lowercase-hyphenated, at most 8, 32 bytes each), and the four `## `
sections (`When to use`, `Procedure`, `Pitfalls`, `Verification`) missing or out of order.
The credential scan and scope-mismatch checks of `publish_skill` still apply.

An existing id is refused unless `--replace` is given (`skill ID already exists at vN;
pass --replace to publish vN+1`); `--replace` also acknowledges replacing a proven
version (the replaced version keeps its record at half weight in ranking). `--scope project` uses `--project KEY`, else the project detected like `serve`;
the scope is fixed on first publish. Prints `Published ID vN (scope).`

## search / list / get

```
skillvolution search QUERY [--limit N] [--project KEY] [--json]
skillvolution list [--project KEY] [--json]
skillvolution get ID [--version N] [--project KEY]
```

`search` ranks like the MCP `search_skills` tool (default limit 20) and prints one line
per skill, `ID vN [scope] (helped H, failed F): description`, plus `N of TOTAL` when more
match. `list` is `search` with an empty query and limit 100. `get` prints a skill as a
`SKILL.md` (frontmatter `name`, `description`, `tags`, then the content) that
`publish --replace` accepts, so you can fetch, edit, and republish it. None of them
record outcomes. The project is resolved like `serve`.

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
skillvolution outcomes [ID] [--failing] [--unreported] [--json]
```

Without `ID`, prints one summary line per skill, at its current published version,
ordered by failures then successes:

```
rust-sqlite-busy-timeout v2: helped 3, failed 0, not applicable 1, loaded 5, unreported 1
```

`loaded` counts the (project, UTC day) pairs in which an MCP `get_skill` returned that
version; `unreported` counts those pairs with no outcome report on the version from the
same project and day (a report sent after midnight UTC lands on the next day, so it can
leave its load counted as unreported). Loads are local telemetry: they are not exported
or imported, and `purge` deletes them.

With `ID`, prints that skill's outcome log, newest first:

```
2026-09-26T14:03:11Z rust-sqlite-busy-timeout v2 helped: replaced a sleep loop
```

`--failing` (conflicts with `ID`) restricts the summary to skills whose current version
has at least one `failed` report. `--unreported` (conflicts with `ID`) restricts it to
skills whose current version has at least one unreported load; combined with
`--failing`, both must hold. `--json` prints the raw summary objects (with `loaded` and
`unreported`) or records.

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

Prints the skill catalog as session context: a header telling the agent to check it
before non-trivial work (matching skills are also suggested with each request) and say
in one line which skill it applies (or that none matched), and to follow the
`skill-acquisition` skill when it lacks the know-how, then up to 30 published skills visible to the project with id, version, description,
and helped/failed counts (or `No published skills are visible to this project yet.`).

`--client` selects the output envelope (default `claude-code`): `claude-code`, `devin`,
`codex`, and `gemini` print
`{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":…}}`;
`cursor` prints `{"additional_context":…}`. `--project` resolves like `serve`'s.

### hook prompt

```
skillvolution hook prompt [--client CLIENT] [--project KEY]
```

Prompt-time retrieval. Reads the client's prompt-submit payload on stdin and uses its
`prompt` string (a payload without one prints nothing; invalid JSON is
reported on stderr). It extracts up to 16 keywords (English and Spanish stopwords, tokens under 3
characters (bar 2-letter tech terms such as `ci`, `go`, `js`, `db`, matched as whole words) or over 30 bytes, and repeats dropped; accents folded), and does nothing
when fewer than 2 remain. Spanish developer words are translated to English through a
built-in dictionary (`contenedor` -> container, `reinician` -> restart). Otherwise it
searches the vault (project-scoped like `serve`), takes the top 5 hits, drops those
scoring under half the top hit's score, keeps those where at least 2 distinct
non-generic keywords match the id, tags, or description in 2 different places (a whole
id, a whole tag such as `docker-compose`, and each description token count as one
place each; generic words like fix, test, build, error, file, script never count),
drops any whose current version has 2 or more net failures (failed minus helped) or 2
or more net `not_applicable` reports, and keeps at most 3. Prints, when at least one
remains,
`{"hookSpecificOutput":{"hookEventName":EVENT,"additionalContext":…}}` where the context
is `Skillvolution: vault skills matching this request:`, one
`- ID vN: DESCRIPTION [helped H, failed F]` line per skill, and an instruction to call
`get_skill` for the best match, tell the user which skill (id and version) is applied,
and call `report_skill_outcome` afterwards. Prints nothing when no skill is relevant.

`--client` (default `claude-code`) picks `EVENT`: `UserPromptSubmit` for `claude-code`,
`codex`, and `devin`; `BeforeAgent` for `gemini`. `cursor` is rejected (`this client's
prompt hook cannot inject context` on stderr, exit 0), since Cursor's
`beforeSubmitPrompt` cannot add context. `--project` resolves like `serve`'s. The
OpenCode plugin calls this subcommand itself with `{"prompt": …}` on stdin. Exit code is
always `0`.

### hook tool-use

```
skillvolution hook tool-use [--client CLIENT]
```

PostToolUse-style entry point for the flag-tracked clients (every client but Claude
Code). Flags the session as having done work when `tool_name` is one of the client's
file-editing tools — `write`, `edit`, `apply_patch`, `notebook_edit` (Devin);
`apply_patch` (Codex); `write_file`, `replace` (Gemini CLI); `Write` (Cursor) — or as
having done work when the tool name ends with `get_skill` (loading a skill owes an
outcome report), or as having reviewed when it ends with
`publish_skill`/`report_skill_outcome`. Any other tool is ignored. Work after a review re-arms the reminder. Prints nothing.
`--client` defaults to `devin`.

### hook stop

```
skillvolution hook stop [--client CLIENT]
```

Asks for an evolution review when the session has unreviewed work, at most once per
span. For `--client claude-code` (the default) the payload's `transcript_path` is
scanned from the byte offset recorded for that session: if the last `Edit`/`Write`/
`MultiEdit`/`NotebookEdit` call or successful `get_skill` call postdates the last
*successful* review call (`publish_skill`/`report_skill_outcome`), it prints
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
  no `--client` uses detection alone and never prompts. The `claude mcp add-json` line
  appears only when `claude mcp get skillvolution` reports no registration (`Would run:
  …`) or one that differs from setup's — another scope, command or args (`… (replacing
  the existing skillvolution registration)`); an identical one is left alone by setup
  too, so a fully set-up client previews as `No changes.`.

If `claude` isn't on `PATH` for a global Claude Code setup, the file changes still
happen and setup prints the `claude mcp add-json` command to run manually (and the
`claude mcp remove` equivalent for `--remove`); `--dry-run` shows that command as
`Would print (claude CLI not found on PATH): …`.

Every client gets three managed skills, `evolution`, `skill-evaluation`, and
`skill-acquisition`, at `skills/<name>/SKILL.md`, each with its own
`skillvolution-managed:<name>:vN` marker. A user's own file at any of those paths makes
setup refuse (`unowned … skill conflict`); `--remove` skips it with a note and deletes
only the marked ones, plus any skill directory left empty.

## purge

```
skillvolution purge ID [--version N]
```

Permanently deletes a skill — or, with `--version N`, just that revision — together
with its outcomes, loads, and search-index row. Deletes the row(s) immediately, then compacts
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
state and `get_skill` loads (local telemetry) are not exported. `import` merges such a document into the vault in one
transaction: new revisions are validated like fresh publishes (headings, secrets),
identical existing revisions and duplicate outcomes are skipped, and any conflicting
revision — a skill that exists under a different scope — or a revision/outcome whose
id differs from the skill it's listed under — aborts the whole import with no partial
writes. A `draft` or `rejected` revision from an export written before schema v5 is
imported as `superseded`, its note prefixed `legacy <status>`. `import` prints a JSON
report of imported/skipped counts.

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
skillvolution doctor [--json] [--fix]
```

Health report on the vault and on what `setup` wrote. Without `--fix` it never writes —
the database is opened read-only and client files are only compared against what
`setup` would write. Exits `0` when nothing fails (warnings are fine), `1` when any
check fails.

```
[warn] vault: database: will be created on first use
[ok] vault: schema: up to date (v5)
[ok] vault: integrity: PRAGMA quick_check: ok
[ok] vault: search index: 12 skill(s) indexed, no orphans
[ok] vault: stats: 12 skill(s), 15 published revision(s), 34 outcome(s), 1 deprecated
[ok] Claude Code: configured, up to date
[warn] OpenCode: differs from what `skillvolution setup --client opencode` would write: …
[warn] OpenCode skill: rerun setup to update the evolution skill (installed v7, current v11)
[fail] Devin CLI binary: configured binary /old/path/skillvolution does not exist; rerun setup
[warn] Claude Code MCP registration: `claude mcp get skillvolution` reports no registration; rerun setup
[warn] legacy install: project-level install found (…) alongside the global one; hooks will run twice
```

Checks, in order: database presence, a warning if the vault was relocated (old copy
at `PATH.relocated.bak` exists but the current path is missing), `application_id`/
`user_version` (a foreign or newer-schema database fails; an older one warns it will
migrate on next use), `PRAGMA quick_check`, search-index completeness (deprecated skills
with a published revision still count toward the index), row counts; then per client:
installed-but-not-configured vs. configured and up to date vs. differing (a client counts
as configured only when one of its files carries something setup owns: an owned hook
command, a `skillvolution` MCP server entry, the `AGENTS.md`/`GEMINI.md` marker block, a
managed skill file, or the OpenCode plugin — a file that merely mentions
`skillvolution`, e.g. in a permission entry, doesn't count), staleness of
the installed version of each managed skill (`evolution`, `skill-evaluation`,
`skill-acquisition`), a configured binary path that no longer exists (fail), Claude Code's user-scope MCP registration, and a project-level install left in
the current repo next to the global one. A client whose config can't be read or parsed
gets its own `[fail]` check and the report continues. `--json` prints
`{"healthy": bool, "checks": [{"name", "status", "detail"}]}`.

`--fix` runs the checks, repairs what it safely can, printing one line per repair
(`[fixed] <check>: <what was done>` or `[fix failed] <check>: <error>`; a failed repair
doesn't stop the others; on stderr with `--json`), then runs every check again and prints
the final report; the exit code reflects that final report. Repairs:

- an older vault schema is migrated (as any command's first open would do);
- a search index with missing or orphaned rows is rebuilt from each skill's latest
  published revision, in one transaction;
- a client that is configured but differs from what `setup` would write (including an
  outdated managed skill or a configured binary that no longer exists) gets global setup
  re-applied for exactly that client — same validation, numbered backups and symlink
  refusal as `setup --client <name>`, keeping the configured binary when it still exists
  and using the running one otherwise; Claude Code is re-registered too;
- a missing Claude Code MCP registration (for a configured Claude Code) is re-registered.

Never repaired: a client that is only installed (not configured) or not installed —
configuring one is the user's call; a newer-schema or non-Skillvolution database; and a
failing `PRAGMA quick_check`, for which `--fix` prints `[not fixable] vault: integrity: …`
guidance (restore a `skillvolution backup`, or `export` and `import` into a fresh
vault) and makes no other vault repair.

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

Returns `skills` (metadata only: `id`, `version`, `description`, `tags`, `scope`, and
the current version's `helped`, `failed`, `not_applicable`), `total`, `has_more`.
Full-text matching over id, description, tags, and content, ranked by relevance scaled by
the track record `(helped - failed) + 0.5 * (prior_helped - prior_failed)`, where the
prior counts are the immediately preceding published version's — a skill with more
failed than helped reports is demoted, a proven one promoted, and a republished skill
keeps half of its previous version's record. `not_applicable` does not affect ranking. Bodies are
never returned; use `get_skill`.

### get_skill

| Param | Type | Constraint |
|---|---|---|
| `id` | string | required; `^[a-z0-9]+(-[a-z0-9]+)*$`, ≤ 64 bytes |
| `version` | integer | ≥ 1; defaults to the latest published version |

Returns `id`, `version`, `scope`, `deprecated`, `description`, `tags`, `content`, and
`next` for a published revision visible to the server's project (global skills plus this
project's own). `next` is a reminder to apply the procedure only as far as it fits,
verify it with its Verification section, and call `report_skill_outcome` with this `id`
and `version` (`helped`, `failed` naming the wrong step, or `not_applicable`). A
successful `get_skill` also counts as work for the stop reminder, so an unreported load
triggers it, and is recorded (once per version, project, and UTC day) for `skillvolution
outcomes --unreported`; a failure to record it never fails the call. A deprecated skill still returns its body with `deprecated: true`. Fails with
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
