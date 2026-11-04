# Client integrations

`skillvolution setup` wires each agent client to the vault. Run it globally
(once per user, shared by every project) or per project with
`--project DIR`.

```bash
skillvolution setup                          # global, detect + prompt
skillvolution setup --client claude-code,codex
skillvolution setup --project .              # all clients, this project
skillvolution setup --dry-run ...            # preview as unified diffs
skillvolution setup --remove ...             # undo only what setup owns
```

`--client` accepts `all`, `both` (Claude Code + OpenCode), `claude-code`,
`opencode`, `devin`, `codex`, `gemini`, `cursor`. Global setup detects a
client by its executable on `PATH` or an existing config directory;
undetected clients are skipped. Project setup defaults to all clients.
Every hook command has the form `'BIN' --db 'DB' hook <subcommand>` with a
10-second timeout; project setups add `--project 'KEY'` to `session-start` and
`prompt` (the two retrieval hooks) only. A rerun recognizes its own entries by that
command text and rewrites them in their existing slot; a hook group that removing
ours leaves empty is dropped, while any other group — including a foreign one that
was already empty — is kept as is.

Every client also gets three managed skills, each at `skills/<name>/SKILL.md`
under its config directory (`.claude/skills/…`, `.codex/skills/…` and so on in a
project): `evolution`, `skill-evaluation` (the evaluator subagent's rubric), and
`skill-acquisition` (what an agent follows when it lacks know-how). Each carries
its own marker `skillvolution-managed:<name>:vN`. Where this page says "the skill
file" below, it means all three. The `AGENTS.md`/`GEMINI.md` marker block and the
Cursor rule mention `skill-acquisition` and `skill-evaluation`.

Retrieval happens in two hooks: `session-start` (catalog) and `prompt` (skills
matching each submitted prompt, see
[architecture.md](architecture.md#prompt-hook)). `prompt` is wired as
`UserPromptSubmit` for Claude Code, Codex, and Devin, `BeforeAgent` for Gemini CLI,
via the plugin's `chat.message` for OpenCode, and not at all for Cursor.

All snippets below come from real `setup --dry-run` output with paths
shortened to `BIN`/`DB`.

> **Security note:** Every project-scope MCP server entry is written whole, discarding any pre-existing `env`, `cwd`, or `url` on that entry; global entries are merged and keep those keys. This prevents malicious pre-seeded MCP configurations in untrusted repositories.

## Where the vault lives

`--db` wins, then `SKILLVOLUTION_DB`, then:

- Windows: `%LOCALAPPDATA%\skillvolution\skills.db`
- Elsewhere: `$XDG_DATA_HOME/skillvolution/skills.db` (absolute paths only),
  else `~/.local/share/skillvolution/skills.db`

Setup warns (but proceeds) when the database sits on a network filesystem —
SQLite WAL needs local storage.

## Claude Code

**Global** (`$CLAUDE_CONFIG_DIR`, default `~/.claude`):

- `settings.json` — merges hooks and permissions:

```json
{
  "hooks": {
    "SessionStart": [{"hooks": [{"type": "command",
      "command": "'BIN' --db 'DB' hook session-start", "timeout": 10}]}],
    "UserPromptSubmit": [{"hooks": [{"type": "command",
      "command": "'BIN' --db 'DB' hook prompt", "timeout": 10}]}],
    "Stop": [{"hooks": [{"type": "command",
      "command": "'BIN' --db 'DB' hook stop", "timeout": 10}]}]
  },
  "permissions": {"allow": ["Agent", "Task", "mcp__skillvolution"]}
}
```

- `skills/evolution/SKILL.md`, `skills/skill-evaluation/SKILL.md`,
  `skills/skill-acquisition/SKILL.md` — managed skill files (markers
  `skillvolution-managed:<name>:vN`).
- MCP registration via the `claude` CLI — `~/.claude.json` is never edited:

```bash
claude mcp add-json --scope user skillvolution \
  '{"type":"stdio","command":"BIN","args":["--db","DB","serve"]}'
```

  Setup first reads `claude mcp get skillvolution`: a user-scope stdio registration
  whose `Command:` is `BIN` and whose `Args:` are `--db DB serve` is left as is
  (setup prints `skillvolution is already registered with Claude Code; unchanged.`,
  and `--dry-run` shows no `add-json` line). Anything else — missing, another
  scope, command or args, or output in a shape it doesn't recognize — is
  (re-)registered: `add-json`, and on "already exists" `remove` then `add-json`.

**Project** (`<dir>`): `.mcp.json` gets the same server entry (with
`"--project", "KEY"` in `args`), `.claude/settings.local.json` gets the
hooks and permissions, and the three skills are installed under
`.claude/skills/<name>/SKILL.md`. SessionStart and UserPromptSubmit carry
`--project 'KEY'`; Stop does not (it infers nothing — it only scans the transcript).

Legacy cleanup: a Skillvolution-managed block in `CLAUDE.md` is removed.

**Reminder behavior** (verified live): `SessionStart` prints the catalog as
`hookSpecificOutput.additionalContext` JSON; plain stdout reaches the model only
as a "hook success" status line. `Stop` scans the session transcript JSONL from the stored offset;
work = `Edit`/`Write`/`MultiEdit`/`NotebookEdit` or a successful `get_skill`, a review
counts only when `publish_skill` or `report_skill_outcome` succeeds. If the latest work is
newer than the latest successful review, the hook prints
`{"hookSpecificOutput":{"hookEventName":"Stop","additionalContext":...}}`
and Claude continues the same turn; `stop_hook_active` prevents repeat
blocking.

**Prompt suggestions**: `UserPromptSubmit` runs `hook prompt` with the prompt on
stdin and prints
`{"hookSpecificOutput":{"hookEventName":"UserPromptSubmit","additionalContext":...}}`
when a vault skill matches, nothing otherwise.

**`setup --remove`**: deletes the three skill files (and their directories once empty;
a file without its marker is skipped with a note), strips the three hook entries
and the three permission grants, removes the `.mcp.json`/user-scope MCP
entry (`claude mcp remove` globally), and keeps every user-owned key.

## OpenCode

**Global** (`$XDG_CONFIG_HOME/opencode`, default `~/.config/opencode`;
`opencode.jsonc` is used when it already exists and parses as JSON (trailing
commas allowed) — with comments it errors and asks you to merge manually;
both files existing is an error):

- `opencode.json` (or `.jsonc`):

```json
{
  "mcp": {"skillvolution": {"type": "local",
    "command": ["BIN", "--db", "DB", "serve"], "enabled": true}},
  "permission": {
    "task": "allow",
    "skillvolution_search_skills": "allow",
    "skillvolution_get_skill": "allow",
    "skillvolution_report_skill_outcome": "allow",
    "skillvolution_publish_skill": "allow"
  }
}
```

- the three skills (`skills/<name>/SKILL.md`), `AGENTS.md` marker block, and
  `plugins/skillvolution.js` — the plugin that drives reminders (with `BIN`,
  `DB`, and the project key baked in).

**Project** (`<dir>`): `opencode.json` at the root (`opencode.jsonc` in a
project is refused), `AGENTS.md` marker block,
`.opencode/skills/<name>/SKILL.md` (three), `.opencode/plugins/skillvolution.js`.
A legacy `instructions` entry pointing at the old skill path is dropped and
never recreated.

**Reminder behavior** (plugin, not hooks): the plugin injects the catalog
into the system prompt, tracks work (`edit`, `write`, `multiedit`, `patch`,
`apply_patch`, or a tool ending in `get_skill`) and review tools (any tool containing
`publish_skill` or `report_skill_outcome`) per top-level session — subagent sessions
count toward the parent — and appends a reminder to the next turn after unreviewed
work. For each new user message, the `chat.message` hook runs `hook prompt` (the
message's non-synthetic text parts as `{"prompt": ...}` on stdin, `--project KEY` in
project setups, a 15-second timeout) and the plugin appends the suggestions to that
turn's system prompt; subagent sessions get none, and a failed lookup only costs that
turn's suggestions. The plugin never spawns its own prompt turn, retries catalog-load
failures, and strips an inherited `CLAUDE_PROJECT_DIR` from the child environment.

**`setup --remove`**: deletes the three skill files and the plugin (and emptied skill
directories; a skill file without its marker is skipped with a note), strips the `mcp`
entry, the four `skillvolution_*` grants, `task` if setup set it, and the
AGENTS.md block.

## Devin CLI

**Global** (`~/.config/devin`, or `%APPDATA%\devin` on Windows;
`XDG_CONFIG_HOME` is not consulted):

- `mcp_config.json`:

```json
{"mcpServers": {"skillvolution": {"command": "BIN",
  "args": ["--db", "DB", "serve"]}}}
```

- `config.json` — permissions plus all six hooks:

```json
{
  "permissions": {"allow": ["run_subagent", "read_subagent",
                            "mcp__skillvolution__*"]},
  "hooks": {
    "SessionStart": ["... hook session-start --client devin ..."],
    "UserPromptSubmit": ["... hook prompt --client devin ..."],
    "PostToolUse":  ["... hook tool-use ...",
                     "matcher": "^(write|edit|apply_patch|notebook_edit|mcp__skillvolution__.*)$"],
    "Stop":         ["... hook stop --client devin ..."],
    "SessionEnd":   ["... hook session-end ..."],
    "PermissionRequest": ["... hook approve ...",
                     "matcher": "^(run_subagent|read_subagent|mcp__skillvolution__.*)$"]
  }
}
```

- the three skills (`skills/<name>/SKILL.md`) and the `AGENTS.md` marker block.

**Project** (`<dir>`): `.devin/mcp_config.json`, `.devin/config.json`,
`.devin/skills/<name>/SKILL.md` (three), `AGENTS.md`.

**Reminder behavior** (flag-tracked): `PostToolUse` sets `worked` on
`write`/`edit`/`apply_patch`/`notebook_edit` and on the vault's `get_skill`, and
`reviewed` on `publish_skill`/`report_skill_outcome`; `UserPromptSubmit` injects the
matching skills;
`Stop` prints `{"decision":"block","reason":"..."}` while `worked` is set
after `reviewed`; `SessionEnd` drops the session row. `PermissionRequest`
→ `hook approve` returns
`{"decision":"approve","reason":"Skillvolution-managed tool"}` for
`run_subagent`, `read_subagent`, and every `mcp__skillvolution__*` tool —
including `publish_skill`, so the evaluator dispatch and the publish both
run unattended.

**Unverified**: the Devin hook payload shapes (including `UserPromptSubmit`'s
`prompt` field and `additionalContext` output, taken from Devin's lifecycle-hooks
docs) are exercised by tests but have not been verified against a live Devin session.

## Codex CLI

**Global** (`$CODEX_HOME`, default `~/.codex`):

- `config.toml`:

```toml
[mcp_servers.skillvolution]
command = "BIN"
args = ["--db", "DB", "serve"]
```

- `hooks.json` — same `{hooks: [{hooks: [...], matcher?}]}` group shape as
  Devin, with `SessionStart`, `UserPromptSubmit` (`hook prompt --client codex`),
  `PostToolUse`
  (matcher `^(apply_patch|mcp__skillvolution__.*)$`), `Stop`, `SessionEnd`,
  and `PermissionRequest` (matcher `^mcp__skillvolution__.*$` →
  `hook approve`).
- the three skills (`skills/<name>/SKILL.md`), `AGENTS.md` marker block.

**Project** (`<dir>`): `.codex/config.toml` (with a comment noting Codex
only loads project config after you accept its trust prompt),
`.codex/hooks.json`, `.codex/skills/<name>/SKILL.md` (three), `AGENTS.md`.

Codex has **no per-tool permission grant** — auto-approval runs through the
`PermissionRequest` hook instead. Work tools: `apply_patch` and `get_skill` (the
`PostToolUse` matcher covers every `mcp__skillvolution__*` tool). `prompt`, `tool-use`,
and `stop` pass `--client codex`; `UserPromptSubmit` reads `prompt` and its
`additionalContext` is added to the model's context.

**Unverified**: hook events and payload shapes not verified against a live
Codex session; MCP tool naming inside Codex (`mcp__skillvolution__*`) is
assumed from the matcher convention.

## Gemini CLI

**Global** (`$GEMINI_CLI_HOME/.gemini`, default `~/.gemini` — note
`GEMINI_CLI_HOME` substitutes for the home directory, not for `.gemini`
itself):

- `settings.json`:

```json
{
  "mcpServers": {"skillvolution": {"command": "BIN",
    "args": ["--db", "DB", "serve"], "trust": true}},
  "hooks": {
    "SessionStart": ["... hook session-start --client gemini ..."],
    "BeforeAgent":  ["... hook prompt --client gemini ..."],
    "AfterTool":    ["... hook tool-use --client gemini ...",
                     "matcher": "^(write_file|replace|mcp_skillvolution_.*)$"],
    "AfterAgent":   ["... hook stop --client gemini ..."],
    "SessionEnd":   ["... hook session-end ..."]
  }
}
```

`"trust": true` is set on the Skillvolution server only, and only when the
entry has no `trust` key — a `trust: false` you set survives reruns.

- the three skills (`skills/<name>/SKILL.md`, inside the `.gemini` dir) and the
  `GEMINI.md` marker block.

**Project** (`<dir>`): `.gemini/settings.json`, `.gemini/skills/<name>/SKILL.md`
(three), `GEMINI.md` at the root.

**Reminder behavior**: flag-tracked like Devin. Work tools:
`write_file`, `replace`, and `get_skill` (the `AfterTool` matcher covers
`mcp_skillvolution_.*`); `AfterAgent` is the stop event. `BeforeAgent` runs `hook prompt`
(input field `prompt`); its `additionalContext` is appended to the prompt for that turn only.

**Unverified**: hook events (`BeforeAgent`/`AfterTool`/`AfterAgent`) and the
`mcp_skillvolution_*` tool naming not verified against a live Gemini
session.

## Cursor

**Global** (`~/.cursor`): the three skills (`skills/<name>/SKILL.md`), `mcp.json`
(`mcpServers` entry, `"type": "stdio"`), `hooks.json`, and
`cli-config.json` permissions:

```json
{"permissions": {"allow": ["Mcp(skillvolution:*)"]}}
```

**Project** (`<dir>`): `.cursor/skills/<name>/SKILL.md` (three),
`.cursor/mcp.json`, `.cursor/hooks.json`, `.cursor/cli.json` (same
permission grant), and `.cursor/rules/skillvolution.mdc` — a rule file with
`alwaysApply: true` carrying the trigger instruction. There is no global
rules file, so globally the skills plus the SessionStart catalog are the only
trigger. The rule mentions `skill-acquisition` and `skill-evaluation` too.

`hooks.json` uses Cursor's flat shape (no group wrapper), with `"version": 1`
added only when the file doesn't already carry it:

```json
{"hooks": {
  "sessionStart": [{"command": "'BIN' --db 'DB' hook session-start --client cursor",
                    "timeout": 10}],
  "postToolUse":  [{"command": "... hook tool-use --client cursor", "timeout": 10,
                    "matcher": "^(Write|MCP:skillvolution_.*)$"}],
  "stop":         [{"command": "... hook stop --client cursor", "timeout": 10}],
  "sessionEnd":   [{"command": "... hook session-end", "timeout": 10}]}}
```

**Reminder behavior**: flag-tracked like Devin. Work tools: `Write` and `get_skill`
(the `postToolUse` matcher covers `MCP:skillvolution_.*`).

**No prompt hook**: unlike the other clients, Cursor gets no per-prompt suggestions.
Its `beforeSubmitPrompt` hook can only allow the prompt or block it
(`continue`/`user_message`) and has no field to add context for the model, so there is
nothing for `hook prompt` to return (`hook prompt --client cursor` is rejected).
Retrieval relies on the SessionStart catalog, the skills, and the rule file.

**Unverified**: hook events and the `postToolUse` matcher not verified
against a live Cursor session; the `MCP:skillvolution_*` tool naming and the
`Mcp(skillvolution:*)` permission string follow Cursor's documented
conventions.

## What `setup --remove` keeps

`--remove` deletes only entries Skillvolution owns (recognized by ownership
markers or command text) and never touches keys it didn't write — your other
`mcpServers`, hook entries, permissions, `hooks.json` `version`, and files
without our marker all survive. Managed files that exist solely because of
setup (the three skill files, plugin, rule file) are deleted, each only when it carries
its own marker (`skillvolution-managed:<name>:`) — a user's own file at one of those
paths is skipped with a `skipped … (not ours)` note, and the same file makes `setup`
itself refuse with an `unowned … skill conflict`. A `skills/<name>` directory left
empty is removed; one that still holds your files stays. Empty marker blocks (`skillvolution:start`
/`end`) are stripped from `AGENTS.md`/`GEMINI.md`, leaving your own content.

## Environment variables

| Variable | Effect |
|---|---|
| `SKILLVOLUTION_DB` | Default vault path when `--db` is omitted |
| `SKILLVOLUTION_NO_INPUT` | Non-empty → non-interactive setup: every detected client configured |
| `SKILLVOLUTION_NO_SETUP` | Installer: skip `setup` after install |
| `SKILLVOLUTION_VERSION` | Installer: version to download |
| `SKILLVOLUTION_INSTALL_DIR` | Installer: binary destination |
| `SKILLVOLUTION_NO_MODIFY_PATH` | Installer: don't touch `PATH` |
| `SKILLVOLUTION_CLAUDE_TIMEOUT_SECS` | Timeout for `claude mcp` calls (default 30) |
| `CLAUDE_CONFIG_DIR` | Claude global config dir (default `~/.claude`) |
| `XDG_CONFIG_HOME` | OpenCode global config root (absolute paths only) |
| `XDG_DATA_HOME` | Vault location root (absolute paths only) |
| `CODEX_HOME` | Codex global config dir (default `~/.codex`) |
| `GEMINI_CLI_HOME` | Substitutes for home when locating `~/.gemini` |
| `APPDATA` | Devin config dir base on Windows |
| `LOCALAPPDATA` | Vault location on Windows |
| `CLAUDE_PROJECT_DIR` | Fallback project-key source for hooks |
| `HOME`, `USERPROFILE`, `PATH` | Home/config resolution and client detection |

## Known unverified points

- Devin, Codex, Gemini and Cursor hook wiring is generated and
  unit-tested but has not been verified against live sessions of those
  clients. Claude Code's hook flow is verified live.
- Cursor: the `postToolUse` event name, its `matcher` support, and the
  `MCP:` tool-name prefix follow Cursor docs but are unverified live.
- Codex/Gemini/Cursor MCP tool naming (`mcp__skillvolution__*`,
  `mcp_skillvolution_*`, `MCP:skillvolution_*`) is assumed from each
  client's conventions.
- Codex has no static per-tool grant — approval relies on the
  `PermissionRequest` hook.
- Gemini `trust: true` is scoped to the Skillvolution server entry and is
  not re-asserted once you set `trust` yourself.
- `hook approve` auto-approves `publish_skill` for Devin (and any
  `mcp__skillvolution__*` tool under Codex's `PermissionRequest`) — this is
  what lets the review flow run without prompts.
