#!/usr/bin/env bash
# stele-owned hook — self-scoping Hermes shim. Hermes hooks are global-only,
# so stele self-scopes: it reads `cwd` from the payload on stdin and stays
# silent (allow) unless that directory is a git worktree with active Stele
# rules. No JSON pre-parsing here — `stele hook` does it and emits the allow.
#
# Hermes reads empty stdout as undefined rather than allow, so when stele is
# not installed this shim must emit the allow itself.
command -v stele >/dev/null 2>&1 || { echo '{}'; exit 0; }
exec stele hook hermes pre_tool_call --scope all
