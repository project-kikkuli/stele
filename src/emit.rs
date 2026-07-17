//! Per-harness response protocols — the only place harness dialects live.
//!
//! Empirical basis (conformance/RESULTS.md):
//! - claude-code, codex, devin-cli share one contract: `{"decision":"block",
//!   "reason":...}` on stop; plain stdout = injected context on prompt-submit.
//! - cursor (IDE): `{"followup_message":...}` on stop — auto-submitted as the
//!   next user turn, loop-capped by the harness.
//! - hermes: `{"action":"block","message":...}` on pre_tool_call; `{}` = allow.

use serde_json::json;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Harness {
    ClaudeCode,
    Codex,
    DevinCli,
    Cursor,
    Hermes,
}

impl Harness {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "claude-code" | "claude" => Some(Self::ClaudeCode),
            "codex" => Some(Self::Codex),
            "devin" | "devin-cli" => Some(Self::DevinCli),
            "cursor" => Some(Self::Cursor),
            "hermes" => Some(Self::Hermes),
            _ => None,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude-code",
            Self::Codex => "codex",
            Self::DevinCli => "devin-cli",
            Self::Cursor => "cursor",
            Self::Hermes => "hermes",
        }
    }
}

/// The stop-time block, in the finishing harness's native protocol.
/// Hermes deliberately has no arm here: v0.14.0 has no stop channel that
/// consumes a response (post_llm_call is fire-and-forget — verified in
/// run_agent.py); its enforcement lives in the pre_tool_call gate. Emitting
/// nothing is honest; emitting a pretend-block would be silent no-op wiring.
pub fn stop_block(harness: Harness, reason: &str) -> String {
    match harness {
        Harness::ClaudeCode | Harness::Codex | Harness::DevinCli => {
            json!({"decision": "block", "reason": reason}).to_string()
        }
        Harness::Cursor => json!({"followup_message": reason}).to_string(),
        Harness::Hermes => String::new(),
    }
}

/// Tool-gate responses (hermes pre_tool_call; claude/codex/devin PreToolUse).
pub fn tool_block(harness: Harness, reason: &str) -> String {
    match harness {
        Harness::Hermes => json!({"action": "block", "message": reason}).to_string(),
        _ => json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": reason,
            }
        })
        .to_string(),
    }
}

pub fn tool_allow(harness: Harness) -> Option<String> {
    match harness {
        Harness::Hermes => Some("{}".to_string()),
        _ => None, // silence = allow
    }
}

/// Prompt-time context injection: claude/codex/devin consume raw stdout.
pub fn prompt_context(reason: &str) -> String {
    reason.to_string()
}
