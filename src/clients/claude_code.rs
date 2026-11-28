//! Claude Code: setup in `setup/claude.rs`. Its Stop payload names the session
//! transcript, so the review judgment scans it instead of keeping per-session flags.

use super::{
    ANY, AgentSpec, Caps, ClientSpec, DelegationTool, FrontMatter, HookCommand, HookCommands,
    HookSpec, ModelChoice, ModelRefFormat, PluginFormat, SessionOutput, SetupSpec, StopJudgment,
    Version, VersionRange, VersionSpec,
};
use crate::{agents::Agent, setup::claude, skill_file::yaml_scalar};
use anyhow::Result;
use std::path::{Path, PathBuf};

/// `Agent` delegates to a subagent; `Task` is its name in older releases.
const DELEGATION: &[DelegationTool] = &[
    DelegationTool {
        tool: "Agent",
        agent_field: "subagent_type",
    },
    DelegationTool {
        tool: "Task",
        agent_field: "subagent_type",
    },
];

pub(super) const SPEC: ClientSpec = ClientSpec {
    token: "claude-code",
    display_name: "Claude Code",
    clis: &["claude"],
    setup: SetupSpec {
        global_dir: claude::global_dir,
        changes: claude::changes,
        removals: claude::removals,
    },
    hooks: HookSpec {
        work_tools: &["Edit", "Write", "MultiEdit", "NotebookEdit"],
        delegation_tools: DELEGATION,
        commands: Some(HookCommands {
            events: &[
                ("SessionStart", HookCommand::SessionStart),
                ("UserPromptSubmit", HookCommand::Prompt),
                ("Stop", HookCommand::Stop),
            ],
            session_output: SessionOutput::HookSpecificOutput,
            stop: StopJudgment::Transcript,
        }),
    },
    versions: VersionSpec {
        // The `Agent` tool's `model` input takes only an alias; agent files reload while
        // running.
        ranges: &[VersionRange {
            min: ANY,
            max: None,
            caps: Caps {
                delegation: DELEGATION,
                plugin: PluginFormat::None,
                per_call_model: ModelRefFormat::Alias,
                hot_reload: true,
            },
        }],
        last_tested: Some(Version(2, 1, 332)),
    },
    agents: Some(AgentSpec {
        dirs,
        render,
        default_models: &[
            ("high", "opus", Some("high")),
            ("mid", "sonnet", Some("medium")),
            ("low", "haiku", Some("low")),
        ],
    }),
};

fn dirs(project: Option<&Path>) -> Result<PathBuf> {
    Ok(match project {
        Some(dir) => dir.join(".claude").join("agents"),
        None => claude::global_dir()?.join("agents"),
    })
}

fn render(agent: &Agent, model: Option<&ModelChoice>) -> FrontMatter {
    let meta = &agent.meta;
    let mut front = vec![
        format!("name: {}", yaml_scalar(&agent.id)),
        format!("description: {}", yaml_scalar(&agent.description)),
    ];
    let mapped_effort = model.and_then(|(_, effort)| effort.clone());
    if let Some((model, _)) = model {
        front.push(format!("model: {}", yaml_scalar(model)));
    }
    if let Some(effort) = meta.effort.clone().or(mapped_effort) {
        front.push(format!("effort: {}", yaml_scalar(&effort)));
    }
    let mut tools = Vec::new();
    for (tool, names) in [
        ("edit", "Edit, Write, NotebookEdit, MultiEdit"),
        ("shell", "Bash, PowerShell, Monitor"),
        ("web", "WebFetch, WebSearch"),
        ("mcp", "mcp__*"),
    ] {
        if !meta.allows(tool) {
            tools.push(names);
        }
    }
    // A Skillvolution agent never delegates to other subagents.
    tools.push("Agent, Task");
    front.push(format!("disallowedTools: {}", tools.join(", ")));
    (front, Vec::new())
}
