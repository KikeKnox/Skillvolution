# Architecture

## Modules

- `src/vault/mod.rs` — opens the SQLite connection (WAL, busy timeout, foreign keys), runs schema
  migration, and holds shared validation helpers, transcript-offset storage, and deprecation
  toggling.
- `src/vault/revisions.rs` — `propose`, `publish`, `reject`, `inspect`, `drafts`, `diff`, `get`.
- `src/vault/search.rs` — builds and ranks FTS5 queries.
- `src/vault/outcomes.rs` — records and summarizes `helped`/`failed`/`not_applicable` reports.
- `src/vault/schema.sql` — table and view definitions (below).
- `src/mcp.rs` — JSON-RPC framing over stdio, protocol negotiation, tool dispatch.
- `src/hook.rs` — the SessionStart catalog text and the Stop transcript scan, independent of stdio
  so both are unit-testable against fixture transcripts.
- `src/main.rs` — CLI parsing and dispatch (`clap`); opens the vault, creating the database file on
  first use.
- `src/setup/mod.rs` — resolves paths and the project key, collects each client's changes, then
  writes them.
- `src/setup/fs_safe.rs` — shared filesystem/JSON helpers: load JSON, merge a marker block, refuse
  an unowned skill file, write-with-backup.
- `src/setup/claude.rs` — Claude Code changes: `.mcp.json` entry, `.claude/settings.local.json`
  hooks, native skill file.
- `src/setup/opencode.rs` — OpenCode changes: `opencode.json` entry, `AGENTS.md` marker block,
  native skill file.
- `assets/evolution/SKILL.md` — the native `evolution` skill installed for both clients.

## Data model

- **skills**(`id`, `scope`, `deprecated`) — `scope` is `NULL` (global) or a project key, fixed on
  the skill's first proposal. `deprecated` hides it from search without deleting history.
- **revisions**(`id`, `version`, `description`, `tags`, `content`, `evidence`,
  `expected_version`, `status`, `created_at`, `review_note`) — `status` is `draft`, `published`, or
  `rejected`. `expected_version` names the published version a proposal was based on (`0` for a new
  skill); publishing checks it still matches and rejects any other draft sharing that same base,
  with a `review_note` noting which version superseded it.
- **outcomes**(`id`, `version`, `result`, `note`, `project`, `created_at`) — `result` is `helped`,
  `failed`, or `not_applicable`; only recordable against a published revision visible to `project`.
- **hook_state**(`session_id`, `transcript_offset`) — the byte offset each Claude Code session's
  transcript has been reviewed up to.
- **skills_fts** — an FTS5 virtual table (`id`, `description`, `tags`, `content`, tokenizer
  `unicode61 remove_diacritics 2`) rewritten for a skill each time a draft is published; search
  ranks matches by `bm25` weighted 4:3:2:1 across those columns, tied-broken by `helped - failed`
  then id.
- **current_skills** — a view joining each skill to its latest published revision and outcome
  counts; backs search and the outcome summary.
- The schema is versioned with SQLite's `user_version` pragma. A fresh database is initialized to
  the current version; a database already there is left alone; any other version fails to open.

## MCP tools

- `search_skills(query, limit)` — search published skill metadata; an empty query lists the
  catalog. Bodies are never returned.
- `get_skill(id, version?)` — fetch one published skill's body, defaulting to its latest version.
- `report_skill_outcome(id, version, result, note)` — record whether an applied skill helped.
- `propose_skill_change(id, description, tags?, content, evidence, expected_version, scope?)` —
  store a draft for human review; never publishes.

## Concurrency

Every connection enables WAL mode and a 5-second busy timeout on open. `propose`, `publish`, and
`reject` each run in an `IMMEDIATE` transaction, so two writers never interleave a read-check with
a conflicting write — a stale or competing draft fails cleanly instead of corrupting state.
