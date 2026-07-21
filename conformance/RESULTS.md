# Conformance validation results

## 2026-07-20 — prompt-time context providers

Added a `[[context]]` provider channel: a command run at prompt/session-start
whose stdout is injected as agent context, independent of any rule's verdict
(runs even when everything is green; never gates, never at stop or in CI). The
command owns relevance and dedup via `$STELE_CHANGED` + marker files, so stele
injects its output verbatim and stays silent when it prints nothing. Ported from
the august/auth-plane intent-hook's prompt-mode plank-context injection, which
had no equivalent in stele's pass/fail rule model. Covered by
`context_provider_injects_at_prompt_but_not_at_stop`.

## 2026-07-17 — Hermes shim carries no runtime dependencies

The generated Hermes shim is now a bare `exec stele hook hermes pre_tool_call
--scope all`. Previously it pre-parsed the payload with `python3` to extract
`cwd` and `cd` into it — an undeclared runtime dependency on any machine running
Hermes. `stele hook` already resolves `cwd` from the payload (`resolve_root`) and
self-scopes, so the parse was redundant. To preserve the one behavior the shim
uniquely guaranteed — an explicit `{}` allow when the directory is not a git
worktree, since Hermes treats empty stdout as undefined — the hook's fail-open
exits now emit the per-harness allow themselves (`fail_open`). Covered by
`hermes_gate_fails_open_with_an_explicit_allow_outside_a_repo`.

## 2026-07-17 — reversible personal install and ergonomic launcher

Personal dogfooding now has one symmetric lifecycle:

- `stele install global` creates the personal worktree policy when absent and
  wires Claude Code, Codex, Cursor IDE, and detected Hermes in one pass.
- `stele uninstall global` removes only Stele-owned entries; `--purge` also
  removes the personal rule file.
- `stele run <agent>` creates or reuses a linked worktree. For Cursor headless,
  `stele run cursor '<task>'` selects the synthesized resume-loop internally.

Live Cursor result on `cursor-agent 2026.07.16-899851b`:

| harness | result | detail |
|---|---|---|
| cursor-run | PASS | managed linked worktree reused; repository rule corrected; synthetic stop fired; green ✓ |

A separate native-hook smoke run against that same Cursor build completed
successfully but emitted **zero** project-hook events. This reconfirms that the
external adapter is still necessary even though current Cursor documentation
describes CLI hook support. The normal command now hides that discrepancy.

The deterministic suite has 51 integration tests covering managed worktree
creation/reuse, automatic Cursor adapter selection, one-command bootstrap,
Hermes merge behavior, merge-safe uninstall, purge, idempotence, and rollback
when an existing user hook file is invalid.

## 2026-07-17 — layered personal/system rules and session preflight

Live sweep after adding system + personal + repository rule layers, scoped
global/repository hooks, `SessionStart`, and pre-mutation enforcement for
`trigger = "always"` rules:

| harness | result | detail |
|---|---|---|
| codex-global | PASS | no repo `stele.toml`; SessionStart context ✓ PreToolUse gate ✓ primary checkout untouched ✓ |
| codex | PASS | repository rule corrected; green ✓ artifact ✓ stop gate fired ✓ |
| cursor-wrap | PASS | repository rule corrected; synthetic stop fired ✓ |
| hermes | PASS | repository artifact gate corrected the task on retry ✓ |
| git-pre-push | PASS | red exit 1, green exit 0 |
| claude-code | NOT RUN | Claude exited before the task: account session limit until 16:30 America/New_York; no hook event fired |

The new `codex-global` conformance case uses an isolated `CODEX_HOME` with a
real user-level `hooks.json` and a personal config outside the fixture repo.
It asks Codex to edit from the primary checkout and passes only if the personal
policy fires while `app.py` remains byte-for-byte unchanged. The latest run
recorded both `context-injected` and `preflight-blocked`, validating the first
demo moment through the real harness, not just by invoking the hook CLI.

