#!/usr/bin/env bash
# Conformance rule under test: requirements.md must exist at repo root with
# a '# Requirements' title and '## Functional' + '## Risks' sections.
# Pure function of the tree at $1: prints findings, exit 1 when non-compliant.
ROOT="${1:-.}"
f="$ROOT/requirements.md"
findings=()
if [[ ! -f "$f" ]]; then
  findings+=("✗ requirements.md missing at repo root")
else
  grep -q '^# Requirements' "$f" || findings+=("✗ requirements.md: missing '# Requirements' title")
  grep -q '^## Functional' "$f" || findings+=("✗ requirements.md: missing '## Functional' section")
  grep -q '^## Risks' "$f" || findings+=("✗ requirements.md: missing '## Risks' section")
fi
if ((${#findings[@]})); then
  printf '%s\n' "${findings[@]}"
  exit 1
fi
exit 0
