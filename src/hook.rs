//! `stele hook <harness> <event>` — the single entrypoint every harness's
//! wiring invokes, with the event payload on stdin.
//!
//! Fail-open discipline: a hook must never break the user's session. Any
//! internal failure exits 0 silently — but is logged to state, and CI runs
//! the same check fail-loud, so "hook crashed" can never masquerade as
//! compliance forever.

use crate::config;
use crate::emit::{self, Harness};
use crate::engine::{self, State, DEFAULT_MAX_BLOCKS};
use crate::substrate;
use serde_json::Value;
use std::io::Read;
use std::path::PathBuf;

pub fn run(harness_name: &str, event: &str) -> i32 {
    // Everything inside is fail-open; the wrapper only translates errors.
    run_inner(harness_name, event).unwrap_or_default()
}

fn run_inner(harness_name: &str, event: &str) -> Result<i32, String> {
    let Some(harness) = Harness::parse(harness_name) else {
        return Ok(0);
    };

    let mut payload_text = String::new();
    let _ = std::io::stdin().read_to_string(&mut payload_text);
    let payload: Value = serde_json::from_str(&payload_text).unwrap_or(Value::Null);
    let root = resolve_root(&payload);

    // Loop guards, per harness (empirical: claude/codex/devin send
    // stop_hook_active on the retry; cursor sends loop_count). Logged so
    // retries never vanish from telemetry.
    if event_is_stop(event) {
        let guarded = payload["stop_hook_active"].as_bool() == Some(true)
            || payload["loop_count"].as_i64().unwrap_or(0) > 0;
        if guarded {
            log_light(&root, harness.name(), event, "loop-guard");
            return Ok(0);
        }
    }

    let Ok(sub) = substrate::compute(&root) else {
        return Ok(0);
    };
    let Ok(rule_set) = config::load(&sub.root) else {
        return Ok(0); // no stele.toml here: not a stele repo, stay silent
    };
    let state = State::new(&sub);

    // Green verdict cache: a measured-green change-set stays silent for free.
    if event_is_stop(event) && state.is_green_cached(&sub.signature) {
        state.log_event(harness.name(), event, "green-cached", "");
        return Ok(0);
    }

    let verdict = engine::check(&rule_set, &sub);
    for errored in verdict.errors() {
        state.log_event(
            harness.name(),
            event,
            "unmeasurable",
            errored.error.as_deref().unwrap_or(""),
        );
    }

    match event {
        e if event_is_stop(e) => handle_stop(harness, event, &state, &sub.signature, &verdict),
        "prompt" | "user-prompt-submit" => {
            handle_prompt(harness, event, &state, &sub.signature, &verdict)
        }
        "pre-tool-use" | "pre_tool_call" => {
            handle_toolgate(harness, event, &state, &verdict, &payload_text, &payload)
        }
        _ => Ok(0),
    }
}

fn event_is_stop(event: &str) -> bool {
    matches!(event, "stop" | "Stop" | "session-end" | "SessionEnd")
}

