# Client integrations

What `skillvolution setup` writes, for each client selected with `--client both|opencode|claude-code`.
Without `--client`, global setup configures only detected clients (executable on `PATH` or existing
config directory) and project setup defaults to `both`. With no `--project`, setup configures each client **globally, once per user**;
`--project PATH [--project-key KEY]` instead configures one project's own files (`--project-key`
requires `--project`).

## Global setup (default)

**Claude Code** — skill at `$CLAUDE_CONFIG_DIR/skills/evolution/SKILL.md` (`~/.claude/...` if
unset); `SessionStart`/`Stop` hooks merged into that directory's shared `settings.json` (existing
hooks and settings preserved); MCP server registered at user scope via `claude mcp add-json
--scope user` — if `claude` isn't on `PATH`, setup prints that command instead of failing.

**OpenCode** — config directory `$XDG_CONFIG_HOME/opencode` (`~/.config/opencode` if unset);
`mcp.skillvolution` entry merged into whichever of `opencode.json`/`opencode.jsonc` already exists
there (both present is refused; `opencode.json` is created if neither exists; a `.jsonc` file must
already be free of comments and trailing commas, since it's parsed as strict JSON, or setup
refuses); skill at `<config dir>/skills/evolution/SKILL.md`; `AGENTS.md` marker block; plugin at
`<config dir>/plugins/skillvolution.js` (below).

Neither global MCP entry passes `--project`; `serve` detects it per-invocation instead (below).

## Project setup (`--project PATH`)

- `.mcp.json` / `opencode.json` — `skillvolution` entry running `serve --project <key>`, `<key>`
  defaulting to the project directory's sanitized name (`--project-key` to override).
- `.claude/skills/evolution/SKILL.md` / `.opencode/skills/evolution/SKILL.md` — the native skill.
- `.claude/settings.local.json` — hooks with `--project <key>` baked in; the **local** file, not
  the shared `settings.json`, since the commands embed this machine's absolute paths.
- `AGENTS.md` (OpenCode) — the same marker-block reminder as global setup.
- `.opencode/plugins/skillvolution.js` (OpenCode) — the plugin.
- `opencode.jsonc` is unsupported here: merge it into a strict `opencode.json` first.

## Runtime project detection

`serve` and `hook session-start` use an explicit `--project KEY` if given; otherwise
`CLAUDE_PROJECT_DIR` (set by Claude Code for MCP servers and hooks) if set, else the working
directory — and derive the key from the sanitized name of that directory's enclosing git
repository root. Outside a git repository, no key is detected and only global skills are visible.
Two unrelated repositories checked out under directories with the same name therefore share
project scope. OpenCode launches each local MCP server with the project directory as its working
directory, so this resolves correctly per project despite the global config having no `--project`.

## Setup safety

- Every planned change is validated before anything is written, so a failing check leaves the
  target untouched.
- Config files must be strict JSON with unique keys (global OpenCode setup accepts `opencode.jsonc`
  on the same strict-JSON terms; project setup does not).
- Merging an MCP entry only touches the `skillvolution` key, preserving other entries in the file.
- Each changed file gets numbered backups (`.skillvolution.bak`, `.bak.1`, ...) before being
  modified, created once and never overwritten by later runs.
- Setup refuses to write through a config file that is itself a symlink, and refuses an existing
  `SKILL.md` or plugin file that isn't already Skillvolution-managed.
- A write that would produce byte-identical content is skipped, so repeated setup makes no changes.

## Hooks

`hook session-start` prints a short reminder plus a catalog of up to 30 published skills visible
to the project (id, version, description, helped/failed counts). Global hooks omit `--project` and
rely on the same auto-detection as `serve`; project-mode hooks bake in `--project <key>`.

`hook stop` reads the Claude Code Stop payload from stdin and scans the transcript since the
offset last recorded for that session in `hook_state`. If that span used a work tool (`Edit`,
`Write`, `MultiEdit`, `NotebookEdit`, `Bash`) without a matching `report_skill_outcome` or
`propose_skill_change` call, it prints a reason to stderr and exits 2, which Claude Code treats as
"block the stop and show the model this reason." The reviewed offset always advances to the last
complete transcript line, so a given unreviewed span blocks at most once, and `stop_hook_active` is
honored so a stop already continued by this hook is never blocked again.

## OpenCode plugin

`plugins/skillvolution.js` (global) or `.opencode/plugins/skillvolution.js` (project) does two
things: injects the skill catalog into the system prompt once per session by running
`skillvolution hook session-start` in the project directory; and, once a session goes idle after
doing work (`edit`, `write`, `patch`/`apply_patch`, `bash`) without a `report_skill_outcome` or
`propose_skill_change` call, sends one review prompt for the evolution skill's report/reflect
steps. It doesn't loop — the turn that prompt itself causes is never re-reviewed — skips subagent
(task) sessions (their work is attributed to the parent instead), and skips a session that just
errored or aborted, so the next idle starts clean. `opencode run` (one-shot) exits on idle before
any review prompt is sent, so non-interactive runs never see one.

## Evolution skill

Both clients get the same file, `assets/evolution/SKILL.md`, installed as their native skill. It
walks an agent through retrieving relevant skills before non-trivial work, reporting outcomes after
applying one, and proposing a verified, non-obvious, reusable lesson afterward — never publishing
it itself. See that file for the exact procedure.
