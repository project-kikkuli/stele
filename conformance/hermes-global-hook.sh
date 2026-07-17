#!/usr/bin/env bash
# Hermes hooks are global-only (~/.hermes/config.yaml), so this shim self-scopes:
# it acts only when the session's cwd carries a .stele/ rig, else silent no-op.
payload=$(cat 2>/dev/null)
cwd=$(printf '%s' "$payload" | python3 -c 'import json,sys
try: print(json.load(sys.stdin).get("cwd") or "")
except Exception: print("")' 2>/dev/null)
if [ -z "$cwd" ] || [ ! -x "$cwd/.stele/hook.sh" ]; then
  echo '{}'
  exit 0
fi
printf '%s' "$payload" | exec "$cwd/.stele/hook.sh" hermes "${1:-pre_verify}"
