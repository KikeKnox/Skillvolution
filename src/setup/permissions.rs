//! Permission grants merged into client settings so the review flow runs
//! unattended: the evaluator subagent must launch without a prompt, and the
//! vault's MCP tools must be callable without per-call approval.

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

/// Claude Code `permissions.allow` rules: `Task` covers every subagent
/// dispatch, `mcp__skillvolution` every tool on the vault server.
const CLAUDE_ALLOW: [&str; 2] = ["Task", "mcp__skillvolution"];

/// Devin `permissions.allow` rules: the subagent tools the evolution skill
/// dispatches, and the vault MCP tools (`mcp__<server>__<tool>` naming).
const DEVIN_ALLOW: [&str; 3] = ["run_subagent", "read_subagent", "mcp__skillvolution__*"];

/// OpenCode exposes MCP tools as `<server>_<tool>`; each is allowed explicitly
/// since permission keys are tool ids, not globs.
const OPENCODE_MCP_TOOLS: [&str; 4] = [
    "skillvolution_search_skills",
    "skillvolution_get_skill",
    "skillvolution_report_skill_outcome",
    "skillvolution_publish_skill",
];

/// Claude Code: appends the missing entries of `CLAUDE_ALLOW` to
/// `permissions.allow`, preserving rules already present.
pub fn merge_claude(config: &mut Value) -> Result<()> {
    allow_rules(config, &CLAUDE_ALLOW)
}

/// Devin: appends the missing entries of `DEVIN_ALLOW` to `permissions.allow`,
/// preserving rules already present.
pub fn merge_devin(config: &mut Value) -> Result<()> {
    allow_rules(config, &DEVIN_ALLOW)
}

/// Appends `rules` to `permissions.allow`, preserving rules already present. A
/// malformed `permissions`/`allow` value is refused rather than rewritten.
fn allow_rules(config: &mut Value, rules: &[&str]) -> Result<()> {
    let permissions = config
        .as_object_mut()
        .context("config must be an object")?
        .entry("permissions")
        .or_insert_with(|| json!({}));
    let permissions = permissions
        .as_object_mut()
        .context("permissions must be an object")?;
    let allow = permissions.entry("allow").or_insert_with(|| json!([]));
    let allow = allow
        .as_array_mut()
        .context("permissions.allow must be an array")?;
    for rule in rules {
        if !allow.iter().any(|entry| entry.as_str() == Some(*rule)) {
            allow.push(json!(rule));
        }
    }
    Ok(())
}

/// OpenCode: sets `permission.task` and each vault MCP tool to `"allow"`, but
/// only where the user has not already chosen a value — an explicit `ask` or
/// `deny` is left untouched. A bare-string `permission` (`"ask"`, `"allow"`,
/// `"deny"`) applies to every tool at once and cannot carry per-tool rules, so
/// it is refused instead of silently dropping that choice.
pub fn merge_opencode(config: &mut Value) -> Result<()> {
    let root = config.as_object_mut().context("config must be an object")?;
    if let Some(permission) = root.get("permission")
        && !permission.is_object()
    {
        bail!("permission must be an object to add the task/vault grants");
    }
    let permission = root
        .entry("permission")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .context("permission must be an object")?;
    permission.entry("task").or_insert_with(|| json!("allow"));
    for tool in OPENCODE_MCP_TOOLS {
        permission.entry(tool).or_insert_with(|| json!("allow"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_merge_is_idempotent_and_preserves_existing_rules() {
        let mut config =
            json!({"permissions": {"allow": ["Bash(git status)", "Task"], "deny": ["Bash(rm)"]}});
        merge_claude(&mut config).unwrap();
        merge_claude(&mut config).unwrap();
        assert_eq!(
            config["permissions"]["allow"],
            json!(["Bash(git status)", "Task", "mcp__skillvolution"])
        );
        assert_eq!(config["permissions"]["deny"], json!(["Bash(rm)"]));
    }

    #[test]
    fn claude_merge_refuses_malformed_permissions() {
        for bad in [
            json!({"permissions": "nope"}),
            json!({"permissions": {"allow": "nope"}}),
        ] {
            let mut config = bad.clone();
            assert!(merge_claude(&mut config).is_err(), "{bad}");
            assert_eq!(config, bad, "input must be untouched on refusal");
        }
    }

    #[test]
    fn devin_merge_adds_subagent_and_mcp_rules() {
        let mut config = json!({"permissions": {"allow": ["exec"], "deny": ["exec(sudo *)"]}});
        merge_devin(&mut config).unwrap();
        merge_devin(&mut config).unwrap();
        assert_eq!(
            config["permissions"]["allow"],
            json!([
                "exec",
                "run_subagent",
                "read_subagent",
                "mcp__skillvolution__*"
            ])
        );
        assert_eq!(config["permissions"]["deny"], json!(["exec(sudo *)"]));
    }

    #[test]
    fn opencode_merge_sets_missing_keys_and_respects_explicit_choices() {
        let mut config = json!({"permission": {"task": "ask", "edit": "deny"}});
        merge_opencode(&mut config).unwrap();
        assert_eq!(config["permission"]["task"], "ask");
        assert_eq!(config["permission"]["edit"], "deny");
        for tool in OPENCODE_MCP_TOOLS {
            assert_eq!(config["permission"][tool], "allow", "{tool}");
        }
    }

    #[test]
    fn opencode_merge_refuses_a_bare_action_string() {
        let mut config = json!({"permission": "ask"});
        assert!(merge_opencode(&mut config).is_err());
        assert_eq!(config, json!({"permission": "ask"}));
    }
}
