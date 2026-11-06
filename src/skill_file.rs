//! The SKILL.md / AGENT.md format `publish` reads and `get` prints: a small
//! frontmatter subset (one-line values, inline `[a, b]` or block `- a` lists)
//! and a markdown body.

use crate::{setup::MANAGED_MARKER, vault::AgentMeta};

/// A skill or agent file's frontmatter fields and body.
#[derive(Default)]
pub struct SkillFile {
    pub name: Option<String>,
    pub description: Option<String>,
    pub tags: Vec<String>,
    /// `global` or `project` (anything else is reported in `problems`).
    pub scope: Option<String>,
    /// Agent fields; any of `role`/`tier` makes the file an agent.
    pub role: Option<String>,
    pub tier: Option<String>,
    pub effort: Option<String>,
    pub tools: Option<Vec<String>>,
    pub skills: Vec<String>,
    /// Claude Code subagent fields, read by `agents import`.
    pub model: Option<String>,
    pub disallowed_tools: Option<Vec<String>>,
    pub content: String,
    pub problems: Vec<String>,
}

impl SkillFile {
    pub fn is_agent(&self) -> bool {
        self.role.is_some() || self.tier.is_some()
    }

    /// The agent metadata the frontmatter declares (role defaults to subagent,
    /// a missing tier is left empty so validation names it).
    pub fn agent_meta(&self) -> AgentMeta {
        AgentMeta {
            role: self.role.clone().unwrap_or_else(|| "subagent".into()),
            tier: self.tier.clone().unwrap_or_default(),
            effort: self.effort.clone(),
            tools: self.tools.clone(),
            skills: self.skills.clone(),
        }
    }
}

fn unquote(value: &str) -> String {
    let v = value.trim();
    if v.len() >= 2 && v.starts_with('"') && v.ends_with('"') {
        // A JSON string is a valid YAML double-quoted scalar; `get` writes those.
        if let Ok(text) = serde_json::from_str::<String>(v) {
            return text;
        }
    }
    for q in ['"', '\''] {
        if let Some(inner) = v.strip_prefix(q).and_then(|v| v.strip_suffix(q))
            && !inner.contains(q)
        {
            return inner.to_owned();
        }
    }
    v.to_owned()
}

/// `value` without a YAML-style trailing comment: a `#` at the start or after
/// whitespace, outside a quoted value ("C#" and "a#b" keep theirs).
fn strip_comment(value: &str) -> &str {
    let v = value.trim();
    // A leading quoted string can hold a " #" of its own: look past its closing quote.
    let from = v
        .strip_prefix(['"', '\''])
        .and_then(|rest| closing_quote(rest, &v[..1]))
        .map_or(0, |end| end + 2);
    v.match_indices('#')
        .find(|&(i, _)| i >= from && (i == 0 || v[..i].ends_with(char::is_whitespace)))
        .map_or(v, |(i, _)| v[..i].trim_end())
}

/// Byte offset of the closing `quote` in `rest`; in a double-quoted scalar a
/// backslash escapes the next character.
fn closing_quote(rest: &str, quote: &str) -> Option<usize> {
    if quote != "\"" {
        return rest.find(quote);
    }
    let mut chars = rest.char_indices();
    while let Some((i, c)) = chars.next() {
        match c {
            '\\' => {
                chars.next();
            }
            '"' => return Some(i),
            _ => {}
        }
    }
    None
}

fn inline_list(value: &str) -> Vec<String> {
    value
        .trim_start_matches('[')
        .trim_end_matches(']')
        .split(',')
        .map(unquote)
        .filter(|t| !t.is_empty())
        .collect()
}

const LIST_KEYS: [&str; 4] = ["tags", "tools", "skills", "disallowedTools"];

