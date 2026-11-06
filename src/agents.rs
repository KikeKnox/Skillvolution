//! Subagent files generated from vault agents (and the built-in tier agents) for Claude
//! Code and OpenCode: rendering, the marker that tells our files from the user's, and the
//! `add` / `remove` / `sync` / `import` operations behind `skillvolution agents`.
//!
//! A generated file carries `<!-- skillvolution-managed:agent:<id>:v<N>:<hash8> -->` right
//! after its frontmatter. The hash covers the whole file except marker lines, so a hand
//! edit is detected and never overwritten.

use crate::{
    project,
    setup::{self, ClientKind, Edit, MANAGED_MARKER, Scope},
    skill_file,
    vault::{self, AgentMeta, AgentModel, EFFORTS, TIERS, TOOLS, Vault, validate_id},
};
use anyhow::{Context, Result, bail, ensure};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub const BUILTINS: [(&str, &str); 3] = [
    ("sv-high", include_str!("../assets/agents/sv-high/AGENT.md")),
    ("sv-mid", include_str!("../assets/agents/sv-mid/AGENT.md")),
    ("sv-low", include_str!("../assets/agents/sv-low/AGENT.md")),
];
/// Bump when a built-in asset changes so `sync` regenerates it.
pub const BUILTIN_VERSION: i64 = 1;
/// Appended to the body of an agent with vault skills; `import` strips it back out.
const SKILLS_LINE: &str = "Before starting, load these vault skills with get_skill, passing the version number shown (the #hash only detects content changes; do not pass it): ";
/// Follows `SKILLS_LINE`, asking the subagent to report the skills it loaded.
const REPORT_LINE: &str = "Before returning, call report_skill_outcome once for each of them (helped, failed with the wrong step, or not_applicable).";
const MARKER_PREFIX: &str = "<!-- skillvolution-managed:agent:";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    ClaudeCode,
    OpenCode,
}

impl Target {
    pub const ALL: [Target; 2] = [Target::ClaudeCode, Target::OpenCode];

    pub fn token(self) -> &'static str {
        match self {
            Target::ClaudeCode => "claude-code",
            Target::OpenCode => "opencode",
        }
    }

    pub fn from_token(token: &str) -> Option<Target> {
        Target::ALL.into_iter().find(|t| t.token() == token)
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Target::ClaudeCode => "Claude Code",
            Target::OpenCode => "OpenCode",
        }
    }

    fn kind(self) -> ClientKind {
        match self {
            Target::ClaudeCode => ClientKind::ClaudeCode,
            Target::OpenCode => ClientKind::OpenCode,
        }
    }

    /// Where this client reads agents: the project's own directory, or the per-user one
    /// when `project` is `None`.
    pub fn dir(self, project: Option<&Path>) -> Result<PathBuf> {
        Ok(match (self, project) {
            (Target::ClaudeCode, Some(dir)) => dir.join(".claude").join("agents"),
            (Target::OpenCode, Some(dir)) => dir.join(".opencode").join("agents"),
            (_, None) => self.kind().global_dir()?.join("agents"),
        })
    }
}

/// Tier model compiled in for `target`; stored `agent_models` rows override it.
fn default_model(target: Target, tier: &str) -> Option<(&'static str, &'static str)> {
    match (target, tier) {
        (Target::ClaudeCode, "high") => Some(("opus", "high")),
        (Target::ClaudeCode, "mid") => Some(("sonnet", "medium")),
        (Target::ClaudeCode, "low") => Some(("haiku", "low")),
        _ => None,
    }
}

/// The `(model, effort)` generated files get for `tier` on `target`.
pub fn model_for(
    stored: &[AgentModel],
    target: Target,
    tier: &str,
) -> Option<(String, Option<String>)> {
    stored
        .iter()
        .find(|m| m.client == target.token() && m.tier == tier)
        .map(|m| (m.model.clone(), m.effort.clone()))
        .or_else(|| default_model(target, tier).map(|(m, e)| (m.to_owned(), Some(e.to_owned()))))
}

/// The effort `models set` keeps when `--effort` is not given: the stored row's, else the
/// compiled tier default.
pub fn current_effort(stored: &[AgentModel], client: &str, tier: &str) -> Option<String> {
    Target::from_token(client)
        .and_then(|t| model_for(stored, t, tier))
        .and_then(|(_, effort)| effort)
}

/// A warning when `model` looks like it belongs to the other client's naming scheme.
pub fn model_warning(client: &str, model: &str) -> Option<String> {
    match client {
        "claude-code" if model.contains('/') => Some(format!(
            "Claude Code models are names or aliases such as `opus` or `claude-sonnet-4-5`, not `provider/model`: `{model}` may not load"
        )),
        "opencode" if !model.contains('/') => Some(format!(
            "OpenCode models are `provider/model` (e.g. `anthropic/claude-sonnet-4-5`): `{model}` may not load"
        )),
        _ => None,
    }
}

/// The `agents models` table: a header, then one aligned line per client and tier.
pub fn models_table(stored: &[AgentModel]) -> Vec<String> {
    let mut rows = vec![["client", "tier", "model", "effort", "origin"].map(str::to_owned)];
    for client in vault::MODEL_CLIENTS {
        let target = Target::from_token(client);
        for tier in TIERS {
            let set = stored.iter().any(|m| m.client == client && m.tier == tier);
            let mapped = target.and_then(|t| model_for(stored, t, tier));
            let (model, effort) = mapped.clone().unwrap_or(("-".into(), None));
            let origin = match (set, mapped) {
                (true, _) => "(set)",
                (false, Some(_)) => "(default)",
                (false, None) => "(none)",
            };
            rows.push([
                client.to_owned(),
                tier.to_owned(),
                model,
                effort.unwrap_or_else(|| "-".into()),
                origin.to_owned(),
            ]);
        }
    }
    let widths: Vec<usize> = (0..5)
        .map(|col| rows.iter().map(|r| r[col].len()).max().unwrap_or(0))
        .collect();
    rows.iter()
        .map(|row| {
            let cells: Vec<String> = row
                .iter()
                .zip(&widths)
                .map(|(cell, width)| format!("{cell:<width$}"))
                .collect();
            cells.join("  ").trim_end().to_owned()
        })
        .collect()
}

