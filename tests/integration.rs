//! Engine, config, emit, compile, and end-to-end hook tests.
//! (The live-agent conformance runs stay in conformance/ — these tests cover
//! everything that doesn't need a real model.)

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tempfile::TempDir;

use stele::config::{self, Artifact, Rule, Severity, Trigger};
use stele::emit::{self, Harness};
use stele::engine;
use stele::rules;
use stele::substrate::{self, Substrate};

const RULES_TOML: &str = r###"
[[rule]]
id = "requirements-doc"
description = "every change ships with requirements.md"
severity = "block"

[rule.artifact]
path = "requirements.md"
sections = ["# Requirements", "## Functional", "## Risks"]
"###;

fn sh(dir: &Path, cmd: &str) {
    let status = Command::new("bash")
        .arg("-c")
        .arg(cmd)
        .current_dir(dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "command failed: {cmd}");
}

/// Fresh git repo with an initial commit and a stele.toml.
fn fixture() -> (TempDir, PathBuf) {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    sh(
        &root,
        "git init -q -b main && git config user.email t@t && git config user.name t",
    );
    fs::write(root.join("app.py"), "def add(a, b):\n    return a + b\n").unwrap();
    fs::write(root.join("stele.toml"), RULES_TOML).unwrap();
    sh(&root, "git add -A && git commit -qm init");
    (tmp, root)
}

fn make_substrate(root: &Path) -> Substrate {
    substrate::compute(root).unwrap()
}

// ---------------------------------------------------------------- config

#[test]
fn config_rejects_rule_with_both_check_and_artifact() {
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("stele.toml"),
        r#"
[[rule]]
id = "bad"
check = "true"
[rule.artifact]
path = "x.md"
"#,
    )
    .unwrap();
    assert!(config::load_repo(tmp.path())
        .unwrap_err()
        .contains("exactly one"));
}

#[test]
fn config_rejects_duplicate_ids() {
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("stele.toml"),
        r#"
[[rule]]
id = "a"
check = "true"
[[rule]]
id = "a"
check = "true"
"#,
    )
    .unwrap();
    assert!(config::load_repo(tmp.path())
        .unwrap_err()
        .contains("duplicate"));
}

#[test]
fn config_rejects_scope_on_always_rule() {
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("stele.toml"),
        r#"
[[rule]]
id = "confused"
trigger = "always"
scope = ["src/**"]
check = "true"
"#,
    )
    .unwrap();
    let error = config::load_repo(tmp.path()).unwrap_err();
    assert!(error.contains("cannot be combined"), "{error}");
}

#[test]
fn config_parses_semantic_rule_and_judges() {
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("stele.toml"),
        r#"
[[judge]]
name = "local"
command = "true"

[[rule]]
id = "no-slop"
severity = "nudge"
[rule.semantic]
prompt = "flag slop"
cases = ".stele/evals/no-slop.jsonl"
models = ["local"]
"#,
    )
    .unwrap();
    let rules = config::load_repo(tmp.path()).unwrap();
    let sem = rules[0].semantic.as_ref().expect("semantic parsed");
    assert_eq!(sem.samples, 3, "samples defaults to 3");
    assert_eq!(sem.models, vec!["local"]);
    let judges = config::load_judges(tmp.path(), config::LoadScope::Repository);
    assert_eq!(judges[0].name, "local");
}

#[test]
fn config_rejects_semantic_combined_with_check() {
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("stele.toml"),
        r#"
[[rule]]
id = "bad"
check = "true"
[rule.semantic]
prompt = "x"
cases = "c.jsonl"
models = ["m"]
"#,
    )
    .unwrap();
    assert!(config::load_repo(tmp.path())
        .unwrap_err()
        .contains("exactly one"));
}

#[test]
fn eval_scores_semantic_rule_and_gates_on_the_verdict() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    sh(root, "git init -q -b main");
    fs::create_dir_all(root.join(".stele/evals")).unwrap();
    // One correction: the ticket must go, the code must stay.
    fs::write(
        root.join(".stele/evals/c.jsonl"),
        "{\"id\":\"c1\",\"before\":\"# TODO(ENG-1)\\nx = 1\",\"removed\":[\"ENG-1\"],\"kept\":[\"x = 1\"]}\n",
    )
    .unwrap();
    // Two fixed judges: one returns the surgical fix, one leaves the slop in.
    fs::write(
        root.join("stele.toml"),
        r#"
[[judge]]
name = "good"
command = 'printf "<<<REWRITE\nx = 1\nREWRITE>>>\n"'
[[judge]]
name = "bad"
command = 'printf "<<<REWRITE\n# TODO(ENG-1)\nx = 1\nREWRITE>>>\n"'

[[rule]]
id = "passes"
severity = "block"
[rule.semantic]
prompt = "remove slop"
cases = ".stele/evals/c.jsonl"
models = ["good"]
samples = 1

[[rule]]
id = "fails"
severity = "block"
[rule.semantic]
prompt = "remove slop"
cases = ".stele/evals/c.jsonl"
models = ["bad"]
samples = 1
"#,
    )
    .unwrap();

    let eval = |id: &str| {
        Command::new(env!("CARGO_BIN_EXE_stele"))
            .args(["eval", id])
            .current_dir(root)
            .output()
            .unwrap()
    };
    let good = eval("passes");
    assert_eq!(good.status.code(), Some(0), "a reproduced correction proves the rule");
    assert!(String::from_utf8_lossy(&good.stdout).contains("proven"));
    let bad = eval("fails");
    assert_eq!(bad.status.code(), Some(1), "a rule the judge can't satisfy must gate");
}

// ----------------------------------------------------------------- rules

fn artifact_rule(scope: Vec<String>) -> Rule {
    Rule {
        id: "requirements-doc".into(),
        description: String::new(),
        severity: Severity::Block,
        trigger: Trigger::Changes,
        scope,
        acknowledge: true,
        message: String::new(),
        check: None,
        artifact: Some(Artifact {
            path: "requirements.md".into(),
            sections: vec![
                "# Requirements".into(),
                "## Functional".into(),
                "## Risks".into(),
            ],
            nonempty_sections: false,
        }),
        semantic: None,
    }
}

#[test]
fn artifact_rule_reports_missing_file_then_missing_sections_then_green() {
    let (_tmp, root) = fixture();
    fs::write(
        root.join("app.py"),
        "def add(a, b):\n    return a + b  # changed\n",
    )
    .unwrap();
    let rule = artifact_rule(vec![]);

    let sub = make_substrate(&root);
    let result = rules::evaluate(&rule, &sub);
    assert!(result.red());
    assert!(result.findings[0].contains("missing at repo root"));

    fs::write(root.join("requirements.md"), "# Requirements\n").unwrap();
    let result = rules::evaluate(&rule, &make_substrate(&root));
    assert!(result.red());
    assert_eq!(result.findings.len(), 2); // Functional + Risks

    fs::write(
        root.join("requirements.md"),
        "# Requirements\n## Functional\nx\n## Risks\ny\n",
    )
    .unwrap();
    let result = rules::evaluate(&rule, &make_substrate(&root));
    assert!(!result.red());
}

#[test]
fn scope_gates_rule_triggering() {
    let (_tmp, root) = fixture();
    fs::write(root.join("notes.txt"), "just a note\n").unwrap();

    let py_only = artifact_rule(vec!["**/*.py".into()]);
    let result = rules::evaluate(&py_only, &make_substrate(&root));
    assert!(
        !result.triggered,
        "txt change must not trigger py-scoped rule"
    );

    fs::write(
        root.join("app.py"),
        "def add(a, b):\n    return a + b  # v2\n",
    )
    .unwrap();
    let result = rules::evaluate(&py_only, &make_substrate(&root));
    assert!(result.triggered);
}

#[test]
fn scope_excludes_win() {
    let (_tmp, root) = fixture();
    fs::create_dir_all(root.join("docs")).unwrap();
    fs::write(root.join("docs/guide.py"), "x = 1\n").unwrap();
    let rule = artifact_rule(vec!["**/*.py".into(), "!docs/**".into()]);
    let result = rules::evaluate(&rule, &make_substrate(&root));
    assert!(!result.triggered, "excluded path must not trigger");
}

#[test]
fn always_rule_triggers_on_a_clean_tree() {
    let (_tmp, root) = fixture();
    let mut rule = artifact_rule(vec![]);
    rule.trigger = Trigger::Always;
    let result = rules::evaluate(&rule, &make_substrate(&root));
    assert!(result.triggered);
    assert!(result.red());
}

#[test]
fn command_rule_distinguishes_red_from_unmeasurable() {
    let (_tmp, root) = fixture();
    fs::write(root.join("app.py"), "changed\n").unwrap();
    let sub = make_substrate(&root);

    let red = Rule {
        id: "cmd".into(),
        check: Some("echo '✗ nope'; exit 1".into()),
        artifact: None,
        semantic: None,
        description: String::new(),
        severity: Severity::Block,
        trigger: Trigger::Changes,
        scope: vec![],
        acknowledge: true,
        message: String::new(),
    };
    let result = rules::evaluate(&red, &sub);
    assert!(result.red());
    assert_eq!(result.findings, vec!["✗ nope"]);

    let green = Rule {
        id: "g".into(),
        check: Some("true".into()),
        ..red.clone()
    };
    assert!(!rules::evaluate(&green, &sub).red());
}

