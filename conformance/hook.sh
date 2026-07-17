#!/usr/bin/env bash
# Instrumented conformance hook: hook.sh <harness> <event>
# Logs every invocation (timestamp, harness, event, raw payload, verdict) to
# .stele/events.jsonl, runs the rule checker, and emits the harness's native
# block/continue response while non-compliant. Blocks at most MAX_BLOCKS times
# so a non-complying agent terminates rather than looping forever.
harness="${1:-unknown}"
event="${2:-unknown}"
MAX_BLOCKS="${STELE_MAX_BLOCKS:-2}"
payload=$(cat 2>/dev/null)

ROOT=$(pwd)
# Hermes and Cursor run hooks from their own process; trust the payload's
# cwd / workspace root there rather than the hook's inherited cwd.
if [ "$harness" = "hermes" ] || [ "$harness" = "cursor" ]; then
  c=$(printf '%s' "$payload" | python3 -c 'import json,sys
try:
    d = json.load(sys.stdin)
    print(d.get("cwd") or (d.get("workspace_roots") or [""])[0] or "")
except Exception: print("")' 2>/dev/null)
  [ -n "$c" ] && ROOT="$c"
fi
STELE="$ROOT/.stele"
mkdir -p "$STELE"

log() { # $1=verdict $2=detail
  python3 - "$harness" "$event" "$1" "$2" "$STELE/events.jsonl" <<'EOF' 2>/dev/null
import json, sys, os, datetime
harness, event, verdict, detail, path = sys.argv[1:6]
payload = os.environ.get("STELE_PAYLOAD", "")
try: payload = json.loads(payload)
except Exception: pass
rec = {"ts": datetime.datetime.now().isoformat(timespec="seconds"),
       "harness": harness, "event": event, "verdict": verdict,
       "detail": detail, "payload": payload}
with open(path, "a") as f: f.write(json.dumps(rec) + "\n")
EOF
}
export STELE_PAYLOAD="$payload"

findings=$(bash "$STELE/checker.sh" "$ROOT" 2>/dev/null)
if [ -z "$findings" ]; then
  log green ""
  [ "$harness" = "hermes" ] && echo '{}'
  exit 0
fi

# Gatekeeper mode (hermes 0.14.0 has no stop-loop channel): block tool calls
# while non-compliant, but let through anything that works on requirements.md
# itself — otherwise the agent could never comply.
if [ "$harness" = "hermes" ] && [ "$event" = "pre_tool_call" ]; then
  if printf '%s' "$payload" | grep -q "requirements\.md"; then
    log allowed-remediation ""
    echo '{}'
    exit 0
  fi
  log blocked "$findings"
  printf 'stele gatekeeper: %s\nCreate requirements.md at the repo root first (sections: # Requirements, ## Functional, ## Risks). Then retry this tool call.' "$findings" \
    | python3 -c 'import json,sys
print(json.dumps({"action": "block", "message": sys.stdin.read()}))'
  exit 0
fi

count=$(cat "$STELE/blocks" 2>/dev/null || echo 0)
case "$count" in *[!0-9]*) count=0 ;; esac
if [ "$count" -ge "$MAX_BLOCKS" ]; then
  log gave-up "$findings"
  [ "$harness" = "hermes" ] && echo '{}'
  exit 0
fi
echo $((count + 1)) > "$STELE/blocks"
log blocked "$findings"

REASON="stele: this change-set is missing a required artifact.
$findings
Before finishing, create requirements.md at the repo root with exactly these sections: a '# Requirements' title, a '## Functional' section describing what the change does, and a '## Risks' section. Then finish."

case "$harness" in
  claude-code | codex)
    printf '%s' "$REASON" | python3 -c 'import json,sys
print(json.dumps({"decision": "block", "reason": sys.stdin.read()}))' ;;
  hermes)
    printf '%s' "$REASON" | python3 -c 'import json,sys
print(json.dumps({"action": "continue", "message": sys.stdin.read()}))' ;;
  cursor)
    printf '%s' "$REASON" | python3 -c 'import json,sys
print(json.dumps({"followup_message": sys.stdin.read()}))' ;;
  *)
    printf '%s\n' "$REASON" ;;
esac
exit 0