pub struct Agent {
    pub id: String,
    pub version: i64,
    pub description: String,
    pub content: String,
    pub meta: AgentMeta,
    pub deprecated: bool,
    /// Project key for a project-scoped vault agent; `None` for global ones and built-ins.
    pub scope: Option<String>,
    /// The published version and a short content hash of each of `meta.skills` visible
    /// from the agent's scope when it was resolved; a skill that is not available has no
    /// entry and is left out of the file. Rendered as `id vN (#hash)`.
    pub skill_versions: Vec<(String, i64, String)>,
}

/// Why a project-scoped `agent` cannot be written to `project_dir` for `target`: the
/// directory is the per-user one (e.g. the working directory is $HOME, so writing would
/// leak it to every project) or belongs to another project.
fn placement_error(target: Target, agent: &Agent, project_dir: Option<&Path>) -> Option<String> {
    let scope = agent.scope.as_deref()?;
    let user_dir = target.dir(project_dir).ok()?;
    if same_dir(&user_dir, &target.dir(None).ok()?) {
        return Some(format!(
            "{} is project-scoped; run inside the project repository",
            agent.id
        ));
    }
    let dir = project_dir?;
    (project::detect(dir).as_deref() != Some(scope)).then(|| {
        format!(
            "{} is scoped to project {scope}; run this from inside {scope}",
            agent.id
        )
    })
}

/// The path with its nearest existing ancestor canonicalized (so a symlinked $HOME
/// compares equal to its target) and the not-yet-created tail re-appended.
fn canonical(path: &Path) -> PathBuf {
    let mut tail = Vec::new();
    let mut current = path;
    loop {
        if let Ok(real) = fs::canonicalize(current) {
            return tail.iter().rev().fold(real, |acc, name| acc.join(name));
        }
        match (current.file_name(), current.parent()) {
            (Some(name), Some(parent)) => {
                tail.push(name.to_owned());
                current = parent;
            }
            _ => return path.to_path_buf(),
        }
    }
}

fn same_dir(a: &Path, b: &Path) -> bool {
    a == b || canonical(a) == canonical(b)
}

pub fn builtin_source(id: &str) -> Option<&'static str> {
    BUILTINS
        .iter()
        .find(|(builtin, _)| *builtin == id)
        .map(|(_, text)| *text)
}

pub fn builtin(id: &str) -> Option<Agent> {
    let file = skill_file::parse(builtin_source(id)?);
    let meta = file.agent_meta();
    Some(Agent {
        id: id.to_owned(),
        version: BUILTIN_VERSION,
        description: file.description?,
        content: file.content,
        meta,
        deprecated: false,
        scope: None,
        skill_versions: Vec::new(),
    })
}

/// A built-in, else the vault agent `id` visible from `project`.
pub fn resolve(vault: &Vault, id: &str, project: Option<&str>) -> Result<Agent> {
    if let Some(agent) = builtin(id) {
        return Ok(agent);
    }
    if let Some(reason) = vault::reserved_agent_id(id) {
        bail!("agent id {id} is reserved: {reason}");
    }
    let view = vault
        .get(id, None, project)
        .map_err(|_| anyhow::anyhow!("no published agent {id} is visible from this project"))?;
    ensure!(view.kind == "agent", "{id} is a skill, not an agent");
    let meta = view.meta.context("agent revision without metadata")?;
    let mut skill_versions = Vec::new();
    for skill in &meta.skills {
        if vault
            .skill_unavailable(skill, view.scope.as_deref())?
            .is_none()
        {
            let version = vault.published_version(skill)?;
            let hash = fnv8(vault.inspect(skill, version)?.content.as_bytes());
            skill_versions.push((skill.clone(), version, hash));
        }
    }
    Ok(Agent {
        id: view.id,
        version: view.version,
        description: view.description,
        content: view.content,
        meta,
        deprecated: view.deprecated,
        scope: view.scope,
        skill_versions,
    })
}

/// `[warning]` lines for the vault skills `agent` references that are not available from
/// the agent's own scope.
fn skill_warnings(vault: &Vault, agent: &Agent) -> Vec<String> {
    agent
        .meta
        .skills
        .iter()
        .filter_map(|skill| {
            if let Some(reason) = vault
                .skill_unavailable(skill, agent.scope.as_deref())
                .ok()
                .flatten()
            {
                return Some(format!(
                    "[warning] agent {}: skill {skill} is not available ({reason})",
                    agent.id
                ));
            }
            vault
                .skill_deprecated(skill)
                .ok()?
                .then(|| format!("[warning] agent {}: skill {skill} is deprecated", agent.id))
        })
        .collect()
}

fn body(agent: &Agent) -> String {
    let mut body = agent.content.trim_end().to_owned();
    // Skills that are not available are left out: there is no version to pin.
    let items: Vec<String> = agent
        .meta
        .skills
        .iter()
        .filter_map(|skill| {
            let (_, version, hash) = agent.skill_versions.iter().find(|(id, ..)| id == skill)?;
            Some(format!("{skill} v{version} (#{hash})"))
        })
        .collect();
    if !items.is_empty() {
        body += &format!("\n\n{SKILLS_LINE}{}.\n{REPORT_LINE}", items.join(", "));
    }
    body.push('\n');
    body
}

// ponytail: FNV-1a 32-bit instead of a hash crate; it only has to notice hand edits.
/// 8 lowercase hex digits of FNV-1a over `bytes`.
fn fnv8(bytes: &[u8]) -> String {
    let mut hash: u32 = 0x811c_9dc5;
    for &byte in bytes {
        hash = (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193);
    }
    format!("{hash:08x}")
}

/// 8 lowercase hex digits of FNV-1a over the file's lines minus the marker line (the one
/// right after the closing `---`; any other marker-looking line is content) and a leading
/// BOM, joined by `\n`, so line endings and a trailing newline don't matter.
fn hash8(text: &str) -> String {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines: Vec<&str> = text.lines().collect();
    let closing = lines.iter().skip(1).position(|l| l.trim_end() == "---");
    if let Some(at) = closing.map(|i| i + 2)
        && lines.get(at).is_some_and(|l| l.starts_with(MARKER_PREFIX))
    {
        lines.remove(at);
    }
    fnv8(lines.join("\n").as_bytes())
}

