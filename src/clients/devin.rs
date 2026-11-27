//! Devin CLI: setup in `setup/devin.rs`. Its hook payloads carry no transcript path, so
//! the review judgment keeps per-session worked/reviewed flags instead.

use super::{
    ClientSpec, ContinueOutput, HookCommand, HookCommands, HookSpec, SessionOutput, SetupSpec,
    StopJudgment,
};
use crate::setup::devin;

pub(super) const SPEC: ClientSpec = ClientSpec {
    token: "devin",
    display_name: "Devin CLI",
    clis: &["devin"],
    setup: SetupSpec {
        global_dir: devin::global_dir,
        changes: devin::changes,
        removals: devin::removals,
    },
    hooks: HookSpec {
        // Devin tool names are lowercase.
        work_tools: &["write", "edit", "apply_patch", "notebook_edit"],
        delegation_tools: &[],
        commands: Some(HookCommands {
            events: &[
                ("SessionStart", HookCommand::SessionStart),
                ("UserPromptSubmit", HookCommand::Prompt),
                ("PostToolUse", HookCommand::ToolUse),
                ("Stop", HookCommand::Stop),
                ("SessionEnd", HookCommand::SessionEnd),
                ("PermissionRequest", HookCommand::Approve),
            ],
            session_output: SessionOutput::HookSpecificOutput,
            stop: StopJudgment::Flags(ContinueOutput::Block),
        }),
    },
};