// ---------------------------------------------------------------- engine

#[test]
fn block_slots_cap_per_signature_and_reset_on_new_signature() {
    let (_tmp, root) = fixture();
    fs::write(root.join("app.py"), "v2\n").unwrap();
    let sub = make_substrate(&root);
    let state = engine::State::new(&sub);

    assert!(state.take_block_slot("sig-a", 2));
    assert!(state.take_block_slot("sig-a", 2));
    assert!(
        !state.take_block_slot("sig-a", 2),
        "third block must be denied"
    );
    assert!(
        state.take_block_slot("sig-b", 2),
        "new signature resets the count"
    );
}

#[test]
fn green_cache_round_trips() {
    let (_tmp, root) = fixture();
    let sub = make_substrate(&root);
    let state = engine::State::new(&sub);
    assert!(!state.is_green_cached("s1"));
    state.mark_green("s1");
    assert!(state.is_green_cached("s1"));
    assert!(!state.is_green_cached("s2"));
}

#[test]
fn global_and_repo_hook_noise_state_are_independent() {
    let (_tmp, root) = fixture();
    let sub = make_substrate(&root);
    let global = engine::State::scoped(&sub, "global");
    let repo = engine::State::scoped(&sub, "repo");
    assert!(global.take_prompt_slot("same-tree"));
    assert!(repo.take_prompt_slot("same-tree"));
    assert!(!global.take_prompt_slot("same-tree"));
    assert!(!repo.take_prompt_slot("same-tree"));
}

#[test]
fn policy_signature_moves_when_external_rule_definition_moves() {
    let (_tmp, root) = fixture();
    let sub = make_substrate(&root);
    let mut rule = artifact_rule(vec![]);
    let first = engine::policy_signature(&[rule.clone()], &sub.signature);
    rule.description = "new personal policy text".into();
    let second = engine::policy_signature(&[rule], &sub.signature);
    assert_ne!(first, second);
}

// ------------------------------------------------------------------ emit

#[test]
fn emit_protocols_match_validated_contracts() {
    let reason = "fix it";
    for h in [Harness::ClaudeCode, Harness::Codex, Harness::DevinCli] {
        let v: serde_json::Value = serde_json::from_str(&emit::stop_block(h, reason)).unwrap();
        assert_eq!(v["decision"], "block");
        assert_eq!(v["reason"], reason);
    }
    let v: serde_json::Value =
        serde_json::from_str(&emit::stop_block(Harness::Cursor, reason)).unwrap();
    assert_eq!(v["followup_message"], reason);
    let v: serde_json::Value =
        serde_json::from_str(&emit::tool_block(Harness::Hermes, reason)).unwrap();
    assert_eq!(v["action"], "block");
    assert_eq!(v["message"], reason);
    assert_eq!(emit::tool_allow(Harness::Hermes).unwrap(), "{}");
    let v: serde_json::Value = serde_json::from_str(&emit::lifecycle_context(
        Harness::Cursor,
        "session-start",
        reason,
    ))
    .unwrap();
    assert_eq!(v["additional_context"], reason);
}

// --------------------------------------------------------------- compile

#[test]
fn compile_writes_all_channels_and_is_idempotent() {
    let (_tmp, root) = fixture();
    let rules = config::load_repo(&root).unwrap();

    let written = stele::compile::run(&root, &rules).unwrap();
    for expected in [
        ".claude/settings.json",
        ".codex/hooks.json",
        ".devin/hooks.v1.json",
        ".cursor/hooks.json",
        ".stele/hermes-shim.sh",
        ".git/hooks/pre-push",
        ".github/workflows/stele.yml",
        "AGENTS.md",
    ] {
        assert!(
            written.iter().any(|w| w.ends_with(expected)),
            "missing {expected} in {written:?}"
        );
    }

    let again = stele::compile::run(&root, &rules).unwrap();
    assert!(
        again.is_empty(),
        "second compile must be a no-op, wrote {again:?}"
    );
    // CI is the unbypassable terminus: it must NOT tolerate a missing binary
    // the way the local channels do.
    let ci = fs::read_to_string(root.join(".github/workflows/stele.yml")).unwrap();
    assert!(!ci.contains("command -v"), "CI must fail loud: {ci}");
    let codex = fs::read_to_string(root.join(".codex/hooks.json")).unwrap();
    assert!(codex.contains("session-start --scope repo"), "{codex}");
    assert!(codex.contains("pre-tool-use --scope repo"), "{codex}");
    // The repo pre-push evaluates only repository rules; personal/system rules
    // gate through their own global hooks, not this repository's wiring.
    let pre_push = fs::read_to_string(root.join(".git/hooks/pre-push")).unwrap();
    assert!(
        pre_push.contains("stele check --quiet-green --scope repo"),
        "{pre_push}"
    );
}

/// Generated hooks are committed but the binary is not. A teammate without
/// stele installed must get silence, not a hook failure on every event — that
/// is how a team decides to delete the wiring. CI is exempt: it fails loud.
#[test]
fn generated_hooks_are_silent_no_ops_when_stele_is_not_installed() {
    let (_tmp, root) = fixture();
    let rules = config::load_repo(&root).unwrap();
    stele::compile::run(&root, &rules).unwrap();

    // An empty PATH is the "not installed" case for every generated channel.
    // `bash` is invoked by absolute path so the harness itself still resolves.
    let run_isolated = |script: &str| -> std::process::Output {
        Command::new("/bin/bash")
            .arg("-c")
            .arg(script)
            .current_dir(&root)
            .env("PATH", "")
            .output()
            .unwrap()
    };

    let claude: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(root.join(".claude/settings.json")).unwrap())
            .unwrap();
    let stop = claude["hooks"]["Stop"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap();
    let out = run_isolated(stop);
    assert!(
        out.status.success(),
        "hook must exit 0 without stele: {:?}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.stdout.is_empty(),
        "silence is allow: {:?}",
        String::from_utf8_lossy(&out.stdout)
    );

    // Pre-push must let the push through; CI still holds the line.
    let out = run_isolated("/bin/bash .git/hooks/pre-push");
    assert!(out.status.success(), "pre-push must not block without stele");

    // Hermes reads empty stdout as undefined, so its shim owes an explicit
    // allow even when stele is absent.
    let out = run_isolated("/bin/bash .stele/hermes-shim.sh");
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "{}",
        "hermes needs an explicit allow"
    );
}

/// `binary` lets a project ship stele inside its own toolchain (a pnpm
/// devDependency, a vendored release) instead of depending on every
/// teammate's PATH.
#[test]
fn configured_binary_is_used_by_every_generated_channel() {
    let (_tmp, root) = fixture();
    let existing = fs::read_to_string(root.join("stele.toml")).unwrap();
    fs::write(
        root.join("stele.toml"),
        format!("binary = \"node_modules/.bin/stele\"\n{existing}"),
    )
    .unwrap();
    let rules = config::load_repo(&root).unwrap();
    stele::compile::run(&root, &rules).unwrap();

    for channel in [
        ".claude/settings.json",
        ".codex/hooks.json",
        ".cursor/hooks.json",
        ".stele/hermes-shim.sh",
        ".git/hooks/pre-push",
    ] {
        let body = fs::read_to_string(root.join(channel)).unwrap();
        assert!(
            body.contains("node_modules/.bin/stele"),
            "{channel} ignored the configured binary: {body}"
        );
    }

    // Ownership is tracked by the argument tail, so a repo that switches
    // binaries replaces its hooks rather than accumulating a second set.
    fs::write(
        root.join("stele.toml"),
        format!("binary = \"vendor/stele\"\n{existing}"),
    )
    .unwrap();
    stele::compile::run(&root, &rules).unwrap();
    let claude: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(root.join(".claude/settings.json")).unwrap())
            .unwrap();
    let stops = claude["hooks"]["Stop"].as_array().unwrap();
    assert_eq!(stops.len(), 1, "switching binary must replace, not append");
    assert!(stops[0]["hooks"][0]["command"]
        .as_str()
        .unwrap()
        .contains("vendor/stele"));
}

#[test]
fn check_scope_repo_ignores_personal_rules_that_full_check_reports() {
    let (_tmp, root) = fixture(); // repo stele.toml has a passing artifact rule
                                  // A failing personal (user-layer) rule that would gate a full `stele check`.
                                  // Kept outside the repo so it doesn't dirty the tree and trip a repo rule.
    let cfg = TempDir::new().unwrap();
    let user = cfg.path().join("user-stele.toml");
    fs::write(
        &user,
        "[[rule]]\nid = \"personal-fail\"\nseverity = \"block\"\ntrigger = \"always\"\ncheck = \"exit 1\"\n",
    )
    .unwrap();
    let missing_system = cfg.path().join("missing-system.toml");
    let run = |scope: &str| {
        Command::new(stele_bin())
            .args(["check", "--scope", scope])
            .current_dir(&root)
            .env(config::USER_CONFIG_ENV, &user)
            .env(config::SYSTEM_CONFIG_ENV, &missing_system)
            .env_remove("CLAUDE_PROJECT_DIR")
            .output()
            .unwrap()
    };
    // Full check sees the personal rule and fails; repo-scoped ignores it.
    assert_eq!(run("all").status.code(), Some(1));
    assert_eq!(
        run("repo").status.code(),
        Some(0),
        "repo scope must not evaluate the personal layer"
    );
}

