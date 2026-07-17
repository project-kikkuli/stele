//! `stele conformance` — drive REAL harnesses against a throwaway fixture and
//! verify the gates actually fire. This is the product's regression suite
//! against harness drift: docs lie, versions drift, channels silently vanish
//! (all observed live — see conformance/RESULTS.md).
//!
//! Each run: fresh temp git repo + stele.toml + `stele compile`, give the
//! agent a task that never mentions the required artifact, then assert the
//! artifact exists, `stele check` is green, and telemetry shows the gate
//! fired. Costs real agent invocations; it's an explicit command, not CI.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::{compile, config, substrate};

const TASK: &str =
    "Add a greet(name) function to app.py that returns f'Hello, {name}!'. Keep the change minimal.";

pub struct Outcome {
    pub harness: String,
    pub passed: bool,
    pub detail: String,
}

pub fn run(harnesses: &[String]) -> i32 {
    let all = [
        "claude-code",
        "codex",
        "codex-global",
        "cursor-run",
        "hermes",
        "git-pre-push",
    ];
    let selected: Vec<&str> = if harnesses.is_empty() {
        all.to_vec()
    } else {
        all.iter()
            .copied()
            .filter(|h| harnesses.iter().any(|s| s == h))
            .collect()
    };
    if selected.is_empty() {
        eprintln!("stele conformance: no known harness in {harnesses:?} (known: {all:?})");
        return 2;
    }

    let mut outcomes = Vec::new();
    for harness in selected {
        eprintln!("── conformance: {harness}");
        let outcome = match run_one(harness) {
            Ok(o) => o,
            Err(e) => Outcome {
                harness: harness.to_string(),
                passed: false,
                detail: e,
            },
        };
        eprintln!(
            "   {} {}",
            if outcome.passed { "PASS" } else { "FAIL" },
            outcome.detail
        );
        outcomes.push(outcome);
    }

    println!("\nharness        result  detail");
    println!("─────────────  ──────  ──────");
    let mut failed = false;
    for o in &outcomes {
        println!(
            "{:<13}  {:<6}  {}",
            o.harness,
            if o.passed { "PASS" } else { "FAIL" },
            o.detail
        );
        failed |= !o.passed;
    }
    if failed {
        1
    } else {
        0
    }
}

fn run_one(harness: &str) -> Result<Outcome, String> {
    if harness == "codex-global" {
        return run_codex_global();
    }
    let fixture = provision(harness)?;
    let dir = fixture.path();

    match harness {
        "claude-code" => {
            require("claude")?;
            agent(
                dir,
                "claude",
                &[
                    "-p",
                    TASK,
                    "--dangerously-skip-permissions",
                    "--model",
                    "sonnet",
                ],
                &[],
            )?;
        }
        "codex" => {
            require("codex")?;
            agent(
                dir,
                "codex",
                &[
                    "exec",
                    "--dangerously-bypass-approvals-and-sandbox",
                    "--dangerously-bypass-hook-trust",
                    TASK,
                ],
                &[],
            )?;
        }
        "cursor-run" => {
            require("cursor-agent")?;
            let linked = dir.with_file_name(format!(
                "{}-linked",
                dir.file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("stele-conformance-cursor")
            ));
            let _ = fs::remove_dir_all(&linked);
            let status = Command::new("git")
                .args(["worktree", "add", "--quiet", "-b", "cursor-run"])
                .arg(&linked)
                .arg("HEAD")
                .current_dir(dir)
                .status()
                .map_err(|e| format!("git worktree add: {e}"))?;
            if !status.success() {
                return Err("git worktree add failed".into());
            }
            let _linked_guard = TempPath(linked.clone());
            let stele = std::env::current_exe().map_err(|e| e.to_string())?;
            agent(
                &linked,
                stele.to_str().unwrap_or("stele"),
                &["run", "cursor", TASK],
                &[],
            )?;
            return assess(harness, &linked);
        }
        "hermes" => {
            require("hermes")?;
            let _guard = HermesWiring::install(dir)?;
            agent(
                dir,
                "hermes",
                &["--yolo", "-z", TASK],
                &[("HERMES_ACCEPT_HOOKS", "1")],
            )?;
        }
        "git-pre-push" => {
            // Environment tier: no agent needed. Red worktree must make the
            // stele-owned pre-push hook exit nonzero; green must pass it.
            let hook = dir.join(".git/hooks/pre-push");
            // Dirty the tree first — a clean fixture is green by definition.
            fs::write(dir.join("app.py"), "def add(a, b):\n    return a + b\n\n\ndef greet(name):\n    return f'Hello, {name}!'\n")
                .map_err(|e| e.to_string())?;
            let red = Command::new(&hook)
                .current_dir(dir)
                .env(config::DISABLE_GLOBAL_ENV, "1")
                .output()
                .map_err(|e| format!("pre-push: {e}"))?;
            fs::write(
                dir.join("requirements.md"),
                "# Requirements\n\n## Functional\n\ngreet added\n\n## Risks\n\nnone\n",
            )
            .map_err(|e| e.to_string())?;
            let green = Command::new(&hook)
                .current_dir(dir)
                .env(config::DISABLE_GLOBAL_ENV, "1")
                .output()
                .map_err(|e| format!("pre-push: {e}"))?;
            let passed = !red.status.success() && green.status.success();
            return Ok(Outcome {
                harness: harness.into(),
                passed,
                detail: format!(
                    "red exit={:?} green exit={:?}",
                    red.status.code(),
                    green.status.code()
                ),
            });
        }
        other => return Err(format!("unknown harness {other}")),
    }

    assess(harness, dir)
}

