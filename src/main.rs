use stele::{ack, compile, config, conformance, devin, doctor, engine, hook, substrate, wrap};

use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;
use std::process::ExitCode;

/// Declare a rule once; enforce it across every AI coding agent harness.
#[derive(Parser)]
#[command(name = "stele", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Write a starter stele.toml into the current repo.
    Init {
        /// Create the personal config used across all repositories.
        #[arg(long, conflicts_with = "system")]
        global: bool,
        /// Create the machine/admin config (normally requires root).
        #[arg(long)]
        system: bool,
    },
    /// Measure the current change-set against the rules.
    /// Exit: 0 green · 1 findings · 3 couldn't measure (CI treats both as failure).
    Check {
        /// Print nothing when green (for git hooks).
        #[arg(long)]
        quiet_green: bool,
    },
    /// Harness hook entrypoint (payload on stdin). Always exits 0: fail-open.
    Hook {
        harness: String,
        event: String,
        /// Rule layers evaluated by this hook source.
        #[arg(long, value_enum, default_value_t = HookScope::All)]
        scope: HookScope,
    },
    /// Synthesized stop-loop for hook-less CLIs (validated: cursor-agent).
    Wrap {
        /// The task prompt to give the agent on the first run.
        #[arg(long)]
        prompt: String,
        /// Max synthetic blocks before giving up.
        #[arg(long, default_value_t = 2)]
        max_loops: u32,
        /// Agent command, after `--` (e.g. cursor-agent -p --force).
        #[arg(last = true)]
        cmd: Vec<String>,
    },
    /// Fan stele.toml out to every delivery channel (idempotent, merge-safe).
    Compile,
    /// Install user-level Stele hook wiring.
    Install {
        /// `global` (Claude/Codex/Cursor) or `hermes`.
        harness: String,
    },
    /// Acknowledge an intentional red: records a `Stele-Ack:` commit trailer
    /// so the rule reports as acknowledged and stops gating for this branch.
    Ack {
        rule_id: String,
        /// Why this red is intentional (recorded in the commit, visible in review).
        #[arg(short, long)]
        message: String,
    },
    /// Verify the wiring actually exists and can fire, per channel.
    Doctor,
    /// Drive real installed harnesses against a throwaway fixture end-to-end.
    Conformance {
        /// Harnesses to run (default: all installed): claude-code codex codex-global cursor-wrap hermes git-pre-push
        harnesses: Vec<String>,
    },
    /// Cloud Devin: `setup` prints snapshot wiring; `watch <session-id>`
    /// polls the session and injects findings via the API.
    Devin {
        #[command(subcommand)]
        cmd: DevinCmd,
    },
}

#[derive(Subcommand)]
enum DevinCmd {
    Setup,
    Watch {
        session_id: String,
        #[arg(long, default_value_t = 2)]
        max_nudges: u32,
        #[arg(long, default_value_t = 30)]
        poll_secs: u64,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum, Default)]
enum HookScope {
    #[default]
    All,
    Global,
    Repo,
}

impl From<HookScope> for config::LoadScope {
    fn from(value: HookScope) -> Self {
        match value {
            HookScope::All => Self::All,
            HookScope::Global => Self::Global,
            HookScope::Repo => Self::Repository,
        }
    }
}

const STARTER: &str = r###"# stele.toml — rules measured on every change-set, enforced on every harness.
# Docs: https://github.com/project-kikkuli/stele

[[rule]]
id = "requirements-doc"
description = "every change ships with an up-to-date requirements.md"
severity = "block"

[rule.artifact]
path = "requirements.md"
sections = ["# Requirements", "## Functional", "## Risks"]
"###;

const GLOBAL_STARTER: &str = r###"# Personal Stele rules — evaluated in every git repository on this machine.
# Run `stele install global` once to wire supported agent harnesses.

[[rule]]
id = "personal-worktree-only"
description = "agents always work in linked git worktrees"
severity = "block"
trigger = "always"
acknowledge = false
message = "Create a linked worktree (`git worktree add ../<name> -b <branch>`) and relaunch the agent from that directory."
check = '''
git_dir=$(git rev-parse --path-format=absolute --git-dir 2>/dev/null) || exit 1
common_dir=$(git rev-parse --path-format=absolute --git-common-dir 2>/dev/null) || exit 1
[ "$git_dir" != "$common_dir" ] || {
  echo '✗ agent session is running in the primary checkout, not a linked worktree'
  exit 1
}
'''
"###;

