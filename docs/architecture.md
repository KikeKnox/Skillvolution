# Skillvolution: architecture

## Scope

A local procedural-memory vault, shared by OpenCode and Claude Code through MCP. English is the
project's language. Rust implements storage, protocol handling, and client configuration. Linux is
the deployment target. Hermes integration is excluded.

Engineering goals: KISS, cohesive modules, minimal comments, no dead code, and extensibility
through module boundaries rather than speculative abstractions. An agent vault, vector search, web
UI, HTTP service, schedulers, and plugin framework are not part of this version.

## Runtime

Each client starts the same binary as a stdio MCP subprocess. Processes share a SQLite database
using explicitly enabled WAL mode and a busy timeout. Standard output carries MCP messages only.
Logs go to standard error. Setup is a separate CLI command, not a side effect of starting the MCP
server. Delivery is MCP-only: there is no HTTP or filesystem-watch path, so every client reads and
writes the same vault through the same protocol surface.

The MCP tool surface is fixed at four tools:

- `search_skills`: query published skill metadata (id, version, description, tags, helped/failed
  counts) with a bounded limit and pagination; never return full bodies, so a search is cheap even
  against a large catalog. Empty query lists the catalog, ordered by `helped - failed` then id.
- `get_skill`: retrieve one published skill body, optionally at an exact published revision;
  defaults to the latest published version.
- `report_skill_outcome`: record `helped` / `failed` / `not_applicable` plus a note against a
  published revision the caller applied.
- `propose_skill_change`: create an immutable draft revision with its evidence, tags, optional
  scope, and expected base revision. It cannot publish.

Both `initialize` and `tools/call` negotiate protocol version `2025-06-18`, `2025-03-26`, or
`2024-11-05` against what the client requests, falling back to the newest supported version for an
unknown request. `clientInfo.name` from `initialize` is recorded as `client` on every revision and
outcome the connection creates, so `skillvolution drafts` and `outcomes` can show provenance.
`tools/list` and `tools/call` are refused with a JSON-RPC error before `initialize` completes.

The local CLI provides draft inspection, diffing, and explicit publication or rejection.
Publication means a human accepted the proposal, not that an automated quality evaluation proved
it. Validation checks content structure, identifiers, sizes, tags, evidence, and revision
conflicts. Stored evidence is supplied by the proposing agent and is not independently verified by
the vault.

Skills use stable lowercase hyphenated identifiers and monotonically increasing integer revisions,
and are either global or scoped to a project key fixed at first proposal. Publication verifies the
proposal's base against the currently published revision; publishing a draft also marks every
other draft sharing that same base as `superseded`, so competing stale proposals fail or are
retired rather than silently overwriting newer work. Previously published revisions remain
retrievable by exact version. Drafts do not appear in the default MCP catalog. A new publication is
visible to an already-running client on its next tool query; existing tool responses are not
retroactively replaced. Outcome counters are keyed to a skill's current published version, so
publishing a new revision starts its `helped`/`failed`/`not_applicable` counts at zero.

Search is backed by an FTS5 virtual table (`id`, `description`, `tags`, `content`) using the
`unicode61 remove_diacritics 2` tokenizer, kept in sync with the currently published revision
inside the `publish` transaction. A free-text query is split on non-alphanumeric characters,
lowercased, and rebuilt as quoted prefix terms joined with `OR`, so arbitrary user input can never
be interpreted as FTS query syntax. Matches are ranked with `bm25`, weighted 4:3:2:1 across
id/description/tags/content, then tied-broken by `helped - failed` and id.

The database schema is versioned with SQLite's `user_version` pragma (currently `1`). Opening a
fresh file initializes it to that version; opening one already at that version is a no-op; opening
one with an unrecognized *later* version refuses to run (forward compatibility is not attempted);
and opening a pre-versioning database (detected by the presence of a legacy `revisions` table with
no `user_version` set) is refused outright, since this rewrite ships no data migration path.

## Modules and ownership contracts

- `src/vault/mod.rs`: connection setup (WAL, busy timeout, foreign keys), schema migration and
  version checks, shared validation helpers, transcript-offset and deprecation state.
- `src/vault/revisions.rs`: `propose` / `publish` / `reject` / `inspect` / `drafts` / `diff` /
  `get`, all backed by immutable, versioned rows in `revisions`.
- `src/vault/search.rs`: FTS5 query construction, ranking, and pagination.
- `src/vault/outcomes.rs`: recording and summarizing `helped`/`failed`/`not_applicable` reports.
- `src/vault/schema.sql`: the `skills`, `revisions`, `outcomes`, `hook_state` tables, the
  `skills_fts` virtual table, and the `current_skills` view joining a skill to its latest published
  revision and outcome counts.
- `src/mcp.rs`: JSON-RPC framing over stdio, protocol negotiation, and tool routing.
- `src/hook.rs`: Claude Code hook logic — the SessionStart catalog text and the Stop
  transcript scan — kept independent of stdio so it is unit-testable against fixture transcripts.
- `src/main.rs`: CLI dispatch (`clap`), default database path resolution.
- `src/setup/mod.rs`: orchestration — path checks, project-key resolution, collecting and then
  atomically writing per-client changes.
