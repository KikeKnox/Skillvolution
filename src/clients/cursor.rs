//! Cursor: setup in `setup/cursor.rs`. Flag-tracked, like Devin. It has no prompt hook:
//! `beforeSubmitPrompt` can only allow or block the prompt, not add context to it.

use super::{
    ClientSpec, ContinueOutput, HookCommand, HookCommands, HookSpec, SessionOutput, SetupSpec,
    StopJudgment,
};
use crate::setup::cursor;

pub(super) const SPEC: ClientSpec = ClientSpec {
    token: "cursor",
    display_name: "Cursor",
    clis: &["cursor-agent", "cursor"],
    setup: SetupSpec {
        global_dir: cursor::global_dir,
        changes: cursor::changes,
        removals: cursor::removals,
    },
    hooks: HookSpec {
        // Cursor's documented native tools are Shell, Read, Write, Grep, Delete, and Task;
        // file edits (not just whole-file writes) go through the same "Write" tool, with no
        // separate "Edit"/"MultiEdit" name. https://cursor.com/docs/hooks
        work_tools: &["Write"],
        delegation_tools: &[],
        commands: Some(HookCommands {
            events: &[
                ("sessionStart", HookCommand::SessionStart),
                ("postToolUse", HookCommand::ToolUse),
                ("stop", HookCommand::Stop),
                ("sessionEnd", HookCommand::SessionEnd),
            ],
            session_output: SessionOutput::AdditionalContext,
            stop: StopJudgment::Flags(ContinueOutput::FollowupMessage),
        }),
    },
    agents: None,
};
