# Architecture

Skillvolution is a single Rust binary (`skillvolution`) that stores reusable
"skills" (verified lessons) in a SQLite vault, serves them to agent clients
over MCP (JSON-RPC on stdio), and wires each client up via `setup` and
client hooks.

## Module map

### CLI / entry

| Module | Role |
|---|---|
| `skill_file.rs` | SKILL.md / AGENT.md frontmatter parser and YAML scalar quoting |
| `agents.rs` | Built-in workers, per-client rendering, marker/hash ownership, `agents add/remove/sync/import` |
| `main.rs` | Arg parsing, command dispatch, `hook` subcommands, project detection |
| `lib.rs` | Re-exports the modules so integration tests and the binary share them |

### MCP

| Module | Role |
|---|---|
| `mcp.rs` | JSON-RPC server loop on stdio; `initialize`, `tools/list`, `tools/call`; tool argument validation |

### Hooks

| Module | Role |
|---|---|
| `hook.rs` | `session-start`, `prompt`, `tool-use`, `stop`, `session-end`, `approve` implementations; prompt keyword extraction and relevance gate; Claude transcript scan; per-session work/review flags |

### Vault

| Module | Role |
|---|---|
| `vault/mod.rs` | `Vault` connection setup, pragmas, migrations, validation helpers, deprecate/undeprecate |
| `vault/schema.sql` | Table/view definitions for the initial schema |
| `vault/search.rs` | FTS5 search, ranking, catalog listing |
| `vault/revisions.rs` | Publish logic: versioning, scope, optimistic concurrency, proven-version guard |
| `vault/agent.rs` | Agent metadata (`AgentMeta`), tiers/efforts/tools, validation, `agent_models` access, reserved ids |
| `vault/outcomes.rs` | Outcome recording and listing (`helped`/`failed`/`not_applicable`) |
| `vault/transfer.rs` | `purge`, JSON export/import, `backup` (`VACUUM INTO`) |
| `vault/location.rs` | Default vault path; network-filesystem detection (advisory) |

### Setup / clients

| Module | Role |
|---|---|
| `setup/mod.rs` | `Scope`, `Change`/`Edit` model, atomic apply with rollback, `setup`/`--remove`/`--dry-run` orchestration; `SKILLS`, the table of managed skills (`evolution`, `skill-evaluation`, `skill-acquisition`) embedded from `assets/<name>/SKILL.md` |
| `setup/client.rs` | `ClientKind` enum, client sets, per-client `global_dir`/`changes`/`removals` dispatch |
| `setup/common.rs` | Shared pieces: config dirs from env, the managed skills' changes/removals (one per `SKILLS` entry) and `AGENTS.md` block, MCP server argv |
| `setup/fs_safe.rs` | Atomic file writes with backup; per-skill ownership checks (`check_skill`/`is_our_skill` by marker name); strict-JSON load/merge/remove helpers |
| `setup/hooks.rs` | Builds hook entries for all clients; own entries are recognized by command text so reruns replace instead of duplicating |
| `setup/permissions.rs` | Permission grants merged into client settings (subagent dispatch + MCP tools) |
| `setup/plugin.rs` | Embeds `assets/opencode/skillvolution.js` with `--bin`/`--db` substitution; a test pins the plugin's `REVIEW_REMINDER` to `hook::STOP_REASON` |
| `setup/claude.rs` | Claude Code: settings, `.mcp.json`, skill install, legacy `CLAUDE.md` cleanup |
| `setup/claude_cli.rs` | Shelling out to `claude mcp add-json/remove/get` for user-scope MCP registration |
| `setup/opencode.rs` | OpenCode: `opencode.json`/`opencode.jsonc`, permissions, plugin |
| `setup/devin.rs` | Devin CLI: `mcp_config.json`, `config.json`, `AGENTS.md` |
| `setup/codex.rs` | Codex CLI: `config.toml`, `hooks.json`, `AGENTS.md` |
| `setup/gemini.rs` | Gemini CLI: `settings.json`, `GEMINI.md` |
| `setup/cursor.rs` | Cursor: `mcp.json`, `hooks.json`, `cli.json`, rules file (no prompt hook: `beforeSubmitPrompt` cannot inject context) |
| `setup/global.rs` | Global-scope orchestration: client detection, `claude mcp` registration |
| `setup/inspect.rs` | Read-only inspection `doctor` uses to compare files against what setup would write; reads each managed skill's marker version |
| `setup/remove.rs` | `setup --remove` planning and diff printing |
| `setup/prompt.rs` | Interactive client selection via `/dev/tty`; `SKILLVOLUTION_NO_INPUT` handling |

