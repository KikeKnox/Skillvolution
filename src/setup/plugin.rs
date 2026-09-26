//! OpenCode plugin asset (`assets/opencode/skillvolution.js`): `--bin`/`--db`
//! substitution, shared by project and global OpenCode setup.
//!
//! The asset embeds `skillvolution-managed:opencode-plugin` in a comment so reruns
//! recognize their own prior output, exactly like the Evolution skill's marker.

use super::fs_safe;
use anyhow::{Context, Result};
use std::path::Path;

const TEMPLATE: &str = include_str!("../../assets/opencode/skillvolution.js");
const OWNER_MARKER: &str = "skillvolution-managed:opencode-plugin";
const BIN_PLACEHOLDER: &str = "__SKILLVOLUTION_BIN__";
const DB_PLACEHOLDER: &str = "__SKILLVOLUTION_DB__";
const PROJECT_KEY_PLACEHOLDER: &str = "__SKILLVOLUTION_PROJECT_KEY__";

/// Substitutes the bin/db/project-key placeholders in `template`. `bin`/`db` are encoded
/// as JSON string literals so the plugin source can assign them straight to a JS const
/// (e.g. `const BIN = __SKILLVOLUTION_BIN__;` becomes `const BIN = "/path/to/bin";`).
/// `key` becomes a JSON string literal too when `Some` (project-mode setup), or the bare
/// `null` literal when `None` (global setup, where the plugin detects the project at
/// runtime instead).
pub fn render(template: &str, bin: &Path, db: &Path, key: Option<&str>) -> Result<String> {
    let bin = serde_json::to_string(&bin.display().to_string()).context("encode --bin path")?;
    let db = serde_json::to_string(&db.display().to_string()).context("encode --db path")?;
    let key = match key {
        Some(key) => serde_json::to_string(key).context("encode --project-key")?,
        None => "null".to_owned(),
    };
    Ok(template
        .replace(BIN_PLACEHOLDER, &bin)
        .replace(DB_PLACEHOLDER, &db)
        .replace(PROJECT_KEY_PLACEHOLDER, &key))
}

/// The plugin file content for this `bin`/`db` pair and, for a project-mode setup, the
/// project key the MCP server was started with (`None` for a global setup).
pub fn content(bin: &Path, db: &Path, key: Option<&str>) -> Result<String> {
    render(TEMPLATE, bin, db, key)
}

/// Refuses an existing plugin file that doesn't carry our managed marker, so we never
/// clobber a user's own file at that path.
pub fn check_owner(path: &Path) -> Result<()> {
    fs_safe::check_owner(path, OWNER_MARKER, "Skillvolution OpenCode plugin")
}

#[cfg(test)]
mod tests {
    use super::{TEMPLATE, content, render};
    use std::{path::Path, process::Command};

    #[test]
    fn substitutes_placeholders_as_valid_json_string_literals() {
        let template = "const BIN = __SKILLVOLUTION_BIN__;\nconst DB = __SKILLVOLUTION_DB__;\n";
        let bin = Path::new("/opt/bin with spaces/skillvolution");
        let db = Path::new("/home/user/data \"quoted\"/skills.db");
        let rendered = render(template, bin, db, None).unwrap();

        assert!(!rendered.contains("__SKILLVOLUTION_"));
        // Each substituted line must itself be valid JSON when parsed as a value, and
        // must round-trip back to the original path.
        let bin_line = rendered.lines().next().unwrap();
        let bin_literal = bin_line
            .trim_start_matches("const BIN = ")
            .trim_end_matches(';');
        let decoded: String = serde_json::from_str(bin_literal).unwrap();
        assert_eq!(decoded, bin.display().to_string());

        let db_line = rendered.lines().nth(1).unwrap();
        let db_literal = db_line
            .trim_start_matches("const DB = ")
            .trim_end_matches(';');
        let decoded: String = serde_json::from_str(db_literal).unwrap();
        assert_eq!(decoded, db.display().to_string());
    }

    #[test]
    fn project_key_placeholder_becomes_a_json_string_when_present() {
        let template = "const PROJECT_KEY = __SKILLVOLUTION_PROJECT_KEY__;\n";
        let rendered = render(
            template,
            Path::new("/bin"),
            Path::new("/db"),
            Some("my-key"),
        )
        .unwrap();
        assert_eq!(rendered, "const PROJECT_KEY = \"my-key\";\n");
    }

    #[test]
    fn project_key_placeholder_becomes_null_when_absent() {
        let template = "const PROJECT_KEY = __SKILLVOLUTION_PROJECT_KEY__;\n";
        let rendered = render(template, Path::new("/bin"), Path::new("/db"), None).unwrap();
        assert_eq!(rendered, "const PROJECT_KEY = null;\n");
    }

    /// Runs `node --check` on `source`, skipping (rather than failing) when `node` isn't
    /// on PATH, so this test stays meaningful in CI without making `node` a hard
    /// dependency for a plain `cargo test`.
    fn assert_valid_js(source: &str) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("plugin.mjs");
        std::fs::write(&path, source).unwrap();
        let output = match Command::new("node").arg("--check").arg(&path).output() {
            Ok(output) => output,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                eprintln!("skipping node --check: node not found on PATH");
                return;
            }
            Err(error) => panic!("run node --check: {error}"),
        };
        assert!(
            output.status.success(),
            "node --check failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn raw_template_is_valid_js_before_substitution() {
        // Placeholders are bare identifiers, which `node --check` accepts: it only
        // parses syntax, it doesn't resolve them.
        assert_valid_js(TEMPLATE);
    }

    #[test]
    fn rendered_template_is_valid_js_with_and_without_project_key() {
        let bin = Path::new("/opt/bin with spaces/skillvolution");
        let db = Path::new("/home/user/data \"quoted\"/skills.db");

        assert_valid_js(&content(bin, db, Some("my-project")).unwrap());
        assert_valid_js(&content(bin, db, None).unwrap());
    }
}
