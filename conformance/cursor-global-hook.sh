#!/usr/bin/env bash
# User-level Cursor hooks apply to every session, so this shim self-scopes:
# it acts only when the session's workspace carries a .stele/ rig.
TRACE="${STELE_CURSOR_TRACE:-/tmp/stele-cursor-shim.log}"
echo "invoked $(date +%T) args=$* cwd=$(pwd)" >> "$TRACE"
payload=$(cat 2>/dev/null)
printf 'payload: %s\n' "$payload" >> "$TRACE"
root=$(printf '%s' "$payload" | python3 -c 'import json,sys
try:
    d = json.load(sys.stdin)
    print(d.get("cwd") or (d.get("workspace_roots") or [""])[0] or "")
except Exception: print("")' 2>/dev/null)
if [ -z "$root" ] || [ ! -x "$root/.stele/hook.sh" ]; then
  echo '{}'
  exit 0
fi
printf '%s' "$payload" | exec "$root/.stele/hook.sh" cursor "${1:-stop}"
