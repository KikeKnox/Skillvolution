//! The registry of AI clients Skillvolution configures: one `ClientSpec` per client, in
//! `clients/<client>.rs`, saying how to detect it, set it up, and read its hooks. Code
//! that works on every client iterates `ClientKind::ALL` and reads each client's spec
//! instead of matching on the client itself.

use crate::setup::{Change, Edit, Scope};
use anyhow::Result;
use std::path::{Path, PathBuf};

mod claude_code;
mod codex;
mod cursor;
mod devin;
mod gemini;
mod opencode;

/// One AI coding client Skillvolution knows how to configure. A closed enum, so adding
/// a client makes the compiler point at every place that still has to handle it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClientKind {
    ClaudeCode,
    OpenCode,
    Devin,
    Codex,
    Gemini,
    Cursor,
}

impl ClientKind {
    /// Every client, in the stable order setup lists, prompts for, and configures them.
    pub const ALL: [ClientKind; 6] = [
        Self::ClaudeCode,
        Self::OpenCode,
        Self::Devin,
        Self::Codex,
        Self::Gemini,
        Self::Cursor,
    ];

    /// This client's entry in the registry.
    pub const fn spec(self) -> &'static ClientSpec {
        match self {
            Self::ClaudeCode => &claude_code::SPEC,
            Self::OpenCode => &opencode::SPEC,
            Self::Devin => &devin::SPEC,
            Self::Codex => &codex::SPEC,
            Self::Gemini => &gemini::SPEC,
            Self::Cursor => &cursor::SPEC,
        }
    }

    /// The `--client` token.
    pub fn token(self) -> &'static str {
        self.spec().token
    }

    pub fn display_name(self) -> &'static str {
        self.spec().display_name
    }
}

/// Everything Skillvolution needs to know about one client.
pub struct ClientSpec {
    /// The `--client` token, for `setup`, `doctor`, and the `hook` commands.
    pub token: &'static str,
    pub display_name: &'static str,
    /// Executables whose presence on `PATH` means the client is installed.
    pub clis: &'static [&'static str],
    pub(crate) setup: SetupSpec,
    pub hooks: HookSpec,
}

/// How setup configures a client: the code in `setup/<client>.rs`.
pub(crate) struct SetupSpec {
    /// The client's per-user config directory, where global setup writes.
    pub(crate) global_dir: fn() -> Result<PathBuf>,
    /// Every file setup writes in a scope, given the binary and database paths.
    pub(crate) changes: fn(Scope, &Path, &Path) -> Result<Vec<Change>>,
    /// Every edit that undoes `changes` in a scope, plus notes for the user.
    pub(crate) removals: fn(Scope, &mut Vec<String>) -> Result<Vec<Edit>>,
}

/// What a client's hooks report and how Skillvolution answers them.
pub struct HookSpec {
    /// The client's built-in tools that edit files: calling one is work a review must
    /// cover.
    pub work_tools: &'static [&'static str],
    /// The tools that delegate to a subagent, with the input field naming the agent: a
    /// delegation to a vault agent is work too, attributed to that agent. Empty when the
    /// client's Stop judgment does not tell delegations apart.
    pub delegation_tools: &'static [DelegationTool],
    /// The `hook` commands setup installs in the client's config. `None` for a client
    /// whose hooks are a plugin that subscribes to the client's events itself (OpenCode:
    /// `assets/opencode/skillvolution.js`, which keeps its own copy of the tool names
    /// above).
    pub commands: Option<HookCommands>,
}

/// A tool that delegates to a subagent.
pub struct DelegationTool {
    /// The tool name, as the client reports it.
    pub tool: &'static str,
    /// The tool input field holding the subagent's name.
    pub agent_field: &'static str,
}

/// The `hook` commands a client runs, and the output each expects back.
pub struct HookCommands {
    /// Each client event setup hooks, with the `hook` command it runs.
    pub events: &'static [(&'static str, HookCommand)],
    /// How `hook session-start` hands the catalog to the client.
    pub session_output: SessionOutput,
    /// How `hook stop` judges whether a review is owed.
    pub stop: StopJudgment,
}

