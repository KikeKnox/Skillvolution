//! Agent entries: the metadata an agent revision carries, its validation, and
//! the machine-local tier-to-model mapping (`agent_models`).

use super::{Vault, validate_id, validate_single_line};
use anyhow::{Result, ensure};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

pub const TIERS: [&str; 3] = ["high", "mid", "low"];
/// Claude Code's effort levels (it has no `minimal`).
pub const EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];
/// Abstract tools an agent may ask for; each client maps them to its own.
pub const TOOLS: [&str; 5] = ["read", "edit", "shell", "web", "mcp"];
/// Client built-in agent names and our managed skills; a vault agent with one of these ids
/// would shadow the client's native agent or a managed skill.
const RESERVED_AGENT_IDS: [&str; 13] = [
    "general-purpose",
    "statusline-setup",
    "claude-code-guide",
    "explore",
    "plan",
    "build",
    "general",
    "compaction",
    "title",
    "summary",
    "evolution",
    "skill-evaluation",
    "skill-acquisition",
];

/// Why no vault agent may use `id` (agents only; skills may), or `None` when it is free.
pub fn reserved_agent_id(id: &str) -> Option<&'static str> {
    if id.starts_with("sv-") {
        Some("the `sv-` prefix is reserved for built-in agents")
    } else if RESERVED_AGENT_IDS.contains(&id) {
        Some("it would shadow a client built-in agent or a managed skill")
    } else {
        None
    }
}
/// Clients `agent_models` can hold a mapping for.
pub const MODEL_CLIENTS: [&str; 2] = ["claude-code", "opencode"];
/// Headings an agent body may use, in this order; only `## Output contract`
/// is required (it is what the orchestrator gets back).
pub const AGENT_SECTIONS: [&str; 4] =
    ["## Role", "## Procedure", "## Output contract", "## Limits"];
const REQUIRED_AGENT_SECTION: &str = "## Output contract";
const MAX_AGENT_SKILLS: usize = 8;

/// What an agent revision declares besides its prompt, stored as JSON in
/// `revisions.meta`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentMeta {
    /// Only `subagent` for now; `primary` is reserved.
    pub role: String,
    pub tier: String,
    /// Overrides the effort the tier's mapping assigns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// Abstract tools (`TOOLS`); `None` means every tool.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<String>>,
    /// Vault skills the agent loads with `get_skill` before starting.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<String>,
}

impl AgentMeta {
    /// Whether the agent may use abstract tool `tool` (`read` always).
    pub fn allows(&self, tool: &str) -> bool {
        tool == "read"
            || self
                .tools
                .as_ref()
                .is_none_or(|tools| tools.iter().any(|t| t == tool))
    }
}

/// Every problem with `meta`, for a human to fix all at once.
pub fn agent_meta_problems(meta: &AgentMeta) -> Vec<String> {
    let mut problems = Vec::new();
    match meta.role.as_str() {
        "subagent" => {}
        "primary" => problems.push("role: primary is not supported yet; use subagent".into()),
        other => problems.push(format!("role: `{other}` is not subagent")),
    }
    if meta.tier.is_empty() {
        problems.push("tier is required (high, mid or low)".to_string());
    } else if !TIERS.contains(&meta.tier.as_str()) {
        problems.push(format!(
            "tier: `{}` is not one of {}",
            meta.tier,
            TIERS.join(", ")
        ));
    }
    if let Some(effort) = &meta.effort
        && !EFFORTS.contains(&effort.as_str())
    {
        problems.push(format!(
            "effort: `{effort}` is not one of {}",
            EFFORTS.join(", ")
        ));
    }
    for tool in meta.tools.iter().flatten() {
        if !TOOLS.contains(&tool.as_str()) {
            problems.push(format!(
                "tools: `{tool}` is not one of {}",
                TOOLS.join(", ")
            ));
        }
    }
    if meta.skills.len() > MAX_AGENT_SKILLS {
        problems.push(format!("skills: at most {MAX_AGENT_SKILLS} are allowed"));
    }
    for skill in &meta.skills {
        if validate_id(skill).is_err() {
            problems.push(format!("skills: `{skill}` is not a valid skill id"));
        }
    }
    if !meta.skills.is_empty() && !meta.allows("mcp") {
        problems.push("skills: loading vault skills needs the `mcp` tool; add it to tools".into());
    }
    problems
}