/// The file `target` gets for `agent`, and warnings about what it cannot express.
pub fn render(
    target: Target,
    agent: &Agent,
    model: Option<(String, Option<String>)>,
) -> (String, Vec<String>) {
    let scalar = skill_file::yaml_scalar;
    let mut warnings = Vec::new();
    let mut front = Vec::new();
    let meta = &agent.meta;
    let denied = |tool: &str| !meta.allows(tool);
    match target {
        Target::ClaudeCode => {
            front.push(format!("name: {}", scalar(&agent.id)));
            front.push(format!("description: {}", scalar(&agent.description)));
            let mapped_effort = model.as_ref().and_then(|(_, effort)| effort.clone());
            if let Some((model, _)) = &model {
                front.push(format!("model: {}", scalar(model)));
            }
            if let Some(effort) = meta.effort.clone().or(mapped_effort) {
                front.push(format!("effort: {}", scalar(&effort)));
            }
            let mut tools = Vec::new();
            for (tool, names) in [
                ("edit", "Edit, Write, NotebookEdit, MultiEdit"),
                ("shell", "Bash, PowerShell, Monitor"),
                ("web", "WebFetch, WebSearch"),
                ("mcp", "mcp__*"),
            ] {
                if denied(tool) {
                    tools.push(names);
                }
            }
            // A restricted agent must not delegate to an unrestricted subagent.
            if meta.tools.is_some() {
                tools.push("Agent, Task");
            }
            if !tools.is_empty() {
                front.push(format!("disallowedTools: {}", tools.join(", ")));
            }
        }
        Target::OpenCode => {
            front.push(format!("description: {}", scalar(&agent.description)));
            front.push("mode: subagent".into());
            if let Some((model, _)) = &model {
                front.push(format!("model: {}", scalar(model)));
            }
            // OpenCode merges a user-level file with a project one of the same name, so
            // every tool is written explicitly: an omitted key would be inherited.
            front.push("permission:".into());
            for (tool, key) in [
                ("edit", "edit"),
                ("shell", "bash"),
                ("web", "webfetch"),
                ("web", "websearch"),
            ] {
                let rule = if denied(tool) { "deny" } else { "allow" };
                front.push(format!("  {key}: {rule}"));
            }
            front.push(format!(
                "  task: {}",
                if meta.tools.is_some() {
                    "deny"
                } else {
                    "allow"
                }
            ));
            if denied("mcp") {
                warnings.push("OpenCode cannot block MCP tools by kind; not enforced".into());
            }
            if meta.effort.is_some() {
                warnings.push("effort is not written for OpenCode".into());
            }
        }
    }
    if model.is_none() {
        warnings.push(format!(
            "no {} model for tier {}; the client inherits its own (set one with \
             `skillvolution agents models set {} {} <model>`)",
            target.display_name(),
            meta.tier,
            target.token(),
            meta.tier
        ));
    }
    let front = front.join("\n");
    let body = body(agent);
    let hash = hash8(&format!("---\n{front}\n---\n{body}"));
    let marker = format!("{MARKER_PREFIX}{}:v{}:{hash} -->", agent.id, agent.version);
    (format!("---\n{front}\n---\n{marker}\n{body}"), warnings)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Marker {
    pub id: String,
    pub version: i64,
    pub hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileState {
    Missing,
    /// Exists but is not a regular file of ours (no valid marker, a symlink, ...).
    Foreign,
    /// Has our marker but its content no longer matches the marker's hash.
    Edited(Marker),
    Managed(Marker),
}

impl FileState {
    fn marker(&self) -> Option<&Marker> {
        match self {
            FileState::Edited(m) | FileState::Managed(m) => Some(m),
            _ => None,
        }
    }
}

/// The marker on the line right after the closing `---`, in its strict form and with an
/// id equal to the file stem; anything else is not ours.
fn parse_marker(text: &str, stem: &str) -> Option<Marker> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = text.lines();
    if lines.next()?.trim_end() != "---" {
        return None;
    }
    lines.find(|line| line.trim_end() == "---")?;
    let inner = lines
        .next()?
        .strip_prefix(MARKER_PREFIX)?
        .strip_suffix(" -->")?;
    let mut parts = inner.splitn(3, ':');
    let (id, version, hash) = (parts.next()?, parts.next()?, parts.next()?);
    let version = version.strip_prefix('v')?;
    let ok = validate_id(id).is_ok()
        && id == stem
        && !version.is_empty()
        && version.bytes().all(|b| b.is_ascii_digit())
        && hash.len() == 8
        && hash.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    if !ok {
        return None;
    }
    Some(Marker {
        id: id.to_owned(),
        version: version.parse().ok()?,
        hash: hash.to_owned(),
    })
}

pub fn file_state(path: &Path) -> Result<FileState> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(FileState::Missing),
        Err(e) => return Err(e).with_context(|| format!("inspect {}", path.display())),
    };
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let Some(text) = meta
        .is_file()
        .then(|| fs::read_to_string(path).ok())
        .flatten()
    else {
        return Ok(FileState::Foreign);
    };
    Ok(match parse_marker(&text, stem) {
        None => FileState::Foreign,
        Some(marker) if marker.hash == hash8(&text) => FileState::Managed(marker),
        Some(marker) => FileState::Edited(marker),
    })
}

/// The `*.md` files of `dir` that carry our marker, sorted by name.
pub fn managed_files(dir: &Path) -> Vec<(PathBuf, FileState)> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "md"))
        .collect();
    paths.sort();
    paths
        .into_iter()
        .filter_map(|path| {
            let state = file_state(&path).ok()?;
            state.marker().is_some().then_some((path, state))
        })
        .collect()
}

/// Whether `agent` at `version` is already on disk exactly as rendered in `text`.
fn is_current(marker: &Marker, version: i64, text: &str) -> bool {
    marker.version == version && marker.hash == hash8(text)
}

struct Plan {
    target: Target,
    path: PathBuf,
    text: String,
    warnings: Vec<String>,
}

fn plan(
    target: Target,
    agent: &Agent,
    project_dir: Option<&Path>,
    models: &[AgentModel],
) -> Result<Plan> {
    let (text, warnings) = render(target, agent, model_for(models, target, &agent.meta.tier));
    Ok(Plan {
        target,
        path: target.dir(project_dir)?.join(format!("{}.md", agent.id)),
        text,
        warnings,
    })
}

