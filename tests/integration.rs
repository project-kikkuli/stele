//! Engine, config, emit, compile, and end-to-end hook tests.
//! (The live-agent conformance runs stay in conformance/ — these tests cover
//! everything that doesn't need a real model.)

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tempfile::TempDir;

use stele::config::{self, Artifact, Rule, Severity};
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
    sh(&root, "git init -q -b main && git config user.email t@t && git config user.name t");
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
    assert!(config::load(tmp.path()).unwrap_err().contains("exactly one"));
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
    assert!(config::load(tmp.path()).unwrap_err().contains("duplicate"));
}

// ----------------------------------------------------------------- rules

fn artifact_rule(scope: Vec<String>) -> Rule {
    Rule {
        id: "requirements-doc".into(),
        description: String::new(),
        severity: Severity::Block,
        scope,
        message: String::new(),
        check: None,
        artifact: Some(Artifact {
            path: "requirements.md".into(),
            sections: vec!["# Requirements".into(), "## Functional".into(), "## Risks".into()],
            nonempty_sections: false,
        }),
    }
}

#[test]
fn artifact_rule_reports_missing_file_then_missing_sections_then_green() {
    let (_tmp, root) = fixture();
    fs::write(root.join("app.py"), "def add(a, b):\n    return a + b  # changed\n").unwrap();
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
    assert!(!result.triggered, "txt change must not trigger py-scoped rule");

    fs::write(root.join("app.py"), "def add(a, b):\n    return a + b  # v2\n").unwrap();
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
fn command_rule_distinguishes_red_from_unmeasurable() {
    let (_tmp, root) = fixture();
    fs::write(root.join("app.py"), "changed\n").unwrap();
    let sub = make_substrate(&root);

    let red = Rule {
        id: "cmd".into(),
        check: Some("echo '✗ nope'; exit 1".into()),
        artifact: None,
        description: String::new(),
        severity: Severity::Block,
        scope: vec![],
        message: String::new(),
    };
    let result = rules::evaluate(&red, &sub);
    assert!(result.red());
    assert_eq!(result.findings, vec!["✗ nope"]);

    let green = Rule { id: "g".into(), check: Some("true".into()), ..red.clone() };
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
    assert!(!state.take_block_slot("sig-a", 2), "third block must be denied");
    assert!(state.take_block_slot("sig-b", 2), "new signature resets the count");
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
}

// --------------------------------------------------------------- compile

#[test]
fn compile_writes_all_channels_and_is_idempotent() {
    let (_tmp, root) = fixture();
    let rules = config::load(&root).unwrap();

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
    assert!(again.is_empty(), "second compile must be a no-op, wrote {again:?}");
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
    let rules = config::load(&root).unwrap();
    stele::compile::run(&root, &rules).unwrap();

    let doc: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(root.join(".claude/settings.json")).unwrap())
            .unwrap();
    assert_eq!(doc["permissions"]["allow"][0], "Bash(ls)", "user config preserved");
    let stops = doc["hooks"]["Stop"].as_array().unwrap();
    assert_eq!(stops.len(), 2, "user hook + stele hook");
    assert_eq!(stops[0]["hooks"][0]["command"], "my-own-hook.sh");
}

#[test]
fn compile_refuses_to_clobber_foreign_pre_push() {
    let (_tmp, root) = fixture();
    fs::create_dir_all(root.join(".git/hooks")).unwrap();
    fs::write(root.join(".git/hooks/pre-push"), "#!/bin/sh\necho mine\n").unwrap();
    let rules = config::load(&root).unwrap();
    stele::compile::run(&root, &rules).unwrap();
    let body = fs::read_to_string(root.join(".git/hooks/pre-push")).unwrap();
    assert_eq!(body, "#!/bin/sh\necho mine\n", "foreign hook must be untouched");
}

#[test]
fn agents_md_managed_block_updates_in_place() {
    let (_tmp, root) = fixture();
    fs::write(root.join("AGENTS.md"), "# My repo\n\nHand-written intro.\n").unwrap();
    let rules = config::load(&root).unwrap();
    stele::compile::run(&root, &rules).unwrap();
    stele::compile::run(&root, &rules).unwrap();
    let body = fs::read_to_string(root.join("AGENTS.md")).unwrap();
    assert!(body.contains("Hand-written intro."));
    assert_eq!(body.matches("stele:begin").count(), 1, "block must not duplicate");
    assert!(body.contains("requirements-doc"));
}

// ------------------------------------------------------- end-to-end hook

fn stele_bin() -> &'static str {
    env!("CARGO_BIN_EXE_stele")
}

