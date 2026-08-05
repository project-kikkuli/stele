# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- `[rule.semantic]`: a third rule kind, judged by a model, for invariants no
  script can express ("no slop comments"). Its correctness lives in its prompt,
  so it ships with its own eval: `stele eval <rule-id>` runs the prompt across
  the configured `[[judge]]` fleet against held-out before→after corrections.
  Scoring the resulting edit rather than the flag credits a judge that flags a
  whole comment but rewrites it to the intended surgical cut. A rule may only
  enforce at the severity its weakest *measurable* vendor earns; a vendor with
  no gradeable output is a coverage gap (exit 3), not a zero. Judges are config,
  not hardcoded, so the fleet is never Claude-only.
- `binary` key in `stele.toml` (and a `STELE_BIN` override): the path generated
  hooks should invoke, for projects that ship stele inside their own toolchain
  (`node_modules/.bin/stele`, a vendored release) rather than depending on every
  teammate's `PATH`. Hook ownership is now tracked by the argument tail, so
  switching binaries rewrites the existing hooks rather than appending a second
  set, and hooks written by earlier versions migrate in place.

### Changed
- Every generated local channel — agent hooks, the Hermes shim, git pre-push —
  now tests for the stele binary and exits 0 (silent allow) when it is absent.
  Committed hooks no longer fail on the machine of a teammate who has not
  installed stele, which makes partial adoption survivable. CI is deliberately
  exempt and still fails loud. The Hermes shim emits its explicit `{}` allow in
  the not-installed case, since Hermes reads empty stdout as undefined.

### Fixed
- Telemetry records are written with a single `write(2)`. `writeln!` routed
  through `write_fmt`, and serde_json's `Display` serializes token by token, so
  one event became dozens of small appends — concurrent hooks interleaved
  mid-record and shredded each other's lines in `events.jsonl`.

### Added (earlier)
- `[[context]]` providers: a prompt-time channel distinct from rules. Each is a
  command whose stdout is injected as agent context at prompt/session-start,
  regardless of any rule's green/red. It never gates, never appears at stop or in
  CI; the command owns its own relevance and dedup (via `$STELE_CHANGED` and
  marker files). Fail-open — a malformed provider yields no context, never an error.

### Safety
- `stele install global` now refuses when run from inside an agent session
  (`CLAUDECODE`/`CURSOR_*`/`CODEX_*`), since global hooks activate immediately
  and would gate that very session; pass `--yes` to override. It also always
  prints the blast radius (every repo on the machine, including running agents).
- The shipped personal/system worktree starter is now a `nudge`, not a `block`:
  installing it reminds but can never freeze a running agent. Set `severity =
  "block"` to enforce (that's what `stele run` is for).

### Changed
- The generated repo pre-push hook now runs `stele check --scope repo`, so it
  gates only the repository's own rules. Personal/system rules gate through
  their own global hooks — a personal `block` rule no longer blocks pushes in
  every repository. `stele check` gains a `--scope` flag (default: all layers).
- Hermes shim is now a bare `exec stele hook` — stele reads `cwd` from the
  payload and self-scopes, dropping the shim's undeclared `python3` dependency.
- Hermes fail-open paths now emit an explicit `{}` allow, so the gate never sees
  empty stdout when a directory has no active Stele rules.

### Documentation
- Stated the POSIX-shell (`bash`) runtime requirement; scoped Windows support to
  WSL/Git Bash rather than implying native `cmd`/PowerShell.

### Packaging
- Added `keywords`, `categories`, `readme`, and `documentation` to `Cargo.toml`.
  The `stele` name on crates.io belongs to an unrelated crate, so `cargo install
  --git` stays the canonical install path.
- Added `CHANGELOG.md` and Dependabot for Cargo and GitHub Actions.

## [0.1.0]

Initial release: declare a rule once in `stele.toml`; enforce it across every AI
coding agent harness (Claude Code, Codex CLI, Cursor, Devin, Hermes) plus git
pre-push hooks and CI. Merge-aware measurement substrate, layered redundant
enforcement channels, acknowledgement trailers, and a live-harness conformance
rig.