fn configured_targets(project_dir: Option<&Path>) -> Vec<Target> {
    Target::ALL
        .into_iter()
        .filter(|t| setup::client_configured(t.kind(), project_dir))
        .collect()
}

/// Claude Code's agent directories for a hook: the project's (when known), then the
/// per-user one.
pub fn claude_agent_dirs(project_dir: Option<&Path>) -> Vec<PathBuf> {
    [project_dir, None]
        .into_iter()
        .filter_map(|dir| Target::ClaudeCode.dir(dir).ok())
        .collect()
}

/// The version in the marker of the file we generated for agent `id` in the first of
/// `dirs` that has one (edited or not; the same test the OpenCode plugin's
/// `managedAgentVersion` applies), or `None` when no directory holds one.
pub fn managed_agent_version(dirs: &[PathBuf], id: &str) -> Option<i64> {
    if validate_id(id).is_err() {
        return None;
    }
    dirs.iter().find_map(|dir| {
        let state = file_state(&dir.join(format!("{id}.md"))).ok()?;
        state.marker().map(|m| m.version)
    })
}

/// Where the managed copy of `id` for the other level (user when `project_dir` is a
/// project, the working directory's project otherwise) lives, if there is one.
fn other_level_copy(target: Target, id: &str, project_dir: Option<&Path>) -> Option<PathBuf> {
    let other_dir = match project_dir {
        Some(_) => target.dir(None).ok()?,
        None => {
            let cwd = std::env::current_dir().ok()?;
            target.dir(Some(&project::repo_dir(&cwd)?)).ok()?
        }
    };
    if same_dir(&other_dir, &target.dir(project_dir).ok()?) {
        return None;
    }
    let path = other_dir.join(format!("{id}.md"));
    file_state(&path)
        .ok()
        .is_some_and(|state| state.marker().is_some())
        .then_some(path)
}

fn double_install_warning(target: Target, id: &str, project_dir: Option<&Path>) -> Option<String> {
    let path = other_level_copy(target, id, project_dir)?;
    let name = target.display_name();
    Some(format!(
        "[warning] {name} {id}: also installed at {}; {name} merges the user-level and project files of the same name, so one can inherit settings from the other. Keep only one (`skillvolution agents remove {id}`, with or without --global)",
        path.display()
    ))
}

fn contains_marker(bytes: &[u8]) -> bool {
    let needle = MANAGED_MARKER.as_bytes();
    bytes.windows(needle.len()).any(|w| w == needle)
}

fn backup_note(path: &Path) -> String {
    let mut name = path.as_os_str().to_owned();
    name.push(".skillvolution.bak");
    format!("previous file kept as {}", PathBuf::from(name).display())
}

/// Writes `ids` for every configured client (project dir when `project_dir` is given,
/// else per-user). Refuses before writing anything if a target file is not ours, unless
/// `force`, which overwrites foreign and hand-edited files after a backup.
pub fn add(
    vault: &Vault,
    ids: &[String],
    project_key: Option<&str>,
    project_dir: Option<&Path>,
    force: bool,
) -> Result<Vec<String>> {
    let targets = configured_targets(project_dir);
    ensure!(
        !targets.is_empty(),
        "{}",
        if project_dir.is_some() {
            "no configured client supports agents; run `skillvolution setup` (per user) or \
             `skillvolution setup --project DIR` for Claude Code or OpenCode"
        } else {
            "no client with agent support is set up for the user; run `skillvolution setup` \
             (global is its default) for Claude Code or OpenCode"
        }
    );
    let models = vault.agent_models()?;
    let mut agents = Vec::new();
    let mut seen: Vec<&String> = Vec::new();
    for id in ids {
        validate_id(id)?;
        if seen.contains(&id) {
            continue;
        }
        seen.push(id);
        let agent = match resolve(vault, id, project_key) {
            Ok(agent) => agent,
            Err(e) => {
                if project_key.is_none()
                    && project_dir.is_none()
                    && vault.published_version(id)? > 0
                    && vault.kind(id)?.as_deref() == Some("agent")
                {
                    bail!(
                        "{id} is project-scoped; run `skillvolution agents add {id}` (without --global) inside its project"
                    );
                }
                return Err(e);
            }
        };
        agents.push(agent);
    }
    let (mut lines, mut edits, mut backups) = (Vec::new(), Vec::new(), Vec::new());
    for agent in &agents {
        lines.extend(skill_warnings(vault, agent));
        if agent.deprecated {
            lines.push(format!(
                "[deprecated] {}: deprecated in the vault; not added",
                agent.id
            ));
            continue;
        }
        if !agent.meta.skills.is_empty()
            && (agent.meta.allows("shell") || agent.meta.allows("edit"))
        {
            lines.push(format!(
                "note: {}: its vault skills run with its shell/edit tools and stay pinned to the versions listed in the file until `skillvolution agents sync --apply`",
                agent.id
            ));
        }
        for &target in &targets {
            if let Some(problem) = placement_error(target, agent, project_dir) {
                bail!("{problem}");
            }
            let plan = plan(target, agent, project_dir, &models)?;
            let tag = match file_state(&plan.path)? {
                FileState::Foreign | FileState::Edited(_) if force => {
                    // `fs_safe::write` skips its own backup when old and new both carry
                    // the marker (an edited file, or a renamed copy of a generated one).
                    if fs::read(&plan.path).is_ok_and(|old| contains_marker(&old)) {
                        backups.push(plan.path.clone());
                    }
                    lines.push(format!("  {}", backup_note(&plan.path)));
                    "overwritten"
                }
                FileState::Foreign => bail!(
                    "{} exists and was not generated by skillvolution; move or delete it first, or pass --force to overwrite it (a backup is kept)",
                    plan.path.display()
                ),
                FileState::Edited(_) => {
                    lines.push(format!(
                        "[edited] {} {}: {} was edited by hand; skipped (--force overwrites it after a backup)",
                        target.display_name(),
                        agent.id,
                        plan.path.display()
                    ));
                    continue;
                }
                FileState::Managed(m) if is_current(&m, agent.version, &plan.text) => {
                    lines.push(format!(
                        "[unchanged] {} {} v{}",
                        target.display_name(),
                        agent.id,
                        agent.version
                    ));
                    lines.extend(double_install_warning(target, &agent.id, project_dir));
                    continue;
                }
                FileState::Managed(_) => "updated",
                FileState::Missing => "added",
            };
            lines.push(format!(
                "[{tag}] {} {} v{} -> {}",
                plan.target.display_name(),
                agent.id,
                agent.version,
                plan.path.display()
            ));
            lines.extend(plan.warnings.iter().map(|w| format!("  warning: {w}")));
            lines.extend(double_install_warning(target, &agent.id, project_dir));
            edits.push(Edit::Write(plan.path, plan.text));
        }
    }
    setup::validate_edits(&edits)?;
    backups
        .iter()
        .try_for_each(|path| setup::backup_file(path))?;
    setup::write_all(edits)?;
    if targets.contains(&Target::ClaudeCode) {
        lines.push(
            "note: Cursor also reads .claude/agents/ and may not accept Claude Code model aliases"
                .into(),
        );
    }
    Ok(lines)
}

