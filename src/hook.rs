use crate::vault::{SkillMetadata, Vault};
use anyhow::Result;
use serde_json::Value;
use std::{
    collections::HashSet,
    fmt::Write as _,
    fs::File,
    io::{Read, Seek, SeekFrom},
};

const CATALOG_LIMIT: i64 = 30;
pub const WORK_TOOLS: &[&str] = &["Edit", "Write", "MultiEdit", "NotebookEdit"];
const REVIEW_TOOLS: [&str; 2] = ["publish_skill", "report_skill_outcome"];
// Loading a skill is work too: whoever applied it owes an outcome report. Matched by
// suffix, like the review tools, since each client prefixes MCP tool names its own way.
const LOAD_TOOL: &str = "get_skill";
// Devin tool names are lowercase and its hook payloads carry no transcript path,
// so the same worked/reviewed judgment is kept as per-session flags instead.
pub const DEVIN_WORK_TOOLS: &[&str] = &["write", "edit", "apply_patch", "notebook_edit"];
// The file-editing tool names of the other flag-tracked clients, as their
// PostToolUse-style hook payloads report them in `tool_name`.
pub const CODEX_WORK_TOOLS: &[&str] = &["apply_patch"];
pub const GEMINI_WORK_TOOLS: &[&str] = &["write_file", "replace"];
// Cursor's documented native tools are Shell, Read, Write, Grep, Delete, and Task; file
// edits (not just whole-file writes) go through the same "Write" tool, with no separate
// "Edit"/"MultiEdit" name. https://cursor.com/docs/hooks
pub const CURSOR_WORK_TOOLS: &[&str] = &["Write"];

// Keep in sync with REVIEW_REMINDER in assets/opencode/skillvolution.js (a test in
// setup/plugin.rs checks it).
pub const STOP_REASON: &str = "Skillvolution review: you edited files or loaded vault skills since the last review. \
Follow the Report and Reflect steps of the evolution skill now: first call report_skill_outcome for every skill you loaded with get_skill \
(helped, failed with the wrong step, or not_applicable); then, for any candidate lesson that meets every lesson criterion, \
dispatch a fresh subagent that follows the skill-evaluation skill now \
— do not ask the user first — to judge it (global scope, project scope, or discard), then call publish_skill with the verdict and its reason. \
If there is nothing to report or evaluate, say so in a single short line with no elaboration and no heading.";

/// Prompt retrieval limits: at most `PROMPT_TERMS` keywords of at most
/// `MAX_TERM_BYTES` each (16 * 30 bytes plus separators stays under `search`'s
/// 512-byte query cap), `PROMPT_CANDIDATES` FTS hits judged, `PROMPT_MATCHES` kept.
const PROMPT_TERMS: usize = 16;
const MAX_TERM_BYTES: usize = 30;
const PROMPT_CANDIDATES: i64 = 5;
const PROMPT_MATCHES: usize = 3;
/// A prompt needs this many keywords before it is searched at all: one word left
/// after filtering ("tests", "hola") says too little about the task to suggest a skill.
const MIN_PROMPT_TERMS: usize = 2;
/// Net failures (failed - helped) on the current version that keep a skill out of
/// prompt suggestions: search only demotes it, but an unprompted suggestion should not
/// push a procedure that keeps failing.
const MAX_NET_FAILURES: i64 = 2;

/// Words that carry no task meaning, English then Spanish (users prompt in both),
/// whitespace-separated, already lowercased and stripped of diacritics the way `fold`
/// leaves a token. Words shorter than 3 characters are dropped separately, so they need
/// no entry here. "use" and "when" matter most: every skill description starts with
/// "Use when", so either one would otherwise match the whole vault.
const STOPWORDS: &str = "\
about above after again all also and any anything are aren because been before \
being below between both but can cannot could couldn did didn does doesn doing \
don done down during each else even every everything few for from further get \
gets getting give got had has hasn have haven having hello help her here hers \
herself hey him himself his how into isn its itself just know let lets like \
look make may might more most much must myself need needs new not now off once \
one only other our ours ourselves out over own please put really same see she \
should shouldn some something still such sure take than thank thanks that the \
their theirs them themselves then there these they thing things this those \
through too try under until upon use used uses using very want wants was wasn \
way well were weren what when where which while who whom why will with within \
without won would wouldn yes yet you your yours yourself yourselves \
ahora algo alguna algunas alguno algunos ante antes aqui asi aun ayuda ayudame \
ayudar bien cada como con contra cosa cosas cual cuales cuando cuanto dame del \
desde despues dime donde durante ella ellas ellos entonces entre era eres esa \
esas ese eso esos esta estan estar estas este esto estos estoy favor fue \
gracias haber hace hacer hacia hago hasta hay haz hola las les los luego mas \
mio mis mismo misma mucha muchas mucho muchos muy nada necesita necesito nos \
nosotros nuestra nuestro otra otras otro otros para pero poco por porque pues \
puede puedes puedo que quien quienes quiere quieres quiero segun sea ser sido \
sin sino sobre solo son suya suyo sus tal tambien tan tanto tengo tiene tienes \
todas todo todos toda tras tus tuyo una unas uno unos usa usar uso vez vosotros";

