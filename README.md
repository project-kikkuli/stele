# stele

*Unignorable edicts for your agents. A stele is the stone an empire carves its
laws into — you can't prompt-inject a rock.*

Declare a repository rule **once**; get it enforced across **every** AI coding
agent harness — Claude Code, Codex CLI, Cursor, Devin, Hermes — plus git hooks
and CI.

## Install and try it

Until the first crates.io release, install from the repository:

```console
$ cargo install --git https://github.com/project-kikkuli/stele.git --locked
```

For a repository-owned policy:

```console
$ cd your-project
$ stele init
$ $EDITOR stele.toml
$ stele compile
$ stele doctor
```

Commit `stele.toml` and the generated agent/CI files. From then on the same
rule is checked in agent lifecycle hooks, pre-push, and CI.

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

## Personal and system rules

Personal rules apply in every git repository without adding files to those
repositories:

```console
$ stele install global      # create starter policy + wire every detected harness
$ stele run codex           # launch in a managed linked worktree
$ stele run cursor 'task'   # same, with Cursor's fallback loop selected for you
```

That single install command creates `~/.config/stele/stele.toml` when needed,
merges Stele into Claude Code, Codex, and Cursor's user hooks, and wires Hermes
when `~/.hermes/config.yaml` exists. Existing settings and third-party hooks
are preserved. Rerunning it is a no-op; installing Hermes later only requires
rerunning the same command.

The personal starter requires agent sessions to run inside a linked git
worktree. `stele run` creates a branch and worktree under
`~/.local/state/stele/worktrees`, launches the requested agent there, and
reuses the current checkout when it is already a linked worktree. It also
selects Cursor headless's synthesized stop-loop automatically. The low-level
`stele wrap` command remains available for custom resumable CLIs, but is not
part of the normal workflow.

To edit before enabling, `stele init --global` still writes only the personal
config. To turn dogfooding off without touching repositories:

```console
$ stele uninstall global          # remove Stele-owned hooks; keep personal rules
$ stele uninstall global --purge  # also remove the personal rule file
```

Uninstall removes only Stele-owned entries, including the Hermes shim, and
preserves every unrelated user hook. Runtime caches and telemetry live under
`.git/stele/`; they cannot be committed or pushed.

Machine or organization policy can live at `/etc/stele/stele.toml` (Windows:
`%PROGRAMDATA%\\stele\\stele.toml`). Provision that file together with managed
agent hooks and on CI runners. `STELE_USER_CONFIG` and `STELE_SYSTEM_CONFIG`
override both paths; `XDG_CONFIG_HOME` is honored for the personal config.

Rules accumulate in this order:

1. system
2. personal/user
3. repository

IDs must be unique across active layers. Global hooks evaluate only system and
personal rules; generated repository hooks evaluate only repository rules, so
installing both does not duplicate findings. Personal rules are never copied
into a repository's `AGENTS.md` or generated CI workflow.

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
| synthesized stop-loop | Cursor headless (`stele run cursor`) and custom resumable CLIs | measure at exit, `--resume` with findings |
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
"nudge"` speaks once but never blocks. `trigger = "always"` defines a session
precondition that also runs on clean trees. `acknowledge = false` makes an
invariant ineligible for `stele ack`.

## Noise economics

Credibility is the whole game: a gate that nags gets disabled. The engine is
silent on green, speaks **once per change-signature**, caches green verdicts
(free silence until the change-set moves), and gives up after `STELE_MAX_BLOCKS`
(default 2) rather than looping an agent forever — the environment tier still
stands behind it. Every hook invocation is logged to `.git/stele/events.jsonl`,
so you can measure which layer catches what, per harness.

## Per-harness notes

- **Hermes** hooks are global-only. `stele install global` installs a
  self-scoping shim that no-ops outside git repositories with active rules;
  Hermes may request first-use consent for the new shell hook.
- **Cursor headless** (`cursor-agent -p`) still emits no hook events in live
  testing. `stele run cursor '<task>'` hides the external resume-loop adapter.
- **Cloud Devin**: install the git hooks in the machine snapshot (fast
  channel); a watcher can inject findings via the send-message API, but must
  poll session state rather than fire-and-forget.
