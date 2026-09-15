# Skillvolution: initial implementation

## Scope

A local procedural-memory vault, shared by OpenCode and Claude Code through MCP. English is the project's language. Rust implements storage, protocol handling, and client configuration. Linux is the deployment target; Windows is only the current development host. Hermes integration is excluded.

Engineering goals: KISS, cohesive modules, minimal comments, no dead code, and extensibility through module boundaries rather than speculative abstractions. An agent vault, vector search, web UI, HTTP service, schedulers, and plugin framework are not part of this version.

## Runtime

Each client starts the same binary as a stdio MCP subprocess. Processes share a SQLite database using explicitly enabled WAL mode and a busy timeout. Standard output carries MCP messages only. Logs go to standard error. Setup is a separate CLI command, not a side effect of starting the MCP server.

The MCP tool surface is fixed:

- `search_skills`: query published skill metadata with a bounded limit and pagination; never return full bodies. Empty query lists the catalog.
- `get_skill`: retrieve one published skill, optionally at an exact published revision.
- `propose_skill_change`: create an immutable draft revision with its evidence and expected base revision. It cannot publish.

The local CLI provides draft inspection and explicit publication. Publication means a human accepted the proposal, not that an automated quality evaluation proved it. Validation checks content structure, identifiers, sizes, evidence, and revision conflicts. Stored evidence is supplied by the proposing agent and is not independently verified by the vault.

Skills use stable lowercase hyphenated identifiers and monotonically increasing integer revisions. Publication verifies the proposal's base against the currently published revision. Competing stale proposals fail rather than overwrite newer work. Previously published revisions remain retrievable. Drafts do not appear in the default MCP catalog. A new publication is visible to an already-running client on its next tool query; existing tool responses are not retroactively replaced.

## Modules and ownership contracts

- `src/vault.rs` and related core types: persistence, search, proposal, publication.
- `src/mcp.rs`: protocol and tool routing.
- `src/main.rs`: CLI dispatch.
- `src/setup.rs`: client configuration and native Evolution installation.
- `assets/`: Evolution skill and small per-client instruction snippets.
- `launch.sh`: prerequisite checks, build/install, setup command invocation.
- `tests/`: real SQLite, stdio protocol, safe setup, and launcher checks.

Integration contract: `src/setup.rs` defines `#[derive(clap::Args)] pub struct SetupArgs` and `pub fn run(args: SetupArgs) -> anyhow::Result<()>`. `main.rs` includes `mod setup` and a `Setup(setup::SetupArgs)` subcommand. Setup accepts `--project PATH`, `--client both|opencode|claude-code`, `--bin PATH`, and `--db PATH`. The installed command is `[absolute_binary, "--db", absolute_database, "serve"]`. Setup depends only on anyhow, clap, serde_json, and std; tempfile is available for tests.

## Client integration

Project-scoped configuration is the safe default. Claude Code uses `.mcp.json` with an explicit `type: stdio`, command, and args. OpenCode uses its documented local MCP entry in `opencode.json`. Existing values outside Skillvolution's entry are preserved. Malformed, ambiguous, or unsupported config is rejected before writes. Existing files are backed up on changes; repeated identical setup does not duplicate content. Partial failures must be reported honestly. Config symlinks and conflicting unowned Skillvolution files require a safe refusal.

Each client receives a native `evolution` bootstrap skill and a concise project instruction reference. The bootstrap searches and loads vault skills on demand, reviews meaningful outcomes, updates existing skills before creating duplicates, and proposes evidence-backed lessons. No new lesson is a valid result. It separates reusable procedures from task state, excludes secrets and personal data, respects tool permissions, and never publishes its own proposals.

Initial review triggering is instruction-driven, not a guaranteed lifecycle hook. This limitation is explicit; installing a skill does not ensure an LLM follows it. Client configuration acceptance and native discovery must be distinguished from a live authenticated LLM end-to-end test.

## Launcher

`launch.sh` builds the Rust release binary, installs it at a user-writable location, creates/initializes the shared database, and configures the requested project/clients. Paths containing spaces must work. No sudo, hidden dependency downloads, system services, or automatic agent installation. If Rust is absent, an explicit opt-in may bootstrap rustup; otherwise give actionable prerequisites. Help must work without Rust. Invalid options fail before side effects. Setup is tested only against disposable destinations on the development machine.

## Acceptance

1. Rust builds; formatting, all-target tests, and Clippy pass.
2. SQLite survives restart and concurrent access; draft/publication conflicts are tested.
3. A real subprocess completes MCP initialization and tools/list, proposes a draft, and observes a later CLI publication without restart.
4. Search responses omit skill bodies, and exact published revision reads are stable.
5. Both client configurations match their documented schemas; repeated setup preserves unrelated settings and produces no duplicate instructions.
6. Evolution, English usage documentation, and launch.sh are present and exercised.
7. Linux-only verification and live client sessions are reported as unverified unless actually executed.

## Documentation references

- https://opencode.ai/docs/mcp-servers/
- https://opencode.ai/docs/skills/
- https://code.claude.com/docs/en/mcp
- https://code.claude.com/docs/en/skills
- https://modelcontextprotocol.io/specification/2025-06-18/server/tools
