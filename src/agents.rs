//! Subagent files generated from vault agents (and the built-in tier agents) for Claude
//! Code and OpenCode: rendering, the marker that tells our files from the user's, and the
//! `add` / `remove` / `sync` / `import` operations behind `skillvolution agents`.
//!
//! A generated file carries `<!-- skillvolution-managed:agent:<id>:v<N>:<hash8> -->` right
//! after its frontmatter. The hash covers the whole file except marker lines, so a hand
//! edit is detected and never overwritten.

use crate::{
    clients::{AgentSpec, ModelChoice},
    git_exclude, project,
    setup::{self, ClientKind, Edit, MANAGED_MARKER, Scope},
    skill_file,
    vault::{self, AgentMeta, ClientModel, EFFORTS, TIERS, TOOLS, Vault, validate_id},
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

/// A client that supports subagents: a `ClientKind` whose spec has an `AgentSpec`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target(ClientKind);

#[allow(non_upper_case_globals)]
impl Target {
    pub const ClaudeCode: Target = Target(ClientKind::ClaudeCode);
    pub const OpenCode: Target = Target(ClientKind::OpenCode);

    /// Every client with subagents, in registry order.
    pub fn all() -> impl Iterator<Item = Target> {
        ClientKind::with_agents().map(Target)
    }

    pub fn from_token(token: &str) -> Option<Target> {
        Target::all().find(|t| t.token() == token)
    }

    pub fn from_kind(kind: ClientKind) -> Option<Target> {
        Target::all().find(|t| t.0 == kind)
    }

    pub fn token(self) -> &'static str {
        self.0.token()
    }

    pub fn display_name(self) -> &'static str {
        self.0.display_name()
    }

    pub fn kind(self) -> ClientKind {
        self.0
    }

    fn spec(self) -> &'static AgentSpec {
        self.0
            .spec()
            .agents
            .as_ref()
            .expect("a Target is a client with an AgentSpec")
    }

    /// Where this client reads agents: the project's own directory, or the per-user one
    /// when `project` is `None`.
    pub fn dir(self, project: Option<&Path>) -> Result<PathBuf> {
        (self.spec().dirs)(project)
    }
}

/// Tier model compiled in for `target`; stored `client_models` rows override it.
fn default_model(target: Target, tier: &str) -> Option<(&'static str, &'static str)> {
    target
        .spec()
        .default_models
        .iter()
        .find(|(t, ..)| *t == tier)
        .and_then(|(_, model, effort)| Some((*model, (*effort)?)))
}

/// The `(model, effort)` generated files get for `tier` on `target`.
pub fn model_for(
    stored: &[ClientModel],
    target: Target,
    tier: &str,
) -> Option<(String, Option<String>)> {
    tier_models(stored, target, tier)
        .into_iter()
        .next()
        .map(|m| (m.model, m.effort))
}

/// One model a tier can use on a client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TierModel {
    pub model: String,
    /// `None` when unknown (a bare OpenCode model).
    pub provider: Option<String>,
    pub effort: Option<String>,
    /// The stored row's priority; `None` for the compiled default.
    pub priority: Option<i64>,
}

/// The models `tier` can use on `target`, best first: its enabled stored rows, or the
/// compiled default when none is enabled. `model_for` takes the first; an explicit model
/// (`install_agent`) must be one of them.
pub fn tier_models(stored: &[ClientModel], target: Target, tier: &str) -> Vec<TierModel> {
    let rows: Vec<TierModel> = stored
        .iter()
        .filter(|m| m.enabled && m.client == target.token() && m.tier == tier)
        .map(|m| TierModel {
            model: m.model.clone(),
            provider: m.provider().map(str::to_owned),
            effort: m.effort.clone(),
            priority: Some(m.priority),
        })
        .collect();
    if !rows.is_empty() {
        return rows;
    }
    default_model(target, tier)
        .map(|(model, effort)| TierModel {
            model: model.to_owned(),
            provider: vault::implied_provider(target.token(), model),
            effort: Some(effort.to_owned()),
            priority: None,
        })
        .into_iter()
        .collect()
}