const SYSTEM_STARTER: &str = r###"# Machine-wide Stele rules — provision this file on developer machines and CI runners.

[[rule]]
id = "system-worktree-only"
description = "agents always work in linked git worktrees"
severity = "block"
trigger = "always"
acknowledge = false
message = "Create a linked worktree (`git worktree add ../<name> -b <branch>`) and relaunch the agent from that directory."
check = '''
git_dir=$(git rev-parse --path-format=absolute --git-dir 2>/dev/null) || exit 1
common_dir=$(git rev-parse --path-format=absolute --git-common-dir 2>/dev/null) || exit 1
[ "$git_dir" != "$common_dir" ] || {
  echo '✗ agent session is running in the primary checkout, not a linked worktree'
  exit 1
}
'''
"###;

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Cmd::Init { global, system } => run_init(global, system),
        Cmd::Check { quiet_green } => run_check(quiet_green),
        Cmd::Hook {
            harness,
            event,
            scope,
        } => ExitCode::from(hook::run(&harness, &event, scope.into()) as u8),
        Cmd::Wrap {
            prompt,
            max_loops,
            cmd,
        } => ExitCode::from(wrap::run(max_loops, &prompt, &cmd) as u8),
        Cmd::Compile => run_compile(),
        Cmd::Install { harness } => run_install(&harness),
        Cmd::Ack { rule_id, message } => run_ack(&rule_id, &message),
        Cmd::Doctor => ExitCode::from(doctor::run() as u8),
        Cmd::Conformance { harnesses } => ExitCode::from(conformance::run(&harnesses) as u8),
        Cmd::Devin { cmd } => match cmd {
            DevinCmd::Setup => ExitCode::from(devin::setup() as u8),
            DevinCmd::Watch {
                session_id,
                max_nudges,
                poll_secs,
            } => ExitCode::from(devin::watch(&session_id, max_nudges, poll_secs) as u8),
        },
    }
}

fn run_init(global: bool, system: bool) -> ExitCode {
    let (path, starter, next) = if global {
        let path = match config::user_config_path() {
            Ok(path) => path,
            Err(e) => {
                eprintln!("stele init --global: {e}");
                return ExitCode::from(2);
            }
        };
        (path, GLOBAL_STARTER, "run `stele install global`")
    } else if system {
        (
            config::system_config_path(),
            SYSTEM_STARTER,
            "provision the same file and Stele hooks on each machine/runner",
        )
    } else {
        (
            PathBuf::from(config::CONFIG_NAME),
            STARTER,
            "edit it, then run `stele compile`",
        )
    };
    if path.exists() {
        eprintln!("{} already exists", path.display());
        return ExitCode::from(2);
    }
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            eprintln!("could not create {}: {e}", parent.display());
            return ExitCode::from(2);
        }
    }
    if let Err(e) = std::fs::write(&path, starter) {
        eprintln!("could not write {}: {e}", path.display());
        return ExitCode::from(2);
    }
    println!("wrote {} — {next}", path.display());
    ExitCode::SUCCESS
}

fn run_ack(rule_id: &str, message: &str) -> ExitCode {
    let cwd = std::env::current_dir().unwrap_or_default();
    let sub = match substrate::compute(&cwd) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("stele: {e}");
            return ExitCode::from(2);
        }
    };
    // Refuse to ack a rule that isn't actually failing or doesn't exist —
    // pre-emptive blanket acks would hollow the whole system out.
    let rules = match config::load(&sub.root) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("stele: {e}");
            return ExitCode::from(2);
        }
    };
    if !rules.iter().any(|r| r.id == rule_id) {
        eprintln!("stele ack: no rule with id `{rule_id}`");
        return ExitCode::from(2);
    }
    if rules
        .iter()
        .find(|r| r.id == rule_id)
        .is_some_and(|r| !r.acknowledge)
    {
        eprintln!("stele ack: rule `{rule_id}` does not allow acknowledgements");
        return ExitCode::from(2);
    }
    let verdict = engine::check(&rules, &sub);
    if !verdict.red().iter().any(|r| r.rule.id == rule_id) {
        eprintln!("stele ack: rule `{rule_id}` is not currently failing — nothing to acknowledge");
        return ExitCode::from(2);
    }
    match ack::create(&sub, rule_id, message) {
        Ok(()) => {
            println!("acknowledged `{rule_id}` (commit trailer recorded; visible in review)");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("stele ack: {e}");
            ExitCode::from(2)
        }
    }
}