- `src/setup/fs_safe.rs`: shared filesystem/JSON primitives — symlink/traversal refusal, strict
  JSON parsing (duplicate keys rejected), backup-before-write, marker-block merge/removal.
- `src/setup/claude.rs`: `.mcp.json` entry, `.claude/settings.local.json` hook merge, native skill
  file, legacy `CLAUDE.md` marker removal.
- `src/setup/opencode.rs`: `opencode.json` entry, `AGENTS.md` marker block, native skill file,
  legacy `instructions` entry removal.
- `assets/evolution/SKILL.md`: the native Evolution skill installed for both clients.
- `launch.sh`: prerequisite checks, build/install, database init, setup command invocation.
- `tests/`: real SQLite (`core_vault`, `core_concurrency`), a real stdio MCP subprocess
  (`core_mcp`), the CLI (`core_cli`), hook logic and its CLI wrapper (`hook`), safe client
  configuration (`setup`), and the launcher (`setup_launch`).

Integration contract: `src/setup/mod.rs` defines `#[derive(clap::Args)] pub struct SetupArgs` and
`pub fn run(args: SetupArgs) -> anyhow::Result<()>`. `main.rs` includes `mod setup` (re-exported
from `lib.rs`) and a `Setup(setup::SetupArgs)` subcommand. Setup accepts `--project PATH`,
`--client both|opencode|claude-code`, `--bin PATH`, `--db PATH`, and `--project-key KEY`. The
installed MCP command is `[absolute_binary, "--db", absolute_database, "serve", "--project", key]`.

## Client integration

Project-scoped configuration is the safe default. Claude Code uses `.mcp.json` with an explicit
`type: stdio`, command, and args, plus two hooks (SessionStart, Stop) merged into
`.claude/settings.local.json`. OpenCode uses its documented local MCP entry in `opencode.json` plus
a short marker block in `AGENTS.md`; OpenCode has no hook mechanism, so its trigger is
instruction-only. Existing values outside Skillvolution's entries are preserved. Malformed,
ambiguous, or unsupported config is rejected before any write. Existing files are backed up on
change; repeated identical setup does not duplicate content or produce a backup. Partial failures
must be reported honestly: every change is validated before any file is written, so a failing
check leaves the project untouched. Config symlinks, ancestor symlinks, and conflicting unowned
Skillvolution files require a safe refusal. Legacy artifacts from the original single-tool design —
the `CLAUDE.md` `@import` marker block and the OpenCode `instructions` array entry — are removed on
setup rather than left to accumulate.

Each client receives the same native `evolution` bootstrap skill. The bootstrap searches and loads
vault skills on demand, reports meaningful outcomes, updates existing skills before creating
duplicates, and proposes evidence-backed lessons that pass an explicit verified/non-obvious/
reusable/costly-to-miss test. No new lesson is a valid result. It separates reusable procedures
from task state, excludes secrets and personal data, respects tool permissions, and never publishes
its own proposals.

For Claude Code, the SessionStart hook prints a compact catalog (up to 30 published skills, scoped
to the project key) so discovery does not require an explicit search on trivial tasks. The Stop
hook inspects the transcript since the last reviewed offset (persisted in `hook_state`, keyed by
session id) for work tool calls (`Edit`, `Write`, `MultiEdit`, `NotebookEdit`, `Bash`) without a
matching `report_skill_outcome` or `propose_skill_change` call; if it finds unreviewed work it exits
2 with a reason on stderr, blocking the stop exactly once per unreviewed span, and always respects
`stop_hook_active` to avoid looping. Both hooks are instruction-driven at the LLM level even when
the Stop hook's trigger is a guaranteed lifecycle event: blocking the stop only asks the model to
review; it cannot force a specific tool call. This limitation is explicit; installing a hook does
not ensure an LLM's response satisfies it. Client configuration acceptance and native discovery
must be distinguished from a live authenticated LLM end-to-end test, which this repository has not
run.

## Launcher

`launch.sh` builds the Rust release binary, installs it at a user-writable location, initializes
the shared database, and configures the requested project/clients. Paths containing spaces must
work. No sudo, hidden dependency downloads, system services, or automatic agent installation. If
Rust is absent, an explicit opt-in (`--install-rust`, Linux only) may bootstrap rustup; otherwise
give actionable prerequisites. Help works without Rust. Invalid options fail before side effects.
Setup is tested only against disposable destinations on the development machine.

## Acceptance

1. Rust builds; formatting, all-target tests, and Clippy pass.
2. SQLite survives restart and concurrent access; draft/publication conflicts are tested.
3. A real subprocess completes MCP initialization and tools/list, proposes a draft, and observes a
   later CLI publication without restart.
4. Search responses omit skill bodies, and exact published revision reads are stable.
5. Both client configurations match their documented schemas; repeated setup preserves unrelated
   settings and produces no duplicate hooks, entries, or instructions.
6. Evolution, English usage documentation, and launch.sh are present and exercised.
7. Live client sessions are reported as unverified unless actually executed.

## Documentation references

- https://opencode.ai/docs/mcp-servers/
- https://opencode.ai/docs/skills/
- https://code.claude.com/docs/en/mcp
- https://code.claude.com/docs/en/skills
- https://code.claude.com/docs/en/hooks
- https://modelcontextprotocol.io/specification/2025-06-18/server/tools
