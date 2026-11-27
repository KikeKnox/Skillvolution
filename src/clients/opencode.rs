//! OpenCode: setup in `setup/opencode.rs`. Its hooks are the plugin setup installs
//! (`assets/opencode/skillvolution.js`), which subscribes to OpenCode's events itself and
//! keeps its own copy of the tool names below (a test checks they match).

use super::{
    ANY, AgentSpec, Caps, ClientSpec, DelegationTool, FrontMatter, HookSpec, ModelChoice,
    ModelRefFormat, PluginFormat, SetupSpec, Version, VersionRange, VersionSpec,
};
use crate::{agents::Agent, setup::opencode, skill_file::yaml_scalar};
use anyhow::Result;
use std::path::{Path, PathBuf};

/// OpenCode 2.x starts here; 1.x is everything before.
const V2: Version = Version(2, 0, 0);
/// 1.x delegates with `task`/`subagent_type`.
const TASK: DelegationTool = DelegationTool {
    tool: "task",
    agent_field: "subagent_type",
};
/// 2.x delegates with `subagent`/`agent`.
const SUBAGENT: DelegationTool = DelegationTool {
    tool: "subagent",
    agent_field: "agent",
};

pub(super) const SPEC: ClientSpec = ClientSpec {
    token: "opencode",
    display_name: "OpenCode",
    clis: &["opencode"],
    setup: SetupSpec {
        global_dir: opencode::global_dir,
        changes: opencode::changes,
        removals: opencode::removals,
    },
    hooks: HookSpec {
        // Built-in tool ids that edit files: 1.x has edit, write, multiedit, patch and
        // apply_patch; 2.x has edit, write and patch.
        work_tools: &["edit", "write", "multiedit", "patch", "apply_patch"],
        // Both versions' tools: the plugin tells them apart by name.
        delegation_tools: &[TASK, SUBAGENT],
        commands: None,
    },
    versions: VersionSpec {
        ranges: &[
            // 1.x (up to 1.19.x): `task`, plugin as `{id, server}` (or named exports),
            // no model per delegation call, agent config cached until restart.
            VersionRange {
                min: ANY,
                max: Some(V2),
                caps: Caps {
                    delegation: &[TASK],
                    plugin: PluginFormat::ServerExport,
                    per_call_model: ModelRefFormat::None,
                    hot_reload: false,
                },
            },
            // 2.x: `subagent`, plugin as `{id, setup}`, `provider/model[#variant]` per
            // call, agent files watched. The plugin exports both shapes.
            VersionRange {
                min: V2,
                max: None,
                caps: Caps {
                    delegation: &[SUBAGENT],
                    plugin: PluginFormat::SetupExport,
                    per_call_model: ModelRefFormat::ProviderModel,
                    hot_reload: true,
                },
            },
        ],
        last_tested: Some(Version(2, 2, 5)),
    },
    agents: Some(AgentSpec {
        dirs,
        render,
        default_models: &[],
    }),
};

fn dirs(project: Option<&Path>) -> Result<PathBuf> {
    Ok(match project {
        Some(dir) => dir.join(".opencode").join("agents"),
        None => opencode::global_dir()?.join("agents"),
    })
}

fn render(agent: &Agent, model: Option<&ModelChoice>) -> FrontMatter {
    let meta = &agent.meta;
    let mut warnings = Vec::new();
    let mut front = vec![
        format!("description: {}", yaml_scalar(&agent.description)),
        "mode: subagent".to_owned(),
    ];
    if let Some((model, _)) = model {
        front.push(format!("model: {}", yaml_scalar(model)));
    }
    // OpenCode merges a user-level file with a project one of the same name, so every tool
    // is written explicitly: an omitted key would be inherited.
    front.push("permission:".into());
    for (tool, key) in [
        ("edit", "edit"),
        ("shell", "bash"),
        ("web", "webfetch"),
        ("web", "websearch"),
    ] {
        let rule = if meta.allows(tool) { "allow" } else { "deny" };
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
    if !meta.allows("mcp") {
        warnings.push("OpenCode cannot block MCP tools by kind; not enforced".into());
    }
    if meta.effort.is_some() {
        warnings.push("effort is not written for OpenCode".into());
    }
    (front, warnings)
}