impl HookCommands {
    /// The client event that runs `command`, if setup hooks it.
    pub fn event(&self, command: HookCommand) -> Option<&'static str> {
        self.events
            .iter()
            .find(|(_, hooked)| *hooked == command)
            .map(|(event, _)| *event)
    }
}

/// A `hook` subcommand.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HookCommand {
    SessionStart,
    Prompt,
    ToolUse,
    Stop,
    SessionEnd,
    Approve,
}

/// The shape of the SessionStart output that adds context to a session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionOutput {
    /// `{"hookSpecificOutput": {"hookEventName": "SessionStart", "additionalContext"}}`.
    HookSpecificOutput,
    /// `{"additional_context"}`.
    AdditionalContext,
}

/// How a client's Stop hook tells whether work since the last review is unreported.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopJudgment {
    /// The Stop payload names a transcript, scanned for work and review tool calls; a
    /// review is asked for with `hookSpecificOutput.additionalContext`.
    Transcript,
    /// The `tool-use` hook keeps per-session worked/reviewed flags, and a review is asked
    /// for with the given output.
    Flags(ContinueOutput),
}

/// The Stop output asking a flag-tracked client to continue with a reason.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContinueOutput {
    /// `{"decision": "block", "reason"}`.
    Block,
    /// `{"decision": "deny", "reason"}`.
    Deny,
    /// `{"followup_message"}`.
    FollowupMessage,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::collections::{BTreeSet, HashSet};

    /// The `hook` subcommand each `HookCommand` stands for.
    fn subcommand(command: HookCommand) -> &'static str {
        match command {
            HookCommand::SessionStart => "session-start",
            HookCommand::Prompt => "prompt",
            HookCommand::ToolUse => "tool-use",
            HookCommand::Stop => "stop",
            HookCommand::SessionEnd => "session-end",
            HookCommand::Approve => "approve",
        }
    }

    fn assert_unique<'a>(what: &str, items: impl IntoIterator<Item = &'a str>) {
        let mut seen = HashSet::new();
        for item in items {
            assert!(!item.is_empty(), "empty {what}");
            assert!(seen.insert(item), "duplicate {what} {item:?}");
        }
    }

    #[test]
    fn every_client_has_a_unique_token_detection_setup_and_hooks() {
        assert_unique("token", ClientKind::ALL.map(ClientKind::token));
        assert_unique(
            "display name",
            ClientKind::ALL.map(ClientKind::display_name),
        );
        for kind in ClientKind::ALL {
            let spec = kind.spec();
            let token = spec.token;
            assert!(
                token
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
                "{token}: a --client token is lowercase kebab-case"
            );
            assert!(!spec.clis.is_empty(), "{token}: no CLI to detect it by");
            assert_unique("CLI", spec.clis.iter().copied());
            assert!(
                (spec.setup.global_dir)().is_ok(),
                "{token}: no global config dir"
            );

            let hooks = &spec.hooks;
            assert!(!hooks.work_tools.is_empty(), "{token}: no work tools");
            assert_unique("work tool", hooks.work_tools.iter().copied());
            assert_unique(
                "delegation tool",
                hooks.delegation_tools.iter().map(|d| d.tool),
            );
            for delegation in hooks.delegation_tools {
                assert!(
                    !delegation.agent_field.is_empty(),
                    "{token}: no agent field"
                );
                assert!(
                    !hooks.work_tools.contains(&delegation.tool),
                    "{token}: {} is both a work and a delegation tool",
                    delegation.tool
                );
            }

            let Some(commands) = &hooks.commands else {
                // A plugin client: the plugin reads these tool names itself.
                assert_eq!(kind, ClientKind::OpenCode, "{token}: no hooks");
                continue;
            };
            assert_unique(
                "hook event",
                commands.events.iter().map(|(event, _)| *event),
            );
            assert_unique(
                "hook command",
                commands
                    .events
                    .iter()
                    .map(|(_, command)| subcommand(*command)),
            );
            for required in [HookCommand::SessionStart, HookCommand::Stop] {
                assert!(
                    commands.event(required).is_some(),
                    "{token}: no {} hook",
                    subcommand(required)
                );
            }
            match commands.stop {
                // A transcript judgment reads tool calls from the transcript, not from
                // tool-use hooks.
                StopJudgment::Transcript => assert!(
                    commands.event(HookCommand::ToolUse).is_none(),
                    "{token}: tool-use hook with a transcript Stop"
                ),
                // Flags only record work and reviews: nothing reads delegations.
                StopJudgment::Flags(_) => {
                    assert!(
                        commands.event(HookCommand::ToolUse).is_some(),
                        "{token}: flag-tracked Stop with no tool-use hook"
                    );
                    assert!(
                        hooks.delegation_tools.is_empty(),
                        "{token}: delegation tools no Stop judgment reads"
                    );
                }
            }
        }
    }

    /// Every `(event, hook subcommand)` pair in the `hooks` objects of the JSON files
    /// `changes` writes.
    fn hooked_events(changes: &[Change]) -> BTreeSet<(String, String)> {
        fn commands(value: &Value, found: &mut Vec<String>) {
            match value {
                Value::Object(map) => {
                    if let Some(Value::String(command)) = map.get("command") {
                        found.push(command.clone());
                    }
                    map.values().for_each(|child| commands(child, found));
                }
                Value::Array(items) => items.iter().for_each(|child| commands(child, found)),
                _ => {}
            }
        }
        let mut pairs = BTreeSet::new();
        for (_, content) in changes {
            let Ok(config) = serde_json::from_str::<Value>(content) else {
                continue;
            };
            let Some(hooks) = config.get("hooks").and_then(Value::as_object) else {
                continue;
            };
            for (event, entries) in hooks {
                let mut found = Vec::new();
                commands(entries, &mut found);
                for command in found {
                    let (_, rest) = command.split_once(" hook ").expect("a hook command");
                    let subcommand = rest.split_whitespace().next().unwrap_or_default();
                    pairs.insert((event.clone(), subcommand.to_owned()));
                }
            }
        }
        pairs
    }

    #[test]
    fn hook_events_match_the_hooks_setup_writes() {
        let dir = tempfile::tempdir().unwrap();
        let scope = Scope::Project {
            dir: dir.path(),
            key: "project",
        };
        let (bin, db) = (Path::new("/bin/skillvolution"), Path::new("/skills.db"));
        for kind in ClientKind::ALL {
            let spec = kind.spec();
            let changes = (spec.setup.changes)(scope, bin, db).unwrap();
            assert!(!changes.is_empty(), "{}: setup writes nothing", spec.token);
            let expected: BTreeSet<(String, String)> = spec
                .hooks
                .commands
                .iter()
                .flat_map(|commands| commands.events)
                .map(|(event, command)| ((*event).to_owned(), subcommand(*command).to_owned()))
                .collect();
            assert_eq!(hooked_events(&changes), expected, "{}", spec.token);
        }
    }

    #[test]
    fn the_opencode_plugin_lists_the_registry_tool_names() {
        let plugin = include_str!("../../assets/opencode/skillvolution.js");
        let hooks = &ClientKind::OpenCode.spec().hooks;
        let work_tools: Vec<String> = hooks
            .work_tools
            .iter()
            .map(|tool| format!("{tool:?}"))
            .collect();
        let work_tools = format!("const WORK_TOOLS = new Set([{}]);", work_tools.join(", "));
        assert!(plugin.contains(&work_tools), "plugin lacks {work_tools}");
        let delegation_tools: Vec<String> = hooks
            .delegation_tools
            .iter()
            .map(|d| format!("{}: {:?}", d.tool, d.agent_field))
            .collect();
        let delegation_tools = format!(
            "const DELEGATION_TOOLS = {{ {} }};",
            delegation_tools.join(", ")
        );
        assert!(
            plugin.contains(&delegation_tools),
            "plugin lacks {delegation_tools}"
        );
    }
}