/// The effort `models set` keeps when `--effort` is not given: the stored row's, else the
/// compiled tier default.
pub fn current_effort(stored: &[ClientModel], client: &str, tier: &str) -> Option<String> {
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

/// The `agents models` table: a header, then one aligned line per stored model of each
/// client and tier (best first), plus the compiled default when no enabled row applies.
pub fn models_table(stored: &[ClientModel]) -> Vec<String> {
    let mut rows = vec![["client", "tier", "model", "effort", "origin"].map(str::to_owned)];
    for kind in ClientKind::with_agents() {
        let (client, target) = (kind.token(), Target::from_kind(kind));
        for tier in TIERS {
            let mut any_enabled = false;
            for m in stored
                .iter()
                .filter(|m| m.client == client && m.tier == tier)
            {
                any_enabled |= m.enabled;
                let mut origin = vec!["set".to_owned()];
                if m.provider().is_none() {
                    origin.push("unknown provider".into());
                }
                if m.priority != 0 {
                    origin.push(format!("priority {}", m.priority));
                }
                if !m.enabled {
                    origin.push("disabled".into());
                }
                rows.push([
                    client.to_owned(),
                    tier.to_owned(),
                    m.model.clone(),
                    m.effort.clone().unwrap_or_else(|| "-".into()),
                    format!("({})", origin.join(", ")),
                ]);
            }
            if any_enabled {
                continue;
            }
            let mapped = target.and_then(|t| model_for(&[], t, tier));
            let (model, effort) = mapped.clone().unwrap_or(("-".into(), None));
            rows.push([
                client.to_owned(),
                tier.to_owned(),
                model,
                effort.unwrap_or_else(|| "-".into()),
                if mapped.is_some() {
                    "(default)"
                } else {
                    "(none)"
                }
                .to_owned(),
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

pub(crate) fn same_dir(a: &Path, b: &Path) -> bool {
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
    render_pinned(target, agent, model, None)
}

/// `render`, recording `pin` (a model chosen explicitly at install) in the marker as
/// `:model=<pin>`, so `sync` keeps it. The marker line is outside the hash.
fn render_pinned(
    target: Target,
    agent: &Agent,
    model: Option<(String, Option<String>)>,
    pin: Option<&str>,
) -> (String, Vec<String>) {
    let (front, mut warnings) = (target.spec().render)(agent, model.as_ref());
    if model.is_none() {
        warnings.push(format!(
            "no {} model for tier {}; the client inherits its own (set one with \
             `skillvolution agents models set {} {} <model>`)",
            target.display_name(),
            agent.meta.tier,
            target.token(),
            agent.meta.tier
        ));
    }
    let front = front.join("\n");
    let body = body(agent);
    let hash = hash8(&format!("---\n{front}\n---\n{body}"));
    let pin = pin.map(|m| format!(":model={m}")).unwrap_or_default();
    let marker = format!(
        "{MARKER_PREFIX}{}:v{}:{hash}{pin} -->",
        agent.id, agent.version
    );
    (format!("---\n{front}\n---\n{marker}\n{body}"), warnings)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Marker {
    pub id: String,
    pub version: i64,
    pub hash: String,
    /// The model pinned at install (`:model=<model>`), which `sync` keeps while the tier
    /// still has it enabled.
    pub pin: Option<String>,
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
    pub fn marker(&self) -> Option<&Marker> {
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
    // The pin comes last and may itself contain ':' (`ollama/llama3:8b`).
    let mut parts = inner.splitn(4, ':');
    let (id, version, hash) = (parts.next()?, parts.next()?, parts.next()?);
    let pin = match parts.next() {
        None => None,
        Some(rest) => Some(
            rest.strip_prefix("model=")
                .filter(|m| !m.is_empty() && !m.contains(char::is_whitespace))?
                .to_owned(),
        ),
    };
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
        pin,
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

/// Whether the installed file with `marker` is already exactly `plan` of an agent at
/// `version` (content, version and pin).
fn is_current(marker: &Marker, version: i64, plan: &Plan) -> bool {
    marker.version == version && marker.hash == hash8(&plan.text) && marker.pin == plan.pin
}

/// A file to write for one client.
pub(crate) struct Plan {
    target: Target,
    path: PathBuf,
    text: String,
    /// The model pinned in its marker.
    pin: Option<String>,
    warnings: Vec<String>,
}

fn plan(
    target: Target,
    agent: &Agent,
    project_dir: Option<&Path>,
    model: Option<ModelChoice>,
    pin: Option<String>,
) -> Result<Plan> {
    let (text, warnings) = render_pinned(target, agent, model, pin.as_deref());
    Ok(Plan {
        target,
        path: target.dir(project_dir)?.join(format!("{}.md", agent.id)),
        text,
        pin,
        warnings,
    })
}

/// The file `agent` should have for `target` given the installed file's `marker`: the
/// model pinned there while it is still one of the tier's models, else the tier's best
/// (with a warning when a pin is dropped). What `sync` writes and `search_agents` compares
/// against; the one place that decides what an installed file is expected to be.
pub(crate) fn expected_plan(
    models: &[ClientModel],
    target: Target,
    agent: &Agent,
    project_dir: Option<&Path>,
    marker: Option<&Marker>,
) -> Result<Plan> {
    let tier = &agent.meta.tier;
    let Some(pin) = marker.and_then(|m| m.pin.as_deref()) else {
        return plan(
            target,
            agent,
            project_dir,
            model_for(models, target, tier),
            None,
        );
    };
    if let Some(found) = tier_models(models, target, tier)
        .into_iter()
        .find(|m| m.model == pin)
    {
        let model = Some((found.model, found.effort));
        return plan(target, agent, project_dir, model, Some(pin.to_owned()));
    }
    let mut fallback = plan(
        target,
        agent,
        project_dir,
        model_for(models, target, tier),
        None,
    )?;
    fallback.warnings.insert(
        0,
        format!(
            "pinned model {pin} is no longer enabled for {} {tier}; using the tier's preferred model",
            target.display_name()
        ),
    );
    Ok(fallback)
}

/// Whether the installed file in `state` lags `agent`: a managed file that differs from
/// `expected_plan`, or a hand-edited one carrying an older version.
pub fn is_outdated(
    models: &[ClientModel],
    target: Target,
    agent: &Agent,
    project_dir: Option<&Path>,
    state: &FileState,
) -> bool {
    match state {
        FileState::Managed(marker) => {
            expected_plan(models, target, agent, project_dir, Some(marker))
                .is_ok_and(|plan| !is_current(marker, agent.version, &plan))
        }
        FileState::Edited(marker) => marker.version < agent.version,
        _ => false,
    }
}

/// `global` when `target`'s files for `project_dir` land in its per-user directory (no
/// project, or the project is that directory's owner such as $HOME), else `project`: the
/// scope `agent_*` events record.
pub fn install_scope(target: Target, project_dir: Option<&Path>) -> &'static str {
    let per_user = match (project_dir, target.dir(None)) {
        (None, _) => true,
        (Some(dir), Ok(user)) => target.dir(Some(dir)).is_ok_and(|d| same_dir(&d, &user)),
        (Some(_), Err(_)) => false,
    };
    if per_user { "global" } else { "project" }
}

/// The clients with subagents set up per user or, with `project_dir`, in that project.
pub fn configured_targets(project_dir: Option<&Path>) -> Vec<Target> {
    Target::all()
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
    Ok(add_with_changes(vault, ids, project_key, project_dir, force)?.0)
}

/// `add`, also returning the files it wrote.
pub fn add_with_changes(
    vault: &Vault,
    ids: &[String],
    project_key: Option<&str>,
    project_dir: Option<&Path>,
    force: bool,
) -> Result<(Vec<String>, Vec<AgentChange>)> {
    let options = InstallOptions {
        force,
        ..InstallOptions::default()
    };
    let report = install(vault, ids, project_key, project_dir, &options)?;
    Ok((report.lines, report.changes))
}

/// What `install` does beyond `add`'s defaults.
#[derive(Default)]
pub struct InstallOptions<'a> {
    /// Overwrite foreign and hand-edited files after a backup.
    pub force: bool,
    /// Write only for this client instead of every configured one.
    pub client: Option<Target>,
    /// This model instead of the tier's best one; it must be one of `tier_models` on
    /// every client written.
    pub model: Option<&'a str>,
}

/// What `install` did: `add`'s output lines, the files it wrote (for the `agent_*`
/// events), every file it considered, and the notes among its lines (warnings, the
/// shell/edit and Cursor notes, an exclude that could not be written) as plain text.
pub struct InstallReport {
    pub lines: Vec<String>,
    pub changes: Vec<AgentChange>,
    pub files: Vec<InstalledFile>,
    pub notes: Vec<String>,
}

/// One agent file `install` considered.
#[derive(Debug)]
pub struct InstalledFile {
    pub target: Target,
    pub id: String,
    pub version: i64,
    pub path: PathBuf,
    /// `added`, `updated`, `overwritten`, `unchanged`, or `edited` (hand-edited, skipped).
    pub status: &'static str,
    /// The model written in the file, if any.
    pub model: Option<String>,
}

/// The model `target` writes for `tier`: the tier's best, or `requested` when it is one of
/// the tier's models there.
fn chosen_model(
    stored: &[ClientModel],
    target: Target,
    tier: &str,
    requested: Option<&str>,
) -> Result<Option<ModelChoice>> {
    let Some(requested) = requested else {
        return Ok(model_for(stored, target, tier));
    };
    let models = tier_models(stored, target, tier);
    if let Some(found) = models.iter().find(|m| m.model == requested) {
        return Ok(Some((found.model.clone(), found.effort.clone())));
    }
    if models.is_empty() {
        bail!(
            "{} has no {tier} model configured, so `{requested}` cannot be used; add one with \
             `skillvolution agents models add {} {tier} {requested}`",
            target.display_name(),
            target.token()
        );
    }
    bail!(
        "`{requested}` is not an enabled {} {tier} model; use one of: {} (see list_models)",
        target.display_name(),
        models
            .iter()
            .map(|m| m.model.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// `add` with options: the code behind `agents add` and the MCP `install_agent`.
pub fn install(
    vault: &Vault,
    ids: &[String],
    project_key: Option<&str>,
    project_dir: Option<&Path>,
    options: &InstallOptions,
) -> Result<InstallReport> {
    let force = options.force;
    let targets = match options.client {
        Some(target) => vec![target],
        None => configured_targets(project_dir),
    };
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
    let models = vault.client_models()?;
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
    let mut choices = Vec::new();
    for agent in &agents {
        for &target in &targets {
            let choice = chosen_model(&models, target, &agent.meta.tier, options.model)?;
            choices.push((target, agent.id.clone(), choice));
        }
    }
    let choice = |target: Target, id: &str| {
        choices
            .iter()
            .find(|(t, i, _)| *t == target && i == id)
            .and_then(|(.., choice)| choice.clone())
    };
    let (mut lines, mut edits, mut backups) = (Vec::new(), Vec::new(), Vec::new());
    let (mut changes, mut files, mut notes) = (Vec::new(), Vec::new(), Vec::new());
    // A note goes to the printed lines (indented as given) and, trimmed, to `notes`.
    let mut note = |lines: &mut Vec<String>, line: String| {
        notes.push(line.trim().to_owned());
        lines.push(line);
    };
    for agent in &agents {
        for warning in skill_warnings(vault, agent) {
            note(&mut lines, warning);
        }
        if agent.deprecated {
            note(
                &mut lines,
                format!(
                    "[deprecated] {}: deprecated in the vault; not added",
                    agent.id
                ),
            );
            continue;
        }
        if !agent.meta.skills.is_empty()
            && (agent.meta.allows("shell") || agent.meta.allows("edit"))
        {
            note(
                &mut lines,
                format!(
                    "note: {}: its vault skills run with its shell/edit tools and stay pinned to the versions listed in the file until `skillvolution agents sync --apply`",
                    agent.id
                ),
            );
        }
        for &target in &targets {
            if let Some(problem) = placement_error(target, agent, project_dir) {
                bail!("{problem}");
            }
            let model = choice(target, &agent.id);
            let written_model = model.as_ref().map(|(m, _)| m.clone());
            // An explicit model is pinned in the marker; without one any pin is cleared.
            let pin = options.model.map(str::to_owned);
            let plan = plan(target, agent, project_dir, model, pin)?;
            let mut file = InstalledFile {
                target,
                id: agent.id.clone(),
                version: agent.version,
                path: plan.path.clone(),
                status: "unchanged",
                model: written_model,
            };
            let (tag, kind) = match file_state(&plan.path)? {
                state @ (FileState::Foreign | FileState::Edited(_)) if force => {
                    let kind = if matches!(state, FileState::Foreign) {
                        "agent_install"
                    } else {
                        "agent_update"
                    };
                    // `fs_safe::write` skips its own backup when old and new both carry
                    // the marker (an edited file, or a renamed copy of a generated one).
                    if fs::read(&plan.path).is_ok_and(|old| contains_marker(&old)) {
                        backups.push(plan.path.clone());
                    }
                    note(&mut lines, format!("  {}", backup_note(&plan.path)));
                    ("overwritten", kind)
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
                    file.status = "edited";
                    files.push(file);
                    continue;
                }
                FileState::Managed(m) if is_current(&m, agent.version, &plan) => {
                    lines.push(format!(
                        "[unchanged] {} {} v{}",
                        target.display_name(),
                        agent.id,
                        agent.version
                    ));
                    if let Some(warning) = double_install_warning(target, &agent.id, project_dir) {
                        note(&mut lines, warning);
                    }
                    files.push(file);
                    continue;
                }
                FileState::Managed(_) => ("updated", "agent_update"),
                FileState::Missing => ("added", "agent_install"),
            };
            changes.push(AgentChange {
                kind,
                target,
                id: agent.id.clone(),
                version: Some(agent.version),
            });
            lines.push(format!(
                "[{tag}] {} {} v{} -> {}",
                plan.target.display_name(),
                agent.id,
                agent.version,
                plan.path.display()
            ));
            for warning in &plan.warnings {
                note(&mut lines, format!("  warning: {warning}"));
            }
            if let Some(warning) = double_install_warning(target, &agent.id, project_dir) {
                note(&mut lines, warning);
            }
            edits.push(Edit::Write(plan.path, plan.text));
            file.status = tag;
            files.push(file);
        }
    }
    setup::validate_edits(&edits)?;
    backups
        .iter()
        .try_for_each(|path| setup::backup_file(path))?;
    setup::write_all(edits)?;
    let ours: Vec<PathBuf> = files
        .iter()
        .filter(|f| f.status != "edited")
        .map(|f| f.path.clone())
        .collect();
    if let Some(excluded) = exclude_from_git(project_dir, &ours, true) {
        note(&mut lines, excluded);
    }
    if targets.contains(&Target::ClaudeCode) {
        note(
            &mut lines,
            "note: Cursor also reads .claude/agents/ and may not accept Claude Code model aliases"
                .into(),
        );
    }
    Ok(InstallReport {
        lines,
        changes,
        files,
        notes,
    })
}

/// Adds (or, with `add` false, removes) the `.git/info/exclude` lines of `paths` that lie
/// in a project's own agent directories; per-user files are never excluded. A note when
/// that could not be done.
fn exclude_from_git(project_dir: Option<&Path>, paths: &[PathBuf], add: bool) -> Option<String> {
    let dir = project_dir?;
    let user_dirs: Vec<PathBuf> = Target::all().filter_map(|t| t.dir(None).ok()).collect();
    let in_project: Vec<PathBuf> = paths
        .iter()
        .filter(|path| {
            path.parent()
                .is_some_and(|parent| !user_dirs.iter().any(|user| same_dir(parent, user)))
        })
        .cloned()
        .collect();
    if add {
        git_exclude::add(dir, &in_project)
    } else {
        git_exclude::remove(dir, &in_project)
    }
}

/// Deletes our unedited generated files for `ids` in every client's directory. The
/// count is how many ids had nothing removed (not installed, or only foreign/edited files).
pub fn remove(ids: &[String], project_dir: Option<&Path>) -> Result<(Vec<String>, usize)> {
    let (lines, missing, _) = remove_with_changes(ids, project_dir)?;
    Ok((lines, missing))
}

/// `remove`, also returning the files it deleted.
pub fn remove_with_changes(
    ids: &[String],
    project_dir: Option<&Path>,
) -> Result<(Vec<String>, usize, Vec<AgentChange>)> {
    let (mut lines, mut edits, mut missing) = (Vec::new(), Vec::new(), 0);
    let mut changes = Vec::new();
    for id in ids {
        validate_id(id)?;
        let (mut found, mut removed) = (false, false);
        for target in Target::all() {
            let path = target.dir(project_dir)?.join(format!("{id}.md"));
            let name = target.display_name();
            match file_state(&path)? {
                FileState::Missing => continue,
                FileState::Managed(_) => {
                    lines.push(format!("[removed] {name} {id} -> {}", path.display()));
                    edits.push(Edit::Delete(path));
                    changes.push(AgentChange {
                        kind: "agent_remove",
                        target,
                        id: id.clone(),
                        version: None,
                    });
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
    let deleted: Vec<PathBuf> = edits.iter().map(|e| e.path().to_path_buf()).collect();
    setup::write_all(edits)?;
    lines.extend(exclude_from_git(project_dir, &deleted, false));
    Ok((lines, missing, changes))
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

/// Whether going from installed revision `old` to `agent` widens what the agent may do in a
/// way `flags` reports and the user should see before the file is rewritten unattended: a
/// tier change, added skills or widened tools, or a previous revision that cannot be
/// compared against.
fn needs_review(vault: &Vault, old: i64, agent: &Agent) -> bool {
    if old == agent.version || builtin(&agent.id).is_some() {
        return false;
    }
    let Ok(previous) = vault.inspect(&agent.id, old) else {
        return true;
    };
    let Some(previous) = &previous.meta else {
        return false;
    };
    previous.tier != agent.meta.tier
        || agent
            .meta
            .skills
            .iter()
            .any(|s| !previous.skills.contains(s))
        || TOOLS
            .into_iter()
            .any(|t| agent.meta.allows(t) && !previous.allows(t))
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
    Ok(sync_with_changes(vault, project_key, project_dir, apply, force)?.0)
}

/// `sync`, also returning the files it rewrote (none on a dry run).
pub fn sync_with_changes(
    vault: &Vault,
    project_key: Option<&str>,
    project_dir: Option<&Path>,
    apply: bool,
    force: bool,
) -> Result<(Vec<String>, Vec<AgentChange>)> {
    let models = vault.client_models()?;
    let mut changes = Vec::new();
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
            let plan = expected_plan(&models, target, &agent, project_dir, Some(marker))?;
            if !edited && is_current(marker, agent.version, &plan) {
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
            changes.push(AgentChange {
                kind: "agent_update",
                target,
                id: id.clone(),
                version: Some(agent.version),
            });
            edits.push(Edit::Write(plan.path, plan.text));
        }
    }
    let updates = edits.len();
    if apply {
        let written: Vec<PathBuf> = edits.iter().map(|e| e.path().to_path_buf()).collect();
        setup::validate_edits(&edits)?;
        backups
            .iter()
            .try_for_each(|path| setup::backup_file(path))?;
        setup::write_all(edits)?;
        lines.extend(exclude_from_git(project_dir, &written, true));
        lines.push(format!("{updates} file(s) updated, {unchanged} unchanged."));
    } else {
        lines.push(format!("{unchanged} unchanged."));
        if updates > 0 {
            lines.push(format!(
                "dry run: re-run with --apply to write {updates} file(s)"
            ));
        }
    }
    if !apply {
        changes.clear();
    }
    Ok((lines, changes))
}

/// The directories a session start looks after for `target`: the project's (when it is
/// not the per-user one, e.g. a repository at $HOME) with `project_dir`, then the
/// per-user one with `None`.
pub(crate) fn session_levels(
    target: Target,
    project_dir: Option<&Path>,
) -> Vec<(PathBuf, Option<&Path>)> {
    let Ok(user_dir) = target.dir(None) else {
        return Vec::new();
    };
    let project = project_dir.and_then(|dir| {
        target
            .dir(Some(dir))
            .ok()
            .filter(|d| !same_dir(d, &user_dir))
            .map(|d| (d, Some(dir)))
    });
    project.into_iter().chain([(user_dir, None)]).collect()
}

/// What a session-start refresh did.
#[derive(Debug, Default)]
pub struct Refresh {
    /// Each file rewritten, with its scope (`project` or `global`).
    pub updated: Vec<(AgentChange, &'static str)>,
    /// The agents left at their installed version because `sync` flags the new one for
    /// review (`needs_review`): client, id and the vault's version, once per client.
    pub needs_review: Vec<(Target, String, i64)>,
}

/// Session-start refresh: rewrites, with `expected_plan`, the managed agent files of the
/// project and the per-user directories (every client) that `is_outdated`. Hand-edited
/// and foreign files are never touched, nor files whose agent is gone, deprecated or
/// misplaced, nor a new version that changes the tier, adds skills or widens the tools
/// (`needs_review`: those wait for `agents sync`).
pub fn refresh_outdated(
    vault: &Vault,
    project_key: Option<&str>,
    project_dir: Option<&Path>,
) -> Result<Refresh> {
    let models = vault.client_models()?;
    let mut refresh = Refresh::default();
    for target in Target::all() {
        for (dir, level) in session_levels(target, project_dir) {
            let key = level.and(project_key);
            let mut edits = Vec::new();
            for (_, state) in managed_files(&dir) {
                let FileState::Managed(marker) = &state else {
                    continue;
                };
                let Ok(agent) = resolve(vault, &marker.id, key) else {
                    continue;
                };
                if agent.deprecated || placement_error(target, &agent, level).is_some() {
                    continue;
                }
                if !is_outdated(&models, target, &agent, level, &state) {
                    continue;
                }
                if needs_review(vault, marker.version, &agent) {
                    let held = (target, agent.id.clone(), agent.version);
                    if !refresh.needs_review.contains(&held) {
                        refresh.needs_review.push(held);
                    }
                    continue;
                }
                let plan = expected_plan(&models, target, &agent, level, Some(marker))?;
                edits.push(Edit::Write(plan.path, plan.text));
                refresh.updated.push((
                    AgentChange {
                        kind: "agent_update",
                        target,
                        id: agent.id.clone(),
                        version: Some(agent.version),
                    },
                    install_scope(target, level),
                ));
            }
            if edits.is_empty() {
                continue;
            }
            let written: Vec<PathBuf> = edits.iter().map(|e| e.path().to_path_buf()).collect();
            setup::validate_edits(&edits)?;
            setup::write_all(edits)?;
            exclude_from_git(level, &written, true);
        }
    }
    Ok(refresh)
}

/// Deletes the project copies of `ids` that are managed (never hand-edited) and whose
/// client already has our copy in its per-user directory, with their
/// `.git/info/exclude` lines: what a promoted agent's project copies become.
pub fn retire_project_copies(ids: &[String], project_dir: &Path) -> Result<Vec<AgentChange>> {
    let (mut edits, mut changes) = (Vec::new(), Vec::new());
    for target in Target::all() {
        let Some((dir, _)) = session_levels(target, Some(project_dir))
            .into_iter()
            .find(|(_, level)| level.is_some())
        else {
            continue;
        };
        let user_dir = target.dir(None)?;
        for id in ids {
            validate_id(id)?;
            let path = dir.join(format!("{id}.md"));
            let per_user = file_state(&user_dir.join(format!("{id}.md")))?;
            if per_user.marker().is_some() && matches!(file_state(&path)?, FileState::Managed(_)) {
                edits.push(Edit::Delete(path));
                changes.push(AgentChange {
                    kind: "agent_remove",
                    target,
                    id: id.clone(),
                    version: None,
                });
            }
        }
    }
    let deleted: Vec<PathBuf> = edits.iter().map(|e| e.path().to_path_buf()).collect();
    setup::write_all(edits)?;
    exclude_from_git(Some(project_dir), &deleted, false);
    Ok(changes)
}

/// One agent file written or removed, for the `agent_*` usage events.
///
/// Classification: a file that did not exist, or a foreign (not generated) one that
/// `--force` replaced, is an `agent_install` (a managed file now exists where none did);
/// replacing a managed or hand-edited generated file is an `agent_update`. `version` is
/// the version written (`None` for a removal).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentChange {
    pub kind: &'static str,
    pub target: Target,
    pub id: String,
    pub version: Option<i64>,
}

/// Records an `agent_install`, `agent_update` or `agent_remove` event for each of
/// `changes`. `scope` is `global` or `project`.
pub fn log_agent_events(
    vault: &Vault,
    changes: &[AgentChange],
    scope: &str,
    project: Option<&str>,
) {
    for change in changes {
        vault.log_event(
            crate::vault::Event::new(change.kind)
                .client(Some(change.target.kind()))
                .project(project)
                .subject(&change.id, change.version)
                .data(serde_json::json!({"scope": scope})),
        );
    }
}

/// A note for `setup --remove`: managed agent files its client leaves in `scope`.
pub(crate) fn left_behind_note(kind: ClientKind, scope: Scope) -> Option<String> {
    let target = Target::from_kind(kind)?;
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
    let stored = vault.client_models()?;
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
    fn a_pinned_model_round_trips_through_the_marker_without_touching_the_hash() {
        let agent = builtin("sv-mid").unwrap();
        let model = Some(("ollama/llama3:8b".to_owned(), None));
        let (pinned, _) = render_pinned(
            Target::OpenCode,
            &agent,
            model.clone(),
            Some("ollama/llama3:8b"),
        );
        let (plain, _) = render(Target::OpenCode, &agent, model);
        let marker = parse_marker(&pinned, "sv-mid").unwrap();
        assert_eq!(marker.pin.as_deref(), Some("ollama/llama3:8b"));
        assert_eq!(marker.hash, hash8(&pinned));
        assert_eq!(hash8(&pinned), hash8(&plain));
        assert_eq!(parse_marker(&plain, "sv-mid").unwrap().pin, None);
        for bad in [":model=", ":model=a b", ":other=x"] {
            let text = format!(
                "---\nname: a\n---\n<!-- skillvolution-managed:agent:my-agent:v3:0123abcd{bad} -->\n"
            );
            assert!(parse_marker(&text, "my-agent").is_none(), "{text}");
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
            for target in Target::all() {
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
