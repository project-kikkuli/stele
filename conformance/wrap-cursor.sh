#!/usr/bin/env bash
# Synthesized stop-hook for harnesses with no native hook channel:
# run headless, measure at process exit, re-inject findings via --resume.
# Usage: wrap-cursor.sh <fixture-dir> <task prompt>
set -uo pipefail
dir="$1"; task="$2"
MAX_LOOPS="${STELE_MAX_LOOPS:-2}"
cd "$dir"

json=$(cursor-agent -p --force --output-format json "$task" 2>/dev/null)
sid=$(printf '%s' "$json" | python3 -c 'import json,sys
try: print(json.load(sys.stdin).get("session_id") or "")
except Exception: print("")')
echo "run 1 session=$sid"

for i in $(seq 1 "$MAX_LOOPS"); do
  findings=$(bash .stele/checker.sh . 2>/dev/null) && { echo "verdict: GREEN after $((i-1)) synthetic block(s)"; exit 0; }
  python3 - "$i" "$findings" .stele/events.jsonl <<'EOF'
import json, sys, datetime
i, findings, path = sys.argv[1:4]
rec = {"ts": datetime.datetime.now().isoformat(timespec="seconds"),
       "harness": "cursor-wrap", "event": f"synthetic-stop-{i}",
       "verdict": "blocked", "detail": findings, "payload": {}}
open(path, "a").write(json.dumps(rec) + "\n")
EOF
  [ -z "$sid" ] && { echo "no session_id; cannot resume"; exit 1; }
  msg="stele: this change-set is missing a required artifact.
$findings
Before finishing, create requirements.md at the repo root with exactly these sections: a '# Requirements' title, a '## Functional' section describing what the change does, and a '## Risks' section. Then finish."
  json=$(cursor-agent -p --force --output-format json --resume "$sid" "$msg" 2>/dev/null)
  echo "resume $i done"
done

bash .stele/checker.sh . >/dev/null 2>&1 && echo "verdict: GREEN after $MAX_LOOPS synthetic block(s)" || echo "verdict: STILL RED after $MAX_LOOPS synthetic blocks"