fn resolve_root(payload: &Value) -> PathBuf {
    if let Ok(dir) = std::env::var("CLAUDE_PROJECT_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    if let Some(cwd) = payload["cwd"].as_str() {
        return PathBuf::from(cwd);
    }
    if let Some(root) = payload["workspace_roots"][0].as_str() {
        return PathBuf::from(root);
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// Telemetry for paths that exit before a full substrate exists.
fn log_light(root: &PathBuf, harness: &str, event: &str, verdict: &str) {
    if let Ok(git_dir) = substrate::find_root(root)
        .and_then(|r| substrate::find_git_dir(&r))
    {
        State::at(git_dir).log_event(harness, event, verdict, "");
    }
}

fn handle_stop(
    harness: Harness,
    event: &str,
    state: &State,
    signature: &str,
    verdict: &engine::Verdict,
) -> Result<i32, String> {
    let blocking = verdict.blocking();
    if blocking.is_empty() {
        // Nudges and acked reds never block the stop channel — on Claude and
        // Codex the stop channel can ONLY block, so anything else would be a
        // block in disguise. They live in prompt context and `stele check`.
        if verdict.is_green() {
            state.mark_green(signature);
        }
        state.log_event(harness.name(), event, "green", "");
        return Ok(0);
    }
    let max_blocks = std::env::var("STELE_MAX_BLOCKS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_MAX_BLOCKS);
    if !state.take_block_slot(signature, max_blocks) {
        // Spoke enough about this exact change-set: give up (fail-open),
        // the environment tier and CI still stand behind us.
        state.log_event(harness.name(), event, "gave-up", "");
        return Ok(0);
    }
    let block = emit::stop_block(harness, &engine::render_reason(verdict));
    if block.is_empty() {
        // No stop channel on this harness (hermes): the tool gate carries it.
        state.log_event(harness.name(), event, "no-stop-channel", "");
        return Ok(0);
    }
    state.log_event(harness.name(), event, "blocked", "");
    println!("{block}");
    Ok(0)
}

fn handle_prompt(
    harness: Harness,
    event: &str,
    state: &State,
    signature: &str,
    verdict: &engine::Verdict,
) -> Result<i32, String> {
    let blocking = verdict.blocking();
    let nudges = verdict.nudges();
    if blocking.is_empty() && nudges.is_empty() {
        return Ok(0);
    }
    // Speak once per signature here too: repeated context is noise the agent
    // learns to ignore.
    if !state.take_prompt_slot(signature) {
        return Ok(0);
    }
    state.log_event(harness.name(), event, "context-injected", "");
    let text = if blocking.is_empty() {
        engine::render_nudges(verdict)
    } else {
        engine::render_reason(verdict)
    };
    println!("{}", emit::prompt_context(&text));
    Ok(0)
}

/// Tool gate. Two duties:
/// - hermes: the only enforcement channel — block tools while red, allow
///   remediation (calls touching a required artifact).
/// - claude/codex/devin: protect the git layer — deny `--no-verify` while
///   blocking rules are red, so an agent can't skip the pre-push wall.
fn handle_toolgate(
    harness: Harness,
    event: &str,
    state: &State,
    verdict: &engine::Verdict,
    payload_text: &str,
    payload: &Value,
) -> Result<i32, String> {
    let blocking = verdict.blocking();

    if harness != Harness::Hermes {
        // --no-verify guard: only while red; when green the flag is harmless.
        let command = payload["tool_input"]["command"].as_str().unwrap_or("");
        let dodging = command.contains("--no-verify")
            && (command.contains("git commit") || command.contains("git push"));
        if dodging && !blocking.is_empty() {
            state.log_event(harness.name(), event, "no-verify-denied", command);
            println!(
                "{}",
                emit::tool_block(
                    harness,
                    "stele: --no-verify is disabled while rules are failing. Fix the findings (run `stele check`) or acknowledge them (`stele ack <rule-id> -m \"why\"`), then commit normally.",
                )
            );
        }
        return Ok(0);
    }

    // Hermes gatekeeper. Only artifact rules engage: a command rule gives no
    // safe remediation-allowance heuristic and would deadlock the agent.
    let artifact_reds: Vec<_> = blocking
        .iter()
        .filter(|r| r.rule.artifact.is_some())
        .collect();
    if artifact_reds.is_empty() {
        if let Some(allow) = emit::tool_allow(harness) {
            println!("{allow}");
        }
        return Ok(0);
    }
    let remediation = artifact_reds.iter().any(|r| {
        r.rule
            .artifact
            .as_ref()
            .is_some_and(|a| payload_text.contains(a.path.as_str()))
    });
    if remediation {
        state.log_event(harness.name(), event, "allowed-remediation", "");
        if let Some(allow) = emit::tool_allow(harness) {
            println!("{allow}");
        }
        return Ok(0);
    }
    state.log_event(harness.name(), event, "tool-blocked", "");
    println!("{}", emit::tool_block(harness, &engine::render_reason(verdict)));
    Ok(0)
}