### Doctor / relocate

| Module | Role |
|---|---|
| `doctor.rs` | Health checks (vault + client configuration); read-only unless `--fix` |
| `relocate.rs` | Move the vault file and repoint client configs |
| `project.rs` | Derive the project key from the enclosing git repository |

## Data model

From `src/vault/schema.sql` (current schema v6):

| Table / view | Purpose |
|---|---|
| `skills` | One row per skill `id`; `scope` (`global`/`project`, fixed at first publish), `deprecated` flag, `fts_rowid` |
| `revisions` | Every published version: `id`, `version`, `description`, `tags` (JSON), `content`, `evidence`, `expected_version`, `status` (`published`, or `superseded` for a legacy draft/rejected revision), timestamps and review fields |
| `outcomes` | Agent reports: `id`, `version`, `result` (`helped`/`failed`/`not_applicable`), `note`, `project`, `created_at` |
| `loads` | Successful MCP `get_skill` calls: `id`, `version`, `project`, `created_at`; at most one row per revision, project and UTC day. Local telemetry: not exported or imported |
| `hook_state` | Claude Code transcript offsets (incremental Stop scan) |
| `devin_hook_state` | Per-session `worked`/`reviewed` flags; used by every flag-tracked client (Devin, Codex, Gemini, Cursor) |
| `skills_fts` | FTS5 index over id/description/tags/content, `porter unicode61 remove_diacritics 2` (English stemming on text and queries alike) |
| `skills.kind` | `skill` (default) or `agent`; fixed at an id's first publish |
| `revisions.meta` | JSON agent metadata (`role`, `tier`, `effort`, `tools`, `skills`); NULL for skills |
| `agent_models` | `(client, tier)` -> `model`, `effort`; user overrides of the tier mapping; local, not exported |
| `current_skills` | View (exposes `kind` and `meta`): each skill's latest published revision plus its helped/failed/not_applicable counts, and `prior_helped`/`prior_failed` from the immediately preceding published version (0 if none) |

## Schema versioning and migrations

- Current version: **v6**. Ordered `MIGRATIONS` chain in `src/vault/mod.rs`:
  - v1 → v2: add `devin_hook_state`.
  - v2 → v3: FTS rows addressed by `rowid`; one outcome per skill/version/project/UTC day; legacy drafts retired; hook-state timestamp columns and pruning support.
  - v3 → v4: `loads` table; `current_skills` recreated with `prior_helped`/`prior_failed`.
  - v4 → v5: `revisions` rebuilt with `status IN ('published', 'superseded')`; any remaining `draft`/`rejected` row becomes `superseded` with its `review_note` prefixed `legacy <status>` (`legacy <status>: <old note>`); `current_skills` recreated unchanged; `skills_fts` dropped and recreated with the `porter` tokenizer, repopulated from `current_skills` keyed by `skills.rowid`, and `fts_rowid` reset (as v3 did).
  - v5 → v6: `skills.kind` (`skill`/`agent`, existing rows become `skill`), `revisions.meta`, table `agent_models`; `current_skills` recreated with `kind` and `meta`.
- Migrations run in one IMMEDIATE transaction with foreign keys switched off around it (a table rebuild needs that); the `PRAGMA foreign_key_check` violation count is taken before the steps run and must not be higher afterwards (violations already in the vault are left as they are).
- `PRAGMA application_id` is `SKV1`; a non-empty database with a different `application_id` is refused as foreign.
- A database with a newer schema version is refused.
- When the schema is already current the vault opens without taking a write lock, so concurrent read-only opens don't serialize.
- `hook_state`/`devin_hook_state` rows older than 30 days are pruned.

## Search ranking

From `src/vault/search.rs`:

- FTS5 `bm25` column weights: **id 4.0, description 3.0, tags 2.0, content 1.0**. The raw `bm25` score is sign-inverted and clamped at zero so higher is better.
- Query: each word becomes a quoted prefix term (`"word"*`), OR-joined; words under 3 characters match whole words only (`go` never matches `google`). The porter tokenizer stems query terms like indexed text (`restarts` searches `restart*`, finding `restarting`). Since porter turns a final `y` into `i` (`deploy` → `deploi`) but keeps it inside longer words (`deployment` → `deploy`), a term ending in `y` also searches its stem without the `y`.
- Track record: the current version's net plus half of the immediately preceding
  published version's, so republishing a skill keeps part of what it earned instead of
  starting from zero (and a fix of a failing version starts half-demoted):

  ```text
  net = (helped - failed) + 0.5 * (prior_helped - prior_failed)
  ```

