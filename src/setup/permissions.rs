//! Permission grants merged into client settings so the review flow runs
//! unattended: the evaluator subagent must launch without a prompt, and the
//! vault's MCP tools must be callable without per-call approval.

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

/// Claude Code `permissions.allow` rules: `Agent` covers every subagent dispatch
/// (`Task` is its name in older Claude Code versions), `mcp__skillvolution` every
/// tool on the vault server.
const CLAUDE_ALLOW: [&str; 3] = ["Agent", "Task", "mcp__skillvolution"];

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

/// Claude Code removal: drops `mcp__skillvolution` from `permissions.allow`, keeping
/// `Agent`/`Task` (the user may have granted them independently of Skillvolution).
/// Returns whether anything was removed, and which of those two remain present so the
/// caller can note them for the user to remove manually.
pub fn remove_claude(config: &mut Value) -> Result<(bool, Vec<&'static str>)> {
    remove_allow_rules(config, &["mcp__skillvolution"], &["Agent", "Task"])
}

/// Devin removal: drops `mcp__skillvolution__*` from `permissions.allow`, keeping
/// `run_subagent`/`read_subagent`. Returns whether anything was removed, and which of
/// those two remain present.
pub fn remove_devin(config: &mut Value) -> Result<(bool, Vec<&'static str>)> {
    remove_allow_rules(
        config,
        &["mcp__skillvolution__*"],
        &["run_subagent", "read_subagent"],
    )
}

/// Removes `ours` from `permissions.allow`, dropping `allow` if that empties it and
/// `permissions` if that in turn empties it. Returns whether anything was removed, and
/// which of `generic` are still present in `allow` (a malformed or absent
/// `permissions`/`allow` is left untouched: there is nothing of ours to remove from it
/// either way).
fn remove_allow_rules(
    config: &mut Value,
    ours: &[&str],
    generic: &[&'static str],
) -> Result<(bool, Vec<&'static str>)> {
    let Some(root) = config.as_object_mut() else {
        return Ok((false, Vec::new()));
    };
    let Some(permissions) = root.get_mut("permissions").and_then(Value::as_object_mut) else {
        return Ok((false, Vec::new()));
    };
    let Some(allow) = permissions.get_mut("allow").and_then(Value::as_array_mut) else {
        return Ok((false, Vec::new()));
    };
    let before = allow.len();
    allow.retain(|entry| !entry.as_str().is_some_and(|rule| ours.contains(&rule)));
    let removed = allow.len() != before;
    let remaining: Vec<&'static str> = generic
        .iter()
        .filter(|rule| allow.iter().any(|entry| entry.as_str() == Some(**rule)))
        .copied()
        .collect();
    if allow.is_empty() {
        permissions.remove("allow");
    }
    if permissions.is_empty() {
        root.remove("permissions");
    }
    Ok((removed, remaining))
}

/// OpenCode removal: drops the vault MCP tool grants from `permission`, dropping the
/// container if that empties it. Returns whether anything was removed.
pub fn remove_opencode(config: &mut Value) -> Result<bool> {
    let Some(root) = config.as_object_mut() else {
        return Ok(false);
    };
    let Some(permission) = root.get_mut("permission").and_then(Value::as_object_mut) else {
        return Ok(false);
    };
    let mut removed = false;
    for tool in OPENCODE_MCP_TOOLS {
        removed |= permission.remove(tool).is_some();
    }
    if permission.is_empty() {
        root.remove("permission");
    }
    Ok(removed)
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
            json!(["Bash(git status)", "Task", "Agent", "mcp__skillvolution"])
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

    #[test]
    fn claude_removal_drops_only_the_mcp_rule_and_reports_kept_generic_grants() {
        let mut config = json!({"permissions": {"allow": ["Bash(git status)", "Task", "Agent", "mcp__skillvolution"], "deny": ["Bash(rm)"]}});
        let (removed, kept) = remove_claude(&mut config).unwrap();
        assert!(removed);
        assert_eq!(kept, ["Agent", "Task"]);
        assert_eq!(
            config["permissions"]["allow"],
            json!(["Bash(git status)", "Task", "Agent"])
        );
        assert_eq!(config["permissions"]["deny"], json!(["Bash(rm)"]));
    }

    #[test]
    fn claude_removal_drops_emptied_allow_and_permissions() {
        let mut config = json!({"permissions": {"allow": ["mcp__skillvolution"]}});
        let (removed, kept) = remove_claude(&mut config).unwrap();
        assert!(removed);
        assert!(kept.is_empty());
        assert_eq!(config, json!({}));
    }

    #[test]
    fn claude_removal_is_a_no_op_without_our_rule() {
        let mut config = json!({"permissions": {"allow": ["Task"]}});
        let before = config.clone();
        let (removed, kept) = remove_claude(&mut config).unwrap();
        assert!(!removed);
        assert_eq!(kept, ["Task"]);
        assert_eq!(config, before);
    }

    #[test]
    fn devin_removal_drops_only_the_mcp_rule_and_reports_kept_generic_grants() {
        let mut config = json!({"permissions": {"allow": ["exec", "run_subagent", "read_subagent", "mcp__skillvolution__*"]}});
        let (removed, kept) = remove_devin(&mut config).unwrap();
        assert!(removed);
        assert_eq!(kept, ["run_subagent", "read_subagent"]);
        assert_eq!(
            config["permissions"]["allow"],
            json!(["exec", "run_subagent", "read_subagent"])
        );
    }

    #[test]
    fn opencode_removal_drops_our_tools_and_container_when_emptied() {
        let mut config = json!({"permission": {"task": "allow"}});
        for tool in OPENCODE_MCP_TOOLS {
            config["permission"][tool] = json!("allow");
        }
        assert!(remove_opencode(&mut config).unwrap());
        assert_eq!(config, json!({"permission": {"task": "allow"}}));

        let mut config = json!({});
        for tool in OPENCODE_MCP_TOOLS {
            config["permission"] = json!({});
            config["permission"][tool] = json!("allow");
        }
        assert!(remove_opencode(&mut config).unwrap());
        assert_eq!(config, json!({}));
    }

    #[test]
    fn opencode_removal_is_a_no_op_without_our_tools() {
        let mut config = json!({"permission": {"task": "ask", "edit": "deny"}});
        let before = config.clone();
        assert!(!remove_opencode(&mut config).unwrap());
        assert_eq!(config, before);
    }
}
