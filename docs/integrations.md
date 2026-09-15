# Client integrations

What `skillvolution setup` writes into a project, for each client selected with
`--client both|opencode|claude-code` (default `both`).

## Claude Code

- `.mcp.json` — inserts or updates `mcpServers.skillvolution`:
  ```json
  {"type": "stdio", "command": "<absolute-bin>", "args": ["--db", "<absolute-db>", "serve", "--project", "<key>"]}
  ```
- `.claude/skills/evolution/SKILL.md` — the native skill (see below).
- `.claude/settings.local.json` — merges a `SessionStart` and a `Stop` hook, each invoking
  `<bin> --db <db> hook ...` with the path and project key shell-quoted in. Hooks go in the
  **local** settings file, not the shared `settings.json`, because the commands embed this
  machine's absolute binary and database paths, which should not be committed or shared.

## OpenCode

- `opencode.json` — inserts or updates `mcp.skillvolution`:
  ```json
  {"type": "local", "command": ["<absolute-bin>", "--db", "<absolute-db>", "serve", "--project", "<key>"], "enabled": true}
  ```
- `.opencode/skills/evolution/SKILL.md` — the native skill (see below).
- `AGENTS.md` — merges a marker block (`<!-- skillvolution:start -->` / `:end`) reminding the agent
  to use the `evolution` skill; OpenCode has no hook mechanism, so this is its only trigger.

## Setup safety

- Every planned change is validated before anything is written, so a failing check (a malformed
  config, a conflicting entry) leaves the project untouched.
- Merging an MCP entry only touches the `skillvolution` key, preserving any other server entries
  and foreign keys in the file.
- Each changed file gets one `.skillvolution.bak` backup, made once and never overwritten by a
  later setup run.
- A write that would produce byte-identical content is skipped, so repeated setup makes no changes.
- Setup refuses an existing `SKILL.md` at the target path that isn't already Skillvolution-managed,
  and refuses a project using `opencode.jsonc` (merge it into a strict `opencode.json` first).

## Hooks

`hook session-start` prints a short reminder plus a catalog of up to 30 published skills visible
to the project (id, version, description, helped/failed counts) so discovery doesn't need an
explicit search for trivial tasks.

`hook stop` reads the Claude Code Stop payload from stdin and scans the transcript since the
offset last recorded for that session in `hook_state`. If that span used a work tool (`Edit`,
`Write`, `MultiEdit`, `NotebookEdit`, `Bash`) without a matching `report_skill_outcome` or
`propose_skill_change` call, it prints a reason to stderr and exits 2, which Claude Code treats as
"block the stop and show the model this reason." The reviewed offset always advances to the last
complete transcript line, so a given unreviewed span blocks at most once, and `stop_hook_active` is
honored so a stop already continued by this hook is never blocked again.

## Evolution skill

Both clients get the same file, `assets/evolution/SKILL.md`, installed as their native skill. It
walks an agent through retrieving relevant skills before non-trivial work, reporting outcomes after
applying one, and proposing a verified, non-obvious, reusable lesson afterward — never publishing
it itself. See that file for the exact procedure.
