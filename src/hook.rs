use crate::vault::Vault;
use anyhow::Result;
use serde_json::Value;
use std::{
    fmt::Write as _,
    fs::File,
    io::{Read, Seek, SeekFrom},
};

const CATALOG_LIMIT: i64 = 30;
const WORK_TOOLS: [&str; 4] = ["Edit", "Write", "MultiEdit", "NotebookEdit"];
const REVIEW_TOOLS: [&str; 2] = ["publish_skill", "report_skill_outcome"];
// Devin tool names are lowercase and its hook payloads carry no transcript path,
// so the same worked/reviewed judgment is kept as per-session flags instead.
pub(crate) const DEVIN_WORK_TOOLS: [&str; 4] = ["write", "edit", "apply_patch", "notebook_edit"];

pub const STOP_REASON: &str = "Skillvolution review: you changed files or ran commands since the last review. \
Follow the Report and Reflect steps of the evolution skill now: call report_skill_outcome for any vault skill you applied, \
and for any candidate lesson that meets every lesson criterion, dispatch a fresh subagent now \
— do not ask the user first — to judge it (global scope, project scope, or discard), then call publish_skill with the verdict and its reason. \
If there is nothing to report or evaluate, say so in a single short line with no elaboration and no heading.";

pub fn session_start(vault: &Vault, project: Option<&str>) -> Result<String> {
    let page = vault.search("", project, CATALOG_LIMIT, 0)?;
    let mut text = String::from(
        "Skillvolution vault (shared procedural memory): follow the `evolution` skill.\n",
    );
    if page.skills.is_empty() {
        text.push_str("No published skills are visible to this project yet.\n");
        return Ok(text);
    }
    writeln!(text, "Published skills ({} visible):", page.total)?;
    for skill in &page.skills {
        write!(
            text,
            "- {} v{}: {}",
            skill.id, skill.version, skill.description
        )?;
        if skill.helped + skill.failed > 0 {
            write!(text, " [helped {}, failed {}]", skill.helped, skill.failed)?;
        }
        text.push('\n');
    }
    let shown = page.skills.len() as i64;
    if page.total > shown {
        writeln!(
            text,
            "- ...and {} more; use search_skills",
            page.total - shown
        )?;
    }
    Ok(text)
}

/// Returns a reason to block the stop when the transcript shows work since the
/// last review and no outcome report or proposal. Each transcript span is
/// judged once, and a stop already continued by this hook is never blocked.
pub fn stop(vault: &Vault, input: &str) -> Result<Option<&'static str>> {
    let input: Value = serde_json::from_str(input)?;
    let stop_hook_active = input["stop_hook_active"].as_bool() == Some(true);
    let (Some(session), Some(path)) = (
        input["session_id"].as_str(),
        input["transcript_path"].as_str(),
    ) else {
        return Ok(None);
    };
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let mut offset = vault.transcript_offset(session)?;
    if offset > file.metadata()?.len() {
        offset = 0;
    }
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let complete = bytes.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
    // Advance the offset even when this stop was itself caused by our own
    // block (stop_hook_active): otherwise the lines Claude wrote in response
    // (e.g. a report_skill_outcome call) get re-read on the next real stop,
    // mixed in with new unreviewed work, and mask it as already reviewed.
    vault.set_transcript_offset(session, offset + complete as u64)?;
    if stop_hook_active {
        return Ok(None);
    }
    let (mut worked, mut reviewed) = (false, false);
    for line in bytes[..complete].split(|b| *b == b'\n') {
        if let Ok(entry) = serde_json::from_slice::<Value>(line) {
            visit_tool_uses(&entry, &mut |name| {
                worked |= WORK_TOOLS.contains(&name);
                reviewed |= REVIEW_TOOLS.iter().any(|tool| name.ends_with(tool));
            });
        }
    }
    Ok((worked && !reviewed).then_some(STOP_REASON))
}