/// The agent headings that are out of place: `## Output contract` when absent,
/// else any of `AGENT_SECTIONS` appearing out of order.
pub fn agent_section_problems(content: &str) -> Vec<&'static str> {
    let present: Vec<&'static str> = content
        .lines()
        .map(str::trim)
        .filter_map(|line| {
            AGENT_SECTIONS
                .into_iter()
                .find(|s| line.eq_ignore_ascii_case(s))
        })
        .collect();
    if !present.contains(&REQUIRED_AGENT_SECTION) {
        return vec![REQUIRED_AGENT_SECTION];
    }
    let rank = |s: &str| AGENT_SECTIONS.iter().position(|a| *a == s).unwrap();
    present
        .windows(2)
        .filter(|pair| rank(pair[0]) >= rank(pair[1]))
        .map(|pair| pair[1])
        .collect()
}

/// Every problem with an agent revision's `content` and `meta`, ready to show.
pub fn agent_content_problems(content: &str, meta: &AgentMeta) -> Vec<String> {
    let sections = agent_section_problems(content);
    let mut problems = Vec::new();
    if !sections.is_empty() {
        problems.push(format!(
            "agent content must contain {REQUIRED_AGENT_SECTION}, and any of {} in that order; missing or out of order: {}",
            AGENT_SECTIONS.join(", "),
            sections.join(", ")
        ));
    }
    problems.extend(agent_meta_problems(meta));
    problems
}

pub(super) fn validate_agent(content: &str, meta: &AgentMeta) -> Result<()> {
    let problems = agent_content_problems(content, meta);
    ensure!(problems.is_empty(), "{}", problems.join("; "));
    Ok(())
}

/// Why vault skill `id` cannot be used by an agent seen from `project` (the agent's own
/// scope), or `None` when it is a published, visible skill.
pub fn skill_unavailable(
    conn: &Connection,
    id: &str,
    project: Option<&str>,
) -> Result<Option<&'static str>> {
    let row: Option<(String, Option<String>)> = conn
        .query_row(
            "SELECT kind, scope FROM current_skills WHERE id = ?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    Ok(match row {
        None => Some("missing or purged"),
        Some((kind, _)) if kind != "skill" => Some("is an agent, not a skill"),
        Some((_, Some(scope))) if Some(scope.as_str()) != project => Some(if project.is_none() {
            "project-scoped; a global agent can only use global skills (or give the agent `scope: project`)"
        } else {
            "project-scoped to another project"
        }),
        _ => None,
    })
}

/// Whether vault skill `id` is deprecated (still usable by agents, but worth a warning).
pub fn skill_deprecated(conn: &Connection, id: &str) -> Result<bool> {
    Ok(conn
        .query_row("SELECT deprecated FROM skills WHERE id = ?1", [id], |row| {
            row.get::<_, bool>(0)
        })
        .optional()?
        .unwrap_or(false))
}

impl Vault {
    /// Whether `skill` is deprecated in the vault.
    pub fn skill_deprecated(&self, skill: &str) -> Result<bool> {
        skill_deprecated(&self.conn, skill)
    }

    /// Why `skill` is not available to an agent visible from `project`, if it is not.
    pub fn skill_unavailable(
        &self,
        skill: &str,
        project: Option<&str>,
    ) -> Result<Option<&'static str>> {
        skill_unavailable(&self.conn, skill, project)
    }
}

/// Parses `revisions.meta` as read from a row.
pub(super) fn parse_meta(text: Option<String>) -> rusqlite::Result<Option<AgentMeta>> {
    text.map(|text| {
        serde_json::from_str(&text).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
        })
    })
    .transpose()
}