/// Lowercases `token` and strips the Latin diacritics FTS5's `remove_diacritics 2`
/// folds too, so "Cómo"/"como" and "configuración"/"configuracion" compare equal.
fn fold(token: &str) -> String {
    token
        .chars()
        .flat_map(char::to_lowercase)
        .map(|c| match c {
            'á' | 'à' | 'â' | 'ä' | 'ã' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'í' | 'ì' | 'î' | 'ï' => 'i',
            'ó' | 'ò' | 'ô' | 'ö' | 'õ' => 'o',
            'ú' | 'ù' | 'û' | 'ü' => 'u',
            'ñ' => 'n',
            'ç' => 'c',
            other => other,
        })
        .collect()
}

/// The search keywords in `text`: split on non-alphanumerics, folded (see `fold`),
/// dropping stopwords, tokens shorter than 3 characters or longer than
/// `MAX_TERM_BYTES` (hashes, paths), and repeats, in first-seen order, capped at
/// `PROMPT_TERMS`. FTS matches any prefix of any term, so every word let through
/// here can pull in skills; this keeps a prompt's filler from flooding the results.
pub fn keywords(text: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    text.split(|c: char| !c.is_alphanumeric())
        .map(fold)
        .filter(|term| term.chars().count() >= 3 && term.len() <= MAX_TERM_BYTES)
        .filter(|term| !STOPWORDS.split_whitespace().any(|word| word == term))
        .filter(|term| seen.insert(term.clone()))
        .take(PROMPT_TERMS)
        .collect()
}

/// The distinct `terms` that prefix some word of `text`: the same prefix semantics
/// FTS applies to the quoted `"term"*` expressions `search` builds.
fn matched<'a>(terms: &'a [String], text: &str) -> HashSet<&'a str> {
    let words: Vec<String> = text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(fold)
        .collect();
    terms
        .iter()
        .filter(|term| words.iter().any(|word| word.starts_with(term.as_str())))
        .map(String::as_str)
        .collect()
}

/// Whether a search hit is relevant enough to suggest unprompted. bm25 depends on the
/// whole corpus, so instead of a score threshold this re-checks the hit's own metadata:
/// it must match one keyword in its id or tags (the curated, most specific fields), or
/// two distinct keywords across id, description, and tags together. A hit that matched
/// only in its body does neither. A skill whose current version has `MAX_NET_FAILURES`
/// or more net failures is never suggested.
fn clears_gate(terms: &[String], skill: &SkillMetadata) -> bool {
    if skill.failed - skill.helped >= MAX_NET_FAILURES {
        return false;
    }
    let strong = matched(terms, &format!("{} {}", skill.id, skill.tags.join(" ")));
    !strong.is_empty() || strong.union(&matched(terms, &skill.description)).count() >= 2
}

/// The skills worth suggesting for `prompt`: the top `PROMPT_CANDIDATES` search hits
/// for its keywords, in search order, that clear `clears_gate`, at most
/// `PROMPT_MATCHES` of them. Empty for a prompt with fewer than `MIN_PROMPT_TERMS`
/// keywords.
pub fn relevant_skills(
    vault: &Vault,
    project: Option<&str>,
    prompt: &str,
) -> Result<Vec<SkillMetadata>> {
    let terms = keywords(prompt);
    if terms.len() < MIN_PROMPT_TERMS {
        return Ok(Vec::new());
    }
    let page = vault.search(&terms.join(" "), project, PROMPT_CANDIDATES, 0)?;
    Ok(page
        .skills
        .into_iter()
        .filter(|skill| clears_gate(&terms, skill))
        .take(PROMPT_MATCHES)
        .collect())
}