/// Live proof for the personal-policy wow path: Codex starts in a clean repo
/// with no repository `stele.toml`; a user-level `trigger = "always"` rule and
/// user hook must surface before the agent can edit the primary checkout.
fn run_codex_global() -> Result<Outcome, String> {
    require("codex")?;
    let fixture = Fixture(std::env::temp_dir().join(format!(
        "stele-conformance-codex-global-{}",
        std::process::id()
    )));
    let base = fixture.path();
    let _ = fs::remove_dir_all(base);
    fs::create_dir_all(base).map_err(|e| e.to_string())?;
    let dir = base.join("repo");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    sh(
        &dir,
        "git init -qb main && git config user.email t@t && git config user.name stele",
    )?;
    const ORIGINAL: &str = "def add(a, b):\n    return a + b\n";
    fs::write(dir.join("app.py"), ORIGINAL).map_err(|e| e.to_string())?;
    sh(
        &dir,
        "git add -A && git commit -qm 'fixture: initial state'",
    )?;

    let user_config = base.join("personal/stele.toml");
    fs::create_dir_all(user_config.parent().unwrap_or(base)).map_err(|e| e.to_string())?;
    fs::write(
        &user_config,
        r###"[[rule]]
id = "personal-worktree-only"
description = "agents always work in linked git worktrees"
trigger = "always"
acknowledge = false
message = "Relaunch from a linked worktree."
check = '''
git_dir=$(git rev-parse --path-format=absolute --git-dir 2>/dev/null) || exit 1
common_dir=$(git rev-parse --path-format=absolute --git-common-dir 2>/dev/null) || exit 1
[ "$git_dir" != "$common_dir" ] || {
  echo '✗ agent session is running in the primary checkout, not a linked worktree'
  exit 1
}
'''
"###,
    )
    .map_err(|e| e.to_string())?;

    // Isolate Codex's user config while reusing the caller's authentication.
    let fake_home = base.join("home");
    let codex_home = fake_home.join(".codex");
    fs::create_dir_all(&codex_home).map_err(|e| e.to_string())?;
    let real_codex_home = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".codex")))
        .ok_or("no CODEX_HOME or HOME for Codex authentication")?;
    let auth = real_codex_home.join("auth.json");
    if !auth.is_file() {
        return Err(format!("{} missing; cannot run live Codex", auth.display()));
    }
    fs::copy(&auth, codex_home.join("auth.json")).map_err(|e| e.to_string())?;
    compile::install_global_at(&fake_home)?;

    let codex_home = codex_home.to_string_lossy().into_owned();
    let user_config = user_config.to_string_lossy().into_owned();
    let system_config = base
        .join("missing-system.toml")
        .to_string_lossy()
        .into_owned();
    agent(
        &dir,
        "codex",
        &[
            "exec",
            "--dangerously-bypass-approvals-and-sandbox",
            "--dangerously-bypass-hook-trust",
            TASK,
        ],
        &[
            ("CODEX_HOME", &codex_home),
            (config::USER_CONFIG_ENV, &user_config),
            (config::SYSTEM_CONFIG_ENV, &system_config),
            (config::DISABLE_GLOBAL_ENV, "0"),
        ],
    )?;

    let unchanged = fs::read_to_string(dir.join("app.py")).unwrap_or_default() == ORIGINAL;
    let events = fs::read_to_string(dir.join(".git/stele/events.jsonl")).unwrap_or_default();
    let context_fired = events.contains("context-injected");
    let gate_fired = events.contains("preflight-blocked");
    let policy_fired = context_fired || gate_fired;
    Ok(Outcome {
        harness: "codex-global".into(),
        passed: unchanged && policy_fired,
        detail: format!(
            "primary-untouched={unchanged} session-context={context_fired} pretool-gate={gate_fired}"
        ),
    })
}