pub fn parse(text: &str) -> SkillFile {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let text = text.replace("\r\n", "\n");
    let text = text.as_str();
    let mut file = SkillFile::default();
    let mut body = text;
    let mut lines = text.lines();
    if lines.next().is_some_and(|l| l.trim_end() == "---") {
        let mut consumed = text.lines().next().unwrap().len() + 1;
        let mut closed = false;
        // The list key whose `- item` lines are being read.
        let mut block: Option<(&str, Vec<String>)> = None;
        for line in lines {
            consumed += line.len() + 1;
            let item = line.trim_start().strip_prefix("- ").map(strip_comment);
            if let (Some((_, items)), Some(item)) = (&mut block, item) {
                items.push(unquote(item));
                continue;
            }
            if let Some((key, items)) = block.take() {
                set_block_list(&mut file, key, items);
            }
            if line.trim_end() == "---" {
                body = text.get(consumed..).unwrap_or("");
                closed = true;
                break;
            }
            if line.starts_with(char::is_whitespace) {
                continue;
            }
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            let (key, value) = (key.trim_end(), strip_comment(value));
            if let Some(list_key) = LIST_KEYS.iter().find(|k| **k == key) {
                if value.is_empty() {
                    block = Some((list_key, Vec::new()));
                } else {
                    let items = inline_list(value);
                    if *list_key == "tools" && items.is_empty() {
                        file.problems.push(
                            "tools: empty list; omit `tools` for all tools or list at least one"
                                .into(),
                        );
                    } else {
                        set_list(&mut file, list_key, items);
                    }
                }
                continue;
            }
            if matches!(key, "name" | "description")
                && matches!(value, "" | ">" | "|" | ">-" | "|-")
            {
                file.problems.push(format!(
                    "{key}: multi-line YAML values are not supported; write it on one line"
                ));
                continue;
            }
            let value = unquote(value);
            match key {
                "name" => file.name = Some(value),
                "description" => file.description = Some(value),
                "scope" => match value.as_str() {
                    "global" | "project" => file.scope = Some(value),
                    other => file
                        .problems
                        .push(format!("scope: `{other}` is not global or project")),
                },
                "role" => file.role = Some(value),
                "tier" => file.tier = Some(value),
                "effort" => file.effort = Some(value),
                "model" => file.model = Some(value),
                _ => {}
            }
        }
        if let Some((key, items)) = block.take() {
            set_block_list(&mut file, key, items);
        }
        if !closed {
            file.problems
                .push("frontmatter starts with --- but is never closed by a --- line".into());
        }
    }
    file.content = body
        .lines()
        .filter(|l| {
            !l.trim_start()
                .strip_prefix("<!-- ")
                .is_some_and(|rest| rest.starts_with(MANAGED_MARKER))
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim_start_matches('\n')
        .to_owned();
    file
}

/// An AGENT.md that `parse` reads back: frontmatter for `meta` (and `tags`, and `scope:
/// project` when `scope` is given), then `content` with no trailing newline.
pub fn agent_md(
    id: &str,
    description: &str,
    tags: &[String],
    scope: Option<&str>,
    meta: &AgentMeta,
    content: &str,
) -> String {
    let list = |items: &[String]| format!("[{}]", items.join(", "));
    let mut out = format!(
        "---\nname: {}\ndescription: {}\n",
        yaml_scalar(id),
        yaml_scalar(description)
    );
    if !tags.is_empty() {
        out += &format!("tags: {}\n", list(tags));
    }
    out += &format!("role: {}\ntier: {}\n", meta.role, meta.tier);
    if let Some(effort) = &meta.effort {
        out += &format!("effort: {}\n", yaml_scalar(effort));
    }
    if let Some(tools) = &meta.tools {
        out += &format!("tools: {}\n", list(tools));
    }
    if !meta.skills.is_empty() {
        out += &format!("skills: {}\n", list(&meta.skills));
    }
    if let Some(scope) = scope {
        out += &format!("scope: {scope}\n");
    }
    out + "---\n" + content.trim_end()
}

/// A block list's items; a bare `tools:` with none means all tools (`None`).
fn set_block_list(file: &mut SkillFile, key: &str, items: Vec<String>) {
    if !(key == "tools" && items.is_empty()) {
        set_list(file, key, items);
    }
}

fn set_list(file: &mut SkillFile, key: &str, items: Vec<String>) {
    match key {
        "tags" => file.tags = items,
        "tools" => file.tools = Some(items),
        "skills" => file.skills = items,
        _ => file.disallowed_tools = Some(items),
    }
}

/// `text` as a YAML scalar: plain when that reads back unchanged, else a
/// double-quoted (JSON) string.
pub fn yaml_scalar(text: &str) -> String {
    let lower = text.to_ascii_lowercase();
    let hex = ["0x", "0o"].iter().any(|p| {
        lower
            .strip_prefix(p)
            .is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_hexdigit()))
    });
    let special = matches!(
        lower.as_str(),
        "true" | "false" | "null" | "yes" | "no" | "on" | "off" | "~"
    ) || hex
        || text.parse::<f64>().is_ok();
    let plain = !special
        && !text.is_empty()
        && !text
            .chars()
            .zip(text.chars().skip(1))
            .any(|(a, b)| (a == ':' && b.is_whitespace()) || (a.is_whitespace() && b == '#'))
        && !text.ends_with(':')
        && !text.starts_with(|c: char| "[]{}&*!|>'\"%@`#,?-".contains(c) || c.is_whitespace())
        && !text.ends_with(char::is_whitespace);
    if plain {
        text.to_owned()
    } else {
        serde_json::Value::from(text).to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_agent_fields_inline_and_block_lists() {
        let file = parse(
            "---\nname: a\ndescription: \"Use when: x\"\ntier: mid # comment\ntools: [read, edit]\nskills:\n  - one\n  - two\ndisallowedTools: Write, Edit\n---\n## Output contract\nx\n",
        );
        assert!(file.problems.is_empty(), "{:?}", file.problems);
        assert_eq!(file.description.as_deref(), Some("Use when: x"));
        assert_eq!(file.tier.as_deref(), Some("mid"));
        assert_eq!(file.tools, Some(vec!["read".into(), "edit".into()]));
        assert_eq!(file.skills, ["one", "two"]);
        assert_eq!(
            file.disallowed_tools,
            Some(vec!["Write".into(), "Edit".into()])
        );
        assert!(file.is_agent());
        assert_eq!(file.content, "## Output contract\nx");
    }

    #[test]
    fn yaml_scalar_quotes_only_when_needed() {
        for text in [
            "Use when x",
            "Use when: x",
            "- dash",
            "a #b",
            "a\t#b",
            "Use when \"x\" breaks #2",
            "say \"hi\" #1",
            "Use when \"cargo test\" fails on issue #12: x",
            "\"q\"",
            "true",
            "Yes",
            "null",
            "~",
            "123",
            "1e3",
            "0x1f",
            ".5",
        ] {
            let line = format!("---\ndescription: {}\n---\n", yaml_scalar(text));
            assert_eq!(parse(&line).description.as_deref(), Some(text), "{line}");
        }
        assert_eq!(yaml_scalar("Use when x"), "Use when x");
        assert_eq!(yaml_scalar("truest"), "truest");
        assert_eq!(yaml_scalar("a#b"), "a#b");
    }

    #[test]
    fn escaped_quotes_do_not_end_a_quoted_value_early() {
        let file = parse("---\ndescription: \"say \\\"hi\\\" #1\" # real\n---\n");
        assert_eq!(file.description.as_deref(), Some("say \"hi\" #1"));
    }

    #[test]
    fn empty_tools_means_all_tools_unless_written_as_an_empty_list() {
        for text in [
            "---\ntools:\ntier: mid\n---\n",
            "---\ntier: mid\ntools:\n---\n",
        ] {
            let file = parse(text);
            assert_eq!(file.tools, None, "{text}");
            assert!(file.problems.is_empty(), "{:?}", file.problems);
        }
        let file = parse("---\ntools: []\n---\n");
        assert_eq!(file.tools, None);
        assert_eq!(file.problems.len(), 1);
        assert!(file.problems[0].contains("tools: empty list"));
        assert_eq!(
            parse("---\ntools:\n  - read\n---\n").tools,
            Some(vec!["read".into()])
        );
    }

    #[test]
    fn a_leading_bom_is_ignored() {
        let file = parse("\u{feff}---\nname: a\n---\nbody\n");
        assert_eq!(file.name.as_deref(), Some("a"));
        assert_eq!(file.content, "body");
    }
}
