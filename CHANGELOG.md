# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed
- Hermes shim is now a bare `exec stele hook` — stele reads `cwd` from the
  payload and self-scopes, dropping the shim's undeclared `python3` dependency.
- Hermes fail-open paths now emit an explicit `{}` allow, so the gate never sees
  empty stdout when a directory has no active Stele rules.

### Documentation
- Stated the POSIX-shell (`bash`) runtime requirement; scoped Windows support to
  WSL/Git Bash rather than implying native `cmd`/PowerShell.

### Packaging
- Added `keywords`, `categories`, `readme`, and `documentation` to `Cargo.toml`
  ahead of the first crates.io release.
- Added `CHANGELOG.md` and Dependabot for Cargo and GitHub Actions.

## [0.1.0]

Initial release: declare a rule once in `stele.toml`; enforce it across every AI
coding agent harness (Claude Code, Codex CLI, Cursor, Devin, Hermes) plus git
pre-push hooks and CI. Merge-aware measurement substrate, layered redundant
enforcement channels, acknowledgement trailers, and a live-harness conformance
rig.
