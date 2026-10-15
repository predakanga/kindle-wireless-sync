#!/bin/sh
# Prints the body of one version's section of CHANGELOG.md, for release notes
# (ADR 0014). Fails if the section is missing or empty, so a release can't
# go out without a changelog entry.
#
# Usage: tools/changelog-notes.sh VERSION
set -eu

version=$1
notes=$(awk -v want="## [$version]" '
    index($0, "## [") == 1 { if (found) exit; found = (index($0, want) == 1); next }
    /^\[[^]]+\]: / { next }
    found { print }
' CHANGELOG.md | sed -e '/./,$!d')
if [ -z "$(printf '%s' "$notes" | tr -d '[:space:]')" ]; then
    echo "CHANGELOG.md has no entry for $version" >&2
    exit 1
fi
printf '%s\n' "$notes"
