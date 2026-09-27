# Configuration

Skillvolution has no config file of its own. Everything it needs is the vault database
path plus what `setup` writes into each client's own configuration. This page lists the
environment variables it reads, the file locations per client, the default database
path, and how the project key is derived.

## Environment variables

Variables the `skillvolution` binary reads:

| Variable | Used by | Effect | Default |
|---|---|---|---|
| `SKILLVOLUTION_DB` | every command | Vault database path. `--db` wins over it; a relative value resolves against the current directory. | platform data dir (below) |
| `SKILLVOLUTION_NO_INPUT` | `setup` | Any non-empty value forces the non-interactive path: every detected client is configured and the default database path is kept, with no prompts. | unset (prompts on a terminal) |
| `SKILLVOLUTION_CLAUDE_TIMEOUT_SECS` | `setup`, `doctor` | Seconds a `claude mcp …` call may run before being killed. | `30` |
| `CLAUDE_CONFIG_DIR` | `setup`, `doctor` | Claude Code's config directory. | `~/.claude` |
| `CLAUDE_PROJECT_DIR` | `serve`, `hook session-start` | Fallback project root for project-key detection when the working directory isn't inside a git repo. Set by Claude Code. | unset |
| `CODEX_HOME` | `setup`, `doctor` | Codex CLI's config directory. | `~/.codex` |
| `GEMINI_CLI_HOME` | `setup`, `doctor` | Substitutes for the *home* directory `~/.gemini` is computed from — the config dir is `<value>/.gemini`. | `HOME` |
| `XDG_CONFIG_HOME` | `setup`, `doctor` | OpenCode's config base — honored only when set to an absolute path. | `~/.config/opencode` |
| `XDG_DATA_HOME` | every command | Vault base directory — honored only when set to an absolute path. | `~/.local/share` |
| `HOME` | every command | Home directory. | — |
| `USERPROFILE` | every command | Home-directory fallback when `HOME` is unset (Windows). | — |
| `APPDATA` | `setup`, `doctor` | Devin CLI's config base on Windows (`%APPDATA%\devin`). | — |
| `LOCALAPPDATA` | every command | Vault base directory on Windows (`%LOCALAPPDATA%\skillvolution`). | falls back to `XDG_DATA_HOME`/`HOME` |
| `PATH` | `setup`, `doctor` | Client detection (each client's CLI) and resolving the `claude` CLI for MCP registration. | — |

Variables the installers read (they are not read by the binary itself):

| Variable | Used by | Effect | Default |
|---|---|---|---|
| `SKILLVOLUTION_VERSION` | `install.sh`, `install.ps1` | Release version to install (`0.3.0` or `v0.3.0`). | latest release |
| `SKILLVOLUTION_NO_SETUP` | `install.sh`, `install.ps1` | Set to `1` to skip running `skillvolution setup` after the install. | setup runs |
| `SKILLVOLUTION_NO_INPUT` | `install.sh`, `install.ps1` | Passed through to `setup`: configure every detected client without prompting. | prompts when a terminal exists |
| `SKILLVOLUTION_INSTALL_DIR` | `install.sh`, `install.ps1` | Directory the binary is expected in (passed to the release installer). | `~/.local/bin` |
| `SKILLVOLUTION_NO_MODIFY_PATH` | `install.sh` | Set to `1` to skip adding the install dir to `PATH` (handled by the release's cargo-dist installer). | PATH is updated |

## File locations

`setup` writes into each client's own config directory (global mode) or into the project
directory (`--project DIR`, project mode). Every client also gets the `evolution` skill
at `<dir>/skills/evolution/SKILL.md`.

| Client | Global config directory | Global files | Project files (under `DIR`) |
|---|---|---|---|
| Claude Code | `$CLAUDE_CONFIG_DIR`, else `~/.claude` | `settings.json` (hooks + `permissions.allow`); MCP registered via `claude mcp add-json --scope user` (not a file) | `.claude/settings.local.json`, `.mcp.json`; also removes the legacy `CLAUDE.md` marker block |
| OpenCode | `$XDG_CONFIG_HOME/opencode` when absolute, else `~/.config/opencode` | `opencode.json` (or an existing `opencode.jsonc`), `AGENTS.md`, `plugins/skillvolution.js` | `opencode.json` and `AGENTS.md` at the root, `.opencode/plugins/skillvolution.js` (`opencode.jsonc` refused) |
| Devin CLI | `~/.config/devin` on Linux/macOS, `%APPDATA%\devin` on Windows | `mcp_config.json`, `config.json` (permissions + hooks), `AGENTS.md` | `.devin/mcp_config.json`, `.devin/config.json`, `AGENTS.md` at the root |
| Codex CLI | `$CODEX_HOME`, else `~/.codex` | `config.toml` (`[mcp_servers.skillvolution]`), `hooks.json`, `AGENTS.md` | `.codex/config.toml`, `.codex/hooks.json`, `AGENTS.md` at the root |
| Gemini CLI | `$GEMINI_CLI_HOME/.gemini`, else `~/.gemini` | `settings.json` (MCP entry + hooks), `GEMINI.md` | `.gemini/settings.json`, `GEMINI.md` at the root |
| Cursor | `~/.cursor` | `mcp.json`, `hooks.json`, `cli-config.json` (`permissions.allow`) | `.cursor/mcp.json`, `.cursor/hooks.json`, `.cursor/cli.json`, `.cursor/rules/skillvolution.mdc` |

On Windows, `~` resolves through `USERPROFILE` when `HOME` is unset.

Notes:

- The managed skill file lives at `<config dir>/skills/evolution/SKILL.md` globally and
  `.<dir>/skills/evolution/SKILL.md` in a project (e.g. `.claude/skills/…`,
  `.codex/skills/…`).
- `AGENTS.md` carries the same marker block for OpenCode, Devin, and Codex — shared, so
  it's written once per scope however many of those clients are configured. Gemini CLI
  uses `GEMINI.md`, Cursor uses a `.cursor/rules/skillvolution.mdc` rule file (project
  mode only — Cursor has no global rules file, so globally the skill plus the
  SessionStart catalog is the only trigger).
- MCP server entries all run `<bin> --db <db> serve` plus `--project <key>` in project
  mode (no global entry passes `--project`). They differ only in container:
  `.mcp.json`/Cursor `mcp.json`/Devin `mcp_config.json` use `mcpServers.skillvolution`;
  OpenCode uses `mcp.skillvolution` with `"type": "local"`; Codex uses
  `[mcp_servers.skillvolution]` in TOML. In project mode `--project <key>` is also baked
  into the OpenCode plugin and Claude Code's `SessionStart` hook (its `Stop` hook doesn't
  need it — the transcript itself locates the session).
- Setup's backups sit next to each edited file as `NAME.skillvolution.bak`,
  `.bak.1`, `.bak.2` (three slots, newest first). Fully managed files get no backup.

## Vault database path

`--db PATH` > `SKILLVOLUTION_DB` > the platform default:

| Platform | Default |
|---|---|
| Linux / macOS | `$XDG_DATA_HOME/skillvolution/skills.db` when `XDG_DATA_HOME` is an absolute, non-empty path; otherwise `~/.local/share/skillvolution/skills.db` (`USERPROFILE` standing in for `HOME` when unset) |
| Windows | `%LOCALAPPDATA%\skillvolution\skills.db`; if `LOCALAPPDATA` is unset, the same `XDG_DATA_HOME`/`HOME` logic as Linux/macOS; if that `LOCALAPPDATA` path doesn't exist yet but a legacy `%USERPROFILE%\.local\share\skillvolution\skills.db` vault does, the legacy vault keeps being used |

The database and its parent directory are created on first open; the schema migrates
automatically. A database written by a newer version refuses to open. See
[README](../README.md#where-the-vault-lives) for `relocate` and the network-filesystem
warning.

## Project key

`serve`, `hook session-start`, and `publish_skill`'s `scope: "project"` resolve the
project key as follows:

1. An explicit `--project KEY` wins.
2. Otherwise the sanitized name of the enclosing git repository's root directory —
   detected from the process's working directory, falling back to `CLAUDE_PROJECT_DIR`
   when the working directory isn't inside a repo. A git worktree resolves to the main
   repository's name; a submodule resolves to its own directory name.
3. Outside a git repository there is no key: only global skills are visible and
   project-scoped publishes are refused.

Sanitization keeps lowercase ASCII letters and digits, collapses every other run of
characters into a single `-`, trims leading/trailing hyphens, and truncates to 64 bytes;
a name that leaves nothing has no key (`__café__` → `caf`).

Two consequences worth knowing: `setup --project DIR` defaults the key to `DIR`'s own
sanitized name, which differs from runtime detection when `DIR` is a subdirectory of a
larger repo; and two unrelated repositories that share a directory name share a project
scope.
