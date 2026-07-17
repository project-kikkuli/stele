//! Layered `stele.toml` loading.
//!
//! Rules accumulate from three scopes:
//! - system: `/etc/stele/stele.toml` (or `STELE_SYSTEM_CONFIG`)
//! - user: `$XDG_CONFIG_HOME/stele/stele.toml`, falling back to
//!   `~/.config/stele/stele.toml` (or `STELE_USER_CONFIG`)
//! - repository: `<git-root>/stele.toml`
//!
//! ```toml
//! [[rule]]
//! id = "requirements-doc"
//! description = "why this rule exists (shown to agents)"
//! severity = "block"            # block | nudge (nudge never loops the agent)
//! trigger = "changes"           # changes | always
//! scope = ["**/*.py"]           # change-set globs; valid with trigger=changes
//! acknowledge = true             # allow `stele ack`; defaults to true
//! message = "extra remediation guidance"
//!
//! # exactly one of:
//! [rule.artifact]                # built-in artifact-shape check
//! path = "requirements.md"
//! sections = ["# Requirements", "## Functional", "## Risks"]
//! # or:
//! # check = "scripts/check_foo.sh"  # exit 0 green, nonzero red on stdout
//! ```

use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

pub const CONFIG_NAME: &str = "stele.toml";
pub const USER_CONFIG_ENV: &str = "STELE_USER_CONFIG";
pub const SYSTEM_CONFIG_ENV: &str = "STELE_SYSTEM_CONFIG";
pub const DISABLE_GLOBAL_ENV: &str = "STELE_DISABLE_GLOBAL";

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

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Trigger {
    /// Evaluate only when the current change-set is nonempty and in scope.
    #[default]
    Changes,
    /// Evaluate even on a clean tree. Intended for session preconditions such
    /// as "the agent must be running in a linked worktree".
    Always,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize, Clone)]
pub struct Rule {
    pub id: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub severity: Severity,
    #[serde(default)]
    pub trigger: Trigger,
    #[serde(default)]
    pub scope: Vec<String>,
    #[serde(default = "default_true")]
    pub acknowledge: bool,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayerKind {
    System,
    User,
    Repository,
}

impl LayerKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::User => "user",
            Self::Repository => "repository",
        }
    }
}

#[derive(Debug)]
pub struct Layer {
    pub kind: LayerKind,
    pub path: PathBuf,
    pub rules: Vec<Rule>,
}

/// Which layers a hook invocation should evaluate. User-level hooks evaluate
/// global rules; generated repository hooks evaluate repository rules. This
/// prevents the same rule from firing twice when both hook layers are active.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LoadScope {
    #[default]
    All,
    Global,
    Repository,
}

impl LoadScope {
    pub fn name(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Global => "global",
            Self::Repository => "repo",
        }
    }
}

pub fn repo_config_path(root: &Path) -> PathBuf {
    root.join(CONFIG_NAME)
}

pub fn user_config_path() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os(USER_CONFIG_ENV) {
        return Ok(PathBuf::from(path));
    }
    if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(dir).join("stele").join(CONFIG_NAME));
    }
    let home = std::env::var_os("HOME").ok_or("no $HOME; set STELE_USER_CONFIG")?;
    Ok(PathBuf::from(home)
        .join(".config")
        .join("stele")
        .join(CONFIG_NAME))
}

pub fn system_config_path() -> PathBuf {
    if let Some(path) = std::env::var_os(SYSTEM_CONFIG_ENV) {
        return PathBuf::from(path);
    }
    #[cfg(windows)]
    {
        if let Some(dir) = std::env::var_os("PROGRAMDATA") {
            return PathBuf::from(dir).join("stele").join(CONFIG_NAME);
        }
    }
    PathBuf::from("/etc/stele").join(CONFIG_NAME)
}

fn global_disabled() -> bool {
    std::env::var(DISABLE_GLOBAL_ENV)
        .map(|v| !matches!(v.as_str(), "" | "0" | "false" | "no"))
        .unwrap_or(false)
}

/// Return every active configuration layer for diagnostics and callers that
/// need provenance. Missing optional layers are skipped.
pub fn layers(root: &Path, scope: LoadScope) -> Result<Vec<Layer>, String> {
    let mut candidates = Vec::new();
    if scope != LoadScope::Repository && !global_disabled() {
        candidates.push((LayerKind::System, system_config_path()));
        if let Ok(path) = user_config_path() {
            candidates.push((LayerKind::User, path));
        }
    }
    if scope != LoadScope::Global {
        candidates.push((LayerKind::Repository, repo_config_path(root)));
    }

    let mut loaded = Vec::new();
    for (kind, path) in candidates {
        if path.is_file() {
            loaded.push(Layer {
                kind,
                rules: load_file(&path)?,
                path,
            });
        }
    }
    if loaded.is_empty() {
        let expected = match scope {
            LoadScope::Repository => repo_config_path(root).display().to_string(),
            LoadScope::Global => "the system or user Stele config".to_string(),
            LoadScope::All => "a system, user, or repository Stele config".to_string(),
        };
        return Err(format!("no rules found in {expected}"));
    }
    Ok(loaded)
}

pub fn load(root: &Path) -> Result<Vec<Rule>, String> {
    load_scope(root, LoadScope::All)
}

pub fn load_repo(root: &Path) -> Result<Vec<Rule>, String> {
    load_scope(root, LoadScope::Repository)
}

pub fn load_global(root: &Path) -> Result<Vec<Rule>, String> {
    load_scope(root, LoadScope::Global)
}

pub fn load_scope(root: &Path, scope: LoadScope) -> Result<Vec<Rule>, String> {
    merge_layers(layers(root, scope)?)
}

fn merge_layers(layers: Vec<Layer>) -> Result<Vec<Rule>, String> {
    let mut seen: HashMap<String, PathBuf> = HashMap::new();
    let mut rules = Vec::new();
    for layer in layers {
        for rule in layer.rules {
            if let Some(first) = seen.insert(rule.id.clone(), layer.path.clone()) {
                return Err(format!(
                    "duplicate rule id {:?} across {} and {}",
                    rule.id,
                    first.display(),
                    layer.path.display()
                ));
            }
            rules.push(rule);
        }
    }
    Ok(rules)
}

fn load_file(path: &Path) -> Result<Vec<Rule>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let file: File = toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    if file.rules.is_empty() {
        return Err(format!("{} defines no [[rule]] entries", path.display()));
    }
    let mut seen: HashSet<&str> = HashSet::new();
    for rule in &file.rules {
        if rule.id.is_empty() {
            return Err(format!("{}: every [[rule]] needs an id", path.display()));
        }
        if !seen.insert(&rule.id) {
            return Err(format!(
                "{}: duplicate rule id {:?}",
                path.display(),
                rule.id
            ));
        }
        if rule.check.is_some() == rule.artifact.is_some() {
            return Err(format!(
                "{}: rule {}: exactly one of `check` or `[rule.artifact]` is required",
                path.display(),
                rule.id
            ));
        }
        if rule.trigger == Trigger::Always && !rule.scope.is_empty() {
            return Err(format!(
                "{}: rule {}: `trigger = \"always\"` cannot be combined with `scope`",
                path.display(),
                rule.id
            ));
        }
    }
    Ok(file.rules)
}
