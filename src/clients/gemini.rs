//! Gemini CLI: setup in `setup/gemini.rs`. Flag-tracked, like Devin; BeforeAgent is its
//! prompt hook and AfterAgent its Stop.

use super::{
    ClientSpec, ContinueOutput, HookCommand, HookCommands, HookSpec, SessionOutput, SetupSpec,
    StopJudgment,
};
use crate::setup::gemini;

pub(super) const SPEC: ClientSpec = ClientSpec {
    token: "gemini",
    display_name: "Gemini CLI",
    clis: &["gemini"],
    setup: SetupSpec {
        global_dir: gemini::global_dir,
        changes: gemini::changes,
        removals: gemini::removals,
    },
    hooks: HookSpec {
        // As AfterTool payloads report it in `tool_name`.
        work_tools: &["write_file", "replace"],
        delegation_tools: &[],
        commands: Some(HookCommands {
            events: &[
                ("SessionStart", HookCommand::SessionStart),
                ("BeforeAgent", HookCommand::Prompt),
                ("AfterTool", HookCommand::ToolUse),
                ("AfterAgent", HookCommand::Stop),
                ("SessionEnd", HookCommand::SessionEnd),
            ],
            session_output: SessionOutput::HookSpecificOutput,
            stop: StopJudgment::Flags(ContinueOutput::Deny),
        }),
    },
    agents: None,
};
