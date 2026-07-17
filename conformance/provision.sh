#!/usr/bin/env bash
# provision.sh <harness> <target-dir> — build a fresh fixture repo wired for one harness.
set -euo pipefail
RIG="$(cd "$(dirname "$0")" && pwd)"
harness="$1"
dir="$2"

rm -rf "$dir"
mkdir -p "$dir/.stele"
cd "$dir"
git init -q

cat > app.py <<'EOF'
def add(a, b):
    return a + b
EOF

cp "$RIG/checker.sh" "$RIG/hook.sh" .stele/
chmod +x .stele/checker.sh .stele/hook.sh

case "$harness" in
  claude-code)
    mkdir -p .claude
    cat > .claude/settings.json <<'EOF'
{
  "hooks": {
    "Stop": [
      {
        "matcher": "",
        "hooks": [
          { "type": "command", "command": "\"$CLAUDE_PROJECT_DIR\"/.stele/hook.sh claude-code stop" }
        ]
      }
    ]
  }
}
EOF
    ;;
  codex)
    mkdir -p .codex
    cat > .codex/hooks.json <<'EOF'
{
  "hooks": {
    "Stop": [
      {
        "matcher": null,
        "hooks": [
          { "type": "command", "command": ".stele/hook.sh codex stop", "timeout": 30 }
        ]
      }
    ]
  }
}
EOF
    ;;
  hermes)
    : # wiring is global (~/.hermes/config.yaml) via hermes-global-hook.sh
    ;;
  cursor)
    mkdir -p .cursor
    cat > .cursor/hooks.json <<'EOF'
{
  "version": 1,
  "hooks": {
    "stop": [
      { "command": ".stele/hook.sh cursor stop", "timeout": 30 }
    ]
  }
}
EOF
    ;;
esac

git add -A
git commit -qm "fixture: initial state"
echo "$dir"
