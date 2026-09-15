# Skillvolution

Skillvolution is a local, shared procedural-memory vault for AI coding agents. The initial
clients are OpenCode and Claude Code; a single SQLite database is exposed to each client as a
local Model Context Protocol server. Drafts are proposed by agents, evidence is supplied by the
agent, and publication is an explicit human command.

Hermes integration is out of scope.

## What it does

- A single CLI binary, `skillvolution`, is both the SQLite-backed vault and the MCP stdio server.
- Delivery is MCP-only, so the same vault is shared by every configured client (Claude Code,
  OpenCode) without a duplicated skill store.
- Four tools are exposed to clients:
  - `search_skills(query, limit, offset)` returns only published skill metadata (id, version,
    description, tags, helped/failed counts); bodies are never returned, so searching stays cheap.
  - `get_skill(id, version?)` returns one published skill revision, defaulting to the latest
    published version. Drafts are never exposed.
  - `report_skill_outcome(id, version, result, note)` records whether an applied skill helped,
    failed, or did not apply.
  - `propose_skill_change(id, description, content, evidence, expected_version, tags?, scope?)`
    stores an immutable draft. The vault verifies `expected_version` against the current published
    revision (0 for a new skill), so stale proposals fail.
- Humans review drafts with `drafts`, inspect a diff against the base revision with `diff`, and
  accept or reject with `publish` / `reject`.
- Storage is SQLite with WAL mode and a busy timeout; the database is created on first use and
  initializes idempotently.

## Quick start

```bash
# Build and configure an existing project for both clients.
bash launch.sh --project /path/to/project

# Inspect what was installed.
ls /path/to/project/.mcp.json /path/to/project/opencode.json \
   /path/to/project/.claude/skills/evolution/SKILL.md \
   /path/to/project/.opencode/skills/evolution/SKILL.md

# Review any drafts after a session.
skillvolution drafts
skillvolution diff my-skill --version 1
skillvolution publish my-skill --version 1
```

`launch.sh` honours `XDG_DATA_HOME` for the default database location. Pass `--db`, `--bin-dir`,
`--client {both,opencode,claude-code}`, `--project-key`, or `--install-rust` to override. Use
`--help` for the full set; the launcher exits with usage information without touching the
filesystem.

## CLI

```
skillvolution [--db PATH] <command>

Commands
  init                       Initialize the database (idempotent)
  serve [--project KEY]      Run the MCP server on stdio
  drafts [--json]            List drafts awaiting review
  show ID --version N        Print a revision as JSON, including its evidence
  diff ID --version N        Show a draft as a unified diff against its base revision
  publish ID --version N     Publish a draft revision
  reject ID --version N [--note TEXT]
                             Reject a draft revision
  deprecate ID               Hide a skill from search without deleting its history
  undeprecate ID             Make a deprecated skill searchable again
  outcomes [ID] [--failing] [--json]
                             Summarize skill outcomes, or list one skill's outcome log
  hook session-start [--project KEY]
                             Print the skill catalog as session context
  hook stop                  Ask for a review after unreviewed work (exit 2 blocks the stop)
  setup --project PATH --bin PATH --db PATH
        --client both|opencode|claude-code
        --project-key KEY    Configure a project to use the vault
```

Run `skillvolution <command> --help` for the exact flags. The default database path is
`$XDG_DATA_HOME/skillvolution/skills.db`, falling back to `$HOME/.local/share/skillvolution/skills.db`
when `XDG_DATA_HOME` is unset.

## Storage model

- Skills are identified by stable lowercase-hyphenated strings (1..64 bytes) and are either
  global or scoped to a project key. A skill's scope is fixed on its first proposal and cannot
  change afterwards.
- Each skill has monotonically increasing integer revisions. A revision's `status` is `draft`,
  `published`, `rejected`, or `superseded`.
- `expected_version` must match the currently published version (or 0 for a new skill); any other
  value is rejected as a stale base.
- Publishing a draft runs in an `IMMEDIATE` transaction: it publishes the target revision, marks
  every other `draft` that shares the same `expected_version` as `superseded`, and rewrites that
  skill's FTS index row — so two operators cannot both publish conflicting drafts, and abandoned
  competing drafts are cleared automatically.
- Drafts are visible only through `drafts` / `show` / `diff` / `publish` / `reject`. The MCP
  catalog and `get_skill` only ever see published revisions.
