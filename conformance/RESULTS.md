# Conformance validation results — 2026-07-16

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
