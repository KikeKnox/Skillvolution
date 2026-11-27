//! OpenCode: setup in `setup/opencode.rs`. Its hooks are the plugin setup installs
//! (`assets/opencode/skillvolution.js`), which subscribes to OpenCode's events itself and
//! keeps its own copy of the tool names below (a test checks they match).

use super::{
    AgentSpec, ClientSpec, DelegationTool, FrontMatter, HookSpec, ModelChoice, ModelRefFormat,
    SetupSpec,
};
use crate::{agents::Agent, setup::opencode, skill_file::yaml_scalar};
use anyhow::Result;
use std::path::{Path, PathBuf};

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
        // `task`/`subagent_type` in 1.x, `subagent`/`agent` in 2.x.
        delegation_tools: &[
            DelegationTool {
                tool: "task",
                agent_field: "subagent_type",
            },
            DelegationTool {
                tool: "subagent",
                agent_field: "agent",
            },
        ],
        commands: None,
    },
    agents: Some(AgentSpec {
        dirs,
        render,
        // 1.x delegation calls cannot pick a model; 2.x takes `provider/model[#variant]`
        // and reloads agent files while running. The version-dependent values arrive with
        // the per-client versions of Phase 2; these are the 1.x ones.
        per_call_model: ModelRefFormat::None,
        hot_reload: false,
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