/// One stored `agent_models` row.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AgentModel {
    pub client: String,
    pub tier: String,
    pub model: String,
    pub effort: Option<String>,
}

impl Vault {
    /// Every stored tier mapping, ordered by client then tier.
    pub fn agent_models(&self) -> Result<Vec<AgentModel>> {
        let mut statement = self.conn.prepare(
            "SELECT client, tier, model, effort FROM agent_models ORDER BY client, tier",
        )?;
        Ok(statement
            .query_map([], |row| {
                Ok(AgentModel {
                    client: row.get(0)?,
                    tier: row.get(1)?,
                    model: row.get(2)?,
                    effort: row.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?)
    }

    /// Maps `tier` to `model` (and optionally `effort`) for `client`.
    pub fn set_agent_model(
        &self,
        client: &str,
        tier: &str,
        model: &str,
        effort: Option<&str>,
    ) -> Result<()> {
        ensure!(
            MODEL_CLIENTS.contains(&client),
            "client must be one of {}",
            MODEL_CLIENTS.join(", ")
        );
        ensure!(
            TIERS.contains(&tier),
            "tier must be one of {}",
            TIERS.join(", ")
        );
        validate_single_line("model", model, 128)?;
        ensure!(
            !model.contains(char::is_whitespace),
            "model must not contain whitespace"
        );
        if let Some(effort) = effort {
            ensure!(
                EFFORTS.contains(&effort),
                "effort must be one of {}",
                EFFORTS.join(", ")
            );
        }
        self.conn.execute(
            "INSERT INTO agent_models (client, tier, model, effort) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(client, tier) DO UPDATE SET model = excluded.model, effort = excluded.effort",
            params![client, tier, model, effort],
        )?;
        Ok(())
    }

    /// Deletes the stored mapping of `tier` for `client`, so it reverts to the compiled
    /// default (or to none); `false` when there was no row.
    pub fn unset_agent_model(&self, client: &str, tier: &str) -> Result<bool> {
        ensure!(
            MODEL_CLIENTS.contains(&client),
            "client must be one of {}",
            MODEL_CLIENTS.join(", ")
        );
        ensure!(
            TIERS.contains(&tier),
            "tier must be one of {}",
            TIERS.join(", ")
        );
        let removed = self.conn.execute(
            "DELETE FROM agent_models WHERE client = ?1 AND tier = ?2",
            params![client, tier],
        )?;
        Ok(removed > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta() -> AgentMeta {
        AgentMeta {
            role: "subagent".into(),
            tier: "mid".into(),
            effort: None,
            tools: None,
            skills: vec![],
        }
    }

    #[test]
    fn output_contract_is_required_and_sections_keep_their_order() {
        assert_eq!(agent_section_problems("## Role\nx"), ["## Output contract"]);
        assert!(agent_section_problems("intro\n## Output contract\nx").is_empty());
        assert!(
            agent_section_problems("## Role\n## Procedure\n## Output contract\n## Limits")
                .is_empty()
        );
        assert_eq!(
            agent_section_problems("## Output contract\n## Role"),
            ["## Role"]
        );
    }

    #[test]
    fn meta_rejects_primary_unknown_values_and_skills_without_mcp() {
        assert!(agent_meta_problems(&meta()).is_empty());
        let primary = AgentMeta {
            role: "primary".into(),
            ..meta()
        };
        assert!(agent_meta_problems(&primary)[0].contains("not supported yet"));
        let bad = AgentMeta {
            tier: "huge".into(),
            effort: Some("minimal".into()),
            tools: Some(vec!["read".into(), "net".into()]),
            skills: vec!["some-skill".into()],
            ..meta()
        };
        let problems = agent_meta_problems(&bad);
        assert_eq!(problems.len(), 4, "{problems:?}");
        let with_mcp = AgentMeta {
            tools: Some(vec!["mcp".into()]),
            skills: vec!["some-skill".into()],
            ..meta()
        };
        assert!(agent_meta_problems(&with_mcp).is_empty());
    }
}