#[test]
fn compile_preserves_existing_user_hooks() {
    let (_tmp, root) = fixture();
    fs::create_dir_all(root.join(".claude")).unwrap();
    fs::write(
        root.join(".claude/settings.json"),
        r#"{"permissions":{"allow":["Bash(ls)"]},"hooks":{"Stop":[{"matcher":"","hooks":[{"type":"command","command":"my-own-hook.sh"}]}]}}"#,
    )
    .unwrap();
    let rules = config::load_repo(&root).unwrap();
    stele::compile::run(&root, &rules).unwrap();

    let doc: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(root.join(".claude/settings.json")).unwrap())
            .unwrap();
    assert_eq!(
        doc["permissions"]["allow"][0], "Bash(ls)",
        "user config preserved"
    );
    let stops = doc["hooks"]["Stop"].as_array().unwrap();
    assert_eq!(stops.len(), 2, "user hook + stele hook");
    assert_eq!(stops[0]["hooks"][0]["command"], "my-own-hook.sh");
}

#[test]
fn compile_refuses_to_clobber_foreign_pre_push() {
    let (_tmp, root) = fixture();
    fs::create_dir_all(root.join(".git/hooks")).unwrap();
    fs::write(root.join(".git/hooks/pre-push"), "#!/bin/sh\necho mine\n").unwrap();
    let rules = config::load_repo(&root).unwrap();
    stele::compile::run(&root, &rules).unwrap();
    let body = fs::read_to_string(root.join(".git/hooks/pre-push")).unwrap();
    assert_eq!(
        body, "#!/bin/sh\necho mine\n",
        "foreign hook must be untouched"
    );
}

#[test]
fn agents_md_managed_block_updates_in_place() {
    let (_tmp, root) = fixture();
    fs::write(root.join("AGENTS.md"), "# My repo\n\nHand-written intro.\n").unwrap();
    let rules = config::load_repo(&root).unwrap();
    stele::compile::run(&root, &rules).unwrap();
    stele::compile::run(&root, &rules).unwrap();
    let body = fs::read_to_string(root.join("AGENTS.md")).unwrap();
    assert!(body.contains("Hand-written intro."));
    assert_eq!(
        body.matches("stele:begin").count(),
        1,
        "block must not duplicate"
    );
    assert!(body.contains("requirements-doc"));
}

#[test]
fn global_install_preserves_user_config_migrates_old_hooks_and_is_idempotent() {
    let home = TempDir::new().unwrap();
    fs::create_dir_all(home.path().join(".claude")).unwrap();
    fs::create_dir_all(home.path().join(".hermes")).unwrap();
    fs::write(
        home.path().join(".claude/settings.json"),
        r#"{
  "permissions": {"allow": ["Bash(ls)"]},
  "hooks": {
    "Stop": [{
      "matcher": "",
      "hooks": [{"type": "command", "command": "stele hook claude-code stop"}]
    }]
  }
}"#,
    )
    .unwrap();
    fs::write(
        home.path().join(".hermes/config.yaml"),
        "model: test\nhooks:\n  pre_tool_call:\n    - command: \"/usr/local/bin/my-hook\"\n      timeout: 10\n  post_tool_call:\n    - command: \"/usr/local/bin/after\"\n",
    )
    .unwrap();

    let written = stele::compile::install_global_at(home.path()).unwrap();
    for expected in [
        ".claude/settings.json",
        ".codex/hooks.json",
        ".cursor/hooks.json",
    ] {
        assert!(
            written.iter().any(|path| path.ends_with(expected)),
            "missing {expected}: {written:?}"
        );
    }
    assert!(
        stele::compile::install_global_at(home.path())
            .unwrap()
            .is_empty(),
        "second install must be a no-op"
    );

    let claude: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(home.path().join(".claude/settings.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(claude["permissions"]["allow"][0], "Bash(ls)");
    let stops = claude["hooks"]["Stop"].as_array().unwrap();
    assert_eq!(stops.len(), 1, "stale Stele hook must be replaced");
    assert_eq!(
        stops[0]["hooks"][0]["command"],
        "command -v stele >/dev/null 2>&1 || exit 0; stele hook claude-code stop --scope global"
    );
    assert_eq!(claude["hooks"]["SessionStart"].as_array().unwrap().len(), 1);
    assert_eq!(claude["hooks"]["PreToolUse"].as_array().unwrap().len(), 1);

    let codex = fs::read_to_string(home.path().join(".codex/hooks.json")).unwrap();
    assert!(codex.contains("session-start --scope global"), "{codex}");
    let cursor = fs::read_to_string(home.path().join(".cursor/hooks.json")).unwrap();
    assert!(cursor.contains("pre-tool-use --scope global"), "{cursor}");
    let hermes = fs::read_to_string(home.path().join(".hermes/config.yaml")).unwrap();
    assert!(hermes.contains("/usr/local/bin/my-hook"), "{hermes}");
    assert!(hermes.contains("/usr/local/bin/after"), "{hermes}");
    assert!(hermes.contains("hermes-shim.sh pre_tool_call"), "{hermes}");
    assert!(home.path().join(".config/stele/hermes-shim.sh").is_file());
}

#[test]
fn global_uninstall_removes_only_stele_owned_wiring_and_is_idempotent() {
    let home = TempDir::new().unwrap();
    fs::create_dir_all(home.path().join(".config/stele")).unwrap();
    fs::create_dir_all(home.path().join(".claude")).unwrap();
    fs::create_dir_all(home.path().join(".hermes")).unwrap();
    let personal = home.path().join(".config/stele/stele.toml");
    fs::write(
        &personal,
        "[[rule]]\nid = \"mine\"\ntrigger = \"always\"\ncheck = \"true\"\n",
    )
    .unwrap();
    fs::write(
        home.path().join(".claude/settings.json"),
        r#"{"permissions":{"allow":["Bash(ls)"]},"hooks":{"Stop":[{"matcher":"","hooks":[{"type":"command","command":"my stop hook"}]}]}}"#,
    )
    .unwrap();
    fs::write(
        home.path().join(".hermes/config.yaml"),
        "model: test\nhooks:\n  pre_tool_call:\n    - command: \"/usr/local/bin/my-hook\"\n      timeout: 10\n",
    )
    .unwrap();

    stele::compile::install_global_at(home.path()).unwrap();
    let changed = stele::compile::uninstall_global_at(home.path()).unwrap();
    assert!(!changed.is_empty());
    assert!(
        stele::compile::uninstall_global_at(home.path())
            .unwrap()
            .is_empty(),
        "second uninstall must be a no-op"
    );

    let claude = fs::read_to_string(home.path().join(".claude/settings.json")).unwrap();
    assert!(claude.contains("Bash(ls)"), "{claude}");
    assert!(claude.contains("my stop hook"), "{claude}");
    assert!(!claude.contains("stele hook"), "{claude}");
    let codex = fs::read_to_string(home.path().join(".codex/hooks.json")).unwrap();
    assert!(!codex.contains("stele hook"), "{codex}");
    let cursor = fs::read_to_string(home.path().join(".cursor/hooks.json")).unwrap();
    assert!(!cursor.contains("stele hook"), "{cursor}");
    let hermes = fs::read_to_string(home.path().join(".hermes/config.yaml")).unwrap();
    assert!(hermes.contains("/usr/local/bin/my-hook"), "{hermes}");
    assert!(!hermes.contains("hermes-shim"), "{hermes}");
    assert!(!home.path().join(".config/stele/hermes-shim.sh").exists());
    assert!(personal.is_file(), "personal rules are retained by default");
}

#[test]
fn global_install_refuses_to_clobber_invalid_user_json() {
    let home = TempDir::new().unwrap();
    fs::create_dir_all(home.path().join(".codex")).unwrap();
    let path = home.path().join(".codex/hooks.json");
    fs::write(&path, "{ definitely not json\n").unwrap();
    let error = stele::compile::install_global_at(home.path()).unwrap_err();
    assert!(error.contains("not valid JSON"), "{error}");
    assert_eq!(
        fs::read_to_string(path).unwrap(),
        "{ definitely not json\n",
        "invalid user config must remain byte-for-byte untouched"
    );
    assert!(!home.path().join(".claude/settings.json").exists());
}

#[test]
fn global_hook_install_accepts_system_rules_without_personal_config() {
    let home = TempDir::new().unwrap();
    let system = home.path().join("system-stele.toml");
    let missing_user = home.path().join("missing-user.toml");
    fs::write(
        &system,
        "[[rule]]\nid = \"system-rule\"\ntrigger = \"always\"\ncheck = \"true\"\n",
    )
    .unwrap();
    let out = Command::new(stele_bin())
        .args(["install", "global", "--yes"])
        .current_dir(home.path())
        .env("HOME", home.path())
        .env(config::USER_CONFIG_ENV, &missing_user)
        .env(config::SYSTEM_CONFIG_ENV, &system)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(home.path().join(".codex/hooks.json").is_file());
}

#[test]
fn global_install_bootstraps_personal_rules_and_uninstall_can_purge_them() {
    let home = TempDir::new().unwrap();
    let user = home.path().join("config/stele/stele.toml");
    let system = home.path().join("missing-system.toml");
    let install = Command::new(stele_bin())
        .args(["install", "global", "--yes"])
        .current_dir(home.path())
        .env("HOME", home.path())
        .env(config::USER_CONFIG_ENV, &user)
        .env(config::SYSTEM_CONFIG_ENV, &system)
        .output()
        .unwrap();
    assert!(
        install.status.success(),
        "{}",
        String::from_utf8_lossy(&install.stderr)
    );
    let personal = fs::read_to_string(&user).unwrap();
    assert!(personal.contains("personal-worktree-only"), "{personal}");
    assert!(home.path().join(".codex/hooks.json").is_file());

    let uninstall = Command::new(stele_bin())
        .args(["uninstall", "global", "--purge"])
        .current_dir(home.path())
        .env("HOME", home.path())
        .env(config::USER_CONFIG_ENV, &user)
        .env(config::SYSTEM_CONFIG_ENV, &system)
        .output()
        .unwrap();
    assert!(
        uninstall.status.success(),
        "{}",
        String::from_utf8_lossy(&uninstall.stderr)
    );
    assert!(!user.exists());
    let codex = fs::read_to_string(home.path().join(".codex/hooks.json")).unwrap();
    assert!(!codex.contains("stele hook"), "{codex}");
}

#[test]
fn install_global_refuses_inside_an_agent_session_unless_forced() {
    let home = TempDir::new().unwrap();
    let user = home.path().join("config/stele/stele.toml");
    let system = home.path().join("missing-system.toml");

    // Inside an agent session (CLAUDECODE set), refuse: wiring global hooks
    // would gate this very session on its next tool call.
    let refused = Command::new(stele_bin())
        .args(["install", "global"])
        .current_dir(home.path())
        .env("HOME", home.path())
        .env("CLAUDECODE", "1")
        .env(config::USER_CONFIG_ENV, &user)
        .env(config::SYSTEM_CONFIG_ENV, &system)
        .output()
        .unwrap();
    assert_eq!(refused.status.code(), Some(3));
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("refusing"),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );
    assert!(!user.exists(), "must not write config when refusing");

    // --yes overrides and wires it, and the blast-radius note is always shown.
    let forced = Command::new(stele_bin())
        .args(["install", "global", "--yes"])
        .current_dir(home.path())
        .env("HOME", home.path())
        .env("CLAUDECODE", "1")
        .env(config::USER_CONFIG_ENV, &user)
        .env(config::SYSTEM_CONFIG_ENV, &system)
        .output()
        .unwrap();
    assert!(
        forced.status.success(),
        "{}",
        String::from_utf8_lossy(&forced.stderr)
    );
    assert!(user.exists());
    assert!(
        String::from_utf8_lossy(&forced.stdout).contains("every git repo"),
        "{}",
        String::from_utf8_lossy(&forced.stdout)
    );
}

