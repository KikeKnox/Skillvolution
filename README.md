# Skillvolution

Skillvolution is a shared procedural-memory vault for AI coding agents: a single SQLite
database exposed over MCP (stdio) to Claude Code, OpenCode, Devin CLI, Codex CLI, Gemini
CLI, and Cursor. Agents search and apply published skills, report whether they worked,
and publish new lessons once a fresh-context subagent has judged them worth keeping
(global or project scope — or discarded). Humans curate the result afterwards with
`deprecate`, `purge`, `export`/`import`, and `backup`.

## Install

Linux / macOS:

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://raw.githubusercontent.com/KikeKnox/Skillvolution/main/install.sh | sh
```

Windows (PowerShell):

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://raw.githubusercontent.com/KikeKnox/Skillvolution/main/install.ps1 | iex"
```

[`install.sh`](install.sh) / [`install.ps1`](install.ps1) run the release's cargo-dist
installer, which verifies each archive's SHA-256 and puts the binary in `~/.local/bin`
(adding it to `PATH`; skip with `SKILLVOLUTION_NO_MODIFY_PATH=1`, or relocate with
`SKILLVOLUTION_INSTALL_DIR`). Then they run `skillvolution setup` (skip with
`SKILLVOLUTION_NO_SETUP=1`), which detects installed AI clients and asks which to
configure — the questions are skipped when there is no terminal or when
`SKILLVOLUTION_NO_INPUT` is set, in which case every detected client is configured.
Pin a version with `SKILLVOLUTION_VERSION=0.3.0`. Rerunning the same command updates the
binary; setup is idempotent. To install without configuring anything, run the release
installer directly:
`https://github.com/KikeKnox/Skillvolution/releases/latest/download/skillvolution-installer.sh`
(`.ps1` on Windows).

**Build from source** (requires Rust ≥ 1.88):

```bash
cargo install --locked --path .
```

Prebuilt binaries are published for Linux x86_64/aarch64 (gnu and musl), macOS
x86_64/aarch64, and Windows x86_64 (MSVC); releases are gated on CI.

## Quick start

```bash
skillvolution setup     # detect installed clients, configure each one for your user
skillvolution doctor    # verify the vault and everything setup wrote
```

`setup` writes the `evolution` skill, the MCP server entry, and the hooks or plugin each
client uses, plus the permission grants the review flow needs to run unattended. The
vault database is created automatically on first use — there is no init step.
`doctor` exits 0 when everything is healthy and prints what to fix when it is not.

## Supported clients