- Outcome multiplier applied to the text score:

  ```text
  1 + net / (ABS(net) + 2.0)
  ```

  The factor is in `(0, 2)`; `not_applicable` outcomes do not affect it.
- Ranked ordering: score after multiplier, then `net` descending, then `id` ascending.
- Empty query (catalog): `net` descending, then `id` ascending.
- The `helped`/`failed`/`not_applicable` counts returned with each hit are the current
  version's only.
- Limits: `limit` 1–100, `offset` >= 0, query at most 512 UTF-8 bytes with no NUL.

## Publishing rules

Enforced in `src/vault/revisions.rs` (and `mcp.rs` for tool-level checks):

- `id` and tags: lowercase ASCII letters/digits in hyphen-separated groups. `id` 1–64 bytes; at most 8 tags of 32 bytes each.
- `description` and `verdict_reason`: single line, at most 280 bytes.
- `content` must contain these headings, each on its own `## ` line, in this order (case-insensitive): `## When to use`, `## Procedure`, `## Pitfalls`, `## Verification`. Preamble and extra sections are allowed.
- Secret scanning over `description`, `content`, `evidence`, `verdict_reason` and outcome `note`: token prefixes (AWS/GitHub/OpenAI/Google/Slack/Stripe/GitLab/npm/Hugging Face/PyPI/OAuth shapes), full JWTs, quoted bearer tokens, PEM private-key blocks, credential URLs with passwords, Slack webhook URLs. Local-dev defaults like `root:secret@localhost` are allowed.
- Verdict: exactly `keep global` or `keep project` (compared trimmed/lowercased, stored verbatim); everything else is refused.
- Verdict/scope pairing: `keep project` requires `scope: project` and a project key; `keep global` requires global scope.
- `scope` is fixed per skill id at first publication.
- `expected_version` provides optimistic concurrency; stale versions are rejected.
- Replacing a proven version (more `helped` than `failed`) requires `replaces_proven: true`.
- Publication is immediate; there is no draft or approval stage.

## Outcomes

- Unique key: `(id, version, project, UTC day)`. A second report on the same day replaces the first — last write wins.
- `helped`/`failed` feed the search multiplier (current version plus half of the previous one) and the proven-version check (current version only).
- `not_applicable` never affects search ranking; it only keeps a skill out of prompt-time suggestions (see the prompt hook gate).
- `loads`: a successful MCP `get_skill` records `(id, version, project)` with `INSERT OR IGNORE` under a unique index on `(id, version, project, UTC day)`. Recording is best effort: a failure is logged to stderr and `get_skill` still returns the skill. `skillvolution outcomes` shows `loaded` (load pairs on the current version) and `unreported` (load pairs with no outcome report on that version from the same project and UTC day; a report made after midnight UTC counts against the next day). Loads are local telemetry: `export` leaves them out and `purge` deletes them.

## Concurrency

- SQLite WAL journal with a busy timeout, so readers and the occasional writer tolerate each other. `synchronous=NORMAL`: commits fsync only at WAL checkpoints, so a slow disk doesn't stall every writer; an OS crash or power loss can lose the last commits but never corrupts the vault.
- Mutations run in `IMMEDIATE` transactions.
- Flag-tracked hook state updates are atomic upserts on `devin_hook_state` keyed by session.

## Hooks

`src/hook.rs`. All hook entry points **fail open**: errors print
`skillvolution hook: <err>` to stderr and the process exits 0, so a broken
vault never stalls the agent.

Two tracking designs. In both, work is a file edit, **a successful `get_skill`**, or a delegation to a managed vault agent (Claude Code `Agent`/`Task`, OpenCode `task`; not `sv-*`; the generated file must carry the skillvolution marker): whoever loaded a skill or delegated to an agent owes an outcome report, so those count like editing. Vault tools are matched by name suffix, since each client prefixes MCP tool names its own way.

