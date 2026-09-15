use anyhow::{bail, ensure, Context, Result};
use serde::de::{Deserializer, MapAccess, Visitor};
use serde_json::{json, Value};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

const SKILL: &str = include_str!("../assets/evolution/SKILL.md");
const OWNER: &str = "<!-- skillvolution-managed:evolution:v1 -->";
const START: &str = "<!-- skillvolution:evolution:start -->";
const END: &str = "<!-- skillvolution:evolution:end -->";
const CLAUDE: &str = "<!-- skillvolution:evolution:start -->\n@.claude/skills/evolution/SKILL.md\n<!-- skillvolution:evolution:end -->";

#[derive(clap::Args)]
pub struct SetupArgs {
    #[arg(long)]
    project: PathBuf,
    #[arg(long, default_value = "both", value_parser = ["both", "opencode", "claude-code"])]
    client: String,
    #[arg(long)]
    bin: PathBuf,
    #[arg(long)]
    db: PathBuf,
}

pub fn run(args: SetupArgs) -> Result<()> {
    for path in [&args.project, &args.bin, &args.db] {
        check_path(path)?;
    }
    let project = fs::canonicalize(&args.project).context("project must exist")?;
    let bin = fs::canonicalize(&args.bin).context("binary must exist")?;
    let db = std::path::absolute(&args.db)?;
    ensure!(project.is_dir(), "project must be a directory");
    ensure!(bin.is_file(), "binary must be a regular file");
    ensure!(
        !db.exists() || db.is_file(),
        "database must be a regular file or a new path"
    );
    let mut changes = Vec::new();
    if args.client != "claude-code" {
        ensure!(
            !project.join("opencode.jsonc").exists(),
            "opencode.jsonc is unsupported; merge it into strict opencode.json manually first"
        );
        let skill_path = project.join(".opencode/skills/evolution/SKILL.md");
        let owned = check_skill(&skill_path)?;
        let path = project.join("opencode.json");
        let mut config = load_json(&path)?;
        merge_server(
            &mut config,
            "mcp",
            json!({"type":"local", "command":[bin,"--db",db,"serve"], "enabled":true}),
            owned,
        )?;
        let instructions = config
            .as_object_mut()
            .context("config must be an object")?
            .entry("instructions")
            .or_insert(json!([]));
        let instructions = instructions
            .as_array_mut()
            .context("instructions must be an array of strings")?;
        ensure!(
            instructions.iter().all(Value::is_string),
            "instructions must be an array of strings"
        );
        let reference = json!(".opencode/skills/evolution/SKILL.md");
        if !instructions.contains(&reference) {
            instructions.push(reference);
        }
        changes.push((path, serde_json::to_string_pretty(&config)? + "\n"));
        changes.push((skill_path, SKILL.to_owned()));
    }
    if args.client != "opencode" {
        let skill_path = project.join(".claude/skills/evolution/SKILL.md");
        let owned = check_skill(&skill_path)?;
        let path = project.join(".mcp.json");
        let mut config = load_json(&path)?;
        merge_server(
            &mut config,
            "mcpServers",
            json!({"type":"stdio", "command":bin, "args":["--db",db,"serve"]}),
            owned,
        )?;
        changes.push((path, serde_json::to_string_pretty(&config)? + "\n"));
        changes.push((skill_path, SKILL.to_owned()));
        let path = project.join("CLAUDE.md");
        let text = read_optional(&path)?.unwrap_or_default();
        changes.push((path, merge_claude(text)?));
    }
    for (path, _) in &changes {
        check_path(path)?;
        if path.exists() {
            backup_path(path)?;
        }
        if path.exists() {
            ensure!(path.is_file(), "not a regular file: {}", path.display());
        }
        for parent in path.ancestors().skip(1) {
            if parent.exists() {
                ensure!(parent.is_dir(), "not a directory: {}", parent.display());
            }
        }
    }
    for (path, content) in changes {
        write(&path, &content).with_context(|| {
            format!(
                "setup failed at {}; earlier changes may exist; inspect .skillvolution.bak backups",
                path.display()
            )
        })?;
    }
    Ok(())
}

fn read_optional(path: &Path) -> Result<Option<String>> {
    check_path(path)?;
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("read {}", path.display())),
    }
}

fn load_json(path: &Path) -> Result<Value> {
    let config: Value = match read_optional(path)? {
        Some(text) => parse_strict_object(&text).with_context(|| {
            format!(
                "{} must be a strict JSON object with unique keys (no comments, trailing commas, or duplicate keys)",
                path.display()
            )
        })?,
        None => json!({}),
    };
    ensure!(
        config.is_object(),
        "{} must contain a JSON object",
        path.display()
    );
    Ok(config)
}

