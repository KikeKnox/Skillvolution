use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::{Value, json};
use std::io::{BufRead, Read, Write};

use crate::vault::{Proposal, Vault};

const PROTOCOL_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];
const SERVER_NAME: &str = "skillvolution";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");
/// A request line larger than this is refused outright rather than buffered
/// in full; 4 MiB comfortably covers the largest legitimate tool call
/// (`content` alone is capped at 65,536 bytes) with headroom to spare.
const MAX_LINE_BYTES: u64 = 4 * 1024 * 1024;
/// Matches `validate_id` in `vault/mod.rs`: lowercase letters and digits,
/// separated by single hyphens. Applies to skill ids and, separately, to each
/// tag (tags additionally cap at 32 bytes and 8 items — see `validate_tags`).
const ID_PATTERN: &str = "^[a-z0-9]+(-[a-z0-9]+)*$";

fn tools() -> Value {
    json!([
        {
            "name": "search_skills",
            "description": "Search the shared skill vault before starting a non-trivial task. Matches words in skill ids, descriptions, tags, and bodies, ranked by relevance and by reported outcomes: a skill with more failed than helped reports on its current version is demoted, so it can land below a weaker text match, while a proven one is promoted. Returns `skills` (metadata only: id, version, description, tags, helped/failed counts — call get_skill for the body), `total`, and `has_more` for pagination. An empty query lists the catalog.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {"type": "string", "maxLength": 512, "description": "Keywords such as technology, action, and symptom, e.g. 'cargo flaky test timeout'"},
                    "limit": {"type": "integer", "minimum": 1, "maximum": 100, "default": 20},
                    "offset": {"type": "integer", "minimum": 0, "default": 0}
                },
                "additionalProperties": false
            },
            "annotations": {"readOnlyHint": true}
        },
        {
            "name": "get_skill",
            "description": "Load the body of one published skill that matched your task. Defaults to the latest published version. Only returns skills visible to the current project (global skills, plus this project's own); a deprecated skill still returns its body, with a `deprecated: true` flag.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": {"type": "string", "maxLength": 64, "pattern": ID_PATTERN},
                    "version": {"type": "integer", "minimum": 1}
                },
                "required": ["id"],
                "additionalProperties": false
            },
            "annotations": {"readOnlyHint": true}
        },
        {
            "name": "report_skill_outcome",
            "description": "After applying a skill loaded with get_skill, record whether it helped. Failures with a concrete note are the most valuable signal for improving skills.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": {"type": "string", "maxLength": 64, "pattern": ID_PATTERN},
                    "version": {"type": "integer", "minimum": 1},
                    "result": {"type": "string", "enum": ["helped", "failed", "not_applicable"]},
                    "note": {"type": "string", "maxLength": 2048, "description": "What happened when the skill was applied; for failures, which step was wrong and why"}
                },
                "required": ["id", "version", "result", "note"],
                "additionalProperties": false
            },
            "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": true}
        },
        {
            "name": "publish_skill",
            "description": "Publish a new skill or a complete replacement of an existing one, visible to agents immediately. Publish only lessons that a fresh-context evaluation judged worth keeping: verified, reusable, non-obvious. `scope` is fixed on an id's first publish and can't change on later revisions. `content` must contain the headings `## When to use`, `## Procedure`, `## Pitfalls`, and `## Verification`, in that order (extra `##` sections are fine). `description`, `content`, and `evidence` are scanned for credential-shaped values and the publish is refused if one is found. `verdict` must be exactly \"keep global\" or \"keep project\" (compared case-insensitively after trimming) and must agree with `scope`.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": {"type": "string", "maxLength": 64, "pattern": ID_PATTERN, "description": "Stable lowercase-hyphenated id, e.g. 'rust-sqlite-busy-timeout'"},
                    "description": {"type": "string", "maxLength": 280, "description": "One line starting with 'Use when', naming the situation that should trigger the skill"},
                    "tags": {"type": "array", "items": {"type": "string", "maxLength": 32, "pattern": ID_PATTERN}, "maxItems": 8},
                    "content": {"type": "string", "maxLength": 65536, "description": "Full markdown body with sections: When to use, Procedure, Pitfalls, Verification"},
                    "evidence": {"type": "string", "maxLength": 16384, "description": "Observed / Tried / Result: what actually happened in this session"},
                    "expected_version": {"type": "integer", "minimum": 0, "description": "Current published version of the skill, or 0 for a new skill"},
                    "scope": {"type": "string", "enum": ["global", "project"], "default": "global", "description": "project when the lesson only applies to this repository"},
                    "verdict": {"type": "string", "enum": ["keep global", "keep project"], "description": "The fresh-context evaluator's verdict line, verbatim. A discard verdict must never be published."},
                    "verdict_reason": {"type": "string", "maxLength": 280, "description": "The evaluator's one-line reason for that verdict, verbatim"},
                    "replaces_proven": {"type": "boolean", "default": false, "description": "Set true only when replacing a version with more helped than failed reports, after merging its content rather than rewriting it"}
                },
                "required": ["id", "description", "content", "evidence", "expected_version", "verdict", "verdict_reason"],
                "additionalProperties": false
            },
            "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": false}
        }
    ])
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchArgs {
    #[serde(default)]
    query: String,
    #[serde(default = "default_limit")]
    limit: i64,
    #[serde(default)]
    offset: i64,
}

