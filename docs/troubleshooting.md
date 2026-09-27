# Troubleshooting

Start with the health check:

```bash
skillvolution doctor          # human-readable report
skillvolution doctor --json   # {"healthy": bool, "checks": [...]}
```

`doctor` is read-only. It verifies the database (`application_id`, `user_version`,
`PRAGMA quick_check`, search index, row counts) and compares every detected client's
on-disk config against what the current binary's `setup` would write — flagging stale
skill files, differing configs, a configured binary path that no longer exists, a
missing Claude Code MCP registration, and a project-level install left in the current
repo next to the global one. It exits `0` when nothing fails (warnings are fine) and
`1` when a check fails.

## The `skillvolution` MCP server isn't visible in my client

- Confirm `setup` configured that client: `skillvolution doctor` shows
  `installed but not configured` vs `configured, up to date` vs `differs from what
  … would write`. Run (or rerun) `skillvolution setup --client <name>` and restart the
  client.
- Claude Code global setup registers the server through the `claude` CLI. If `claude`
  wasn't on `PATH`, setup printed the manual command — run it:
  `claude mcp add-json --scope user skillvolution '{"type":"stdio","command":"<bin>","args":["--db","<db>","serve"]}'`.
  `doctor` reports the registration as missing until `claude mcp get skillvolution`
  succeeds.
- Codex loads a project's `.codex/config.toml` only after the project is *trusted* —
  accept Codex's trust prompt the first time you run `codex` in that directory.
- Check the server starts at all: run `<bin> --db <db> serve` with the same paths the
  client's config records and look at stderr (see below). A client restarting a stale
  config is also covered by doctor's `binary does not exist` check.

## `skillvolution: project = …` on `serve`'s stderr