/// Deletes our unedited generated files for `ids` in every client's directory. The
/// count is how many ids had nothing removed (not installed, or only foreign/edited files).
pub fn remove(ids: &[String], project_dir: Option<&Path>) -> Result<(Vec<String>, usize)> {
    let (mut lines, mut edits, mut missing) = (Vec::new(), Vec::new(), 0);
    for id in ids {
        validate_id(id)?;
        let (mut found, mut removed) = (false, false);
        for target in Target::ALL {
            let path = target.dir(project_dir)?.join(format!("{id}.md"));
            let name = target.display_name();
            match file_state(&path)? {
                FileState::Missing => continue,
                FileState::Managed(_) => {
                    lines.push(format!("[removed] {name} {id} -> {}", path.display()));
                    edits.push(Edit::Delete(path));
                    removed = true;
                }
                FileState::Edited(_) => lines.push(format!(
                    "[edited] {name} {id}: edited by hand; delete it yourself or `skillvolution agents import` it"
                )),
                FileState::Foreign => lines.push(format!(
                    "[foreign] {name} {id}: {} was not generated by skillvolution; left alone",
                    path.display()
                )),
            }
            found = true;
        }
        if !found || !removed {
            missing += 1;
        }
        if !found {
            lines.push(format!(
                "{id}: not installed{}",
                if project_dir.is_some() {
                    " in this project (pass --global for the per-user directories)"
                } else {
                    " in the per-user directories"
                }
            ));
        }
    }
    setup::write_all(edits)?;
    Ok((lines, missing))
}

/// How the skill pins of the installed file `old_text` differ from `agent`'s, ignoring
/// the skills in `skip` (already reported another way).
fn pin_changes(agent: &Agent, old_text: &str, skip: &[&str]) -> Vec<String> {
    let (_, pinned) = strip_skills_line(&skill_file::parse(old_text).content);
    let mut changes = Vec::new();
    for skill in &agent.meta.skills {
        if skip.contains(&skill.as_str()) {
            continue;
        }
        let was = pinned.iter().find(|(id, ..)| id == skill);
        let now = agent.skill_versions.iter().find(|(id, ..)| id == skill);
        match (was, now) {
            (Some((_, Some(was), _)), Some((_, new, _))) if was != new => {
                changes.push(format!("skill {skill} v{was} -> v{new}"));
            }
            (Some((_, Some(_), Some(was))), Some((_, new, hash))) if was != hash => {
                changes.push(format!("skill {skill} v{new} content changed"));
            }
            (Some((_, Some(_), _)), Some(_)) => {}
            (_, Some((_, new, _))) => changes.push(format!("skill {skill} now v{new}")),
            (Some(_), None) => {
                changes.push(format!("skill {skill} dropped (not available)"));
            }
            (None, None) => {}
        }
    }
    changes
}

/// What changed in permissions or prompt between the installed revision and `agent`.
fn flags(vault: &Vault, old: i64, agent: &Agent, old_text: &str) -> Vec<String> {
    // Skills are pinned in the body, so a new skill version (or new content under the
    // same number) rewrites the file even when the agent's own revision did not change.
    let flag = |changes: Vec<String>| -> Vec<String> {
        changes.into_iter().map(|c| format!("  ! {c}")).collect()
    };
    let mut pins = flag(pin_changes(agent, old_text, &[]));
    if old == agent.version || builtin(&agent.id).is_some() {
        return pins;
    }
    let Ok(previous) = vault.inspect(&agent.id, old) else {
        pins.push(format!(
            "  ! previous revision v{old} unavailable; review the new prompt and tools"
        ));
        return pins;
    };
    let mut flags = Vec::new();
    match &previous.meta {
        None => flags.append(&mut pins),
        Some(previous_meta) => {
            if previous_meta.tier != agent.meta.tier {
                flags.push(format!(
                    "  ! tier changed: {} -> {}",
                    previous_meta.tier, agent.meta.tier
                ));
            }
            let (old, new) = (&previous_meta.skills, &agent.meta.skills);
            let added: Vec<&str> = new
                .iter()
                .filter(|s| !old.contains(s))
                .map(String::as_str)
                .collect();
            let removed: Vec<&str> = old
                .iter()
                .filter(|s| !new.contains(s))
                .map(String::as_str)
                .collect();
            if !added.is_empty() {
                flags.push(format!("  ! skills added: {}", added.join(", ")));
            }
            if !removed.is_empty() {
                flags.push(format!("  ! skills removed: {}", removed.join(", ")));
            }
            flags.extend(flag(pin_changes(agent, old_text, &added)));
            let widened: Vec<&str> = TOOLS
                .into_iter()
                .filter(|t| agent.meta.allows(t) && !previous_meta.allows(t))
                .collect();
            if !widened.is_empty() {
                flags.push(format!("  ! tools widened: {}", widened.join(", ")));
            }
        }
    }
    if previous.content.trim_end() != agent.content.trim_end() && agent.meta.allows("shell") {
        flags.push("  ! prompt changed on an agent with shell access".into());
    }
    flags
}

/// The top-level frontmatter keys of `text` with their values; the indented lines of a
/// block (OpenCode's `permission:`) are joined into the key's value.
fn front_pairs(text: &str) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = Vec::new();
    for line in text
        .lines()
        .skip(1)
        .take_while(|line| line.trim_end() != "---")
    {
        if line.starts_with(' ') {
            if let Some((_, value)) = pairs.last_mut() {
                if !value.is_empty() {
                    value.push_str(", ");
                }
                value.push_str(line.trim());
            }
        } else if let Some((key, value)) = line.split_once(':') {
            pairs.push((key.to_owned(), value.trim().to_owned()));
        }
    }
    pairs
}