/// Fresh fixture: git repo + app.py + stele.toml (red rule: touching *.py
/// requires requirements.md) + compiled wiring. The task seeds a py change,
/// so the rule triggers as soon as the agent works.
struct Fixture(PathBuf);
impl Fixture {
    fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn provision(harness: &str) -> Result<Fixture, String> {
    let dir = std::env::temp_dir().join(format!(
        "stele-conformance-{harness}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    sh(
        &dir,
        "git init -qb main && git config user.email t@t && git config user.name stele",
    )?;
    fs::write(dir.join("app.py"), "def add(a, b):\n    return a + b\n")
        .map_err(|e| e.to_string())?;
    fs::write(
        dir.join("stele.toml"),
        r###"[[rule]]
id = "requirements-doc"
description = "every change ships with an up-to-date requirements.md"
severity = "block"

[rule.artifact]
path = "requirements.md"
sections = ["# Requirements", "## Functional", "## Risks"]
"###,
    )
    .map_err(|e| e.to_string())?;

    let stele = std::env::current_exe().map_err(|e| e.to_string())?;
    let out = Command::new(&stele)
        .arg("compile")
        .current_dir(&dir)
        .env(config::DISABLE_GLOBAL_ENV, "1")
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!(
            "stele compile failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    // Isolate the enforcement channel under test: the generated AGENTS.md is
    // the exhortation layer, and agents that read it comply proactively —
    // which would leave the gate untested (observed live: codex and cursor
    // created the artifact from AGENTS.md alone).
    let _ = fs::remove_file(dir.join("AGENTS.md"));
    sh(
        &dir,
        "git add -A && git commit -qm 'fixture: initial state'",
    )?;
    Ok(Fixture(dir))
}

fn assess(harness: &str, dir: &Path) -> Result<Outcome, String> {
    let stele = std::env::current_exe().map_err(|e| e.to_string())?;
    let check = Command::new(&stele)
        .arg("check")
        .current_dir(dir)
        .env(config::DISABLE_GLOBAL_ENV, "1")
        .output()
        .map_err(|e| e.to_string())?;
    let green = check.status.success();
    let artifact = dir.join("requirements.md").is_file();
    let events = substrate::find_git_dir(dir)
        .ok()
        .and_then(|git_dir| fs::read_to_string(git_dir.join("stele/events.jsonl")).ok())
        .unwrap_or_default();
    let gate_fired = events.contains("\"blocked\"")
        || events.contains("tool-blocked")
        || events.contains("synthetic-stop");
    Ok(Outcome {
        harness: harness.into(),
        passed: green && artifact && gate_fired,
        detail: format!("green={green} artifact={artifact} gate-fired={gate_fired}"),
    })
}

struct TempPath(PathBuf);

impl Drop for TempPath {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn agent(dir: &Path, bin: &str, args: &[&str], envs: &[(&str, &str)]) -> Result<(), String> {
    let mut cmd = Command::new(bin);
    cmd.args(args)
        .current_dir(dir)
        .env(config::DISABLE_GLOBAL_ENV, "1");
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let out = cmd.output().map_err(|e| format!("{bin}: {e}"))?;
    if !out.status.success() {
        eprintln!(
            "   ({bin} exited {:?}: stderr={} stdout={})",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
                .lines()
                .last()
                .unwrap_or(""),
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .last()
                .unwrap_or("")
        );
    }
    Ok(())
}

fn require(bin: &str) -> Result<(), String> {
    Command::new(bin)
        .arg("--version")
        .output()
        .map(|_| ())
        .map_err(|_| format!("{bin} not installed — skipping requires explicit harness list"))
}

fn sh(dir: &Path, script: &str) -> Result<(), String> {
    let out = Command::new("bash")
        .arg("-c")
        .arg(script)
        .current_dir(dir)
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!(
            "{script}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(())
}

/// Temporarily wire ~/.hermes/config.yaml to the fixture's shim; restore on
/// drop. Hermes hooks are global-only, so a conformance run must mutate the
/// user config — with a backup, and only for the run's duration.
struct HermesWiring {
    config: PathBuf,
    original: String,
}

impl HermesWiring {
    fn install(fixture: &Path) -> Result<Self, String> {
        let home = std::env::var("HOME").map_err(|_| "no $HOME")?;
        let config = Path::new(&home).join(".hermes/config.yaml");
        let original =
            fs::read_to_string(&config).map_err(|e| format!("{}: {e}", config.display()))?;
        let shim = fixture.join(".stele/hermes-shim.sh");
        let entry = format!(
            "hooks:\n  pre_tool_call:\n    - command: \"{} pre_tool_call\"\n      timeout: 60",
            shim.display()
        );
        let updated = if original.contains("\nhooks: {}\n") {
            original.replace("\nhooks: {}\n", &format!("\n{entry}\n"))
        } else if !original.lines().any(|l| l.starts_with("hooks:")) {
            format!("{}\n\n{entry}\n", original.trim_end())
        } else {
            return Err(
                "~/.hermes/config.yaml has a custom hooks section; run hermes conformance manually"
                    .into(),
            );
        };
        fs::write(&config, updated).map_err(|e| e.to_string())?;
        Ok(HermesWiring { config, original })
    }
}

impl Drop for HermesWiring {
    fn drop(&mut self) {
        let _ = fs::write(&self.config, &self.original);
    }
}
