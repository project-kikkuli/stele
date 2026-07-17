#!/usr/bin/env bash
# stele-owned hook — self-scoping Hermes shim. Hermes hooks are global-only,
# so this acts only inside a git worktree with active Stele rules.
payload=$(cat 2>/dev/null)
cwd=$(printf '%s' "$payload" | python3 -c 'import json,sys
try: print(json.load(sys.stdin).get("cwd") or "")
except Exception: print("")' 2>/dev/null)
if [ -z "$cwd" ]; then
  echo '{}'
  exit 0
fi
cd "$cwd" 2>/dev/null || { echo '{}'; exit 0; }
printf '%s' "$payload" | exec stele hook hermes pre_tool_call --scope all