/// Why `new` differs from `old` when the version did not change: the frontmatter keys
/// (model, effort, tools, permissions) whose value moved, then the skill pins that did.
fn rendering_changes(agent: &Agent, old: &str, new: &str) -> Vec<String> {
    let (before, after) = (front_pairs(old), front_pairs(new));
    let mut changes = Vec::new();
    for (key, value) in &after {
        match before.iter().find(|(k, _)| k == key) {
            Some((_, was)) if was == value => {}
            Some((_, was)) => changes.push(format!("{key} {was} -> {value}")),
            None => changes.push(format!("{key} added: {value}")),
        }
    }
    for (key, was) in &before {
        if !after.iter().any(|(k, _)| k == key) {
            changes.push(format!("{key} removed (was {was})"));
        }
    }
    changes.extend(pin_changes(agent, old, &[]));
    if changes.is_empty() {
        changes.push("rendering changed".into());
    }
    changes
}

/// Regenerates our unedited files from the vault. Without `apply` it only reports. With
/// `force`, hand-edited managed files are regenerated too, after a backup.
pub fn sync(
    vault: &Vault,
    project_key: Option<&str>,
    project_dir: Option<&Path>,
    apply: bool,
    force: bool,
) -> Result<Vec<String>> {
    let models = vault.agent_models()?;
    let (mut lines, mut edits, mut unchanged, mut backups) =
        (Vec::new(), Vec::new(), 0, Vec::new());
    let mut warned = Vec::new();
    for target in configured_targets(project_dir) {
        let name = target.display_name();
        for (path, state) in managed_files(&target.dir(project_dir)?) {
            let (FileState::Managed(marker) | FileState::Edited(marker)) = &state else {
                continue;
            };
            let id = &marker.id;
            let edited = matches!(state, FileState::Edited(_));
            if edited && !force {
                lines.push(format!(
                    "[edited] {name} {id}: edited by hand; skipped (`--apply --force` overwrites it after a backup)"
                ));
                continue;
            }
            let agent = match resolve(vault, id, project_key) {
                Ok(agent) => agent,
                Err(e) => {
                    lines.push(format!("[missing] {name} {id}: {e:#}"));
                    continue;
                }
            };
            if !warned.contains(id) {
                warned.push(id.clone());
                lines.extend(skill_warnings(vault, &agent));
            }
            if agent.deprecated {
                lines.push(format!(
                    "[deprecated] {name} {id}: deprecated in the vault; left as is"
                ));
                continue;
            }
            if let Some(problem) = placement_error(target, &agent, project_dir) {
                lines.push(format!("[refused] {name} {id}: {problem}"));
                continue;
            }
            let plan = plan(target, &agent, project_dir, &models)?;
            if !edited && is_current(marker, agent.version, &plan.text) {
                unchanged += 1;
                lines.push(format!("[ok] {name} {id} v{}", agent.version));
                lines.extend(double_install_warning(target, id, project_dir));
                continue;
            }
            let old = fs::read_to_string(&path).unwrap_or_default();
            if edited {
                lines.push(format!(
                    "[update] {name} {id} v{}: edited by hand; overwritten with v{} ({})",
                    marker.version,
                    agent.version,
                    backup_note(&path)
                ));
                backups.push(path.clone());
            } else if marker.version == agent.version {
                lines.push(format!(
                    "[update] {name} {id} v{}: {}",
                    marker.version,
                    rendering_changes(&agent, &old, &plan.text).join("; ")
                ));
            } else {
                lines.push(format!(
                    "[update] {name} {id} v{} -> v{}",
                    marker.version, agent.version
                ));
            }
            lines.extend(flags(vault, marker.version, &agent, &old));
            lines.extend(plan.warnings.iter().map(|w| format!("  warning: {w}")));
            lines.extend(double_install_warning(target, id, project_dir));
            edits.push(Edit::Write(plan.path, plan.text));
        }
    }
    let updates = edits.len();
    if apply {
        setup::validate_edits(&edits)?;
        backups
            .iter()
            .try_for_each(|path| setup::backup_file(path))?;
        setup::write_all(edits)?;
        lines.push(format!("{updates} file(s) updated, {unchanged} unchanged."));
    } else {
        lines.push(format!("{unchanged} unchanged."));
        if updates > 0 {
            lines.push(format!(
                "dry run: re-run with --apply to write {updates} file(s)"
            ));
        }
    }
    Ok(lines)
}

/// A note for `setup --remove`: managed agent files its client leaves in `scope`.
pub(crate) fn left_behind_note(kind: ClientKind, scope: Scope) -> Option<String> {
    let target = Target::ALL.into_iter().find(|t| t.kind() == kind)?;
    let dir = target
        .dir(match scope {
            Scope::Project { dir, .. } => Some(dir),
            Scope::Global => None,
        })
        .ok()?;
    let ids: Vec<String> = managed_files(&dir)
        .iter()
        .filter_map(|(_, state)| state.marker().map(|m| m.id.clone()))
        .collect();
    (!ids.is_empty()).then(|| {
        format!(
            "managed agent files were left in {} ({}); remove them with `skillvolution agents remove <id>`",
            dir.display(),
            ids.join(", ")
        )
    })
}

pub struct Imported {
    pub id: String,
    /// A publishable AGENT.md.
    pub text: String,
    pub notes: Vec<String>,
}

/// AGENT.md text(s) from a Claude Code agent file, or a directory of them; a file that
/// cannot be imported is an `Err` entry, not a failure of the batch.
pub fn import(vault: &Vault, path: &Path) -> Result<Vec<(PathBuf, Result<Imported>)>> {
    let files = if path.is_dir() {
        let mut files: Vec<PathBuf> = fs::read_dir(path)
            .with_context(|| format!("read {}", path.display()))?
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_file() && p.extension().is_some_and(|e| e == "md"))
            .collect();
        files.sort();
        files
    } else {
        vec![path.to_path_buf()]
    };
    ensure!(!files.is_empty(), "no *.md files in {}", path.display());
    Ok(files
        .into_iter()
        .map(|file| {
            let imported = import_file(vault, &file);
            (file, imported)
        })
        .collect())
}