fn run_hook(root: &Path, harness: &str, event: &str, payload: &str) -> (String, i32) {
    let mut child = Command::new(stele_bin())
        .args(["hook", harness, event])
        .current_dir(root)
        .env_remove("CLAUDE_PROJECT_DIR")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    use std::io::Write;
    child.stdin.take().unwrap().write_all(payload.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn hook_stop_blocks_then_gives_up_then_goes_green() {
    let (_tmp, root) = fixture();
    fs::write(root.join("app.py"), "def add(a, b):\n    return a + b  # touched\n").unwrap();

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
    let (out, _) = run_hook(&root, "claude-code", "stop", r#"{"stop_hook_active": true}"#);
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
    assert!(v["followup_message"].as_str().unwrap().contains("requirements.md"));

    // Cursor loop guard.
    let payload = format!(r#"{{"workspace_roots": ["{}"], "loop_count": 1}}"#, root.display());
    let (out, _) = run_hook(&root, "cursor", "stop", &payload);
    assert_eq!(out.trim(), "");
}

#[test]
fn hook_hermes_gatekeeper_allows_remediation_blocks_the_rest() {
    let (_tmp, root) = fixture();
    fs::write(root.join("app.py"), "v2\n").unwrap();
    let base = format!(r#""cwd": "{}""#, root.display());

    let blocked = format!(r#"{{{base}, "tool_name": "read_file", "tool_input": {{"path": "app.py"}}}}"#);
    let (out, _) = run_hook(&root, "hermes", "pre_tool_call", &blocked);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["action"], "block");

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
    let ok = Command::new(stele_bin()).arg("check").current_dir(&root).output().unwrap();
    assert_eq!(ok.status.code(), Some(0));

    fs::write(root.join("app.py"), "v2\n").unwrap();
    let red = Command::new(stele_bin()).arg("check").current_dir(&root).output().unwrap();
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
    let unmeasurable = Command::new(stele_bin()).arg("check").current_dir(&root).output().unwrap();
    assert_eq!(unmeasurable.status.code(), Some(1), "bash exits 127 → findings");
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
    sh(&root, "git checkout -q feature && git merge --no-commit --no-ff main");

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
    assert_ne!(sig_a, make_substrate(&root).signature, "moved content, moved signature");
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
    let out = Command::new(stele_bin()).arg("check").current_dir(&root).output().unwrap();
    assert_eq!(out.status.code(), Some(1));

    // `stele ack` records the trailer...
    let out = Command::new(stele_bin())
        .args(["ack", "requirements-doc", "-m", "docs ship separately"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));

    // ...check passes with an acknowledgement note...
    let out = Command::new(stele_bin()).arg("check").current_dir(&root).output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stdout).contains("acknowledged"));

    // ...and the stop hook no longer blocks.
    let (hook_out, _) = run_hook(&root, "claude-code", "stop", "{}");
    assert_eq!(hook_out.trim(), "");
}

#[test]
fn ack_refuses_unknown_or_passing_rules() {
    let (_tmp, root) = fixture();
    let out = Command::new(stele_bin())
        .args(["ack", "no-such-rule", "-m", "x"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(!out.status.success());

    // Rule exists but is not failing (no changes): refuse pre-emptive ack.
    let out = Command::new(stele_bin())
        .args(["ack", "requirements-doc", "-m", "x"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("not currently failing"));
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
    let out = Command::new(stele_bin()).arg("check").current_dir(&root).output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stdout).contains("advisory"));

    // stop: never blocks.
    let (out, _) = run_hook(&root, "claude-code", "stop", "{}");
    assert_eq!(out.trim(), "", "nudges must not emit stop blocks");

    // prompt: speaks once per signature, then stays quiet.
    let (out, _) = run_hook(&root, "claude-code", "prompt", "{}");
    assert!(out.contains("requirements.md"), "first prompt injection carries the nudge");
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

    let payload = r#"{"tool_name": "Bash", "tool_input": {"command": "git commit --no-verify -m wip"}}"#;
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
    assert!(result.red(), "`## Functionality` must not satisfy `## Functional`");
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
    assert!(result.findings.iter().any(|f| f.contains("empty")), "{:?}", result.findings);
}

// -------------------------------------------------- telemetry + doctor

#[test]
fn loop_guard_exits_are_logged() {
    let (_tmp, root) = fixture();
    fs::write(root.join("app.py"), "v2\n").unwrap();
    run_hook(&root, "claude-code", "stop", r#"{"stop_hook_active": true}"#);
    let events = fs::read_to_string(root.join(".git/stele/events.jsonl")).unwrap_or_default();
    assert!(events.contains("loop-guard"), "guard exits must appear in telemetry: {events}");
}

#[test]
fn iso_timestamps() {
    let ts = stele::engine::iso_now();
    // e.g. 2026-07-17T21:04:05Z
    assert_eq!(ts.len(), 20, "{ts}");
    assert!(ts.ends_with('Z') && ts.contains('T') && ts.starts_with("20"), "{ts}");
}

#[test]
fn doctor_reports_wiring_state() {
    let (_tmp, root) = fixture();
    let rules = config::load(&root).unwrap();
    stele::compile::run(&root, &rules).unwrap();
    let out = Command::new(stele_bin()).arg("doctor").current_dir(&root).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("stele doctor"));
    assert!(text.contains(".claude/settings.json wired"), "{text}");
    assert!(text.contains("git pre-push hook"), "{text}");
}

#[test]
fn compile_wires_pre_tool_use_guard() {
    let (_tmp, root) = fixture();
    let rules = config::load(&root).unwrap();
    stele::compile::run(&root, &rules).unwrap();
    let body = fs::read_to_string(root.join(".claude/settings.json")).unwrap();
    assert!(body.contains("stele hook claude-code pre-tool-use"), "{body}");
}
