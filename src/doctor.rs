//! `stele doctor` — verify the wiring actually exists and can fire.
//!
//! The conformance runs proved wiring can silently not exist (cursor headless
//! executes no hooks; hermes ignores duplicate YAML keys; codex needs hook
//! trust). doctor makes "is the gate actually armed?" a one-command question.

use crate::config::{self, LayerKind, LoadScope};
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

    // bash is the runtime every rule check and generated hook (pre-push,
    // Hermes shim) executes through. Without it nothing can fire — on Windows
    // that means running under WSL or Git Bash.
    match version_of("bash", &["--version"]) {
        Some(v) => r.ok(&format!("bash: {v}")),
        None => r.bad("bash not found — rule checks and generated hooks run through it (Windows: use WSL or Git Bash)"),
    }

    let cwd = std::env::current_dir().unwrap_or_default();
    let Ok(root) = substrate::find_root(&cwd) else {
        r.bad("not inside a git repository");
        return finish(r);
    };
    let rules = match config::load(&root) {
        Ok(rules) => {
            r.ok(&format!("{} active rule(s)", rules.len()));
            rules
        }
        Err(e) => {
            r.bad(&format!("rules: {e}"));
            return finish(r);
        }
    };
    let layers = match config::layers(&root, LoadScope::All) {
        Ok(layers) => layers,
        Err(e) => {
            r.bad(&format!("layers: {e}"));
            return finish(r);
        }
    };
    for layer in &layers {
        r.ok(&format!(
            "{} config: {} ({} rule(s))",
            layer.kind.name(),
            layer.path.display(),
            layer.rules.len()
        ));
    }
    let has_repo = layers
        .iter()
        .any(|layer| layer.kind == LayerKind::Repository);
    let has_user = layers.iter().any(|layer| layer.kind == LayerKind::User);
    let has_system = layers.iter().any(|layer| layer.kind == LayerKind::System);
    let _ = rules;

    if has_repo {
        // Per-repository channel wiring.
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
    }

    if has_user {
        if let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) {
            check_json_path(
                &mut r,
                &home.join(".claude/settings.json"),
                "~/.claude/settings.json",
                "claude-code",
            );
            check_json_path(
                &mut r,
                &home.join(".codex/hooks.json"),
                "~/.codex/hooks.json",
                "codex",
            );
            check_json_path(
                &mut r,
                &home.join(".cursor/hooks.json"),
                "~/.cursor/hooks.json",
                "cursor",
            );
        } else {
            r.bad("no $HOME; cannot inspect personal hook wiring");
        }
    }
    if has_system {
        r.warn("system rules require managed hooks on each developer machine and CI runner");
    }

    // Hermes: global config, so check the user's config for our shim.
    if has_repo || has_user || has_system {
        let hermes_cfg = std::env::var("HOME")
            .map(|h| Path::new(&h).join(".hermes/config.yaml"))
            .ok();
        match hermes_cfg.and_then(|p| std::fs::read_to_string(p).ok()) {
            Some(cfg) if cfg.contains("stele") => r.ok("hermes global config wired"),
            Some(_) => r.warn("hermes installed but not wired — rerun `stele install global`"),
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
    check_json_path(r, &path, rel, harness);
}

fn check_json_path(r: &mut Report, path: &Path, label: &str, harness: &str) {
    match std::fs::read_to_string(path) {
        Ok(body) if body.contains(&format!("stele hook {harness}")) => {
            r.ok(&format!("{label} wired"));
            if harness == "cursor" {
                r.warn("cursor: hooks fire in the IDE only — use `stele run cursor` headless");
            }
        }
        Ok(_) => r.bad(&format!(
            "{label} exists but has no stele hook — run the relevant Stele installer"
        )),
        Err(_) => r.bad(&format!(
            "{label} missing — run the relevant Stele installer"
        )),
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