#[test]
fn global_install_rolls_back_bootstrap_when_user_hooks_are_invalid() {
    let home = TempDir::new().unwrap();
    fs::create_dir_all(home.path().join(".codex")).unwrap();
    let codex = home.path().join(".codex/hooks.json");
    fs::write(&codex, "{ invalid\n").unwrap();
    let user = home.path().join("config/stele/stele.toml");
    let system = home.path().join("missing-system.toml");
    let install = Command::new(stele_bin())
        .args(["install", "global", "--yes"])
        .current_dir(home.path())
        .env("HOME", home.path())
        .env(config::USER_CONFIG_ENV, &user)
        .env(config::SYSTEM_CONFIG_ENV, &system)
        .output()
        .unwrap();
    assert_eq!(install.status.code(), Some(2));
    assert!(!user.exists(), "failed bootstrap must be rolled back");
    assert_eq!(fs::read_to_string(codex).unwrap(), "{ invalid\n");
    assert!(!home.path().join(".claude/settings.json").exists());
}

#[test]
fn hermes_global_install_uses_stable_personal_shim() {
    let home = TempDir::new().unwrap();
    fs::create_dir_all(home.path().join(".hermes")).unwrap();
    fs::write(
        home.path().join(".hermes/config.yaml"),
        "model: test\nhooks: {}\n",
    )
    .unwrap();
    let state_dir = home.path().join("personal-stele");
    stele::compile::install_hermes_at(home.path(), &state_dir).unwrap();
    stele::compile::install_hermes_at(home.path(), &state_dir).unwrap();

    let config = fs::read_to_string(home.path().join(".hermes/config.yaml")).unwrap();
    assert!(config.contains(&state_dir.join("hermes-shim.sh").display().to_string()));
    assert_eq!(config.matches("pre_tool_call:").count(), 1, "{config}");
    let shim = fs::read_to_string(state_dir.join("hermes-shim.sh")).unwrap();
    assert!(shim.contains("--scope all"), "{shim}");
    assert!(!shim.contains("$cwd/stele.toml"), "{shim}");
    // The shim must carry no undeclared runtime dependencies: stele reads `cwd`
    // from the payload and self-scopes, so there is no in-shim JSON parsing.
    assert!(!shim.contains("python"), "{shim}");
    assert!(
        shim.trim_end()
            .ends_with("exec stele hook hermes pre_tool_call --scope all"),
        "{shim}"
    );

    let old_home = TempDir::new().unwrap();
    fs::create_dir_all(old_home.path().join(".hermes")).unwrap();
    fs::write(
        old_home.path().join(".hermes/config.yaml"),
        "hooks:\n  pre_tool_call:\n    - command: \"/tmp/repo/.stele/hermes-shim.sh pre_tool_call\"\n      timeout: 60\n",
    )
    .unwrap();
    let migrated_state = old_home.path().join("personal-stele");
    stele::compile::install_hermes_at(old_home.path(), &migrated_state).unwrap();
    let migrated = fs::read_to_string(old_home.path().join(".hermes/config.yaml")).unwrap();
    assert!(migrated.contains(&migrated_state.display().to_string()));
    assert!(!migrated.contains("/tmp/repo/.stele"), "{migrated}");
}

// ------------------------------------------------------- end-to-end hook

fn stele_bin() -> &'static str {
    env!("CARGO_BIN_EXE_stele")
}

fn stele_command(root: &Path) -> Command {
    let mut command = Command::new(stele_bin());
    command
        .current_dir(root)
        .env(config::DISABLE_GLOBAL_ENV, "1");
    command
}

fn global_stele_command(root: &Path, user_config: &Path, system_config: &Path) -> Command {
    let mut command = Command::new(stele_bin());
    command
        .current_dir(root)
        .env_remove(config::DISABLE_GLOBAL_ENV)
        .env(config::USER_CONFIG_ENV, user_config)
        .env(config::SYSTEM_CONFIG_ENV, system_config);
    command
}