fn parse_strict_object(text: &str) -> Result<Value> {
    struct StrictObjectVisitor;

    impl<'de> Visitor<'de> for StrictObjectVisitor {
        type Value = Value;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a JSON object with unique keys")
        }

        fn visit_map<M>(self, mut access: M) -> Result<Self::Value, M::Error>
        where
            M: MapAccess<'de>,
        {
            let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
            let mut entries: Vec<(String, Value)> = Vec::new();
            while let Some((key, value)) = access.next_entry::<String, Value>()? {
                if !seen.insert(key.clone()) {
                    return Err(<M::Error as serde::de::Error>::custom(format!(
                        "duplicate key {key:?}"
                    )));
                }
                entries.push((key, value));
            }
            let map: serde_json::Map<String, Value> = entries.into_iter().collect();
            Ok(Value::Object(map))
        }
    }

    let mut deserializer = serde_json::Deserializer::from_str(text);
    let value = deserializer.deserialize_map(StrictObjectVisitor)?;
    Ok(value)
}

fn check_skill(path: &Path) -> Result<bool> {
    if let Some(text) = read_optional(path)? {
        ensure!(
            text.contains(OWNER),
            "unowned Evolution skill conflict: {}",
            path.display()
        );
        Ok(true)
    } else {
        Ok(false)
    }
}

fn merge_server(config: &mut Value, key: &str, entry: Value, owned: bool) -> Result<()> {
    let servers = config
        .as_object_mut()
        .context("config must be an object")?
        .entry(key)
        .or_insert(json!({}));
    let servers = servers
        .as_object_mut()
        .with_context(|| format!("{key} must be an object"))?;
    if let Some(old) = servers.get("skillvolution") {
        ensure!(old.is_object(), "{key}.skillvolution must be an object");
        ensure!(old == &entry || owned, "unowned {key}.skillvolution conflict; review and remove the conflicting entry manually");
    }
    servers.insert("skillvolution".to_owned(), entry);
    Ok(())
}

fn merge_claude(mut text: String) -> Result<String> {
    let starts: Vec<_> = text.match_indices(START).map(|(i, _)| i).collect();
    let ends: Vec<_> = text.match_indices(END).map(|(i, _)| i).collect();
    match (starts.as_slice(), ends.as_slice()) {
        ([], []) => {
            if !text.is_empty() && !text.ends_with('\n') {
                text.push('\n');
            }
            text.push_str(CLAUDE);
            text.push('\n');
        }
        ([start], [end]) if start < end => {
            text.replace_range(*start..(*end + END.len()), CLAUDE);
        }
        _ => bail!(
            "CLAUDE.md has malformed or duplicate Skillvolution markers; repair them manually"
        ),
    }
    Ok(text)
}

fn write(path: &Path, content: &str) -> Result<()> {
    check_path(path)?;
    if path.exists() {
        let old = fs::read(path)?;
        if old == content.as_bytes() {
            return Ok(());
        }
        let backup = backup_path(path)?;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&backup)?;
        file.set_permissions(fs::metadata(path)?.permissions())?;
        file.write_all(&old)?;
        file.sync_all()?;
    }
    fs::create_dir_all(path.parent().context("missing parent")?)?;
    fs::write(path, content).with_context(|| format!("write {}", path.display()))
}

fn backup_path(path: &Path) -> Result<PathBuf> {
    for index in 0..10_000 {
        let suffix = if index == 0 {
            ".skillvolution.bak".to_string()
        } else {
            format!(".skillvolution.bak.{index}")
        };
        let mut name = path.as_os_str().to_owned();
        name.push(suffix);
        let backup = PathBuf::from(name);
        check_path(&backup)?;
        if !backup.exists() {
            return Ok(backup);
        }
        ensure!(
            backup.is_file(),
            "backup is not a regular file: {}",
            backup.display()
        );
    }
    bail!("too many backups for {}", path.display())
}

fn check_path(path: &Path) -> Result<()> {
    ensure!(!path.as_os_str().is_empty(), "path must not be empty");
    ensure!(
        !path
            .components()
            .any(|c| c == std::path::Component::ParentDir),
        "parent traversal (..) is unsupported: {}",
        path.display()
    );
    let absolute = std::path::absolute(path)?;
    for item in absolute.ancestors() {
        match fs::symlink_metadata(item) {
            Ok(meta) => {
                let linked = meta.file_type().is_symlink();
                #[cfg(windows)]
                let linked = {
                    use std::os::windows::fs::MetadataExt;
                    linked || meta.file_attributes() & 0x400 != 0
                };
                ensure!(
                    !linked,
                    "symlink or reparse point refused: {}",
                    item.display()
                );
                if item != absolute {
                    ensure!(meta.is_dir(), "not a directory: {}", item.display());
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).with_context(|| format!("inspect {}", item.display())),
        }
    }
    Ok(())
}
