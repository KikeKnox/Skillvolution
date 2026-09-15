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

/// Substitutes the bin/db placeholders in `template` with `bin`/`db` encoded as JSON
/// string literals, so the plugin source can assign them straight to a JS const
/// (e.g. `const BIN = __SKILLVOLUTION_BIN__;` becomes `const BIN = "/path/to/bin";`).
pub fn render(template: &str, bin: &Path, db: &Path) -> Result<String> {
    let bin = serde_json::to_string(&bin.display().to_string()).context("encode --bin path")?;
    let db = serde_json::to_string(&db.display().to_string()).context("encode --db path")?;
    Ok(template
        .replace(BIN_PLACEHOLDER, &bin)
        .replace(DB_PLACEHOLDER, &db))
}

/// The plugin file content for this `bin`/`db` pair.
pub fn content(bin: &Path, db: &Path) -> Result<String> {
    render(TEMPLATE, bin, db)
}

/// Refuses an existing plugin file that doesn't carry our managed marker, so we never
/// clobber a user's own file at that path.
pub fn check_owner(path: &Path) -> Result<()> {
    fs_safe::check_owner(path, OWNER_MARKER, "Skillvolution OpenCode plugin")
}

#[cfg(test)]
mod tests {
    use super::render;
    use std::path::Path;

    #[test]
    fn substitutes_placeholders_as_valid_json_string_literals() {
        let template = "const BIN = __SKILLVOLUTION_BIN__;\nconst DB = __SKILLVOLUTION_DB__;\n";
        let bin = Path::new("/opt/bin with spaces/skillvolution");
        let db = Path::new("/home/user/data \"quoted\"/skills.db");
        let rendered = render(template, bin, db).unwrap();

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
}
