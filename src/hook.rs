use crate::vault::Vault;
use anyhow::Result;
use serde_json::Value;
use std::{
    fmt::Write as _,
    fs::File,
    io::{Read, Seek, SeekFrom},
};

const CATALOG_LIMIT: i64 = 30;
const WORK_TOOLS: [&str; 5] = ["Edit", "Write", "MultiEdit", "NotebookEdit", "Bash"];
const REVIEW_TOOLS: [&str; 2] = ["propose_skill_change", "report_skill_outcome"];

pub const STOP_REASON: &str = "Skillvolution review: you changed files or ran commands since the last review. \
Follow the Report and Reflect steps of the evolution skill now: call report_skill_outcome for any vault skill you applied, \
and call propose_skill_change only for a lesson that meets every lesson criterion. \
If there is nothing to report or propose, reply only \"No lesson.\" and stop.";

pub fn session_start(vault: &Vault, project: Option<&str>) -> Result<String> {
    let page = vault.search("", project, CATALOG_LIMIT, 0)?;
    let mut text = String::from(
        "Skillvolution vault (shared procedural memory): follow the `evolution` skill. \
         Before non-trivial work, check relevant skills with search_skills/get_skill; \
         after applying one, call report_skill_outcome; propose verified lessons with propose_skill_change.\n",
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
    if page.has_more {
        writeln!(
            text,
            "- ...and {} more; use search_skills",
            page.total - page.skills.len() as i64
        )?;
    }
    Ok(text)
}

/// Returns a reason to block the stop when the transcript shows work since the
/// last review and no outcome report or proposal. Each transcript span is
/// judged once, and a stop already continued by this hook is never blocked.
pub fn stop(vault: &Vault, input: &str) -> Result<Option<&'static str>> {
    let input: Value = serde_json::from_str(input)?;
    if input["stop_hook_active"].as_bool() == Some(true) {
        return Ok(None);
    }
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
    let (mut worked, mut reviewed) = (false, false);
    for line in bytes[..complete].split(|b| *b == b'\n') {
        if let Ok(entry) = serde_json::from_slice::<Value>(line) {
            visit_tool_uses(&entry, &mut |name| {
                worked |= WORK_TOOLS.contains(&name);
                reviewed |= REVIEW_TOOLS.iter().any(|tool| name.ends_with(tool));
            });
        }
    }
    vault.set_transcript_offset(session, offset + complete as u64)?;
    Ok((worked && !reviewed).then_some(STOP_REASON))
}

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