fn default_limit() -> i64 {
    20
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GetArgs {
    id: String,
    version: Option<i64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OutcomeArgs {
    id: String,
    version: i64,
    result: String,
    note: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PublishArgs {
    id: String,
    description: String,
    #[serde(default)]
    tags: Vec<String>,
    content: String,
    evidence: String,
    expected_version: i64,
    #[serde(default)]
    scope: Option<String>,
    verdict: String,
    verdict_reason: String,
    #[serde(default)]
    replaces_proven: bool,
}

struct Server {
    vault: Vault,
    project: Option<String>,
}

pub fn serve(vault: Vault, project: Option<String>) -> Result<()> {
    let mut server = Server { vault, project };
    let mut output = std::io::stdout().lock();
    let mut input = std::io::stdin().lock();
    let mut line = Vec::new();
    loop {
        line.clear();
        let read = (&mut input)
            .take(MAX_LINE_BYTES)
            .read_until(b'\n', &mut line)
            .context("reading MCP request line")?;
        if read == 0 {
            return Ok(());
        }
        if line.len() as u64 >= MAX_LINE_BYTES && !line.ends_with(b"\n") {
            // The rest of this oversized line is still sitting unread on
            // stdin; drain it so the next `read_until` starts clean on the
            // following line instead of parsing its tail as a new request.
            drain_until_newline(&mut input)?;
            write_message(
                &mut output,
                &error_message(
                    Value::Null,
                    -32600,
                    &format!("request line exceeds {MAX_LINE_BYTES} bytes"),
                ),
            )?;
            continue;
        }
        let trimmed = line.strip_suffix(b"\n").unwrap_or(&line);
        let trimmed = trimmed.strip_suffix(b"\r").unwrap_or(trimmed);
        if trimmed.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        // A single malformed line (invalid UTF-8, or valid UTF-8 that isn't
        // JSON) must not take the whole server down: reply with a parse
        // error and keep reading.
        let text = match std::str::from_utf8(trimmed) {
            Ok(text) => text,
            Err(error) => {
                write_message(
                    &mut output,
                    &error_message(Value::Null, -32700, &format!("parse error: {error}")),
                )?;
                continue;
            }
        };
        let request: Value = match serde_json::from_str(text) {
            Ok(value) => value,
            Err(error) => {
                write_message(
                    &mut output,
                    &error_message(Value::Null, -32700, &format!("parse error: {error}")),
                )?;
                continue;
            }
        };
        if request.is_array() {
            write_message(
                &mut output,
                &error_message(Value::Null, -32600, "batch requests are not supported"),
            )?;
            continue;
        }
        // A JSON-RPC id must be a string, number, or null; an id we can't
        // trust the shape of gets `null` back rather than being echoed.
        let jsonrpc_valid = request.get("jsonrpc").and_then(Value::as_str) == Some("2.0");
        let id_shape_valid = !matches!(
            request.get("id"),
            Some(Value::Object(_)) | Some(Value::Array(_))
        );
        if !jsonrpc_valid || !id_shape_valid {
            // JSON-RPC forbids replying to a notification (an object with no
            // `id` member), even with an error.
            if request.as_object().is_some_and(|o| !o.contains_key("id")) {
                eprintln!("skillvolution: ignoring invalid notification: {text}");
                continue;
            }
            write_message(
                &mut output,
                &error_message(
                    Value::Null,
                    -32600,
                    "request must have \"jsonrpc\": \"2.0\" and an id that is a string, number, or null",
                ),
            )?;
            continue;
        }
        let method = request.get("method").and_then(Value::as_str);
        let is_response =
            method.is_none() && (request.get("result").is_some() || request.get("error").is_some());
        if is_response {
            continue;
        }
        let Some(id) = request.get("id").cloned() else {
            continue;
        };
        let Some(method) = method else {
            write_message(&mut output, &error_message(id, -32600, "missing method"))?;
            continue;
        };
        let params = request.get("params").cloned().unwrap_or(Value::Null);
        let reply = match server.dispatch(method, params) {
            Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
            Err((code, message)) => error_message(id, code, &message),
        };
        write_message(&mut output, &reply)?;
    }
}

/// Discards input up to and including the next newline, in bounded chunks, so
/// the tail of a line rejected as oversized isn't parsed as the start of the
/// next request.
fn drain_until_newline(input: &mut impl BufRead) -> Result<()> {
    let mut junk = Vec::new();
    loop {
        junk.clear();
        let read = (&mut *input)
            .take(MAX_LINE_BYTES)
            .read_until(b'\n', &mut junk)
            .context("draining oversized MCP request line")?;
        if read == 0 || junk.ends_with(b"\n") {
            return Ok(());
        }
    }
}

/// A tool call can fail two different ways: a malformed request the client
/// sent (unknown tool, missing `name`) is a JSON-RPC protocol error, while a
/// failure the tool itself raised (bad argument values, a vault error) is
/// reported as an `isError` tool result so the model can see and react to it.
enum ToolCallError {
    InvalidParams(String),
    ToolFailure(anyhow::Error),
}

impl From<anyhow::Error> for ToolCallError {
    fn from(error: anyhow::Error) -> Self {
        ToolCallError::ToolFailure(error)
    }
}

impl From<serde_json::Error> for ToolCallError {
    fn from(error: serde_json::Error) -> Self {
        ToolCallError::ToolFailure(error.into())
    }
}

impl Server {
    fn dispatch(&mut self, method: &str, params: Value) -> Result<Value, (i32, String)> {
        match method {
            "initialize" => Ok(initialize(&params)),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({"tools": tools()})),
            "tools/call" => match self.call(params) {
                Ok(value) => Ok(json!({
                    "content": [{"type": "text", "text": value.to_string()}],
                    "isError": false,
                })),
                Err(ToolCallError::InvalidParams(message)) => Err((-32602, message)),
                Err(ToolCallError::ToolFailure(error)) => Ok(json!({
                    "content": [{"type": "text", "text": format!("{error:#}")}],
                    "isError": true,
                })),
            },
            _ => Err((-32601, format!("method not found: {method}"))),
        }
    }

    fn call(&mut self, params: Value) -> Result<Value, ToolCallError> {
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolCallError::InvalidParams("missing tool name".to_string()))?;
        let arguments = match params.get("arguments") {
            None | Some(Value::Null) => json!({}),
            Some(arguments) => arguments.clone(),
        };
        let project = self.project.as_deref();
        let parse_error = |error| anyhow::anyhow!("invalid arguments for {name}: {error}");
        match name {
            "search_skills" => {
                let args: SearchArgs = serde_json::from_value(arguments).map_err(parse_error)?;
                let page = self
                    .vault
                    .search(&args.query, project, args.limit, args.offset)?;
                Ok(serde_json::to_value(page)?)
            }
            "get_skill" => {
                let args: GetArgs = serde_json::from_value(arguments).map_err(parse_error)?;
                Ok(serde_json::to_value(self.vault.get(
                    &args.id,
                    args.version,
                    project,
                )?)?)
            }
            "report_skill_outcome" => {
                let args: OutcomeArgs = serde_json::from_value(arguments).map_err(parse_error)?;
                self.vault.record_outcome(
                    &args.id,
                    args.version,
                    &args.result,
                    &args.note,
                    project,
                )?;
                Ok(json!({"recorded": true}))
            }
            "publish_skill" => {
                let args: PublishArgs = serde_json::from_value(arguments).map_err(parse_error)?;
                let scope = match args.scope.as_deref() {
                    None | Some("global") => None,
                    Some("project") => Some(project.context(
                        "this server has no project key: it was not started inside a git \
                         repository (or with --project); use scope global instead",
                    )?),
                    Some(other) => {
                        return Err(anyhow::anyhow!(
                            "scope must be global or project, not {other:?}"
                        )
                        .into());
                    }
                };
                let revision = self.vault.propose(&Proposal {
                    id: &args.id,
                    description: &args.description,
                    tags: &args.tags,
                    content: &args.content,
                    evidence: &args.evidence,
                    expected_version: args.expected_version,
                    scope,
                    verdict: &args.verdict,
                    verdict_reason: &args.verdict_reason,
                    replaces_proven: args.replaces_proven,
                })?;
                Ok(json!({
                    "id": revision.id,
                    "version": revision.version,
                    "status": revision.status,
                    "expected_version": revision.expected_version,
                    "scope": revision.scope,
                    "next": "Published and immediately visible to agents. Tell the user the skill id and version; they can hide it later with `skillvolution deprecate ID`."
                }))
            }
            other => Err(ToolCallError::InvalidParams(format!(
                "unknown tool: {other}"
            ))),
        }
    }
}

fn initialize(params: &Value) -> Value {
    let requested = params.get("protocolVersion").and_then(Value::as_str);
    let version = PROTOCOL_VERSIONS
        .into_iter()
        .find(|supported| Some(*supported) == requested)
        .unwrap_or(PROTOCOL_VERSIONS[0]);
    json!({
        "protocolVersion": version,
        "serverInfo": {"name": SERVER_NAME, "version": SERVER_VERSION},
        "capabilities": {"tools": {"listChanged": false}},
        "instructions": "Call search_skills before starting non-trivial work, and get_skill to load a match's body. \
            After applying a skill, call report_skill_outcome so future rankings reflect what happened. \
            Only call publish_skill after a fresh-context evaluator has verified the lesson is worth keeping."
    })
}

fn error_message(id: Value, code: i32, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

fn write_message(output: &mut impl Write, message: &Value) -> Result<()> {
    serde_json::to_writer(&mut *output, message)?;
    output.write_all(b"\n")?;
    output.flush()?;
    Ok(())
}
