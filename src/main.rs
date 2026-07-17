use stele::{ack, compile, config, conformance, devin, doctor, engine, hook, substrate, wrap};

use clap::{Parser, Subcommand};
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
    Init,
    /// Measure the current change-set against the rules.
    /// Exit: 0 green · 1 findings · 3 couldn't measure (CI treats both as failure).
    Check {
        /// Print nothing when green (for git hooks).
        #[arg(long)]
        quiet_green: bool,
    },
    /// Harness hook entrypoint (payload on stdin). Always exits 0: fail-open.
    Hook { harness: String, event: String },
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
    /// One-time per-user wiring for global-config harnesses.
    Install {
        /// Currently: hermes
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
        /// Harnesses to run (default: all installed): claude-code codex cursor-wrap hermes git-pre-push
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

const STARTER: &str = r###"# stele.toml — rules measured on every change-set, enforced on every harness.
# Docs: https://github.com/august-innovations/stele

[[rule]]
id = "requirements-doc"
description = "every change ships with an up-to-date requirements.md"
severity = "block"

[rule.artifact]
path = "requirements.md"
sections = ["# Requirements", "## Functional", "## Risks"]
"###;

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Cmd::Init => {
            let path = PathBuf::from(config::CONFIG_NAME);
            if path.exists() {
                eprintln!("{} already exists", config::CONFIG_NAME);
                return ExitCode::from(2);
            }
            if std::fs::write(&path, STARTER).is_err() {
                eprintln!("could not write {}", config::CONFIG_NAME);
                return ExitCode::from(2);
            }
            println!("wrote {} — edit it, then run `stele compile`", config::CONFIG_NAME);
            ExitCode::SUCCESS
        }
        Cmd::Check { quiet_green } => run_check(quiet_green),
        Cmd::Hook { harness, event } => ExitCode::from(hook::run(&harness, &event) as u8),
        Cmd::Wrap { prompt, max_loops, cmd } => ExitCode::from(wrap::run(max_loops, &prompt, &cmd) as u8),
        Cmd::Compile => run_compile(),
        Cmd::Install { harness } => run_install(&harness),
        Cmd::Ack { rule_id, message } => run_ack(&rule_id, &message),
        Cmd::Doctor => ExitCode::from(doctor::run() as u8),
        Cmd::Conformance { harnesses } => ExitCode::from(conformance::run(&harnesses) as u8),
        Cmd::Devin { cmd } => match cmd {
            DevinCmd::Setup => ExitCode::from(devin::setup() as u8),
            DevinCmd::Watch { session_id, max_nudges, poll_secs } => {
                ExitCode::from(devin::watch(&session_id, max_nudges, poll_secs) as u8)
            }
        },
    }
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
            println!("stele: `{}` failing but acknowledged (Stele-Ack trailer)", r.rule.id);
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
            engine::State::new(&sub).mark_green(&sub.signature);
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
    let rules = match config::load(&root) {
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
            println!("cursor headless: use `stele wrap --prompt '<task>' -- cursor-agent -p --force`");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("stele compile: {e}");
            ExitCode::from(2)
        }
    }
}

fn run_install(harness: &str) -> ExitCode {
    if harness != "hermes" {
        eprintln!("stele install: only `hermes` needs per-user install today");
        return ExitCode::from(2);
    }
    let cwd = std::env::current_dir().unwrap_or_default();
    let root = match substrate::find_root(&cwd) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("stele: {e}");
            return ExitCode::from(2);
        }
    };
    match compile::install_hermes(&root) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("stele install hermes: {e}");
            ExitCode::from(2)
        }
    }
}
