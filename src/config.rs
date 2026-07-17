//! stele.toml loading.
//!
//! ```toml
//! [[rule]]
//! id = "requirements-doc"
//! description = "why this rule exists (shown to agents)"
//! severity = "block"            # block | nudge (nudge never loops the agent)
//! scope = ["**/*.py"]           # change-set globs that trigger the rule; omit = any change
//! message = "extra remediation guidance"
//!
//! # exactly one of:
//! [rule.artifact]               # built-in artifact-shape check
//! path = "requirements.md"
//! sections = ["# Requirements", "## Functional", "## Risks"]
//! # or:
//! # check = "scripts/check_foo.sh"  # exit 0 green, nonzero red with findings on stdout
//! ```

use serde::Deserialize;
use std::collections::HashSet;
use std::path::Path;

pub const CONFIG_NAME: &str = "stele.toml";

#[derive(Debug, Deserialize, Clone)]
pub struct Artifact {
    pub path: String,
    #[serde(default)]
    pub sections: Vec<String>,
    /// Require non-whitespace content under every section heading.
    #[serde(default)]
    pub nonempty_sections: bool,
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    #[default]
    Block,
    Nudge,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Rule {
    pub id: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub severity: Severity,
    #[serde(default)]
    pub scope: Vec<String>,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub check: Option<String>,
    #[serde(default)]
    pub artifact: Option<Artifact>,
}

#[derive(Debug, Deserialize)]
struct File {
    #[serde(default, rename = "rule")]
    rules: Vec<Rule>,
}

pub fn load(root: &Path) -> Result<Vec<Rule>, String> {
    let path = root.join(CONFIG_NAME);
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("no {} at {}: {e}", CONFIG_NAME, root.display()))?;
    let file: File = toml::from_str(&text).map_err(|e| format!("{CONFIG_NAME}: {e}"))?;
    if file.rules.is_empty() {
        return Err(format!("{CONFIG_NAME} defines no [[rule]] entries"));
    }
    let mut seen: HashSet<&str> = HashSet::new();
    for rule in &file.rules {
        if rule.id.is_empty() {
            return Err("every [[rule]] needs an id".into());
        }
        if !seen.insert(&rule.id) {
            return Err(format!("duplicate rule id {:?}", rule.id));
        }
        if rule.check.is_some() == rule.artifact.is_some() {
            return Err(format!(
                "rule {}: exactly one of `check` or `[rule.artifact]` is required",
                rule.id
            ));
        }
    }
    Ok(file.rules)
}
