//! Claude Code: setup in `setup/claude.rs`. Its Stop payload names the session
//! transcript, so the review judgment scans it instead of keeping per-session flags.

use super::{
    ClientSpec, DelegationTool, HookCommand, HookCommands, HookSpec, SessionOutput, SetupSpec,
    StopJudgment,
};
use crate::setup::claude;

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
        delegation_tools: &[
            DelegationTool {
                tool: "Agent",
                agent_field: "subagent_type",
            },
            DelegationTool {
                tool: "Task",
                agent_field: "subagent_type",
            },
        ],
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
};
