# Contributing

## Prerequisites

- **Rust 1.88+** (`rust-version` in `Cargo.toml`; edition 2024). CI checks
  the MSRV with `cargo check --locked --all-targets` on 1.88 and builds on
  stable.
- **Node.js 20+** (optional) — `node --check` on the rendered OpenCode
  plugin (`assets/opencode/skillvolution.js`).
- **shellcheck** (optional) — for `install.sh`.

## Build, test, lint

Exactly what CI runs (`.github/workflows/ci.yml`):

```bash
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo build --locked --all-targets
cargo test --locked
cargo check --locked --all-targets   # also on Rust 1.88 (MSRV job)
shellcheck install.sh
```

Plugin syntax check (mirrors the CI job):

```bash
sed -e 's|__SKILLVOLUTION_BIN__|"/usr/bin/skillvolution"|g' \
    -e 's|__SKILLVOLUTION_DB__|"/tmp/skillvolution.db"|g' \
    -e 's|__SKILLVOLUTION_PROJECT_KEY__|null|g' \
    assets/opencode/skillvolution.js > /tmp/skillvolution.mjs
node --check /tmp/skillvolution.mjs
```

Tests run on Linux, macOS, and Windows in CI; fmt/clippy run on Linux only.

## Test layout

`tests/` — each file shares helpers via `tests/support/mod.rs`:

| File | Covers |
|---|---|
| `core_vault.rs` | Publish validation (ids, tags, sections, secrets), revisions, deprecate, outcomes — in-process against `Vault` |
| `core_mcp.rs` | MCP server over stdio: initialize, tools/list, tools/call against a spawned binary |
| `core_gaps.rs` | MCP and CLI edge cases (ping, unknown methods, malformed calls) |
| `core_cli.rs` | CLI commands end-to-end (`show`, `deprecate`, `outcomes`, help, error paths) via the built binary |
| `core_concurrency.rs` | Concurrent opens/publishes, busy-timeout behavior, atomic hook flags |
| `hook.rs` | Claude transcript scanning and flag-tracked client hooks (`tool-use`/`stop`/`session-end`/`approve`) |
| `setup.rs` | Project-scope setup for Claude/OpenCode in-process: merges, backups, idempotence, conflict rejection |
| `setup_global.rs` | Global setup via the built binary with `HOME`/`XDG_CONFIG_HOME`/`CLAUDE_CONFIG_DIR`/`PATH` pinned |
| `setup_remove.rs` | `setup --remove` and `setup --dry-run` |
| `setup_codex.rs`, `setup_gemini.rs`, `setup_cursor.rs` | Per-client setup: project scope in-process, global scope via the binary |
| `plugin.rs` | OpenCode plugin: project-key propagation and syntax sanity |
| `doctor.rs` | `doctor` checks via the built binary |
| `location.rs` | `SKILLVOLUTION_DB` precedence and `relocate`, via the built binary |
| `transfer.rs` | `purge`, `export`/`import`, `backup` |

In-process tests use `skillvolution::setup`/`Vault` directly; tests that
need to pin process-global state (env vars, stdin, PATH) spawn the built
binary.

## Conventions

- `cargo fmt` before committing; clippy runs with `-D warnings`.
- Always pass `--locked`; update `Cargo.lock` in the same commit when you
  change dependencies.
- `assets/evolution/SKILL.md` carries a managed-version marker
  (`skillvolution-managed:evolution:vN`); bump `N` whenever the file
  changes — it's what `setup` uses to recognize and refresh its own
  installed copies.
- `CLAUDE.md` is gitignored — don't commit it.
- Client configs are written atomically with backup + rollback; preserve
  that property (see `setup/fs_safe.rs`, `setup/mod.rs::write_all`).
- Hooks must keep failing open: print to stderr, exit 0.

## Adding a client

1. Add a `ClientKind` variant in `src/setup/client.rs` (and its
   `token`/`display_name`/`detect`/parse arms, plus the `--client` value in
   `SetupArgs`).
2. Add `src/setup/<name>.rs` implementing `global_dir()`, `changes(scope,
   bin, db)`, and `removals(scope, notes)` — return `Change`/`Edit` lists;
   never write directly.
3. Wire it into the inspect/global/remove arms (`setup/inspect.rs`,
   `setup/global.rs`, `setup/remove.rs` including `summary_line`).
4. If the client has lifecycle hooks, add it to `HookClient` in
   `src/hook.rs` (work tools, session-id extraction, output shape) and reuse
   `setup/hooks.rs` to build entries.
5. Add `tests/setup_<name>.rs` following the per-client pattern above.

## Release process

1. Bump `version` in `Cargo.toml`, run `cargo build` so `Cargo.lock` picks
   up the new version, and commit both.
2. Update `CHANGELOG.md` (move `Unreleased` changes under the new version).
3. Tag `vX.Y.Z` and push the tag.
4. `cargo dist` (`dist-workspace.toml`) builds the release binaries for
   Linux (glibc + musl, x86_64/aarch64), macOS (x86_64/aarch64), and Windows
   (x86_64). The release workflow is gated on CI via
   `plan-jobs = ["./ci"]` — a tag only releases code that passed tests.
5. The release publishes `install.sh` and `install.ps1`; installers support
   `SKILLVOLUTION_VERSION`, `SKILLVOLUTION_INSTALL_DIR`,
   `SKILLVOLUTION_NO_MODIFY_PATH`, `SKILLVOLUTION_NO_SETUP`, and
   `SKILLVOLUTION_NO_INPUT`.