/// Devin SessionStart: same catalog as `session_start`, wrapped in the JSON
/// envelope Devin expects for injected context.
pub fn devin_session_start(vault: &Vault, project: Option<&str>) -> Result<String> {
    let context = session_start(vault, project)?;
    Ok(serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "SessionStart",
            "additionalContext": context,
        }
    })
    .to_string())
}

/// Devin PostToolUse: flags the session as having done work or having reviewed
/// it, based on the tool that just ran. Any other tool leaves the flags alone.
pub fn devin_tool_use(vault: &Vault, input: &str) -> Result<()> {
    let input: Value = serde_json::from_str(input)?;
    let (Some(session), Some(tool)) = (input["session_id"].as_str(), input["tool_name"].as_str())
    else {
        return Ok(());
    };
    let (mut worked, mut reviewed) = vault.devin_hook_state(session)?;
    if REVIEW_TOOLS.iter().any(|t| tool.ends_with(t)) {
        reviewed = true;
    } else if DEVIN_WORK_TOOLS.contains(&tool) {
        worked = true;
    } else {
        return Ok(());
    }
    vault.set_devin_hook_state(session, worked, reviewed)
}

/// Devin Stop: returns the review reason once per unreviewed span. The span is
/// consumed when it blocks, so a stop caused by the agent simply ignoring the
/// reminder is not blocked again; a stop already continued by this hook
/// (stop_hook_active) is never blocked either.
pub fn devin_stop(vault: &Vault, input: &str) -> Result<Option<&'static str>> {
    let input: Value = serde_json::from_str(input)?;
    if input["stop_hook_active"].as_bool() == Some(true) {
        return Ok(None);
    }
    let Some(session) = input["session_id"].as_str() else {
        return Ok(None);
    };
    let (worked, reviewed) = vault.devin_hook_state(session)?;
    if !worked || reviewed {
        return Ok(None);
    }
    vault.set_devin_hook_state(session, false, false)?;
    Ok(Some(STOP_REASON))
}

/// Devin SessionEnd: drops the session's flag row so the table does not grow
/// with dead sessions.
pub fn devin_session_end(vault: &Vault, input: &str) -> Result<()> {
    let input: Value = serde_json::from_str(input)?;
    if let Some(session) = input["session_id"].as_str() {
        vault.clear_devin_hook_state(session)?;
    }
    Ok(())
}

/// Devin tool names the evolution workflow needs without prompting: subagent
/// dispatch (not covered by the documented `permissions` tool list, which only
/// names read/edit/grep/glob/exec) is handled here via PermissionRequest.
const DEVIN_AUTO_APPROVE_TOOLS: [&str; 2] = ["run_subagent", "read_subagent"];

/// Devin PermissionRequest: approves the subagent and vault MCP tools the
/// evolution flow requires; any other tool prints nothing so the normal
/// permission prompt still runs.
pub fn devin_approve(input: &str) -> Result<Option<&'static str>> {
    let input: Value = serde_json::from_str(input)?;
    let Some(tool) = input["tool_name"].as_str() else {
        return Ok(None);
    };
    let approved =
        DEVIN_AUTO_APPROVE_TOOLS.contains(&tool) || tool.starts_with("mcp__skillvolution__");
    Ok(approved.then_some("Skillvolution-managed tool"))
}

/// Walks the whole JSON value rather than a fixed path: transcript entry
/// shapes vary across Claude Code versions, and tool_use blocks can be
/// nested inside content arrays at different depths.
fn visit_tool_uses(value: &Value, visit: &mut impl FnMut(&str)) {
    match value {
        Value::Object(map) => {
            if map.get("type").and_then(Value::as_str) == Some("tool_use")
                && let Some(name) = map.get("name").and_then(Value::as_str)
            {
                visit(name);
            }
            map.values().for_each(|child| visit_tool_uses(child, visit));
        }
        Value::Array(items) => items.iter().for_each(|child| visit_tool_uses(child, visit)),
        _ => {}
    }
}
