# Architecture

## Modules

- `src/vault/mod.rs` — opens the SQLite connection (WAL, busy timeout, foreign keys), runs schema
  migration, and holds shared validation helpers, transcript-offset storage, and deprecation
  toggling. Validation includes `validate_sections` (the four required `content` headings, in
  order) and `validate_no_secrets` (rejects credential-shaped values in `description`, `content`,
  and `evidence` by prefix/JWT/bearer-token shape); there is deliberately no generic `key=value`
  rule, since that flags legitimate documentation about *how* to configure a credential (e.g.
  `api_key=$OPENAI_API_KEY`) as if it were one.
- `src/vault/revisions.rs` — `propose` (publishes immediately), `inspect`, `diff`, `get`.
- `src/vault/search.rs` — builds and ranks FTS5 queries.
- `src/vault/outcomes.rs` — records and summarizes `helped`/`failed`/`not_applicable` reports.
- `src/vault/schema.sql` — table and view definitions (below).
- `src/mcp.rs` — JSON-RPC framing over stdio, protocol negotiation, tool dispatch.
- `src/hook.rs` — the SessionStart catalog text and the Stop transcript scan, independent of stdio
  so both are unit-testable against fixture transcripts.
- `src/main.rs` — CLI parsing and dispatch (`clap`); `resolve_project` picks `--project` when
  given, else the working directory's git repository, falling back to `CLAUDE_PROJECT_DIR`'s, for
  `serve` and `hook session-start`.
- `src/project.rs` — derives a project key from a directory's enclosing git repository root
  (lowercased, non-alphanumeric runs collapsed to `-`, ≤64 bytes); `None` outside a git repo. Used
  for runtime auto-detection and to default `setup --project`'s `--project-key`.
- `src/setup/mod.rs` — resolves the binary/database paths and dispatches to project setup
  (`--project PATH`) or global, per-user setup (no `--project`).
- `src/setup/fs_safe.rs` — shared filesystem/JSON helpers: strict-JSON parsing with duplicate-key
  rejection, marker-block merge, managed-file ownership checks, backup-before-write.
- `src/setup/hooks.rs` — merges SessionStart/Stop hook entries into a settings JSON value, shared
  by project (`.claude/settings.local.json`) and global (`~/.claude/settings.json`) setup.
- `src/setup/plugin.rs` — renders the OpenCode plugin asset with `--bin`/`--db` substituted.
- `src/setup/claude.rs` / `src/setup/opencode.rs` — per-project changes; `src/setup/global.rs`,
  `global_claude.rs`, `global_opencode.rs` — the global equivalents. The global Claude Code side
  registers its MCP server via the `claude` CLI (`src/setup/claude_cli.rs`) instead of editing
  `~/.claude.json` directly, which Claude Code itself rewrites. `src/setup/detect.rs` decides which
  clients global setup configures when `--client` is omitted.
- `install.sh` — one-command install: runs the release installer, then `skillvolution setup`.
- `assets/evolution/SKILL.md` — the native `evolution` skill installed for both clients.
- `assets/opencode/skillvolution.js` — the OpenCode plugin template: catalog injection plus the
  post-idle review reminder.

## Data model

- **skills**(`id`, `scope`, `deprecated`) — `scope` is `NULL` (global) or a project key, fixed on
  the skill's first proposal. `deprecated` hides it from search without deleting history.
- **revisions**(`id`, `version`, `description`, `tags`, `content`, `evidence`,
  `expected_version`, `status`, `created_at`, `reviewed_at`, `review_note`) — new revisions are
  always `published`; `draft`, `rejected`, and `superseded` remain only for rows written before
  proposals published directly. `expected_version` names the published version a proposal was based
  on (`0` for a new skill); proposing checks it still matches and marks any legacy draft sharing
  that same base as `superseded` with a `review_note` noting the published version that superseded
  them. Every normal publish also fills `reviewed_at` (the publish timestamp) and `review_note`,
  set to `"<verdict>: <verdict_reason>"` from the evaluator's verdict and its one-line reason —
  not just the legacy `superseded` path.
- **outcomes**(`id`, `version`, `result`, `note`, `project`, `created_at`) — `result` is `helped`,
  `failed`, or `not_applicable`; only recordable against a published revision visible to `project`.
- **hook_state**(`session_id`, `transcript_offset`) — the byte offset each Claude Code session's
  transcript has been reviewed up to.
- **skills_fts** — an FTS5 virtual table (`id`, `description`, `tags`, `content`, tokenizer
  `unicode61 remove_diacritics 2`) rewritten for a skill each time a revision is published; search
  ranks matches by `bm25` weighted 4:3:2:1 across those columns, scaled by a factor
  `1 + (helped - failed) / (ABS(helped - failed) + 2.0)` derived from the *current published
  version's* outcome counts. That factor stays in the open range `(0, 2)`: exactly `1.0` when
  `helped = failed` (an unreported or evenly-split skill ranks purely on text relevance), below
  `1.0` as `failed` grows past `helped` (a weaker text match can then outrank it), and up to `2.0`
  for a proven skill — `not_applicable` reports never enter the count either way. Ties break on
  `helped - failed` then id. Outcome counts are per published *version*, so republishing a skill
  (even as a wholesale replacement) starts its ranking factor fresh at `1.0`. The empty-query
  catalog (used for the SessionStart listing) does not use this factor at all: it is ordered only
  by `helped - failed` then id.
- **current_skills** — a view joining each skill to its latest published revision and outcome
  counts; backs search and the outcome summary.
- The schema is versioned with SQLite's `user_version` pragma. A fresh database is initialized to
  the current version; a database at the current version is left alone; a database with a pre-1
  schema is refused with a clear message to move it aside and create a new one; any other version
  fails to open with an upgrade prompt.

## MCP tools

- `search_skills(query, limit, offset)` — search published skill metadata; an empty query lists the
  catalog. Returns `skills` (metadata only), `total` count, and `has_more` flag for pagination.
  Bodies are never returned.
- `get_skill(id, version?)` — fetch one published skill's body, defaulting to its latest version.
- `report_skill_outcome(id, version, result, note)` — record whether an applied skill helped.
- `publish_skill(id, description, tags?, content, evidence, expected_version, scope?, verdict,
  verdict_reason, replaces_proven?)` — publish a new or replacement skill immediately; meant to be
  called only after a fresh-context evaluation. `verdict` must be exactly `"keep global"` or
  `"keep project"` (a `"discard"` verdict is refused outright) and must agree with `scope` (`keep
  project` requires a project `scope`, `keep global` requires none); `verdict_reason` is the
  evaluator's one-line reason, at most 280 bytes. `content` must contain the four required
  headings — `## When to use`, `## Procedure`, `## Pitfalls`, `## Verification` — in order.
  `description`, `content`, and `evidence` are scanned for credential-shaped values and the
  publish is refused if one is found. `replaces_proven` (default `false`) must be `true` to
  overwrite a version whose `helped` count exceeds its `failed` count.

## Concurrency

Every connection enables WAL mode and a 5-second busy timeout on open. `propose` runs in an
`IMMEDIATE` transaction, so two writers never interleave a read-check with a conflicting write — a
stale or competing proposal fails cleanly instead of corrupting state.
