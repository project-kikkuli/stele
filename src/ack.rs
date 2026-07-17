//! Git-native acknowledgements: an intentional red must be able to merge.
//!
//! `stele ack <rule-id> -m "reason"` records an empty commit carrying a
//! `Stele-Ack: <rule-id>` trailer. Every layer (hooks, git hooks, CI) scans
//! commit messages in base..HEAD for trailers, so an ack is visible to
//! reviewers in history, travels with the branch, and expires naturally when
//! the branch merges (the next branch starts from a new base).

use crate::substrate::{self, Substrate};
use std::collections::HashSet;
use std::process::Command;

pub const TRAILER: &str = "Stele-Ack:";

/// Rule ids acknowledged in commits between base and HEAD.
pub fn acked_rules(sub: &Substrate) -> HashSet<String> {
    let messages = substrate::commit_messages_since_base(sub);
    parse(&messages)
}

pub fn parse(messages: &str) -> HashSet<String> {
    messages
        .lines()
        .filter_map(|line| line.trim().strip_prefix(TRAILER))
        .map(|rest| rest.split_whitespace().next().unwrap_or("").to_string())
        .filter(|id| !id.is_empty())
        .collect()
}

/// Record an acknowledgement as an empty commit with the trailer.
pub fn create(sub: &Substrate, rule_id: &str, reason: &str) -> Result<(), String> {
    let message = format!("stele: acknowledge `{rule_id}`\n\n{reason}\n\n{TRAILER} {rule_id}\n");
    let out = Command::new("git")
        .arg("-C")
        .arg(&sub.root)
        // `--only` with no paths keeps staged user changes out of the
        // acknowledgement commit; `--allow-empty` makes that legal.
        .args(["commit", "--allow-empty", "--only", "-m", &message])
        .output()
        .map_err(|e| format!("git commit: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git commit: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(())
}