fn run_global_hook(
    root: &Path,
    user_config: &Path,
    harness: &str,
    event: &str,
    payload: &str,
) -> (String, i32) {
    let missing_system = user_config.with_extension("missing-system.toml");
    let mut child = global_stele_command(root, user_config, &missing_system)
        .args(["hook", harness, event, "--scope", "global"])
        .env_remove("CLAUDE_PROJECT_DIR")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    use std::io::Write;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

fn run_hook(root: &Path, harness: &str, event: &str, payload: &str) -> (String, i32) {
    let mut child = stele_command(root)
        .args(["hook", harness, event, "--scope", "repo"])
        .env_remove("CLAUDE_PROJECT_DIR")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    use std::io::Write;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn hook_stop_blocks_then_gives_up_then_goes_green() {
    let (_tmp, root) = fixture();
    fs::write(
        root.join("app.py"),
        "def add(a, b):\n    return a + b  # touched\n",
    )
    .unwrap();

    // Block 1 and 2: same signature, default cap 2.
    let (out, code) = run_hook(&root, "claude-code", "stop", "{}");
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["decision"], "block");
    assert!(v["reason"].as_str().unwrap().contains("requirements.md"));

    let (out, _) = run_hook(&root, "claude-code", "stop", "{}");
    assert!(out.contains("block"), "second block within cap");

    // Third stop, same signature: gave up — silence (fail-open).
    let (out, code) = run_hook(&root, "claude-code", "stop", "{}");
    assert_eq!((out.trim(), code), ("", 0));

    // Loop guard: retry payload must always be silent.
    let (out, _) = run_hook(
        &root,
        "claude-code",
        "stop",
        r#"{"stop_hook_active": true}"#,
    );
    assert_eq!(out.trim(), "");

    // Agent complies → green, silent, and cached.
    fs::write(
        root.join("requirements.md"),
        "# Requirements\n## Functional\nx\n## Risks\ny\n",
    )
    .unwrap();
    let (out, code) = run_hook(&root, "claude-code", "stop", "{}");
    assert_eq!((out.trim(), code), ("", 0));
    let (out, _) = run_hook(&root, "claude-code", "stop", "{}");
    assert_eq!(out.trim(), "", "green cache keeps silence free");
}

#[test]
fn hook_is_silent_outside_stele_repos() {
    let tmp = TempDir::new().unwrap();
    sh(tmp.path(), "git init -q");
    let (out, code) = run_hook(tmp.path(), "claude-code", "stop", "{}");
    assert_eq!((out.trim(), code), ("", 0));
}

#[test]
fn hook_cursor_emits_followup_message() {
    let (_tmp, root) = fixture();
    fs::write(root.join("app.py"), "v2\n").unwrap();
    let payload = format!(r#"{{"workspace_roots": ["{}"]}}"#, root.display());
    let (out, _) = run_hook(&root, "cursor", "stop", &payload);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert!(v["followup_message"]
        .as_str()
        .unwrap()
        .contains("requirements.md"));

    // Cursor loop guard.
    let payload = format!(
        r#"{{"workspace_roots": ["{}"], "loop_count": 1}}"#,
        root.display()
    );
    let (out, _) = run_hook(&root, "cursor", "stop", &payload);
    assert_eq!(out.trim(), "");
}

#[test]
fn hook_hermes_gatekeeper_allows_remediation_blocks_the_rest() {
    let (_tmp, root) = fixture();
    fs::write(root.join("app.py"), "v2\n").unwrap();
    let base = format!(r#""cwd": "{}""#, root.display());

    // Mutating tools are gated while red; read-only tools always pass.
    let blocked = format!(
        r#"{{{base}, "tool_name": "terminal", "tool_input": {{"command": "touch app.py"}}}}"#
    );
    let (out, _) = run_hook(&root, "hermes", "pre_tool_call", &blocked);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["action"], "block");
    let readonly =
        format!(r#"{{{base}, "tool_name": "read_file", "tool_input": {{"path": "app.py"}}}}"#);
    let (out, _) = run_hook(&root, "hermes", "pre_tool_call", &readonly);
    assert_eq!(out.trim(), "{}");

    let remediation = format!(
        r#"{{{base}, "tool_name": "write_file", "tool_input": {{"path": "requirements.md"}}}}"#
    );
    let (out, _) = run_hook(&root, "hermes", "pre_tool_call", &remediation);
    assert_eq!(out.trim(), "{}", "artifact write must be allowed through");
}

// ------------------------------------------------------------- check CLI

#[test]
fn check_exit_codes_distinguish_green_red_unmeasurable() {
    let (_tmp, root) = fixture();

    // Nothing changed: green (no rules trigger).
    let ok = stele_command(&root).arg("check").output().unwrap();
    assert_eq!(ok.status.code(), Some(0));

    fs::write(root.join("app.py"), "v2\n").unwrap();
    let red = stele_command(&root).arg("check").output().unwrap();
    assert_eq!(red.status.code(), Some(1));

    // A check that cannot run is exit 3, not green.
    fs::write(
        root.join("stele.toml"),
        r#"
[[rule]]
id = "broken"
check = "/nonexistent/checker"
"#,
    )
    .unwrap();
    let unmeasurable = stele_command(&root).arg("check").output().unwrap();
    assert_eq!(
        unmeasurable.status.code(),
        Some(1),
        "bash exits 127 → findings"
    );
}

#[test]
fn personal_worktree_starter_is_an_advisory_nudge_that_passes_in_a_worktree() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("repo");
    fs::create_dir_all(&root).unwrap();
    sh(
        &root,
        "git init -q -b main && git config user.email t@t && git config user.name t",
    );
    fs::write(root.join("app.py"), "x = 1\n").unwrap();
    sh(&root, "git add -A && git commit -qm init");

    let user_config = tmp.path().join("config/stele/stele.toml");
    let system_config = tmp.path().join("missing-system.toml");
    let init = global_stele_command(&root, &user_config, &system_config)
        .args(["init", "--global"])
        .output()
        .unwrap();
    assert!(
        init.status.success(),
        "{}",
        String::from_utf8_lossy(&init.stderr)
    );
    // The shipped default is a nudge — installing it can never freeze an agent.
    let personal = fs::read_to_string(&user_config).unwrap();
    assert!(personal.contains("severity = \"nudge\""), "{personal}");
    assert!(personal.contains("personal-worktree-only"), "{personal}");

    // A clean primary checkout surfaces the advisory but stays green (exit 0):
    // a nudge informs, it never fails.
    let check = global_stele_command(&root, &user_config, &system_config)
        .arg("check")
        .output()
        .unwrap();
    assert_eq!(
        check.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&check.stdout)
    );
    assert!(
        String::from_utf8_lossy(&check.stdout).contains("primary checkout"),
        "{}",
        String::from_utf8_lossy(&check.stdout)
    );

    // SessionStart injects the reminder as context...
    let session = format!(r#"{{"cwd":"{}"}}"#, root.display());
    let (out, code) = run_global_hook(&root, &user_config, "codex", "session-start", &session);
    assert_eq!(code, 0);
    assert!(out.contains("personal-worktree-only"), "{out}");

    // ...but the first mutating tool is NOT gated: a nudge never blocks.
    let mutation = format!(
        r#"{{"cwd":"{}","tool_name":"apply_patch","tool_input":{{"patch":"x"}}}}"#,
        root.display()
    );
    let (out, _) = run_global_hook(&root, &user_config, "codex", "pre-tool-use", &mutation);
    assert_eq!(out.trim(), "", "nudge must not gate tool calls: {out}");

    // The exact same personal config goes green with no advisory in a worktree.
    let linked = tmp.path().join("agent-worktree");
    let status = Command::new("git")
        .args(["worktree", "add", "-q", "-b", "agent-work"])
        .arg(&linked)
        .current_dir(&root)
        .status()
        .unwrap();
    assert!(status.success());
    let check = global_stele_command(&linked, &user_config, &system_config)
        .arg("check")
        .output()
        .unwrap();
    assert!(
        check.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&check.stdout),
        String::from_utf8_lossy(&check.stderr)
    );
}

// A user who wants the worktree invariant *enforced* flips severity to block.
// This preserves coverage of the preflight gate that `stele run` relies on.
const BLOCK_WORKTREE_RULE: &str = r###"
[[rule]]
id = "worktree-only"
description = "agents always work in linked git worktrees"
severity = "block"
trigger = "always"
acknowledge = false
message = "Launch with `stele run <agent>`."
check = '''
git_dir=$(git rev-parse --path-format=absolute --git-dir 2>/dev/null) || exit 1
common_dir=$(git rev-parse --path-format=absolute --git-common-dir 2>/dev/null) || exit 1
[ "$git_dir" != "$common_dir" ] || {
  echo '✗ agent session is running in the primary checkout, not a linked worktree'
  exit 1
}
'''
"###;

#[test]
fn block_severity_worktree_rule_gates_tools_ack_and_wrap_preflight() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("repo");
    fs::create_dir_all(&root).unwrap();
    sh(
        &root,
        "git init -q -b main && git config user.email t@t && git config user.name t",
    );
    fs::write(root.join("app.py"), "x = 1\n").unwrap();
    sh(&root, "git add -A && git commit -qm init");

    let user_config = tmp.path().join("config/stele/stele.toml");
    fs::create_dir_all(user_config.parent().unwrap()).unwrap();
    fs::write(&user_config, BLOCK_WORKTREE_RULE).unwrap();
    let system_config = tmp.path().join("missing-system.toml");

    // Primary checkout is red (exit 1) with the finding.
    let check = global_stele_command(&root, &user_config, &system_config)
        .arg("check")
        .output()
        .unwrap();
    assert_eq!(check.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&check.stdout).contains("primary checkout"),
        "{}",
        String::from_utf8_lossy(&check.stdout)
    );

    // The first mutating tool is denied; read-only exploration passes.
    let mutation = format!(
        r#"{{"cwd":"{}","tool_name":"apply_patch","tool_input":{{"patch":"x"}}}}"#,
        root.display()
    );
    let (out, _) = run_global_hook(&root, &user_config, "codex", "pre-tool-use", &mutation);
    let denial: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(denial["hookSpecificOutput"]["permissionDecision"], "deny");
    let read = format!(
        r#"{{"cwd":"{}","tool_name":"Read","tool_input":{{"path":"app.py"}}}}"#,
        root.display()
    );
    let (out, _) = run_global_hook(&root, &user_config, "codex", "pre-tool-use", &read);
    assert_eq!(out.trim(), "", "read-only exploration remains available");

    // Non-acknowledgeable.
    let ack = global_stele_command(&root, &user_config, &system_config)
        .args(["ack", "worktree-only", "-m", "skip it"])
        .output()
        .unwrap();
    assert_eq!(ack.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&ack.stderr).contains("does not allow"));

    // Hook-less harnesses are preflight-checked before the agent process starts.
    let wrapped = global_stele_command(&root, &user_config, &system_config)
        .args([
            "wrap",
            "--prompt",
            "edit app.py",
            "--",
            "/definitely/not/an/agent",
        ])
        .output()
        .unwrap();
    assert_eq!(wrapped.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&wrapped.stderr);
    assert!(stderr.contains("session preflight failed"), "{stderr}");
    assert!(!stderr.contains("failed to run agent"), "{stderr}");

    // The exact same config goes green in a linked worktree.
    let linked = tmp.path().join("agent-worktree");
    let status = Command::new("git")
        .args(["worktree", "add", "-q", "-b", "agent-work"])
        .arg(&linked)
        .current_dir(&root)
        .status()
        .unwrap();
    assert!(status.success());
    let check = global_stele_command(&linked, &user_config, &system_config)
        .arg("check")
        .output()
        .unwrap();
    assert!(
        check.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&check.stdout),
        String::from_utf8_lossy(&check.stderr)
    );
}

