use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::{
    io::{BufRead, Write},
    path::{Path, PathBuf},
};

use crate::vault::Vault;

const PROTOCOL_VERSION: &str = "2024-11-05";
const SERVER_NAME: &str = "skillvolution";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

fn tools() -> Vec<Value> {
    vec![
        json!({
            "name": "search_skills",
            "description": "List published skill metadata whose id or description contains the query. The full skill body is never returned; use get_skill to retrieve a specific revision.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "Free-text substring; empty string lists the entire catalog"},
                    "limit": {"type": "integer", "minimum": 1, "maximum": 100, "default": 20},
                    "offset": {"type": "integer", "minimum": 0, "default": 0}
                },
                "additionalProperties": false
            }
        }),
        json!({
            "name": "get_skill",
            "description": "Retrieve one published skill revision by id, optionally at an exact integer version. Drafts are not exposed.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": {"type": "string"},
                    "version": {"type": "integer", "minimum": 1}
                },
                "required": ["id"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "propose_skill_change",
            "description": "Store an immutable draft revision with supplied evidence. Agents must never publish drafts; a human reviews and runs `skillvolution publish`. `expected_version` must equal the current published version (0 for a new skill) or the proposal is rejected as stale.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": {"type": "string"},
                    "description": {"type": "string"},
                    "content": {"type": "string"},
                    "evidence": {"type": "string"},
                    "expected_version": {"type": "integer", "minimum": 0}
                },
                "required": ["id", "description", "content", "evidence", "expected_version"],
                "additionalProperties": false
            }
        }),
    ]
}

pub fn serve(database: PathBuf) -> Result<()> {
    let vault = Vault::open(&database)?;
    let database_for_writes = database;
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut input = stdin.lock();
    let mut output = stdout.lock();
    let mut initialized = false;

    let mut buffer = String::new();
    loop {
        buffer.clear();
        let read = input
            .read_line(&mut buffer)
            .context("reading MCP request line")?;
        if read == 0 {
            return Ok(());
        }
        let trimmed = buffer.trim();
        if trimmed.is_empty() {
            continue;
        }
        let request: Value = match serde_json::from_str(trimmed) {
            Ok(value) => value,
            Err(error) => {
                write_error(&mut output, None, -32700, &format!("parse error: {error}"))?;
                continue;
            }
        };
        let id = request.get("id").cloned();
        if id.is_none() {
            continue;
        }
        let id = id.unwrap();
        let method = match request.get("method").and_then(Value::as_str) {
            Some(method) => method.to_string(),
            None => {
                write_error(&mut output, Some(id), -32600, "missing method")?;
                continue;
            }
        };
        let params = request.get("params").cloned().unwrap_or(Value::Null);
        match dispatch(&vault, &database_for_writes, &mut initialized, &method, params, id) {
            Outcome::Reply { id, result } => {
                write_response(&mut output, Some(&id), result)?;
            }
            Outcome::Error { id, code, message } => {
                write_error(&mut output, Some(id), code, &message)?;
            }
        }
    }
}

enum Outcome {
    Reply {
        id: Value,
        result: Value,
    },
    Error {
        id: Value,
        code: i32,
        message: String,
    },
}

fn dispatch(
    vault: &Vault,
    database: &Path,
    initialized: &mut bool,
    method: &str,
    params: Value,
    id: Value,
) -> Outcome {
    match method {
        "initialize" => {
            *initialized = true;
            Outcome::Reply {
                id,
                result: json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "serverInfo": {"name": SERVER_NAME, "version": SERVER_VERSION},
                    "capabilities": {"tools": {"listChanged": false}}
                }),
            }
        }
        "ping" => Outcome::Reply { id, result: json!({}) },
        "tools/list" => {
            if !*initialized {
                return Outcome::Error {
                    id,
                    code: -32002,
                    message: "server not initialized".to_string(),
                };
            }
            Outcome::Reply {
                id,
                result: json!({ "tools": tools() }),
            }
        }
        "tools/call" => {
            if !*initialized {
                return Outcome::Error {
                    id,
                    code: -32002,
                    message: "server not initialized".to_string(),
                };
            }
            match handle_tool_call(vault, database, params) {
                Ok(result) => Outcome::Reply { id, result },
                Err(error) => Outcome::Reply {
                    id,
                    result: tool_error(&error.to_string()),
                },
            }
        }
        _ => Outcome::Error {
            id,
            code: -32601,
            message: format!("method not found: {method}"),
        },
    }
}

fn handle_tool_call(vault: &Vault, database: &Path, params: Value) -> Result<Value> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("missing tool name"))?;
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| Value::Object(Default::default()));
    match name {
        "search_skills" => call_search(vault, arguments),
        "get_skill" => call_get(vault, arguments),
        "propose_skill_change" => call_propose(database, arguments),
        other => bail!("unknown tool: {other}"),
    }
}

fn call_search(vault: &Vault, arguments: Value) -> Result<Value> {
    let query = arguments
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let limit = arguments.get("limit").and_then(Value::as_i64).unwrap_or(20);
    let offset = arguments.get("offset").and_then(Value::as_i64).unwrap_or(0);
    let page = vault.search(query, limit, offset)?;
    text_reply(serde_json::to_value(&page)?)
}

fn call_get(vault: &Vault, arguments: Value) -> Result<Value> {
    let id = arguments
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("missing id"))?;
    let version = arguments.get("version").and_then(Value::as_i64);
    let revision = vault.get(id, version)?;
    text_reply(serde_json::to_value(&revision)?)
}

fn call_propose(database: &Path, arguments: Value) -> Result<Value> {
    let id = arguments
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("missing id"))?;
    let description = arguments
        .get("description")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("missing description"))?;
    let content = arguments
        .get("content")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("missing content"))?;
    let evidence = arguments
        .get("evidence")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("missing evidence"))?;
    let expected_version = arguments
        .get("expected_version")
        .and_then(Value::as_i64)
        .ok_or_else(|| anyhow!("missing expected_version"))?;
    let mut owned = Vault::open(database)?;
    let revision = owned.propose(id, description, content, evidence, expected_version)?;
    text_reply(serde_json::to_value(&revision)?)
}

fn text_reply(value: Value) -> Result<Value> {
    let text = serde_json::to_string(&value)?;
    Ok(json!({
        "content": [{"type": "text", "text": text}],
        "isError": false,
    }))
}

fn tool_error(message: &str) -> Value {
    json!({
        "content": [{"type": "text", "text": message}],
        "isError": true,
    })
}

fn write_response<W: Write>(output: &mut W, id: Option<&Value>, result: Value) -> Result<()> {
    let payload = json!({
        "jsonrpc": "2.0",
        "id": id.cloned().unwrap_or(Value::Null),
        "result": result,
    });
    let line = serde_json::to_string(&payload)?;
    output.write_all(line.as_bytes())?;
    output.write_all(b"\n")?;
    output.flush()?;
    Ok(())
}

fn write_error<W: Write>(
    output: &mut W,
    id: Option<Value>,
    code: i32,
    message: &str,
) -> Result<()> {
    let payload = json!({
        "jsonrpc": "2.0",
        "id": id.unwrap_or(Value::Null),
        "error": {"code": code, "message": message},
    });
    let line = serde_json::to_string(&payload)?;
    output.write_all(line.as_bytes())?;
    output.write_all(b"\n")?;
    output.flush()?;
    Ok(())
}
