//! `stele doctor` — verify the wiring actually exists and can fire.
//!
//! The conformance runs proved wiring can silently not exist (cursor headless
//! executes no hooks; hermes ignores duplicate YAML keys; codex needs hook
//! trust). doctor makes "is the gate actually armed?" a one-command question.

use crate::config;
use crate::substrate;
use std::path::Path;
use std::process::Command;

struct Report {
    problems: u32,
}

impl Report {
    fn ok(&mut self, msg: &str) {
        println!("  ✓ {msg}");
    }
    fn warn(&mut self, msg: &str) {
        println!("  ! {msg}");
    }
    fn bad(&mut self, msg: &str) {
        println!("  ✗ {msg}");
        self.problems += 1;
    }
}

pub fn run() -> i32 {
    let mut r = Report { problems: 0 };

    println!("stele doctor");

    // Binary reachable by the name the generated wiring uses.
    match which("stele") {
        Some(path) => r.ok(&format!("`stele` on PATH ({path})")),
        None => r.bad("`stele` not on PATH — every generated hook invokes it by name"),
    }

    let cwd = std::env::current_dir().unwrap_or_default();
    let Ok(root) = substrate::find_root(&cwd) else {
        r.bad("not inside a git repository");
        return finish(r);
    };
    match config::load(&root) {
        Ok(rules) => r.ok(&format!("stele.toml with {} rule(s)", rules.len())),
        Err(e) => {
            r.bad(&format!("stele.toml: {e}"));
            return finish(r);
        }
    }

    // Per-channel wiring.
    check_json_wiring(&mut r, &root, ".claude/settings.json", "claude-code");
    check_json_wiring(&mut r, &root, ".codex/hooks.json", "codex");
    check_json_wiring(&mut r, &root, ".devin/hooks.v1.json", "devin-cli");
    check_json_wiring(&mut r, &root, ".cursor/hooks.json", "cursor");

    let pre_push = root.join(".git/hooks/pre-push");
    match std::fs::read_to_string(&pre_push) {
        Ok(body) if body.contains("stele") => r.ok("git pre-push hook (stele-owned)"),
        Ok(_) => r.warn("git pre-push exists but is not stele's (CI still enforces)"),
        Err(_) => r.bad("git pre-push hook missing — run `stele compile`"),
    }
    if root.join(".github/workflows/stele.yml").is_file() {
        r.ok("CI workflow present");
    } else {
        r.bad("CI workflow missing — the unbypassable floor isn't installed");
    }

    // Hermes: global config, so check the user's config for our shim.
    let shim = root.join(".stele/hermes-shim.sh");
    if shim.is_file() {
        let hermes_cfg = std::env::var("HOME")
            .map(|h| Path::new(&h).join(".hermes/config.yaml"))
            .ok();
        match hermes_cfg.and_then(|p| std::fs::read_to_string(p).ok()) {
            Some(cfg) if cfg.contains(&shim.display().to_string()) => {
                r.ok("hermes global config wired to this repo's shim")
            }
            Some(_) => r.warn("hermes installed but not wired — run `stele install hermes`"),
            None => r.warn("hermes config not found (skip if you don't use hermes)"),
        }
    }

    // Installed harness versions — drift is the enemy; record what's here.
    for (bin, args) in [
        ("claude", vec!["--version"]),
        ("codex", vec!["--version"]),
        ("cursor-agent", vec!["--version"]),
        ("hermes", vec!["--version"]),
        ("devin", vec!["--version"]),
    ] {
        match version_of(bin, &args) {
            Some(v) => r.ok(&format!("{bin}: {v}")),
            None => r.warn(&format!("{bin}: not installed")),
        }
    }

    finish(r)
}

fn finish(r: Report) -> i32 {
    if r.problems == 0 {
        println!("all armed.");
        0
    } else {
        println!("{} problem(s).", r.problems);
        1
    }
}

fn check_json_wiring(r: &mut Report, root: &Path, rel: &str, harness: &str) {
    let path = root.join(rel);
    match std::fs::read_to_string(&path) {
        Ok(body) if body.contains(&format!("stele hook {harness}")) => {
            r.ok(&format!("{rel} wired"));
            if harness == "cursor" {
                r.warn("cursor: hooks fire in the IDE only — headless runs need `stele wrap`");
            }
        }
        Ok(_) => r.bad(&format!("{rel} exists but has no stele hook — run `stele compile`")),
        Err(_) => r.bad(&format!("{rel} missing — run `stele compile`")),
    }
}

fn which(bin: &str) -> Option<String> {
    let out = Command::new("which").arg(bin).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn version_of(bin: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(bin).args(args).output().ok()?;
    out.status.success().then(|| {
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .next()
            .unwrap_or("")
            .trim()
            .to_string()
    })
}