#[test]
fn stele_run_creates_and_reuses_a_managed_linked_worktree() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("repo");
    fs::create_dir_all(&root).unwrap();
    sh(
        &root,
        "git init -q -b main && git config user.email t@t && git config user.name t",
    );
    fs::write(root.join("app.py"), "x = 1\n").unwrap();
    sh(&root, "git add -A && git commit -qm init");

    let user_config = tmp.path().join("config/stele/stele.toml");
    let system_config = tmp.path().join("missing-system.toml");
    let state_home = tmp.path().join("state");
    let init = global_stele_command(&root, &user_config, &system_config)
        .args(["init", "--global"])
        .output()
        .unwrap();
    assert!(init.status.success());

    let launched = global_stele_command(&root, &user_config, &system_config)
        .args(["run", "--name", "dogfood", "pwd"])
        .env(stele::launch::STATE_HOME_ENV, &state_home)
        .output()
        .unwrap();
    assert!(
        launched.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&launched.stdout),
        String::from_utf8_lossy(&launched.stderr)
    );
    let worktree = PathBuf::from(String::from_utf8_lossy(&launched.stdout).trim());
    assert!(worktree.is_dir(), "{}", worktree.display());
    assert_ne!(
        fs::canonicalize(&worktree).unwrap(),
        fs::canonicalize(&root).unwrap()
    );
    let git_dir = Command::new("git")
        .args(["rev-parse", "--path-format=absolute", "--git-dir"])
        .current_dir(&worktree)
        .output()
        .unwrap();
    let common_dir = Command::new("git")
        .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .current_dir(&worktree)
        .output()
        .unwrap();
    assert_ne!(
        git_dir.stdout, common_dir.stdout,
        "must be a linked worktree"
    );

    let reused = global_stele_command(&worktree, &user_config, &system_config)
        .args(["run", "pwd"])
        .env(stele::launch::STATE_HOME_ENV, &state_home)
        .output()
        .unwrap();
    assert!(reused.status.success());
    assert_eq!(
        fs::canonicalize(String::from_utf8_lossy(&reused.stdout).trim()).unwrap(),
        fs::canonicalize(&worktree).unwrap(),
        "a linked checkout must not be nested in another worktree"
    );
    let status = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(
        status.stdout.is_empty(),
        "primary worktree must stay untouched"
    );
}

#[cfg(unix)]
#[test]
fn stele_run_cursor_hides_the_synthetic_stop_loop() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("repo");
    fs::create_dir_all(&root).unwrap();
    sh(
        &root,
        "git init -q -b main && git config user.email t@t && git config user.name t",
    );
    fs::write(root.join("app.py"), "x = 1\n").unwrap();
    fs::write(root.join("stele.toml"), RULES_TOML).unwrap();
    sh(&root, "git add -A && git commit -qm init");

    let user_config = tmp.path().join("config/stele/stele.toml");
    let system_config = tmp.path().join("missing-system.toml");
    let state_home = tmp.path().join("state");
    let init = global_stele_command(&root, &user_config, &system_config)
        .args(["init", "--global"])
        .output()
        .unwrap();
    assert!(init.status.success());

    let bin = tmp.path().join("bin");
    fs::create_dir_all(&bin).unwrap();
    let cursor = bin.join("cursor-agent");
    fs::write(
        &cursor,
        r###"#!/bin/sh
case " $* " in
  *" --resume "*)
    printf '# Requirements\n\n## Functional\n\ndone\n\n## Risks\n\nnone\n' > requirements.md
    ;;
  *)
    printf '\n# changed by fake cursor\n' >> app.py
    ;;
esac
printf '{"session_id":"fake-session"}\n'
"###,
    )
    .unwrap();
    let mut permissions = fs::metadata(&cursor).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&cursor, permissions).unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let out = global_stele_command(&root, &user_config, &system_config)
        .args([
            "run",
            "--name",
            "cursor-dogfood",
            "cursor",
            "make the requested change",
        ])
        .env(stele::launch::STATE_HOME_ENV, &state_home)
        .env("PATH", path)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("synthetic block(s)"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let repo_state = fs::read_dir(state_home.join("worktrees"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let worktree = repo_state.join("cursor-dogfood");
    assert!(worktree.join("requirements.md").is_file());
    let git_dir = Command::new("git")
        .args(["rev-parse", "--absolute-git-dir"])
        .current_dir(&worktree)
        .output()
        .unwrap();
    let git_dir = PathBuf::from(String::from_utf8_lossy(&git_dir.stdout).trim());
    let events = fs::read_to_string(git_dir.join("stele/events.jsonl")).unwrap();
    assert!(events.contains("synthetic-stop"), "{events}");
    assert_eq!(fs::read_to_string(root.join("app.py")).unwrap(), "x = 1\n");
}

#[test]
fn system_user_and_repository_rules_accumulate() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("repo");
    fs::create_dir_all(&root).unwrap();
    sh(
        &root,
        "git init -q -b main && git config user.email t@t && git config user.name t",
    );
    fs::write(root.join("app.py"), "x = 1\n").unwrap();
    fs::write(
        root.join("stele.toml"),
        "[[rule]]\nid = \"repo-rule\"\ncheck = \"true\"\n",
    )
    .unwrap();
    sh(&root, "git add -A && git commit -qm init");
    fs::write(root.join("app.py"), "x = 2\n").unwrap();

    let user = tmp.path().join("user.toml");
    let system = tmp.path().join("system.toml");
    fs::write(
        &user,
        "[[rule]]\nid = \"user-rule\"\ntrigger = \"always\"\ncheck = \"true\"\n",
    )
    .unwrap();
    fs::write(
        &system,
        "[[rule]]\nid = \"system-rule\"\ntrigger = \"always\"\ncheck = \"true\"\n",
    )
    .unwrap();

    let out = global_stele_command(&root, &user, &system)
        .arg("check")
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("3 rule(s) measured"),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );

    // IDs are unique across all active layers; ambiguity fails measurement.
    fs::write(
        &user,
        "[[rule]]\nid = \"system-rule\"\ntrigger = \"always\"\ncheck = \"true\"\n",
    )
    .unwrap();
    let out = global_stele_command(&root, &user, &system)
        .arg("check")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&out.stderr).contains("duplicate rule id"));
}

// ----------------------------------------------------- merge attribution

#[test]
fn merge_in_progress_does_not_attribute_mainline_files() {
    let (_tmp, root) = fixture();
    // Feature branch does its own work.
    sh(&root, "git checkout -qb feature");
    fs::write(root.join("feature.py"), "feature = 1\n").unwrap();
    sh(&root, "git add -A && git commit -qm feature-work");
    // Mainline advances independently.
    sh(&root, "git checkout -q main");
    fs::write(root.join("mainline.py"), "mainline = 1\n").unwrap();
    sh(&root, "git add -A && git commit -qm mainline-work");
    // Back on feature, merge main WITHOUT committing: MERGE_HEAD exists and
    // the index holds the whole merged tree, including mainline.py.
    sh(
        &root,
        "git checkout -q feature && git merge --no-commit --no-ff main",
    );

    let sub = make_substrate(&root);
    assert!(
        sub.changed.iter().any(|f| f == "feature.py"),
        "own work must be attributed: {:?}",
        sub.changed
    );
    assert!(
        !sub.changed.iter().any(|f| f == "mainline.py"),
        "incoming mainline files must NOT be attributed mid-merge: {:?}",
        sub.changed
    );
}

#[test]
fn snapshot_signature_tracks_content_exactly() {
    let (_tmp, root) = fixture();
    fs::write(root.join("app.py"), "v2\n").unwrap();
    let sig_a = make_substrate(&root).signature;
    let sig_a2 = make_substrate(&root).signature;
    assert_eq!(sig_a, sig_a2, "same content, same signature");
    fs::write(root.join("app.py"), "v3\n").unwrap();
    assert_ne!(
        sig_a,
        make_substrate(&root).signature,
        "moved content, moved signature"
    );
}

// -------------------------------------------------------------------- ack

#[test]
fn ack_trailer_parsing() {
    let acked = stele::ack::parse("subject\n\nStele-Ack: requirements-doc\nStele-Ack: other-rule extra words\nNot-A-Trailer: x\n");
    assert!(acked.contains("requirements-doc"));
    assert!(acked.contains("other-rule"));
    assert_eq!(acked.len(), 2);
}

#[test]
fn acked_rule_stops_gating_everywhere() {
    let (_tmp, root) = fixture();
    // Acks live in base..HEAD commit trailers, so they're a branch-workflow
    // feature: on the mainline itself there is no base..HEAD range to carry them.
    sh(&root, "git checkout -qb feature");
    fs::write(root.join("app.py"), "v2\n").unwrap();
    sh(&root, "git add -A && git commit -qm wip");

    // Red without ack.
    let out = stele_command(&root).arg("check").output().unwrap();
    assert_eq!(out.status.code(), Some(1));

    // `stele ack` records the trailer...
    let out = stele_command(&root)
        .args(["ack", "requirements-doc", "-m", "docs ship separately"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    // ...check passes with an acknowledgement note...
    let out = stele_command(&root).arg("check").output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stdout).contains("acknowledged"));

    // ...and the stop hook no longer blocks.
    let (hook_out, _) = run_hook(&root, "claude-code", "stop", "{}");
    assert_eq!(hook_out.trim(), "");
}