/// Prompt-submit hook: the context naming the vault skills that match the prompt in
/// the `prompt` field of `input` (the field Claude Code, Codex, Devin, and Gemini CLI
/// all use), or `None` when nothing is relevant, so the client adds nothing.
pub fn prompt(vault: &Vault, project: Option<&str>, input: &str) -> Result<Option<String>> {
    let input: Value = serde_json::from_str(input)?;
    let Some(prompt) = input["prompt"].as_str() else {
        return Ok(None);
    };
    let skills = relevant_skills(vault, project, prompt)?;
    if skills.is_empty() {
        return Ok(None);
    }
    let mut text = String::from("Skillvolution: vault skills matching this request:\n");
    for skill in &skills {
        writeln!(
            text,
            "- {} v{}: {} [helped {}, failed {}]",
            skill.id, skill.version, skill.description, skill.helped, skill.failed
        )?;
    }
    text.push_str(
        "Before starting, call get_skill for the best match and tell the user in one line \
         which vault skill you are applying (id and version), or that none fits. After \
         applying it, call report_skill_outcome.",
    );
    Ok(Some(text))
}

pub fn session_start(vault: &Vault, project: Option<&str>) -> Result<String> {
    let page = vault.search("", project, CATALOG_LIMIT, 0)?;
    let mut text = String::from(
        "Skillvolution vault (shared procedural memory). Before any non-trivial task, follow \
         step 1 of the `evolution` skill: check this catalog (run `search_skills` if nothing \
         here fits; matching skills are also suggested with each request), then tell the user \
         in one line which vault skill you are applying (id and version) or that none \
         matched. When you realize mid-task that you lack the know-how, follow the \
         `skill-acquisition` skill.\n",
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
/// last review tool call. Work is a file edit or a skill load (`get_skill`), whose
/// outcome is owed. A review or load whose tool_result is an error (a rejected
/// publish, an unknown skill id) did nothing, so it does not count.
fn has_unreviewed_work(span: &[u8]) -> bool {
    let mut position = 0;
    let mut last_edit = None;
    let mut loads = Vec::new();
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
                let id = object["id"].as_str().unwrap_or_default();
                if WORK_TOOLS.contains(&name) {
                    last_edit = Some(position);
                } else if is_load_tool(name) {
                    loads.push((position, id.to_owned()));
                } else if is_review_tool(name) {
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
    let last_success = |calls: Vec<(usize, String)>| {
        calls
            .into_iter()
            .filter(|(_, id)| !failed.contains(id))
            .map(|(position, _)| position)
            .max()
    };
    let last_work = last_edit.max(last_success(loads));
    let last_review = last_success(reviews);
    // None orders below any Some, so no work never blocks and work with no
    // successful review always does.
    last_work > last_review
}

fn is_review_tool(name: &str) -> bool {
    REVIEW_TOOLS.iter().any(|tool| name.ends_with(tool))
}

fn is_load_tool(name: &str) -> bool {
    name.ends_with(LOAD_TOOL)
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

/// The session a hook payload belongs to: `session_id`, or `conversation_id`
/// for clients that name it that way.
fn session_of(input: &Value) -> Option<&str> {
    input["session_id"]
        .as_str()
        .or_else(|| input["conversation_id"].as_str())
}

/// Devin PostToolUse: `tool_use` with Devin's file-editing tools.
pub fn devin_tool_use(vault: &Vault, input: &str) -> Result<()> {
    tool_use(vault, input, DEVIN_WORK_TOOLS)
}

/// PostToolUse for the flag-tracked clients (every client but Claude Code, whose
/// transcript is scanned instead): flags the session as having done work (which a
/// previous review no longer covers) when the tool is one of `work_tools` or the
/// vault's `get_skill`, or as having reviewed it after a review tool. Any other tool
/// leaves the flags alone. The flags live in `devin_hook_state`, whatever the client.
pub fn tool_use(vault: &Vault, input: &str, work_tools: &[&str]) -> Result<()> {
    let input: Value = serde_json::from_str(input)?;
    let (Some(session), Some(tool)) = (session_of(&input), input["tool_name"].as_str()) else {
        return Ok(());
    };
    if is_review_tool(tool) {
        vault.mark_devin_review(session)
    } else if work_tools.contains(&tool) || is_load_tool(tool) {
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
/// The same judgment serves every flag-tracked client.
pub fn devin_stop(vault: &Vault, input: &str) -> Result<Option<&'static str>> {
    let input: Value = serde_json::from_str(input)?;
    if input["stop_hook_active"].as_bool() == Some(true) {
        return Ok(None);
    }
    let Some(session) = session_of(&input) else {
        return Ok(None);
    };
    let (worked, reviewed) = vault.take_devin_hook_state(session)?;
    Ok((worked && !reviewed).then_some(STOP_REASON))
}

/// Devin SessionEnd: drops the session's flag row so the table does not grow
/// with dead sessions.
pub fn devin_session_end(vault: &Vault, input: &str) -> Result<()> {
    let input: Value = serde_json::from_str(input)?;
    if let Some(session) = session_of(&input) {
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
