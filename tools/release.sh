#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
# Releases (ADR 0014), in two steps so the release commit can be reviewed
# before anything is published:
#
#   tools/release.sh prepare X.Y.Z   bump Cargo.toml and Cargo.lock, move the
#                                    Unreleased changelog entries under
#                                    "## [X.Y.Z] - today", update the compare
#                                    links, and commit. Nothing is pushed.
#   tools/release.sh publish         tag the version in Cargo.toml and push
#                                    main and the tag, which runs the release
#                                    workflow. Asks first; -y skips the prompt.
set -eu

cd "$(dirname "$0")/.."
REPO_URL=https://github.com/predakanga/kindle-wireless-sync

die() { echo "release: $*" >&2; exit 1; }

current_version() {
    sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -1
}

require_clean_main() {
    [ "$(git rev-parse --abbrev-ref HEAD)" = main ] || die "not on main"
    git diff --quiet && git diff --cached --quiet || die "uncommitted changes"
}

# Succeeds if $1 is a higher X.Y.Z than $2.
newer() {
    awk -v a="$1" -v b="$2" 'BEGIN {
        split(a, x, "."); split(b, y, ".")
        for (i = 1; i <= 3; i++) {
            if (x[i] + 0 > y[i] + 0) exit 0
            if (x[i] + 0 < y[i] + 0) exit 1
        }
        exit 1
    }'
}

prepare() {
    version=$1
    echo "$version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$' || die "version must be X.Y.Z"
    require_clean_main
    old=$(current_version)
    newer "$version" "$old" || die "$version is not newer than $old"
    git rev-parse -q --verify "refs/tags/v$version" >/dev/null && die "tag v$version already exists"
    tools/changelog-notes.sh Unreleased >/dev/null 2>&1 ||
        die "CHANGELOG.md has nothing under [Unreleased]"
    grep -q '^\[Unreleased\]: ' CHANGELOG.md || die "CHANGELOG.md has no [Unreleased] link"
    today=$(date +%Y-%m-%d)

    # The first `version =` line is the package's own.
    awk -v old="$old" -v new="$version" '
        !done && $0 == "version = \"" old "\"" { print "version = \"" new "\""; done = 1; next }
        { print }
    ' Cargo.toml > Cargo.toml.new && mv Cargo.toml.new Cargo.toml
    cargo update --workspace --offline --quiet

    awk -v v="$version" -v old="$old" -v today="$today" -v url="$REPO_URL" '
        $0 == "## [Unreleased]" { print; print ""; print "## [" v "] - " today; next }
        index($0, "[Unreleased]: ") == 1 {
            print "[Unreleased]: " url "/compare/v" v "...HEAD"
            print "[" v "]: " url "/compare/v" old "...v" v
            next
        }
        { print }
    ' CHANGELOG.md > CHANGELOG.md.new && mv CHANGELOG.md.new CHANGELOG.md

    [ "$(current_version)" = "$version" ] || die "Cargo.toml was not updated"
    tools/changelog-notes.sh "$version" >/dev/null
    git commit -q -m "Release v$version" Cargo.toml Cargo.lock CHANGELOG.md
    git show --stat HEAD
    echo
    echo "Prepared v$version. Review it, then run \`just publish-release\`."
}

publish() {
    require_clean_main
    version=$(current_version)
    tag=v$version
    git rev-parse -q --verify "refs/tags/$tag" >/dev/null && die "tag $tag already exists"
    tools/changelog-notes.sh "$version" >/dev/null
    if [ "${1:-}" != -y ]; then
        printf 'Tag %s at %s and push main and the tag to origin? [y/N] ' \
            "$tag" "$(git log -1 --format='%h %s')"
        read -r answer
        [ "$answer" = y ] || [ "$answer" = Y ] || die "not published"
    fi
    git tag -a "$tag" -m "$tag"
    # Atomic: if either push is refused, neither happens.
    git push --atomic origin main "$tag" || {
        git tag -d "$tag" >/dev/null
        die "push failed; removed the local tag"
    }
    echo "Pushed $tag. The release workflow publishes it: $REPO_URL/actions"
}

case "${1:-}" in
    prepare) [ $# -eq 2 ] || die "usage: $0 prepare X.Y.Z"; prepare "$2" ;;
    publish) publish "${2:-}" ;;
    *) die "usage: $0 prepare X.Y.Z | publish [-y]" ;;
esac