#[test]
fn ack_commit_does_not_capture_staged_user_changes() {
    let (_tmp, root) = fixture();
    sh(&root, "git checkout -qb feature");
    fs::write(root.join("app.py"), "v2\n").unwrap();
    sh(&root, "git add app.py && git commit -qm work");
    fs::write(root.join("keep-staged.txt"), "user work\n").unwrap();
    sh(&root, "git add keep-staged.txt");

    let out = stele_command(&root)
        .args(["ack", "requirements-doc", "-m", "intentional"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let staged = Command::new("git")
        .args(["diff", "--cached", "--name-only"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&staged.stdout).trim(),
        "keep-staged.txt"
    );
    let committed = Command::new("git")
        .args(["show", "--pretty=format:", "--name-only", "HEAD"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&committed.stdout).trim().is_empty());
}

#[test]
fn ack_refuses_unknown_or_passing_rules() {
    let (_tmp, root) = fixture();
    let out = stele_command(&root)
        .args(["ack", "no-such-rule", "-m", "x"])
        .output()
        .unwrap();
    assert!(!out.status.success());

    // Rule exists but is not failing (no changes): refuse pre-emptive ack.
    let out = stele_command(&root)
        .args(["ack", "requirements-doc", "-m", "x"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("not currently failing"));
}

#[test]
fn trailer_cannot_bypass_non_acknowledgeable_rule() {
    let (_tmp, root) = fixture();
    fs::write(
        root.join("stele.toml"),
        r###"[[rule]]
id = "requirements-doc"
acknowledge = false

[rule.artifact]
path = "requirements.md"
sections = ["# Requirements"]
"###,
    )
    .unwrap();
    sh(
        &root,
        "git add stele.toml && git commit -qm config && git checkout -qb feature && git commit --allow-empty -qm $'attempted bypass\n\nStele-Ack: requirements-doc'",
    );
    fs::write(root.join("app.py"), "v2\n").unwrap();

    let verdict = engine::check(&config::load_repo(&root).unwrap(), &make_substrate(&root));
    assert!(!verdict.blocking().is_empty());
    assert!(verdict.acknowledged().is_empty());

    let payload = r#"{"tool_name":"terminal","tool_input":{"command":"touch app.py"}}"#;
    let (out, _) = run_hook(&root, "hermes", "pre_tool_call", payload);
    let response: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(response["action"], "block");
}

// ----------------------------------------------------- nudge + prompt gate

const NUDGE_RULES: &str = r###"
[[rule]]
id = "requirements-doc"
severity = "nudge"

[rule.artifact]
path = "requirements.md"
sections = ["# Requirements"]
"###;

#[test]
fn nudge_never_blocks_but_speaks_in_prompt_once() {
    let (_tmp, root) = fixture();
    fs::write(root.join("stele.toml"), NUDGE_RULES).unwrap();
    sh(&root, "git add -A && git commit -qm nudge-config");
    fs::write(root.join("app.py"), "v2\n").unwrap();

    // check: exit 0 with advisory output.
    let out = stele_command(&root).arg("check").output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stdout).contains("advisory"));

    // stop: never blocks.
    let (out, _) = run_hook(&root, "claude-code", "stop", "{}");
    assert_eq!(out.trim(), "", "nudges must not emit stop blocks");

    // prompt: speaks once per signature, then stays quiet.
    let (out, _) = run_hook(&root, "claude-code", "prompt", "{}");
    assert!(
        out.contains("requirements.md"),
        "first prompt injection carries the nudge"
    );
    let (out, _) = run_hook(&root, "claude-code", "prompt", "{}");
    assert_eq!(out.trim(), "", "second prompt injection is gated");
}

#[test]
fn blocking_prompt_context_also_speaks_once() {
    let (_tmp, root) = fixture();
    fs::write(root.join("app.py"), "v2\n").unwrap();
    let (out, _) = run_hook(&root, "claude-code", "prompt", "{}");
    assert!(out.contains("requirements.md"));
    let (out, _) = run_hook(&root, "claude-code", "prompt", "{}");
    assert_eq!(out.trim(), "");
}

// ------------------------------------------------------- no-verify guard

#[test]
fn no_verify_denied_while_red_allowed_when_green() {
    let (_tmp, root) = fixture();
    fs::write(root.join("app.py"), "v2\n").unwrap();

    let payload =
        r#"{"tool_name": "Bash", "tool_input": {"command": "git commit --no-verify -m wip"}}"#;
    let (out, _) = run_hook(&root, "claude-code", "pre-tool-use", payload);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["hookSpecificOutput"]["permissionDecision"], "deny");

    // Ordinary commands pass silently even while red.
    let payload = r#"{"tool_name": "Bash", "tool_input": {"command": "git commit -m wip"}}"#;
    let (out, _) = run_hook(&root, "claude-code", "pre-tool-use", payload);
    assert_eq!(out.trim(), "");

    // Green: --no-verify is harmless, no deny.
    fs::write(
        root.join("requirements.md"),
        "# Requirements\n## Functional\nx\n## Risks\ny\n",
    )
    .unwrap();
    let payload = r#"{"tool_name": "Bash", "tool_input": {"command": "git push --no-verify"}}"#;
    let (out, _) = run_hook(&root, "claude-code", "pre-tool-use", payload);
    assert_eq!(out.trim(), "");
}

// ------------------------------------------------- artifact schema depth

#[test]
fn exact_heading_match_rejects_lookalikes() {
    let (_tmp, root) = fixture();
    fs::write(root.join("app.py"), "v2\n").unwrap();
    fs::write(
        root.join("requirements.md"),
        "# Requirements\n## Functionality\nx\n## Risks\ny\n",
    )
    .unwrap();
    let rule = artifact_rule(vec![]);
    let result = rules::evaluate(&rule, &make_substrate(&root));
    assert!(
        result.red(),
        "`## Functionality` must not satisfy `## Functional`"
    );
}

#[test]
fn nonempty_sections_require_content() {
    let (_tmp, root) = fixture();
    fs::write(root.join("app.py"), "v2\n").unwrap();
    fs::write(
        root.join("requirements.md"),
        "# Requirements\ncontent\n## Functional\n\n## Risks\nreal risk\n",
    )
    .unwrap();
    let mut rule = artifact_rule(vec![]);
    rule.artifact.as_mut().unwrap().nonempty_sections = true;
    let result = rules::evaluate(&rule, &make_substrate(&root));
    assert!(result.red());
    assert!(
        result.findings.iter().any(|f| f.contains("empty")),
        "{:?}",
        result.findings
    );
}

// -------------------------------------------------- telemetry + doctor

#[test]
fn loop_guard_exits_are_logged() {
    let (_tmp, root) = fixture();
    fs::write(root.join("app.py"), "v2\n").unwrap();
    run_hook(
        &root,
        "claude-code",
        "stop",
        r#"{"stop_hook_active": true}"#,
    );
    let events = fs::read_to_string(root.join(".git/stele/events.jsonl")).unwrap_or_default();
    assert!(
        events.contains("loop-guard"),
        "guard exits must appear in telemetry: {events}"
    );
}