- **Claude Code** — transcript scan. `Stop` reads the session transcript JSONL from the stored byte offset (`hook_state`). Work tools: `Edit`, `Write`, `MultiEdit`, `NotebookEdit` (Bash does not count), plus `get_skill`, plus a successful `Agent`/`Task` delegation whose `input.subagent_type` is a vault agent visible from the current project (built-in `sv-*` and unknown types do not count; the subagent's own tool calls are not in this transcript). A review or load counts only when its tool result succeeds (`is_error` results don't). The hook blocks when the latest work (edit or successful load) is newer than the latest successful `publish_skill`/`report_skill_outcome`. Each transcript span is judged once; `stop_hook_active=true` prevents repeated blocking.
- **Flag-tracked clients** (Devin, Codex, Gemini, Cursor) — these do not count delegations to vault agents. `tool-use` sets `worked` on matching file-editing tools or `get_skill` and `reviewed` on vault review tools; `stop` emits the reminder if `worked` is set after `reviewed`. Work after a review re-arms the reminder; every judged stop consumes the span. `session-end` deletes the session state. Work tools per client: Devin `write`/`edit`/`apply_patch`/`notebook_edit`, Codex `apply_patch`, Gemini `write_file`/`replace`, Cursor `Write` (plus `get_skill` everywhere; each client's `PostToolUse` matcher includes its vault tools).

When the span holds successful delegations to managed vault agents after the last review, the Claude Code hook and the OpenCode plugin append a second line to the reminder, `Delegated vault agents to report: counter-agent v2, reviewer v1.` (ids deduplicated, versions from the generated file's marker), so the delegation is not mistaken for a built-in agent; the constant itself is unchanged. In OpenCode, tool calls of a child (subagent) session count toward the parent only when they are file edits: a child's own `get_skill` loads and `report_skill_outcome` calls are about its skills and never mark the parent reviewed. Project agent files are looked up under the plugin input's `worktree` (the repository root, where `agents add` writes), falling back to `directory`.

The reminder text is `STOP_REASON` in `hook.rs`: it asks for `report_skill_outcome` on every skill loaded with `get_skill` and every vault agent delegated to, then for any lesson meeting the criteria a fresh subagent following the `skill-evaluation` skill, then `publish_skill`. The OpenCode plugin's `REVIEW_REMINDER` must be identical (a test checks it).

`hook approve` (wired to PermissionRequest for Devin and Codex) emits
`{"decision":"approve","reason":"Skillvolution-managed tool"}` for
`run_subagent`, `read_subagent` and every `mcp__skillvolution__*` tool.

`session-start` prints the catalog as a JSON payload: `hookSpecificOutput.additionalContext`
for Claude Code, Devin, Codex, and Gemini, `additional_context` for Cursor. Its header
tells the agent to check the catalog (`search_skills` if nothing fits; matches are also
suggested per request), say in one line which skill it applies, and follow
`skill-acquisition` when it lacks the know-how.

### Prompt hook

`hook prompt` is prompt-time retrieval: SessionStart runs before the task is known, so
the catalog alone rarely surfaces the right skill. The hook reads the payload's `prompt`
field and runs, in `hook.rs`:

1. **Keywords** — split on non-alphanumerics, lowercase and fold Latin diacritics (the
   same folding FTS5's `remove_diacritics 2` does), then drop English and Spanish
   stopwords (including `use` and `when`, which start every skill description), tokens
   shorter than 3 characters (except 2-letter tech terms such as `ci`, `go`, `js`, `ui`, `db`, which match whole words only) or longer than 30 bytes (hashes, paths), and repeats.
   First-seen order, capped at 16 keywords.
2. **Minimum** — fewer than 2 keywords means no search and no output.
3. **Translation** — a keyword found in `ES_EN`, a const Spanish→English table of about
   470 developer words (`contenedor` → container, `despliegue` → deploy, `colgado` →
   hang), is replaced by its English translations, so Spanish prompts reach the
   (English) skills and a Spanish word stops prefix-matching unrelated English
   (`genera` → generation). Inflected forms are looked up by stripping a Spanish ending
   (`ES_INFLECTIONS`: `fallan` → `fallar`, `contenedores` → `contenedor`, `vieja` →
   `viejo`); a word with no entry is never altered. Spanish words that are common
   English words (`red`, `once`, `come`, `pie`, `sin`) are not keys, and English words
   an ending rule would misread (`cargo` as a form of `carga`) are pinned untranslated.
4. **Search** — the keywords (translations in place of their Spanish words) go through
   the normal `search` (project-scoped, outcome ranked); the top 5 hits are candidates.
5. **Cutoff** — a candidate scoring below half the top candidate's ranked score is
   dropped: it matched a word or two where the top hit matched the gist, and such hits
   are what crowded suggestions with same-domain skills. The score rides along on
   `SkillMetadata` (`#[serde(skip)]`, never serialized).
6. **Gate** — bm25 depends on the whole corpus, so a hit is re-checked against its own
   metadata: at least 2 distinct non-generic keywords must match its id, tags, or
   description, in at least 2 different *places*. A place is the whole id, one whole
   tag, or one distinct whitespace-separated description token, deduplicated, so
   "docker compose" matching only the tag `docker-compose` (or a hyphenated id, or
   `ipfs-http-client` in a tag and the description) counts once. Prompt words match as
   FTS does (a prefix, or the whole word under 3 characters); translations must be the
   whole word, give or take a plural or -ing/-ed ending, and a Spanish word with two
   translations is still one keyword, as are two forms of one word (equal up to that
   ending, or one a 5+ character prefix of the other: "deploy deployment"). Generic developer words (`GENERIC`: add, app,
   build, change, code, create, error, file, fix, issue, problem, run, script, set,
   test, update, work, write and their inflections) still go to search but never count:
   they say nothing about which procedure applies. A body-only match fails. A skill
   whose current version has net failures (`failed - helped`) of 2 or
   more, or net not-applicable reports (`not_applicable - helped`) of 2 or more (its
   description triggers on tasks it doesn't fit), is never suggested. Both use the
   current version's counts only, so a republished version gets a fresh chance; an
   explicit `search_skills` still finds the skill. At most 3 hits are kept, in search order.
7. **Output** — `Skillvolution: vault skills matching this request:` followed by one
   `- id vN: description [helped H, failed F]` line per skill and an instruction to call
   `get_skill` for the best match, say in one line which skill is applied (or that none
   fits), and call `report_skill_outcome` afterwards. Printed as
   `hookSpecificOutput.additionalContext` with the client's event name (`UserPromptSubmit`
   for Claude Code, Codex, and Devin; `BeforeAgent` for Gemini CLI). When nothing is
   relevant nothing is printed, so the client adds nothing.

Cursor is not wired: its `beforeSubmitPrompt` can only allow or block the prompt, with
no field that adds context, so `hook prompt --client cursor` fails (stderr, exit 0,
like every hook). OpenCode has no hook config; its plugin calls `hook prompt` from the
`chat.message` hook for each top-level user message and appends the result to that
turn's system prompt. Devin's `UserPromptSubmit` payload follows its docs but is
unverified against a live session.

## MCP tool changes for accountability

`get_skill` results carry a `next` field instructing the agent to apply the procedure
only as far as it fits, verify it with the Verification section, and call
`report_skill_outcome` with the loaded id and version. The tool descriptions and the
`initialize` instructions mention per-request suggestions, the one-line "which skill I
am applying" statement, the `skill-acquisition` skill, and that publication requires a
fresh subagent following `skill-evaluation`.

## Managed skills

`setup` installs every entry of `SKILLS` (`setup/mod.rs`) per client at
`<skills dir>/<name>/SKILL.md`:

| Skill | Purpose |
|---|---|
| `evolution` (v13) | The vault loop: search and apply, report outcomes, reflect, dispatch the evaluator, publish |
| `skill-evaluation` | The evaluator subagent's rubric: hard gates, 16-point rubric (keep at 11+), verdict format `publish_skill` expects |
| `skill-acquisition` | What an agent follows on noticing it lacks know-how: vault first, then local and authoritative sources, a small experiment, stop-loss, capture |

Each file starts with the frontmatter `name` and carries its own marker
`<!-- skillvolution-managed:<name>:vN -->`. The marker decides ownership per file: setup
overwrites a file with its marker (any version), refuses to write over a file at that
path without it (an `unowned` conflict), and `--remove` deletes only marked files,
skipping a user's own file with a note, then removes each skill directory that ended up
empty. `doctor` compares each installed marker version with the binary's. The
AGENTS.md block and the Cursor rule tell the agent to use `skill-acquisition` and to
have the evaluator follow `skill-evaluation`.

## Agents

Subagent definitions live in the same vault as skills (`skills.kind = 'agent'`, metadata in
`revisions.meta`) and share versioning, scope, outcomes and export/import. They are excluded from
`search_skills`, the SessionStart catalog and prompt suggestions (`search()` is skills only;
`search_kind` selects by kind). Only the CLI lists them (`list --kind agent`).

**Format.** `AGENT.md` = frontmatter (`name`, `description`, `tier`, optional `role`, `effort`,
`tools`, `skills`, `tags`, `scope`) + body. `role: primary` is rejected (not supported yet).
`tools` is an allowlist over `read, edit, shell, web, mcp` (omitted = all; `read` always allowed);
`skills` requires `mcp`. The body needs `## Output contract`; `## Role`, `## Procedure`,
`## Output contract`, `## Limits` keep that order when present. Agent ids starting
with `sv-` are reserved for the built-in workers (`sv-high/mid/low`, embedded from `assets/agents/`,
no vault row, so no outcome reports), as are the client/managed names `general-purpose`,
`statusline-setup`, `claude-code-guide`, `explore`, `plan`, `build`, `general`, `compaction`, `title`,
`summary`, `evolution`, `skill-evaluation`, `skill-acquisition` (`reserved_agent_id`; skills may use
them). Checked on publish, on `import`, and in `agents add/sync`. Publishing an agent also requires
every id in `skills` to be a published skill visible from the agent's scope; `import` does not
check this, and `agents add/sync` and `doctor` warn about skills that are gone. `purge` refuses a
skill listed by a current agent revision unless `--force`. A skill id cannot be reused as an agent or the reverse.

**Tiers.** `high|mid|low` abstract the model. Claude Code defaults: high -> `opus`/`high`,
mid -> `sonnet`/`medium`, low -> `haiku`/`low`. OpenCode has no default (the file inherits the
client's model, with a warning). `agents models set` overrides per client in `agent_models`
(not exported).

**Generation** (`agents add`/`sync`), one file per agent and configured client:
- Claude Code (`.claude/agents/<id>.md`, global `$CLAUDE_CONFIG_DIR/agents`): frontmatter `name`,
  `description`, `model`, `effort`, `disallowedTools` built from the denied abstract tools
  (edit -> `Edit, Write, NotebookEdit, MultiEdit`; shell -> `Bash, PowerShell, Monitor`; web -> `WebFetch, WebSearch`; mcp -> `mcp__*`; a restricted agent also denies `Agent, Task`).
- OpenCode (`.opencode/agents/<id>.md`, global `~/.config/opencode/agents`): `description`,
  `mode: subagent`, `model`, and a `permission` block with `deny` for edit, bash, webfetch and
  websearch, plus `task: deny` (restricted) or `task: allow`. MCP tools can only be denied per server
  (`<server>_*`), which we do not write, so a denied `mcp` is not enforced (warning);
  effort is not written (warning).
- The body is the content plus a line telling the agent to load its `skills` with `get_skill` at the pinned versions (`id vN (#hash)`, resolved when the file is rendered; the hash is a content digest for change detection, and skills not available from the agent's scope are left out, along with the report line when none remains), and a second line asking it to call `report_skill_outcome` for each before returning (`import` strips both).

**Ownership.** The first line after the frontmatter is
`<!-- skillvolution-managed:agent:<id>:v<N>:<hash8> -->`, hash = FNV-1a 32 of the file without
marker lines (BOM and line endings normalized). Id must equal the file stem; anything else is
foreign. States: missing, foreign (never overwritten), edited (hash mismatch: skipped by
`add`/`remove`/`sync`, `doctor` warns), managed. Writes go through `setup::write_all`
(atomic, rollback). `sync` is a dry run unless `--apply`, flags widened tools and changed prompts
on shell-capable agents, and nothing runs it automatically. `agents import` converts Claude Code
agent files back into canonical `AGENT.md` for `publish`.

Postponed: `role: primary`, Codex/Cursor/Hermes/Antigravity targets (Cursor only reads
`.claude/agents/`), and `publish_skill` for agents.

## Setup design

- `ClientKind`: `ClaudeCode`, `OpenCode`, `Devin`, `Codex`, `Gemini`, `Cursor`. `--client` also accepts `all` and `both` (Claude Code + OpenCode).
- `Scope`: global (per-user config directories) or `--project` (files inside the project directory).
- Each client module produces a list of `FileChange`s and `removals`; the applier writes files atomically with rollback on failure.
- Backups: overwritten user files get `.skillvolution.bak` rotation (up to three). Fully managed files get none.
- Ownership markers decide what `setup --remove` may delete: it removes only Skillvolution-owned entries and preserves user keys and values (global install merges and preserves user keys like `env` and `enabled`; project install replaces the MCP server entry whole as a security measure; surgical removal on uninstall).
- Global setup detects clients by executable on `PATH` or an existing config dir; non-detected clients are skipped. Prompts default to yes; `SKILLVOLUTION_NO_INPUT` (non-empty) or no TTY configures every detected client.
- Project setup defaults to all clients and derives the project key from the directory name (`setup --project-key` overrides).
- `setup --dry-run` prints unified diffs and writes nothing.
- A vault on a network filesystem triggers an advisory warning (SQLite WAL should live on local storage); setup proceeds.

## Transfer

- `publish PATH`, `search`, `list`, `get` — the human counterparts of `publish_skill`, `search_skills` and `get_skill`. `publish` preflights the SKILL.md (`vault::skill_problems`, built on `section_problems`), then calls `propose` with the user as evaluator (`keep global`/`keep project`, `expected_version` = current version, `replaces_proven` with `--replace`). `get` prints a round-trippable SKILL.md and records no outcome.
- `purge ID [--version N]` — permanently deletes a skill or one revision plus its outcomes and loads in an `IMMEDIATE` transaction with `secure_delete` on. Deletes the row(s) immediately, then compacts the file (secure_delete, VACUUM, WAL checkpoint) so purged content doesn't linger on disk. Compaction can't complete while another connection (an AI client's MCP server) holds the vault open; then it prints a warning and the content may linger until a later purge compacts it.
- `export [-o FILE]` — dumps all skills (agents carry `kind` and revision `meta`; `agent_models` is not exported), revisions and outcomes as JSON, format `skillvolution-export` version 1 (version 2 when the export contains any agent, so older binaries refuse it; skill-only exports stay version 1). Loads and hook state are per-machine telemetry and are not exported (so `import` never brings any).
- `import FILE` — merges an export (version 1 or 2): identical revisions are skipped, same-id/version content conflicts are errors, outcomes insert-or-skip on the uniqueness key, `draft`/`rejected` revisions from pre-v5 exports are stored as `superseded` (same note prefix as the v5 migration), and aborts if a revision's or outcome's id differs from the skill it is listed under.
- `backup PATH` — `VACUUM INTO` a path that must not exist; creates parent directories.

## Doctor

`skillvolution doctor` is read-only. Managed agent files get warn-only checks (edited, outdated, missing from the vault, no model for the tier, catalog size). It opens the vault read-only (schema
version, `PRAGMA quick_check`, FTS orphan check, stats) and diffs each
detected client's files against what `setup` would write — stale skill
versions (each managed skill checked separately), missing configured binaries, Claude MCP registration status, and
legacy installs. `--json` emits the same checks as data; the command exits
non-zero on failures. `--fix` attaches a repair to each fixable check, applies
them (schema migration via `Vault::open`, `Vault::rebuild_search_index`, global
setup re-applied per configured client through `write_all`, Claude MCP
re-registration), then re-runs every check; see [cli.md](cli.md#doctor).

## Relocate

`skillvolution relocate PATH` moves the vault:

1. Validates every global client config it would repoint first (a config that can't be read aborts with nothing written).
2. Prints a `NOTE: close running AI client sessions: …` warning.
3. Copies the database via the same `VACUUM INTO` path as `backup`.
4. Verifies the copy (integrity and contents) before touching clients.
5. Writes repointed configs for global clients that referenced the old path (re-registering Claude Code's MCP server); project-level installs are left untouched.
6. Best-effort WAL checkpoint.
7. Renames the old database to `<name>.relocated.bak` and any `-wal`/`-shm` sidecars to `.relocated.bak-wal`/`.relocated.bak-shm` (nothing is deleted).
8. Prints a `SKILLVOLUTION_DB=<new>` hint when the CLI's default vault was relocated. Path matching handles both the displayable path and escaped variants (Windows canonical paths with doubled backslashes).

## Project key

`src/project.rs`: at runtime the key is the enclosing git repository's
directory name — ASCII alphanumerics lowercased, runs of anything else
collapsed to one hyphen, trimmed, capped at 64 bytes. Submodules key on
their own folder name; worktrees resolve to the main repository name.
`setup --project DIR` instead defaults the key from `DIR` itself, which can
differ when `DIR` is a subdirectory of a larger repo.
