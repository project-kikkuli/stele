#!/usr/bin/env bash
# stele check: identifying strings must not enter a public repository.
#
# Two pattern sources, split by whether the pattern is itself safe to publish:
#
#   structural  — committed below. Shapes, not names: absolute home paths and
#                 email addresses. They identify a person or machine without
#                 naming anything private, so the list can live in the open.
#
#   private     — .stele/private-patterns.txt, gitignored. Names of internal
#                 codebases, services, orgs, ticket formats. A committed list of
#                 these would leak exactly what it exists to catch, so the file
#                 is never tracked and the rule degrades to structural-only
#                 without it (on a teammate's machine, in CI).
#
# Pure function of STELE_ROOT / STELE_CHANGED, per the command-rule contract.
# Scans the working-tree content of changed files, so it catches a string while
# it is still uncommitted — the only point where the fix is free.

set -uo pipefail

root="${STELE_ROOT:-$(git rev-parse --show-toplevel 2>/dev/null)}"
[ -n "$root" ] || exit 0
cd "$root" || exit 0

private_patterns=".stele/private-patterns.txt"

# A line carrying this marker is exempt. Test fixtures and documentation
# examples must be able to contain these shapes on purpose; without an escape
# hatch the rule flags its own test suite, and a gate that cries wolf on the
# repository it guards is a gate someone deletes.
ALLOW_MARKER="leak-guard-ok"

# One extended-regex alternation per line. Anchored loosely on purpose: the goal
# is to catch a paste, not to parse.
structural=$(
    cat <<'PATTERNS'
/Users/[a-zA-Z0-9._-]+
/home/[a-zA-Z0-9._-]+
[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}
PATTERNS
)

# `git@host` is the SSH remote convention, not somebody's address. It appears in
# every clone instruction ever written, so matching it teaches people to ignore
# this rule without protecting anything.
not_an_identity='^git@'

patterns="$structural"
if [ -f "$private_patterns" ]; then
    # Skip blanks and # comments so the private file can explain itself.
    extra=$(grep -vE '^\s*(#|$)' "$private_patterns" 2>/dev/null)
    [ -n "$extra" ] && patterns=$(printf '%s\n%s' "$patterns" "$extra")
fi

alternation=$(printf '%s' "$patterns" | paste -sd '|' -)
[ -n "$alternation" ] || exit 0

findings=""
while IFS= read -r file; do
    [ -n "$file" ] || continue
    # Deleted paths still appear in the change-set; nothing to scan.
    [ -f "$file" ] || continue
    # The private list is allowed to contain the strings it forbids.
    [ "$file" = "$private_patterns" ] && continue
    # Binary content produces unreadable findings and false matches.
    grep -Iq . "$file" 2>/dev/null || continue

    hits=$(grep -nEi "$alternation" "$file" 2>/dev/null | head -5)
    [ -n "$hits" ] || continue
    while IFS= read -r hit; do
        line="${hit%%:*}"
        text="${hit#*:}"
        case "$text" in *"$ALLOW_MARKER"*) continue ;; esac
        # Findings are read by agents and printed in CI logs, so quote the match
        # rather than the surrounding line: echoing the whole line back would
        # republish the very string being flagged.
        match=$(printf '%s' "$text" | grep -oEi "$alternation" |
            grep -vE "$not_an_identity" | head -1)
        [ -n "$match" ] || continue
        findings="${findings}✗ ${file}:${line} contains an identifying string: ${match}"$'\n'
    done <<<"$hits"
done <<<"${STELE_CHANGED:-}"

if [ -n "$findings" ]; then
    printf '%s' "$findings"
    exit 1
fi
exit 0
