//! OpenCode: setup in `setup/opencode.rs`. Its hooks are the plugin setup installs
//! (`assets/opencode/skillvolution.js`), which subscribes to OpenCode's events itself and
//! keeps its own copy of the tool names below (a test checks they match).

use super::{ClientSpec, DelegationTool, HookSpec, SetupSpec};
use crate::setup::opencode;

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
};
