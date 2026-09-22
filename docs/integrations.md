# Client integrations

What `skillvolution setup` writes, for each client selected with `--client`, a comma-separated
list of `claude-code`, `opencode`, `devin`, `all`, or `both` (claude-code + opencode, kept for
backwards compatibility).
Without `--client`, global setup detects installed clients (executable on `PATH` or existing
config directory), asks which of them to configure when a terminal is attached — including the
`curl | sh` install, which reads answers from `/dev/tty` — and configures every detected client
when there is no terminal or `SKILLVOLUTION_NO_INPUT` is set. Project setup without `--client`
defaults to `all`. With no `--project`, setup configures each client **globally, once per user**;
`--project PATH [--project-key KEY]` instead configures one project's own files (`--project-key`
requires `--project`).

## Global setup (default)

**Claude Code** — skill at `$CLAUDE_CONFIG_DIR/skills/evolution/SKILL.md` (`~/.claude/...` if
unset); `SessionStart`/`Stop` hooks merged into that directory's shared `settings.json` (existing
hooks and settings preserved); `permissions.allow` gains `Task` and `mcp__skillvolution` so the
evaluator subagent and the vault tools run without prompting (existing allow/deny rules are kept);
MCP server registered at user scope via `claude mcp add-json
--scope user` — if `claude` isn't on `PATH`, setup prints that command instead of failing.

**OpenCode** — config directory `$XDG_CONFIG_HOME/opencode` (`~/.config/opencode` if unset);
`mcp.skillvolution` entry merged into whichever of `opencode.json`/`opencode.jsonc` already exists
there (both present is refused; `opencode.json` is created if neither exists; a `.jsonc` file must
already be free of comments and trailing commas, since it's parsed as strict JSON, or setup
refuses); `permission.task` and each `skillvolution_*` MCP tool are set to `allow` where no value
exists yet, so the evaluator subagent and vault calls never prompt — an explicit user choice is
left untouched, and a bare-string `permission` (`"ask"`/`"allow"`/`"deny"`) is refused rather than
rewritten; skill at `<config dir>/skills/evolution/SKILL.md`; `AGENTS.md` marker block; plugin at
`<config dir>/plugins/skillvolution.js` (below).

**Devin CLI** — config directory `~/.config/devin` (`%APPDATA%\devin` on Windows; `XDG_CONFIG_HOME`
is not a documented Devin location and is not honored); `mcpServers.skillvolution` merged into
`mcp_config.json`; `config.json` gains `permissions.allow` entries `run_subagent`, `read_subagent`,
and `mcp__skillvolution__*` (existing allow/deny rules kept) plus lifecycle `hooks` (below);
skill at `skills/evolution/SKILL.md`; `AGENTS.md` marker block, which is also the standing
authorization Devin's prompt requires before dispatching subagents on its own.

Neither global MCP entry passes `--project`; `serve` detects it per-invocation instead (below).

## Project setup (`--project PATH`)

- `.mcp.json` / `opencode.json` / `.devin/mcp_config.json` — `skillvolution` entry running
  `serve --project <key>`, `<key>` defaulting to the project directory's sanitized name
  (`--project-key` to override).
- `.claude/skills/evolution/SKILL.md` / `.opencode/skills/evolution/SKILL.md` /
  `.devin/skills/evolution/SKILL.md` — the native skill.
- `.claude/settings.local.json` — hooks with `--project <key>` baked in; the **local** file, not
  the shared `settings.json`, since the commands embed this machine's absolute paths. The same
  `permissions.allow` grants (`Task`, `mcp__skillvolution`) are merged in.
- `opencode.json` also gets the `permission` grants described above.
- `.devin/config.json` — the Devin permissions and lifecycle hooks described below, with
  `--project <key>` baked into `session-start`.
- `AGENTS.md` (OpenCode, Devin) — the same marker-block reminder as global setup, shared by both
  clients through one marker pair.
- `.opencode/plugins/skillvolution.js` (OpenCode) — the plugin.
- `opencode.jsonc` is unsupported here: merge it into a strict `opencode.json` first.

## Runtime project detection

