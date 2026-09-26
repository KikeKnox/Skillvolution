use crate::vault::Vault;
use anyhow::Result;
use serde_json::Value;
use std::{
    collections::HashSet,
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

pub const STOP_REASON: &str = "Skillvolution review: you edited files since the last review. \
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

/// Returns a reason to block the stop when the transcript shows work not
/// followed by an outcome report or proposal. Each transcript span is judged
/// once, and a stop already continued by this hook is never blocked.
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
    Ok(has_unreviewed_work(&bytes[..complete]).then_some(STOP_REASON))
}

/// Whether the transcript lines in `span` contain a work tool call after the
/// last review tool call. A review whose tool_result is an error (a rejected
/// publish, say) reviewed nothing, so it does not count.
fn has_unreviewed_work(span: &[u8]) -> bool {
    let mut position = 0;
    let mut last_work = None;
    let mut reviews = Vec::new();
    let mut failed = HashSet::new();
    for line in span.split(|b| *b == b'\n') {
        let Ok(entry) = serde_json::from_slice::<Value>(line) else {
            continue;
        };
        visit_objects(&entry, &mut |object| match object["type"].as_str() {
            Some("tool_use") => {
                position += 1;
                let name = object["name"].as_str().unwrap_or_default();
                if WORK_TOOLS.contains(&name) {
                    last_work = Some(position);
                } else if is_review_tool(name) {
                    let id = object["id"].as_str().unwrap_or_default();
                    reviews.push((position, id.to_owned()));
                }
            }
            Some("tool_result") if object["is_error"].as_bool() == Some(true) => {
                if let Some(id) = object["tool_use_id"].as_str() {
                    failed.insert(id.to_owned());
                }
            }
            _ => {}
        });
    }
    let last_review = reviews
        .into_iter()
        .filter(|(_, id)| !failed.contains(id))
        .map(|(position, _)| position)
        .max();
    // None orders below any Some, so no work never blocks and work with no
    // successful review always does.
    last_work > last_review
}

fn is_review_tool(name: &str) -> bool {
    REVIEW_TOOLS.iter().any(|tool| name.ends_with(tool))
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

/// Devin PostToolUse: flags the session as having done work (which a previous
/// review no longer covers) or as having reviewed it, based on the tool that
/// just ran. Any other tool leaves the flags alone.
pub fn devin_tool_use(vault: &Vault, input: &str) -> Result<()> {
    let input: Value = serde_json::from_str(input)?;
    let (Some(session), Some(tool)) = (input["session_id"].as_str(), input["tool_name"].as_str())
    else {
        return Ok(());
    };
    if is_review_tool(tool) {
        vault.mark_devin_review(session)
    } else if DEVIN_WORK_TOOLS.contains(&tool) {
        vault.mark_devin_work(session)
    } else {
        Ok(())
    }
}

/// Devin Stop: returns the review reason when the span since the last stop
/// has work not followed by a review. Every judged stop consumes its span, so
/// a stop caused by the agent simply ignoring the reminder is not blocked
/// again. A stop already continued by this hook (stop_hook_active) is never
/// blocked and leaves the flags alone.
pub fn devin_stop(vault: &Vault, input: &str) -> Result<Option<&'static str>> {
    let input: Value = serde_json::from_str(input)?;
    if input["stop_hook_active"].as_bool() == Some(true) {
        return Ok(None);
    }
    let Some(session) = input["session_id"].as_str() else {
        return Ok(None);
    };
    let (worked, reviewed) = vault.take_devin_hook_state(session)?;
    Ok((worked && !reviewed).then_some(STOP_REASON))
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

/// Calls `visit` on every object in the value, walking all of it rather than a
/// fixed path: transcript entry shapes vary across Claude Code versions, and
/// tool_use/tool_result blocks can be nested inside content arrays at
/// different depths. Array items are visited in order, so the blocks of one
/// content array keep their transcript order.
fn visit_objects(value: &Value, visit: &mut impl FnMut(&Value)) {
    match value {
        Value::Object(map) => {
            visit(value);
            map.values().for_each(|child| visit_objects(child, visit));
        }
        Value::Array(items) => items.iter().for_each(|child| visit_objects(child, visit)),
        _ => {}
    }
}
