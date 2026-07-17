//! Verdict aggregation, noise gates, and telemetry.
//!
//! Noise economics are engine concerns, not per-rule concerns: silence on
//! green, speak once per change-signature (stop AND prompt channels), give up
//! (fail-open) after MAX_BLOCKS so a non-complying agent terminates rather
//! than loops forever. State lives under `<git-dir>/stele/` so it never
//! dirties the worktree.
//!
//! Severity semantics, honestly stated:
//! - `block`: loops the agent at stop-time, gates tools, fails CI.
//! - `nudge`: appears in prompt-time context (once per signature), in
//!   `stele check` output as a warning, and in CI logs — but NEVER emits a
//!   stop-channel block (on Claude/Codex the stop channel can only block, so
//!   anything else would be a block in disguise) and never fails CI.
//!
//! Acknowledged rules (`Stele-Ack:` trailer in base..HEAD) are excluded from
//! blocking everywhere and reported as acknowledged.

use crate::ack;
use crate::config::{Rule, Severity};
use crate::rules::{self, RuleResult};
use crate::substrate::Substrate;
use serde_json::json;
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

pub const DEFAULT_MAX_BLOCKS: u32 = 2;

#[derive(Debug)]
pub struct Verdict {
    pub results: Vec<RuleResult>,
    pub acked: HashSet<String>,
}

impl Verdict {
    /// All failing rules, acked or not.
    pub fn red(&self) -> Vec<&RuleResult> {
        self.results.iter().filter(|r| r.red()).collect()
    }
    /// Failing block-severity rules that are NOT acknowledged: these gate.
    pub fn blocking(&self) -> Vec<&RuleResult> {
        self.red()
            .into_iter()
            .filter(|r| r.rule.severity == Severity::Block && !self.acked.contains(&r.rule.id))
            .collect()
    }
    /// Failing rules that are acknowledged (reported, never gate).
    pub fn acknowledged(&self) -> Vec<&RuleResult> {
        self.red()
            .into_iter()
            .filter(|r| self.acked.contains(&r.rule.id))
            .collect()
    }
    /// Failing nudge-severity rules (reported, never gate).
    pub fn nudges(&self) -> Vec<&RuleResult> {
        self.red()
            .into_iter()
            .filter(|r| r.rule.severity == Severity::Nudge && !self.acked.contains(&r.rule.id))
            .collect()
    }
    pub fn errors(&self) -> Vec<&RuleResult> {
        self.results.iter().filter(|r| r.error.is_some()).collect()
    }
    pub fn is_green(&self) -> bool {
        self.red().is_empty() && self.errors().is_empty()
    }
}

pub fn check(rule_set: &[Rule], substrate: &Substrate) -> Verdict {
    Verdict {
        results: rule_set
            .iter()
            .map(|r| rules::evaluate(r, substrate))
            .collect(),
        acked: ack::acked_rules(substrate),
    }
}

/// Per-repo state under `<git-dir>/stele/`.
pub struct State {
    dir: PathBuf,
}

impl State {
    pub fn new(substrate: &Substrate) -> Self {
        Self::at(substrate.git_dir.clone())
    }

    /// For light-weight logging before a full substrate exists.
    pub fn at(git_dir: PathBuf) -> Self {
        let dir = git_dir.join("stele");
        let _ = fs::create_dir_all(&dir);
        State { dir }
    }

    fn read(&self, name: &str) -> String {
        fs::read_to_string(self.dir.join(name)).unwrap_or_default()
    }

    fn write(&self, name: &str, value: &str) {
        let _ = fs::write(self.dir.join(name), value);
    }

    /// A change-set already measured green stays silent for free until it moves.
    pub fn is_green_cached(&self, signature: &str) -> bool {
        self.read("lastgreen").trim() == signature
    }

    pub fn mark_green(&self, signature: &str) {
        self.write("lastgreen", signature);
    }

    /// Blocks issued for this signature so far; increments on use.
    pub fn take_block_slot(&self, signature: &str, max_blocks: u32) -> bool {
        self.take_slot("blocks", signature, max_blocks)
    }

    /// Prompt-context speaks once per signature too.
    pub fn take_prompt_slot(&self, signature: &str) -> bool {
        self.take_slot("prompted", signature, 1)
    }

    fn take_slot(&self, file: &str, signature: &str, max: u32) -> bool {
        let current = self.read(file);
        let (sig, count) = current
            .split_once(' ')
            .map(|(s, c)| (s.to_string(), c.trim().parse::<u32>().unwrap_or(0)))
            .unwrap_or((String::new(), 0));
        let count = if sig == signature { count } else { 0 };
        if count >= max {
            return false;
        }
        self.write(file, &format!("{signature} {}", count + 1));
        true
    }

    /// Telemetry: every hook invocation appends one JSONL record — including
    /// loop-guard exits, so retries never vanish from the count. This is the
    /// data that later answers "which layer catches what, per harness".
    pub fn log_event(&self, harness: &str, event: &str, verdict: &str, detail: &str) {
        let record = json!({
            "ts": iso_now(),
            "harness": harness,
            "event": event,
            "verdict": verdict,
            "detail": detail,
        });
        let path = self.dir.join("events.jsonl");
        if let Ok(mut existing) = fs::OpenOptions::new().create(true).append(true).open(path) {
            use std::io::Write;
            let _ = writeln!(existing, "{record}");
        }
    }
}

/// ISO-8601 UTC without a chrono dependency (civil-from-days, Howard Hinnant's
/// algorithm).
pub fn iso_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0) as i64;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mo <= 2 { y + 1 } else { y };
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

/// The agent-facing message: findings plus exact remediation, in the "failing
/// check output" register agents are trained to fix. Acked rules are omitted.
pub fn render_reason(verdict: &Verdict) -> String {
    render(verdict, &verdict.blocking(), "violates repository rules")
}

/// Nudge-only variant for the prompt channel.
pub fn render_nudges(verdict: &Verdict) -> String {
    render(verdict, &verdict.nudges(), "has advisory findings")
}

fn render(verdict: &Verdict, results: &[&RuleResult], verb: &str) -> String {
    let mut out = format!("stele: this change-set {verb}.\n");
    for result in results {
        out.push_str(&format!("\nrule `{}`", result.rule.id));
        if !result.rule.description.is_empty() {
            out.push_str(&format!(" — {}", result.rule.description));
        }
        out.push('\n');
        for f in &result.findings {
            out.push_str(f);
            out.push('\n');
        }
        if let Some(art) = &result.rule.artifact {
            out.push_str(&format!(
                "Fix: create or update `{}` so it contains these sections: {}.\n",
                art.path,
                art.sections
                    .iter()
                    .map(|s| format!("`{s}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if !result.rule.message.is_empty() {
            out.push_str(&result.rule.message);
            out.push('\n');
        }
    }
    let acked = verdict.acknowledged();
    if !acked.is_empty() {
        out.push_str(&format!(
            "\n(acknowledged, not gating: {})\n",
            acked
                .iter()
                .map(|r| format!("`{}`", r.rule.id))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    out.push_str(
        "\nFix the findings above before finishing (or, if intentional, run \
         `stele ack <rule-id> -m \"why\"`). Run `stele check` to re-verify.",
    );
    out
}
