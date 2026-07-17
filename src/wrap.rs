//! `stele wrap --prompt "<task>" -- <agent command>` — the synthesized
//! stop-loop for harnesses with no native hook channel (validated on
//! cursor-agent headless).
//!
//! Runs `<agent command> <task>`, measures at process exit, then re-runs
//! `<agent command> --resume <session_id> <findings>` until green or the loop
//! cap. No hook API required, immune to hook-system version drift.

use crate::config;
use crate::engine;
use crate::substrate;
use serde_json::Value;
use std::process::Command;

pub fn run(max_loops: u32, prompt: &str, cmd: &[String]) -> i32 {
    if cmd.is_empty() {
        eprintln!("stele wrap: no agent command given (use: stele wrap --prompt '<task>' -- cursor-agent -p --force)");
        return 2;
    }

    // Session preconditions must be satisfied before the hook-less agent gets
    // its first chance to mutate the checkout. This is what makes personal
    // `trigger = "always"` rules (for example, worktree-only) meaningful on
    // wrapped CLIs too.
    if let Some(verdict) = verdict_now() {
        if !verdict.preflight().is_empty() {
            eprintln!("stele wrap: session preflight failed:");
            eprintln!("{}", engine::render_preflight(&verdict));
            return 1;
        }
    }

    let mut argv: Vec<String> = cmd.to_vec();
    if argv[0].contains("cursor-agent") && !argv.iter().any(|a| a == "--output-format") {
        argv.extend(["--output-format".into(), "json".into()]);
    }

    let first = run_agent(&argv, &[prompt.to_string()]);
    let Some(first) = first else { return 2 };
    let session_id = extract_session_id(&first);

    for i in 1..=max_loops {
        match verdict_now() {
            None => return 0, // can't measure: fail open, env tier still stands
            Some(v) if v.blocking().is_empty() => {
                eprintln!("stele wrap: green after {} synthetic block(s)", i - 1);
                return 0;
            }
            Some(v) => {
                let Some(sid) = &session_id else {
                    eprintln!("stele wrap: findings remain but no session id to resume; giving up");
                    eprintln!("{}", engine::render_reason(&v));
                    return 1;
                };
                if let Ok(sub) = substrate::compute(&std::env::current_dir().unwrap_or_default()) {
                    engine::State::new(&sub).log_event(
                        "wrap",
                        &format!("synthetic-stop-{i}"),
                        "blocked",
                        "",
                    );
                }
                eprintln!("stele wrap: findings — resuming session {sid} (loop {i}/{max_loops})");
                run_agent(
                    &argv,
                    &["--resume".into(), sid.clone(), engine::render_reason(&v)],
                );
            }
        }
    }

    match verdict_now() {
        Some(v) if !v.blocking().is_empty() => {
            eprintln!("stele wrap: STILL RED after {max_loops} synthetic blocks:");
            eprintln!("{}", engine::render_reason(&v));
            1
        }
        _ => {
            eprintln!("stele wrap: green after {max_loops} synthetic block(s)");
            0
        }
    }
}

fn run_agent(argv: &[String], extra: &[String]) -> Option<String> {
    let out = Command::new(&argv[0])
        .args(&argv[1..])
        .args(extra)
        .output()
        .map_err(|e| eprintln!("stele wrap: failed to run agent: {e}"))
        .ok()?;
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    print!("{stdout}");
    Some(stdout)
}

fn verdict_now() -> Option<engine::Verdict> {
    let root = std::env::current_dir().ok()?;
    let sub = substrate::compute(&root).ok()?;
    let rules = config::load(&sub.root).ok()?;
    Some(engine::check(&rules, &sub))
}

fn extract_session_id(stdout: &str) -> Option<String> {
    for line in stdout.lines().rev() {
        if let Ok(v) = serde_json::from_str::<Value>(line) {
            if let Some(sid) = v["session_id"].as_str() {
                return Some(sid.to_string());
            }
        }
    }
    None
}