That's the resolved project key the server will use for project-scoped skills and
outcome records, printed once at startup. `none` means the process wasn't started
inside a git repository and `--project` wasn't given — only global skills are visible,
and `scope: "project"` publishes are refused. If the printed key isn't your repo's name,
the working directory the client launches the server in isn't what you expect, or the
sanitized directory name differs (see
[configuration.md](configuration.md#project-key)).

## A project's skills don't show up in a session

- The session's project key must equal the skill's `scope`. `serve` resolves the key
  from the working directory's git root (falling back to `CLAUDE_PROJECT_DIR`);
  `setup --project DIR` instead defaults it to `DIR`'s own directory name — those
  differ when `DIR` is a subdirectory of a larger repo. Check the `project =` line on
  stderr and republish under the right scope, or rerun `setup --project DIR
  --project-key <key>`.
- Outside a git repository there is no project scope at all — publish globally or move
  the work inside a repo.
- A deprecated skill is still returned by `get_skill` but excluded from `search_skills`
  and the session catalog; check `skillvolution show ID`.

## The review reminder never fires

- Run `skillvolution doctor` — a hook config that differs from the current binary's
  output, or a missing configured binary, is the usual cause.
- For Claude Code the Stop hook only blocks when the transcript contains an
  `Edit`/`Write`/`MultiEdit`/`NotebookEdit` call after the last *successful*
  `publish_skill`/`report_skill_outcome` — `Bash` and other tools don't count as work,
  and a review call that returned an error doesn't count as a review.
- For the flag-tracked clients the `PostToolUse`/`AfterTool`/`postToolUse` hook must
  have matched a file-editing tool — `write`/`edit`/`apply_patch`/`notebook_edit`
  (Devin), `apply_patch` (Codex), `write_file`/`replace` (Gemini CLI), `Write` (Cursor).
  If your client names the tool something else, no work is flagged.
- OpenCode never blocks: the reminder is added to the *system prompt* of turns after a
  working turn goes idle, so it's invisible until the next turn.

## The reminder fires twice in one repository

A project-level install (the only kind 0.1.x had) next to the global one runs the hooks
twice. Two places warn about it: global `setup` prints a stderr warning when the current
repo's `.claude/settings.local.json` or `.mcp.json` still holds Skillvolution entries
(`warning: … already configures Skillvolution for this repo (project-level setup); with
the global setup too, it would run twice here`), and `doctor` — run inside the affected
repo — reports it for every globally configured client as
`legacy install: project-level install found (…) alongside the global one; hooks will run twice`.
Remove the project-level install with `skillvolution setup --project DIR --remove`.

## `publish_skill` is rejected

The rejection message names the check. In order of likelihood:

- **id/tag format** — `id` and each tag must be lowercase letters or digits in
  hyphen-separated groups (`^[a-z0-9]+(-[a-z0-9]+)*$`); `id` ≤ 64 bytes, ≤ 8 tags of
  ≤ 32 bytes each. `node.js`, `SQLite`, `rust_sqlite` are all rejected — write
  `nodejs`, `sqlite`, `rust-sqlite`.
- **headings** — `content` must contain `## When to use`, `## Procedure`,
  `## Pitfalls`, `## Verification`, each on its own line at `## ` level, in that order
  (matched case-insensitively). Preamble and extra `##` sections are allowed.
- **credentials** — `description`, `content`, `evidence`, and `verdict_reason` are
  scanned for credential shapes (AWS/GitHub/OpenAI/Google-style key prefixes, private
  key blocks, full JWTs, bearer tokens, `user:password@host` URLs, Slack webhook URLs).
  Replace the value with `<token>` or `$ENV_VAR`.
- **verdict/scope** — `verdict` must be exactly `keep global` or `keep project`
  (compared trimmed and lowercased — pass the evaluator's line verbatim) and must match
  `scope`: `keep project` needs `scope: "project"`, `keep global` needs `global` or no
  `scope`. A `discard` verdict must not be published at all.
- **scope is fixed per id** — `skill ID already exists with scope …; choose a different
  id`. Republishing under a different scope isn't allowed; use a new id.
- **no project key** — `scope: "project"` on a server started outside a git repo fails
  with `this server has no project key …`; publish globally instead.
- **stale base** — `stale base: expected N, current published version is M` means
  someone published first: `get_skill` the new version, merge your lesson into it, and
  evaluate the merged candidate again with `expected_version` set to `M`.
- **proven base** — `version N of ID is proven (helped H, failed F) …`: merge the
  base's content rather than rewriting it and set `replaces_proven: true`, or publish
  the lesson under a new id.

Outcome notes are scanned for the same credential shapes, and `report_skill_outcome`
fails with `no published revision ID version N visible to this project` when the
revision isn't published or isn't visible to the server's project.

## `database is locked` / `database is busy`

The vault uses WAL mode with a 5-second busy timeout, and open-time migration retries
busy/locked errors for a bounded window — but a heavily contended vault (many clients
running hooks at once) can still surface a lock error. Wait and retry; if it's
persistent, check that the vault isn't on a network filesystem (below) and that no
other process holds a long write transaction (`doctor`'s integrity check will still
read it fine).

## `warning: SQLite WAL mode is not safe on network filesystems (fstype)`

`setup` and `relocate` print this (Linux only) when the vault path sits on NFS, CIFS/
SMB, sshfs, 9p, ceph, or similar. WAL mode relies on file locking and shared memory
that network filesystems don't provide reliably under concurrent access — keep the
vault on a local disk. The warning is advisory; it doesn't block the write. If this is
your actual database path, move it with `skillvolution relocate <local-path>`.

## `unsupported database schema version …; upgrade skillvolution`

The vault's `user_version` is newer than this build's schema — the database was written
by a newer Skillvolution. Upgrade the binary; never downgrade against a newer vault.
The sibling errors `not a skillvolution database (application_id …)` and `… (it already
holds other tables)` mean the path points at a foreign database — choose another path.
A pre-1 database is refused outright (`move it aside and create a new one`).

## `opencode.jsonc is unsupported` in project mode

Project setup only writes strict `opencode.json` — a project-level `opencode.jsonc` is
refused with `merge it into strict opencode.json manually first`. Global setup does use
an existing `opencode.jsonc`; trailing commas in it are accepted, but comments fail
with `… could not be parsed as strict JSON; add the mcp.skillvolution entry manually,
or remove the comments and rerun setup`. Both `opencode.json` and `opencode.jsonc` existing at once is refused in global
mode too.

## `claude` CLI wasn't on `PATH` during setup

Global Claude Code setup writes the skill/settings files regardless, but the
user-scope MCP registration goes through `claude mcp add-json`, so setup prints:

```
claude CLI not found on PATH; register the MCP server manually: claude mcp add-json --scope user skillvolution '{…}'
```

Run that command once `claude` is installed (or on PATH). `setup --remove` prints the
matching `claude mcp remove --scope user skillvolution` when it can't reach the CLI.
`doctor` also warns when the registration is missing.

## Where are my backups?

Two kinds, both beside the file they protect:

- `NAME.skillvolution.bak`, `.bak.1`, `.bak.2` — the previous content of each file
  `setup` modified (three rotating slots, newest first). Fully managed files (the
  skill, the OpenCode plugin, the Cursor rule) get none. On Unix a backup keeps the
  original file's mode.
- `PATH.relocated.bak` — the whole old vault after `relocate` moves it (the old path
  renamed aside so a stale config can't silently reopen it). Any `-wal` and `-shm`
  sidecars are renamed to `.relocated.bak-wal` and `.relocated.bak-shm` so they're found
  if the backup is opened later; `doctor` reports this with a `vault: relocated` warning
  and suggests setting `SKILLVOLUTION_DB` to point at the new vault.

For your own snapshots use `skillvolution backup PATH` (a `VACUUM INTO` copy safe on a
live vault) or `skillvolution export` (a portable JSON document you can `import`
elsewhere). Backups and the database are deliberately left behind by
`setup --remove`.

## The hook ran but did nothing

All `hook` subcommands fail open: an error prints `skillvolution hook: …` on stderr and
the process still exits 0, so a broken vault or payload never blocks the client. If a
reminder or catalog silently didn't appear, run the hook command yourself with a
representative payload — e.g. `echo '{"session_id":"x"}' | skillvolution hook stop
--client devin` — and read stderr.