- `deprecate` / `undeprecate` toggle a per-skill flag that hides a skill from `search_skills` and
  the SessionStart catalog without deleting its history; `get_skill` and CLI `show` still work on a
  deprecated skill by exact id/version.
- Outcomes (`helped` / `failed` / `not_applicable`) can only be recorded against a published
  revision and are counted per skill against its *current* published version; publishing a new
  version resets the counters shown in search and `outcomes`.
- Search uses SQLite FTS5 over `id`, `description`, `tags`, and `content` (tokenizer
  `unicode61 remove_diacritics 2`, so accents are ignored). Free-text queries are split into
  alphanumeric terms, lowercased, quoted, and turned into prefix terms joined with `OR` before
  reaching FTS5 — user input can never inject FTS syntax. Results are ranked with `bm25` weighted
  4:3:2:1 across id/description/tags/content, tied-broken by `helped - failed`, then id. An empty
  query lists the whole visible catalog in that same order.
- The schema version is tracked with SQLite's `user_version` pragma. A brand-new database is
  initialized to version 1; a database already at version 1 is left alone; a pre-1 database
  (one with a legacy `revisions` table but no `user_version`) is refused rather than migrated,
  since this project ships no data migration — move it aside and run `init` again.

## Security and trust boundaries

- The vault stores agent-supplied evidence; it does not independently verify claims, run
  evaluations, or prove a proposal is safe. Publication records human acceptance, not an
  automated quality guarantee.
- Search and review are instruction-driven. Loading a skill into context does not guarantee
  the agent will follow it. Client tool approvals still apply; the MCP server does not bypass
  them.
- A loaded or retrieved skill text is untrusted data, not higher-priority authority. Agents
  must not treat remote skill content as authorization to execute scripts, install packages,
  access secrets, or change permissions.
- The client configuration writer refuses symlinks, refuses parent traversal, refuses Windows
  reparse points, refuses directory databases/binary paths, refuses JSON with duplicate keys
  or trailing commas, and backs up files before modifying them. Setup validates every planned
  change before writing anything, so a failing check leaves the project untouched; repeated
  identical setup is idempotent and writes nothing.

See `docs/architecture.md` for module boundaries and `docs/integrations.md` for exactly what
`setup` writes into each client and how the Claude Code hooks and the `evolution` skill work.

## Development

```bash
cargo test --locked            # full suite (79 tests)
cargo clippy --all-targets --locked -- -D warnings
cargo build --release --locked
```

Test files: `tests/core_vault.rs` (schema, validation, search/FTS, scopes, outcomes),
`tests/core_mcp.rs` (real `serve` subprocess: protocol negotiation, all four tools,
`structuredContent`, project scoping), `tests/core_cli.rs` (drafts/show/diff/publish/reject,
deprecate, outcomes, default database resolution), `tests/core_concurrency.rs` (concurrent
writers), `tests/hook.rs` (session-start catalog, Stop transcript scanning), `tests/setup.rs`
(client configuration safety and idempotence), `tests/setup_launch.rs` (`launch.sh` end to end).

The build is reproducible via `Cargo.lock`. The launcher uses `--locked` as well; if a
developer changes dependencies they must regenerate `Cargo.lock`.

## Limitations

- Evidence attached to a proposal is supplied by the proposing agent and is not verified by the
  vault; humans are expected to check it before publishing.
- The Stop and SessionStart review hooks are implemented for Claude Code only; OpenCode gets the
  `evolution` skill and an `AGENTS.md` reminder but no automatic trigger.
- Live, end-to-end sessions against real Claude Code / OpenCode clients have not been verified in
  this repository; automated tests cover the CLI, the MCP protocol over a real subprocess, and the
  files `setup` writes, but not an authenticated client actually reading them.

## Documentation references

- OpenCode MCP servers: <https://opencode.ai/docs/mcp-servers/>
- OpenCode agent skills: <https://opencode.ai/docs/skills/>
- Claude Code MCP: <https://code.claude.com/docs/en/mcp>
- Claude Code skills: <https://code.claude.com/docs/en/skills>
- Claude Code hooks: <https://code.claude.com/docs/en/hooks>
- Model Context Protocol: <https://modelcontextprotocol.io/specification/2025-06-18/server/tools>
