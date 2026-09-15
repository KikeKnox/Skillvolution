use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::{Value, json};
use std::io::{BufRead, Write};

use crate::vault::{Proposal, Vault};

const PROTOCOL_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];
const SERVER_NAME: &str = "skillvolution";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

fn tools() -> Value {
    json!([
        {
            "name": "search_skills",
            "description": "Search the shared skill vault before starting a non-trivial task. Matches words in skill ids, descriptions, tags, and bodies, ranked by relevance. Returns metadata only (id, version, description, tags, helped/failed counts); call get_skill for the body. An empty query lists the catalog.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "Keywords such as technology, action, and symptom, e.g. 'cargo flaky test timeout'"},
                    "limit": {"type": "integer", "minimum": 1, "maximum": 100, "default": 20},
                    "offset": {"type": "integer", "minimum": 0, "default": 0}
                },
                "additionalProperties": false
            }
        },
        {
            "name": "get_skill",
            "description": "Load the body of one published skill that matched your task. Defaults to the latest published version.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": {"type": "string"},
                    "version": {"type": "integer", "minimum": 1}
                },
                "required": ["id"],
                "additionalProperties": false
            }
        },
        {
            "name": "report_skill_outcome",
            "description": "After applying a skill loaded with get_skill, record whether it helped. Failures with a concrete note are the most valuable signal for improving skills.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": {"type": "string"},
                    "version": {"type": "integer", "minimum": 1},
                    "result": {"type": "string", "enum": ["helped", "failed", "not_applicable"]},
                    "note": {"type": "string", "description": "What happened when the skill was applied; for failures, which step was wrong and why"}
                },
                "required": ["id", "version", "result", "note"],
                "additionalProperties": false
            }
        },
        {
            "name": "propose_skill_change",
            "description": "Store a draft of a new skill or a complete replacement of an existing one, for human review. Only propose verified, reusable, non-obvious lessons. Drafts are never visible to agents until a human publishes them.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": {"type": "string", "description": "Stable lowercase-hyphenated id, e.g. 'rust-sqlite-busy-timeout'"},
                    "description": {"type": "string", "description": "One line starting with 'Use when', naming the situation that should trigger the skill"},
                    "tags": {"type": "array", "items": {"type": "string"}, "maxItems": 8},
                    "content": {"type": "string", "description": "Full markdown body with sections: When to use, Procedure, Pitfalls, Verification"},
                    "evidence": {"type": "string", "description": "Observed / Tried / Result: what actually happened in this session"},
                    "expected_version": {"type": "integer", "minimum": 0, "description": "Current published version of the skill, or 0 for a new skill"},
                    "scope": {"type": "string", "enum": ["global", "project"], "default": "global", "description": "project when the lesson only applies to this repository"}
                },
                "required": ["id", "description", "content", "evidence", "expected_version"],
                "additionalProperties": false
            }
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
struct ProposeArgs {
    id: String,
    description: String,
    #[serde(default)]
    tags: Vec<String>,
    content: String,
    evidence: String,
    expected_version: i64,
    #[serde(default)]
    scope: Option<String>,
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
        let read = input
            .read_until(b'\n', &mut line)
            .context("reading MCP request line")?;
        if read == 0 {
            return Ok(());
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

impl Server {
    fn dispatch(&mut self, method: &str, params: Value) -> Result<Value, (i32, String)> {
        match method {
            "initialize" => Ok(initialize(&params)),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({"tools": tools()})),
            "tools/call" => Ok(match self.call(params) {
                Ok(value) => json!({
                    "content": [{"type": "text", "text": value.to_string()}],
                    "isError": false,
                }),
                Err(error) => json!({
                    "content": [{"type": "text", "text": format!("{error:#}")}],
                    "isError": true,
                }),
            }),
            _ => Err((-32601, format!("method not found: {method}"))),
        }
    }

    fn call(&mut self, params: Value) -> Result<Value> {
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .context("missing tool name")?;
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
            "propose_skill_change" => {
                let args: ProposeArgs = serde_json::from_value(arguments).map_err(parse_error)?;
                let scope = match args.scope.as_deref() {
                    None | Some("global") => None,
                    Some("project") => Some(project.context(
                        "this server has no project key; use scope global or rerun setup",
                    )?),
                    Some(other) => bail!("scope must be global or project, not {other:?}"),
                };
                let revision = self.vault.propose(&Proposal {
                    id: &args.id,
                    description: &args.description,
                    tags: &args.tags,
                    content: &args.content,
                    evidence: &args.evidence,
                    expected_version: args.expected_version,
                    scope,
                })?;
                Ok(json!({
                    "id": revision.id,
                    "version": revision.version,
                    "status": revision.status,
                    "expected_version": revision.expected_version,
                    "scope": revision.scope,
                    "next": "Tell the user a draft awaits human review: skillvolution show ID --version N, then publish or reject. Never publish it yourself."
                }))
            }
            other => bail!("unknown tool: {other}"),
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
        "capabilities": {"tools": {"listChanged": false}}
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
