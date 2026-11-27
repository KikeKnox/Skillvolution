//! Codex CLI: setup in `setup/codex.rs`. Flag-tracked, like Devin.

use super::{
    ANY, Caps, ClientSpec, ContinueOutput, HookCommand, HookCommands, HookSpec, SessionOutput,
    SetupSpec, StopJudgment, VersionRange, VersionSpec,
};
use crate::setup::codex;

pub(super) const SPEC: ClientSpec = ClientSpec {
    token: "codex",
    display_name: "Codex CLI",
    clis: &["codex"],
    setup: SetupSpec {
        global_dir: codex::global_dir,
        changes: codex::changes,
        removals: codex::removals,
    },
    hooks: HookSpec {
        // As PostToolUse payloads report it in `tool_name`.
        work_tools: &["apply_patch"],
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
    versions: VersionSpec {
        // No capability of this client changes with its version yet.
        ranges: &[VersionRange {
            min: ANY,
            max: None,
            caps: Caps::NONE,
        }],
        last_tested: None,
    },
    agents: None,
};