Hermes was nondeterministic across two runs: the first run recorded a real
tool block but the model abandoned the task without editing; the immediate
retry complied and passed. The channel worked in both runs, but agent recovery
after a block is not deterministic and should remain visible in the record.

The deterministic suite now has 46 integration tests, including primary vs.
linked-worktree behavior, preflight-before-wrapper-launch, global hook config
migration/idempotence, and accumulation of system + user + repository rules.

## 2026-07-17 — `stele conformance` (the built binary, all channels)

All five channels PASS end-to-end through the shipped `stele` binary
(`stele compile`-generated wiring, real agents, telemetry-verified gates):

| harness | result | detail |
|---|---|---|
| claude-code | PASS | green ✓ artifact ✓ gate fired ✓ |
| codex | PASS | green ✓ artifact ✓ gate fired ✓ |
| cursor-wrap | PASS | green ✓ artifact ✓ synthetic stop fired ✓ |
| hermes | PASS | green ✓ artifact ✓ gatekeeper fired ✓ |
| git-pre-push | PASS | red exit 1, green exit 0 |

Three real bugs the sweep itself caught (each now fixed + unit-tested):

1. **Env leak**: hooks honored `CLAUDE_PROJECT_DIR` for every harness, so
   codex/hermes hooks running nested inside a Claude session measured the
   WRONG repo. Now claude-code-only.
2. **Exhortation contaminated the experiment**: agents (codex, cursor) read
   the fixture's generated AGENTS.md and complied proactively, leaving the
   gate untested. Conformance fixtures now strip AGENTS.md to isolate the
   enforcement channel. (Silver lining: the prose layer demonstrably works.)
3. **The gatekeeper one-step-behind hole**: scope-triggered rules don't fire
   on a clean tree, so the FIRST mutating tool call — the one that creates
   the red — passed the hermes gate, and a one-write task escaped entirely.
   Gatekeeper now checks artifact rules unconditionally and gates only
   mutating tools (read-only sessions never harassed).

## 2026-07-16 — original bash-rig validation

Rule under test: `requirements.md` must exist at repo root with `# Requirements`,
`## Functional`, `## Risks`. Task given to each agent never mentions the file.
Instrumented hook logs every invocation to `.stele/events.jsonl`.

| Harness | Version | Channel | Loops agent? | Outcome |
|---|---|---|---|---|
| Claude Code | 2.1.186 | `Stop` hook, `{"decision":"block"}` | **Yes** — 1 block → compliant | ✅ green, exact schema |
| Codex CLI | 0.144.1 | `Stop` hook, `{"decision":"block"}` | **Yes** — 1 block → compliant | ✅ green, exact schema |
| Hermes | 0.14.0 | `pre_tool_call` gatekeeper, `{"action":"block"}` | **Yes** — 2 tool blocks → wrote artifact → unblocked | ✅ green, exact schema |
| Cursor CLI | 2026.07.13 | native hooks: **absent headless**; synthesized stop-loop via `wrap-cursor.sh` (measure at exit + `--resume <session_id>` with findings) | **Yes** — 1 synthetic block → compliant | ✅ green, exact schema |
| Devin (cloud) | SWE-1.7, 2026-07-16 | mid-session message injection (watcher channel) | **Yes** — complied with exact schema, then stopped | ✅ green |

## Key findings

1. **Claude Code + Codex are protocol-identical** for this purpose: same event
   name, same block JSON, `stop_hook_active`-style loop flag observed on retry.
   One adapter serves both with a config-path switch.
2. **Version drift is real and silent.** Hermes docs (main) describe `pre_verify`
   — the purpose-built "gate before the agent finishes" event with
   `{"action":"continue"}` loop semantics. Installed v0.14.0 does not have it;
   `post_llm_call` exists but is fire-and-forget (return value never consumed —
   confirmed in `run_agent.py`). Only a live conformance run catches this.