/// Several hooks can append at once (multiple harnesses, or a stop racing a
/// tool gate in one session). Every record must land whole: one line, one
/// parseable object, nothing shredded into its neighbour.
#[test]
fn concurrent_appends_never_interleave_records() {
    let tmp = TempDir::new().unwrap();
    let git_dir = tmp.path().join(".git");
    fs::create_dir_all(&git_dir).unwrap();

    const WRITERS: usize = 8;
    const PER_WRITER: usize = 40;
    let threads: Vec<_> = (0..WRITERS)
        .map(|w| {
            let git_dir = git_dir.clone();
            std::thread::spawn(move || {
                let state = stele::engine::State::at(git_dir);
                for i in 0..PER_WRITER {
                    state.log_event("claude-code", "stop", "green", &format!("w{w}-{i}"));
                }
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }

    let events = fs::read_to_string(git_dir.join("stele/events.jsonl")).unwrap();
    let lines: Vec<&str> = events.lines().collect();
    assert_eq!(
        lines.len(),
        WRITERS * PER_WRITER,
        "every record must be exactly one line"
    );
    for line in &lines {
        serde_json::from_str::<serde_json::Value>(line)
            .unwrap_or_else(|e| panic!("corrupt record: {e}\n{line}"));
    }
}

#[test]
fn iso_timestamps() {
    let ts = stele::engine::iso_now();
    // e.g. 2026-07-17T21:04:05Z
    assert_eq!(ts.len(), 20, "{ts}");
    assert!(
        ts.ends_with('Z') && ts.contains('T') && ts.starts_with("20"),
        "{ts}"
    );
}

#[test]
fn doctor_reports_wiring_state() {
    let (_tmp, root) = fixture();
    let rules = config::load_repo(&root).unwrap();
    stele::compile::run(&root, &rules).unwrap();
    let out = stele_command(&root).arg("doctor").output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("stele doctor"));
    assert!(text.contains("bash: "), "{text}"); // the runtime every check fires through
    assert!(text.contains(".claude/settings.json wired"), "{text}");
    assert!(text.contains("git pre-push hook"), "{text}");
}

#[test]
fn compile_wires_pre_tool_use_guard() {
    let (_tmp, root) = fixture();
    let rules = config::load_repo(&root).unwrap();
    stele::compile::run(&root, &rules).unwrap();
    let body = fs::read_to_string(root.join(".claude/settings.json")).unwrap();
    assert!(
        body.contains("stele hook claude-code pre-tool-use"),
        "{body}"
    );
}

#[test]
fn hermes_gatekeeper_blocks_the_call_that_would_create_the_red() {
    // CLEAN fixture: change-set empty, scoped evaluation wouldn't trigger —
    // but the gate must stop the first mutating call anyway (the one-step-
    // behind hole found by `stele conformance`).
    let (_tmp, root) = fixture();
    let base = format!(r#""cwd": "{}""#, root.display());

    // Mutating tool on a clean tree: blocked.
    let write =
        format!(r#"{{{base}, "tool_name": "write_file", "tool_input": {{"path": "app.py"}}}}"#);
    let (out, _) = run_hook(&root, "hermes", "pre_tool_call", &write);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(
        v["action"], "block",
        "first mutating call must be gated: {out}"
    );

    // Read-only tool: always allowed.
    let read =
        format!(r#"{{{base}, "tool_name": "read_file", "tool_input": {{"path": "app.py"}}}}"#);
    let (out, _) = run_hook(&root, "hermes", "pre_tool_call", &read);
    assert_eq!(out.trim(), "{}", "read-only tools pass: {out}");

    // Remediation: allowed.
    let fix = format!(
        r#"{{{base}, "tool_name": "write_file", "tool_input": {{"path": "requirements.md"}}}}"#
    );
    let (out, _) = run_hook(&root, "hermes", "pre_tool_call", &fix);
    assert_eq!(out.trim(), "{}", "artifact write passes: {out}");

    // Artifact satisfied: everything allowed again.
    fs::write(
        root.join("requirements.md"),
        "# Requirements\n## Functional\nx\n## Risks\ny\n",
    )
    .unwrap();
    let (out, _) = run_hook(&root, "hermes", "pre_tool_call", &write);
    assert_eq!(out.trim(), "{}", "green tree passes: {out}");
}

#[test]
fn hermes_gate_fails_open_with_an_explicit_allow_outside_a_repo() {
    // The shim is a bare `exec stele hook`, so stele must emit `{}` itself when
    // `cwd` is not a git worktree — Hermes treats empty stdout as undefined.
    let outside = TempDir::new().unwrap();
    let payload = format!(
        r#"{{"cwd": "{}", "tool_name": "write_file", "tool_input": {{"path": "app.py"}}}}"#,
        outside.path().display()
    );
    let (out, code) = run_hook(outside.path(), "hermes", "pre_tool_call", &payload);
    assert_eq!(
        out.trim(),
        "{}",
        "must allow explicitly outside a repo: {out}"
    );
    assert_eq!(code, 0, "fail-open exit: {code}");
}

#[test]
fn context_provider_injects_at_prompt_but_not_at_stop() {
    let (_tmp, root) = fixture();
    let mut cfg = fs::read_to_string(root.join("stele.toml")).unwrap();
    cfg.push_str(
        "\n[[context]]\nid = \"ctx\"\ncommand = \"printf 'CTX %s' \\\"$STELE_CHANGED\\\"\"\n",
    );
    fs::write(root.join("stele.toml"), cfg).unwrap();
    fs::write(root.join("app.py"), "x = 2\n").unwrap();

    let payload = format!(r#"{{"cwd":"{}"}}"#, root.display());
    // Prompt time: the provider runs and its stdout is injected, with the
    // change-set handed to it via $STELE_CHANGED.
    let (out, code) = run_hook(&root, "claude", "prompt", &payload);
    assert_eq!(code, 0);
    assert!(out.contains("CTX"), "context not injected: {out}");
    assert!(out.contains("app.py"), "$STELE_CHANGED not passed: {out}");

    // Stop time: context is a prompt-only channel — never injected here.
    let (out, _) = run_hook(&root, "claude", "stop", &payload);
    assert!(!out.contains("CTX"), "context must not fire at stop: {out}");
}

// ------------------------------------------------------------ ci generation

#[test]
fn generated_ci_self_hosts_in_stele_and_installs_stele_not_the_consumer_repo() {
    // A repo that IS the stele source builds from its own checkout.
    let (_tmp, root) = fixture();
    fs::write(root.join("Cargo.toml"), "[package]\nname = \"stele\"\n").unwrap();
    let rules = config::load_repo(&root).unwrap();
    stele::compile::run(&root, &rules).unwrap();
    let wf = fs::read_to_string(root.join(".github/workflows/stele.yml")).unwrap();
    assert!(wf.contains("cargo install --path . --locked"), "{wf}");

    // An ordinary repo installs Stele itself, never the consumer repository.
    let (_tmp2, root2) = fixture();
    sh(
        &root2,
        "git remote add origin git@github.com:acme/widgets.git",
    );
    let rules2 = config::load_repo(&root2).unwrap();
    stele::compile::run(&root2, &rules2).unwrap();
    let wf2 = fs::read_to_string(root2.join(".github/workflows/stele.yml")).unwrap();
    assert!(
        wf2.contains("https://github.com/project-kikkuli/stele"),
        "{wf2}"
    );
    assert!(!wf2.contains("acme/widgets"), "{wf2}");
}

// -------------------------------------------- this repo's own leak guard

/// `scripts/no-identifying-strings.sh` backs this repository's
/// `no-identifying-strings` rule. It is the one check whose failure mode is
/// silent: a broken script exits 0 and every leak sails through, so its
/// behavior is pinned here rather than trusted.
#[test]
fn leak_guard_catches_identifying_strings_and_stays_quiet_otherwise() {
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("scripts/no-identifying-strings.sh");
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    fs::create_dir_all(root.join(".stele")).unwrap();

    let run = |changed: &str| -> std::process::Output {
        Command::new("bash")
            .arg(&script)
            .current_dir(root)
            .env("STELE_ROOT", root)
            .env("STELE_CHANGED", changed)
            .output()
            .unwrap()
    };

    // Structural patterns ship in the script, so they hold with no private list.
    fs::write(root.join("clean.rs"), "fn main() {}\n").unwrap();
    assert!(run("clean.rs").status.success(), "clean tree must pass");

    fs::write(root.join("home.rs"), "// see /Users/someone/dev\n").unwrap(); // leak-guard-ok
    let out = run("home.rs");
    assert!(!out.status.success(), "absolute home path must be caught");
    assert!(String::from_utf8_lossy(&out.stdout).contains("/Users/someone")); // leak-guard-ok

    fs::write(root.join("mail.rs"), "// ping someone@example.ai\n").unwrap(); // leak-guard-ok
    assert!(
        !run("mail.rs").status.success(),
        "email address must be caught"
    );

    // Private names are only known through the gitignored list.
    fs::write(root.join("named.rs"), "//! ported from privatename\n").unwrap();
    assert!(
        run("named.rs").status.success(),
        "unknown name passes without a private list"
    );
    fs::write(
        root.join(".stele/private-patterns.txt"),
        "# comment\n\nprivatename\n",
    )
    .unwrap();
    let out = run("named.rs");
    assert!(!out.status.success(), "private list must be honored");
    assert!(String::from_utf8_lossy(&out.stdout).contains("privatename"));

    // The list is allowed to contain the strings it forbids.
    assert!(
        run(".stele/private-patterns.txt").status.success(),
        "the pattern file must not flag itself"
    );

    // Deleted paths stay in the change-set; binaries produce garbage findings.
    fs::write(root.join("blob.bin"), b"\x00\x01privatename\x00").unwrap();
    assert!(
        run("deleted.rs\nblob.bin").status.success(),
        "deleted and binary files must be skipped"
    );

    // A finding quotes the match, never the whole line — echoing the line back
    // would republish the secret into agent context and CI logs.
    fs::write(
        root.join("ctx.rs"),
        "// privatename appears beside other sensitive words here\n",
    )
    .unwrap();
    let stdout = String::from_utf8_lossy(&run("ctx.rs").stdout).into_owned();
    assert!(stdout.contains("privatename"));
    assert!(
        !stdout.contains("beside other sensitive words"),
        "finding must not echo the surrounding line: {stdout}"
    );
}

/// The escape hatches exist so the guard does not cry wolf on the repository it
/// protects: a marked line is exempt, and `git@host` is an SSH remote rather
/// than anybody's address.
#[test]
fn leak_guard_exempts_marked_lines_and_ssh_remotes() {
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("scripts/no-identifying-strings.sh");
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    let run = |changed: &str| -> std::process::Output {
        Command::new("bash")
            .arg(&script)
            .current_dir(root)
            .env("STELE_ROOT", root)
            .env("STELE_CHANGED", changed)
            .output()
            .unwrap()
    };

    fs::write(
        root.join("fixture.rs"),
        "let p = \"/Users/someone/dev\"; // leak-guard-ok\n",
    )
    .unwrap();
    assert!(
        run("fixture.rs").status.success(),
        "a marked line must be exempt"
    );

    fs::write(
        root.join("clone.md"),
        "git clone git@github.com:acme/widgets.git\n",
    )
    .unwrap();
    assert!(
        run("clone.md").status.success(),
        "an SSH remote is not an identity"
    );

    // The marker exempts its own line only, never the whole file.
    fs::write(
        root.join("mixed.rs"),
        "let a = \"/Users/one\"; // leak-guard-ok\nlet b = \"/Users/two\";\n",
    )
    .unwrap();
    let out = run("mixed.rs");
    assert!(!out.status.success(), "unmarked line must still be caught");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("/Users/two"), "{stdout}");
    assert!(!stdout.contains("/Users/one"), "{stdout}");
}
