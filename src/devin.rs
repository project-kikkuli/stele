//! Cloud Devin support.
//!
//! Cloud Devin has no hook API. Its two validated channels (2026-07-16 live
//! session, see conformance/RESULTS.md):
//! - fast: git hooks installed in the machine snapshot — Devin sees pre-push
//!   failures as instant stderr in its own terminal.
//! - corrective: the External API message channel. Caveat found live: a
//!   message sent right after Devin stops can sit unprocessed for minutes, so
//!   the watcher POLLS session state and RE-SENDS instead of fire-and-forget.
//!
//! API access shells out to curl (ubiquitous, keeps the binary TLS-free);
//! auth via DEVIN_API_KEY.

use crate::config;
use crate::engine;
use crate::substrate;
use serde_json::Value;
use std::process::Command;

const API: &str = "https://api.devin.ai/v1";

pub fn setup() -> i32 {
    println!(
        r#"Cloud Devin setup — two channels, do both:

1. Fast channel (git hooks in the machine snapshot)
   In Devin's machine setup (Settings → Devin's Machine), add to the
   startup commands:

     cargo install stele --locked || true   # or copy a prebuilt binary
     cd <your-repo> && stele compile        # installs .git/hooks/pre-push

   Devin then hits the same wall locally that CI enforces, as stderr in
   its own terminal — the fastest feedback cloud Devin can get.

2. Corrective channel (message watcher)
   From your repo, after kicking off a session:

     DEVIN_API_KEY=... stele devin watch <session-id>

   The watcher polls the session; when Devin goes idle while `stele check`
   is red, it sends the findings as a message — and re-sends once if the
   session doesn't wake (messages to stopped sessions can sit unprocessed;
   found live 2026-07-16)."#
    );
    0
}

pub fn watch(session_id: &str, max_nudges: u32, poll_secs: u64) -> i32 {
    let Ok(key) = std::env::var("DEVIN_API_KEY") else {
        eprintln!("stele devin watch: set DEVIN_API_KEY (org settings → API keys)");
        return 2;
    };
    let mut nudges = 0u32;
    let mut last_status = String::new();
    loop {
        let Some(session) = api_get(&key, &format!("{API}/session/{session_id}")) else {
            eprintln!("stele devin watch: cannot reach session {session_id}");
            return 2;
        };
        let status = session["status_enum"]
            .as_str()
            .or(session["status"].as_str())
            .unwrap_or("unknown")
            .to_string();
        if status != last_status {
            eprintln!("stele devin watch: session status = {status}");
            last_status = status.clone();
        }

        let idle = matches!(
            status.as_str(),
            "blocked" | "stopped" | "finished" | "expired" | "awaiting_user" | "sleeping"
        );
        if matches!(status.as_str(), "finished" | "expired") && nudges > 0 {
            // Session over after our nudging: report final local verdict.
            return report_final();
        }
        if idle {
            match verdict_now() {
                None => {
                    eprintln!("stele devin watch: cannot measure locally (not in the repo?); watching only");
                }
                Some(v) if v.blocking().is_empty() => {
                    eprintln!("stele devin watch: green — done");
                    return 0;
                }
                Some(v) => {
                    if nudges >= max_nudges {
                        eprintln!("stele devin watch: still red after {max_nudges} nudges; giving up (CI still stands)");
                        eprintln!("{}", engine::render_reason(&v));
                        return 1;
                    }
                    nudges += 1;
                    eprintln!(
                        "stele devin watch: red — sending findings (nudge {nudges}/{max_nudges})"
                    );
                    let reason = engine::render_reason(&v);
                    api_post_message(&key, session_id, &reason);
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_secs(poll_secs));
    }
}

fn report_final() -> i32 {
    match verdict_now() {
        Some(v) if !v.blocking().is_empty() => {
            eprintln!("stele devin watch: session ended RED:");
            eprintln!("{}", engine::render_reason(&v));
            1
        }
        _ => {
            eprintln!("stele devin watch: session ended green");
            0
        }
    }
}

fn verdict_now() -> Option<engine::Verdict> {
    let root = std::env::current_dir().ok()?;
    let sub = substrate::compute(&root).ok()?;
    let rules = config::load(&sub.root).ok()?;
    Some(engine::check(&rules, &sub))
}

fn api_get(key: &str, url: &str) -> Option<Value> {
    let out = Command::new("curl")
        .args([
            "-sS",
            "-m",
            "30",
            "-H",
            &format!("Authorization: Bearer {key}"),
            url,
        ])
        .output()
        .ok()?;
    serde_json::from_slice(&out.stdout).ok()
}

fn api_post_message(key: &str, session_id: &str, message: &str) {
    let body = serde_json::json!({ "message": message }).to_string();
    let _ = Command::new("curl")
        .args([
            "-sS",
            "-m",
            "30",
            "-X",
            "POST",
            "-H",
            &format!("Authorization: Bearer {key}"),
            "-H",
            "Content-Type: application/json",
            "-d",
            &body,
            &format!("{API}/session/{session_id}/message"),
        ])
        .output();
}