`serve` and `hook session-start` use an explicit `--project KEY` if given; otherwise the enclosing
git repository of the working directory, and only if there is none, that of `CLAUDE_PROJECT_DIR`
(set by Claude Code, which may start user-scope MCP servers outside the project). The key is the
sanitized name of the repository root; in a git worktree it is the main repository's name. Outside
a git repository only global skills are visible. Two unrelated repositories with the same directory
name share project scope. OpenCode starts local MCP servers in the project directory, so an
inherited `CLAUDE_PROJECT_DIR` never overrides it.

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
offset last recorded for that session in `hook_state`. If that span used a work tool that edits
files directly (`Edit`, `Write`, `MultiEdit`, `NotebookEdit` — shell commands such as `Bash` no
longer count as work) without a matching `report_skill_outcome` or `publish_skill` call, it prints
`{"hookSpecificOutput": {"hookEventName": "Stop", "additionalContext": <STOP_REASON>}}` to stdout
and exits 0. Claude Code still blocks the stop and feeds `STOP_REASON` (`src/hook.rs`) back to the
model, with the same `stop_hook_active` loop protection as the older `decision: "block"`/exit-2
mechanism, but the transcript labels it "Stop hook feedback" rather than a hook error notification
— `additionalContext` is documented as the softer of the two for the exact same blocking effect.
That reason tells the model to relay the fresh-context evaluator's verdict and its one-line reason
to `publish_skill` as the `verdict`/`verdict_reason` arguments; `publish_skill` rejects the call if
either is missing. The reviewed offset always advances to the last complete transcript line, so a
given unreviewed span blocks at most once, and `stop_hook_active` is honored so a stop already
continued by this hook is never blocked again.

## Devin hooks

Devin hook payloads carry a `session_id` but no transcript path, so the same worked/reviewed
judgment is kept as per-session flags in the vault's `devin_hook_state` table. The `config.json`
`hooks` key wires four entry points plus an approval hook:

- `SessionStart` → `hook session-start --client devin` prints the catalog inside a
  `hookSpecificOutput.additionalContext` envelope, the only form Devin injects.
- `PostToolUse` (matched on `write|edit|apply_patch|notebook_edit|mcp__skillvolution__.*`, built
  from `DEVIN_WORK_TOOLS` — shell commands such as `exec` no longer count as work) →
  `hook tool-use` marks the session as having worked or reviewed.
- `Stop` → `hook stop --client devin` prints `{"decision":"block","reason":...}` once per
  unreviewed span; the span is consumed on block, `stop_hook_active` is honored, and a stop after
  the agent ignored the reminder is not blocked again.
- `SessionEnd` → `hook session-end` drops the session's flag row.
- `PermissionRequest` (matched on `run_subagent|read_subagent|mcp__skillvolution__.*`) →
  `hook approve` prints `{"decision":"approve"}`. This is what makes the evaluator-subagent
  dispatch and the vault MCP calls run without prompting: `run_subagent`/`read_subagent` are real
  tool names but not among the documented `permissions` tool matchers (only `read`, `edit`,
  `grep`, `glob`, `exec` are), so the hook — not a permission rule — is the mechanism that actually
  lifts the prompt. The `permissions.allow` entries are kept anyway: `mcp__skillvolution__*` is a
  documented rule, and the subagent entries are harmless if Devin learns to honor them.

## OpenCode plugin

`plugins/skillvolution.js` (global) or `.opencode/plugins/skillvolution.js` (project) does two
things: injects the skill catalog into the system prompt once per session by running
`skillvolution hook session-start` in the project directory; and, once a turn that did work
(`edit`, `write`, `multiedit`, `patch`/`apply_patch` — shell commands such as `bash` no longer
count as work) goes idle without a `report_skill_outcome` or `publish_skill` call, adds a review
reminder for the evolution skill's report/reflect steps to the
system prompt of the following turns until a review tool is called. It never prompts the session
itself, so no extra model turn is spent; subagent (task) sessions hand their work and reviews to
the parent session, whose context alone carries the reminder; and a span ending in an error or
abort is skipped without clearing a pending reminder. `opencode run` (one-shot) sessions see the
reminder inside their normal steps like any other turn.

## Evolution skill

All clients get the same file, `assets/evolution/SKILL.md`, installed as their native skill. It
walks an agent through retrieving relevant skills before non-trivial work, reporting outcomes after
applying one, and — for a candidate lesson meeting every criterion — having a fresh subagent judge
it with clean context (`keep global`, `keep project`, or `discard`) before `publish_skill` makes it
visible immediately. See that file for the exact procedure.