fn abstract_tool(name: &str) -> Option<&'static str> {
    match name.split('(').next().unwrap_or(name).trim() {
        "Edit" | "Write" | "MultiEdit" | "NotebookEdit" => Some("edit"),
        "Bash" => Some("shell"),
        "WebFetch" | "WebSearch" => Some("web"),
        "Read" | "Grep" | "Glob" => Some("read"),
        n if n.starts_with("mcp__") => Some("mcp"),
        _ => None,
    }
}

/// A skill listed on the `SKILLS_LINE`: id, pinned version and content hash, when shown.
type Pin = (String, Option<i64>, Option<String>);

/// Parses one item of the line: `id`, `id vN` or `id vN (#hash)`.
fn parse_pin(item: &str) -> Pin {
    let bare = || (item.to_owned(), None, None);
    let Some((id, rest)) = item.split_once(' ') else {
        return bare();
    };
    let (version, hash) = match rest.split_once(' ') {
        Some((v, h)) => (v, h.strip_prefix("(#").and_then(|h| h.strip_suffix(')'))),
        None => (rest, None),
    };
    match version.strip_prefix('v').and_then(|v| v.parse().ok()) {
        Some(version) => (id.to_owned(), Some(version), hash.map(str::to_owned)),
        None => bare(),
    }
}

/// Splits our `SKILLS_LINE` and `REPORT_LINE` out of `content`; each skill is an `id`, a
/// pinned `id vN` or `id vN (#hash)`.
fn strip_skills_line(content: &str) -> (String, Vec<Pin>) {
    let mut skills = Vec::new();
    let kept: Vec<&str> = content
        .lines()
        .filter(|line| match line.trim_end().strip_prefix(SKILLS_LINE) {
            None if line.trim_end() == REPORT_LINE => false,
            Some(list) => {
                skills = list
                    .trim_end_matches('.')
                    .split(", ")
                    .map(parse_pin)
                    .collect();
                false
            }
            None => true,
        })
        .collect();
    (kept.join("\n").trim_end().to_owned(), skills)
}

fn import_file(vault: &Vault, path: &Path) -> Result<Imported> {
    let text = fs::read_to_string(path).context("read file")?;
    let file = skill_file::parse(&text);
    ensure!(file.problems.is_empty(), "{}", file.problems.join("; "));
    let mut notes = Vec::new();
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let state = file_state(path)?;
    let marker = state.marker().cloned();
    let managed = marker.is_some();
    let edited = matches!(state, FileState::Edited(_));
    let mut vault_skills_scope = None;

    // Our own file: the abstract metadata comes from the vault (or the built-in), the
    // prompt and description from the file as it is now.
    let (id, meta, tags, scope) = if let Some(marker) = marker {
        let (meta, tags, scope) = match builtin(&marker.id) {
            Some(agent) => (agent.meta, Vec::new(), None),
            None => {
                let revision = vault.inspect(&marker.id, marker.version)?;
                let meta = revision
                    .meta
                    .with_context(|| format!("{} is not an agent in the vault", marker.id))?;
                let current = vault.published_version(&marker.id)?;
                if marker.version < current {
                    notes.push(format!(
                        "file is from v{}; the vault is at v{current} (metadata taken from v{})",
                        marker.version, marker.version
                    ));
                }
                vault_skills_scope = revision.scope.clone();
                (meta, revision.tags, revision.scope)
            }
        };
        (marker.id, meta, tags, scope)
    } else {
        ensure!(
            !front_pairs(&text).iter().any(|(key, _)| key == "mode")
                && !path.components().any(|c| c.as_os_str() == ".opencode"),
            "only Claude Code agent files can be imported"
        );
        let name = file.name.clone().unwrap_or_else(|| stem.to_owned());
        let id = if validate_id(&name).is_ok() {
            name
        } else {
            let id = project::key_from_dir(Path::new(&name))
                .with_context(|| format!("cannot derive an agent id from name `{name}`"))?;
            notes.push(format!("name `{name}` normalized to id `{id}`"));
            id
        };
        if !file.skills.is_empty() {
            notes.push(
                "native `skills:` dropped; vault skills come from the \"Before starting\" line"
                    .into(),
            );
        }
        let tier = tier_from_model(vault, file.model.as_deref(), &mut notes)?;
        let effort = match file.effort.clone() {
            Some(e) if EFFORTS.contains(&e.as_str()) => Some(e),
            Some(e) => {
                notes.push(format!("effort `{e}` is not valid; dropped"));
                None
            }
            None => None,
        };
        for key in ["permissionMode", "hooks", "mcpServers"] {
            if front_pairs(&text).iter().any(|(k, _)| k == key) {
                notes.push(format!("`{key}` is not imported; it is dropped"));
            }
        }
        let tools = claude_tools(&file, &mut notes);
        (
            id,
            AgentMeta {
                role: "subagent".into(),
                tier,
                effort,
                tools,
                skills: Vec::new(),
            },
            Vec::new(),
            None,
        )
    };

    let (content, line_skills) = strip_skills_line(&file.content);
    let mut meta = meta;
    // Our own file keeps the vault's `skills` (the line omits unavailable ones).
    if !managed && !line_skills.is_empty() {
        meta.skills = line_skills.into_iter().map(|(id, ..)| id).collect();
    } else if edited {
        let mut scratch = Vec::new();
        let mut differs = Vec::new();
        if claude_tools(&file, &mut scratch) != meta.tools {
            differs.push("tools");
        }
        if tier_from_model(vault, file.model.as_deref(), &mut scratch)? != meta.tier {
            differs.push("model");
        }
        if file.effort != meta.effort {
            differs.push("effort");
        }
        let mut expected = Vec::new();
        for skill in &meta.skills {
            if vault
                .skill_unavailable(skill, vault_skills_scope.as_deref())?
                .is_none()
            {
                expected.push(skill.clone());
            }
        }
        let in_file: Vec<String> = line_skills.into_iter().map(|(id, ..)| id).collect();
        if in_file != expected {
            differs.push("skills");
        }
        for field in differs {
            notes.push(format!(
                "`{field}` edit in the file not imported; set it in AGENT.md"
            ));
        }
    }
    let mut description = file
        .description
        .as_deref()
        .unwrap_or_default()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if description.len() > 280 {
        let mut end = 280;
        while !description.is_char_boundary(end) {
            end -= 1;
        }
        description.truncate(end);
        notes.push("description truncated to 280 bytes".into());
    }
    let problems = vault::agent_section_problems(&content);
    for problem in problems {
        notes.push(if problem == "## Output contract" {
            "add a `## Output contract` section describing what the agent returns, then publish"
                .to_owned()
        } else {
            format!("section {problem} is out of order (Role, Procedure, Output contract, Limits); fix it before publishing")
        });
    }

    let text = skill_file::agent_md(
        &id,
        &description,
        &tags,
        scope.is_some().then_some("project"),
        &meta,
        &content,
    ) + "\n";
    Ok(Imported { id, text, notes })
}