| Client | `--client` token | What global `setup` writes | Review reminder | Notes |
|---|---|---|---|---|
| Claude Code | `claude-code` | `skills/evolution/SKILL.md`, `SessionStart`/`Stop` hooks + `permissions.allow` (`Agent`, `Task`, `mcp__skillvolution`) in `settings.json`; MCP registered via `claude mcp add-json --scope user` | `hook stop` scans the session transcript for file edits (`Edit`, `Write`, `MultiEdit`, `NotebookEdit`) after the last successful review call and returns `additionalContext` JSON, which makes Claude continue the turn | Blocking verified live (Claude Code 2.1.283). If `claude` isn't on `PATH`, setup prints the `mcp add-json` command to run manually |
| OpenCode | `opencode` | `mcp.skillvolution` entry + `permission` allows (`task`, each `skillvolution_*` tool) in `opencode.json`/`opencode.jsonc`, `AGENTS.md` marker block, `skills/evolution/SKILL.md`, `plugins/skillvolution.js` | The plugin injects the catalog into the system prompt and, after a turn that edited files goes idle without a review, adds a reminder to later turns' system prompts | Never blocks a turn; no extra model turn is spent |
| Devin CLI | `devin` | `mcp_config.json` server entry, `config.json` (`permissions.allow`: `run_subagent`, `read_subagent`, `mcp__skillvolution__*`, plus `SessionStart`/`PostToolUse`/`Stop`/`SessionEnd`/`PermissionRequest` hooks), `AGENTS.md` marker block, skill | `hook tool-use`/`hook stop --client devin` keep per-session worked/reviewed flags; a stop after unreviewed work prints `{"decision":"block"}` | `hook approve` auto-approves `run_subagent`/`read_subagent` and every `mcp__skillvolution__*` tool — including `publish_skill`; see the security note in the FAQ |
| Codex CLI | `codex` | `[mcp_servers.skillvolution]` in `config.toml`, `hooks.json` (SessionStart, PostToolUse on `apply_patch`, Stop, SessionEnd, PermissionRequest on `mcp__skillvolution__*`), `AGENTS.md`, skill | Same flag tracking as Devin; `{"decision":"block"}` on Stop | Project-mode `.codex/config.toml` only loads once the project is trusted in Codex; whether tools run without prompting is governed by your `approval_policy` |
| Gemini CLI | `gemini` | `mcpServers.skillvolution` (with `trust: true` unless already set) and `SessionStart`/`AfterTool`/`AfterAgent`/`SessionEnd` hooks in `settings.json`, `GEMINI.md` marker block, skill | Same flag tracking; AfterAgent (Gemini's Stop) prints `{"decision":"deny"}` | MCP tools are `mcp_skillvolution_*`; `trust` is the documented way to skip tool-confirmation prompts for one server |
| Cursor | `cursor` | `mcpServers.skillvolution` in `mcp.json`, `hooks.json` (sessionStart, postToolUse on `Write`, stop, sessionEnd), `permissions.allow` `Mcp(skillvolution:*)` in `cli-config.json`, skill | Same flag tracking; `stop` prints `{"followup_message": ...}` | Cursor has no global rules file — globally the skill plus the SessionStart catalog is the only trigger; project setups also write `.cursor/rules/skillvolution.mdc` |

Setup detects a client by its CLI on `PATH` (`claude`, `opencode`, `devin`, `codex`,
`gemini`, `cursor-agent`/`cursor`) or an existing config directory. For the exact files
and directories, see [docs/configuration.md](docs/configuration.md); for how the hooks
and the plugin behave, see [docs/integrations.md](docs/integrations.md) and the
[FAQ](#faq) below.

## Setup

```bash
skillvolution setup                          # global: configure detected clients for your user
skillvolution setup --client claude-code,codex
skillvolution setup --client all             # every client, detected or not
skillvolution setup --project DIR [--project-key KEY] [--client ...]
skillvolution setup --dry-run                # unified diff of every file, nothing written
skillvolution setup --remove                 # undo (see Uninstall)
```

**Global (default)** configures the current user once, shared by every project. Without
`--client` it detects installed clients and asks which to configure when a terminal is
attached (`y`/`yes`/blank/EOF = yes, `n`/`no` = no, anything else asks once more then
counts as yes); with no terminal or `SKILLVOLUTION_NO_INPUT` set, every detected client
is configured. If no client is detected nothing is configured — the vault is still
provisioned. Right after the client questions, when neither `--db` nor
`SKILLVOLUTION_DB` picked a path, a terminal also gets `Vault database path [<default>]:`
(blank accepts the default; a leading `~/` expands against your home directory). Use `--client` (comma-separated; `both` means
`claude-code` + `opencode` for backwards compatibility) to skip the questions entirely,
and `--bin` to record a binary path other than the running executable.

**Project** (`--project DIR`) writes the same pieces into the project's own files —
`.claude/`, `.opencode/`, `.devin/`, `.codex/`, `.gemini/`, `.cursor/` plus the root-level
`AGENTS.md`/`GEMINI.md`/`opencode.json`/`.mcp.json` — and bakes `--project <key>` into the
MCP server entry (and the OpenCode plugin), where `<key>` defaults to `DIR`'s sanitized
directory name; override with `--project-key`. Note the key defaults to `DIR`'s own name,
while `serve`'s runtime detection uses the enclosing git repository's root name — the
two differ when `DIR` is a subdirectory of a larger repo. Project setup refuses a
project-level `opencode.jsonc` (merge it into strict `opencode.json` first). `--client`
defaults to `all` in this mode.

Writes are atomic (temp file + rename) with rollback if a later write fails; a changed
file's previous content is kept as `.skillvolution.bak`, `.bak.1`, `.bak.2` (three
slots), except for fully managed files (the skill, the plugin, the Cursor rule), which
get none. Existing entries are preserved: global setup merges and preserves user keys
(`env`, `enabled: false`, timeouts) when updating an MCP entry, while project setup
replaces the `skillvolution` MCP entry whole, discarding any pre-seeded `env`/`cwd`/`url`
from a possibly untrusted repo. Permission grants only fill in keys you haven't chosen. Hook commands are
recognized by content, so a rerun replaces them instead of duplicating — and stale
entries an older version wrote under events it no longer uses are stripped. `setup`
refuses to write through a symlinked config file and refuses a `SKILL.md`, plugin, or
`.mdc` rule file that doesn't carry its managed marker. If the current directory's git
repo still carries a project-level install (the only kind 0.1.x had) next to the global
one, hooks would run twice there: global `setup` warns on stderr when it finds Claude
Code's entries (`.claude/settings.local.json`, `.mcp.json`), and `doctor` reports it as
a `legacy install` warning for every globally configured client.

Rerun `setup` after installing a new AI client, and after upgrading the binary — it
refreshes the skill, hooks, and plugin; `doctor` flags installs that differ from what
the current binary would write.

## Where the vault lives

One database, shared by every client and project. Default path:

| Platform | Path |
|---|---|
| Linux / macOS | `$XDG_DATA_HOME/skillvolution/skills.db`, else `~/.local/share/skillvolution/skills.db` (`$USERPROFILE` when `HOME` is unset) |
| Windows | `%LOCALAPPDATA%\skillvolution\skills.db`; if `LOCALAPPDATA` is unset, the same `XDG_DATA_HOME`/`HOME` logic as Linux/macOS; if that `LOCALAPPDATA` path doesn't exist yet but a legacy `%USERPROFILE%\.local\share\skillvolution\skills.db` vault does, the legacy vault keeps being used |

`--db PATH` overrides per invocation and `SKILLVOLUTION_DB` overrides everywhere; the
path is embedded in every client config `setup` writes. The database is created (and
migrated) on first open. `XDG_DATA_HOME`/`XDG_CONFIG_HOME` are honored only when set to
absolute paths.

Keep the vault on a local disk: on Linux, `setup` and `relocate` print a warning when
the target sits on a network filesystem (`nfs`, `cifs`, `sshfs`, etc.) — SQLite WAL
mode is unsafe there under concurrent access. The check is advisory only and doesn't
run on macOS/Windows.

Move the vault later with `skillvolution relocate NEW-PATH`: it copies the database
(`VACUUM INTO`, verified against the source), repoints every global client config that
referenced the old path (re-registering Claude Code's MCP server), and renames the old
file to `PATH.relocated.bak`. Project-level installs are untouched — rerun
`setup --project DIR` for those.

## Curation

Publication is automatic — a fresh subagent evaluates each candidate and kept lessons go
live immediately — so humans review afterwards:

```bash
skillvolution show ID [--version N] [--json]   # status, evidence, diff vs. base
skillvolution outcomes [--json]                # helped/failed/not_applicable per current version
skillvolution outcomes ID [--json]             # one skill's outcome log
skillvolution outcomes --failing [--json]      # only skills whose current version has failures

skillvolution deprecate ID     # hide from search, keep history
skillvolution undeprecate ID

skillvolution purge ID [--version N]           # permanently delete a skill or one revision,
                                               #   with its outcomes (secure-delete, VACUUM, WAL
                                               #   checkpoint); warns if it can't compact while
                                               #   another connection holds the vault open
skillvolution export [-o FILE]                 # whole vault as JSON (stdout by default)
skillvolution import FILE                      # merge an export back (atomic; conflicts abort)
skillvolution backup PATH                      # consistent copy of the live database
```

`import` merges rather than replaces: existing identical revisions are skipped, a
conflicting one aborts the whole import, and a skill that exists under a different scope
is refused. `export` omits per-session hook state; `backup` uses `VACUUM INTO` and needs
the destination not to exist.

## Uninstall

```bash
skillvolution setup --remove                 # from every configured client
skillvolution setup --remove --client codex  # just one
skillvolution setup --project DIR --remove   # a project-level install
```

It removes the managed skill file, hooks, MCP entries, plugin, rule file, and the
Skillvolution-specific permission grants, and unregisters Claude Code's user-scope MCP
server via `claude mcp remove`. It deliberately keeps:

- generic permission grants it also wrote but you might use independently —
  `Agent`/`Task` (Claude Code) and `run_subagent`/`read_subagent` (Devin CLI), reported
  as `note: … still allows …` lines;
- the vault database and `.skillvolution.bak*` backups;
- the `skillvolution` binary — remove it yourself (e.g. `rm ~/.local/bin/skillvolution`).

## Upgrading

Rerun the install command (or `cargo install --locked --path .`), then rerun
`skillvolution setup` — the registered configs keep pointing at the same binary path,
but the skill file, hooks, and plugin are only refreshed when setup runs. `doctor`
reports an install as "differs from what `skillvolution setup --client X` would write"
when it's stale. The database schema migrates automatically on first open; a vault
written by a newer version refuses to open with "upgrade skillvolution".

## FAQ

**Claude Code printed a "Skillvolution review" message (shown as Stop hook feedback) —
is something broken?** No — that's the review reminder working. `hook stop` scans the
session transcript since the offset recorded for that session; when the last file-edit
tool call (`Edit`, `Write`, `MultiEdit`, `NotebookEdit` — `Bash` doesn't count) comes
after the last *successful* `publish_skill`/`report_skill_outcome` call (a rejected one
doesn't count), it prints
`{"hookSpecificOutput":{"hookEventName":"Stop","additionalContext": …}}` on stdout and
exits 0. Claude Code continues the same turn with that text; the continued Stop arrives
with `stop_hook_active: true` and is never blocked again, and each transcript span is
judged at most once. This shows in the transcript as ordinary "Stop hook feedback", not
a "Stop hook error". Verified live against Claude Code 2.1.283.

**How does the reminder work in Devin CLI, Codex CLI, Gemini CLI, and Cursor?** Their
hook payloads carry a session id but no transcript to scan, so `hook tool-use` keeps
per-session worked/reviewed flags in the vault instead (the `PostToolUse`/`AfterTool`/
`postToolUse` hooks match each client's file-editing tools — `write`/`edit`/
`apply_patch`/`notebook_edit`, `apply_patch`, `write_file`/`replace`, `Write`
respectively — plus `publish_skill`/`report_skill_outcome`, which count as a review;
other tools are ignored). The stop hook
(`Stop`/`AfterAgent`/`stop`) then returns its own continue format once per unreviewed
span: `{"decision":"block","reason":…}` for Devin and Codex, `{"decision":"deny",…}` for
Gemini CLI, `{"followup_message":…}` for Cursor. Every judged stop consumes the span —
work after a review re-arms the reminder, but a stop after the agent simply ignored it
isn't blocked again — and the session-end hook drops the flags.

**Devin auto-approves vault tools — what does that cover?** The `PermissionRequest`
hook (`hook approve`) prints `{"decision":"approve","reason":"Skillvolution-managed tool"}`
for `run_subagent`, `read_subagent`, and every `mcp__skillvolution__*` tool —
**including `publish_skill`**: anything an evaluator subagent decides to keep goes live
without a human prompt. That auto-approval is what the evaluator dispatch needs; if
you'd rather approve publishes manually, remove the `PermissionRequest` hook from
`config.json` (it will return on the next `setup` run) or uninstall. Codex wires the
same hook but only approves `mcp__skillvolution__*` tools.

**What does the OpenCode plugin do?** `plugins/skillvolution.js` runs
`hook session-start` to inject the skill catalog into the system prompt once per session
(retrying on the next turn if the load failed), and tracks file-edit tools
(`edit`, `write`, `multiedit`, `patch`, `apply_patch` — `bash` doesn't count) on the
top-level session — subagent work is attributed to its parent, and a subagent's own idle
events are ignored. Once a working turn goes idle without a review call, later turns'
system prompts carry the review reminder until `publish_skill` or
`report_skill_outcome` runs. It never blocks a turn and spends no extra model call.

**The reminder never fired, or fired twice — why?** Usually a missing or doubled
install: run `skillvolution doctor`, which flags configs differing from the current
binary's output and warns when a repo still carries a project-level install alongside
the global one. See [docs/troubleshooting.md](docs/troubleshooting.md).

**A `publish_skill` call was rejected.** The server validates id/tag shape, the four
required `content` headings, credential-shaped values, `verdict`/`scope` agreement, and
that `expected_version` still matches the current published version. The rejection
message names the specific check — see
["`publish_skill` is rejected"](docs/troubleshooting.md#publish_skill-is-rejected) for
each one.

## Limitations

- Skill text and evidence are authored by agents and are not independently verified;
  the evaluator judges them from the text alone. Deprecate publications whose evidence
  looks invented.
- Secret scanning rejects credential-shaped values (AWS/GitHub/OpenAI/Google-style key
  prefixes, private-key blocks, full JWTs, bearer tokens, credential URLs, Slack
  webhooks) in `description`, `content`, `evidence`, `verdict_reason`, and outcome
  notes — enforced, but still shape-based, so treat it as a tripwire, not a guarantee.
- The project key is the sanitized name of the enclosing git repository's root
  directory, so two unrelated repositories that share a directory name share project
  scope. Outside a git repository only global skills are visible.
- Only Claude Code's hook flow (catalog injection and the blocking review reminder) has
  been verified against a live client. Devin CLI, Codex CLI, Gemini CLI, Cursor, and the
  OpenCode plugin are exercised by tests and produce the documented payloads, but their
  hooks firing end-to-end inside the real clients is not verified. On Windows, Devin's
  hook invocation specifically is unverified.
- The vault trusts whatever the MCP client sends; there is no authentication, which is
  fine for a local stdio server but means any process that can run the binary can write
  to the vault.

## Documentation

- [docs/cli.md](docs/cli.md) — every command, flag, exit code, and the MCP tools
- [docs/configuration.md](docs/configuration.md) — environment variables and file locations
- [docs/troubleshooting.md](docs/troubleshooting.md) — symptom → cause → fix
- [docs/architecture.md](docs/architecture.md) — modules, data model, ranking
- [docs/integrations.md](docs/integrations.md) — exactly what `setup` writes per client
- [CHANGELOG.md](CHANGELOG.md) — release notes
- [CONTRIBUTING.md](CONTRIBUTING.md) — development and releasing

## Contributing and releases

See [CONTRIBUTING.md](CONTRIBUTING.md) for building, testing, adding a client, and the
release process, and [CHANGELOG.md](CHANGELOG.md) for what changed in each version.

## License

[MIT](LICENSE)
