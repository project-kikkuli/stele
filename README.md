# stele

*Unignorable edicts for your agents. A stele is the stone an empire carves its
laws into — you can't prompt-inject a rock.*

Declare a repository rule **once**; get it enforced across **every** AI coding
agent harness — Claude Code, Codex CLI, Cursor, Devin, Hermes — plus git hooks
and CI.

```toml
# stele.toml
[[rule]]
id = "requirements-doc"
description = "every change ships with an up-to-date requirements.md"
severity = "block"

[rule.artifact]
path = "requirements.md"
sections = ["# Requirements", "## Functional", "## Risks"]
```

```console
$ stele init      # write a starter stele.toml
$ stele compile   # fan it out to every channel
$ stele check     # measure the current change-set (0 green · 1 red · 3 unmeasurable)
```

## Why

Prompt-level rules (CLAUDE.md, AGENTS.md, .cursorrules) decay: compliance drops
~5.6%/function within a session, and no config restructuring fixes it
([arXiv 2605.10039](https://arxiv.org/abs/2605.10039)). The only thing that
composes is **measurement**: run the same check at every point in the agent's
lifecycle and speak in the one language agents reliably obey — failing checks.

stele is that check, compiled to every delivery channel a harness offers:

| Channel | Harness | Mechanism |
|---|---|---|
| native Stop hook | Claude Code, Codex, Devin CLI | `{"decision":"block"}` loops the agent until green |
| stop follow-up | Cursor IDE | `{"followup_message"}` auto-submits the findings |
| tool gatekeeper | Hermes | blocks tool calls while red; allows remediation |
| synthesized stop-loop | any resumable CLI (`stele wrap`) | measure at exit, `--resume` with findings |
| git pre-push | everything that pushes | fast local wall |
| CI (`stele check`) | everything | the unbypassable terminus |

Every layer runs the *same* check on the *same* measurement substrate. Local
layers fail open (never break a session) but log; CI fails loud — including on
"couldn't measure", which is not the same as green. Bypassing a local layer
only changes *when* you get corrected, never *whether*.

Every channel above is validated against real agents — event logs and findings
in [`conformance/RESULTS.md`](conformance/RESULTS.md).

## Rules

Two kinds:

- **artifact** — a file must exist with required sections (shown above).
- **command** — any script: exit 0 green; nonzero red with findings on stdout.
  Receives `STELE_ROOT`, `STELE_BASE`, `STELE_CHANGED` (newline-separated
  change-set) and must be a pure function of them.

`scope` globs (with `!` excludes) gate when a rule triggers; `severity =
"nudge"` speaks once but never blocks.

## Noise economics

Credibility is the whole game: a gate that nags gets disabled. The engine is
silent on green, speaks **once per change-signature**, caches green verdicts
(free silence until the change-set moves), and gives up after `STELE_MAX_BLOCKS`
(default 2) rather than looping an agent forever — the environment tier still
stands behind it. Every hook invocation is logged to `.git/stele/events.jsonl`,
so you can measure which layer catches what, per harness.

## Per-harness notes

- **Hermes** hooks are global-only: run `stele install hermes` once per user;
  the repo carries a self-scoping shim that no-ops outside stele repos.
- **Cursor headless** (`cursor-agent -p`) executes no hooks at all — use
  `stele wrap --prompt '<task>' -- cursor-agent -p --force`.
- **Cloud Devin**: install the git hooks in the machine snapshot (fast
  channel); a watcher can inject findings via the send-message API, but must
  poll session state rather than fire-and-forget.
