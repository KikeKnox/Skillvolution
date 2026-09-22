# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

Skillvolution is a shared procedural-memory vault for AI coding agents: a Rust binary that is both
a CLI and a local MCP server (SQLite-backed) exposed to Claude Code, OpenCode, and Devin CLI.
Agents search and apply published skills, report whether they worked, and publish new lessons —
after a fresh-context subagent judges them worth keeping — visible immediately to future sessions.

## Commands

```bash
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
cargo test --locked <test_fn_name>       # single test, any integration file
cargo test --locked --test core_vault    # one integration test file
```

This is exactly what CI (`.github/workflows/ci.yml`) runs — match it locally before pushing.
No rustfmt.toml/clippy.toml; both tools use their defaults.

Build/install a local binary: `cargo install --locked --path .`

Releasing (see `README.md` for full detail): bump `Cargo.toml` version, commit, then
`git tag vX.Y.Z && git push origin vX.Y.Z` — cargo-dist's Release workflow builds and publishes.

## Architecture

Full module-by-module and data-model reference: **`docs/architecture.md`**. Client-integration
details (exactly what `setup` writes per client, hook wiring, the OpenCode plugin): **`docs/integrations.md`**.
Read both before touching `src/setup/*` or `src/hook.rs` — the behavior is deliberately precise and
under-documenting-in-code is intentional; the docs are the spec.

High-level shape:

- **`src/vault/`** — SQLite access. `mod.rs` opens the connection (WAL, busy timeout, foreign
  keys), runs schema migration (`schema.sql`, versioned via `PRAGMA user_version`), and holds
  validation helpers (`validate_id`, `validate_text`, `validate_tags`, `validate_sections`,
  `validate_no_secrets`, ...). `revisions.rs` is publish/inspect/diff/get. `search.rs` builds
  ranked FTS5 queries (bm25 relevance scaled by each skill's `helped - failed` record).
  `outcomes.rs` records helped/failed/not_applicable reports.
- **`src/mcp.rs`** — JSON-RPC framing over stdio and tool dispatch for the four MCP tools:
  `search_skills`, `get_skill`, `report_skill_outcome`, `publish_skill(id, description, tags?,
  content, evidence, expected_version, scope?, verdict, verdict_reason, replaces_proven?)`.
- **`src/hook.rs`** — SessionStart catalog text and the Stop transcript scan, kept independent of
  stdio so both are unit-testable against fixture transcripts.
- **`src/main.rs`** — clap CLI; `resolve_project` picks `--project` when given, else the working
  directory's enclosing git repo, falling back to `CLAUDE_PROJECT_DIR`, for `serve` and
  `hook session-start`.
- **`src/project.rs`** — derives a project key from a directory's enclosing git repository root.
- **`src/setup/`** — everything `skillvolution setup` writes, split project (`claude.rs`,
  `opencode.rs`) vs. global/per-user (`global.rs`, `global_claude.rs`, `global_opencode.rs`,
  `global_devin.rs`) vs. shared helpers (`fs_safe.rs` for strict-JSON/marker-block/backup-before-write,
  `hooks.rs` for merging hook entries, `plugin.rs` for the OpenCode plugin template,
  `claude_cli.rs` for registering the MCP server via `claude mcp add-json`, `detect.rs` for
  client auto-detection).
- **`assets/evolution/SKILL.md`** — the native skill installed for every client; the actual
  retrieve/report/reflect/write/evaluate procedure agents follow. Read this to understand the
  product's UX, not just the storage layer.
- **`assets/opencode/skillvolution.js`** — the OpenCode plugin template.

Key invariants worth knowing before editing:

- Revisions publish immediately (no draft/approval step in the current flow); `propose` runs in an
  `IMMEDIATE` transaction so concurrent writers can't interleave a read-check with a conflicting
  write. `expected_version` is optimistic-concurrency: it must match the currently published
  version or the proposal fails.
- A skill's `scope` (global vs. a project key) is fixed on first proposal and can never change.
- Setup is designed to be idempotent and safe to rerun: every planned change is validated before
  anything is written, changed files get numbered backups, byte-identical writes are skipped, and
  setup refuses to write through symlinks or through a `SKILL.md`/plugin file it doesn't already own.
- Two unrelated git repos with the same directory name share project scope — this is a known,
  documented limitation, not a bug to fix incidentally.
- Publishing requires a `keep` verdict (`keep global` or `keep project`, matching `scope`), a
  `content` body with the four required sections in order, and no credential-shaped values in
  `description`/`content`/`evidence`; overwriting a version with a net-positive `helped - failed`
  record additionally requires explicit acknowledgement via `replaces_proven: true`.

## Testing conventions

Integration tests live in `tests/*.rs` (one file per area — `core_vault`, `core_mcp`, `core_cli`,
`core_concurrency`, `hook`, `setup`, `setup_global`) and share fixtures via `tests/support/mod.rs`,
notably the `Draft` builder for `Proposal`s with valid filler defaults. `Draft`'s default `content`
is already wrapped in the four required sections by the `sectioned()` helper; `Draft::content(...)`
wraps whatever body a test passes through `sectioned()` automatically, while
`Draft::raw_content(...)` sets `content` literally, unwrapped, for tests that need to exercise
invalid or exact content. Each test opens its own temp SQLite file/dir — never share a database
path across tests.
