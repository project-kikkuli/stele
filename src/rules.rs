//! Rule evaluation: pure functions of (rule, substrate).

use crate::config::Rule;
use crate::substrate::Substrate;
use globset::Glob;
use std::process::Command;

#[derive(Debug)]
pub struct RuleResult {
    pub rule: Rule,
    /// Scope matched: the rule applies to this change-set.
    pub triggered: bool,
    pub findings: Vec<String>,
    /// Check couldn't run. NOT the same as green — CI fails loud on this,
    /// local channels stay silent but log it.
    pub error: Option<String>,
}

impl RuleResult {
    pub fn red(&self) -> bool {
        self.triggered && !self.findings.is_empty()
    }
}

fn glob_match(pattern: &str, path: &str) -> bool {
    Glob::new(pattern)
        .map(|g| g.compile_matcher().is_match(path))
        .unwrap_or(false)
}

fn in_scope(rule: &Rule, substrate: &Substrate) -> bool {
    if rule.scope.is_empty() {
        return !substrate.changed.is_empty();
    }
    let includes: Vec<&str> = rule
        .scope
        .iter()
        .filter(|p| !p.starts_with('!'))
        .map(String::as_str)
        .collect();
    let excludes: Vec<&str> = rule.scope.iter().filter_map(|p| p.strip_prefix('!')).collect();
    substrate.changed.iter().any(|path| {
        !excludes.iter().any(|pat| glob_match(pat, path))
            && includes.iter().any(|pat| glob_match(pat, path))
    })
}

fn check_artifact(rule: &Rule, substrate: &Substrate) -> Vec<String> {
    let art = rule.artifact.as_ref().expect("artifact rule");
    let target = substrate.root.join(&art.path);
    let Ok(text) = std::fs::read_to_string(&target) else {
        return vec![format!("✗ {} missing at repo root", art.path)];
    };
    let lines: Vec<&str> = text.lines().collect();
    let mut findings = Vec::new();
    for section in &art.sections {
        // Exact heading match, trimmed: `## Functional` must not be satisfied
        // by `## Functionality`.
        let Some(pos) = lines.iter().position(|l| l.trim() == section.trim()) else {
            findings.push(format!(
                "✗ {}: missing required section {:?}",
                art.path, section
            ));
            continue;
        };
        if art.nonempty_sections {
            let body_empty = lines[pos + 1..]
                .iter()
                .take_while(|l| !l.trim_start().starts_with('#'))
                .all(|l| l.trim().is_empty());
            if body_empty {
                findings.push(format!(
                    "✗ {}: section {:?} is empty — write actual content",
                    art.path, section
                ));
            }
        }
    }
    findings
}

fn check_command(rule: &Rule, substrate: &Substrate) -> RuleResult {
    let cmd = rule.check.as_ref().expect("command rule");
    let out = Command::new("bash")
        .arg("-c")
        .arg(cmd)
        .current_dir(&substrate.root)
        .env("STELE_ROOT", &substrate.root)
        .env("STELE_BASE", substrate.base.as_deref().unwrap_or(""))
        .env("STELE_CHANGED", substrate.changed.join("\n"))
        .output();
    let out = match out {
        Ok(o) => o,
        Err(e) => {
            return RuleResult {
                rule: rule.clone(),
                triggered: true,
                findings: vec![],
                error: Some(format!("check failed to run: {e}")),
            }
        }
    };
    if out.status.success() {
        return RuleResult {
            rule: rule.clone(),
            triggered: true,
            findings: vec![],
            error: None,
        };
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut findings: Vec<String> = stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .take(20)
        .map(str::to_string)
        .collect();
    if findings.is_empty() {
        findings.push(format!(
            "✗ check `{cmd}` exited {}",
            out.status.code().unwrap_or(-1)
        ));
    }
    RuleResult {
        rule: rule.clone(),
        triggered: true,
        findings,
        error: None,
    }
}

/// Artifact findings regardless of scope triggering — for gatekeeper channels
/// (hermes pre_tool_call), which must gate the call that would CREATE the red:
/// scope-triggered evaluation is one step behind there, because the change-set
/// is still empty when the first mutating call is decided.
pub fn artifact_findings_unconditional(rule: &Rule, substrate: &Substrate) -> Vec<String> {
    if rule.artifact.is_none() {
        return vec![];
    }
    check_artifact(rule, substrate)
}

pub fn evaluate(rule: &Rule, substrate: &Substrate) -> RuleResult {
    if !in_scope(rule, substrate) {
        return RuleResult {
            rule: rule.clone(),
            triggered: false,
            findings: vec![],
            error: None,
        };
    }
    if rule.artifact.is_some() {
        RuleResult {
            rule: rule.clone(),
            triggered: true,
            findings: check_artifact(rule, substrate),
            error: None,
        }
    } else {
        check_command(rule, substrate)
    }
}
