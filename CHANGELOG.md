# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