/// The tier a Claude Code `model` maps to: exact match on the claude-code mapping, else
/// by family name, else `mid`.
fn tier_from_model(vault: &Vault, model: Option<&str>, notes: &mut Vec<String>) -> Result<String> {
    let Some(model) = model.filter(|m| *m != "inherit") else {
        notes.push("no model; tier set to mid".into());
        return Ok("mid".into());
    };
    let stored = vault.agent_models()?;
    let lower = model.to_ascii_lowercase();
    let exact = TIERS
        .into_iter()
        .find(|tier| model_for(&stored, Target::ClaudeCode, tier).is_some_and(|(m, _)| m == model));
    let family = [("opus", "high"), ("sonnet", "mid"), ("haiku", "low")]
        .into_iter()
        .find(|(name, _)| lower.contains(name))
        .map(|(_, tier)| tier);
    Ok(match exact.or(family) {
        Some(tier) => tier.to_owned(),
        None => {
            notes.push(format!("model `{model}` is unknown; tier set to mid"));
            "mid".into()
        }
    })
}

/// Abstract tools of a Claude Code file's `tools` allowlist and `disallowedTools`;
/// `None` when every tool stays allowed.
fn claude_tools(file: &skill_file::SkillFile, notes: &mut Vec<String>) -> Option<Vec<String>> {
    let mut allowed: Vec<&str> = match &file.tools {
        Some(list) => {
            let mut allowed = vec!["read"];
            for name in list {
                if name.contains('(') && abstract_tool(name).is_some() {
                    notes.push(format!(
                        "tool `{name}` is scoped; imported as unrestricted `{}` access",
                        abstract_tool(name).unwrap_or_default()
                    ));
                }
                match abstract_tool(name) {
                    Some(tool) if !allowed.contains(&tool) => allowed.push(tool),
                    Some(_) => {}
                    None => notes.push(format!("tool `{name}` has no equivalent; ignored")),
                }
            }
            allowed
        }
        None => TOOLS.to_vec(),
    };
    let denied = file.disallowed_tools.as_deref().unwrap_or_default();
    for tool in denied.iter().filter_map(|n| abstract_tool(n)) {
        allowed.retain(|a| *a != tool || tool == "read");
    }
    for (tool, group) in [
        ("edit", &["Edit", "Write", "NotebookEdit"][..]),
        ("web", &["WebFetch", "WebSearch"][..]),
    ] {
        let hit = group
            .iter()
            .filter(|g| denied.iter().any(|d| d == *g))
            .count();
        if hit > 0 && hit < group.len() {
            notes.push(format!(
                "disallowedTools blocks only part of `{tool}`; `{tool}` treated as denied"
            ));
        }
    }
    (allowed.len() < TOOLS.len()).then(|| {
        TOOLS
            .into_iter()
            .filter(|t| allowed.contains(t))
            .map(str::to_owned)
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_must_be_strict_and_match_the_stem() {
        let ok =
            "---\nname: a\n---\n<!-- skillvolution-managed:agent:my-agent:v3:0123abcd -->\nbody\n";
        let marker = parse_marker(ok, "my-agent").unwrap();
        assert_eq!((marker.version, marker.hash.as_str()), (3, "0123abcd"));
        assert!(parse_marker(ok, "other").is_none());
        for bad in [
            "---\nname: a\n---\n\n<!-- skillvolution-managed:agent:my-agent:v3:0123abcd -->\n",
            "---\nname: a\n---\n<!-- skillvolution-managed:agent:my-agent:v3:0123ABCD -->\n",
            "---\nname: a\n---\n<!-- skillvolution-managed:agent:../x:v3:0123abcd -->\n",
            "---\nname: a\n---\n<!-- skillvolution-managed:agent:my-agent:v3:0123abcd -->x\n",
            "name: a\n<!-- skillvolution-managed:agent:my-agent:v3:0123abcd -->\n",
        ] {
            assert!(parse_marker(bad, "my-agent").is_none(), "{bad}");
        }
    }

    #[test]
    fn hash_ignores_markers_line_endings_and_trailing_newline() {
        let a = "---\nx: 1\n---\n<!-- skillvolution-managed:agent:a:v1:00000000 -->\nbody\n";
        let b = "---\r\nx: 1\r\n---\r\nbody";
        assert_eq!(hash8(a), hash8(b));
        // Only the marker at its position is ignored: an extra marker-looking line is an edit.
        let extra = "---\nx: 1\n---\n<!-- skillvolution-managed:agent:a:v1:00000000 -->\n<!-- skillvolution-managed:agent:a:v1:11111111 -->\nbody\n";
        assert_ne!(hash8(a), hash8(extra));
        assert_ne!(hash8(a), hash8("---\nx: 2\n---\nbody\n"));
    }

    #[test]
    fn rendered_files_are_managed_and_every_builtin_parses() {
        for (id, _) in BUILTINS {
            let agent = builtin(id).unwrap();
            for target in Target::ALL {
                let (text, _) = render(target, &agent, None);
                let marker = parse_marker(&text, id).unwrap();
                assert_eq!(marker.hash, hash8(&text));
            }
        }
    }

    #[test]
    fn claude_tools_maps_allowlists_and_partial_denials() {
        let file = skill_file::parse(
            "---\nname: a\ntools: Read, Bash, mcp__x__y\ndisallowedTools: Write\n---\nbody",
        );
        let mut notes = Vec::new();
        assert_eq!(
            claude_tools(&file, &mut notes),
            Some(vec!["read".into(), "shell".into(), "mcp".into()])
        );
        assert!(notes.iter().any(|n| n.contains("only part")), "{notes:?}");
    }
}
