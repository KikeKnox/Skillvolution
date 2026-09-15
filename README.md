# Skillvolution

Skillvolution is a local, shared procedural-memory vault for AI coding agents. The initial
clients are OpenCode and Claude Code; a single SQLite database is exposed to each client as a
local Model Context Protocol server. Drafts are proposed by agents, evidence is supplied by the
agent, and publication is an explicit human command.

Hermes integration is out of scope.

## What it does

- A single CLI binary, `skillvolution`, is both the SQLite-backed vault and the MCP stdio server.
- Three tools are exposed to clients:
  - `search_skills(query, limit, offset)` returns only published skill metadata
    (id, version, one-line description); bodies are never returned.
  - `get_skill(id, version?)` returns one published skill revision, optionally at an exact
    integer version. Drafts are not exposed.
  - `propose_skill_change(id, description, content, evidence, expected_version)` stores an
    immutable draft. The vault verifies `expected_version` against the current published revision
    (0 for a new skill) so stale proposals fail.
- Humans review drafts with `drafts`, inspect with `show ID --version N`, and accept with
  `publish ID --version N`.
- Storage is SQLite with WAL mode and a busy timeout; the database is created on first use and
  initializes idempotently.

## Quick start

```bash
# Build and configure an existing project for both clients.
bash launch.sh --project /path/to/project

# Inspect what was installed.
ls /path/to/project/.mcp.json /path/to/project/.claude/skills/evolution/SKILL.md \
   /path/to/project/.opencode/skills/evolution/SKILL.md

# Review any drafts after a session.
skillvolution drafts
skillvolution show my-skill --version 1
skillvolution publish my-skill --version 1
```

`launch.sh` honours `XDG_DATA_HOME` for the default database location. Pass `--db`, `--bin-dir`,
`--client {both,opencode,claude-code}`, or `--install-rust` to override. Use `--help` for the
full set; the launcher exits with usage information without touching the filesystem.

## CLI

```
skillvolution [global --db PATH] <command>

Commands
  init                    Initialize the database (idempotent)
  serve                   Run the MCP server on stdio
  drafts                  List unpublished drafts as JSON
  show ID --version N     Print a revision as JSON
  publish ID --version N  Publish a draft
  setup --project PATH    Configure a project to use the vault
        --client both|opencode|claude-code
        --bin PATH
        --db PATH
```

The default database path is `$XDG_DATA_HOME/skillvolution/skills.db`, falling back to
`$HOME/.local/share/skillvolution/skills.db` when `XDG_DATA_HOME` is unset.

## Storage model

- Skills are identified by stable lowercase-hyphenated strings (1..64 bytes).
- Each skill has monotonically increasing integer revisions.
- A draft is a row with `published = 0`. Publication sets the flag in an `IMMEDIATE` transaction
  that re-checks the base revision, so two operators cannot both publish conflicting drafts.
- Drafts are visible only through the `drafts`/`show`/`publish` CLI. The MCP catalog always
  reflects the latest published revision per id and never includes drafts.
- `expected_version` must match the current published version (or 0 for a new skill); any other
  value is rejected as a stale base.

## Limits

- id: 1..64 bytes, lowercase letters/digits separated by single hyphens
- description: 1..280 bytes, single line, no control characters
- content: 1..65 536 bytes
- evidence: 1..16 384 bytes
- search query: 1..512 bytes
- search limit: 1..100

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
  or trailing commas, and backs up files before modifying them. Setup is transactional: if any
  configuration check fails, no client file is written.

## Development

```bash
cargo test --locked            # full suite (25 tests)
cargo clippy --all-targets --locked -- -D warnings
cargo build --release --locked
```

The build is reproducible via `Cargo.lock`. The launcher uses `--locked` as well; if a
developer changes dependencies they must regenerate `Cargo.lock`.

## Documentation references

- OpenCode MCP servers: <https://opencode.ai/docs/mcp-servers/>
- OpenCode agent skills: <https://opencode.ai/docs/skills/>
- Claude Code MCP: <https://code.claude.com/docs/en/mcp>
- Claude Code skills: <https://code.claude.com/docs/en/skills>
- Model Context Protocol: <https://modelcontextprotocol.io/specification/2025-06-18/server/tools>