fn run_check(quiet_green: bool) -> ExitCode {
    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("stele: {e}");
            return ExitCode::from(3);
        }
    };
    let sub = match substrate::compute(&cwd) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("stele: couldn't measure: {e}");
            return ExitCode::from(3);
        }
    };
    let rules = match config::load(&sub.root) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("stele: {e}");
            return ExitCode::from(3);
        }
    };
    let verdict = engine::check(&rules, &sub);

    // "Couldn't measure" is not green — surface it and fail distinctly.
    let errors = verdict.errors();
    if !errors.is_empty() {
        for r in &errors {
            eprintln!(
                "stele: rule `{}` couldn't be measured: {}",
                r.rule.id,
                r.error.as_deref().unwrap_or("")
            );
        }
        return ExitCode::from(3);
    }
    if verdict.blocking().is_empty() {
        // Nudges and acked reds surface as warnings but never fail.
        for r in verdict.nudges() {
            println!("stele: advisory `{}`:", r.rule.id);
            for f in &r.findings {
                println!("  {f}");
            }
        }
        for r in verdict.acknowledged() {
            println!(
                "stele: `{}` failing but acknowledged (Stele-Ack trailer)",
                r.rule.id
            );
        }
        if !quiet_green {
            let triggered = verdict.results.iter().filter(|r| r.triggered).count();
            println!(
                "stele: green ({} rule(s) measured, {} change(s))",
                triggered,
                sub.changed.len()
            );
        }
        if verdict.is_green() {
            let signature = engine::policy_signature(&rules, &sub.signature);
            engine::State::new(&sub).mark_green(&signature);
        }
        ExitCode::SUCCESS
    } else {
        print!("{}", engine::render_reason(&verdict));
        println!();
        ExitCode::from(1)
    }
}

fn run_compile() -> ExitCode {
    let cwd = std::env::current_dir().unwrap_or_default();
    let root = match substrate::find_root(&cwd) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("stele: {e}");
            return ExitCode::from(2);
        }
    };
    let rules = match config::load_repo(&root) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("stele: {e} (run `stele init` first)");
            return ExitCode::from(2);
        }
    };
    match compile::run(&root, &rules) {
        Ok(written) if written.is_empty() => {
            println!("stele compile: everything already up to date");
            ExitCode::SUCCESS
        }
        Ok(written) => {
            println!("stele compile: wrote");
            for w in written {
                println!("  {w}");
            }
            println!("\nchannels: claude-code ✓  codex ✓  devin-cli ✓  cursor-ide ✓  git pre-push ✓  ci ✓  AGENTS.md ✓");
            println!("hermes: run `stele install hermes` once per user");
            println!(
                "cursor headless: use `stele wrap --prompt '<task>' -- cursor-agent -p --force`"
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("stele compile: {e}");
            ExitCode::from(2)
        }
    }
}

fn run_install(harness: &str) -> ExitCode {
    if harness == "global" {
        let user_config = match config::user_config_path() {
            Ok(path) => path,
            Err(e) => {
                eprintln!("stele install global: {e}");
                return ExitCode::from(2);
            }
        };
        let system_config = config::system_config_path();
        if !user_config.is_file() && !system_config.is_file() {
            eprintln!(
                "stele install global: no personal config at {} or system config at {} (run `stele init --global`)",
                user_config.display(),
                system_config.display()
            );
            return ExitCode::from(2);
        }
        return match compile::install_global() {
            Ok(written) => {
                if written.is_empty() {
                    println!("stele install global: user hooks already up to date");
                } else {
                    println!("stele install global: wrote");
                    for path in written {
                        println!("  {path}");
                    }
                }
                println!("personal rules: claude-code ✓  codex ✓  cursor-ide ✓");
                println!("hermes: run `stele install hermes` once");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("stele install global: {e}");
                ExitCode::from(2)
            }
        };
    }
    if harness != "hermes" {
        eprintln!("stele install: expected `global` or `hermes`");
        return ExitCode::from(2);
    }
    match compile::install_hermes() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("stele install hermes: {e}");
            ExitCode::from(2)
        }
    }
}
