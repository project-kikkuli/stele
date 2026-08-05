# stele

*Unignorable edicts for your agents. A stele is the stone an empire carves its
laws into — you can't prompt-inject a rock.*

Declare a repository rule **once**; get it enforced across **every** AI coding
agent harness — Claude Code, Codex CLI, Cursor, Devin, Hermes — plus git hooks
and CI.

## Install and try it

Install from the repository (the crates.io name is taken by an unrelated crate,
so the git install is the canonical path):

```console
$ cargo install --git https://github.com/project-kikkuli/stele.git --locked
```

Stele runs rule checks and its generated hooks through a POSIX shell (`bash`),
so Linux and macOS are first-class. On Windows, run stele under WSL or Git Bash;
native `cmd`/PowerShell is not yet supported (the system-policy path below still
resolves, but checks need `bash`).

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

Global hooks take effect immediately in **every** git repository on this
machine, including agent sessions already running. So `install global` refuses
when it detects it's being run from inside an agent session (pass `--yes` to
override), and always prints that blast radius — restart any active agents
afterward so they pick up the change cleanly.

The personal starter *suggests* running agent sessions inside a linked git
worktree, shipping as a `nudge`: it reminds when a session runs in a primary
checkout but never blocks. Change its `severity` to `block` to enforce it — then
agents must be launched with `stele run`, which creates a branch and worktree
under `~/.local/state/stele/worktrees`, launches the requested agent there, and
reuses the current checkout when it is already a linked worktree. `stele run`
also selects Cursor headless's synthesized stop-loop automatically. The
low-level `stele wrap` command remains available for custom resumable CLIs, but
is not part of the normal workflow.

To edit before enabling, `stele init --global` still writes only the personal
config. To turn dogfooding off without touching repositories:

```console
$ stele uninstall global          # remove Stele-owned hooks; keep personal rules
$ stele uninstall global --purge  # also remove the personal rule file
```

Uninstall removes only Stele-owned entries, including the Hermes shim, and
preserves every unrelated user hook. Runtime caches and telemetry live under
`.git/stele/`; they cannot be committed or pushed.

Machine or organization policy can live at `/etc/stele/stele.toml` (under WSL/Git
Bash on Windows, `%PROGRAMDATA%\\stele\\stele.toml`). Provision that file together
with managed agent hooks and on CI runners. `STELE_USER_CONFIG` and `STELE_SYSTEM_CONFIG`
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

Three kinds:

- **artifact** — a file must exist with required sections (shown above).
- **command** — any script: exit 0 green; nonzero red with findings on stdout.
  Receives `STELE_ROOT`, `STELE_BASE`, `STELE_CHANGED` (newline-separated
  change-set) and must be a pure function of them.
- **semantic** — judged by a model, for the rules no script can express ("no
  slop comments"). Carries its own eval; see below.

`scope` globs (with `!` excludes) gate when a rule triggers; `severity =
"nudge"` speaks once but never blocks. `trigger = "always"` defines a session
precondition that also runs on clean trees. `acknowledge = false` makes an
invariant ineligible for `stele ack`.

## Semantic rules and `stele eval`

A deterministic rule is exact and needs no eval. A semantic rule's correctness
lives in its **prompt** — which can be phrased wrong and still look fine by
inspection. So a semantic rule ships with the evidence that its prompt works:

```toml
[[judge]]
name = "claude"
command = "claude -p"        # prompt on stdin, verdict on stdout

[[rule]]
id = "no-slop-comments"
severity = "nudge"

[rule.semantic]
prompt = "Remove comments that restate the code."
cases = "evals/slop.jsonl"   # held-out before→after corrections
models = ["claude"]
samples = 3                  # votes per (model, case); majority wins
```

```console
$ stele eval no-slop-comments
```

Judges are config, not hardcoded — the fleet is whatever `[[judge]]` lists, so
this is never Claude-only. Cases are before→after: the judge rewrites the code
and passes when every `removed` fragment is gone and every kept fragment
survives. Scoring the **edit** rather than the flag credits a judge that flags a
whole comment but rewrites it to the intended surgical cut.

A rule may only enforce at the severity its weakest *measurable* vendor earns.
A vendor that produces no gradeable output is a coverage gap, not a zero — exit
3, the same "couldn't measure" that CI already refuses to read as green.

Exit codes: `0` proven at the declared severity, `1` doesn't hold, `3` fleet not
fully measurable.

## Shipping stele to a team

Generated hooks get committed; the binary does not. So every generated local
channel first tests for stele and exits 0 (silent allow) when it is missing — a
teammate who has not installed it gets silence, not a hook failure on every
event. CI is deliberately exempt: there, a missing stele fails loud.

That makes adoption incremental. Commit the wiring, let people install stele
when they choose, and CI holds the line for everyone either way.

To ship stele inside the project's own toolchain rather than depending on each
teammate's `PATH`, name it in `stele.toml`:

```toml
binary = "node_modules/.bin/stele"
```

Every generated channel then calls that path. `STELE_BIN` overrides it for a
single `stele compile` run. Hook ownership is tracked by the argument tail, so
switching binaries rewrites the existing hooks instead of appending a second
set — and hooks written by earlier versions migrate in place.

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
