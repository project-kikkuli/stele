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

fn default_samples() -> u32 {
    3
}

fn default_block_bar() -> f64 {
    0.9
}

fn default_nudge_bar() -> f64 {
    0.6
}

/// A rule whose verdict is a judgment, not a shell exit — "no slop comments",
/// "comments pass the sally-anne test". A deterministic check is exact and needs
/// no eval; a semantic rule's correctness lives in `prompt`, which is unfalsifiable
/// by inspection. So it carries its own eval: `stele eval` runs the prompt across
/// `models` against held-out corrections in `cases`, and the rule may only enforce
/// at the strength its weakest measurable model earns (`block_bar` / `nudge_bar`).
#[derive(Debug, Deserialize, Clone)]
pub struct Semantic {
    /// Judge prompt fanned to every model. Blunt, failure-named phrasings port
    /// across vendors better than elegant principles (see `stele eval`).
    pub prompt: String,
    /// Path to the before→after correction cases (JSONL), relative to repo root.
    pub cases: String,
    /// Judge names to run, resolved against the configured `[[judge]]` table.
    pub models: Vec<String>,
    /// Votes per (model, case); the majority verdict wins. Judges are nondeterministic.
    #[serde(default = "default_samples")]
    pub samples: u32,
    #[serde(default = "default_block_bar")]
    pub block_bar: f64,
    #[serde(default = "default_nudge_bar")]
    pub nudge_bar: f64,
}

/// A model that can act as a judge. `command` is a shell pipeline that reads the
/// prompt on stdin and writes the verdict to stdout — stele owns the policy, the
/// harness owns the invocation, so the fleet is never Claude-only.
#[derive(Debug, Deserialize, Clone)]
pub struct Judge {
    pub name: String,
    pub command: String,
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    #[default]
    Block,
    Nudge,
}

impl Severity {
    pub fn name(self) -> &'static str {
        match self {
            Self::Block => "block",
            Self::Nudge => "nudge",
        }
    }
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
    #[serde(default)]
    pub semantic: Option<Semantic>,
}

/// A prompt-time context provider: a command whose stdout is injected as agent
/// context at prompt/session-start time. Unlike a rule it has no pass/fail —
/// it never gates, never appears at stop or in CI. The command owns its own
/// relevance and noise control (e.g. filtering on `$STELE_CHANGED`, deduping via
/// a marker file), which is why stele injects its output verbatim.
#[derive(Debug, Deserialize, Clone)]
pub struct ContextProvider {
    pub id: String,
    pub command: String,
}

#[derive(Debug, Deserialize)]
struct File {
    #[serde(default, rename = "rule")]
    rules: Vec<Rule>,
    #[serde(default, rename = "context")]
    context: Vec<ContextProvider>,
    #[serde(default, rename = "judge")]
    judges: Vec<Judge>,
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

/// Prompt-time context providers across the active layers. Best-effort and
/// fail-open: a malformed or missing config yields no providers rather than an
/// error, because context injection must never break a session.
pub fn load_context(root: &Path, scope: LoadScope) -> Vec<ContextProvider> {
    let mut paths = Vec::new();
    if scope != LoadScope::Repository && !global_disabled() {
        paths.push(system_config_path());
        if let Ok(path) = user_config_path() {
            paths.push(path);
        }
    }
    if scope != LoadScope::Global {
        paths.push(repo_config_path(root));
    }
    let mut providers = Vec::new();
    for path in paths {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(file) = toml::from_str::<File>(&text) else {
            continue;
        };
        for provider in file.context {
            if !provider.id.is_empty() && !provider.command.is_empty() {
                providers.push(provider);
            }
        }
    }
    providers
}

/// Judges across the active layers, later layers overriding earlier ones by name
/// so a repository can retarget a personal judge. Fail-open like [`load_context`].
pub fn load_judges(root: &Path, scope: LoadScope) -> Vec<Judge> {
    let mut paths = Vec::new();
    if scope != LoadScope::Repository && !global_disabled() {
        paths.push(system_config_path());
        if let Ok(path) = user_config_path() {
            paths.push(path);
        }
    }
    if scope != LoadScope::Global {
        paths.push(repo_config_path(root));
    }
    let mut by_name: HashMap<String, Judge> = HashMap::new();
    let mut order = Vec::new();
    for path in paths {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(file) = toml::from_str::<File>(&text) else {
            continue;
        };
        for judge in file.judges {
            if judge.name.is_empty() || judge.command.is_empty() {
                continue;
            }
            if by_name.insert(judge.name.clone(), judge.clone()).is_none() {
                order.push(judge.name);
            }
        }
    }
    order.into_iter().filter_map(|n| by_name.remove(&n)).collect()
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
    if file.rules.is_empty() && file.context.is_empty() {
        return Err(format!(
            "{} defines no [[rule]] or [[context]] entries",
            path.display()
        ));
    }
    for provider in &file.context {
        if provider.id.is_empty() {
            return Err(format!("{}: every [[context]] needs an id", path.display()));
        }
        if provider.command.is_empty() {
            return Err(format!(
                "{}: context {}: `command` is required",
                path.display(),
                provider.id
            ));
        }
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
        let kinds = rule.check.is_some() as u8
            + rule.artifact.is_some() as u8
            + rule.semantic.is_some() as u8;
        if kinds != 1 {
            return Err(format!(
                "{}: rule {}: exactly one of `check`, `[rule.artifact]`, or `[rule.semantic]` is required",
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