3. **The gatekeeper pattern works as a stop-loop substitute.** On Hermes 0.14.0,
   blocking all tool calls while non-compliant — except calls that touch the
   required artifact — forced remediation in 2 blocks: read_file ✗ → terminal ✗
   → write_file(requirements.md) allowed → all green after. The agent's final
   message even noted it created the file "only to satisfy the gatekeeper".
4. **Hermes hooks are global-only** (`~/.hermes/config.yaml`, consent allowlist,
   `HERMES_ACCEPT_HOOKS=1` for headless). The adapter must install a
   self-scoping shim (act only when cwd has a stele rig, else `{}` no-op).
   Also: hermes config already contains `hooks: {}` — naive append creates a
   duplicate YAML key that silently loses.
5. **Codex needs `--dangerously-bypass-hook-trust`** (or prior persisted trust
   in `config.toml [hooks.state]`) for repo-local hooks in headless runs.
6. Rig note: `MAX_BLOCKS=2` is right for stop-loops but too low for gatekeeper
   mode (each blocked tool call consumes one); make the cap per-mode.
7. **Cursor Agent CLI 2026.07.13 never invokes hooks in headless `-p` mode.**
   Tested project-level `.cursor/hooks.json`, then user-level
   `~/.cursor/hooks.json`, then a shim with unconditional invocation tracing:
   zero invocations across three runs while the agent completed its task and
   stopped. Cursor hooks are documented (and per practitioner reports work) in
   IDE sessions; the headless CLI has no hook channel today. Adapter tiering:
   Cursor IDE → hook tier; `cursor-agent` headless → environment tier
   (git hooks + CI) only. Untested nuance: interactive `cursor-agent` TUI mode.
8. **Synthesized stop-loop (validated on Cursor):** for any resumable headless
   CLI, an external wrapper — run, measure at process exit, `--resume
   <session_id>` with findings — is a full stop-hook substitute with
   seconds-level latency. `cursor-agent -p --output-format json` returns
   `session_id`; one synthetic block produced a compliant artifact. This
   generalizes: claude `-r`, `codex resume`, `hermes --resume` all support it,
   making it the universal fallback channel — no hook API required.
9. **Devin CLI adopted Claude Code's hook contract wholesale** (PreToolUse/
   PostToolUse/Stop/SessionStart/SessionEnd, `decision:block`, exit-2,
   `additionalContext`, `updatedInput`) via `.devin/hooks.v1.json` — and it
   also reads `.claude/settings.json` directly. The Devin CLI adapter is the
   Claude Code adapter. Cloud Devin sessions: no hooks documented; channel is
   snapshot-installed git hooks + API send-message (the synthesized loop,
   delivered async by a watcher).
10. **Cloud Devin message channel: validated live** (session 731273ad, org
   august-innovations, scratch task, no repo/PR). Devin received the exact
   stele block text mid-session and produced a schema-perfect requirements.md,
   then stopped ("I need to stop now as instructed"). Caveats: (a) a message
   sent right after Devin stops sat unprocessed for ~7 minutes; a second short
   imperative message preceded the work — the watcher must poll session state
   via the API and re-send/wake rather than fire-and-forget; (b) the session UI
   can go stale — poll the API, not the page; (c) latency is minutes, not
   seconds: acceptable for a cloud watcher, reinforcing snapshot-installed git
   hooks as Devin's fast in-session channel.

## Rig

- `checker.sh` — the rule as a pure function of a tree
- `hook.sh <harness> <event>` — instrumented block/allow emitter, per-harness protocol
- `hermes-global-hook.sh` — self-scoping global shim for Hermes
- `provision.sh <harness> <dir>` — fresh wired fixture repo

Reproduce: `provision.sh claude-code /tmp/f && cd /tmp/f && claude -p "<task>" --dangerously-skip-permissions`
